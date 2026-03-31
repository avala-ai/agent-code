//! Configuration schema definitions.

use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub api: ApiConfig,
    pub permissions: PermissionsConfig,
    pub ui: UiConfig,
    /// MCP server configurations.
    #[serde(default)]
    pub mcp_servers: std::collections::HashMap<String, McpServerEntry>,
    /// Lifecycle hooks.
    #[serde(default)]
    pub hooks: Vec<crate::hooks::HookDefinition>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Base URL for the LLM API.
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// API key. Resolved from (in order): config, RC_API_KEY,
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
        let api_key = std::env::var("RC_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .ok();

        // Auto-detect base URL from which key is set.
        let base_url = if std::env::var("OPENAI_API_KEY").is_ok()
            && std::env::var("RC_API_KEY").is_err()
            && std::env::var("ANTHROPIC_API_KEY").is_err()
        {
            "https://api.openai.com/v1".to_string()
        } else {
            "https://api.anthropic.com/v1".to_string()
        };

        Self {
            base_url,
            model: "claude-sonnet-4-20250514".to_string(),
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

/// Permission mode.
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
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            markdown: true,
            syntax_highlight: true,
            theme: "dark".to_string(),
        }
    }
}
