//! OpenAI Chat Completions provider.
//!
//! Handles GPT models and any OpenAI-compatible endpoint (Groq,
//! Together, Ollama, DeepSeek, OpenRouter, vLLM, LMStudio, etc.).
//! The only difference between providers is the base URL and auth.

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::debug;

use super::codex_auth::CodexChatGptAuth;
use super::message::{ContentBlock, Message, StopReason, Usage};
use super::provider::{Provider, ProviderError, ProviderRequest, ToolChoice};
use super::stream::StreamEvent;

/// OpenAI Chat Completions provider (GPT, Groq, Together, DeepSeek, etc.).
pub struct OpenAiProvider {
    http: reqwest::Client,
    base_url: String,
    auth: OpenAiAuth,
    api: OpenAiApi,
}

enum OpenAiAuth {
    ApiKey(String),
    CodexChatGpt(CodexChatGptAuth),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenAiApi {
    ChatCompletions,
    Responses,
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
            auth: OpenAiAuth::ApiKey(api_key.to_string()),
            api: OpenAiApi::ChatCompletions,
        }
    }

    pub fn new_responses_with_codex_auth(base_url: &str, auth: CodexChatGptAuth) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            auth: OpenAiAuth::CodexChatGpt(auth),
            api: OpenAiApi::Responses,
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
            "stream_options": { "include_usage": true },
        });

        if uses_new_token_param {
            body["max_completion_tokens"] = serde_json::json!(request.max_tokens);
        } else {
            body["max_tokens"] = serde_json::json!(request.max_tokens);
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);

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

    async fn auth_headers(&self) -> Result<HeaderMap, ProviderError> {
        match &self.auth {
            OpenAiAuth::ApiKey(api_key) => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {api_key}"))
                        .map_err(|e| ProviderError::Auth(e.to_string()))?,
                );
                Ok(headers)
            }
            OpenAiAuth::CodexChatGpt(auth) => auth.auth_headers().await,
        }
    }

    fn build_responses_body(&self, request: &ProviderRequest) -> serde_json::Value {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": request.model,
            "input": messages_to_responses_input(&request.messages),
            "stream": true,
            "store": false,
            "tools": tools,
            "tool_choice": responses_tool_choice(&request.tool_choice),
            "parallel_tool_calls": false,
            "include": [],
        });

        if !request.system_prompt.is_empty() {
            body["instructions"] = serde_json::Value::String(request.system_prompt.clone());
        }

        body
    }

    async fn stream_chat_completions(
        &self,
        request: &ProviderRequest,
    ) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = self.build_body(request);

        let mut headers = self.auth_headers().await?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

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

        Ok(spawn_chat_completions_stream(
            response,
            request.cancel.clone(),
        ))
    }

    async fn stream_responses(
        &self,
        request: &ProviderRequest,
    ) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        let url = format!("{}/responses", self.base_url);
        let body = self.build_responses_body(request);

        let mut headers = self.auth_headers().await?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        debug!("OpenAI Responses request to {url}");

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

        Ok(spawn_responses_stream(response, request.cancel.clone()))
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
        match self.api {
            OpenAiApi::ChatCompletions => self.stream_chat_completions(request).await,
            OpenAiApi::Responses => self.stream_responses(request).await,
        }
    }
}

fn spawn_chat_completions_stream(
    response: reqwest::Response,
    cancel: tokio_util::sync::CancellationToken,
) -> mpsc::Receiver<StreamEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_args = String::new();
        let mut usage = Usage::default();
        let mut stop_reason: Option<StopReason> = None;

        loop {
            // On cancel, drop the byte stream to abort the HTTP connection.
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

                for data in sse_data_lines(&event_text) {
                    if data == "[DONE]" {
                        emit_pending_chat_tool_call(
                            &tx,
                            &mut current_tool_id,
                            &mut current_tool_name,
                            &mut current_tool_args,
                        )
                        .await;
                        let _ = tx
                            .send(StreamEvent::Done {
                                usage: usage.clone(),
                                stop_reason: stop_reason.clone().or(Some(StopReason::EndTurn)),
                            })
                            .await;
                        return;
                    }

                    let parsed: serde_json::Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let delta = match parsed
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("delta"))
                    {
                        Some(d) => d,
                        None => {
                            if let Some(u) = parsed.get("usage") {
                                usage.input_tokens =
                                    u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                usage.output_tokens = u
                                    .get("completion_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                            }
                            continue;
                        }
                    };

                    if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                        && !content.is_empty()
                    {
                        debug!("OpenAI text delta: {}", &content[..content.len().min(80)]);
                        let _ = tx.send(StreamEvent::TextDelta(content.to_string())).await;
                    }

                    if let Some(finish) = parsed
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("finish_reason"))
                        .and_then(|f| f.as_str())
                    {
                        debug!("OpenAI finish_reason: {finish}");
                        match finish {
                            "stop" => stop_reason = Some(StopReason::EndTurn),
                            "tool_calls" => stop_reason = Some(StopReason::ToolUse),
                            "length" => stop_reason = Some(StopReason::MaxTokens),
                            _ => {}
                        }
                    }

                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tool_calls {
                            if let Some(func) = tc.get("function") {
                                if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                                    if !current_tool_id.is_empty() && !current_tool_args.is_empty()
                                    {
                                        emit_pending_chat_tool_call(
                                            &tx,
                                            &mut current_tool_id,
                                            &mut current_tool_name,
                                            &mut current_tool_args,
                                        )
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
                                if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                                    current_tool_args.push_str(args);
                                }
                            }
                        }
                    }
                }
            }
        }

        emit_pending_chat_tool_call(
            &tx,
            &mut current_tool_id,
            &mut current_tool_name,
            &mut current_tool_args,
        )
        .await;
        let _ = tx
            .send(StreamEvent::Done {
                usage,
                stop_reason: Some(StopReason::EndTurn),
            })
            .await;
    });

    rx
}

