//! Anthropic Messages API provider.
//!
//! Native support for Claude models. Uses the Anthropic-specific
//! wire format: top-level system param, content block arrays,
//! tool definitions with input_schema, and SSE streaming with
//! content_block_start/delta/stop events.

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::message::{messages_to_api_params, messages_to_api_params_cached};
use super::provider::{Provider, ProviderError, ProviderRequest};
use super::stream::{RawSseEvent, StreamEvent, StreamParser};

/// Anthropic Messages API provider (Claude models, Bedrock, Vertex).
pub struct AnthropicProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn stream(
        &self,
        request: &ProviderRequest,
    ) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        let url = format!("{}/messages", self.base_url);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key).map_err(|e| ProviderError::Auth(e.to_string()))?,
        );
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        // Enable beta features.
        let mut betas = Vec::new();
        betas.push("interleaved-thinking-2025-05-14"); // Extended thinking.
        if request.enable_caching {
            betas.push("prompt-caching-2024-07-31");
        }
        if !betas.is_empty() {
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_str(&betas.join(",")).unwrap_or(HeaderValue::from_static("")),
            );
        }

        // Build tool definitions in Anthropic format.
        // When caching is enabled, add cache_control to the last tool definition.
        // This causes the API to cache the entire tools block (system prompt +
        // tools are cached together as a prefix).
        let tool_count = request.tools.len();
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut tool = serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                });
                if request.enable_caching && i == tool_count - 1 && tool_count > 0 {
                    tool["cache_control"] = serde_json::json!({"type": "ephemeral"});
                }
                tool
            })
            .collect();

        // System prompt with optional cache control.
        let system = if request.enable_caching {
            serde_json::json!([{
                "type": "text",
                "text": request.system_prompt,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            serde_json::json!(request.system_prompt)
        };

        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "stream": true,
            "system": system,
            "messages": if request.enable_caching {
                messages_to_api_params_cached(&request.messages)
            } else {
                messages_to_api_params(&request.messages)
            },
            "tools": tools,
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        // Tool choice.
        if !request.tools.is_empty() {
            use super::provider::ToolChoice;
            match &request.tool_choice {
                ToolChoice::Auto => {
                    body["tool_choice"] = serde_json::json!({"type": "auto"});
                }
                ToolChoice::Any => {
                    body["tool_choice"] = serde_json::json!({"type": "any"});
                }
                ToolChoice::None => {
                    // Anthropic doesn't have a "none" tool_choice — just omit tools.
                    body.as_object_mut().unwrap().remove("tools");
                }
                ToolChoice::Specific(name) => {
                    body["tool_choice"] = serde_json::json!({
                        "type": "tool",
                        "name": name
                    });
                }
            }
        }

        // Metadata (e.g., user_id for analytics).
        if let Some(ref meta) = request.metadata {
            body["metadata"] = meta.clone();
        }

        // Thinking configuration (adaptive or budgeted).
        let thinking_budget =
            crate::services::tokens::max_thinking_tokens_for_model(&request.model);
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": thinking_budget,
        });

        debug!("Anthropic request to {url} (thinking budget: {thinking_budget})");

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return match status.as_u16() {
                401 | 403 => Err(ProviderError::Auth(body_text)),
                429 => {
                    let retry = parse_retry_after(&body_text);
                    Err(ProviderError::RateLimited {
                        retry_after_ms: retry,
                    })
                }
                529 => Err(ProviderError::Overloaded),
                413 => Err(ProviderError::RequestTooLarge(body_text)),
                _ => Err(ProviderError::Network(format!("{status}: {body_text}"))),
            };
        }

        // Parse Anthropic SSE stream (reuses existing StreamParser).
        let (tx, rx) = mpsc::channel(64);
        let cancel = request.cancel.clone();
        tokio::spawn(async move {
            let mut parser = StreamParser::new();
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let start = std::time::Instant::now();
            let mut first_token = false;

            loop {
                // Race the next SSE chunk against cancellation. On cancel,
                // drop the byte_stream (and therefore the reqwest::Response),
                // which aborts the underlying HTTP connection immediately.
                let chunk_result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    chunk = byte_stream.next() => match chunk {
                        Some(c) => c,
                        None => break,
                    },
                };
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
