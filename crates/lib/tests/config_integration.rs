//! Integration tests for the configuration system.
//!
//! Tests TOML parsing, default values, and config struct behavior.

use agent_code_lib::config::{Config, PermissionMode, SecurityConfig};

#[test]
fn default_config_has_sane_defaults() {
    let config = Config::default();

    assert_eq!(config.permissions.default_mode, PermissionMode::Ask);
    assert!(config.permissions.rules.is_empty());
    assert!(config.ui.markdown);
    assert!(config.ui.syntax_highlight);
    assert_eq!(config.api.timeout_secs, 120);
    assert_eq!(config.api.max_retries, 3);
    assert!(config.hooks.is_empty());
    assert!(config.mcp_servers.is_empty());
}

#[test]
fn config_parses_from_toml() {
    let toml_str = r#"
[api]
model = "claude-sonnet-4-20250514"
timeout_secs = 60

[permissions]
default_mode = "allow"

[[permissions.rules]]
tool = "Bash"
pattern = "rm *"
action = "deny"

[ui]
theme = "daybreak"

[security]
disable_bypass_permissions = true
disable_skill_shell_execution = true
"#;

    let config: Config = toml::from_str(toml_str).unwrap();

    assert_eq!(config.api.model, "claude-sonnet-4-20250514");
    assert_eq!(config.api.timeout_secs, 60);
    assert_eq!(config.permissions.default_mode, PermissionMode::Allow);
    assert_eq!(config.permissions.rules.len(), 1);
    assert_eq!(config.permissions.rules[0].tool, "Bash");
    assert_eq!(config.permissions.rules[0].pattern.as_deref(), Some("rm *"));
    assert_eq!(config.permissions.rules[0].action, PermissionMode::Deny);
    assert_eq!(config.ui.theme, "daybreak");
    assert!(config.security.disable_bypass_permissions);
    assert!(config.security.disable_skill_shell_execution);
}

#[test]
fn security_config_defaults_to_permissive() {
    let config = SecurityConfig::default();

    assert!(!config.disable_bypass_permissions);
    assert!(!config.disable_skill_shell_execution);
    assert!(config.mcp_server_allowlist.is_empty());
    assert!(config.mcp_server_denylist.is_empty());
    assert!(config.env_allowlist.is_empty());
    assert!(config.additional_directories.is_empty());
}

#[test]
fn features_default_to_enabled() {
    let config = Config::default();

    assert!(config.features.token_budget);
    assert!(config.features.commit_attribution);
    assert!(config.features.extract_memories);
    assert!(config.features.context_collapse);
    assert!(config.features.reactive_compact);
    assert!(config.features.history_snip);
    assert!(config.features.fork_conversation);
}

#[test]
fn mcp_server_entry_parses() {
    let toml_str = r#"
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[mcp_servers.remote]
url = "http://localhost:8080"
"#;

    let config: Config = toml::from_str(toml_str).unwrap();

    assert_eq!(config.mcp_servers.len(), 2);
    let github = &config.mcp_servers["github"];
    assert_eq!(github.command.as_deref(), Some("npx"));
    assert_eq!(
        github.args,
        vec!["-y", "@modelcontextprotocol/server-github"]
    );

    let remote = &config.mcp_servers["remote"];
    assert_eq!(remote.url.as_deref(), Some("http://localhost:8080"));
    assert!(remote.command.is_none());
}
