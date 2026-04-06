//! Configuration schema definitions.

use serde::{Deserialize, Serialize};

/// Top-level configuration for the agent.
///
/// Loaded from three layers (highest priority first):
/// 1. CLI flags and environment variables
/// 2. Project config (`.agent/settings.toml`)
/// 3. User config (`~/.config/agent-code/config.toml`)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub api: ApiConfig,
    pub permissions: PermissionsConfig,
    pub ui: UiConfig,
    /// Feature flags — all enabled by default.
    #[serde(default)]
    pub features: FeaturesConfig,
    /// MCP server configurations.
    #[serde(default)]
    pub mcp_servers: std::collections::HashMap<String, McpServerEntry>,
    /// Lifecycle hooks.
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
    /// Security and enterprise settings.
    #[serde(default)]
    pub security: SecurityConfig,
}

/// Security and enterprise configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Additional directories the agent can access (beyond cwd).
    #[serde(default)]
    pub additional_directories: Vec<String>,
    /// MCP server allowlist. If non-empty, only listed servers can connect.
    #[serde(default)]
    pub mcp_server_allowlist: Vec<String>,
    /// MCP server denylist. Listed servers are blocked from connecting.
    #[serde(default)]
    pub mcp_server_denylist: Vec<String>,
    /// Disable the --dangerously-skip-permissions flag.
    #[serde(default)]
    pub disable_bypass_permissions: bool,
    /// Restrict which environment variables the agent can read.
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    /// Disable inline shell execution within skill templates.
    #[serde(default)]
    pub disable_skill_shell_execution: bool,
}

/// Entry for a configured MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// Command to run (for stdio transport).
    pub command: Option<String>,
    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// URL (for SSE transport).
    pub url: Option<String>,
    /// Environment variables for the server process.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// API connection settings.
///
/// Configures the LLM provider: base URL, model, API key, timeouts,
/// cost limits, and thinking mode. The API key is resolved from
/// multiple sources (env vars, config file, CLI flag).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Base URL for the LLM API.
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// API key. Resolved from (in order): config, AGENT_CODE_API_KEY,
    /// ANTHROPIC_API_KEY, OPENAI_API_KEY env vars.
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// Maximum output tokens per response.
    pub max_output_tokens: Option<u32>,
    /// Thinking mode: "enabled", "disabled", or "adaptive".
    pub thinking: Option<String>,
    /// Effort level: "low", "medium", "high".
    pub effort: Option<String>,
    /// Maximum spend per session in USD.
    pub max_cost_usd: Option<f64>,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum retry attempts for transient errors.
    pub max_retries: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        // Resolve API key from multiple environment variables.
        let api_key = std::env::var("AGENT_CODE_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .or_else(|_| std::env::var("AZURE_OPENAI_API_KEY"))
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
            .ok();

        // Auto-detect base URL from which key is set.
        // Check for cloud provider env vars first.
        let use_bedrock = std::env::var("AGENT_CODE_USE_BEDROCK").is_ok()
            || std::env::var("AWS_REGION").is_ok() && api_key.is_some();
        let use_vertex = std::env::var("AGENT_CODE_USE_VERTEX").is_ok();
        let use_azure = std::env::var("AZURE_OPENAI_ENDPOINT").is_ok()
            || std::env::var("AZURE_OPENAI_API_KEY").is_ok();

        let has_generic = std::env::var("AGENT_CODE_API_KEY").is_ok();
        let base_url = if use_bedrock {
            // AWS Bedrock — URL constructed from region.
            let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
            format!("https://bedrock-runtime.{region}.amazonaws.com")
        } else if use_vertex {
            // Google Vertex AI.
            let project = std::env::var("GOOGLE_CLOUD_PROJECT").unwrap_or_default();
            let location = std::env::var("GOOGLE_CLOUD_LOCATION")
                .unwrap_or_else(|_| "us-central1".to_string());
            format!(
                "https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/anthropic/models"
            )
        } else if use_azure {
            // Azure OpenAI — URL from AZURE_OPENAI_ENDPOINT or placeholder.
            std::env::var("AZURE_OPENAI_ENDPOINT").unwrap_or_else(|_| {
                "https://YOUR_RESOURCE.openai.azure.com/openai/deployments/YOUR_DEPLOYMENT"
                    .to_string()
            })
        } else if has_generic {
            // Generic key — default to OpenAI (default model is gpt-5.4).
            "https://api.openai.com/v1".to_string()
        } else if std::env::var("GOOGLE_API_KEY").is_ok() {
            "https://generativelanguage.googleapis.com/v1beta/openai".to_string()
        } else if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            "https://api.deepseek.com/v1".to_string()
        } else if std::env::var("XAI_API_KEY").is_ok() {
            "https://api.x.ai/v1".to_string()
        } else if std::env::var("GROQ_API_KEY").is_ok() {
            "https://api.groq.com/openai/v1".to_string()
        } else if std::env::var("MISTRAL_API_KEY").is_ok() {
            "https://api.mistral.ai/v1".to_string()
        } else if std::env::var("TOGETHER_API_KEY").is_ok() {
            "https://api.together.xyz/v1".to_string()
        } else if std::env::var("OPENROUTER_API_KEY").is_ok() {
            "https://openrouter.ai/api/v1".to_string()
        } else if std::env::var("COHERE_API_KEY").is_ok() {
            "https://api.cohere.com/v2".to_string()
        } else if std::env::var("PERPLEXITY_API_KEY").is_ok() {
            "https://api.perplexity.ai".to_string()
        } else {
            // Default to OpenAI (default model is gpt-5.4).
            "https://api.openai.com/v1".to_string()
        };

        Self {
            base_url,
            model: "gpt-5.4".to_string(),
            api_key,
            max_output_tokens: Some(16384),
            thinking: None,
            effort: None,
            max_cost_usd: None,
            timeout_secs: 120,
            max_retries: 3,
        }
    }
}

