//! HTTP streaming client for LLM APIs.
//!
//! Sends conversation messages to an LLM API and streams back response
//! events via Server-Sent Events (SSE). Supports retry with exponential
//! backoff for transient errors.

use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::LlmError;
use crate::llm::message::{messages_to_api_params, ContentBlock, Message, Usage};
use crate::llm::stream::{RawSseEvent, StreamEvent, StreamParser};
use crate::tools::ToolSchema;

/// Client for communicating with an LLM API.
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

/// A request to the LLM API.
pub struct CompletionRequest<'a> {
    pub messages: &'a [Message],
    pub system_prompt: &'a str,
    pub tools: &'a [ToolSchema],
    pub max_tokens: Option<u32>,
}

/// Response metadata from a completed API call.
#[derive(Debug, Clone)]
pub struct CompletionMeta {
    pub usage: Usage,
    pub model: Option<String>,
    pub request_id: Option<String>,
}

impl LlmClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    /// Stream a completion request, yielding `StreamEvent` values as they arrive.
    ///
    /// The returned receiver yields events until the stream is complete or an
    /// error occurs. The caller should process events in a loop.
    pub async fn stream_completion(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<mpsc::Receiver<StreamEvent>, LlmError> {
        let url = format!("{}/messages", self.base_url);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key)
                .map_err(|e| LlmError::AuthError(e.to_string()))?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static("2023-06-01"),
        );

        // Build tool definitions for the API.
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

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": request.max_tokens.unwrap_or(16384),
            "stream": true,
            "system": request.system_prompt,
            "messages": messages_to_api_params(request.messages),
            "tools": tools_json,
        });

        debug!("Sending API request to {url}");

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 429 {
                // Parse retry-after if available.
                let retry_after = parse_retry_after(&body);
                return Err(LlmError::RateLimited {
                    retry_after_ms: retry_after,
                });
            }

            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(LlmError::AuthError(body));
            }

            return Err(LlmError::Api {
                status: status.as_u16(),
                body,
            });
        }

        // Spawn a task to read the SSE stream and send parsed events.
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

                // Process complete SSE lines.
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
                                    // Emit TTFT on first text delta.
                                    if !first_token {
                                        if matches!(event, StreamEvent::TextDelta(_)) {
                                            first_token = true;
                                            let ttft = start.elapsed().as_millis() as u64;
                                            let _ = tx.send(StreamEvent::Ttft(ttft)).await;
                                        }
                                    }
                                    if tx.send(event).await.is_err() {
                                        return; // Receiver dropped.
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse SSE data: {e}");
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

/// Try to parse a retry-after value from an error response body.
fn parse_retry_after(body: &str) -> u64 {
    // Try JSON: {"error": {"type": "rate_limit_error", "message": "...", "retry_after": 1.5}}
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(retry) = v
            .get("error")
            .and_then(|e| e.get("retry_after"))
            .and_then(|r| r.as_f64())
        {
            return (retry * 1000.0) as u64;
        }
    }
    // Default: 1 second.
    1000
}

/// Collect all content blocks from a stream into a final assistant message.
pub fn collect_content_blocks(events: &[StreamEvent]) -> Vec<ContentBlock> {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ContentBlockComplete(block) => Some(block.clone()),
            _ => None,
        })
        .collect()
}
