//! Message types for the conversation protocol.
//!
//! These types mirror the wire format used by LLM APIs. The conversation
//! is a sequence of messages with roles (system, user, assistant) and
//! content blocks (text, tool_use, tool_result, thinking).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    /// User input message.
    #[serde(rename = "user")]
    User(UserMessage),
    /// Assistant (model) response.
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    /// System notification (not sent to API).
    #[serde(rename = "system")]
    System(SystemMessage),
}

impl Message {
    pub fn uuid(&self) -> &Uuid {
        match self {
            Message::User(m) => &m.uuid,
            Message::Assistant(m) => &m.uuid,
            Message::System(m) => &m.uuid,
        }
    }
}

/// User-originated message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub uuid: Uuid,
    pub timestamp: String,
    pub content: Vec<ContentBlock>,
    /// If true, this message is metadata (tool results, context injection)
    /// rather than direct user input.
    #[serde(default)]
    pub is_meta: bool,
    /// If true, this is a compact summary replacing earlier messages.
    #[serde(default)]
    pub is_compact_summary: bool,
}

/// Assistant response message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub uuid: Uuid,
    pub timestamp: String,
    pub content: Vec<ContentBlock>,
    /// Model that generated this response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Token usage for this response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Why the model stopped generating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// API request ID for debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// System notification (informational, error, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub uuid: Uuid,
    pub timestamp: String,
    pub subtype: SystemMessageType,
    pub content: String,
    #[serde(default)]
    pub level: MessageLevel,
}

/// System message subtypes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessageType {
    Informational,
    ApiError,
    CompactBoundary,
    TurnDuration,
    MemorySaved,
    ToolProgress,
}

/// Message severity level.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageLevel {
    #[default]
    Info,
    Warning,
    Error,
}

/// A block of content within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text content.
    #[serde(rename = "text")]
    Text { text: String },

    /// A request from the model to execute a tool.
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    /// The result of a tool execution, sent back to the model.
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },

    /// Extended thinking content (model reasoning).
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Image content.
    #[serde(rename = "image")]
    Image {
        #[serde(rename = "media_type")]
        media_type: String,
        data: String,
    },
}

impl ContentBlock {
    /// Extract text content, if this is a text block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Extract tool use info, if this is a tool_use block.
    pub fn as_tool_use(&self) -> Option<(&str, &str, &serde_json::Value)> {
        match self {
            ContentBlock::ToolUse { id, name, input } => Some((id, name, input)),
            _ => None,
        }
    }
}

/// Token usage information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl Usage {
    /// Total tokens consumed.
    pub fn total(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }

    /// Merge usage from a subsequent response.
    pub fn merge(&mut self, other: &Usage) {
        self.input_tokens = other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens = other.cache_creation_input_tokens;
        self.cache_read_input_tokens = other.cache_read_input_tokens;
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
}

/// Helper to create a user message with text content.
pub fn user_message(text: impl Into<String>) -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        content: vec![ContentBlock::Text { text: text.into() }],
        is_meta: false,
        is_compact_summary: false,
    })
}

/// Helper to create an image content block from a file path.
///
/// Reads the file, base64-encodes it, and infers the media type
/// from the file extension.
pub fn image_block_from_file(path: &std::path::Path) -> Result<ContentBlock, String> {
    let data = std::fs::read(path).map_err(|e| format!("Failed to read image: {e}"))?;

    let media_type = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };

    use std::io::Write;
    let mut encoded = String::new();
    {
        let mut encoder = base64_encode_writer(&mut encoded);
        encoder
            .write_all(&data)
            .map_err(|e| format!("base64 error: {e}"))?;
    }

    Ok(ContentBlock::Image {
        media_type: media_type.to_string(),
        data: encoded,
    })
}

/// Simple base64 encoder (no external dependency).
fn base64_encode_writer(output: &mut String) -> Base64Writer<'_> {
    Base64Writer {
        output,
        buffer: Vec::new(),
    }
}

struct Base64Writer<'a> {
    output: &'a mut String,
    buffer: Vec<u8>,
}

impl<'a> std::io::Write for Base64Writer<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i + 2 < self.buffer.len() {
            let b0 = self.buffer[i] as usize;
            let b1 = self.buffer[i + 1] as usize;
            let b2 = self.buffer[i + 2] as usize;
            self.output.push(CHARS[b0 >> 2] as char);
            self.output.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
            self.output
                .push(CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
            self.output.push(CHARS[b2 & 0x3f] as char);
            i += 3;
        }
        let remaining = self.buffer.len() - i;
        if remaining == 1 {
            let b0 = self.buffer[i] as usize;
            self.output.push(CHARS[b0 >> 2] as char);
            self.output.push(CHARS[(b0 & 3) << 4] as char);
            self.output.push('=');
            self.output.push('=');
        } else if remaining == 2 {
            let b0 = self.buffer[i] as usize;
            let b1 = self.buffer[i + 1] as usize;
            self.output.push(CHARS[b0 >> 2] as char);
            self.output.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
            self.output.push(CHARS[(b1 & 0xf) << 2] as char);
            self.output.push('=');
        }
        Ok(())
    }
}

/// Helper to create a user message with an image.
pub fn image_message(path: &std::path::Path, caption: &str) -> Result<Message, String> {
    let image = image_block_from_file(path)?;
    Ok(Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        content: vec![
            image,
            ContentBlock::Text {
                text: caption.to_string(),
            },
        ],
        is_meta: false,
        is_compact_summary: false,
    }))
}

/// Helper to create a tool result message.
pub fn tool_result_message(tool_use_id: &str, content: &str, is_error: bool) -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: content.to_string(),
            is_error,
        }],
        is_meta: true,
        is_compact_summary: false,
    })
}

/// Convert messages to the API wire format (for sending to the LLM).
pub fn messages_to_api_params(messages: &[Message]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .filter_map(|msg| match msg {
            Message::User(u) => Some(serde_json::json!({
                "role": "user",
                "content": content_blocks_to_api(&u.content),
            })),
            Message::Assistant(a) => Some(serde_json::json!({
                "role": "assistant",
                "content": content_blocks_to_api(&a.content),
            })),
            // System messages are not sent to the API.
            Message::System(_) => None,
        })
        .collect()
}

fn content_blocks_to_api(blocks: &[ContentBlock]) -> serde_json::Value {
    let api_blocks: Vec<serde_json::Value> = blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => serde_json::json!({
                "type": "text",
                "text": text,
            }),
            ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
            ContentBlock::Thinking {
                thinking,
                signature,
            } => serde_json::json!({
                "type": "thinking",
                "thinking": thinking,
                "signature": signature,
            }),
            ContentBlock::Image { media_type, data } => serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": data,
                }
            }),
        })
        .collect();

    // If there's only one text block, use the simple string format.
    if api_blocks.len() == 1
        && let Some(text) = blocks[0].as_text()
    {
        return serde_json::Value::String(text.to_string());
    }

    serde_json::Value::Array(api_blocks)
}