async fn emit_pending_chat_tool_call(
    tx: &mpsc::Sender<StreamEvent>,
    current_tool_id: &mut String,
    current_tool_name: &mut String,
    current_tool_args: &mut String,
) {
    if current_tool_id.is_empty() {
        return;
    }

    let input: serde_json::Value = serde_json::from_str(current_tool_args).unwrap_or_default();
    let _ = tx
        .send(StreamEvent::ContentBlockComplete(ContentBlock::ToolUse {
            id: std::mem::take(current_tool_id),
            name: std::mem::take(current_tool_name),
            input,
        }))
        .await;
    current_tool_args.clear();
}

fn spawn_responses_stream(
    response: reqwest::Response,
    cancel: tokio_util::sync::CancellationToken,
) -> mpsc::Receiver<StreamEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut usage = Usage::default();
        let mut stop_reason: Option<StopReason> = None;

        loop {
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

                for data in sse_data_lines(&event_text) {
                    if data == "[DONE]" {
                        let _ = tx
                            .send(StreamEvent::Done {
                                usage: usage.clone(),
                                stop_reason: stop_reason.clone().or(Some(StopReason::EndTurn)),
                            })
                            .await;
                        return;
                    }

                    let parsed: serde_json::Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    match parsed.get("type").and_then(|t| t.as_str()) {
                        Some("response.output_text.delta") => {
                            if let Some(delta) = parsed.get("delta").and_then(Value::as_str) {
                                let _ = tx.send(StreamEvent::TextDelta(delta.to_string())).await;
                            }
                        }
                        Some("response.output_item.done") => {
                            if let Some(item) = parsed.get("item")
                                && let Some(block) = response_item_to_tool_use(item)
                            {
                                stop_reason = Some(StopReason::ToolUse);
                                let _ = tx.send(StreamEvent::ContentBlockComplete(block)).await;
                            }
                        }
                        Some("response.completed") => {
                            if let Some(u) = parsed.get("response").and_then(|r| r.get("usage")) {
                                usage = responses_usage(u);
                            }
                            if response_has_function_call(parsed.get("response")) {
                                stop_reason = Some(StopReason::ToolUse);
                            }
                            let _ = tx
                                .send(StreamEvent::Done {
                                    usage: usage.clone(),
                                    stop_reason: stop_reason.clone().or(Some(StopReason::EndTurn)),
                                })
                                .await;
                            return;
                        }
                        Some("response.failed") => {
                            let message = parsed
                                .get("response")
                                .and_then(|r| r.get("error"))
                                .and_then(|e| e.get("message"))
                                .and_then(Value::as_str)
                                .unwrap_or("Responses API request failed");
                            let _ = tx.send(StreamEvent::Error(message.to_string())).await;
                        }
                        _ => {}
                    }
                }
            }
        }

        let _ = tx
            .send(StreamEvent::Done {
                usage,
                stop_reason: stop_reason.or(Some(StopReason::EndTurn)),
            })
            .await;
    });

    rx
}

fn sse_data_lines(event_text: &str) -> Vec<String> {
    event_text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(str::to_string)
        .collect()
}

