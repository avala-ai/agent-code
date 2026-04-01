//! HTTP streaming client for LLM APIs.
//!
//! Sends conversation messages to an LLM API and streams back response
//! events via Server-Sent Events (SSE). Features:
//!
//! - Prompt caching with cache_control markers
//! - Beta header negotiation (thinking, structured outputs, effort)
//! - Retry with exponential backoff and fallback model
//! - Tool choice constraints
//! - Thinking/reasoning token configuration

use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::LlmError;
use crate::llm::message::{Message, messages_to_api_params};
use crate::llm::stream::{RawSseEvent, StreamEvent, StreamParser};
use crate::tools::ToolSchema;

/// Client for communicating with an LLM API.
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

/// Configuration for thinking/reasoning behavior.
#[derive(Debug, Clone, Default)]
pub enum ThinkingMode {
    /// Let the model decide when to think.
    #[default]
    Adaptive,
    /// Always enable extended thinking with a token budget.
    Enabled { budget_tokens: u32 },
    /// Disable extended thinking.
    Disabled,
}

/// Controls how the model selects tools.
#[derive(Debug, Clone)]
pub enum ToolChoice {
    /// Model decides whether and which tools to use.
    Auto,
    /// Model must use the specified tool.
    Specific { name: String },
    /// Model must not use any tools.
    None,
}

/// Agent effort level (influences thoroughness and token usage).
#[derive(Debug, Clone, Copy)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
}

/// A request to the LLM API.
pub struct CompletionRequest<'a> {
    pub messages: &'a [Message],
    pub system_prompt: &'a str,
    pub tools: &'a [ToolSchema],
    pub max_tokens: Option<u32>,
    /// Tool selection constraint.
    pub tool_choice: Option<ToolChoice>,
    /// Thinking/reasoning configuration.
    pub thinking: Option<ThinkingMode>,
    /// Effort level for the response.
    pub effort: Option<EffortLevel>,
    /// JSON schema for structured output mode.
    pub output_schema: Option<serde_json::Value>,
    /// Enable prompt caching.
    pub enable_caching: bool,
    /// Fallback model if primary is overloaded.
    pub fallback_model: Option<String>,
    /// Temperature override.
    pub temperature: Option<f64>,
}

impl<'a> CompletionRequest<'a> {
    /// Create a simple request with just messages and system prompt.
    pub fn simple(
        messages: &'a [Message],
        system_prompt: &'a str,
        tools: &'a [ToolSchema],
        max_tokens: Option<u32>,
    ) -> Self {
        Self {
            messages,
            system_prompt,
            tools,
            max_tokens,
            tool_choice: None,
            thinking: None,
            effort: None,
            output_schema: None,
            enable_caching: true,
            fallback_model: None,
            temperature: None,
        }
    }
}

impl LlmClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    /// Stream a completion request, yielding `StreamEvent` values.
    pub async fn stream_completion(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<mpsc::Receiver<StreamEvent>, LlmError> {
        let model = request
            .fallback_model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        self.stream_with_model(&model, request).await
    }

    async fn stream_with_model(
        &self,
        model: &str,
        request: CompletionRequest<'_>,
    ) -> Result<mpsc::Receiver<StreamEvent>, LlmError> {
        let url = format!("{}/messages", self.base_url);

        // Build headers with beta features.
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key).map_err(|e| LlmError::AuthError(e.to_string()))?,
        );
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        // Collect beta features to enable.
        let mut betas: Vec<&str> = Vec::new();

        if request.thinking.is_some() {
            betas.push("interleaved-thinking-2025-05-14");
        }
        if request.output_schema.is_some() {
            betas.push("structured-outputs-2025-05-14");
        }
        if request.enable_caching {
            betas.push("prompt-caching-2024-07-31");
        }
        if request.effort.is_some() {
            betas.push("effort-control-2025-01-24");
        }

        if !betas.is_empty() {
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_str(&betas.join(",")).unwrap_or(HeaderValue::from_static("")),
            );
        }

        // Build tool definitions.
        let tools_json: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();

        // Build system prompt with cache control.
        let system = if request.enable_caching {
            serde_json::json!([{
                "type": "text",
                "text": request.system_prompt,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            serde_json::json!(request.system_prompt)
        };

        // Build request body.
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": request.max_tokens.unwrap_or(16384),
            "stream": true,
            "system": system,
            "messages": messages_to_api_params(request.messages),
            "tools": tools_json,
        });

        // Add optional parameters.
        if let Some(ref tc) = request.tool_choice {
            body["tool_choice"] = match tc {
                ToolChoice::Auto => serde_json::json!({"type": "auto"}),
                ToolChoice::Specific { name } => {
                    serde_json::json!({"type": "tool", "name": name})
                }
                ToolChoice::None => serde_json::json!({"type": "none"}),
            };
        }

        if let Some(ref thinking) = request.thinking {
            match thinking {
                ThinkingMode::Enabled { budget_tokens } => {
                    body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens,
                    });
                }
                ThinkingMode::Disabled => {
                    body["thinking"] = serde_json::json!({"type": "disabled"});
                }
                ThinkingMode::Adaptive => {
                    // Adaptive is the default — don't send explicit config.
                }
            }
        }

        if let Some(effort) = request.effort {
            let value = match effort {
                EffortLevel::Low => "low",
                EffortLevel::Medium => "medium",
                EffortLevel::High => "high",
            };
            body["metadata"] = serde_json::json!({
                "effort": value,
            });
        }

        if let Some(ref schema) = request.output_schema {
            body["output_schema"] = schema.clone();
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        debug!("API request to {url} (model={model})");

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();

            if status.as_u16() == 429 {
                let retry_after = parse_retry_after(&body_text);
                return Err(LlmError::RateLimited {
                    retry_after_ms: retry_after,
                });
            }

            if status.as_u16() == 529 {
                // Overloaded — treat like rate limit with longer backoff.
                return Err(LlmError::RateLimited {
                    retry_after_ms: 5000,
                });
            }

            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(LlmError::AuthError(body_text));
            }

            return Err(LlmError::Api {
                status: status.as_u16(),
                body: body_text,
            });
        }

        // Spawn SSE reader task.
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut parser = StreamParser::new();
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let start = std::time::Instant::now();
            let mut first_token = false;

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buffer.find("\n\n") {
                    let event_text = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    if let Some(data) = extract_sse_data(&event_text) {
                        if data == "[DONE]" {
                            return;
                        }

                        match serde_json::from_str::<RawSseEvent>(data) {
                            Ok(raw) => {
                                let events = parser.process(raw);
                                for event in events {
                                    if !first_token && matches!(event, StreamEvent::TextDelta(_)) {
                                        first_token = true;
                                        let ttft = start.elapsed().as_millis() as u64;
                                        let _ = tx.send(StreamEvent::Ttft(ttft)).await;
                                    }
                                    if tx.send(event).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("SSE parse error: {e}");
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }
}

/// Extract the `data:` payload from an SSE event block.
fn extract_sse_data(event_text: &str) -> Option<&str> {
    for line in event_text.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            return Some(data);
        }
        if let Some(data) = line.strip_prefix("data:") {
            return Some(data.trim_start());
        }
    }
    None
}

/// Try to parse a retry-after value from an error response.
fn parse_retry_after(body: &str) -> u64 {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(retry) = v
            .get("error")
            .and_then(|e| e.get("retry_after"))
            .and_then(|r| r.as_f64())
    {
        return (retry * 1000.0) as u64;
    }
    1000
}
