//! xAI Grok SuperGrok / X Premium OAuth (device-code).
//!
//! Lets SuperGrok and X Premium subscribers sign in via
//! `https://auth.x.ai` without an `XAI_API_KEY`, the same product path
//! xAI documents for OpenCode / Hermes / peer coding agents
//! (<https://x.ai/news/grok-opencode>).
//!
//! Flow: OAuth 2.0 device authorization against `auth.x.ai`, public
//! client id shared by OSS harnesses, tokens stored under the user
//! config dir (`xai_auth.json`). Access tokens are used as Bearer
//! credentials against `https://api.x.ai/v1`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::provider::ProviderError;
use crate::config::agent_config_dir;

/// Public OAuth client id used by OSS coding agents for SuperGrok OAuth
/// (Hermes / OpenCode ecosystem). Not a secret.
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const ISSUER: &str = "https://auth.x.ai";
const DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const DEFAULT_INFERENCE_BASE: &str = "https://api.x.ai/v1";
/// Refresh this many seconds before `expires_at`.
const REFRESH_SKEW_SECS: u64 = 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTokens {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    id_token: Option<String>,
    /// Unix epoch seconds when `access_token` expires.
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
}

/// Live SuperGrok / X Premium OAuth session.
pub struct XaiOauthAuth {
    path: PathBuf,
    tokens: Mutex<StoredTokens>,
    http: reqwest::Client,
}

impl XaiOauthAuth {
    /// Load tokens from, in order:
    /// 1. explicit `path` if given
    /// 2. agent-code store (`~/.config/agent-code/xai_auth.json`)
    /// 3. official Grok CLI store (`~/.grok/auth.json`) — same client id
    ///
    /// So `grok login` sessions work with `agent --auth-mode xai_oauth`
    /// without a second sign-in (mirrors Codex reusing `~/.codex`).
    pub fn load(path: Option<&Path>) -> Result<Self, ProviderError> {
        let candidates: Vec<PathBuf> = if let Some(p) = path {
            vec![p.to_path_buf()]
        } else {
            let mut v = Vec::new();
            if let Ok(p) = default_auth_path() {
                v.push(p);
            }
            if let Some(p) = grok_cli_auth_path() {
                v.push(p);
            }
            v
        };

        let mut last_err = String::from(
            "no xAI OAuth session found. Run `agent login xai` or `grok login`, then retry.",
        );
        for path in &candidates {
            if !path.exists() {
                continue;
            }
            match load_tokens_from_file(path) {
                Ok(tokens) => {
                    debug!("loaded xAI OAuth session from {}", path.display());
                    return Ok(Self {
                        path: path.clone(),
                        tokens: Mutex::new(tokens),
                        http: reqwest::Client::new(),
                    });
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        Err(ProviderError::Auth(last_err))
    }

    /// Return a valid access token, refreshing if near expiry.
    pub async fn access_token(&self) -> Result<String, ProviderError> {
        let need_refresh = {
            let g = self
                .tokens
                .lock()
                .map_err(|_| ProviderError::Auth("xAI OAuth token lock poisoned".into()))?;
            match g.expires_at {
                Some(exp) => {
                    let now = now_unix();
                    now + REFRESH_SKEW_SECS >= exp
                }
                // Unknown expiry — use as-is until a 401 forces re-login.
                None => false,
            }
        };
        if need_refresh {
            self.refresh().await?;
        }
        let g = self
            .tokens
            .lock()
            .map_err(|_| ProviderError::Auth("xAI OAuth token lock poisoned".into()))?;
        Ok(g.access_token.clone())
    }

    async fn refresh(&self) -> Result<(), ProviderError> {
        let (refresh_token, token_endpoint) = {
            let g = self
                .tokens
                .lock()
                .map_err(|_| ProviderError::Auth("xAI OAuth token lock poisoned".into()))?;
            (
                g.refresh_token.clone(),
                g.token_endpoint
                    .clone()
                    .unwrap_or_else(|| format!("{ISSUER}/oauth2/token")),
            )
        };

        let resp = self
            .http
            .post(&token_endpoint)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", refresh_token.as_str()),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Auth(format!("xAI token refresh network error: {e}")))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_else(|_| String::new());
        if !status.is_success() {
            if status.as_u16() == 403 {
                return Err(ProviderError::Auth(
                    "xAI token refresh HTTP 403 — this SuperGrok/X Premium account may not \
                     be entitled for OAuth API access. Fall back to XAI_API_KEY from \
                     console.x.ai, or upgrade at https://x.ai/grok."
                        .into(),
                ));
            }
            return Err(ProviderError::Auth(format!(
                "xAI token refresh failed (HTTP {status}): {body}. Re-run `agent login xai`."
            )));
        }
        let payload: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ProviderError::Auth(format!("xAI token refresh returned invalid JSON: {e}"))
        })?;
        let access = payload
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Auth("xAI token refresh missing access_token".into()))?;
        let new_refresh = payload
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let expires_in = payload.get("expires_in").and_then(|v| v.as_u64());

        {
            let mut g = self
                .tokens
                .lock()
                .map_err(|_| ProviderError::Auth("xAI OAuth token lock poisoned".into()))?;
            g.access_token = access.to_string();
            if let Some(r) = new_refresh {
                g.refresh_token = r.to_string();
            }
            if let Some(secs) = expires_in {
                g.expires_at = Some(now_unix().saturating_add(secs));
            }
            // Never rewrite the official Grok CLI multi-entry auth.json as
            // our flat format — persist refreshed tokens into agent-code's
            // own store instead.
            let write_path = if is_grok_cli_auth_path(&self.path) {
                default_auth_path()?
            } else {
                self.path.clone()
            };
            if let Some(parent) = write_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            persist_tokens(&write_path, &g)?;
        }
        debug!("refreshed xAI OAuth access token");
        Ok(())
    }
}

