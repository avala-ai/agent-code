//! Configuration system.
//!
//! Configuration is loaded from multiple sources with the following
//! priority (highest to lowest):
//!
//! 1. CLI flags and environment variables
//! 2. Project-local overrides (`.agent/settings.local.toml`) — gitignored
//! 3. Project settings (`.agent/settings.toml`)
//! 4. User settings (`~/.config/agent-code/config.toml`)
//!
//! Each layer is merged into the final `Config` struct. The `.local.toml`
//! tier is intended for machine-specific or developer-specific overrides
//! that shouldn't be committed (e.g. a personal API base URL while keeping
//! the team's shared `settings.toml` intact).

pub mod atomic;
pub mod migrations;
mod schema;
pub mod supported_settings;

pub use schema::*;

use crate::error::ConfigError;
use std::path::{Path, PathBuf};

/// Re-entrancy guard to prevent Config::load → log → Config::load cycles.
static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Config {
    /// Load configuration from all sources, merging by priority.
    pub fn load() -> Result<Config, ConfigError> {
        // Re-entrancy guard.
        if LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(Config::default());
        }
        let result = Self::load_inner(None, true);
        LOADING.store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    /// Load configuration as it would be seen *from* `dir`, without
    /// entering it.
    ///
    /// The project and project-local layers are discovered by walking up
    /// from the working directory, so reading another project's settings
    /// otherwise meant chdir-ing into it and back — which mutates global
    /// state for every thread and leaves the process in the wrong place
    /// if the return trip fails. Resuming a session saved elsewhere needs
    /// to answer "would this project's settings parse" before committing
    /// to it, so it needs this.
    ///
    /// User-level and environment layers are unchanged: they are not
    /// per-project.
    pub fn load_from(dir: &Path) -> Result<Config, ConfigError> {
        if LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(Config::default());
        }
        let result = Self::load_inner(Some(dir), true);
        LOADING.store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    /// Load another project's *policy* without resolving its API key.
    ///
    /// [`Self::load_from`] runs the destination's `api_key_helper`
    /// through `bash -c`. A resume preflight only reads policy and may
    /// still refuse the resume, so running it there would execute a
    /// command from a project the session never enters — and block the
    /// caller (the TUI event loop, Ctrl+C included) while it does.
    ///
    /// The returned `api.api_key` is therefore whatever the files and
    /// environment gave, with no helper invocation. Callers that need a
    /// usable key must use [`Self::load_from`].
    pub fn load_policy_from(dir: &Path) -> Result<Config, ConfigError> {
        if LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(Config::default());
        }
        let result = Self::load_inner(Some(dir), false);
        LOADING.store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    /// `start` is the directory the project layers are discovered from;
    /// `None` means the process working directory.
    fn load_inner(start: Option<&Path>, resolve_api_key: bool) -> Result<Config, ConfigError> {
        let mut layers: Vec<String> = Vec::new();

        // Layer 1: User-level config (lowest priority file).
        if let Some(path) = user_config_path()
            && path.exists()
        {
            layers.push(read_layer_through_migrations(&path)?);
        }

        // Layer 2: Project-level config (overrides user config).
        let project = match start {
            Some(dir) => find_project_config_from(dir),
            None => find_project_config(),
        };
        if let Some(path) = project {
            layers.push(read_layer_through_migrations(&path)?);
        }

        // Layer 2.5: Project-local overrides (gitignored, overrides
        // committed project settings). Sits next to `settings.toml` as
        // `settings.local.toml` in the same `.agent/` directory; only
        // the one closest to cwd is used, matching the project-config
        // walk so sub-packages inherit the repo-root settings.local.
        let project_local = match start {
            Some(dir) => find_local_config_in_ancestors(dir),
            None => find_project_local_config(),
        };
        if let Some(path) = project_local {
            layers.push(read_layer_through_migrations(&path)?);
        }

        let layer_refs: Vec<&str> = layers.iter().map(String::as_str).collect();
        let mut config = merge_layer_contents(&layer_refs)?;

        // Layer 3: Environment variables override file-based config.
        // Base URL / model / auth mode apply first — the key chosen
        // below prefers the provider they select.
        if let Ok(url) = std::env::var("AGENT_CODE_API_BASE_URL") {
            config.api.base_url = url;
        }
        if let Ok(auth_mode) = std::env::var("AGENT_CODE_AUTH_MODE") {
            config.api.auth_mode = toml::Value::String(auth_mode).try_into()?;
        }
        if let Ok(model) = std::env::var("AGENT_CODE_MODEL") {
            config.api.model = model;
        }

        // CODEX_HOME is honored by the auth loader when codex_home is unset;
        // this env var lets agent-code pin a different Codex home explicitly.
        if let Ok(codex_home) = std::env::var("AGENT_CODE_CODEX_HOME") {
            config.api.codex_home = Some(codex_home);
        }

        // API key from env always wins over config files, because users
        // expect `OPENAI_API_KEY=x agent` to use key x, even if a
        // stale key exists in config.toml. When several provider keys
        // are exported, the one belonging to the effective provider
        // (from base_url/model) is preferred over the fixed priority
        // order, so the session never sends provider A's key to
        // provider B's endpoint.
        let env_api_key = if let Some(var) =
            choose_api_key_var(&config.api.base_url, &config.api.model, |v| {
                std::env::var(v).is_ok_and(|s| !s.trim().is_empty())
            }) {
            std::env::var(var).ok()
        } else {
            resolve_api_key_from_env()
        };
        if env_api_key.is_some() {
            config.api.api_key = env_api_key;
        }

        // Layer 4: Dynamic API key helper.
        //
        // If no API key was resolved from file or env, run the
        // user-configured `api_key_helper` command (via `bash -c`) and
        // use its trimmed stdout as the key. Allows fetching short-lived
        // tokens from a secrets manager without pinning them to disk.
        if resolve_api_key
            && config.api.api_key.is_none()
            && let Some(cmd) = &config.api.api_key_helper
        {
            match resolve_api_key_from_helper(cmd) {
                Ok(key) if !key.is_empty() => config.api.api_key = Some(key),
                Ok(_) => {
                    tracing::warn!("api_key_helper produced empty output — no key set");
                }
                Err(e) => {
                    // Log only the category of failure — never the
                    // raw error text, which could carry helper stdout
                    // or stderr that contains the key itself.
                    tracing::warn!("api_key_helper failed: {}", e.category());
                }
            }
        }

        Ok(config)
    }
}

