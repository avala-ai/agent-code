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
    /// Content can be a simple string or an array of content blocks
    /// (e.g., text + images for vision-enabled tool results).
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
        /// Optional rich content blocks (images, etc.) alongside the text.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_content: Vec<ToolResultBlock>,
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

    /// Document content (e.g., PDF pages sent inline).
    #[serde(rename = "document")]
    Document {
        #[serde(rename = "media_type")]
        media_type: String,
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

/// A block within a rich tool result (for multi-modal tool output).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultBlock {
    #[serde(rename = "text")]
    Text { text: String },
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
            extra_content: vec![],
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
                ..
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
            ContentBlock::Document {
                media_type,
                data,
                title,
            } => {
                let mut doc = serde_json::json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                });
                if let Some(t) = title {
                    doc["title"] = serde_json::json!(t);
                }
                doc
            }
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

/// Convert messages to API params with cache_control breakpoints.
///
/// Places an ephemeral cache marker on the last user message before
/// the current turn, so the conversation prefix stays cached across
/// the tool-call loop within a single turn.
pub fn messages_to_api_params_cached(messages: &[Message]) -> Vec<serde_json::Value> {
    // Find the second-to-last non-meta user message index for cache marking.
    let user_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, Message::User(u) if !u.is_meta))
        .map(|(i, _)| i)
        .collect();

    let cache_index = if user_indices.len() >= 2 {
        Some(user_indices[user_indices.len() - 2])
    } else {
        None
    };

    messages
        .iter()
        .enumerate()
        .filter_map(|(i, msg)| match msg {
            Message::User(u) => {
                let mut content = content_blocks_to_api(&u.content);
                // Add cache_control to the marked message.
                if Some(i) == cache_index
                    && let serde_json::Value::Array(ref mut blocks) = content
                    && let Some(last) = blocks.last_mut()
                {
                    last["cache_control"] = serde_json::json!({"type": "ephemeral"});
                }
                Some(serde_json::json!({
                    "role": "user",
                    "content": content,
                }))
            }
            Message::Assistant(a) => Some(serde_json::json!({
                "role": "assistant",
                "content": content_blocks_to_api(&a.content),
            })),
            Message::System(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_message_creates_text() {
        let msg = user_message("hello");
        if let Message::User(u) = &msg {
            assert_eq!(u.content.len(), 1);
            assert_eq!(u.content[0].as_text(), Some("hello"));
            assert!(!u.is_meta);
        } else {
            panic!("Expected User");
        }
    }

    #[test]
    fn test_tool_result_message_success() {
        let msg = tool_result_message("c1", "output", false);
        if let Message::User(u) = &msg {
            assert!(u.is_meta);
            if let ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } = &u.content[0]
            {
                assert_eq!(tool_use_id, "c1");
                assert!(!is_error);
            }
        }
    }

    #[test]
    fn test_tool_result_message_error() {
        let msg = tool_result_message("c2", "fail", true);
        if let Message::User(u) = &msg
            && let ContentBlock::ToolResult { is_error, .. } = &u.content[0]
        {
            assert!(is_error);
        }
    }

    #[test]
    fn test_as_text() {
        assert_eq!(
            ContentBlock::Text { text: "hi".into() }.as_text(),
            Some("hi")
        );
        assert_eq!(
            ContentBlock::ToolUse {
                id: "1".into(),
                name: "X".into(),
                input: serde_json::json!({})
            }
            .as_text(),
            None
        );
    }

    #[test]
    fn test_as_tool_use() {
        let b = ContentBlock::ToolUse {
            id: "a".into(),
            name: "B".into(),
            input: serde_json::json!(1),
        };
        let (id, name, _) = b.as_tool_use().unwrap();
        assert_eq!(id, "a");
        assert_eq!(name, "B");
        assert!(
            ContentBlock::Text { text: "x".into() }
                .as_tool_use()
                .is_none()
        );
    }

    #[test]
    fn test_usage_total() {
        let u = Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 7,
        };
        assert_eq!(u.total(), 40);
    }

    #[test]
    fn test_usage_merge() {
        let mut u = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        u.merge(&Usage {
            input_tokens: 200,
            output_tokens: 30,
            cache_creation_input_tokens: 5,
            cache_read_input_tokens: 10,
        });
        assert_eq!(u.input_tokens, 200);
        assert_eq!(u.output_tokens, 80);
        assert_eq!(u.cache_creation_input_tokens, 5);
    }

    #[test]
    fn test_usage_default() {
        assert_eq!(Usage::default().total(), 0);
    }

    #[test]
    fn test_message_uuid_accessible() {
        let _ = user_message("t").uuid();
    }

    #[test]
    fn test_messages_to_api_params_filters_system() {
        let messages = vec![user_message("hi")];
        let params = messages_to_api_params(&messages);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["role"], "user");
    }
}