/// Run the device-code SuperGrok / X Premium login and persist tokens.
///
/// Prints the verification URL and user code, optionally opens a browser,
/// then polls until the user approves. Returns the path written.
pub async fn device_code_login(open_browser: bool) -> Result<PathBuf, ProviderError> {
    let http = reqwest::Client::new();
    let discovery = discover_endpoints(&http).await?;
    let token_endpoint = discovery.token_endpoint;

    let device = request_device_code(&http).await?;
    let verification_url = device
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| device.verification_uri.clone());

    // Match official `grok login` UX so headless/SSH is obvious.
    eprintln!();
    eprintln!("To sign in, open this URL in your browser:");
    eprintln!();
    eprintln!("  {verification_url}");
    eprintln!();
    eprintln!("Confirm this code in your browser:");
    eprintln!();
    eprintln!("  {}", device.user_code);
    eprintln!();
    eprintln!("Only continue with a code you requested. Don't share it with anyone.");
    eprintln!();
    if open_browser {
        match try_open_browser(&verification_url) {
            Ok(()) => {}
            Err(e) => {
                warn!("could not open browser automatically: {e}");
            }
        }
    }
    eprintln!("Waiting for authorization...");

    let tokens = poll_device_token(
        &http,
        &token_endpoint,
        &device.device_code,
        device.expires_in,
        device.interval,
    )
    .await?;

    let path = default_auth_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ProviderError::Auth(format!("create config dir {}: {e}", parent.display()))
        })?;
    }
    let expires_at = tokens
        .expires_in
        .map(|secs| now_unix().saturating_add(secs));
    let stored = StoredTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        expires_at,
        token_endpoint: Some(token_endpoint),
        base_url: Some(DEFAULT_INFERENCE_BASE.to_string()),
    };
    persist_tokens(&path, &stored)?;
    Ok(path)
}

fn default_auth_path() -> Result<PathBuf, ProviderError> {
    let dir = agent_config_dir().ok_or_else(|| {
        ProviderError::Auth("could not resolve agent-code config directory".into())
    })?;
    Ok(dir.join("xai_auth.json"))
}

/// Official Grok CLI auth file (`grok login` / `grok login --device-auth`).
fn grok_cli_auth_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("GROK_HOME") {
        let p = PathBuf::from(home).join("auth.json");
        if p.exists() {
            return Some(p);
        }
    }
    dirs::home_dir().map(|h| h.join(".grok").join("auth.json"))
}

fn is_grok_cli_auth_path(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some("auth.json")
        && path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == ".grok" || n.contains("grok"))
}

