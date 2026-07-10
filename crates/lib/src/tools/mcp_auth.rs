//! McpAuth tool: re-trigger the auth flow for a configured MCP
//! server.
//!
//! Designed for the case where a long-running MCP tool call fails
//! with a 401 because the upstream session expired. Rather than
//! kicking the user out of the loop, the model can invoke this tool
//! and the engine refreshes the credential without dropping back to
//! the user.
//!
//! # Supported subset (v1)
//!
//! The current MCP config schema in [`crate::config::McpServerEntry`]
//! does not yet model OAuth-style auth — it only carries `command`,
//! `args`, `url`, and `env`. So in this first cut the tool can:
//!
//! - Recognise the configured server and report what kind of auth it
//!   uses (static env vars, no auth, or unknown).
//! - Return a clear "no auth flow to re-trigger" message for servers
//!   that authenticate via static API keys / env vars (the common
//!   case today).
//! - Stub-respond with "not yet supported for this auth kind" if a
//!   future config layer adds an OAuth field but no flow is wired up.
//!
//! Once a real OAuth surface lands, this tool's `call` becomes the
//! single hand-off point: it'll detect the OAuth kind and delegate
//! to the existing flow. The current behaviour deliberately does
//! not implement OAuth from scratch.

use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use super::{Tool, ToolContext, ToolResult};
use crate::config::{McpServerEntry, find_project_config_from};
use crate::error::ToolError;

pub struct McpAuthTool;

#[async_trait]
impl Tool for McpAuthTool {
    fn name(&self) -> &'static str {
        "McpAuth"
    }

    fn description(&self) -> &'static str {
        "Re-trigger the auth flow for a configured MCP server (use \
         when a tool call to that server returns 401). Looks up the \
         server's auth configuration: if it uses an interactive \
         OAuth-style flow, the tool re-runs that flow; if it uses \
         static API keys or no auth, the tool returns a clear \
         message saying there is no flow to re-trigger."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["server_name"],
            "properties": {
                "server_name": {
                    "type": "string",
                    "description": "Name of a server configured under [mcp_servers] in settings.toml"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        // Conservatively false: even when no flow is re-triggered, a
        // future auth flow would mutate on-disk credentials, so we
        // route through the permission system from day one rather
        // than have to re-permission later.
        false
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let server_name = input
            .get("server_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'server_name' is required".into()))?
            .trim()
            .to_string();
        if server_name.is_empty() {
            return Err(ToolError::InvalidInput(
                "'server_name' must not be empty".into(),
            ));
        }

        let entry = load_mcp_entry(&ctx.cwd, &server_name)
            .map_err(|e| ToolError::ExecutionFailed(format!("config load failed: {e}")))?;

        Ok(handle_auth(&server_name, entry.as_ref()))
    }
}

/// Resolve the configured MCP server entry for `server_name`, using
/// `cwd` (typically `ToolContext::cwd`) as the starting point for the
/// project-config walk. Importantly this does NOT use the process's
/// `current_dir` — server / worktree contexts can have a `ctx.cwd`
/// that points at a different project than the parent process, and
/// using `Config::load()` would silently pick up the wrong config.
///
/// Falls back to the user-level config file for the case where the
/// project has no `.agent/settings.toml`.
fn load_mcp_entry(cwd: &Path, server_name: &str) -> Result<Option<McpServerEntry>, String> {
    let mut layers: Vec<String> = Vec::new();

    if let Some(path) = crate::config::user_config_path()
        && path.exists()
    {
        layers.push(std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?);
    }

    if let Some(path) = find_project_config_from(cwd) {
        layers.push(std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?);
    }

    let layer_refs: Vec<&str> = layers.iter().map(String::as_str).collect();
    let config = crate::config::merge_layer_contents(&layer_refs)
        .map_err(|e| format!("merge config: {e}"))?;

    Ok(config.mcp_servers.get(server_name).cloned())
}

/// Pure helper: classify the auth kind of an MCP server entry and
/// return an appropriate result. Split out so unit tests can drive
/// it without touching the on-disk config.
pub(crate) fn handle_auth(server_name: &str, entry: Option<&McpServerEntry>) -> ToolResult {
    match entry {
        None => ToolResult::error(format!(
            "MCP server '{server_name}' is not configured. Add it under [mcp_servers] in .agent/settings.toml."
        )),
        Some(entry) => dispatch_auth_kind(server_name, classify_auth(entry)),
    }
}

