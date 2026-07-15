//! Config tool: model-callable settings reader/writer with a hard
//! allow-list.
//!
//! The agent can use this tool to inspect or change a small,
//! deliberately narrow set of user-tunable settings. Anything not in
//! [`crate::config::supported_settings::SUPPORTED_SETTINGS`] is
//! rejected with a clear error — there is no escape hatch.
//!
//! # Subcommands (via the `action` arg)
//!
//! - `list_supported` — return every entry on the allow-list with
//!   its key, description, kind, and scope.
//! - `get` — return the current value of one allow-listed setting,
//!   read from the on-disk TOML for its scope. Falls back to "(unset)"
//!   when the file or key is absent.
//! - `set` — validate `value` against the setting's kind, then
//!   write it to the matching on-disk TOML. The user-scope file is
//!   `~/.config/agent-code/config.toml`; the project-scope file is
//!   `<project>/.agent/settings.toml`.
//!
//! # Permission policy
//!
//! `get` and `list_supported` are read-only. `set` mutates a config
//! file and must go through the standard permission gate — by
//! default that means "ask the user" unless an `Allow` rule has
//! been configured for `Config`.
//!
//! The module is named `config_tool` (rather than `config`) to avoid
//! shadowing [`crate::config`].

use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};

use super::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::config::atomic::atomic_write_secret;
use crate::config::supported_settings::{self, Scope, SettingKind, SupportedSetting};
use crate::error::ToolError;
use crate::permissions::PermissionChecker;

pub struct ConfigTool;

#[async_trait]
impl Tool for ConfigTool {
    fn name(&self) -> &'static str {
        "Config"
    }

    fn description(&self) -> &'static str {
        "Read or write a small allow-list of user-tunable settings \
         (theme, default model, opt-in flags, etc.). Use action=\"list_supported\" \
         to discover what can be changed, action=\"get\" to read a value, \
         and action=\"set\" to update one. Anything not on the allow-list \
         is rejected; this tool cannot change permissions, sandbox, MCP, \
         hooks, or API keys."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get", "set", "list_supported"],
                    "description": "Which subcommand to invoke"
                },
                "key": {
                    "type": "string",
                    "description": "Dotted setting key (required for get and set)"
                },
                "value": {
                    "description": "New value (required for set). Must match the setting's kind."
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        // The mode is action-dependent — `set` mutates state. We keep
        // the trait method conservative (false) and let
        // [`check_permissions`] differentiate based on the parsed
        // action: read-only actions auto-allow, `set` runs through
        // the configured permission rule.
        false
    }

    /// Reject disallowed `set` keys at the engine boundary — before
    /// PreToolUse hooks fire, before the permission prompt, before
    /// the audit log records anything. The same allow-list / tripwire
    /// logic also runs from `check_permissions` (so the permission
    /// layer's decision matches what we'd reject anyway) and from
    /// inside `call` (defence in depth for direct callers); putting
    /// it here is what keeps audit hooks from observing rejected
    /// inputs.
    fn validate_input(&self, input: &serde_json::Value) -> Result<(), ToolError> {
        match input.get("action").and_then(|v| v.as_str()) {
            Some("set") => preflight_set_key(input).map_err(ToolError::InvalidInput),
            // get / list_supported / unknown actions surface their
            // errors inside `call`. We only short-circuit the path
            // where rejection has security significance.
            _ => Ok(()),
        }
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        checker: &PermissionChecker,
    ) -> PermissionDecision {
        match input.get("action").and_then(|v| v.as_str()) {
            Some("get") | Some("list_supported") => PermissionDecision::Allow,
            Some("set") => {
                // Validate the key against the allow-list and the
                // security-sensitive tripwire BEFORE routing to the
                // permission rule check. A disallowed key would
                // otherwise surface in the prompter and any audit
                // log even though it was always going to be rejected.
                if let Err(reason) = preflight_set_key(input) {
                    return PermissionDecision::Deny(reason);
                }
                checker.check(self.name(), input)
            }
            _ => checker.check(self.name(), input),
        }
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'action' is required".into()))?;

        match action {
            "list_supported" => Ok(ToolResult::success(format_allowlist())),
            "get" => {
                let key = input
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidInput("'key' is required for get".into()))?;
                let setting = supported_settings::lookup(key).ok_or_else(|| {
                    ToolError::InvalidInput(format!(
                        "setting '{key}' is not on the allow-list. Use action=\"list_supported\" to see what can be read."
                    ))
                })?;
                let value = read_setting(setting, &ctx.cwd)?;
                let value_str = match value {
                    Some(v) => render_toml_value(&v),
                    None => "(unset)".to_string(),
                };
                Ok(ToolResult::success(format!("{key} = {value_str}")))
            }
            "set" => {
                // Validate the key against the allow-list and the
                // security-sensitive tripwire FIRST, before any
                // permission prompter or audit-log surface — a
                // disallowed key must produce an immediate rejection
                // without ever reaching the prompter.
                preflight_set_key(&input).map_err(ToolError::InvalidInput)?;

                let key = input
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidInput("'key' is required for set".into()))?;
                let value = input
                    .get("value")
                    .ok_or_else(|| ToolError::InvalidInput("'value' is required for set".into()))?;

                let setting = supported_settings::lookup(key).ok_or_else(|| {
                    ToolError::InvalidInput(format!(
                        "setting '{key}' is not on the allow-list. Anything not listed by action=\"list_supported\" is intentionally not mutable from a tool call."
                    ))
                })?;

                let coerced = supported_settings::coerce_value(setting, value)
                    .map_err(ToolError::InvalidInput)?;

                // The executor's permission layer is the *only* place
                // we prompt for `set`. Doing it here as well would
                // double-prompt the user, override any "Allow for
                // session" choice they already made, and bypass
                // rule-based grants — permission decisions belong to
                // `check_permissions` / executor, not to the tool body.

                write_setting(setting, &ctx.cwd, coerced.clone())?;

                Ok(ToolResult::success(format!(
                    "Set {} = {} ({})",
                    key,
                    render_toml_value(&coerced),
                    scope_label(setting.scope)
                )))
            }
            other => Err(ToolError::InvalidInput(format!(
                "unknown action '{other}' (expected get, set, or list_supported)"
            ))),
        }
    }
}

