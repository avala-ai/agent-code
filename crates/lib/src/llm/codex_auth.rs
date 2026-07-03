//! Codex ChatGPT authentication support.
//!
//! Two entry points, both keyed on `<codex_home>/auth.json` (default `~/.codex`)
//! so agent-code and the `codex` CLI share one subscription session:
//!
//! - [`CodexChatGptAuth`] loads and refreshes an existing session for use as a
//!   provider (the `codex_chatgpt` auth mode).
//! - [`browser_login`] runs the "Sign in with ChatGPT" browser OAuth flow (PKCE)
//!   and writes that session file, so no `codex` CLI install is required.
//!
//! Tokens are only ever stored in `auth.json`, never in agent-code config.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use super::provider::ProviderError;
use crate::services::oauth::{
    CredentialStore, OAuthError, OAuthProviderConfig, OAuthService, TokenSet,
};

const DEFAULT_CODEX_HOME: &str = ".codex";
const CHATGPT_ACCOUNT_ID_HEADER: &str = "ChatGPT-Account-ID";
const FEDRAMP_HEADER: &str = "X-OpenAI-Fedramp";
const TOKEN_REFRESH_INTERVAL_DAYS: i64 = 8;
const REFRESH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug, Clone)]
pub struct CodexChatGptAuth {
    auth_file: PathBuf,
    http: reqwest::Client,
    state: Arc<Mutex<CodexAuthState>>,
}

#[derive(Clone)]
struct CodexAuthState {
    raw: Value,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
    last_refresh: Option<DateTime<Utc>>,
    is_fedramp_account: bool,
}

// Manual Debug: never print the tokens or `raw` (the full auth.json), so a
// stray `{:?}` in a log or error cannot leak the ChatGPT session.
impl std::fmt::Debug for CodexAuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAuthState")
            .field("access_token", &"***")
            .field("refresh_token", &"***")
            .field("account_id", &self.account_id)
            .field("last_refresh", &self.last_refresh)
            .field("is_fedramp_account", &self.is_fedramp_account)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

impl std::fmt::Debug for RefreshResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshResponse")
            .field("id_token", &self.id_token.as_ref().map(|_| "***"))
            .field("access_token", &self.access_token.as_ref().map(|_| "***"))
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "***"))
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct JwtStandardClaims {
    exp: Option<i64>,
}

impl CodexChatGptAuth {
    pub fn load(codex_home: Option<&str>) -> Result<Self, ProviderError> {
        let codex_home = match codex_home {
            Some(path) => PathBuf::from(path),
            None => default_codex_home().ok_or_else(|| {
                ProviderError::Auth(
                    "could not determine Codex home; set CODEX_HOME or api.codex_home".into(),
                )
            })?,
        };
        Self::load_from_auth_file(codex_home.join("auth.json"))
    }

    pub fn load_from_auth_file(auth_file: PathBuf) -> Result<Self, ProviderError> {
        let state = CodexAuthState::load_from_file(&auth_file)?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        Ok(Self {
            auth_file,
            http,
            state: Arc::new(Mutex::new(state)),
        })
    }

    pub async fn auth_headers(&self) -> Result<HeaderMap, ProviderError> {
        let mut state = self.state.lock().await;
        if state.needs_refresh() {
            let refresh = self.refresh_token(&state.refresh_token).await?;
            state.apply_refresh(refresh)?;
            state.save_to_file(&self.auth_file)?;
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", state.access_token))
                .map_err(|e| ProviderError::Auth(e.to_string()))?,
        );
        if let Some(account_id) = state.account_id.as_ref() {
            headers.insert(
                CHATGPT_ACCOUNT_ID_HEADER,
                HeaderValue::from_str(account_id)
                    .map_err(|e| ProviderError::Auth(e.to_string()))?,
            );
        }
        if state.is_fedramp_account {
            headers.insert(FEDRAMP_HEADER, HeaderValue::from_static("true"));
        }
        Ok(headers)
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<RefreshResponse, ProviderError> {
        let endpoint = std::env::var(REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR)
            .unwrap_or_else(|_| REFRESH_TOKEN_URL.to_string());
        let response = self
            .http
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "client_id": CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
            }))
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let status = response.status();
        if status.is_success() {
            return response
                .json::<RefreshResponse>()
                .await
                .map_err(|e| ProviderError::InvalidResponse(e.to_string()));
        }

        let body = response.text().await.unwrap_or_default();
        Err(ProviderError::Auth(format!(
            "Codex ChatGPT token refresh failed ({status}): {}",
            refresh_error_message(&body)
        )))
    }
}

