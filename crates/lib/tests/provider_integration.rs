//! Integration tests for provider detection and configuration.
//!
//! Tests detect_provider URL and model name mapping, ProviderKind
//! wire format, default base URLs, and environment variable names.

use agent_code_lib::llm::provider::{ProviderKind, WireFormat, detect_provider};

// ---------------------------------------------------------------------------
// detect_provider: URL-based detection (all providers)
// ---------------------------------------------------------------------------

#[test]
fn detect_from_url_anthropic() {
    assert_eq!(
        detect_provider("any-model", "https://api.anthropic.com/v1"),
        ProviderKind::Anthropic
    );
}

#[test]
fn detect_from_url_bedrock() {
    assert_eq!(
        detect_provider(
            "any-model",
            "https://bedrock-runtime.us-east-1.amazonaws.com"
        ),
        ProviderKind::Bedrock
    );
}

#[test]
fn detect_from_url_vertex() {
    assert_eq!(
        detect_provider(
            "any-model",
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project"
        ),
        ProviderKind::Vertex
    );
}

#[test]
fn detect_from_url_openai() {
    assert_eq!(
        detect_provider("any-model", "https://api.openai.com/v1"),
        ProviderKind::OpenAi
    );
}

#[test]
fn detect_from_url_azure_openai() {
    assert_eq!(
        detect_provider(
            "gpt-4",
            "https://myresource.openai.azure.com/openai/deployments/gpt-4"
        ),
        ProviderKind::AzureOpenAi
    );
}

#[test]
fn detect_from_url_xai() {
    assert_eq!(
        detect_provider("any-model", "https://api.x.ai/v1"),
        ProviderKind::Xai
    );
}

#[test]
fn detect_from_url_google() {
    assert_eq!(
        detect_provider(
            "any-model",
            "https://generativelanguage.googleapis.com/v1beta/openai"
        ),
        ProviderKind::Google
    );
}

#[test]
fn detect_from_url_deepseek() {
    assert_eq!(
        detect_provider("any-model", "https://api.deepseek.com/v1"),
        ProviderKind::DeepSeek
    );
}

#[test]
fn detect_from_url_groq() {
    assert_eq!(
        detect_provider("any-model", "https://api.groq.com/openai/v1"),
        ProviderKind::Groq
    );
}

#[test]
fn detect_from_url_mistral() {
    assert_eq!(
        detect_provider("any-model", "https://api.mistral.ai/v1"),
        ProviderKind::Mistral
    );
}

#[test]
fn detect_from_url_together() {
    assert_eq!(
        detect_provider("any-model", "https://api.together.xyz/v1"),
        ProviderKind::Together
    );
}

#[test]
fn detect_from_url_together_ai_domain() {
    assert_eq!(
        detect_provider("any-model", "https://api.together.ai/v1"),
        ProviderKind::Together
    );
}

#[test]
fn detect_from_url_zhipu() {
    assert_eq!(
        detect_provider("any-model", "https://open.bigmodel.cn/api/paas/v4"),
        ProviderKind::Zhipu
    );
}

#[test]
fn detect_from_url_openrouter() {
    assert_eq!(
        detect_provider("any-model", "https://openrouter.ai/api/v1"),
        ProviderKind::OpenRouter
    );
}

#[test]
fn detect_from_url_cohere() {
    assert_eq!(
        detect_provider("any-model", "https://api.cohere.com/v2"),
        ProviderKind::Cohere
    );
}

#[test]
fn detect_from_url_perplexity() {
    assert_eq!(
        detect_provider("any-model", "https://api.perplexity.ai"),
        ProviderKind::Perplexity
    );
}

#[test]
fn detect_from_url_localhost_is_openai_compatible() {
    assert_eq!(
        detect_provider("any-model", "http://localhost:11434/v1"),
        ProviderKind::OpenAiCompatible
    );
    assert_eq!(
        detect_provider("any-model", "http://127.0.0.1:8080"),
        ProviderKind::OpenAiCompatible
    );
}

// ---------------------------------------------------------------------------
// detect_provider: model name-based detection
// ---------------------------------------------------------------------------

#[test]
fn detect_from_model_claude_variants() {
    assert_eq!(
        detect_provider("claude-sonnet-4-20250514", ""),
        ProviderKind::Anthropic
    );
    assert_eq!(
        detect_provider("claude-opus-4", ""),
        ProviderKind::Anthropic
    );
    assert_eq!(
        detect_provider("claude-3-5-haiku-latest", ""),
        ProviderKind::Anthropic
    );
}