/// Permission system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    /// Default permission mode for tools without explicit rules.
    pub default_mode: PermissionMode,
    /// Per-tool permission rules.
    pub rules: Vec<PermissionRule>,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            default_mode: PermissionMode::Ask,
            rules: Vec::new(),
        }
    }
}

/// Permission mode controlling how tool calls are authorized.
///
/// Set globally via `[permissions] default_mode` or per-tool via rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Always allow without asking.
    Allow,
    /// Always deny.
    Deny,
    /// Ask the user interactively.
    Ask,
    /// Accept file edits automatically, ask for other mutations.
    AcceptEdits,
    /// Plan mode: read-only tools only.
    Plan,
}

/// A single permission rule matching a tool and optional pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Tool name to match.
    pub tool: String,
    /// Optional glob/regex pattern for the tool's input.
    pub pattern: Option<String>,
    /// Action to take when this rule matches.
    pub action: PermissionMode,
}

/// UI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Enable markdown rendering in output.
    pub markdown: bool,
    /// Enable syntax highlighting in code blocks.
    pub syntax_highlight: bool,
    /// Theme name.
    pub theme: String,
    /// Editing mode: "emacs" or "vi".
    pub edit_mode: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            markdown: true,
            syntax_highlight: true,
            theme: "dark".to_string(),
            edit_mode: "emacs".to_string(),
        }
    }
}

/// Feature flags. All enabled by default — no artificial gates.
/// Users can disable individual features in config.toml under [features].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeaturesConfig {
    /// Track per-turn token usage and warn when approaching budget.
    pub token_budget: bool,
    /// Add co-author attribution line to git commits.
    pub commit_attribution: bool,
    /// Show a system reminder after context compaction.
    pub compaction_reminders: bool,
    /// Auto retry on capacity/overload errors in non-interactive mode.
    pub unattended_retry: bool,
    /// Enable /snip command to remove message ranges from history.
    pub history_snip: bool,
    /// Auto-detect system dark/light mode for theme.
    pub auto_theme: bool,
    /// Rich formatting for MCP tool output.
    pub mcp_rich_output: bool,
    /// Enable /fork command to branch conversation.
    pub fork_conversation: bool,
    /// Verification agent that checks completed tasks.
    pub verification_agent: bool,
    /// Background memory extraction after each turn.
    pub extract_memories: bool,
    /// Context collapse (snip old messages) when approaching limits.
    pub context_collapse: bool,
    /// Reactive auto-compaction when token budget is tight.
    pub reactive_compact: bool,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            token_budget: true,
            commit_attribution: true,
            compaction_reminders: true,
            unattended_retry: true,
            history_snip: true,
            auto_theme: true,
            mcp_rich_output: true,
            fork_conversation: true,
            verification_agent: true,
            extract_memories: true,
            context_collapse: true,
            reactive_compact: true,
        }
    }
}

// ---- Hook types (defined here so config has no runtime dependencies) ----

/// Hook event types that can trigger user-defined actions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart,
    SessionStop,
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
}

/// A configured hook action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HookAction {
    /// Run a shell command.
    #[serde(rename = "shell")]
    Shell { command: String },
    /// Make an HTTP request.
    #[serde(rename = "http")]
    Http { url: String, method: Option<String> },
}

