//! LLM provider abstraction.
//!
//! Two wire formats cover the entire ecosystem:
//! - Anthropic Messages API (Claude models)
//! - OpenAI Chat Completions (GPT, plus Groq, Together, Ollama, DeepSeek, etc.)
//!
//! Each provider translates between our unified message types and
//! the provider-specific JSON format for requests and SSE streams.

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::message::Message;
use super::stream::StreamEvent;
use crate::tools::ToolSchema;

/// Unified provider trait. Both Anthropic and OpenAI-compatible
/// endpoints implement this.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Send a streaming request. Returns a channel of events.
    async fn stream(
        &self,
        request: &ProviderRequest,
    ) -> Result<mpsc::Receiver<StreamEvent>, ProviderError>;
}

/// Tool choice mode for controlling tool usage.
#[derive(Debug, Clone, Default)]
pub enum ToolChoice {
    /// Model decides whether to use tools.
    #[default]
    Auto,
    /// Model must use a tool.
    Any,
    /// Model must not use tools.
    None,
    /// Model must use a specific tool.
    Specific(String),
}

/// A provider-agnostic request.
pub struct ProviderRequest {
    pub messages: Vec<Message>,
    pub system_prompt: String,
    pub tools: Vec<ToolSchema>,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
    pub enable_caching: bool,
    /// Controls whether/how the model should use tools.
    pub tool_choice: ToolChoice,
    /// Metadata to send with the request (e.g., user_id for Anthropic).
    pub metadata: Option<serde_json::Value>,
}

/// Provider-level errors.
#[derive(Debug)]
pub enum ProviderError {
    Auth(String),
    RateLimited { retry_after_ms: u64 },
    Overloaded,
    RequestTooLarge(String),
    Network(String),
    InvalidResponse(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(msg) => write!(f, "auth: {msg}"),
            Self::RateLimited { retry_after_ms } => {
                write!(f, "rate limited (retry in {retry_after_ms}ms)")
            }
            Self::Overloaded => write!(f, "server overloaded"),
            Self::RequestTooLarge(msg) => write!(f, "request too large: {msg}"),
            Self::Network(msg) => write!(f, "network: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid response: {msg}"),
        }
    }
}

/// Detect the right provider from a model name or base URL.
pub fn detect_provider(model: &str, base_url: &str) -> ProviderKind {
    let model_lower = model.to_lowercase();
    let url_lower = base_url.to_lowercase();

    // AWS Bedrock (Claude via AWS).
    if url_lower.contains("bedrock") || url_lower.contains("amazonaws.com") {
        return ProviderKind::Bedrock;
    }
    // Google Vertex AI (Claude via GCP).
    if url_lower.contains("aiplatform.googleapis.com") {
        return ProviderKind::Vertex;
    }
    if url_lower.contains("anthropic.com") {
        return ProviderKind::Anthropic;
    }
    // Azure OpenAI — must be checked before generic openai.com.
    if url_lower.contains("openai.azure.com")
        || url_lower.contains("azure.com") && url_lower.contains("openai")
    {
        return ProviderKind::AzureOpenAi;
    }
    if url_lower.contains("openai.com") {
        return ProviderKind::OpenAi;
    }
    if url_lower.contains("x.ai") || url_lower.contains("xai.") {
        return ProviderKind::Xai;
    }
    if url_lower.contains("googleapis.com") || url_lower.contains("google") {
        return ProviderKind::Google;
    }
    if url_lower.contains("deepseek.com") {
        return ProviderKind::DeepSeek;
    }
    if url_lower.contains("groq.com") {
        return ProviderKind::Groq;
    }
    if url_lower.contains("mistral.ai") {
        return ProviderKind::Mistral;
    }
    if url_lower.contains("together.xyz") || url_lower.contains("together.ai") {
        return ProviderKind::Together;
    }
    if url_lower.contains("bigmodel.cn")
        || url_lower.contains("z.ai")
        || url_lower.contains("zhipu")
    {
        return ProviderKind::Zhipu;
    }
    if url_lower.contains("openrouter.ai") {
        return ProviderKind::OpenRouter;
    }
    if url_lower.contains("cohere.com") || url_lower.contains("cohere.ai") {
        return ProviderKind::Cohere;
    }
    if url_lower.contains("perplexity.ai") {
        return ProviderKind::Perplexity;
    }
    if url_lower.contains("localhost") || url_lower.contains("127.0.0.1") {
        return ProviderKind::OpenAiCompatible;
    }

    // Detect from model name.
    if model_lower.starts_with("claude")
        || model_lower.contains("opus")
        || model_lower.contains("sonnet")
        || model_lower.contains("haiku")
    {
        return ProviderKind::Anthropic;
    }
    if model_lower.starts_with("gpt")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
    {
        return ProviderKind::OpenAi;
    }
    if model_lower.starts_with("grok") {
        return ProviderKind::Xai;
    }
    if model_lower.starts_with("gemini") {
        return ProviderKind::Google;
    }
    if model_lower.starts_with("deepseek") {
        return ProviderKind::DeepSeek;
    }
    if model_lower.starts_with("llama") && url_lower.contains("groq") {
        return ProviderKind::Groq;
    }
    if model_lower.starts_with("mistral") || model_lower.starts_with("codestral") {
        return ProviderKind::Mistral;
    }
    if model_lower.starts_with("glm") {
        return ProviderKind::Zhipu;
    }
    if model_lower.starts_with("command") {
        return ProviderKind::Cohere;
    }
    if model_lower.starts_with("pplx") || model_lower.starts_with("sonar") {
        return ProviderKind::Perplexity;
    }

    ProviderKind::OpenAiCompatible
}