/// Validate a `set`-action input by reading `key` and ensuring it is
/// present, well-shaped, NOT in a security-sensitive section, NOT an
/// API-key key, and on the allow-list. Returns the rejection message
/// on failure so callers can map it into either `ToolError::InvalidInput`
/// (when called inside `call`) or `PermissionDecision::Deny` (when
/// called from `check_permissions`).
///
/// Runs as the very first step of `set` handling — before any
/// prompter, audit log, or filesystem touch — so that disallowed
/// keys produce identical rejection messages on both paths and never
/// surface in the permission preview.
fn preflight_set_key(input: &serde_json::Value) -> Result<(), String> {
    let key = input
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "'key' is required for set".to_string())?;

    if supported_settings::is_security_sensitive_section(key) {
        return Err(format!(
            "setting '{key}' is in a security-sensitive section and cannot be set via this tool. Edit the config file by hand if you really need this."
        ));
    }
    if supported_settings::any_segment_matches_api_key(key) {
        return Err(format!(
            "setting '{key}' looks like an API key — those must be supplied via environment variables or the api_key_helper command, not written through this tool."
        ));
    }
    if supported_settings::lookup(key).is_none() {
        return Err(format!(
            "setting '{key}' is not on the allow-list. Anything not listed by action=\"list_supported\" is intentionally not mutable from a tool call."
        ));
    }
    Ok(())
}

/// Resolve the on-disk TOML file path for a setting's scope. `User`
/// always points at `~/.config/agent-code/config.toml`; `Project`
/// walks up from `cwd` looking for an existing `.agent/settings.toml`,
/// and falls back to `<cwd>/.agent/settings.toml` if none exists yet.
fn settings_path_for(scope: Scope, cwd: &Path) -> Option<PathBuf> {
    match scope {
        Scope::User => crate::config::user_config_path(),
        Scope::Project => match crate::config::find_project_config_from(cwd) {
            Some(p) => Some(p),
            None => Some(cwd.join(".agent").join("settings.toml")),
        },
    }
}

