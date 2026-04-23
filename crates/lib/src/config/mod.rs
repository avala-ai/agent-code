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

mod schema;

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
        let result = Self::load_inner();
        LOADING.store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    fn load_inner() -> Result<Config, ConfigError> {
        let mut layers: Vec<String> = Vec::new();

        // Layer 1: User-level config (lowest priority file).
        if let Some(path) = user_config_path()
            && path.exists()
        {
            layers.push(
                std::fs::read_to_string(&path)
                    .map_err(|e| ConfigError::FileError(format!("{path:?}: {e}")))?,
            );
        }

        // Layer 2: Project-level config (overrides user config).
        if let Some(path) = find_project_config() {
            layers.push(
                std::fs::read_to_string(&path)
                    .map_err(|e| ConfigError::FileError(format!("{path:?}: {e}")))?,
            );
        }

        // Layer 2.5: Project-local overrides (gitignored, overrides
        // committed project settings). Sits next to `settings.toml` as
        // `settings.local.toml` in the same `.agent/` directory; only
        // the one closest to cwd is used, matching the project-config
        // walk so sub-packages inherit the repo-root settings.local.
        if let Some(path) = find_project_local_config() {
            layers.push(
                std::fs::read_to_string(&path)
                    .map_err(|e| ConfigError::FileError(format!("{path:?}: {e}")))?,
            );
        }

        let layer_refs: Vec<&str> = layers.iter().map(String::as_str).collect();
        let mut config = merge_layer_contents(&layer_refs)?;

        // Layer 3: Environment variables override file-based config.
        // API key from env always wins over config files, because users
        // expect `OPENAI_API_KEY=x agent` to use key x, even if a
        // stale key exists in config.toml.
        let env_api_key = resolve_api_key_from_env();
        if env_api_key.is_some() {
            config.api.api_key = env_api_key;
        }

        // Base URL from env overrides file config.
        if let Ok(url) = std::env::var("AGENT_CODE_API_BASE_URL") {
            config.api.base_url = url;
        }

        // Model from env overrides file config.
        if let Ok(model) = std::env::var("AGENT_CODE_MODEL") {
            config.api.model = model;
        }

        Ok(config)
    }
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

/// Resolve API key from environment variables.
///
/// Checks each provider's env var in priority order. Returns the first
/// one found, or None if no API key is set in the environment.
fn resolve_api_key_from_env() -> Option<String> {
    std::env::var("AGENT_CODE_API_KEY")
        .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .or_else(|_| std::env::var("XAI_API_KEY"))
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
        .or_else(|_| std::env::var("GROQ_API_KEY"))
        .or_else(|_| std::env::var("MISTRAL_API_KEY"))
        .or_else(|_| std::env::var("ZHIPU_API_KEY"))
        .or_else(|_| std::env::var("TOGETHER_API_KEY"))
        .or_else(|_| std::env::var("OPENROUTER_API_KEY"))
        .or_else(|_| std::env::var("COHERE_API_KEY"))
        .or_else(|_| std::env::var("PERPLEXITY_API_KEY"))
        .ok()
}

/// Returns the user-level config file path.
fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("config.toml"))
}

/// Walk up from the current directory to find `.agent/settings.toml`.
fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_config_in_ancestors(&cwd)
}

/// Walk up from the current directory to find `.agent/settings.local.toml`.
///
/// This is the gitignored overlay that sits on top of the shared
/// `settings.toml`. Kept as a separate walk (not just a sibling lookup
/// next to the resolved `settings.toml`) so a developer can place a
/// `settings.local.toml` at a different ancestor level than the
/// committed settings — for example, a repo-root `settings.toml` plus
/// a crate-local `settings.local.toml` overriding just one field.
fn find_project_local_config() -> Option<PathBuf> {
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
}