/// Read one config-file layer, routing it through the schema-version
/// migration runner before handing the bytes to the TOML merge stage.
///
/// Production config files are TOML, so we drive the
/// [`migrations::load_and_migrate_toml`] entry point: it parses the
/// TOML, converts to the JSON shape the migration chain expects, runs
/// any pending migrations, atomically rewrites the file when the chain
/// mutated it (rotating `.bak.N` slots after a successful rename), and
/// hands back the migrated `toml::Value`. We then re-serialize to a
/// String so the rest of `load_inner` keeps its current "merge raw
/// TOML strings" pipeline unchanged.
///
/// The on-disk format is TOML; migrations are pure functions over
/// `serde_json::Value`. The conversion at the boundary is loss-bearing
/// only for TOML datetimes (no settings field uses one today) — see
/// `migrations::load_and_migrate_toml` docs.
fn read_layer_through_migrations(path: &Path) -> Result<String, ConfigError> {
    let (migrated, _outcome) = migrations::load_and_migrate_toml(path)
        .map_err(|e| ConfigError::FileError(format!("{path:?}: migration failed: {e:#}")))?;
    toml::to_string_pretty(&migrated)
        .map_err(|e| ConfigError::FileError(format!("{path:?}: re-serializing TOML: {e}")))
}

/// Why an `api_key_helper` invocation failed. Deliberately coarse so
/// callers can log a category string without risking the key itself
/// (or stderr that carries it) showing up in diagnostic output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiKeyHelperError {
    /// Could not spawn the subprocess (e.g. `bash` missing, permission).
    SpawnFailed,
    /// Subprocess ran but exited non-zero.
    NonZeroExit,
    /// Stdout was not valid UTF-8.
    InvalidUtf8,
}

impl ApiKeyHelperError {
    pub(crate) fn category(self) -> &'static str {
        match self {
            Self::SpawnFailed => "spawn failed",
            Self::NonZeroExit => "non-zero exit",
            Self::InvalidUtf8 => "invalid UTF-8 output",
        }
    }
}

/// Run a user-configured shell command and return its trimmed stdout
/// as the API key. Returns a categorized error if the command fails
/// to spawn, exits non-zero, or produces non-UTF-8 output.
///
/// Error variants intentionally do NOT carry subprocess stdout or
/// stderr — either could contain the API key itself, so callers must
/// not log it.
pub(crate) fn resolve_api_key_from_helper(command: &str) -> Result<String, ApiKeyHelperError> {
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|_| ApiKeyHelperError::SpawnFailed)?;

    if !output.status.success() {
        return Err(ApiKeyHelperError::NonZeroExit);
    }

    let key = String::from_utf8(output.stdout).map_err(|_| ApiKeyHelperError::InvalidUtf8)?;
    Ok(key.trim().to_string())
}