/// A hook definition binding an event to an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub event: HookEvent,
    pub action: HookAction,
    /// Optional tool name filter (for PreToolUse/PostToolUse).
    pub tool_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ApiConfig::default() ----

    #[test]
    fn api_config_default_model() {
        let cfg = ApiConfig::default();
        assert_eq!(cfg.model, "gpt-5.4");
    }

    #[test]
    fn api_config_default_timeout() {
        let cfg = ApiConfig::default();
        assert_eq!(cfg.timeout_secs, 120);
    }

    #[test]
    fn api_config_default_max_retries() {
        let cfg = ApiConfig::default();
        assert_eq!(cfg.max_retries, 3);
    }

    #[test]
    fn api_config_default_max_output_tokens() {
        let cfg = ApiConfig::default();
        assert_eq!(cfg.max_output_tokens, Some(16384));
    }

    #[test]
    fn api_config_default_base_url_contains_scheme() {
        let cfg = ApiConfig::default();
        assert!(
            cfg.base_url.starts_with("https://"),
            "base_url should start with https://, got: {}",
            cfg.base_url
        );
    }

    #[test]
    fn api_config_default_thinking_is_none() {
        let cfg = ApiConfig::default();
        assert!(cfg.thinking.is_none());
    }

    #[test]
    fn api_config_default_effort_is_none() {
        let cfg = ApiConfig::default();
        assert!(cfg.effort.is_none());
    }

    #[test]
    fn api_config_default_max_cost_is_none() {
        let cfg = ApiConfig::default();
        assert!(cfg.max_cost_usd.is_none());
    }

    // ---- PermissionsConfig::default() ----

    #[test]
    fn permissions_config_default_mode_is_ask() {
        let cfg = PermissionsConfig::default();
        assert_eq!(cfg.default_mode, PermissionMode::Ask);
    }

    #[test]
    fn permissions_config_default_rules_empty() {
        let cfg = PermissionsConfig::default();
        assert!(cfg.rules.is_empty());
    }

    // ---- UiConfig::default() ----

    #[test]
    fn ui_config_default_markdown_true() {
        let cfg = UiConfig::default();
        assert!(cfg.markdown);
    }

    #[test]
    fn ui_config_default_syntax_highlight_true() {
        let cfg = UiConfig::default();
        assert!(cfg.syntax_highlight);
    }

    #[test]
    fn ui_config_default_theme_dark() {
        let cfg = UiConfig::default();
        assert_eq!(cfg.theme, "dark");
    }

    #[test]
    fn ui_config_default_edit_mode_emacs() {
        let cfg = UiConfig::default();
        assert_eq!(cfg.edit_mode, "emacs");
    }

    // ---- FeaturesConfig::default() ----

    #[test]
    fn features_config_default_all_true() {
        let cfg = FeaturesConfig::default();
        assert!(cfg.token_budget);
        assert!(cfg.commit_attribution);
        assert!(cfg.compaction_reminders);
        assert!(cfg.unattended_retry);
        assert!(cfg.history_snip);
        assert!(cfg.auto_theme);
        assert!(cfg.mcp_rich_output);
        assert!(cfg.fork_conversation);
        assert!(cfg.verification_agent);
        assert!(cfg.extract_memories);
        assert!(cfg.context_collapse);
        assert!(cfg.reactive_compact);
    }

    // ---- SecurityConfig::default() ----

    #[test]
    fn security_config_default_empty_vecs() {
        let cfg = SecurityConfig::default();
        assert!(cfg.additional_directories.is_empty());
        assert!(cfg.mcp_server_allowlist.is_empty());
        assert!(cfg.mcp_server_denylist.is_empty());
        assert!(cfg.env_allowlist.is_empty());
    }

    #[test]
    fn security_config_default_booleans_false() {
        let cfg = SecurityConfig::default();
        assert!(!cfg.disable_bypass_permissions);
        assert!(!cfg.disable_skill_shell_execution);
    }

    // ---- Config::default() composes sub-defaults ----

    #[test]
    fn config_default_composes_sub_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.api.model, "gpt-5.4");
        assert_eq!(cfg.permissions.default_mode, PermissionMode::Ask);
        assert!(cfg.ui.markdown);
        assert!(cfg.features.token_budget);
        assert!(cfg.mcp_servers.is_empty());
        assert!(cfg.hooks.is_empty());
        assert!(cfg.security.additional_directories.is_empty());
    }

    // ---- PermissionMode serde round-trip ----

    #[test]
    fn permission_mode_serde_roundtrip_allow() {
        let json = serde_json::to_string(&PermissionMode::Allow).unwrap();
        assert_eq!(json, "\"allow\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::Allow);
    }

    #[test]
    fn permission_mode_serde_roundtrip_deny() {
        let json = serde_json::to_string(&PermissionMode::Deny).unwrap();
        assert_eq!(json, "\"deny\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::Deny);
    }

    #[test]
    fn permission_mode_serde_roundtrip_ask() {
        let json = serde_json::to_string(&PermissionMode::Ask).unwrap();
        assert_eq!(json, "\"ask\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::Ask);
    }

    #[test]
    fn permission_mode_serde_roundtrip_accept_edits() {
        let json = serde_json::to_string(&PermissionMode::AcceptEdits).unwrap();
        assert_eq!(json, "\"accept_edits\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::AcceptEdits);
    }

    #[test]
    fn permission_mode_serde_roundtrip_plan() {
        let json = serde_json::to_string(&PermissionMode::Plan).unwrap();
        assert_eq!(json, "\"plan\"");
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PermissionMode::Plan);
    }

    // ---- HookEvent serde round-trip ----

    #[test]
    fn hook_event_serde_roundtrip_session_start() {
        let json = serde_json::to_string(&HookEvent::SessionStart).unwrap();
        assert_eq!(json, "\"session_start\"");
        let back: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HookEvent::SessionStart);
    }

    #[test]
    fn hook_event_serde_roundtrip_session_stop() {
        let json = serde_json::to_string(&HookEvent::SessionStop).unwrap();
        assert_eq!(json, "\"session_stop\"");
        let back: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HookEvent::SessionStop);
    }

    #[test]
    fn hook_event_serde_roundtrip_pre_tool_use() {
        let json = serde_json::to_string(&HookEvent::PreToolUse).unwrap();
        assert_eq!(json, "\"pre_tool_use\"");
        let back: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HookEvent::PreToolUse);
    }

    #[test]
    fn hook_event_serde_roundtrip_post_tool_use() {
        let json = serde_json::to_string(&HookEvent::PostToolUse).unwrap();
        assert_eq!(json, "\"post_tool_use\"");
        let back: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HookEvent::PostToolUse);
    }

    #[test]
    fn hook_event_serde_roundtrip_user_prompt_submit() {
        let json = serde_json::to_string(&HookEvent::UserPromptSubmit).unwrap();
        assert_eq!(json, "\"user_prompt_submit\"");
        let back: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HookEvent::UserPromptSubmit);
    }

    // ---- HookAction serde round-trip ----

    #[test]
    fn hook_action_serde_roundtrip_shell() {
        let action = HookAction::Shell {
            command: "echo hello".into(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"type\":\"shell\""));
        assert!(json.contains("\"command\":\"echo hello\""));
        let back: HookAction = serde_json::from_str(&json).unwrap();
        match back {
            HookAction::Shell { command } => assert_eq!(command, "echo hello"),
            _ => panic!("expected Shell variant"),
        }
    }

    #[test]
    fn hook_action_serde_roundtrip_http() {
        let action = HookAction::Http {
            url: "https://example.com/hook".into(),
            method: Some("POST".into()),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"type\":\"http\""));
        let back: HookAction = serde_json::from_str(&json).unwrap();
        match back {
            HookAction::Http { url, method } => {
                assert_eq!(url, "https://example.com/hook");
                assert_eq!(method.unwrap(), "POST");
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn hook_action_http_method_none() {
        let action = HookAction::Http {
            url: "https://example.com".into(),
            method: None,
        };
        let json = serde_json::to_string(&action).unwrap();
        let back: HookAction = serde_json::from_str(&json).unwrap();
        match back {
            HookAction::Http { method, .. } => assert!(method.is_none()),
            _ => panic!("expected Http variant"),
        }
    }

    // ---- HookDefinition serde round-trip ----

    #[test]
    fn hook_definition_serde_roundtrip() {
        let def = HookDefinition {
            event: HookEvent::PreToolUse,
            action: HookAction::Shell {
                command: "lint.sh".into(),
            },
            tool_name: Some("Bash".into()),
        };
        let json = serde_json::to_string(&def).unwrap();
        let back: HookDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event, HookEvent::PreToolUse);
        assert_eq!(back.tool_name, Some("Bash".into()));
    }

    #[test]
    fn hook_definition_tool_name_none() {
        let def = HookDefinition {
            event: HookEvent::SessionStart,
            action: HookAction::Shell {
                command: "setup.sh".into(),
            },
            tool_name: None,
        };
        let json = serde_json::to_string(&def).unwrap();
        let back: HookDefinition = serde_json::from_str(&json).unwrap();
        assert!(back.tool_name.is_none());
    }

    // ---- Config TOML deserialization ----

    #[test]
    fn config_toml_deserialization_full() {
        let toml_str = r#"
[api]
model = "test-model"
timeout_secs = 60
max_retries = 5
base_url = "https://api.test.com/v1"

[permissions]
default_mode = "allow"

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
additional_directories = ["/tmp"]
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.api.model, "test-model");
        assert_eq!(cfg.api.timeout_secs, 60);
        assert_eq!(cfg.api.max_retries, 5);
        assert_eq!(cfg.api.base_url, "https://api.test.com/v1");
        assert_eq!(cfg.permissions.default_mode, PermissionMode::Allow);
        assert!(!cfg.ui.markdown);
        assert!(!cfg.ui.syntax_highlight);
        assert_eq!(cfg.ui.theme, "light");
        assert_eq!(cfg.ui.edit_mode, "vi");
        assert!(!cfg.features.token_budget);
        assert!(!cfg.features.commit_attribution);
        assert!(cfg.security.disable_bypass_permissions);
        assert_eq!(cfg.security.additional_directories, vec!["/tmp"]);
    }

    #[test]
    fn config_toml_empty_string_uses_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.api.timeout_secs, 120);
        assert_eq!(cfg.permissions.default_mode, PermissionMode::Ask);
        assert!(cfg.ui.markdown);
    }

    #[test]
    fn config_toml_partial_override() {
        let toml_str = r#"
[ui]
theme = "solarized"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        // Overridden field
        assert_eq!(cfg.ui.theme, "solarized");
        // Other fields keep defaults
        assert!(cfg.ui.markdown);
        assert!(cfg.ui.syntax_highlight);
        assert_eq!(cfg.ui.edit_mode, "emacs");
    }

    // ---- McpServerEntry ----

    #[test]
    fn mcp_server_entry_with_command() {
        let json = r#"{"command": "npx mcp-server", "args": ["--port", "3000"]}"#;
        let entry: McpServerEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.command, Some("npx mcp-server".into()));
        assert_eq!(entry.args, vec!["--port", "3000"]);
        assert!(entry.url.is_none());
    }

    #[test]
    fn mcp_server_entry_with_url() {
        let json = r#"{"url": "https://mcp.example.com/sse"}"#;
        let entry: McpServerEntry = serde_json::from_str(json).unwrap();
        assert!(entry.command.is_none());
        assert_eq!(entry.url, Some("https://mcp.example.com/sse".into()));
        assert!(entry.args.is_empty());
    }

    #[test]
    fn mcp_server_entry_with_env() {
        let json = r#"{"command": "server", "env": {"TOKEN": "abc"}}"#;
        let entry: McpServerEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.env.get("TOKEN").unwrap(), "abc");
    }

    // ---- PermissionRule serialization ----

    #[test]
    fn permission_rule_serde_roundtrip_with_pattern() {
        let rule = PermissionRule {
            tool: "Bash".into(),
            pattern: Some("rm -rf *".into()),
            action: PermissionMode::Deny,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let back: PermissionRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool, "Bash");
        assert_eq!(back.pattern, Some("rm -rf *".into()));
        assert_eq!(back.action, PermissionMode::Deny);
    }

    #[test]
    fn permission_rule_serde_roundtrip_without_pattern() {
        let rule = PermissionRule {
            tool: "Read".into(),
            pattern: None,
            action: PermissionMode::Allow,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let back: PermissionRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool, "Read");
        assert!(back.pattern.is_none());
        assert_eq!(back.action, PermissionMode::Allow);
    }

    // ---- Config with hooks in TOML ----

    #[test]
    fn config_toml_with_hooks() {
        let toml_str = r#"
[[hooks]]
event = "session_start"
tool_name = "Bash"

[hooks.action]
type = "shell"
command = "echo starting"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.hooks.len(), 1);
        assert_eq!(cfg.hooks[0].event, HookEvent::SessionStart);
        assert_eq!(cfg.hooks[0].tool_name, Some("Bash".into()));
    }

    // ---- Config with mcp_servers in TOML ----

    #[test]
    fn config_toml_with_mcp_servers() {
        let toml_str = r#"
[mcp_servers.my_server]
command = "npx my-mcp"
args = ["--flag"]
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(cfg.mcp_servers.contains_key("my_server"));
        let server = &cfg.mcp_servers["my_server"];
        assert_eq!(server.command, Some("npx my-mcp".into()));
        assert_eq!(server.args, vec!["--flag"]);
    }
}
