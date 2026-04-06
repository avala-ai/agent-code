//! CLI-level integration tests for configuration loading and parsing.
//!
//! Tests Config::load() defaults, TOML parsing, config merging,
//! FeaturesConfig, SecurityConfig, and PermissionMode serde round-trips.

use agent_code_lib::config::{
    Config, FeaturesConfig, PermissionMode, PermissionRule, SecurityConfig,
};

// ---------------------------------------------------------------------------
// Config::load() returns defaults when no config files exist
// ---------------------------------------------------------------------------

#[test]
fn config_load_returns_defaults_without_config_files() {
    // Config::load() should succeed even with no config files on disk.
    // It may or may not find files depending on the CI environment,
    // but it should never panic.
    let result = Config::load();
    assert!(result.is_ok(), "Config::load() should not fail");
}

// ---------------------------------------------------------------------------
// Config defaults have correct values
// ---------------------------------------------------------------------------

#[test]
fn config_defaults_model() {
    let config = Config::default();
    // Default model is set (non-empty).
    assert!(!config.api.model.is_empty());
}

#[test]
fn config_defaults_base_url() {
    let config = Config::default();
    // Default base_url is set (non-empty).
    assert!(!config.api.base_url.is_empty());
}

#[test]
fn config_defaults_timeout_secs() {
    let config = Config::default();
    assert_eq!(config.api.timeout_secs, 120);
}

#[test]
fn config_defaults_max_retries() {
    let config = Config::default();
    assert_eq!(config.api.max_retries, 3);
}

#[test]
fn config_defaults_max_output_tokens() {
    let config = Config::default();
    assert_eq!(config.api.max_output_tokens, Some(16384));
}

#[test]
fn config_defaults_permission_mode_is_ask() {
    let config = Config::default();
    assert_eq!(config.permissions.default_mode, PermissionMode::Ask);
}

#[test]
fn config_defaults_empty_collections() {
    let config = Config::default();
    assert!(config.permissions.rules.is_empty());
    assert!(config.hooks.is_empty());
    assert!(config.mcp_servers.is_empty());
}

#[test]
fn config_defaults_ui() {
    let config = Config::default();
    assert!(config.ui.markdown);
    assert!(config.ui.syntax_highlight);
    assert_eq!(config.ui.theme, "dark");
    assert_eq!(config.ui.edit_mode, "emacs");
}

// ---------------------------------------------------------------------------
// Config TOML parsing from string
// ---------------------------------------------------------------------------

#[test]
fn config_toml_parsing_full() {
    let toml_str = r#"
[api]
model = "claude-opus-4"
base_url = "https://api.anthropic.com/v1"
timeout_secs = 300
max_retries = 5
max_output_tokens = 32768

[permissions]
default_mode = "allow"

[[permissions.rules]]
tool = "Bash"
pattern = "git *"
action = "allow"

[[permissions.rules]]
tool = "FileWrite"
action = "deny"

[ui]
markdown = false
syntax_highlight = false
theme = "light"
edit_mode = "vi"

[features]
token_budget = false
commit_attribution = false

[security]
disable_bypass_permissions = true
additional_directories = ["/tmp/extra"]
mcp_server_allowlist = ["github", "filesystem"]
"#;

    let config: Config = toml::from_str(toml_str).unwrap();

    // API settings.
    assert_eq!(config.api.model, "claude-opus-4");
    assert_eq!(config.api.base_url, "https://api.anthropic.com/v1");
    assert_eq!(config.api.timeout_secs, 300);
    assert_eq!(config.api.max_retries, 5);
    assert_eq!(config.api.max_output_tokens, Some(32768));

    // Permissions.
    assert_eq!(config.permissions.default_mode, PermissionMode::Allow);
    assert_eq!(config.permissions.rules.len(), 2);
    assert_eq!(config.permissions.rules[0].tool, "Bash");
    assert_eq!(
        config.permissions.rules[0].pattern.as_deref(),
        Some("git *")
    );
    assert_eq!(config.permissions.rules[0].action, PermissionMode::Allow);
    assert_eq!(config.permissions.rules[1].tool, "FileWrite");
    assert!(config.permissions.rules[1].pattern.is_none());
    assert_eq!(config.permissions.rules[1].action, PermissionMode::Deny);

    // UI.
    assert!(!config.ui.markdown);
    assert!(!config.ui.syntax_highlight);
    assert_eq!(config.ui.theme, "light");
    assert_eq!(config.ui.edit_mode, "vi");

    // Features (partially overridden).
    assert!(!config.features.token_budget);
    assert!(!config.features.commit_attribution);
    // Non-overridden features keep their defaults.
    assert!(config.features.extract_memories);

    // Security.
    assert!(config.security.disable_bypass_permissions);
    assert_eq!(config.security.additional_directories, vec!["/tmp/extra"]);
    assert_eq!(
        config.security.mcp_server_allowlist,
        vec!["github", "filesystem"]
    );
}