/// Parse either our flat `StoredTokens` JSON or the Grok CLI multi-entry map.
fn load_tokens_from_file(path: &Path) -> Result<StoredTokens, ProviderError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ProviderError::Auth(format!("read {}: {e}", path.display())))?;
    // 1) agent-code shape
    if let Ok(tokens) = serde_json::from_str::<StoredTokens>(&raw)
        && !tokens.access_token.is_empty()
        && !tokens.refresh_token.is_empty()
    {
        return Ok(tokens);
    }
    // 2) Grok CLI shape: { "https://auth.x.ai::<client_id>": { key, refresh_token, expires_at, ... } }
    if let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw)
        && let Some(tokens) = parse_grok_cli_auth_map(&map)
    {
        return Ok(tokens);
    }
    Err(ProviderError::Auth(format!(
        "unrecognized xAI OAuth file format at {} (expected agent-code or grok CLI auth.json)",
        path.display()
    )))
}

fn parse_grok_cli_auth_map(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<StoredTokens> {
    // Prefer the entry for our known client id; else first usable entry.
    let preferred_key = format!("{ISSUER}::{CLIENT_ID}");
    let order: Vec<&String> = {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by_key(|k| if *k == &preferred_key { 0 } else { 1 });
        keys
    };
    for k in order {
        let Some(entry) = map.get(k).and_then(|v| v.as_object()) else {
            continue;
        };
        // Grok CLI stores the access token under `key`.
        let Some(access) = entry
            .get("key")
            .or_else(|| entry.get("access_token"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(refresh) = entry
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let expires_at = entry
            .get("expires_at")
            .and_then(|v| v.as_str())
            .and_then(parse_rfc3339_unix)
            .or_else(|| entry.get("expires_at").and_then(|v| v.as_u64()));
        let token_endpoint = entry
            .get("oidc_issuer")
            .and_then(|v| v.as_str())
            .map(|iss| format!("{}/oauth2/token", iss.trim_end_matches('/')))
            .or_else(|| Some(format!("{ISSUER}/oauth2/token")));
        return Some(StoredTokens {
            access_token: access.to_string(),
            refresh_token: refresh.to_string(),
            id_token: entry
                .get("id_token")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            expires_at,
            token_endpoint,
            base_url: Some(DEFAULT_INFERENCE_BASE.to_string()),
        });
    }
    None
}

fn parse_rfc3339_unix(s: &str) -> Option<u64> {
    // Accept "2026-07-10T12:17:28.383999638Z" — chrono if available, else rough parse.
    // Prefer chrono since the crate already depends on it elsewhere in lib.
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp().max(0) as u64)
        .or_else(|| {
            // Truncate fractional seconds if too long for chrono
            let trimmed = if let Some((main, rest)) = s.split_once('.') {
                let frac: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .take(6)
                    .collect();
                let tz = rest.trim_start_matches(|c: char| c.is_ascii_digit());
                format!("{main}.{frac}{tz}")
            } else {
                s.to_string()
            };
            chrono::DateTime::parse_from_rfc3339(&trimmed)
                .ok()
                .map(|dt| dt.timestamp().max(0) as u64)
        })
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn persist_tokens(path: &Path, tokens: &StoredTokens) -> Result<(), ProviderError> {
    let body = serde_json::to_vec_pretty(tokens)
        .map_err(|e| ProviderError::Auth(format!("serialize xAI OAuth tokens: {e}")))?;
    // Owner-only when possible (unix 0600).
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| ProviderError::Auth(format!("write {}: {e}", path.display())))?;
        f.write_all(&body)
            .map_err(|e| ProviderError::Auth(format!("write {}: {e}", path.display())))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, &body)
            .map_err(|e| ProviderError::Auth(format!("write {}: {e}", path.display())))?;
    }
    Ok(())
}

struct Discovery {
    token_endpoint: String,
}

async fn discover_endpoints(http: &reqwest::Client) -> Result<Discovery, ProviderError> {
    let resp = http
        .get(DISCOVERY_URL)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| ProviderError::Auth(format!("xAI OIDC discovery failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(ProviderError::Auth(format!(
            "xAI OIDC discovery HTTP {}",
            resp.status()
        )));
    }
    let payload: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ProviderError::Auth(format!("xAI OIDC discovery JSON: {e}")))?;
    let token_endpoint = payload
        .get("token_endpoint")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("https://auth.x.ai/oauth2/token")
        .to_string();
    if !token_endpoint.contains("x.ai") {
        return Err(ProviderError::Auth(format!(
            "xAI OIDC discovery returned unexpected token_endpoint: {token_endpoint}"
        )));
    }
    Ok(Discovery { token_endpoint })
}

struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