/// The two wire formats that cover the entire LLM ecosystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    /// Anthropic Messages API (Claude models, Bedrock, Vertex).
    Anthropic,
    /// OpenAI Chat Completions (GPT, Groq, Together, Ollama, DeepSeek, etc.).
    OpenAiCompatible,
}

/// Provider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Bedrock,
    Vertex,
    OpenAi,
    AzureOpenAi,
    Xai,
    Google,
    DeepSeek,
    Groq,
    Mistral,
    Together,
    Zhipu,
    OpenRouter,
    Cohere,
    Perplexity,
    OpenAiCompatible,
}

impl ProviderKind {
    /// Which wire format this provider uses.
    pub fn wire_format(&self) -> WireFormat {
        match self {
            Self::Anthropic | Self::Bedrock | Self::Vertex => WireFormat::Anthropic,
            Self::OpenAi
            | Self::AzureOpenAi
            | Self::Xai
            | Self::Google
            | Self::DeepSeek
            | Self::Groq
            | Self::Mistral
            | Self::Together
            | Self::Zhipu
            | Self::OpenRouter
            | Self::Cohere
            | Self::Perplexity
            | Self::OpenAiCompatible => WireFormat::OpenAiCompatible,
        }
    }

    /// The default base URL for this provider, or `None` for providers
    /// whose URL must come from user configuration (Bedrock, Vertex,
    /// and generic OpenAI-compatible endpoints).
    pub fn default_base_url(&self) -> Option<&str> {
        match self {
            Self::Anthropic => Some("https://api.anthropic.com/v1"),
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::Xai => Some("https://api.x.ai/v1"),
            Self::Google => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
            Self::DeepSeek => Some("https://api.deepseek.com/v1"),
            Self::Groq => Some("https://api.groq.com/openai/v1"),
            Self::Mistral => Some("https://api.mistral.ai/v1"),
            Self::Together => Some("https://api.together.xyz/v1"),
            Self::Zhipu => Some("https://open.bigmodel.cn/api/paas/v4"),
            Self::OpenRouter => Some("https://openrouter.ai/api/v1"),
            Self::Cohere => Some("https://api.cohere.com/v2"),
            Self::Perplexity => Some("https://api.perplexity.ai"),
            // These require user-supplied URLs.
            Self::Bedrock | Self::Vertex | Self::AzureOpenAi | Self::OpenAiCompatible => None,
        }
    }