/// Read one allow-listed setting from its scope's TOML file. Returns
/// `Ok(None)` if the file or key doesn't exist — the caller renders
/// that as `(unset)`. Wrong-typed values produce an error so a
/// hand-edited file with the wrong shape doesn't silently coerce.
fn read_setting(setting: &SupportedSetting, cwd: &Path) -> Result<Option<toml::Value>, ToolError> {
    let path = match settings_path_for(setting.scope, cwd) {
        Some(p) => p,
        None => return Ok(None),
    };
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| ToolError::ExecutionFailed(format!("read {path:?}: {e}")))?;
    let doc: toml::Value = toml::from_str(&raw)
        .map_err(|e| ToolError::ExecutionFailed(format!("parse {path:?}: {e}")))?;

    let mut cur = &doc;
    for segment in setting.key.split('.') {
        match cur.get(segment) {
            Some(v) => cur = v,
            None => return Ok(None),
        }
    }
    if !value_matches_kind(cur, &setting.kind) {
        return Err(ToolError::ExecutionFailed(format!(
            "value at {} has the wrong type for kind {:?}",
            setting.key, setting.kind
        )));
    }
    Ok(Some(cur.clone()))
}

/// Write a coerced [`toml::Value`] into the scope's TOML file,
/// creating intermediate tables and the file itself as needed. The
/// whole read-modify-write critical section runs under an exclusive
/// advisory lock on a sibling `.lock` file, so two agent processes
/// (or two threads) racing different keys can't both read the same
/// baseline and silently drop one of the writes.
///
/// The actual filesystem mutation goes through
/// [`crate::config::atomic::atomic_write_secret`], which preserves the
/// destination's mode (or defaults to `0600` for new files — these
/// can hold secrets).
fn write_setting(
    setting: &SupportedSetting,
    cwd: &Path,
    value: toml::Value,
) -> Result<(), ToolError> {
    let path = settings_path_for(setting.scope, cwd)
        .ok_or_else(|| ToolError::ExecutionFailed("could not determine settings path".into()))?;

    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| ToolError::ExecutionFailed(format!("create {parent:?}: {e}")))?;
    }

    // Sibling lockfile — avoids contending the on-disk settings file
    // itself (which would prevent a clean `rename` over the top of an
    // open handle on Windows). The lockfile is created on demand and
    // left in place; an empty lockfile is harmless and lets future
    // writers reuse it without a recreate-race.
    let lock_path = path.with_extension("toml.lock");
    let _guard = SettingsLockGuard::acquire(&lock_path)?;

    let mut doc: toml::Value = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| ToolError::ExecutionFailed(format!("read {path:?}: {e}")))?;
        toml::from_str(&raw)
            .map_err(|e| ToolError::ExecutionFailed(format!("parse {path:?}: {e}")))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    set_dotted(&mut doc, setting.key, value)?;

    let serialized = toml::to_string_pretty(&doc)
        .map_err(|e| ToolError::ExecutionFailed(format!("serialize: {e}")))?;
    atomic_write_secret(&path, serialized.as_bytes())
        .map_err(|e| ToolError::ExecutionFailed(format!("atomic write {path:?}: {e}")))?;
    Ok(())
}

/// RAII handle to the per-settings-file advisory lock. Drop releases
/// the OS-level `flock` (POSIX) / `LockFile` (Windows) via `fs2`,
/// which means a panic inside the read-modify-write section never
/// leaves a stale lock behind.
///
/// We deliberately use `fs2::FileExt::lock_exclusive` rather than a
/// custom PID file: the OS-managed advisory lock is automatically
/// released by the kernel if the holding process dies, so there is
/// no stale-lock cleanup story to write. The lockfile itself is just
/// the inode `flock` is associated with — its contents are unused.
struct SettingsLockGuard {
    _file: std::fs::File,
}

impl SettingsLockGuard {
    fn acquire(lock_path: &Path) -> Result<Self, ToolError> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| ToolError::ExecutionFailed(format!("open lockfile {lock_path:?}: {e}")))?;
        // Blocking acquire: short read-modify-write, no deadline. If
        // a stuck holder were ever a real concern we'd swap to
        // `try_lock_exclusive` with a timeout — but for two-agent
        // contention on the same settings file the wait is bounded
        // by the other writer's own (fast) atomic write.
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|e| ToolError::ExecutionFailed(format!("lock {lock_path:?}: {e}")))?;
        Ok(Self { _file: file })
    }
}

impl Drop for SettingsLockGuard {
    fn drop(&mut self) {
        // `unlock` may fail if e.g. the file descriptor is already
        // closed by the OS — there's nothing useful to do with the
        // error at drop time, and the kernel will release the lock
        // on process exit regardless.
        let _ = fs2::FileExt::unlock(&self._file);
    }
}

