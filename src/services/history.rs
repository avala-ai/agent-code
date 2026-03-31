//! Conversation history utilities.
//!
//! Functions for manipulating, searching, and transforming the
//! message history. Used by compaction, export, and the query engine.

use crate::llm::message::{ContentBlock, Message};

/// Count messages by type.
pub fn message_counts(messages: &[Message]) -> (usize, usize, usize) {
    let mut user = 0;
    let mut assistant = 0;
    let mut system = 0;

    for msg in messages {
        match msg {
            Message::User(_) => user += 1,
            Message::Assistant(_) => assistant += 1,
            Message::System(_) => system += 1,
        }
    }

    (user, assistant, system)
}

/// Extract all text content from messages (for search/export).
pub fn extract_text(messages: &[Message]) -> String {
    let mut text = String::new();
    for msg in messages {
        let blocks = match msg {
            Message::User(u) => &u.content,
            Message::Assistant(a) => &a.content,
            Message::System(s) => {
                text.push_str(&s.content);
                text.push('\n');
                continue;
            }
        };
        for block in blocks {
            if let ContentBlock::Text { text: t } = block {
                text.push_str(t);
                text.push('\n');
            }
        }
    }
    text
}

/// Find the index of the last user message (non-meta).
pub fn last_user_message_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|m| {
        matches!(m, Message::User(u) if !u.is_meta)
    })
}

/// Find the index of the last assistant message.
pub fn last_assistant_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|m| matches!(m, Message::Assistant(_)))
}

/// Count tool use blocks in the conversation.
pub fn tool_use_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(&a.content),
            _ => None,
        })
        .flat_map(|blocks| blocks.iter())
        .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .count()
}

/// Get a list of unique tools used in the conversation.
pub fn tools_used(messages: &[Message]) -> Vec<String> {
    let mut tools: Vec<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(&a.content),
            _ => None,
        })
        .flat_map(|blocks| blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();

    tools.sort();
    tools.dedup();
    tools
}

/// Truncate messages to fit within a token budget.
///
/// Removes oldest messages (preserving the first system/summary message)
/// until the estimated token count is within budget.
pub fn truncate_to_budget(messages: &mut Vec<Message>, max_tokens: u64) {
    while crate::services::tokens::estimate_context_tokens(messages) > max_tokens
        && messages.len() > 2
    {
        // Remove the second message (preserve index 0 which may be a summary).
        messages.remove(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::{user_message, AssistantMessage, ContentBlock};
    use uuid::Uuid;

    fn assistant_msg(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })
    }

    #[test]
    fn test_message_counts() {
        let msgs = vec![
            user_message("hello"),
            assistant_msg("hi"),
            user_message("bye"),
        ];
        assert_eq!(message_counts(&msgs), (2, 1, 0));
    }

    #[test]
    fn test_tool_use_count() {
        let msgs = vec![Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![
                ContentBlock::ToolUse {
                    id: "1".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::Text {
                    text: "done".into(),
                },
            ],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })];
        assert_eq!(tool_use_count(&msgs), 1);
    }
}
