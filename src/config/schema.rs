//! Configuration schema definitions.

use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub api: ApiConfig,
    pub permissions: PermissionsConfig,
    pub ui: UiConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            permissions: PermissionsConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

/// API connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Base URL for the LLM API (OpenAI-compatible).
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// API key (prefer env var `RC_API_KEY`).
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// Maximum output tokens per response.
    pub max_output_tokens: Option<u32>,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum retry attempts for transient errors.
    pub max_retries: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com/v1".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_key: std::env::var("RC_API_KEY").ok(),
            max_output_tokens: Some(16384),
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