#[test]
fn detect_from_model_gpt_variants() {
    assert_eq!(detect_provider("gpt-4.1-mini", ""), ProviderKind::OpenAi);
    assert_eq!(detect_provider("gpt-5.4", ""), ProviderKind::OpenAi);
    assert_eq!(detect_provider("o1-preview", ""), ProviderKind::OpenAi);
    assert_eq!(detect_provider("o3-mini", ""), ProviderKind::OpenAi);
}

#[test]
fn detect_from_model_grok() {
    assert_eq!(detect_provider("grok-3", ""), ProviderKind::Xai);
}

#[test]
fn detect_from_model_gemini() {
    assert_eq!(
        detect_provider("gemini-2.5-flash", ""),
        ProviderKind::Google
    );
}

#[test]
fn detect_from_model_deepseek() {
    assert_eq!(
        detect_provider("deepseek-coder-v2", ""),
        ProviderKind::DeepSeek
    );
}

#[test]
fn detect_from_model_mistral_and_codestral() {
    assert_eq!(detect_provider("mistral-large", ""), ProviderKind::Mistral);
    assert_eq!(
        detect_provider("codestral-latest", ""),
        ProviderKind::Mistral
    );
}

#[test]
fn detect_from_model_glm() {
    assert_eq!(detect_provider("glm-4", ""), ProviderKind::Zhipu);
}

#[test]
fn detect_from_model_command() {
    assert_eq!(detect_provider("command-r-plus", ""), ProviderKind::Cohere);
}

#[test]
fn detect_from_model_pplx_and_sonar() {
    assert_eq!(
        detect_provider("pplx-70b-online", ""),
        ProviderKind::Perplexity
    );
    assert_eq!(detect_provider("sonar-pro", ""), ProviderKind::Perplexity);
}

#[test]
fn detect_unknown_model_and_url_defaults_to_openai_compatible() {
    assert_eq!(
        detect_provider("some-random-model", "https://my-custom-server.com"),
        ProviderKind::OpenAiCompatible
    );
}

// ---------------------------------------------------------------------------
// URL detection takes priority over model name
// ---------------------------------------------------------------------------

#[test]
fn url_detection_takes_priority_over_model_name() {
    // Model says "claude" but URL says OpenAI -> URL wins.
    assert_eq!(
        detect_provider("claude-sonnet-4", "https://api.openai.com/v1"),
        ProviderKind::OpenAi
    );

    // Model says "gpt" but URL says Anthropic -> URL wins.
    assert_eq!(
        detect_provider("gpt-4", "https://api.anthropic.com/v1"),
        ProviderKind::Anthropic
    );

    // Model says "gemini" but URL says Groq -> URL wins.
    assert_eq!(
        detect_provider("gemini-pro", "https://api.groq.com/openai/v1"),
        ProviderKind::Groq
    );
}

// ---------------------------------------------------------------------------
// ProviderKind::wire_format()
// ---------------------------------------------------------------------------

#[test]
fn wire_format_anthropic_family() {
    assert_eq!(ProviderKind::Anthropic.wire_format(), WireFormat::Anthropic);
    assert_eq!(ProviderKind::Bedrock.wire_format(), WireFormat::Anthropic);
    assert_eq!(ProviderKind::Vertex.wire_format(), WireFormat::Anthropic);
}