impl CodexAuthState {
    fn load_from_file(path: &Path) -> Result<Self, ProviderError> {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            ProviderError::Auth(format!(
                "Codex ChatGPT auth not found at {}: {e}. Run `codex login` first.",
                path.display()
            ))
        })?;
        let raw: Value =
            serde_json::from_str(&contents).map_err(|e| ProviderError::Auth(e.to_string()))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: Value) -> Result<Self, ProviderError> {
        let tokens = raw
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                ProviderError::Auth(
                    "Codex auth.json does not contain ChatGPT tokens; run `codex login`.".into(),
                )
            })?;

        let access_token = string_field(tokens.get("access_token")).ok_or_else(|| {
            ProviderError::Auth("Codex auth.json is missing tokens.access_token".into())
        })?;
        let refresh_token = string_field(tokens.get("refresh_token")).ok_or_else(|| {
            ProviderError::Auth("Codex auth.json is missing tokens.refresh_token".into())
        })?;
        let id_token = string_field(tokens.get("id_token"));
        let account_id = string_field(tokens.get("account_id")).or_else(|| {
            id_token
                .as_deref()
                .and_then(jwt_payload)
                .and_then(|payload| {
                    payload
                        .get("https://api.openai.com/auth")
                        .and_then(|auth| auth.get("chatgpt_account_id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        });
        let is_fedramp_account = id_token
            .as_deref()
            .and_then(jwt_payload)
            .and_then(|payload| {
                payload
                    .get("https://api.openai.com/auth")
                    .and_then(|auth| auth.get("chatgpt_account_is_fedramp"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false);
        let last_refresh = raw
            .get("last_refresh")
            .and_then(Value::as_str)
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .map(|ts| ts.with_timezone(&Utc));

        Ok(Self {
            raw,
            access_token,
            refresh_token,
            account_id,
            last_refresh,
            is_fedramp_account,
        })
    }

    fn needs_refresh(&self) -> bool {
        if let Some(expires_at) = jwt_expiration(&self.access_token) {
            return expires_at <= Utc::now();
        }
        self.last_refresh.is_some_and(|last| {
            last < Utc::now() - chrono::Duration::days(TOKEN_REFRESH_INTERVAL_DAYS)
        })
    }

    fn apply_refresh(&mut self, response: RefreshResponse) -> Result<(), ProviderError> {
        let now = Utc::now();
        let tokens = self
            .raw
            .get_mut("tokens")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| ProviderError::Auth("Codex auth.json tokens disappeared".into()))?;

        if let Some(id_token) = response.id_token {
            tokens.insert("id_token".to_string(), Value::String(id_token));
        }
        if let Some(access_token) = response.access_token {
            tokens.insert("access_token".to_string(), Value::String(access_token));
        }
        if let Some(refresh_token) = response.refresh_token {
            tokens.insert("refresh_token".to_string(), Value::String(refresh_token));
        }
        let root = self
            .raw
            .as_object_mut()
            .ok_or_else(|| ProviderError::Auth("Codex auth.json root is not an object".into()))?;
        root.insert("last_refresh".to_string(), Value::String(now.to_rfc3339()));

        *self = Self::from_raw(self.raw.clone())?;
        Ok(())
    }

    fn save_to_file(&self, path: &Path) -> Result<(), ProviderError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ProviderError::Auth(e.to_string()))?;
        }

        let data = serde_json::to_string_pretty(&self.raw)
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .map_err(|e| ProviderError::Auth(e.to_string()))?;
        file.write_all(data.as_bytes())
            .map_err(|e| ProviderError::Auth(e.to_string()))
    }
}

fn default_codex_home() -> Option<PathBuf> {
    std::env::var("CODEX_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(DEFAULT_CODEX_HOME)))
}

fn string_field(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn jwt_expiration(jwt: &str) -> Option<DateTime<Utc>> {
    let payload = jwt_payload(jwt)?;
    let claims: JwtStandardClaims = serde_json::from_value(payload).ok()?;
    claims
        .exp
        .and_then(|exp| DateTime::<Utc>::from_timestamp(exp, 0))
}

fn jwt_payload(jwt: &str) -> Option<Value> {
    let mut parts = jwt.split('.');
    let (_header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = base64_url_decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn base64_url_decode(input: &str) -> Result<Vec<u8>, ()> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;

    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return Err(()),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Ok(out)
}

fn refresh_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| match error {
                    Value::Object(map) => map
                        .get("message")
                        .or_else(|| map.get("code"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    Value::String(message) => Some(message.clone()),
                    _ => None,
                })
                .or_else(|| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_else(|| "auth service returned an error".to_string())
}

// ---- Browser "Sign in with ChatGPT" login ----

const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
/// The exact redirect URI registered with the Codex OAuth app. OpenAI rejects
/// any other value, so the loopback server must bind this fixed port and path.
const OAUTH_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const OAUTH_LOOPBACK_PORT: u16 = 1455;

/// A credential store that discards writes. Codex tokens are persisted to
/// `<codex_home>/auth.json` for interop with the `codex` CLI, so the generic
/// OAuth store must not scatter a second copy of the tokens elsewhere on disk.
struct DiscardStore;

impl CredentialStore for DiscardStore {
    fn load(&self, _key: &str) -> Result<Option<TokenSet>, OAuthError> {
        Ok(None)
    }
    fn save(&self, _key: &str, _tokens: &TokenSet) -> Result<(), OAuthError> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), OAuthError> {
        Ok(())
    }
}

fn codex_oauth_config() -> OAuthProviderConfig {
    OAuthProviderConfig {
        provider_name: "codex".to_string(),
        authorization_url: AUTHORIZE_URL.to_string(),
        token_url: REFRESH_TOKEN_URL.to_string(),
        client_id: CLIENT_ID.to_string(),
        scopes: vec![
            "openid".to_string(),
            "profile".to_string(),
            "email".to_string(),
            "offline_access".to_string(),
        ],
        redirect_uri: OAUTH_REDIRECT_URI.to_string(),
        loopback_port: Some(OAUTH_LOOPBACK_PORT),
        // Tells the issuer to include the ChatGPT account/org claims in the
        // id_token, which is where the account id is read from.
        extra_authorize_params: vec![(
            "id_token_add_organizations".to_string(),
            "true".to_string(),
        )],
        allow_insecure_local: false,
    }
}

/// Run the browser "Sign in with ChatGPT" OAuth flow (PKCE) and write the
/// resulting session to `<codex_home>/auth.json`, the same file the `codex` CLI
/// uses, so agent-code and the codex CLI share one subscription session. No
/// `codex` CLI installation is required. Returns the path written.
pub async fn browser_login(codex_home: Option<&str>) -> Result<PathBuf, ProviderError> {
    let auth_file = resolve_auth_file(codex_home)?;

    let service = OAuthService::with_store(codex_oauth_config(), Arc::new(DiscardStore))
        .map_err(|e| ProviderError::Auth(e.to_string()))?;
    let tokens = service
        .login()
        .await
        .map_err(|e| ProviderError::Auth(format!("ChatGPT sign-in failed: {e}")))?;

    let refresh_token = tokens.refresh_token.ok_or_else(|| {
        ProviderError::Auth(
            "sign-in returned no refresh token (offline_access scope missing?)".into(),
        )
    })?;
    let id_token = tokens.id_token.ok_or_else(|| {
        ProviderError::Auth(
            "sign-in returned no id_token; cannot resolve the ChatGPT account".into(),
        )
    })?;
    let account_id = account_id_from_id_token(&id_token);
    // Best-effort: mirror `codex login` by exchanging the id_token for an API
    // key so auth.json is fully equivalent. A failure leaves the key unset; the
    // access token (which the codex_chatgpt mode uses) still works.
    let openai_api_key = obtain_api_key(&id_token).await;

    write_codex_auth_json(
        &auth_file,
        &tokens.access_token,
        &refresh_token,
        &id_token,
        account_id.as_deref(),
        openai_api_key.as_deref(),
    )?;
    Ok(auth_file)
}

fn resolve_auth_file(codex_home: Option<&str>) -> Result<PathBuf, ProviderError> {
    let home = match codex_home {
        Some(path) => PathBuf::from(path),
        None => default_codex_home().ok_or_else(|| {
            ProviderError::Auth("could not determine Codex home; set CODEX_HOME".into())
        })?,
    };
    Ok(home.join("auth.json"))
}

fn account_id_from_id_token(id_token: &str) -> Option<String> {
    jwt_payload(id_token).and_then(|payload| {
        payload
            .get("https://api.openai.com/auth")
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

/// Exchange the id_token for an `OPENAI_API_KEY` via RFC 8693 token-exchange,
/// the same step `codex login` performs so `auth.json` carries an API key.
///
/// Best-effort: agent-code's `codex_chatgpt` auth mode authenticates with the
/// access token, so a failure here is non-fatal — the key is simply left unset,
/// and the written session still works for agent-code. It matters only for
/// interop with tools (or the `codex` CLI's API-key mode) that read the key.
async fn obtain_api_key(id_token: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ExchangeResp {
        access_token: String,
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    let resp = client
        .post(REFRESH_TOKEN_URL)
        .form(&[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("client_id", CLIENT_ID),
            ("requested_token", "openai-api-key"),
            ("subject_token", id_token),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:id_token",
            ),
        ])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<ExchangeResp>()
        .await
        .ok()
        .map(|r| r.access_token)
}

/// Write the codex `auth.json` shape (`{OPENAI_API_KEY, tokens, last_refresh}`)
/// with `0600` permissions, mirroring [`CodexAuthState::save_to_file`].
fn write_codex_auth_json(
    path: &Path,
    access_token: &str,
    refresh_token: &str,
    id_token: &str,
    account_id: Option<&str>,
    openai_api_key: Option<&str>,
) -> Result<(), ProviderError> {
    let raw = serde_json::json!({
        // `null` when the API-key exchange did not run or failed; the ChatGPT
        // access token below is what the codex_chatgpt auth mode uses.
        "OPENAI_API_KEY": openai_api_key,
        // Matches `codex login`, so the codex CLI recognizes this session too.
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": access_token,
            "refresh_token": refresh_token,
            "id_token": id_token,
            "account_id": account_id,
        },
        "last_refresh": Utc::now().to_rfc3339(),
    });

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ProviderError::Auth(e.to_string()))?;
    }
    let data = serde_json::to_string_pretty(&raw)
        .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| ProviderError::Auth(e.to_string()))?;
    file.write_all(data.as_bytes())
        .map_err(|e| ProviderError::Auth(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt_with_payload(payload: &str) -> String {
        format!(
            "header.{}.sig",
            base64_url_encode_for_test(payload.as_bytes())
        )
    }

    fn base64_url_encode_for_test(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        let mut i = 0;
        while i < input.len() {
            let b0 = input[i];
            let b1 = input.get(i + 1).copied().unwrap_or(0);
            let b2 = input.get(i + 2).copied().unwrap_or(0);
            let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
            out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
            if i + 1 < input.len() {
                out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
            }
            if i + 2 < input.len() {
                out.push(ALPHABET[(triple & 0x3f) as usize] as char);
            }
            i += 3;
        }
        out
    }

    #[test]
    fn parses_codex_auth_json_account_from_token_field() {
        let raw = serde_json::json!({
            "tokens": {
                "access_token": jwt_with_payload(r#"{"exp":4102444800}"#),
                "refresh_token": "refresh-token",
                "account_id": "account-1",
                "id_token": jwt_with_payload(r#"{"https://api.openai.com/auth":{"chatgpt_account_is_fedramp":true}}"#)
            },
            "last_refresh": "2026-04-27T00:00:00Z",
            "future_field": {"preserved": true}
        });

        let state = CodexAuthState::from_raw(raw).unwrap();

        assert_eq!(state.account_id.as_deref(), Some("account-1"));
        assert!(state.is_fedramp_account);
        assert!(!state.needs_refresh());
    }

    #[test]
    fn parses_codex_auth_json_account_from_id_token() {
        let raw = serde_json::json!({
            "tokens": {
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "id_token": jwt_with_payload(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"account-from-jwt"}}"#)
            }
        });

        let state = CodexAuthState::from_raw(raw).unwrap();

        assert_eq!(state.account_id.as_deref(), Some("account-from-jwt"));
    }

    #[test]
    fn codex_oauth_config_uses_fixed_loopback_and_org_param() {
        let cfg = codex_oauth_config();
        assert_eq!(cfg.loopback_port, Some(1455));
        assert!(cfg.redirect_uri.contains(":1455/auth/callback"));
        assert!(cfg.scopes.iter().any(|s| s == "offline_access"));
        assert!(
            cfg.extra_authorize_params
                .iter()
                .any(|(k, v)| k == "id_token_add_organizations" && v == "true"),
            "the org param is what makes the id_token carry the account id"
        );
        // https endpoints + loopback redirect: no insecure-local needed.
        assert!(!cfg.allow_insecure_local);
    }

    #[test]
    fn debug_never_prints_tokens() {
        let raw = serde_json::json!({
            "tokens": {
                "access_token": "SECRET-ACCESS-abc",
                "refresh_token": "SECRET-REFRESH-xyz",
            }
        });
        let state = CodexAuthState::from_raw(raw).unwrap();
        let dbg = format!("{state:?}");
        assert!(
            !dbg.contains("SECRET-ACCESS-abc"),
            "access token leaked in Debug"
        );
        assert!(
            !dbg.contains("SECRET-REFRESH-xyz"),
            "refresh token leaked in Debug"
        );
        assert!(dbg.contains("***"));
    }

    #[test]
    fn account_id_from_id_token_reads_chatgpt_claim() {
        let id_token = jwt_with_payload(
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-xyz"}}"#,
        );
        assert_eq!(
            account_id_from_id_token(&id_token).as_deref(),
            Some("acct-xyz")
        );
    }

    #[test]
    fn browser_login_writer_produces_a_loadable_auth_json() {
        // The file the browser flow writes must be the same shape the loader
        // (and the codex CLI) reads, with account_id resolved from the id_token.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let id_token = jwt_with_payload(
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-roundtrip"}}"#,
        );
        write_codex_auth_json(
            &path,
            "access-tok",
            "refresh-tok",
            &id_token,
            Some("acct-roundtrip"),
            Some("sk-test-key"),
        )
        .unwrap();

        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["OPENAI_API_KEY"], "sk-test-key");
        assert_eq!(raw["auth_mode"], "chatgpt");
        assert_eq!(raw["tokens"]["access_token"], "access-tok");
        assert_eq!(raw["tokens"]["account_id"], "acct-roundtrip");
        assert!(raw.get("last_refresh").and_then(Value::as_str).is_some());

        // A failed/absent API-key exchange writes null, not a missing field.
        let path2 = dir.path().join("auth2.json");
        write_codex_auth_json(&path2, "a", "r", &id_token, None, None).unwrap();
        let raw2: Value = serde_json::from_str(&std::fs::read_to_string(&path2).unwrap()).unwrap();
        assert!(raw2.get("OPENAI_API_KEY").unwrap().is_null());

        // The loader's own parser must accept it, account_id resolved.
        let state = CodexAuthState::from_raw(raw).unwrap();
        assert_eq!(state.account_id.as_deref(), Some("acct-roundtrip"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "auth.json holds tokens; must be private"
            );
        }
    }

    #[test]
    fn detects_expired_access_token() {
        let raw = serde_json::json!({
            "tokens": {
                "access_token": jwt_with_payload(r#"{"exp":946684800}"#),
                "refresh_token": "refresh-token"
            },
            "last_refresh": "2026-04-27T00:00:00Z"
        });

        let state = CodexAuthState::from_raw(raw).unwrap();

        assert!(state.needs_refresh());
    }
}
