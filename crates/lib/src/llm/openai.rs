//! OpenAI Chat Completions provider.
//!
//! Handles GPT models and any OpenAI-compatible endpoint (Groq,
//! Together, Ollama, DeepSeek, OpenRouter, vLLM, LMStudio, etc.).
//! The only difference between providers is the base URL and auth.

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use tokio::sync::mpsc;
use tracing::debug;

use super::message::{ContentBlock, Message, StopReason, Usage};
use super::provider::{Provider, ProviderError, ProviderRequest};
use super::stream::StreamEvent;

pub struct OpenAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
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

    /// Build the request body in OpenAI format.
    fn build_body(&self, request: &ProviderRequest) -> serde_json::Value {
        // Convert our messages to OpenAI format.
        // Key difference: system message goes in the messages array, not separate.
        let mut messages = Vec::new();

        // System message as first message.
        if !request.system_prompt.is_empty() {
            messages.push(serde_json::json!({
                "role": "system",
                "content": request.system_prompt,
            }));
        }

        // Convert conversation messages.
        for msg in &request.messages {
            match msg {
                Message::User(u) => {
                    let content = blocks_to_openai_content(&u.content);
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content,
                    }));
                }
                Message::Assistant(a) => {
                    let mut msg_json = serde_json::json!({
                        "role": "assistant",
                    });

                    // Check for tool calls.
                    let tool_calls: Vec<serde_json::Value> = a
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse { id, name, input } => Some(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(input).unwrap_or_default(),
                                }
                            })),
                            _ => None,
                        })
                        .collect();

                    // Text content.
                    let text: String = a
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    // OpenAI requires content to be a string, never null.
                    msg_json["content"] = serde_json::Value::String(text);
                    if !tool_calls.is_empty() {
                        msg_json["tool_calls"] = serde_json::Value::Array(tool_calls);
                    }

                    messages.push(msg_json);
                }
                Message::System(_) => {} // Already handled above.
            }
        }

        // Handle tool results (OpenAI uses role: "tool").
        // We need a second pass to convert our tool_result content blocks.
        let mut final_messages = Vec::new();
        for msg in messages {
            if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                // Check if this is actually a tool result message.
                if let Some(content) = msg.get("content")
                    && let Some(arr) = content.as_array()
                {
                    let mut tool_results = Vec::new();
                    let mut other_content = Vec::new();

                    for block in arr {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                            tool_results.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": block.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or(""),
                                    "content": block.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                                }));
                        } else {
                            other_content.push(block.clone());
                        }
                    }

                    if !tool_results.is_empty() {
                        // Emit tool results as separate messages.
                        for tr in tool_results {
                            final_messages.push(tr);
                        }
                        if !other_content.is_empty() {
                            let mut m = msg.clone();
                            m["content"] = serde_json::Value::Array(other_content);
                            final_messages.push(m);
                        }
                        continue;
                    }
                }
            }
            final_messages.push(msg);
        }

        // Build tools in OpenAI format.
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();

        // Newer models (o1, o3, gpt-5.x) use max_completion_tokens.
        let model_lower = request.model.to_lowercase();
        let uses_new_token_param = model_lower.starts_with("o1")
            || model_lower.starts_with("o3")
            || model_lower.contains("gpt-5")
            || model_lower.contains("gpt-4.1");

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": final_messages,
            "stream": true,
        });

        if uses_new_token_param {
            body["max_completion_tokens"] = serde_json::json!(request.max_tokens);
        } else {
            body["max_tokens"] = serde_json::json!(request.max_tokens);
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);

            // Tool choice.
            use super::provider::ToolChoice;
            match &request.tool_choice {
                ToolChoice::Auto => {
                    body["tool_choice"] = serde_json::json!("auto");
                }
                ToolChoice::Any => {
                    body["tool_choice"] = serde_json::json!("required");
                }
                ToolChoice::None => {
                    body["tool_choice"] = serde_json::json!("none");
                }
                ToolChoice::Specific(name) => {
                    body["tool_choice"] = serde_json::json!({
                        "type": "function",
                        "function": { "name": name }
                    });
                }
            }
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        body
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn stream(
        &self,
        request: &ProviderRequest,
    ) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = self.build_body(request);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|e| ProviderError::Auth(e.to_string()))?,
        );

        debug!("OpenAI request to {url}");

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
                429 => Err(ProviderError::RateLimited {
                    retry_after_ms: 1000,
                }),
                529 => Err(ProviderError::Overloaded),
                413 => Err(ProviderError::RequestTooLarge(body_text)),
                _ => Err(ProviderError::Network(format!("{status}: {body_text}"))),
            };
        }

        // Parse OpenAI SSE stream.
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_args = String::new();
            let mut usage = Usage::default();
            let mut stop_reason: Option<StopReason> = None;

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

                    for line in event_text.lines() {
                        let data = if let Some(d) = line.strip_prefix("data: ") {
                            d
                        } else {
                            continue;
                        };

                        if data == "[DONE]" {
                            // Emit any remaining tool call before Done.
                            if !current_tool_id.is_empty() {
                                let input: serde_json::Value =
                                    serde_json::from_str(&current_tool_args).unwrap_or_default();
                                let _ = tx
                                    .send(StreamEvent::ContentBlockComplete(
                                        ContentBlock::ToolUse {
                                            id: current_tool_id.clone(),
                                            name: current_tool_name.clone(),
                                            input,
                                        },
                                    ))
                                    .await;
                                current_tool_id.clear();
                                current_tool_name.clear();
                                current_tool_args.clear();
                            }

                            let _ = tx
                                .send(StreamEvent::Done {
                                    usage: usage.clone(),
                                    stop_reason: stop_reason.clone().or(Some(StopReason::EndTurn)),
                                })
                                .await;
                            return;
                        }

                        let parsed: serde_json::Value = match serde_json::from_str(data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        // Extract delta from choices[0].delta
                        let delta = match parsed
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("delta"))
                        {
                            Some(d) => d,
                            None => {
                                // Check for usage in the final chunk.
                                if let Some(u) = parsed.get("usage") {
                                    usage.input_tokens = u
                                        .get("prompt_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                    usage.output_tokens = u
                                        .get("completion_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                }
                                continue;
                            }
                        };

                        // Text content.
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                            && !content.is_empty()
                        {
                            debug!("OpenAI text delta: {}", &content[..content.len().min(80)]);
                            let _ = tx.send(StreamEvent::TextDelta(content.to_string())).await;
                        }

                        // Check for finish_reason on the choice level.
                        if let Some(finish) = parsed
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("finish_reason"))
                            .and_then(|f| f.as_str())
                        {
                            debug!("OpenAI finish_reason: {finish}");
                            match finish {
                                "stop" => {
                                    stop_reason = Some(StopReason::EndTurn);
                                }
                                "tool_calls" => {
                                    stop_reason = Some(StopReason::ToolUse);
                                }
                                "length" => {
                                    stop_reason = Some(StopReason::MaxTokens);
                                }
                                _ => {}
                            }
                        }

                        // Tool calls.
                        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array())
                        {
                            for tc in tool_calls {
                                if let Some(func) = tc.get("function") {
                                    if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                                        // New tool call starting.
                                        if !current_tool_id.is_empty()
                                            && !current_tool_args.is_empty()
                                        {
                                            // Emit the previous tool call.
                                            let input: serde_json::Value =
                                                serde_json::from_str(&current_tool_args)
                                                    .unwrap_or_default();
                                            let _ = tx
                                                .send(StreamEvent::ContentBlockComplete(
                                                    ContentBlock::ToolUse {
                                                        id: current_tool_id.clone(),
                                                        name: current_tool_name.clone(),
                                                        input,
                                                    },
                                                ))
                                                .await;
                                        }
                                        current_tool_id = tc
                                            .get("id")
                                            .and_then(|i| i.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        current_tool_name = name.to_string();
                                        current_tool_args.clear();
                                    }
                                    if let Some(args) =
                                        func.get("arguments").and_then(|a| a.as_str())
                                    {
                                        current_tool_args.push_str(args);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Emit any remaining tool call.
            if !current_tool_id.is_empty() {
                let input: serde_json::Value =
                    serde_json::from_str(&current_tool_args).unwrap_or_default();
                let _ = tx
                    .send(StreamEvent::ContentBlockComplete(ContentBlock::ToolUse {
                        id: current_tool_id,
                        name: current_tool_name,
                        input,
                    }))
                    .await;
            }

            let _ = tx
                .send(StreamEvent::Done {
                    usage,
                    stop_reason: Some(StopReason::EndTurn),
                })
                .await;
        });

        Ok(rx)
    }
}

/// Convert content blocks to OpenAI format.
fn blocks_to_openai_content(blocks: &[ContentBlock]) -> serde_json::Value {
    if blocks.len() == 1
        && let ContentBlock::Text { text } = &blocks[0]
    {
        return serde_json::Value::String(text.clone());
    }

    let parts: Vec<serde_json::Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => serde_json::json!({
                "type": "text",
                "text": text,
            }),
            ContentBlock::Image { media_type, data } => serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{media_type};base64,{data}"),
                }
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
            ContentBlock::Thinking { thinking, .. } => serde_json::json!({
                "type": "text",
                "text": thinking,
            }),
            ContentBlock::ToolUse { name, input, .. } => serde_json::json!({
                "type": "text",
                "text": format!("[Tool call: {name}({input})]"),
            }),
            ContentBlock::Document { title, .. } => serde_json::json!({
                "type": "text",
                "text": format!("[Document: {}]", title.as_deref().unwrap_or("untitled")),
            }),
        })
        .collect();

    serde_json::Value::Array(parts)
}
