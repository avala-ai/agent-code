//! SSE (Server-Sent Events) stream parser.
//!
//! Parses the `text/event-stream` format used by LLM APIs for streaming
//! responses. Yields `StreamEvent` values as content blocks arrive.
//!
//! The stream processes these SSE event types:
//! - `message_start` — initializes the response with usage data
//! - `content_block_start` — begins a new content block (text, tool_use, thinking)
//! - `content_block_delta` — appends partial data to the current block
//! - `content_block_stop` — finalizes and yields the completed block
//! - `message_delta` — final usage and stop_reason
//! - `message_stop` — stream complete

use serde::Deserialize;

use crate::llm::message::{ContentBlock, StopReason, Usage};

/// Events yielded by the stream parser.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Partial text being streamed.
    TextDelta(String),

    /// A complete content block has been finalized.
    ContentBlockComplete(ContentBlock),

    /// A tool use block is being accumulated (for progress display).
    ToolUseStart { id: String, name: String },

    /// Partial JSON for a tool input being accumulated.
    ToolInputDelta { id: String, partial_json: String },

    /// The model has finished generating. Contains final usage.
    Done {
        usage: Usage,
        stop_reason: Option<StopReason>,
    },

    /// Time to first token, in milliseconds.
    Ttft(u64),

    /// An error occurred during streaming.
    Error(String),
}

/// Raw SSE data payload (deserialized from each `data:` line).
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RawSseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageStartPayload },

    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: RawContentBlock,
    },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: RawDelta },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },

    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaPayload,
        usage: Option<Usage>,
    },

    #[serde(rename = "message_stop")]
    MessageStop {},

    #[serde(rename = "ping")]
    Ping {},

    #[serde(rename = "error")]
    Error { error: ErrorPayload },
}

#[derive(Debug, Deserialize)]
pub struct MessageStartPayload {
    pub id: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct MessageDeltaPayload {
    pub stop_reason: Option<StopReason>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorPayload {
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RawContentBlock {
    #[serde(rename = "text")]
    Text { text: Option<String> },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Option<serde_json::Value>,
    },

    #[serde(rename = "thinking")]
    Thinking {
        thinking: Option<String>,
        signature: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
pub enum RawDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },

    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },

    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

/// Accumulates streaming data and produces `StreamEvent` values.
///
/// Maintains partial state for each content block index as deltas arrive,
/// then emits a `ContentBlockComplete` when the block is finalized.
pub struct StreamParser {
    /// Partial content blocks being accumulated, indexed by block position.
    blocks: Vec<PartialBlock>,
    /// Usage from message_start, updated by message_delta.
    usage: Usage,
    /// Model that generated this response.
    pub model: Option<String>,
    /// API request ID.
    pub request_id: Option<String>,
}

/// A content block being accumulated from deltas.
enum PartialBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input_json: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
}

impl StreamParser {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            usage: Usage::default(),
            model: None,
            request_id: None,
        }
    }

    /// Process a raw SSE event and return any resulting stream events.
    pub fn process(&mut self, raw: RawSseEvent) -> Vec<StreamEvent> {
        match raw {
            RawSseEvent::MessageStart { message } => {
                if let Some(usage) = message.usage {
                    self.usage = usage;
                }
                self.model = message.model;
                self.request_id = message.id;
                vec![]
            }

            RawSseEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                // Ensure the blocks vec is large enough.
                while self.blocks.len() <= index {
                    self.blocks.push(PartialBlock::Text(String::new()));
                }

                match content_block {
                    RawContentBlock::Text { text } => {
                        self.blocks[index] = PartialBlock::Text(text.unwrap_or_default());
                        vec![]
                    }
                    RawContentBlock::ToolUse { id, name, input: _ } => {
                        let event = StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        };
                        self.blocks[index] = PartialBlock::ToolUse {
                            id,
                            name,
                            input_json: String::new(),
                        };
                        vec![event]
                    }
                    RawContentBlock::Thinking {
                        thinking,
                        signature,
                    } => {
                        self.blocks[index] = PartialBlock::Thinking {
                            thinking: thinking.unwrap_or_default(),
                            signature: signature.unwrap_or_default(),
                        };
                        vec![]
                    }
                }
            }

            RawSseEvent::ContentBlockDelta { index, delta } => {
                if index >= self.blocks.len() {
                    return vec![];
                }

                match delta {
                    RawDelta::TextDelta { text } => {
                        if let PartialBlock::Text(ref mut buf) = self.blocks[index] {
                            buf.push_str(&text);
                        }
                        vec![StreamEvent::TextDelta(text)]
                    }
                    RawDelta::InputJsonDelta { partial_json } => {
                        let mut events = vec![];
                        if let PartialBlock::ToolUse {
                            ref id,
                            ref mut input_json,
                            ..
                        } = self.blocks[index]
                        {
                            input_json.push_str(&partial_json);
                            events.push(StreamEvent::ToolInputDelta {
                                id: id.clone(),
                                partial_json,
                            });
                        }
                        events
                    }
                    RawDelta::ThinkingDelta { thinking } => {
                        if let PartialBlock::Thinking {
                            thinking: ref mut buf,
                            ..
                        } = self.blocks[index]
                        {
                            buf.push_str(&thinking);
                        }
                        vec![]
                    }
                    RawDelta::SignatureDelta { signature } => {
                        if let PartialBlock::Thinking {
                            signature: ref mut buf,
                            ..
                        } = self.blocks[index]
                        {
                            buf.push_str(&signature);
                        }
                        vec![]
                    }
                }
            }

            RawSseEvent::ContentBlockStop { index } => {
                if index >= self.blocks.len() {
                    return vec![];
                }

                let block =
                    std::mem::replace(&mut self.blocks[index], PartialBlock::Text(String::new()));

                let content_block = match block {
                    PartialBlock::Text(text) => ContentBlock::Text { text },
                    PartialBlock::ToolUse {
                        id,
                        name,
                        input_json,
                    } => {
                        let input = serde_json::from_str(&input_json)
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        ContentBlock::ToolUse { id, name, input }
                    }
                    PartialBlock::Thinking {
                        thinking,
                        signature,
                    } => ContentBlock::Thinking {
                        thinking,
                        signature: if signature.is_empty() {
                            None
                        } else {
                            Some(signature)
                        },
                    },
                };

                vec![StreamEvent::ContentBlockComplete(content_block)]
            }

            RawSseEvent::MessageDelta { delta, usage } => {
                if let Some(u) = usage {
                    self.usage.merge(&u);
                }
                vec![StreamEvent::Done {
                    usage: self.usage.clone(),
                    stop_reason: delta.stop_reason,
                }]
            }

            RawSseEvent::MessageStop {} => vec![],

            RawSseEvent::Ping {} => vec![],

            RawSseEvent::Error { error } => {
                let msg = error
                    .message
                    .unwrap_or_else(|| "Unknown stream error".to_string());
                vec![StreamEvent::Error(msg)]
            }
        }
    }
}