/// Map a classified [`McpAuthKind`] to a [`ToolResult`]. Lifted out
/// of [`handle_auth`] so tests can drive each arm — including
/// [`McpAuthKind::Unknown`], which no real config currently produces
/// but which must still fail closed (error, not success) the moment
/// it ever does.
pub(crate) fn dispatch_auth_kind(server_name: &str, kind: McpAuthKind) -> ToolResult {
    match kind {
        McpAuthKind::None => ToolResult::success(format!(
            "MCP server '{server_name}' has no auth flow to re-trigger (it does not declare any credentials). If the server returned 401, the issue is likely on the server side or in its own configuration."
        )),
        McpAuthKind::StaticEnv(keys) => ToolResult::success(format!(
            "MCP server '{server_name}' authenticates via static environment variables ({}). There is no interactive auth flow to re-trigger - update the values in your settings or your shell environment, then restart the server.",
            keys.join(", ")
        )),
        McpAuthKind::OAuth => ToolResult::success(format!(
            "MCP server '{server_name}' is marked as OAuth, but interactive OAuth refresh is not yet supported in this build. Manually re-authenticate and restart the server. (Tracking: see ROADMAP 8.8.)"
        )),
        McpAuthKind::Unknown => ToolResult::error(format!(
            "MCP server '{server_name}' has an auth configuration this build does not recognise (kind: unknown). No flow was re-triggered. Update the server's configuration or refresh credentials manually."
        )),
    }
}

/// Classify the auth surface of an MCP server entry. This is the
/// shape we can detect today, as a stepping stone toward a richer
/// `auth = { kind = "...", ... }` config block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpAuthKind {
    /// No credentials declared at all.
    None,
    /// Authenticates via static env vars; the names are surfaced
    /// so the tool can tell the user what to update.
    StaticEnv(Vec<String>),
    /// Server is marked as OAuth-style auth - flow not yet wired up.
    OAuth,
    /// Recognised an auth marker but couldn't classify it further.
    Unknown,
}

fn classify_auth(entry: &McpServerEntry) -> McpAuthKind {
    // Heuristic: env var names that contain `TOKEN`, `KEY`, `SECRET`,
    // `PASSWORD`, or `BEARER` are treated as static credentials. The
    // env map is the only credential surface in the current schema,
    // so when it's empty we can confidently say "no auth".
    if entry.env.is_empty() {
        return McpAuthKind::None;
    }

    let cred_keys: Vec<String> = entry
        .env
        .keys()
        .filter(|k| looks_like_credential(k))
        .cloned()
        .collect();

    if cred_keys.is_empty() {
        // Non-credential env vars (e.g. PATH overrides, log levels)
        // don't constitute an auth surface.
        return McpAuthKind::None;
    }

    // No OAuth field exists in the current schema. When one lands,
    // we'll branch on it here. Until then, env-var auth always means
    // "static credentials".
    McpAuthKind::StaticEnv(cred_keys)
}