/// Merge a sequence of TOML config layers (lowest → highest priority) into a
/// typed `Config`. Layers are merged at the raw `toml::Value` level *before*
/// typed deserialization so that `#[serde(default)]` cannot synthesize
/// placeholder sections that clobber real values from lower layers
/// (see issue #101). The final `try_into` runs exactly once, so defaults
/// only fill fields nobody set in any layer.
///
/// `permissions.rules` has extend-semantics (layers concatenate rather than
/// replace), implemented by pulling each layer's rules aside and splicing
/// them back after the recursive merge.
pub(crate) fn merge_layer_contents(layers: &[&str]) -> Result<Config, ConfigError> {
    let mut merged = toml::Value::Table(toml::value::Table::new());
    let mut all_rules: Vec<toml::Value> = Vec::new();

    for content in layers {
        if content.is_empty() {
            continue;
        }
        let value: toml::Value = toml::from_str(content)?;
        collect_permission_rules(&value, &mut all_rules);
        merge_toml_values(&mut merged, &value);
    }

    if !all_rules.is_empty()
        && let toml::Value::Table(root) = &mut merged
    {
        let perms = root
            .entry("permissions".to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
        if let toml::Value::Table(pt) = perms {
            pt.insert("rules".to_string(), toml::Value::Array(all_rules));
        }
    }

    Ok(merged.try_into()?)
}

/// Recursively merge `overlay` into `base`. Tables merge key-by-key; any
/// non-table value in `overlay` replaces the value in `base`. Adapted from
/// openai/codex's `merge_toml_values`.
fn merge_toml_values(base: &mut toml::Value, overlay: &toml::Value) {
    if let toml::Value::Table(overlay_table) = overlay
        && let toml::Value::Table(base_table) = base
    {
        for (key, value) in overlay_table {
            if let Some(existing) = base_table.get_mut(key) {
                merge_toml_values(existing, value);
            } else {
                base_table.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = overlay.clone();
    }
}

fn collect_permission_rules(value: &toml::Value, out: &mut Vec<toml::Value>) {
    if let Some(rules) = value
        .get("permissions")
        .and_then(|p| p.get("rules"))
        .and_then(|r| r.as_array())
    {
        out.extend(rules.iter().cloned());
    }
}

/// Provider API-key environment variables, highest priority first.
///
/// This is the single source of truth for which env var wins when
/// several are exported. Anything that persists provider defaults
/// based on env credentials (e.g. the first-run bootstrap in
/// `crates/cli/src/ui/setup.rs`) must follow the same order, or the
/// persisted `base_url`/`model` can belong to a different provider
/// than the key this resolver picks at load time.
pub const API_KEY_ENV_VARS: &[&str] = &[
    "AGENT_CODE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "XAI_API_KEY",
    "GOOGLE_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "ZHIPU_API_KEY",
    "TOGETHER_API_KEY",
    "OPENROUTER_API_KEY",
    "COHERE_API_KEY",
    "PERPLEXITY_API_KEY",
];

/// Resolve API key from environment variables.
///
/// Checks each provider's env var in priority order. Returns the first
/// one found, or None if no API key is set in the environment.
fn resolve_api_key_from_env() -> Option<String> {
    API_KEY_ENV_VARS
        .iter()
        .find_map(|var| std::env::var(var).ok())
}

/// Pick the env var the effective provider's key should come from, or
/// `None` to fall back to the fixed priority order.
///
/// Pure — `has` reports whether a var is set non-empty — so the
/// decision table is testable without touching process env. Rules:
///
/// 1. `AGENT_CODE_API_KEY` keeps its top rank: it is an explicit
///    "use this key" override, relied on for OpenAI-compatible proxies
///    where URL-based detection cannot identify the vendor.
/// 2. When `base_url`/`model` map to a dedicated provider and that
///    provider's key var is set, it wins over other providers' vars.
/// 3. Otherwise (custom/cloud endpoints, or the matching var absent)
///    the caller falls back to [`API_KEY_ENV_VARS`] order — exactly
///    the pre-existing behavior, so single-key setups are unchanged.
fn choose_api_key_var(
    base_url: &str,
    model: &str,
    has: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    use crate::llm::provider::{ProviderKind, detect_provider};

    if has("AGENT_CODE_API_KEY") {
        return Some("AGENT_CODE_API_KEY");
    }
    let kind = detect_provider(model, base_url);
    if matches!(
        kind,
        ProviderKind::OpenAiCompatible
            | ProviderKind::Bedrock
            | ProviderKind::Vertex
            | ProviderKind::AzureOpenAi
    ) {
        return None;
    }
    let var = kind.env_var_name();
    has(var).then_some(var)
}

/// Returns the user-level config file path.
pub fn user_config_path() -> Option<PathBuf> {
    agent_config_dir().map(|d| d.join("config.toml"))
}

/// Returns the root directory that holds every agent-code user-scope
/// config / state file (`config.toml`, `profiles/`, `sessions/`, `skills/`,
/// `plugins/`, `schedules/`, …).
///
/// Resolution order:
///
/// 1. `XDG_CONFIG_HOME` env var, when set and non-empty — joined with
///    `agent-code`. We honor this on every platform, including Windows,
///    so a single env override redirects user-scope state in tests and
///    in POSIX-flavored dev shells. The base `dirs::config_dir()` already
///    consults `XDG_CONFIG_HOME` on Linux but ignores it on Windows
///    (`dirs` reads `%APPDATA%` via `SHGetKnownFolderPath` there);
///    re-checking it here is what fixes the cross-platform gap.
/// 2. `dirs::config_dir()` joined with `agent-code` — the platform
///    default: `$HOME/.config/agent-code` on Linux,
///    `~/Library/Application Support/agent-code` on macOS,
///    `%APPDATA%\agent-code` on Windows.
///
/// Use this in preference to `dirs::config_dir().map(|d| d.join("agent-code"))`
/// so tests and overrides stay consistent across the codebase.
pub fn agent_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("agent-code"));
    }
    dirs::config_dir().map(|d| d.join("agent-code"))
}

/// Raw bytes of a path for hashing, without lossy UTF-8 conversion.
///
/// `to_string_lossy` maps every invalid byte to U+FFFD, which collapses
/// paths that differ only in invalid bytes into one hash input. Anything
/// security-scoped that fingerprints a path (grant filenames, sandbox
/// policy fingerprints) must use this instead.
pub(crate) fn os_path_bytes(path: &std::path::Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

/// Walk up from the current directory to find `.agent/settings.toml`.
pub fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_config_in_ancestors(&cwd)
}

/// Walk up from `start` to find `.agent/settings.toml`. Public so the
/// model-callable [`Config`](crate::tools::config_tool) tool and other
/// helpers can locate the project config relative to a known directory
/// (the tool's `ToolContext::cwd`) instead of `current_dir`.
pub fn find_project_config_from(start: &Path) -> Option<PathBuf> {
    find_config_in_ancestors(start)
}