#[test]
fn config_toml_parsing_minimal() {
    let toml_str = r#"
[api]
model = "gpt-5.4"
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.api.model, "gpt-5.4");
    // Everything else should be defaults.
    assert_eq!(config.permissions.default_mode, PermissionMode::Ask);
    assert!(config.ui.markdown);
}

// ---------------------------------------------------------------------------
// Config merge: project config overrides user config
// ---------------------------------------------------------------------------

#[test]
fn config_merge_project_overrides_user() {
    let user_toml = r#"
[api]
model = "user-model"
base_url = "https://user-api.example.com"
timeout_secs = 60

[permissions]
default_mode = "ask"

[ui]
theme = "dark"
"#;

    let project_toml = r#"
[api]
model = "project-model"
timeout_secs = 300

[permissions]
default_mode = "allow"

[[permissions.rules]]
tool = "Bash"
pattern = "git *"
action = "allow"
"#;

    let mut config: Config = toml::from_str(user_toml).unwrap();
    let project_config: Config = toml::from_str(project_toml).unwrap();

    // Simulate the merge (project overrides user).
    if !project_config.api.model.is_empty() {
        config.api.model = project_config.api.model;
    }
    if !project_config.api.base_url.is_empty() {
        config.api.base_url = project_config.api.base_url;
    }
    if project_config.permissions.default_mode != PermissionMode::Ask {
        config.permissions.default_mode = project_config.permissions.default_mode;
    }
    if !project_config.permissions.rules.is_empty() {
        config
            .permissions
            .rules
            .extend(project_config.permissions.rules);
    }

    // Project overrides.
    assert_eq!(config.api.model, "project-model");
    assert_eq!(config.permissions.default_mode, PermissionMode::Allow);
    assert_eq!(config.permissions.rules.len(), 1);

    // User values retained where project didn't override.
    assert_eq!(config.ui.theme, "dark");
}

// ---------------------------------------------------------------------------
// FeaturesConfig defaults are all true
// ---------------------------------------------------------------------------

#[test]
fn features_config_defaults_all_true() {
    let features = FeaturesConfig::default();

    assert!(features.token_budget);
    assert!(features.commit_attribution);
    assert!(features.compaction_reminders);
    assert!(features.unattended_retry);
    assert!(features.history_snip);
    assert!(features.auto_theme);
    assert!(features.mcp_rich_output);
    assert!(features.fork_conversation);
    assert!(features.verification_agent);
    assert!(features.extract_memories);
    assert!(features.context_collapse);
    assert!(features.reactive_compact);
}

#[test]
fn features_config_can_disable_individually() {
    let toml_str = r#"
[features]
token_budget = false
extract_memories = false
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(!config.features.token_budget);
    assert!(!config.features.extract_memories);
    // Others remain true.
    assert!(config.features.commit_attribution);
    assert!(config.features.fork_conversation);
}

// ---------------------------------------------------------------------------
// SecurityConfig defaults are all empty/false
// ---------------------------------------------------------------------------

#[test]
fn security_config_defaults_empty_and_false() {
    let security = SecurityConfig::default();

    assert!(security.additional_directories.is_empty());
    assert!(security.mcp_server_allowlist.is_empty());
    assert!(security.mcp_server_denylist.is_empty());
    assert!(security.env_allowlist.is_empty());
    assert!(!security.disable_bypass_permissions);
    assert!(!security.disable_skill_shell_execution);
}