/// Insert `value` at the dotted `key` path inside a TOML document,
/// creating any missing tables along the way. Returns an error if a
/// non-table value sits in the path — we refuse to clobber unrelated
/// scalars even though the caller picked an allow-listed key, because
/// the conflict means the file was hand-edited into an unexpected
/// shape and the user should resolve it.
fn set_dotted(doc: &mut toml::Value, key: &str, value: toml::Value) -> Result<(), ToolError> {
    let segments: Vec<&str> = key.split('.').collect();
    if segments.is_empty() {
        return Err(ToolError::InvalidInput("empty key".into()));
    }
    let mut cursor = doc;
    for seg in &segments[..segments.len() - 1] {
        let cursor_table = cursor.as_table_mut().ok_or_else(|| {
            ToolError::ExecutionFailed(format!(
                "cannot descend into non-table at '{seg}' while setting {key}"
            ))
        })?;
        let entry = cursor_table
            .entry((*seg).to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
        if !entry.is_table() {
            return Err(ToolError::ExecutionFailed(format!(
                "key segment '{seg}' is not a table; refusing to overwrite"
            )));
        }
        cursor = entry;
    }
    let leaf = segments.last().unwrap();
    let table = cursor
        .as_table_mut()
        .ok_or_else(|| ToolError::ExecutionFailed("expected table at leaf parent".into()))?;
    table.insert((*leaf).to_string(), value);
    Ok(())
}

fn value_matches_kind(value: &toml::Value, kind: &SettingKind) -> bool {
    match kind {
        SettingKind::Bool => value.is_bool(),
        SettingKind::Int => value.is_integer(),
        SettingKind::Float => value.is_float(),
        SettingKind::String => value.is_str(),
        SettingKind::Enum(allowed) => value.as_str().is_some_and(|s| allowed.contains(&s)),
    }
}

fn render_toml_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "user scope",
        Scope::Project => "project scope",
    }
}

fn format_allowlist() -> String {
    let mut out = String::from("Supported settings (allow-list):\n");
    for s in supported_settings::SUPPORTED_SETTINGS {
        out.push_str(&format!(
            "- {} [{}, {}] - {}\n",
            s.key,
            kind_label(&s.kind),
            scope_label(s.scope),
            s.description
        ));
    }
    out
}