/// Walk up from `start` to find the project root that contains `.agent/`.
/// Returns the directory itself (parent of `.agent/`), not the settings
/// file path.
pub fn find_project_root_from(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".agent").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Walk up from the current directory to find `.agent/settings.local.toml`.
///
/// This is the gitignored overlay that sits on top of the shared
/// `settings.toml`. Kept as a separate walk (not just a sibling lookup
/// next to the resolved `settings.toml`) so a developer can place a
/// `settings.local.toml` at a different ancestor level than the
/// committed settings — for example, a repo-root `settings.toml` plus
/// a crate-local `settings.local.toml` overriding just one field.
pub(crate) fn find_project_local_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_local_config_in_ancestors(&cwd)
}

/// Watch config files for changes and reload when modified.
/// Returns a handle that can be dropped to stop watching.
pub fn watch_config(
    on_reload: impl Fn(Config) + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    let user_path = user_config_path()?;
    let project_path = find_project_config();
    let local_path = find_project_local_config();

    // Get initial mtimes.
    let user_mtime = std::fs::metadata(&user_path)
        .ok()
        .and_then(|m| m.modified().ok());
    let project_mtime = project_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let local_mtime = local_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());

    Some(std::thread::spawn(move || {
        let mut last_user = user_mtime;
        let mut last_project = project_mtime;
        let mut last_local = local_mtime;

        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));

            let cur_user = std::fs::metadata(&user_path)
                .ok()
                .and_then(|m| m.modified().ok());
            let cur_project = project_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok());
            let cur_local = local_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok());

            let changed =
                cur_user != last_user || cur_project != last_project || cur_local != last_local;

            if changed {
                if let Ok(config) = Config::load() {
                    tracing::info!("Config reloaded (file change detected)");
                    on_reload(config);
                }
                last_user = cur_user;
                last_project = cur_project;
                last_local = cur_local;
            }
        }
    }))
}