    /// The environment variable name conventionally used for this provider's API key.
    pub fn env_var_name(&self) -> &str {
        match self {
            Self::Anthropic | Self::Bedrock | Self::Vertex => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::AzureOpenAi => "AZURE_OPENAI_API_KEY",
            Self::Xai => "XAI_API_KEY",
            Self::Google => "GOOGLE_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::Groq => "GROQ_API_KEY",
            Self::Mistral => "MISTRAL_API_KEY",
            Self::Together => "TOGETHER_API_KEY",
            Self::Zhipu => "ZHIPU_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Cohere => "COHERE_API_KEY",
            Self::Perplexity => "PERPLEXITY_API_KEY",
            Self::OpenAiCompatible => "OPENAI_API_KEY",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_from_url_anthropic() {
        assert!(matches!(
            detect_provider("any", "https://api.anthropic.com/v1"),
            ProviderKind::Anthropic
        ));
    }

    #[test]
    fn test_detect_from_url_openai() {
        assert!(matches!(
            detect_provider("any", "https://api.openai.com/v1"),
            ProviderKind::OpenAi
        ));
    }

    #[test]
    fn test_detect_from_url_bedrock() {
        assert!(matches!(
            detect_provider("any", "https://bedrock-runtime.us-east-1.amazonaws.com"),
            ProviderKind::Bedrock
        ));
    }

    #[test]
    fn test_detect_from_url_vertex() {
        assert!(matches!(
            detect_provider("any", "https://us-central1-aiplatform.googleapis.com/v1"),
            ProviderKind::Vertex
        ));
    }

    #[test]
    fn test_detect_from_url_azure_openai() {
        assert!(matches!(
            detect_provider(
                "any",
                "https://myresource.openai.azure.com/openai/deployments/gpt-4"
            ),
            ProviderKind::AzureOpenAi
        ));
    }

    #[test]
    fn test_detect_azure_before_generic_openai() {
        // Azure URL contains "openai" but should match Azure, not generic OpenAI.
        assert!(matches!(
            detect_provider(
                "gpt-4",
                "https://myresource.openai.azure.com/openai/deployments/gpt-4"
            ),
            ProviderKind::AzureOpenAi
        ));
    }

    #[test]
    fn test_detect_from_url_xai() {
        assert!(matches!(
            detect_provider("any", "https://api.x.ai/v1"),
            ProviderKind::Xai
        ));
    }

    #[test]
    fn test_detect_from_url_deepseek() {
        assert!(matches!(
            detect_provider("any", "https://api.deepseek.com/v1"),
            ProviderKind::DeepSeek
        ));
    }

    #[test]
    fn test_detect_from_url_groq() {
        assert!(matches!(
            detect_provider("any", "https://api.groq.com/openai/v1"),
            ProviderKind::Groq
        ));
    }

    #[test]
    fn test_detect_from_url_mistral() {
        assert!(matches!(
            detect_provider("any", "https://api.mistral.ai/v1"),
            ProviderKind::Mistral
        ));
    }

    #[test]
    fn test_detect_from_url_together() {
        assert!(matches!(
            detect_provider("any", "https://api.together.xyz/v1"),
            ProviderKind::Together
        ));
    }

    #[test]
    fn test_detect_from_url_cohere() {
        assert!(matches!(
            detect_provider("any", "https://api.cohere.com/v2"),
            ProviderKind::Cohere
        ));
    }

    #[test]
    fn test_detect_from_url_perplexity() {
        assert!(matches!(
            detect_provider("any", "https://api.perplexity.ai"),
            ProviderKind::Perplexity
        ));
    }

    #[test]
    fn test_detect_from_model_command_r() {
        assert!(matches!(
            detect_provider("command-r-plus", ""),
            ProviderKind::Cohere
        ));
    }

    #[test]
    fn test_detect_from_model_sonar() {
        assert!(matches!(
            detect_provider("sonar-pro", ""),
            ProviderKind::Perplexity
        ));
    }

    #[test]
    fn test_detect_from_url_openrouter() {
        assert!(matches!(
            detect_provider("any", "https://openrouter.ai/api/v1"),
            ProviderKind::OpenRouter
        ));
    }

    #[test]
    fn test_detect_from_url_localhost() {
        assert!(matches!(
            detect_provider("any", "http://localhost:11434/v1"),
            ProviderKind::OpenAiCompatible
        ));
    }

    #[test]
    fn test_detect_from_model_claude() {
        assert!(matches!(
            detect_provider("claude-sonnet-4", ""),
            ProviderKind::Anthropic
        ));
        assert!(matches!(
            detect_provider("claude-opus-4", ""),
            ProviderKind::Anthropic
        ));
    }

    #[test]
    fn test_detect_from_model_gpt() {
        assert!(matches!(
            detect_provider("gpt-4.1-mini", ""),
            ProviderKind::OpenAi
        ));
        assert!(matches!(
            detect_provider("o3-mini", ""),
            ProviderKind::OpenAi
        ));
    }

    #[test]
    fn test_detect_from_model_grok() {
        assert!(matches!(detect_provider("grok-3", ""), ProviderKind::Xai));
    }

    #[test]
    fn test_detect_from_model_gemini() {
        assert!(matches!(
            detect_provider("gemini-2.5-flash", ""),
            ProviderKind::Google
        ));
    }

    #[test]
    fn test_detect_unknown_defaults_openai_compat() {
        assert!(matches!(
            detect_provider("some-random-model", "https://my-server.com"),
            ProviderKind::OpenAiCompatible
        ));
    }

    #[test]
    fn test_url_takes_priority_over_model() {
        // URL says OpenAI but model says Claude — URL wins.
        assert!(matches!(
            detect_provider("claude-sonnet", "https://api.openai.com/v1"),
            ProviderKind::OpenAi
        ));
    }

    #[test]
    fn test_wire_format_anthropic_family() {
        assert_eq!(ProviderKind::Anthropic.wire_format(), WireFormat::Anthropic);
        assert_eq!(ProviderKind::Bedrock.wire_format(), WireFormat::Anthropic);
        assert_eq!(ProviderKind::Vertex.wire_format(), WireFormat::Anthropic);
    }

    #[test]
    fn test_wire_format_openai_compatible_family() {
        let openai_compat_providers = [
            ProviderKind::OpenAi,
            ProviderKind::Xai,
            ProviderKind::Google,
            ProviderKind::DeepSeek,
            ProviderKind::Groq,
            ProviderKind::Mistral,
            ProviderKind::Together,
            ProviderKind::Zhipu,
            ProviderKind::OpenRouter,
            ProviderKind::Cohere,
            ProviderKind::Perplexity,
            ProviderKind::OpenAiCompatible,
        ];
        for p in openai_compat_providers {
            assert_eq!(
                p.wire_format(),
                WireFormat::OpenAiCompatible,
                "{p:?} should use OpenAiCompatible wire format"
            );
        }
    }

    #[test]
    fn test_default_base_url_returns_some_for_known_providers() {
        let providers_with_urls = [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::Xai,
            ProviderKind::Google,
            ProviderKind::DeepSeek,
            ProviderKind::Groq,
            ProviderKind::Mistral,
            ProviderKind::Together,
            ProviderKind::Zhipu,
            ProviderKind::OpenRouter,
            ProviderKind::Cohere,
            ProviderKind::Perplexity,
        ];
        for p in providers_with_urls {
            assert!(
                p.default_base_url().is_some(),
                "{p:?} should have a default base URL"
            );
        }
    }

    #[test]
    fn test_default_base_url_returns_none_for_user_configured() {
        assert!(ProviderKind::Bedrock.default_base_url().is_none());
        assert!(ProviderKind::Vertex.default_base_url().is_none());
        assert!(ProviderKind::AzureOpenAi.default_base_url().is_none());
        assert!(ProviderKind::OpenAiCompatible.default_base_url().is_none());
    }

    #[test]
    fn test_env_var_name_all_variants() {
        assert_eq!(ProviderKind::Anthropic.env_var_name(), "ANTHROPIC_API_KEY");
        assert_eq!(ProviderKind::Bedrock.env_var_name(), "ANTHROPIC_API_KEY");
        assert_eq!(ProviderKind::Vertex.env_var_name(), "ANTHROPIC_API_KEY");
        assert_eq!(ProviderKind::OpenAi.env_var_name(), "OPENAI_API_KEY");
        assert_eq!(ProviderKind::AzureOpenAi.env_var_name(), "AZURE_OPENAI_API_KEY");
        assert_eq!(ProviderKind::Xai.env_var_name(), "XAI_API_KEY");
        assert_eq!(ProviderKind::Google.env_var_name(), "GOOGLE_API_KEY");
        assert_eq!(ProviderKind::DeepSeek.env_var_name(), "DEEPSEEK_API_KEY");
        assert_eq!(ProviderKind::Groq.env_var_name(), "GROQ_API_KEY");
        assert_eq!(ProviderKind::Mistral.env_var_name(), "MISTRAL_API_KEY");
        assert_eq!(ProviderKind::Together.env_var_name(), "TOGETHER_API_KEY");
        assert_eq!(ProviderKind::Zhipu.env_var_name(), "ZHIPU_API_KEY");
        assert_eq!(ProviderKind::OpenRouter.env_var_name(), "OPENROUTER_API_KEY");
        assert_eq!(ProviderKind::Cohere.env_var_name(), "COHERE_API_KEY");
        assert_eq!(ProviderKind::Perplexity.env_var_name(), "PERPLEXITY_API_KEY");
        assert_eq!(ProviderKind::OpenAiCompatible.env_var_name(), "OPENAI_API_KEY");
    }

    #[test]
    fn test_detect_from_url_zhipu_bigmodel() {
        assert!(matches!(
            detect_provider("any", "https://open.bigmodel.cn/api/paas/v4"),
            ProviderKind::Zhipu
        ));
    }

    #[test]
    fn test_detect_from_model_deepseek_chat() {
        assert!(matches!(
            detect_provider("deepseek-chat", ""),
            ProviderKind::DeepSeek
        ));
    }

    #[test]
    fn test_detect_from_model_mistral_large() {
        assert!(matches!(
            detect_provider("mistral-large", ""),
            ProviderKind::Mistral
        ));
    }

    #[test]
    fn test_detect_from_model_glm4() {
        assert!(matches!(
            detect_provider("glm-4", ""),
            ProviderKind::Zhipu
        ));
    }

    #[test]
    fn test_detect_from_model_llama3_with_groq_url() {
        assert!(matches!(
            detect_provider("llama-3", "https://api.groq.com/openai/v1"),
            ProviderKind::Groq
        ));
    }

    #[test]
    fn test_detect_from_model_codestral() {
        assert!(matches!(
            detect_provider("codestral-latest", ""),
            ProviderKind::Mistral
        ));
    }

    #[test]
    fn test_detect_from_model_pplx() {
        assert!(matches!(
            detect_provider("pplx-70b-online", ""),
            ProviderKind::Perplexity
        ));
    }

    #[test]
    fn test_provider_error_display() {
        let err = ProviderError::Auth("bad token".into());
        assert_eq!(format!("{err}"), "auth: bad token");

        let err = ProviderError::RateLimited { retry_after_ms: 1000 };
        assert_eq!(format!("{err}"), "rate limited (retry in 1000ms)");

        let err = ProviderError::Overloaded;
        assert_eq!(format!("{err}"), "server overloaded");

        let err = ProviderError::RequestTooLarge("4MB limit".into());
        assert_eq!(format!("{err}"), "request too large: 4MB limit");

        let err = ProviderError::Network("timeout".into());
        assert_eq!(format!("{err}"), "network: timeout");

        let err = ProviderError::InvalidResponse("missing field".into());
        assert_eq!(format!("{err}"), "invalid response: missing field");
    }

    #[test]
    fn test_tool_choice_default_is_auto() {
        let tc = ToolChoice::default();
        assert!(matches!(tc, ToolChoice::Auto));
    }
}