fn kind_label(kind: &SettingKind) -> String {
    match kind {
        SettingKind::Bool => "bool".to_string(),
        SettingKind::Int => "int".to_string(),
        SettingKind::Float => "float".to_string(),
        SettingKind::String => "string".to_string(),
        SettingKind::Enum(values) => format!("enum({})", values.join("|")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn make_ctx(cwd: PathBuf) -> ToolContext {
        ToolContext {
            cwd,
            cancel: CancellationToken::new(),
            permission_checker: Arc::new(PermissionChecker::allow_all()),
            verbose: false,
            plan_mode: false,
            file_cache: None,
            denial_tracker: None,
            task_manager: None,
            subagent_colors: None,
            session_allows: None,
            permission_prompter: None,
            question_asker: None,
            agent_origin: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
        }
    }

    use crate::test_support::EnvGuard;

    #[tokio::test]
    async fn list_supported_returns_known_keys() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let res = ConfigTool
            .call(json!({ "action": "list_supported" }), &ctx)
            .await
            .unwrap();
        assert!(res.content.contains("ui.theme"));
        assert!(res.content.contains("features.commit_attribution"));
    }

    #[tokio::test]
    async fn get_unset_project_scope_returns_unset_marker() {
        // Project-scope reads avoid the global XDG_CONFIG_HOME, so
        // this test doesn't need any env shenanigans.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".agent")).unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let res = ConfigTool
            .call(json!({ "action": "get", "key": "api.model" }), &ctx)
            .await
            .unwrap();
        assert!(res.content.contains("(unset)"));
    }

    #[tokio::test]
    async fn set_rejects_unlisted_key() {
        // `permissions.default_mode` is in a security-sensitive
        // section, so it's rejected by the up-front tripwire even
        // before the allow-list lookup runs.
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());

        let err = ConfigTool
            .call(
                json!({
                    "action": "set",
                    "key": "permissions.default_mode",
                    "value": "allow"
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => {
                assert!(s.contains("security-sensitive") || s.contains("not on the allow-list"));
            }
            _ => panic!("expected InvalidInput"),
        }

        // Also exercise a key that isn't on the allow-list but lives
        // outside any blocked section, to make sure the allow-list
        // check itself still rejects it.
        let err = ConfigTool
            .call(
                json!({
                    "action": "set",
                    "key": "telemetry.endpoint",
                    "value": "https://example",
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("not on the allow-list")),
            _ => panic!("expected InvalidInput"),
        }
    }

    #[tokio::test]
    async fn set_rejects_wrong_type() {
        // Ride on a project-scope key (api.model) so we don't have
        // to mutate XDG_CONFIG_HOME — but use a key whose kind
        // forces a type error. Override the test to use a bool key
        // but with project scope: we exercise the validator BEFORE
        // any disk write, so the scope of the key is irrelevant for
        // this assertion. ui.markdown happens to be user-scope, but
        // the InvalidInput is raised before the path is touched.
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());

        let err = ConfigTool
            .call(
                json!({ "action": "set", "key": "ui.markdown", "value": "true" }),
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("boolean")),
            _ => panic!("expected InvalidInput"),
        }
    }

    #[tokio::test]
    async fn set_rejects_enum_outside_allowed() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());

        let err = ConfigTool
            .call(
                json!({ "action": "set", "key": "ui.theme", "value": "magenta" }),
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("must be one of")),
            _ => panic!("expected InvalidInput"),
        }
    }

    #[tokio::test]
    async fn set_project_scope_writes_then_get_reads_back() {
        // End-to-end set+get round-trip on the project scope, which
        // is a per-test temp directory and therefore can't race with
        // any other test in the workspace.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".agent")).unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());

        let set = ConfigTool
            .call(
                json!({ "action": "set", "key": "api.model", "value": "gpt-5.4" }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(set.content.contains("project scope"));
        let path = dir.path().join(".agent").join("settings.toml");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("model = \"gpt-5.4\""));

        let get = ConfigTool
            .call(json!({ "action": "get", "key": "api.model" }), &ctx)
            .await
            .unwrap();
        assert!(get.content.contains("\"gpt-5.4\""));
    }

    /// Smoke-test the user-scope set/get path under a guarded env
    /// override. Wrapped in a process-wide mutex so concurrent tests
    /// reading the user config can't see a half-applied value.
    ///
    /// `XDG_CONFIG_HOME` is honored on every platform via the
    /// [`crate::config::agent_config_dir`] helper, so a single env
    /// override is hermetic on Linux, macOS, and Windows alike.
    #[tokio::test]
    async fn set_user_scope_writes_to_xdg_config_home() {
        let xdg = TempDir::new().unwrap();
        let _g = EnvGuard::set("XDG_CONFIG_HOME", xdg.path());
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());

        let set = ConfigTool
            .call(
                json!({ "action": "set", "key": "ui.theme", "value": "light" }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!set.is_error);
        let path = xdg.path().join("agent-code").join("config.toml");
        assert!(path.exists(), "user config file was not created");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("theme = \"light\""));
    }

    #[tokio::test]
    async fn unknown_action_is_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let err = ConfigTool
            .call(json!({ "action": "delete", "key": "ui.theme" }), &ctx)
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("unknown action")),
            _ => panic!("expected InvalidInput"),
        }
    }

    #[tokio::test]
    async fn check_permissions_allows_read_only_actions() {
        let checker = PermissionChecker::allow_all();
        let tool = ConfigTool;
        let dec = tool
            .check_permissions(&json!({ "action": "list_supported" }), &checker)
            .await;
        assert!(matches!(dec, PermissionDecision::Allow));
        let dec = tool
            .check_permissions(&json!({ "action": "get", "key": "ui.theme" }), &checker)
            .await;
        assert!(matches!(dec, PermissionDecision::Allow));
    }

    #[tokio::test]
    async fn check_permissions_denies_security_sensitive_set_before_prompt() {
        // The disallowed key must be rejected at the permission stage
        // — never reaching the prompter or any audit log surface.
        let checker = PermissionChecker::allow_all();
        let tool = ConfigTool;
        let dec = tool
            .check_permissions(
                &json!({
                    "action": "set",
                    "key": "permissions.default_mode",
                    "value": "allow",
                }),
                &checker,
            )
            .await;
        match dec {
            PermissionDecision::Deny(reason) => {
                assert!(reason.contains("security-sensitive"));
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn check_permissions_denies_unlisted_set_before_prompt() {
        let checker = PermissionChecker::allow_all();
        let tool = ConfigTool;
        let dec = tool
            .check_permissions(
                &json!({
                    "action": "set",
                    "key": "telemetry.endpoint",
                    "value": "https://example",
                }),
                &checker,
            )
            .await;
        match dec {
            PermissionDecision::Deny(reason) => {
                assert!(reason.contains("not on the allow-list"));
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn check_permissions_denies_api_key_segment_before_prompt() {
        let checker = PermissionChecker::allow_all();
        let tool = ConfigTool;
        let dec = tool
            .check_permissions(
                &json!({
                    "action": "set",
                    "key": "ui.theme.api_key_color",
                    "value": "#000",
                }),
                &checker,
            )
            .await;
        match dec {
            PermissionDecision::Deny(reason) => {
                assert!(reason.contains("API key"));
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    /// Permission prompter that records each invocation so a test can
    /// assert it was (or was not) asked.
    struct RecordingPrompter {
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingPrompter {
        fn new() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn invocations(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl super::super::PermissionPrompter for RecordingPrompter {
        fn ask(
            &self,
            tool_name: &str,
            _description: &str,
            input_preview: Option<&str>,
            _origin: Option<&str>,
        ) -> super::super::PermissionResponse {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{tool_name}::{}", input_preview.unwrap_or("")));
            super::super::PermissionResponse::AllowOnce
        }
    }

    #[tokio::test]
    async fn set_disallowed_key_does_not_invoke_prompter() {
        // Defence-in-depth: even via the `call` path (which mirrors
        // what the executor does under `Allow`), a disallowed key must
        // never reach a prompter. The tool body no longer prompts
        // (the executor owns prompting), but a future regression that
        // re-introduces an in-tool prompt would still need to skip
        // disallowed keys — this test would catch that.
        let dir = TempDir::new().unwrap();
        let prompter = Arc::new(RecordingPrompter::new());
        let mut ctx = make_ctx(dir.path().to_path_buf());
        ctx.permission_prompter = Some(prompter.clone());

        let err = ConfigTool
            .call(
                json!({
                    "action": "set",
                    "key": "permissions.default_mode",
                    "value": "allow",
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("security-sensitive")),
            _ => panic!("expected InvalidInput"),
        }
        assert!(
            prompter.invocations().is_empty(),
            "prompter must not be invoked for a disallowed key, got: {:?}",
            prompter.invocations()
        );
    }

    /// The tool body must NEVER invoke the permission prompter — that
    /// responsibility lives with the executor's permission layer.
    /// Calling the prompter from inside `call` would double-prompt,
    /// override session-level "Allow for session" decisions, and
    /// bypass rule-based grants. A successful set on a legitimate
    /// project-scope key must complete without ever asking.
    #[tokio::test]
    async fn set_legitimate_key_does_not_invoke_prompter_from_tool_body() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".agent")).unwrap();
        let prompter = Arc::new(RecordingPrompter::new());
        let mut ctx = make_ctx(dir.path().to_path_buf());
        ctx.permission_prompter = Some(prompter.clone());

        let res = ConfigTool
            .call(
                json!({
                    "action": "set",
                    "key": "api.model",
                    "value": "gpt-5.4",
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!res.is_error);
        assert!(
            prompter.invocations().is_empty(),
            "tool body must defer prompting to the executor, got: {:?}",
            prompter.invocations()
        );
    }

    /// `validate_input` is the engine-level rejection point: it runs
    /// before PreToolUse hooks fire and before the permission prompt.
    /// Disallowed keys must surface here so audit hooks never see
    /// them.
    #[test]
    fn validate_input_rejects_security_sensitive_set() {
        let tool = ConfigTool;
        let err = tool
            .validate_input(&json!({
                "action": "set",
                "key": "permissions.default_mode",
                "value": "allow",
            }))
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("security-sensitive")),
            _ => panic!("expected InvalidInput"),
        }
    }

    #[test]
    fn validate_input_allows_get_and_list_supported() {
        let tool = ConfigTool;
        // Read-only actions are not interesting at validate_input
        // time — any per-action checks happen inside `call`.
        assert!(tool.validate_input(&json!({ "action": "get" })).is_ok());
        assert!(
            tool.validate_input(&json!({ "action": "list_supported" }))
                .is_ok()
        );
    }

    /// Two writers racing different keys against the same user-scope
    /// settings file must NOT clobber each other. Without the per-file
    /// advisory lock both threads would read the same baseline, both
    /// would compute their own mutated `doc`, and the loser's key
    /// would disappear. The lock around the read-modify-write critical
    /// section serialises them so both writes land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_set_calls_persist_both_keys() {
        // Both writers target distinct user-scope keys against the
        // same on-disk settings file. `XDG_CONFIG_HOME` is honored
        // on every platform via the [`crate::config::agent_config_dir`]
        // helper. The guard's mutex prevents any other test from
        // reading user config mid-flight.
        let xdg = TempDir::new().unwrap();
        let _g = EnvGuard::set("XDG_CONFIG_HOME", xdg.path());
        let cwd_dir = TempDir::new().unwrap();
        let cwd: PathBuf = cwd_dir.path().to_path_buf();

        let cwd_a = cwd.clone();
        let cwd_b = cwd.clone();
        let a = tokio::spawn(async move {
            let ctx = make_ctx(cwd_a);
            ConfigTool
                .call(
                    json!({ "action": "set", "key": "ui.theme", "value": "light" }),
                    &ctx,
                )
                .await
        });
        let b = tokio::spawn(async move {
            let ctx = make_ctx(cwd_b);
            ConfigTool
                .call(
                    json!({
                        "action": "set",
                        "key": "features.commit_attribution",
                        "value": true,
                    }),
                    &ctx,
                )
                .await
        });
        a.await.unwrap().unwrap();
        b.await.unwrap().unwrap();

        let path = xdg.path().join("agent-code").join("config.toml");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("theme = \"light\""),
            "writer A's key was lost: {contents}"
        );
        assert!(
            contents.contains("commit_attribution = true"),
            "writer B's key was lost: {contents}"
        );
    }

    #[tokio::test]
    async fn atomic_write_leaves_original_intact_when_rename_target_is_unwritable() {
        // If the destination directory is read-only at rename time,
        // the temp file write+rename fails and the existing settings
        // file (if any) must still be intact. This is the property
        // that distinguishes atomic write from `std::fs::write` —
        // a half-written file would corrupt the user's settings.
        //
        // We simulate the failure by pointing the project-scope
        // settings path at a directory we then chmod 0o500 (read+exec
        // only) — `rename` into it will fail with EACCES.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = TempDir::new().unwrap();
            let project_root = dir.path();
            let agent_dir = project_root.join(".agent");
            std::fs::create_dir_all(&agent_dir).unwrap();
            let settings = agent_dir.join("settings.toml");
            std::fs::write(&settings, b"[api]\nmodel = \"original\"\n").unwrap();
            let original = std::fs::read(&settings).unwrap();

            // Make the `.agent/` directory read-only so creating the
            // temp file inside it fails outright.
            let mut perms = std::fs::metadata(&agent_dir).unwrap().permissions();
            perms.set_mode(0o500);
            std::fs::set_permissions(&agent_dir, perms).unwrap();

            // Root (and some sandboxes) bypass directory write bits, so the
            // failure this test depends on never happens — probe and skip
            // instead of failing on such runners.
            if std::fs::write(agent_dir.join(".probe"), b"x").is_ok() {
                let _ = std::fs::remove_file(agent_dir.join(".probe"));
                eprintln!("skipping: read-only dir not enforced (running as root?)");
                return;
            }

            let ctx = make_ctx(project_root.to_path_buf());
            let res = ConfigTool
                .call(
                    json!({
                        "action": "set",
                        "key": "api.model",
                        "value": "should-not-land",
                    }),
                    &ctx,
                )
                .await;

            // Restore write perm so TempDir can clean up regardless
            // of test outcome.
            let mut perms = std::fs::metadata(&agent_dir).unwrap().permissions();
            perms.set_mode(0o700);
            std::fs::set_permissions(&agent_dir, perms).unwrap();

            assert!(res.is_err(), "write should fail with read-only parent");

            // The original file must be untouched.
            let after = std::fs::read(&settings).unwrap();
            assert_eq!(after, original, "atomic write must not corrupt original");
        }
    }
}
