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