fn messages_to_responses_input(messages: &[Message]) -> Vec<Value> {
    let mut input = Vec::new();

    for msg in messages {
        match msg {
            Message::User(user) => {
                let mut content = Vec::new();
                for block in &user.content {
                    match block {
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content: output,
                            ..
                        } => {
                            input.push(serde_json::json!({
                                "type": "function_call_output",
                                "call_id": tool_use_id,
                                "output": output,
                            }));
                        }
                        _ => content.push(content_block_to_responses_part(block, true)),
                    }
                }
                if !content.is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": content,
                    }));
                }
            }
            Message::Assistant(assistant) => {
                let mut content = Vec::new();
                for block in &assistant.content {
                    match block {
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input: args,
                        } => {
                            input.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": serde_json::to_string(args).unwrap_or_default(),
                            }));
                        }
                        _ => content.push(content_block_to_responses_part(block, false)),
                    }
                }
                if !content.is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "assistant",
                        "content": content,
                    }));
                }
            }
            Message::System(_) => {}
        }
    }

    input
}

fn content_block_to_responses_part(block: &ContentBlock, user_originated: bool) -> Value {
    match block {
        ContentBlock::Text { text } => serde_json::json!({
            "type": if user_originated { "input_text" } else { "output_text" },
            "text": text,
        }),
        ContentBlock::Image { media_type, data } => serde_json::json!({
            "type": "input_image",
            "image_url": format!("data:{media_type};base64,{data}"),
        }),
        ContentBlock::Thinking { thinking, .. } => serde_json::json!({
            "type": if user_originated { "input_text" } else { "output_text" },
            "text": thinking,
        }),
        ContentBlock::ToolUse { name, input, .. } => serde_json::json!({
            "type": if user_originated { "input_text" } else { "output_text" },
            "text": format!("[Tool call: {name}({input})]"),
        }),
        ContentBlock::ToolResult { content, .. } => serde_json::json!({
            "type": if user_originated { "input_text" } else { "output_text" },
            "text": content,
        }),
        ContentBlock::Document { title, .. } => serde_json::json!({
            "type": if user_originated { "input_text" } else { "output_text" },
            "text": format!("[Document: {}]", title.as_deref().unwrap_or("untitled")),
        }),
    }
}

fn responses_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => serde_json::json!("auto"),
        ToolChoice::Any => serde_json::json!("required"),
        ToolChoice::None => serde_json::json!("none"),
        ToolChoice::Specific(name) => serde_json::json!({
            "type": "function",
            "name": name,
        }),
    }
}

fn response_item_to_tool_use(item: &Value) -> Option<ContentBlock> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let name = item.get("name").and_then(Value::as_str)?.to_string();
    let id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let input = serde_json::from_str(arguments).unwrap_or_default();
    Some(ContentBlock::ToolUse { id, name, input })
}

fn response_has_function_call(response: Option<&Value>) -> bool {
    response
        .and_then(|r| r.get("output"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        })
}

fn responses_usage(usage: &Value) -> Usage {
    Usage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::{AssistantMessage, UserMessage};
    use crate::tools::ToolSchema;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn user_message(content: Vec<ContentBlock>) -> Message {
        Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: "2026-04-27T00:00:00Z".to_string(),
            content,
            is_meta: false,
            is_compact_summary: false,
        })
    }

    fn assistant_message(content: Vec<ContentBlock>) -> Message {
        Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: "2026-04-27T00:00:00Z".to_string(),
            content,
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })
    }

    #[test]
    fn responses_body_maps_messages_tools_and_tool_results() {
        let provider = OpenAiProvider::new("https://example.test/v1", "test-key");
        let request = ProviderRequest {
            messages: vec![
                user_message(vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }]),
                assistant_message(vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path":"README.md"}),
                }]),
                user_message(vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "contents".to_string(),
                    is_error: false,
                    extra_content: Vec::new(),
                }]),
            ],
            system_prompt: "be useful".to_string(),
            tools: vec![ToolSchema {
                name: "read_file",
                description: "Read a file",
                input_schema: serde_json::json!({"type":"object"}),
            }],
            model: "gpt-5.4".to_string(),
            max_tokens: 1024,
            temperature: None,
            enable_caching: false,
            tool_choice: ToolChoice::Auto,
            metadata: None,
            cancel: CancellationToken::new(),
        };

        let body = provider.build_responses_body(&request);

        assert_eq!(body["instructions"], "be useful");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][1]["call_id"], "call_1");
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["input"][2]["call_id"], "call_1");
    }

    #[test]
    fn response_item_done_maps_function_call_to_tool_use() {
        let item = serde_json::json!({
            "type": "function_call",
            "call_id": "call_7",
            "name": "bash",
            "arguments": "{\"command\":\"pwd\"}"
        });

        let block = response_item_to_tool_use(&item).unwrap();

        match block {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_7");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "pwd");
            }
            _ => panic!("expected tool use"),
        }
    }

    #[test]
    fn responses_usage_maps_cached_tokens() {
        let usage = responses_usage(&serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 25,
            "input_tokens_details": {"cached_tokens": 80}
        }));

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 25);
        assert_eq!(usage.cache_read_input_tokens, 80);
    }
}