#[test]
fn security_config_parses_all_fields() {
    let toml_str = r#"
[security]
additional_directories = ["/opt/data", "/home/user/extra"]
mcp_server_allowlist = ["github"]
mcp_server_denylist = ["untrusted-server"]
env_allowlist = ["HOME", "PATH"]
disable_bypass_permissions = true
disable_skill_shell_execution = true
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.security.additional_directories.len(), 2);
    assert_eq!(config.security.mcp_server_allowlist, vec!["github"]);
    assert_eq!(
        config.security.mcp_server_denylist,
        vec!["untrusted-server"]
    );
    assert_eq!(config.security.env_allowlist, vec!["HOME", "PATH"]);
    assert!(config.security.disable_bypass_permissions);
    assert!(config.security.disable_skill_shell_execution);
}

// ---------------------------------------------------------------------------
// PermissionMode serde round-trips
// ---------------------------------------------------------------------------

#[test]
fn permission_mode_serde_roundtrip_allow() {
    let mode = PermissionMode::Allow;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: PermissionMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, PermissionMode::Allow);
}

#[test]
fn permission_mode_serde_roundtrip_deny() {
    let mode = PermissionMode::Deny;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: PermissionMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, PermissionMode::Deny);
}

#[test]
fn permission_mode_serde_roundtrip_ask() {
    let mode = PermissionMode::Ask;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: PermissionMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, PermissionMode::Ask);
}

#[test]
fn permission_mode_serde_roundtrip_plan() {
    let mode = PermissionMode::Plan;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: PermissionMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, PermissionMode::Plan);
}

#[test]
fn permission_mode_serde_roundtrip_accept_edits() {
    let mode = PermissionMode::AcceptEdits;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: PermissionMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, PermissionMode::AcceptEdits);
}

#[test]
fn permission_mode_toml_roundtrip() {
    let config_str = r#"
[permissions]
default_mode = "plan"
"#;
    let config: Config = toml::from_str(config_str).unwrap();
    assert_eq!(config.permissions.default_mode, PermissionMode::Plan);

    // Serialize back to TOML and re-parse.
    let toml_out = toml::to_string(&config).unwrap();
    let reparsed: Config = toml::from_str(&toml_out).unwrap();
    assert_eq!(reparsed.permissions.default_mode, PermissionMode::Plan);
}

// ---------------------------------------------------------------------------
// PermissionRule round-trip
// ---------------------------------------------------------------------------

#[test]
fn permission_rule_serde_roundtrip() {
    let rule = PermissionRule {
        tool: "Bash".into(),
        pattern: Some("git *".into()),
        action: PermissionMode::Allow,
    };

    let json = serde_json::to_string(&rule).unwrap();
    let deserialized: PermissionRule = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.tool, "Bash");
    assert_eq!(deserialized.pattern.as_deref(), Some("git *"));
    assert_eq!(deserialized.action, PermissionMode::Allow);
}

// ---------------------------------------------------------------------------
// Hook and MCP server parsing
// ---------------------------------------------------------------------------

#[test]
fn hooks_parse_from_toml() {
    let toml_str = r#"
[[hooks]]
event = "pre_tool_use"
tool_name = "Bash"

[hooks.action]
type = "shell"
command = "echo pre-hook"

[[hooks]]
event = "session_start"

[hooks.action]
type = "http"
url = "https://example.com/webhook"
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.hooks.len(), 2);
}

#[test]
fn mcp_servers_parse_from_toml() {
    let toml_str = r#"
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[mcp_servers.github.env]
GITHUB_PERSONAL_ACCESS_TOKEN = "placeholder"

[mcp_servers.local]
url = "http://localhost:3000/mcp"
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.mcp_servers.len(), 2);

    let github = &config.mcp_servers["github"];
    assert_eq!(github.command.as_deref(), Some("npx"));
    assert!(!github.env.is_empty());

    let local = &config.mcp_servers["local"];
    assert_eq!(local.url.as_deref(), Some("http://localhost:3000/mcp"));
    assert!(local.command.is_none());
}
