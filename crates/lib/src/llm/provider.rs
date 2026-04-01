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

    ProviderKind::OpenAiCompatible
}

/// Provider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Bedrock,
    Vertex,
    OpenAi,
    Xai,
    Google,
    DeepSeek,
    Groq,
    Mistral,
    Together,
    OpenAiCompatible,
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
}