#[test]
fn wire_format_openai_compatible_family() {
    let openai_compat_providers = [
        ProviderKind::OpenAi,
        ProviderKind::AzureOpenAi,
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

    for provider in openai_compat_providers {
        assert_eq!(
            provider.wire_format(),
            WireFormat::OpenAiCompatible,
            "{provider:?} should use OpenAiCompatible wire format"
        );
    }
}

// ---------------------------------------------------------------------------
// ProviderKind::default_base_url()
// ---------------------------------------------------------------------------

#[test]
fn default_base_url_returns_correct_urls() {
    assert_eq!(
        ProviderKind::Anthropic.default_base_url(),
        Some("https://api.anthropic.com/v1")
    );
    assert_eq!(
        ProviderKind::OpenAi.default_base_url(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(
        ProviderKind::Xai.default_base_url(),
        Some("https://api.x.ai/v1")
    );
    assert_eq!(
        ProviderKind::DeepSeek.default_base_url(),
        Some("https://api.deepseek.com/v1")
    );
    assert_eq!(
        ProviderKind::Groq.default_base_url(),
        Some("https://api.groq.com/openai/v1")
    );
    assert_eq!(
        ProviderKind::Mistral.default_base_url(),
        Some("https://api.mistral.ai/v1")
    );
    assert_eq!(
        ProviderKind::Together.default_base_url(),
        Some("https://api.together.xyz/v1")
    );
    assert_eq!(
        ProviderKind::Zhipu.default_base_url(),
        Some("https://open.bigmodel.cn/api/paas/v4")
    );
    assert_eq!(
        ProviderKind::OpenRouter.default_base_url(),
        Some("https://openrouter.ai/api/v1")
    );
    assert_eq!(
        ProviderKind::Cohere.default_base_url(),
        Some("https://api.cohere.com/v2")
    );
    assert_eq!(
        ProviderKind::Perplexity.default_base_url(),
        Some("https://api.perplexity.ai")
    );
    assert_eq!(
        ProviderKind::Google.default_base_url(),
        Some("https://generativelanguage.googleapis.com/v1beta/openai")
    );
}

#[test]
fn providers_requiring_user_url_return_none() {
    assert!(ProviderKind::Bedrock.default_base_url().is_none());
    assert!(ProviderKind::Vertex.default_base_url().is_none());
    assert!(ProviderKind::AzureOpenAi.default_base_url().is_none());
    assert!(ProviderKind::OpenAiCompatible.default_base_url().is_none());
}

// ---------------------------------------------------------------------------
// ProviderKind::env_var_name()
// ---------------------------------------------------------------------------

#[test]
fn env_var_names_are_correct() {
    assert_eq!(ProviderKind::Anthropic.env_var_name(), "ANTHROPIC_API_KEY");
    assert_eq!(ProviderKind::Bedrock.env_var_name(), "ANTHROPIC_API_KEY");
    assert_eq!(ProviderKind::Vertex.env_var_name(), "ANTHROPIC_API_KEY");
    assert_eq!(ProviderKind::OpenAi.env_var_name(), "OPENAI_API_KEY");
    assert_eq!(
        ProviderKind::AzureOpenAi.env_var_name(),
        "AZURE_OPENAI_API_KEY"
    );
    assert_eq!(ProviderKind::Xai.env_var_name(), "XAI_API_KEY");
    assert_eq!(ProviderKind::Google.env_var_name(), "GOOGLE_API_KEY");
    assert_eq!(ProviderKind::DeepSeek.env_var_name(), "DEEPSEEK_API_KEY");
    assert_eq!(ProviderKind::Groq.env_var_name(), "GROQ_API_KEY");
    assert_eq!(ProviderKind::Mistral.env_var_name(), "MISTRAL_API_KEY");
    assert_eq!(ProviderKind::Together.env_var_name(), "TOGETHER_API_KEY");
    assert_eq!(ProviderKind::Zhipu.env_var_name(), "ZHIPU_API_KEY");
    assert_eq!(
        ProviderKind::OpenRouter.env_var_name(),
        "OPENROUTER_API_KEY"
    );
    assert_eq!(ProviderKind::Cohere.env_var_name(), "COHERE_API_KEY");
    assert_eq!(
        ProviderKind::Perplexity.env_var_name(),
        "PERPLEXITY_API_KEY"
    );
    assert_eq!(
        ProviderKind::OpenAiCompatible.env_var_name(),
        "OPENAI_API_KEY"
    );
}

// ---------------------------------------------------------------------------
// WireFormat equality comparisons
// ---------------------------------------------------------------------------

#[test]
fn wire_format_equality() {
    assert_eq!(WireFormat::Anthropic, WireFormat::Anthropic);
    assert_eq!(WireFormat::OpenAiCompatible, WireFormat::OpenAiCompatible);
    assert_ne!(WireFormat::Anthropic, WireFormat::OpenAiCompatible);
}

// ---------------------------------------------------------------------------
// Exhaustive: every provider kind is covered by wire_format
// ---------------------------------------------------------------------------

#[test]
fn all_provider_kinds_have_wire_format() {
    // Verify every variant returns a valid wire format without panicking.
    let all_providers = [
        ProviderKind::Anthropic,
        ProviderKind::Bedrock,
        ProviderKind::Vertex,
        ProviderKind::OpenAi,
        ProviderKind::AzureOpenAi,
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

    for provider in all_providers {
        let wf = provider.wire_format();
        assert!(wf == WireFormat::Anthropic || wf == WireFormat::OpenAiCompatible);
    }
}