fn find_config_in_ancestors(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".agent").join("settings.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn find_local_config_in_ancestors(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".agent").join("settings.local.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn merge_layers(user: &str, project: &str) -> Config {
        merge_layer_contents(&[user, project]).unwrap()
    }

    // ---- Issue #101: project config without [api] must not clobber user api ----

    #[test]
    fn project_without_api_section_preserves_user_base_url_and_model() {
        let user = r#"
[api]
base_url = "http://localhost:11434/v1"
model = "gemma4:26b"
"#;
        let project = r#"
[mcp_servers.my-server]
command = "/usr/local/bin/my-mcp"
args = []
"#;
        let cfg = merge_layers(user, project);
        assert_eq!(cfg.api.base_url, "http://localhost:11434/v1");
        assert_eq!(cfg.api.model, "gemma4:26b");
        assert!(cfg.mcp_servers.contains_key("my-server"));
    }

    #[test]
    fn project_partial_api_only_overrides_specified_fields() {
        let user = r#"
[api]
base_url = "http://localhost:11434/v1"
model = "gemma4:26b"
"#;
        let project = r#"
[api]
model = "llama3:70b"
"#;
        let cfg = merge_layers(user, project);
        // Project overrides model.
        assert_eq!(cfg.api.model, "llama3:70b");
        // base_url is inherited from user, not clobbered by default.
        assert_eq!(cfg.api.base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn project_without_ui_section_preserves_user_theme() {
        let user = r#"
[ui]
theme = "solarized"
edit_mode = "vi"
"#;
        let project = r#"
[mcp_servers.foo]
command = "x"
"#;
        let cfg = merge_layers(user, project);
        assert_eq!(cfg.ui.theme, "solarized");
        assert_eq!(cfg.ui.edit_mode, "vi");
    }

    #[test]
    fn project_without_features_preserves_user_feature_flags() {
        let user = r#"
[features]
token_budget = false
prompt_caching = false
"#;
        let project = "";
        let cfg = merge_layers(user, project);
        assert!(!cfg.features.token_budget);
        assert!(!cfg.features.prompt_caching);
        // Unspecified flags fall back to their struct default (true).
        assert!(cfg.features.commit_attribution);
    }

    #[test]
    fn permission_rules_extend_across_layers() {
        let user = r#"
[[permissions.rules]]
tool = "Read"
action = "allow"

[[permissions.rules]]
tool = "Bash"
pattern = "rm -rf *"
action = "deny"
"#;
        let project = r#"
[[permissions.rules]]
tool = "Write"
action = "ask"
"#;
        let cfg = merge_layers(user, project);
        assert_eq!(cfg.permissions.rules.len(), 3);
        assert_eq!(cfg.permissions.rules[0].tool, "Read");
        assert_eq!(cfg.permissions.rules[1].tool, "Bash");
        assert_eq!(cfg.permissions.rules[2].tool, "Write");
    }

    #[test]
    fn mcp_servers_merge_by_name_project_overrides_user() {
        let user = r#"
[mcp_servers.alpha]
command = "user-alpha"

[mcp_servers.beta]
command = "user-beta"
"#;
        let project = r#"
[mcp_servers.beta]
command = "project-beta"

[mcp_servers.gamma]
command = "project-gamma"
"#;
        let cfg = merge_layers(user, project);
        assert_eq!(
            cfg.mcp_servers["alpha"].command.as_deref(),
            Some("user-alpha")
        );
        assert_eq!(
            cfg.mcp_servers["beta"].command.as_deref(),
            Some("project-beta")
        );
        assert_eq!(
            cfg.mcp_servers["gamma"].command.as_deref(),
            Some("project-gamma")
        );
    }

    #[test]
    fn no_layers_yields_default_config() {
        let cfg = merge_layers("", "");
        assert_eq!(cfg.api.model, "gpt-5.4");
        assert_eq!(cfg.permissions.default_mode, PermissionMode::Ask);
    }

    // ---- merge_toml_values primitive ----

    #[test]
    fn merge_toml_values_recursive_table_merge() {
        let mut base: toml::Value = toml::from_str(
            r#"
[api]
base_url = "http://a"
model = "m1"
"#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
[api]
model = "m2"
"#,
        )
        .unwrap();
        merge_toml_values(&mut base, &overlay);
        let api = base.get("api").unwrap();
        assert_eq!(api.get("base_url").unwrap().as_str(), Some("http://a"));
        assert_eq!(api.get("model").unwrap().as_str(), Some("m2"));
    }

    #[test]
    fn merge_toml_values_overlay_replaces_non_table() {
        let mut base = toml::Value::String("old".into());
        let overlay = toml::Value::String("new".into());
        merge_toml_values(&mut base, &overlay);
        assert_eq!(base.as_str(), Some("new"));
    }
}

#[cfg(test)]
mod e2e_tests {
    //! End-to-end tests that write real TOML files to a temp directory and
    //! drive the full file-reading + merge pipeline. These cover everything
    //! `Config::load` does except the XDG path resolution and env overrides,
    //! both of which would require process-global mutation to test.
    //!
    //! Also covers `find_config_in_ancestors` directly against a tempdir tree.

    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Write user + project files and drive the full load pipeline the way
    /// `load_inner` does: read each file to a String, then merge.
    fn load_from_files(user_toml: Option<&str>, project_toml: Option<&str>) -> Config {
        load_from_files_full(user_toml, project_toml, None)
    }

    /// Same as `load_from_files` but with an optional `.local.toml` layer
    /// appended last so it overrides the project layer.
    fn load_from_files_full(
        user_toml: Option<&str>,
        project_toml: Option<&str>,
        local_toml: Option<&str>,
    ) -> Config {
        let dir = TempDir::new().unwrap();
        let mut layers: Vec<String> = Vec::new();

        if let Some(body) = user_toml {
            let path = dir.path().join("user.toml");
            fs::write(&path, body).unwrap();
            layers.push(fs::read_to_string(&path).unwrap());
        }
        if let Some(body) = project_toml {
            let path = dir.path().join("project.toml");
            fs::write(&path, body).unwrap();
            layers.push(fs::read_to_string(&path).unwrap());
        }
        if let Some(body) = local_toml {
            let path = dir.path().join("local.toml");
            fs::write(&path, body).unwrap();
            layers.push(fs::read_to_string(&path).unwrap());
        }

        let refs: Vec<&str> = layers.iter().map(String::as_str).collect();
        merge_layer_contents(&refs).unwrap()
    }

    // ---- Issue #101 reproduction, through real files ----

    #[test]
    fn e2e_issue_101_ollama_user_preserved_when_project_has_only_mcp_servers() {
        let user = r#"
[api]
base_url = "http://localhost:11434/v1"
model = "gemma4:26b"
api_key = "ollama"
"#;
        let project = r#"
[mcp_servers.my-server]
command = "/usr/local/bin/my-mcp"
args = []
"#;
        let cfg = load_from_files(Some(user), Some(project));
        assert_eq!(cfg.api.base_url, "http://localhost:11434/v1");
        assert_eq!(cfg.api.model, "gemma4:26b");
        assert_eq!(cfg.api.api_key.as_deref(), Some("ollama"));
        assert_eq!(
            cfg.mcp_servers["my-server"].command.as_deref(),
            Some("/usr/local/bin/my-mcp")
        );
    }

    #[test]
    fn e2e_only_user_config_exists() {
        let user = r#"
[api]
base_url = "http://example.com/v1"
model = "custom"
"#;
        let cfg = load_from_files(Some(user), None);
        assert_eq!(cfg.api.base_url, "http://example.com/v1");
        assert_eq!(cfg.api.model, "custom");
    }

    #[test]
    fn e2e_only_project_config_exists() {
        let project = r#"
[api]
base_url = "http://proj.example.com/v1"
model = "proj-model"
"#;
        let cfg = load_from_files(None, Some(project));
        assert_eq!(cfg.api.base_url, "http://proj.example.com/v1");
        assert_eq!(cfg.api.model, "proj-model");
    }

    #[test]
    fn e2e_no_config_files_yields_defaults() {
        let cfg = load_from_files(None, None);
        assert_eq!(cfg.api.model, "gpt-5.4");
        assert_eq!(cfg.permissions.default_mode, PermissionMode::Ask);
        assert!(cfg.ui.markdown);
    }

    #[test]
    fn e2e_project_overrides_model_keeps_user_base_url() {
        let user = r#"
[api]
base_url = "http://ollama.local/v1"
model = "gemma4:26b"
"#;
        let project = r#"
[api]
model = "llama3:70b"
"#;
        let cfg = load_from_files(Some(user), Some(project));
        assert_eq!(cfg.api.base_url, "http://ollama.local/v1");
        assert_eq!(cfg.api.model, "llama3:70b");
    }

    #[test]
    fn e2e_project_overrides_single_ui_field_keeps_others() {
        let user = r#"
[ui]
theme = "solarized"
edit_mode = "vi"
markdown = false
"#;
        let project = r#"
[ui]
theme = "light"
"#;
        let cfg = load_from_files(Some(user), Some(project));
        assert_eq!(cfg.ui.theme, "light");
        assert_eq!(cfg.ui.edit_mode, "vi");
        assert!(!cfg.ui.markdown);
    }

    #[test]
    fn e2e_permission_rules_concatenate_across_layers() {
        let user = r#"
[[permissions.rules]]
tool = "Read"
action = "allow"

[[permissions.rules]]
tool = "Bash"
pattern = "rm -rf /"
action = "deny"
"#;
        let project = r#"
[[permissions.rules]]
tool = "Write"
action = "ask"
"#;
        let cfg = load_from_files(Some(user), Some(project));
        assert_eq!(cfg.permissions.rules.len(), 3);
        let tools: Vec<&str> = cfg
            .permissions
            .rules
            .iter()
            .map(|r| r.tool.as_str())
            .collect();
        assert_eq!(tools, vec!["Read", "Bash", "Write"]);
    }

    #[test]
    fn e2e_mcp_servers_union_by_name() {
        let user = r#"
[mcp_servers.alpha]
command = "user-alpha"

[mcp_servers.beta]
command = "user-beta"
"#;
        let project = r#"
[mcp_servers.beta]
command = "project-beta"

[mcp_servers.gamma]
command = "project-gamma"
"#;
        let cfg = load_from_files(Some(user), Some(project));
        assert_eq!(cfg.mcp_servers.len(), 3);
        assert_eq!(
            cfg.mcp_servers["alpha"].command.as_deref(),
            Some("user-alpha")
        );
        assert_eq!(
            cfg.mcp_servers["beta"].command.as_deref(),
            Some("project-beta")
        );
        assert_eq!(
            cfg.mcp_servers["gamma"].command.as_deref(),
            Some("project-gamma")
        );
    }

    #[test]
    fn e2e_feature_flags_partial_override() {
        let user = r#"
[features]
token_budget = false
prompt_caching = false
"#;
        let project = r#"
[features]
token_budget = true
"#;
        let cfg = load_from_files(Some(user), Some(project));
        assert!(cfg.features.token_budget); // project flipped it
        assert!(!cfg.features.prompt_caching); // user value preserved
        assert!(cfg.features.commit_attribution); // struct default
    }

    #[test]
    fn e2e_malformed_toml_is_surfaced_as_parse_error() {
        let bad = "this is = = not valid toml\n[[[";
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.toml");
        fs::write(&path, bad).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        let err = merge_layer_contents(&[&content]).unwrap_err();
        assert!(matches!(err, ConfigError::ParseError(_)));
    }

    // ---- find_config_in_ancestors over a real directory tree ----

    #[test]
    fn e2e_find_project_config_walks_up_from_nested_dir() {
        let root = TempDir::new().unwrap();
        let project_root = root.path().join("myproj");
        let nested = project_root.join("crates").join("deep").join("src");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(project_root.join(".agent")).unwrap();
        let settings = project_root.join(".agent").join("settings.toml");
        fs::write(&settings, "[api]\nmodel = \"from-ancestor\"\n").unwrap();

        let found = find_config_in_ancestors(&nested).unwrap();
        assert_eq!(found, settings);
    }

    #[test]
    fn e2e_find_project_config_returns_none_when_absent() {
        let root = TempDir::new().unwrap();
        let nested = root.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        // No `.agent/settings.toml` anywhere.
        // The walk may still hit a real `.agent/settings.toml` in a parent of
        // the tempdir root (unlikely but possible on dev machines). Guard by
        // checking the result is either None or outside our tempdir.
        if let Some(path) = find_config_in_ancestors(&nested) {
            assert!(
                !path.starts_with(root.path()),
                "unexpected settings.toml inside tempdir: {path:?}"
            );
        }
    }

    #[test]
    fn e2e_find_project_config_stops_at_first_match() {
        let root = TempDir::new().unwrap();
        // Two levels, both with .agent/settings.toml. The inner one should win.
        let outer = root.path().join("outer");
        let inner = outer.join("inner");
        fs::create_dir_all(inner.join(".agent")).unwrap();
        fs::create_dir_all(outer.join(".agent")).unwrap();
        let inner_settings = inner.join(".agent").join("settings.toml");
        let outer_settings = outer.join(".agent").join("settings.toml");
        fs::write(&inner_settings, "[api]\nmodel = \"inner\"\n").unwrap();
        fs::write(&outer_settings, "[api]\nmodel = \"outer\"\n").unwrap();

        let found = find_config_in_ancestors(&inner).unwrap();
        assert_eq!(found, inner_settings);
    }

    // ---- settings.local.toml: gitignored project-local overrides ----

    #[test]
    fn local_layer_overrides_project_layer() {
        let user = r#"
[api]
model = "user-model"
"#;
        let project = r#"
[api]
model = "project-model"
base_url = "https://shared.example.com/v1"
"#;
        let local = r#"
[api]
model = "local-dev-model"
"#;
        let cfg = load_from_files_full(Some(user), Some(project), Some(local));
        // Local wins over project wins over user.
        assert_eq!(cfg.api.model, "local-dev-model");
        // base_url comes from project (local didn't set it).
        assert_eq!(cfg.api.base_url, "https://shared.example.com/v1");
    }

    #[test]
    fn local_layer_partial_override_leaves_other_fields_intact() {
        let project = r#"
[api]
model = "gpt-5"
base_url = "https://team.example.com/v1"
api_key = "team-key"
"#;
        // Dev overrides only the base URL to point at a local proxy.
        let local = r#"
[api]
base_url = "http://localhost:11434/v1"
"#;
        let cfg = load_from_files_full(None, Some(project), Some(local));
        assert_eq!(cfg.api.base_url, "http://localhost:11434/v1");
        assert_eq!(cfg.api.model, "gpt-5");
        assert_eq!(cfg.api.api_key.as_deref(), Some("team-key"));
    }

    #[test]
    fn local_layer_without_project_still_overlays_on_user() {
        let user = r#"
[api]
model = "user-model"
base_url = "https://example.com/v1"
"#;
        let local = r#"
[api]
model = "local-model"
"#;
        let cfg = load_from_files_full(Some(user), None, Some(local));
        assert_eq!(cfg.api.model, "local-model");
        assert_eq!(cfg.api.base_url, "https://example.com/v1");
    }

    #[test]
    fn local_layer_permission_rules_extend_across_all_layers() {
        // `permissions.rules` uses extend-semantics across every layer,
        // including the new local layer. Verify the concat order is
        // user → project → local so local rules apply last.
        let user = r#"
[[permissions.rules]]
tool = "Read"
action = "allow"
"#;
        let project = r#"
[[permissions.rules]]
tool = "Bash"
pattern = "rm -rf /"
action = "deny"
"#;
        let local = r#"
[[permissions.rules]]
tool = "Write"
action = "ask"
"#;
        let cfg = load_from_files_full(Some(user), Some(project), Some(local));
        assert_eq!(cfg.permissions.rules.len(), 3);
        let tools: Vec<&str> = cfg
            .permissions
            .rules
            .iter()
            .map(|r| r.tool.as_str())
            .collect();
        assert_eq!(tools, vec!["Read", "Bash", "Write"]);
    }

    #[test]
    fn e2e_find_local_config_walks_up_from_nested_dir() {
        let root = TempDir::new().unwrap();
        let project_root = root.path().join("myproj");
        let nested = project_root.join("crates").join("deep").join("src");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(project_root.join(".agent")).unwrap();
        let local = project_root.join(".agent").join("settings.local.toml");
        fs::write(&local, "[api]\nmodel = \"local\"\n").unwrap();

        let found = find_local_config_in_ancestors(&nested).unwrap();
        assert_eq!(found, local);
    }

    #[test]
    fn e2e_find_local_config_is_independent_from_settings_toml() {
        // settings.toml present but NO settings.local.toml — local walk
        // should return None even though settings.toml exists nearby.
        let root = TempDir::new().unwrap();
        let project_root = root.path().join("proj");
        fs::create_dir_all(project_root.join(".agent")).unwrap();
        fs::write(
            project_root.join(".agent").join("settings.toml"),
            "[api]\nmodel = \"committed\"\n",
        )
        .unwrap();

        if let Some(path) = find_local_config_in_ancestors(&project_root) {
            assert!(
                !path.starts_with(root.path()),
                "unexpected settings.local.toml inside tempdir: {path:?}"
            );
        }
    }

    #[test]
    fn e2e_find_local_config_stops_at_first_match() {
        // Two levels, both with settings.local.toml — inner wins.
        let root = TempDir::new().unwrap();
        let outer = root.path().join("outer");
        let inner = outer.join("inner");
        fs::create_dir_all(inner.join(".agent")).unwrap();
        fs::create_dir_all(outer.join(".agent")).unwrap();
        let inner_local = inner.join(".agent").join("settings.local.toml");
        let outer_local = outer.join(".agent").join("settings.local.toml");
        fs::write(&inner_local, "[api]\nmodel = \"inner\"\n").unwrap();
        fs::write(&outer_local, "[api]\nmodel = \"outer\"\n").unwrap();

        let found = find_local_config_in_ancestors(&inner).unwrap();
        assert_eq!(found, inner_local);
    }

    // ---- api_key_helper: dynamic API-key sourcing ----

    #[cfg(unix)]
    #[test]
    fn api_key_helper_returns_trimmed_stdout() {
        // `echo` prints with a trailing newline that the helper must trim.
        let key = resolve_api_key_from_helper("echo sk-from-helper").unwrap();
        assert_eq!(key, "sk-from-helper");
    }

    #[cfg(unix)]
    #[test]
    fn api_key_helper_surfaces_nonzero_exit() {
        let err = resolve_api_key_from_helper("exit 7").unwrap_err();
        assert_eq!(err, ApiKeyHelperError::NonZeroExit);
        // Category must stay category-level — no raw subprocess text.
        assert_eq!(err.category(), "non-zero exit");
    }

    #[cfg(unix)]
    #[test]
    fn api_key_helper_error_category_does_not_leak_stderr() {
        // A helper that emits the would-be key on stderr and exits
        // non-zero must not see that text show up in the category
        // string that we log.
        let err = resolve_api_key_from_helper("echo sk-SECRET-1234 >&2; exit 1").unwrap_err();
        let cat = err.category();
        assert!(
            !cat.contains("sk-"),
            "category leaked stderr content: {cat:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn api_key_helper_spawn_failure_is_categorized() {
        // `/nonexistent-shell-xyz` will fail to spawn; bash -c then
        // surfaces as non-zero. This doesn't hit SpawnFailed (bash
        // itself spawns fine) but exercises the NonZeroExit branch
        // when the inner command is missing. The SpawnFailed branch
        // is covered indirectly by the category() mapping.
        let err = resolve_api_key_from_helper("/nonexistent-helper-xyz").unwrap_err();
        assert_eq!(err, ApiKeyHelperError::NonZeroExit);
    }

    #[test]
    fn api_key_helper_category_mapping_is_complete() {
        // Guard against a future enum variant landing without an
        // accompanying category string.
        assert_eq!(ApiKeyHelperError::SpawnFailed.category(), "spawn failed");
        assert_eq!(ApiKeyHelperError::NonZeroExit.category(), "non-zero exit");
        assert_eq!(
            ApiKeyHelperError::InvalidUtf8.category(),
            "invalid UTF-8 output"
        );
    }

    #[cfg(unix)]
    #[test]
    fn api_key_helper_empty_output_resolves_to_empty_string() {
        // Command succeeds but prints nothing; caller is responsible for
        // treating empty-string as "no key set" — verified below.
        //
        // Avoid formatting the resolved key into any assert message:
        // the variable is conceptually a secret, and static analyzers
        // flag paths where the key could land in panic output.
        let key = resolve_api_key_from_helper("true").unwrap();
        assert_eq!(key.len(), 0);
    }

    fn has_vars(set_vars: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |v| set_vars.contains(&v)
    }

    #[test]
    fn key_var_matches_effective_provider_over_priority_order() {
        // ANTHROPIC outranks XAI in the fixed order, but the effective
        // provider is xAI — its key must win.
        let var = choose_api_key_var(
            "https://api.x.ai/v1",
            "grok-build-0.1",
            has_vars(&["ANTHROPIC_API_KEY", "XAI_API_KEY"]),
        );
        assert_eq!(var, Some("XAI_API_KEY"));
    }

    #[test]
    fn generic_key_keeps_top_rank() {
        let var = choose_api_key_var(
            "https://api.x.ai/v1",
            "grok-build-0.1",
            has_vars(&["AGENT_CODE_API_KEY", "XAI_API_KEY"]),
        );
        assert_eq!(var, Some("AGENT_CODE_API_KEY"));
    }

    #[test]
    fn missing_provider_var_falls_back_to_priority_order() {
        // Only a foreign key is exported: preserve the pre-existing
        // fallback (the session may be intentionally cross-configured,
        // e.g. a proxy that accepts any bearer token).
        let var = choose_api_key_var(
            "https://api.x.ai/v1",
            "grok-build-0.1",
            has_vars(&["ANTHROPIC_API_KEY"]),
        );
        assert_eq!(var, None);
    }

    #[test]
    fn custom_and_cloud_endpoints_use_priority_order() {
        for url in [
            "http://localhost:11434/v1",
            "https://myco.openai.azure.com/openai/deployments/x",
            "https://bedrock-runtime.us-east-1.amazonaws.com",
        ] {
            let var = choose_api_key_var(url, "", has_vars(&["OPENAI_API_KEY"]));
            assert_eq!(var, None, "no provider-pinning for {url}");
        }
    }

    #[test]
    fn model_based_detection_pins_the_key_too() {
        // No recognizable URL, but the model names the vendor.
        let var = choose_api_key_var(
            "https://api.openai.com/v1",
            "claude-sonnet-4-20250514",
            has_vars(&["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]),
        );
        // URL detection runs first and says OpenAI — the URL is the
        // stronger signal of where the request will actually be sent.
        assert_eq!(var, Some("OPENAI_API_KEY"));
    }

    #[cfg(unix)]
    #[test]
    fn api_key_helper_stderr_does_not_leak_into_key() {
        // Helpers that emit diagnostics on stderr must not have those
        // mixed into the resolved key.
        let key = resolve_api_key_from_helper("echo sk-real; echo warn >&2").unwrap();
        assert_eq!(key, "sk-real");
    }

    #[cfg(unix)]
    #[test]
    fn api_key_helper_multiline_output_is_trimmed_only_at_edges() {
        // Accept any opaque secret shape — just trim surrounding
        // whitespace. A newline inside the secret (weird but legal for
        // some encoded tokens) must be preserved.
        let key = resolve_api_key_from_helper("printf '  part1\\npart2  '").unwrap();
        assert_eq!(key, "part1\npart2");
    }

    /// Reading another project's settings must not move the process.
    ///
    /// The resume path answers "would this project's config parse" before
    /// committing to it. Entering the directory to find out mutates
    /// global state for every thread, and a failed return trip leaves the
    /// process somewhere the caller does not believe it is.
    #[test]
    fn load_from_reads_a_project_without_entering_it() {
        let dir = tempfile::tempdir().unwrap();
        let agent = dir.path().join(".agent");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("settings.toml"),
            "[permissions]\ndefault_mode = \"deny\"\n",
        )
        .unwrap();

        let before = std::env::current_dir().unwrap();
        let cfg = Config::load_from(dir.path()).expect("the project's settings should parse");
        let after = std::env::current_dir().unwrap();

        assert_eq!(
            cfg.permissions.default_mode,
            crate::config::PermissionMode::Deny,
            "the target project's layer was not read"
        );
        assert_eq!(before, after, "loading moved the process");
    }
}