async fn request_device_code(http: &reqwest::Client) -> Result<DeviceCode, ProviderError> {
    let resp = http
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .send()
        .await
        .map_err(|e| ProviderError::Auth(format!("xAI device-code request failed: {e}")))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderError::Auth(format!(
            "xAI device-code request HTTP {status}: {body}"
        )));
    }
    let payload: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| ProviderError::Auth(format!("xAI device-code JSON: {e}")))?;
    Ok(DeviceCode {
        device_code: required_str(&payload, "device_code")?,
        user_code: required_str(&payload, "user_code")?,
        verification_uri: required_str(&payload, "verification_uri")?,
        verification_uri_complete: payload
            .get("verification_uri_complete")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        expires_in: payload
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(900),
        interval: payload
            .get("interval")
            .and_then(|v| v.as_u64())
            .unwrap_or(5),
    })
}

struct TokenResponse {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

async fn poll_device_token(
    http: &reqwest::Client,
    token_endpoint: &str,
    device_code: &str,
    expires_in: u64,
    poll_interval: u64,
) -> Result<TokenResponse, ProviderError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(expires_in.max(30));
    let mut interval = poll_interval.max(1);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(ProviderError::Auth(
                "Timed out waiting for xAI device authorization. Re-run `agent login xai`.".into(),
            ));
        }
        let resp = http
            .post(token_endpoint)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", CLIENT_ID),
                ("device_code", device_code),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Auth(format!("xAI device-code poll failed: {e}")))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let payload: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| ProviderError::Auth(format!("xAI device-code token JSON: {e}")))?;
            return Ok(TokenResponse {
                access_token: required_str(&payload, "access_token")?,
                refresh_token: required_str(&payload, "refresh_token")?,
                id_token: payload
                    .get("id_token")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                expires_in: payload.get("expires_in").and_then(|v| v.as_u64()),
            });
        }
        let err_code = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_default();
        match err_code.as_str() {
            "authorization_pending" => {
                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
            "slow_down" => {
                interval = (interval + 1).min(30);
                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
            other => {
                return Err(ProviderError::Auth(format!(
                    "xAI device-code token polling failed ({other}): {body}"
                )));
            }
        }
    }
}

fn required_str(v: &serde_json::Value, key: &str) -> Result<String, ProviderError> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ProviderError::Auth(format!("xAI OAuth response missing `{key}`")))
}

fn try_open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let bin = "open";
    #[cfg(target_os = "windows")]
    let bin = "cmd";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let bin = "xdg-open";

    let mut cmd = std::process::Command::new(bin);
    #[cfg(target_os = "windows")]
    {
        cmd.args(["/C", "start", "", url]);
    }
    #[cfg(not(target_os = "windows"))]
    {
        cmd.arg(url);
    }
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{bin}: {e}"))?
        .wait()
        .map_err(|e| format!("{bin} wait: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("xai_auth.json");
        let stored = StoredTokens {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            id_token: None,
            expires_at: Some(now_unix() + 3600),
            token_endpoint: Some("https://auth.x.ai/oauth2/token".into()),
            base_url: Some(DEFAULT_INFERENCE_BASE.into()),
        };
        persist_tokens(&path, &stored).unwrap();
        let auth = XaiOauthAuth::load(Some(&path)).unwrap();
        let g = auth.tokens.lock().unwrap();
        assert_eq!(g.access_token, "access");
        assert_eq!(g.refresh_token, "refresh");
    }

    #[test]
    fn load_grok_cli_auth_json_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        // Shape produced by official `grok login` (redacted values).
        let body = serde_json::json!({
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "key": "access-token-from-grok-cli",
                "auth_mode": "oidc",
                "refresh_token": "refresh-token-from-grok-cli",
                "expires_at": "2099-01-01T00:00:00Z",
                "oidc_issuer": "https://auth.x.ai",
                "oidc_client_id": "b1a00492-073a-47ea-816f-4c329264a828"
            }
        });
        std::fs::write(&path, body.to_string()).unwrap();
        let auth = XaiOauthAuth::load(Some(&path)).unwrap();
        let g = auth.tokens.lock().unwrap();
        assert_eq!(g.access_token, "access-token-from-grok-cli");
        assert_eq!(g.refresh_token, "refresh-token-from-grok-cli");
        assert!(g.expires_at.unwrap() > now_unix());
    }

    #[test]
    fn parse_rfc3339_with_long_fractional_nanos() {
        // Grok CLI emits nanosecond fractions longer than chrono's default.
        let ts = parse_rfc3339_unix("2026-07-10T12:17:28.383999638Z");
        assert!(ts.is_some());
    }
}