fn looks_like_credential(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.contains("TOKEN")
        || n.contains("KEY")
        || n.contains("SECRET")
        || n.contains("PASSWORD")
        || n.contains("BEARER")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn make_ctx() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("."),
            cancel: CancellationToken::new(),
            permission_checker: Arc::new(crate::permissions::PermissionChecker::allow_all()),
            verbose: false,
            plan_mode: false,
            file_cache: None,
            denial_tracker: None,
            task_manager: None,
            subagent_colors: None,
            session_allows: None,
            permission_prompter: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
        }
    }

    #[test]
    fn classify_no_env_returns_none() {
        let entry = McpServerEntry {
            command: Some("foo".into()),
            args: vec![],
            url: None,
            env: Default::default(),
        };
        assert_eq!(classify_auth(&entry), McpAuthKind::None);
    }

    #[test]
    fn classify_credential_env_returns_static() {
        let mut env = HashMap::new();
        env.insert("MY_API_KEY".to_string(), "x".to_string());
        env.insert("LOG_LEVEL".to_string(), "info".to_string());
        let entry = McpServerEntry {
            command: Some("foo".into()),
            args: vec![],
            url: None,
            env,
        };
        match classify_auth(&entry) {
            McpAuthKind::StaticEnv(keys) => {
                assert!(keys.contains(&"MY_API_KEY".to_string()));
                assert!(!keys.contains(&"LOG_LEVEL".to_string()));
            }
            other => panic!("expected StaticEnv, got {other:?}"),
        }
    }

    #[test]
    fn classify_only_non_credential_env_returns_none() {
        let mut env = HashMap::new();
        env.insert("LOG_LEVEL".to_string(), "info".to_string());
        let entry = McpServerEntry {
            command: Some("foo".into()),
            args: vec![],
            url: None,
            env,
        };
        assert_eq!(classify_auth(&entry), McpAuthKind::None);
    }

    #[test]
    fn handle_auth_unknown_server_returns_error_result() {
        let res = handle_auth("ghost", None);
        assert!(res.is_error);
        assert!(res.content.contains("not configured"));
    }

    #[test]
    fn handle_auth_static_env_message_lists_keys() {
        let mut env = HashMap::new();
        env.insert("PROVIDER_API_TOKEN".to_string(), "x".to_string());
        let entry = McpServerEntry {
            command: Some("foo".into()),
            args: vec![],
            url: None,
            env,
        };
        let res = handle_auth("acme", Some(&entry));
        assert!(!res.is_error);
        assert!(res.content.contains("PROVIDER_API_TOKEN"));
        assert!(res.content.contains("no interactive auth flow"));
    }

    #[test]
    fn handle_auth_no_env_says_no_flow() {
        let entry = McpServerEntry {
            command: Some("foo".into()),
            args: vec![],
            url: None,
            env: Default::default(),
        };
        let res = handle_auth("acme", Some(&entry));
        assert!(!res.is_error);
        assert!(res.content.contains("no auth flow to re-trigger"));
    }

    #[tokio::test]
    async fn rejects_empty_server_name() {
        let tool = McpAuthTool;
        let err = tool
            .call(json!({ "server_name": "  " }), &make_ctx())
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("must not be empty")),
            _ => panic!("expected InvalidInput"),
        }
    }

    #[test]
    fn dispatch_unknown_kind_returns_error_result() {
        // Fail-closed property: anything classified as Unknown
        // must surface as an error tool result (so the model sees
        // a failure and won't keep retrying as if the call worked).
        let res = dispatch_auth_kind("acme", McpAuthKind::Unknown);
        assert!(res.is_error, "Unknown must fail closed");
        assert!(res.content.contains("kind: unknown"));
    }

    #[tokio::test]
    async fn call_uses_ctx_cwd_for_project_config_lookup() {
        // McpAuth must read the project config that lives under
        // `ctx.cwd`, not the process's `current_dir`. We seed a
        // tempdir with a `.agent/settings.toml` declaring an MCP
        // server, then drive the tool with `ctx.cwd` pointing at
        // that tempdir; the message we get back should reference
        // the tempdir's server, not anything from the surrounding
        // working directory.
        let dir = TempDir::new().unwrap();
        let agent_dir = dir.path().join(".agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("settings.toml"),
            r#"
[mcp_servers.local-only]
command = "echo"

[mcp_servers.local-only.env]
LOCAL_ONLY_API_KEY = "x"
"#,
        )
        .unwrap();

        let mut ctx = make_ctx();
        ctx.cwd = dir.path().to_path_buf();

        let tool = McpAuthTool;
        let res = tool
            .call(json!({ "server_name": "local-only" }), &ctx)
            .await
            .unwrap();
        // We saw the project-local server, not "not configured".
        assert!(!res.is_error);
        assert!(res.content.contains("LOCAL_ONLY_API_KEY"));
    }

    #[tokio::test]
    async fn call_reports_not_configured_for_unknown_server_in_isolated_cwd() {
        // Inverse of the previous test: an empty project produces
        // "not configured", confirming the tool isn't bleeding in
        // entries from a process-cwd config. We use a UUID-style
        // name to make accidental collision with anyone's user-level
        // config statistically impossible.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".agent")).unwrap();
        std::fs::write(dir.path().join(".agent").join("settings.toml"), "# empty\n").unwrap();

        let mut ctx = make_ctx();
        ctx.cwd = dir.path().to_path_buf();

        let tool = McpAuthTool;
        let res = tool
            .call(
                json!({ "server_name": "ghost-mcp-test-9b0a4d2c-0001" }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(res.is_error);
        assert!(res.content.contains("not configured"));
    }
}
