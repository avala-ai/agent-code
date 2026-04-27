//! Codex ChatGPT authentication support.
//!
//! This reuses an existing `codex login` session from CODEX_HOME. It does
//! not perform browser login and it does not store tokens in agent-code
//! configuration.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use super::provider::ProviderError;

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

#[derive(Debug, Clone)]
struct CodexAuthState {
    raw: Value,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
    last_refresh: Option<DateTime<Utc>>,
    is_fedramp_account: bool,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
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
