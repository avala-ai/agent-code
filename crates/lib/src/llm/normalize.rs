//! Message normalization and validation utilities.
//!
//! Ensures messages conform to API requirements before sending:
//! - Tool use / tool result pairing
//! - Content block ordering
//! - Empty message handling

use super::message::*;

/// Ensure every tool_use block has a matching tool_result in the
/// subsequent user message. Orphaned tool_use blocks cause API errors.
pub fn ensure_tool_result_pairing(messages: &mut Vec<Message>) {
    let mut pending_tool_ids: Vec<String> = Vec::new();

    let mut i = 0;
    while i < messages.len() {
        match &messages[i] {
            Message::Assistant(a) => {
                // Collect tool_use IDs from this message.
                for block in &a.content {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        pending_tool_ids.push(id.clone());
                    }
                }
            }
            Message::User(u) => {
                // Remove tool_result IDs that are satisfied.
                for block in &u.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        pending_tool_ids.retain(|id| id != tool_use_id);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Any remaining pending IDs need synthetic error results.
    if !pending_tool_ids.is_empty() {
        for id in pending_tool_ids {
            messages.push(tool_result_message(
                &id,
                "(tool execution was interrupted)",
                true,
            ));
        }
    }
}

/// Remove empty text blocks from messages.
pub fn strip_empty_blocks(messages: &mut [Message]) {
    for msg in messages.iter_mut() {
        match msg {
            Message::User(u) => {
                u.content.retain(|b| match b {
                    ContentBlock::Text { text } => !text.is_empty(),
                    _ => true,
                });
            }
            Message::Assistant(a) => {
                a.content.retain(|b| match b {
                    ContentBlock::Text { text } => !text.is_empty(),
                    _ => true,
                });
            }
            _ => {}
        }
    }
}

/// Validate that the message sequence alternates correctly
/// (user/assistant/user/assistant...) as required by the API.
pub fn validate_alternation(messages: &[Message]) -> Result<(), String> {
    let mut expect_user = true;

    for (i, msg) in messages.iter().enumerate() {
        match msg {
            Message::System(_) => continue, // System messages don't count.
            Message::User(_) => {
                if !expect_user {
                    return Err(format!("Message {i}: expected assistant, got user"));
                }
                expect_user = false;
            }
            Message::Assistant(_) => {
                if expect_user {
                    return Err(format!("Message {i}: expected user, got assistant"));
                }
                expect_user = true;
            }
        }
    }

    Ok(())
}

/// Remove empty messages (messages with no content blocks after stripping).
pub fn remove_empty_messages(messages: &mut Vec<Message>) {
    messages.retain(|msg| match msg {
        Message::User(u) => !u.content.is_empty(),
        Message::Assistant(a) => !a.content.is_empty(),
        Message::System(_) => true,
    });
}

/// Cap oversized document blocks to prevent context blowout.
pub fn cap_document_blocks(messages: &mut [Message], max_bytes: usize) {
    for msg in messages.iter_mut() {
        let content = match msg {
            Message::User(u) => &mut u.content,
            Message::Assistant(a) => &mut a.content,
            _ => continue,
        };
        for block in content.iter_mut() {
            if let ContentBlock::Document { data, title, .. } = block
                && data.len() > max_bytes
            {
                let name = title.as_deref().unwrap_or("document");
                *block = ContentBlock::Text {
                    text: format!(
                        "(Document '{name}' too large for context: {} bytes, max {max_bytes})",
                        data.len()
                    ),
                };
            }
        }
    }
}

/// Merge consecutive user messages into a single message.
/// The API requires strict user/assistant alternation.
pub fn merge_consecutive_user_messages(messages: &mut Vec<Message>) {
    let mut i = 0;
    while i + 1 < messages.len() {
        let both_user = matches!(&messages[i], Message::User(_))
            && matches!(&messages[i + 1], Message::User(_));

        if both_user {
            // Merge content from i+1 into i.
            if let Message::User(next) = messages.remove(i + 1)
                && let Message::User(ref mut current) = messages[i]
            {
                current.content.extend(next.content);
            }
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_tool_result_pairing() {
        let mut messages = vec![
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
            // No tool_result for call_1!
        ];

        ensure_tool_result_pairing(&mut messages);

        // Should have added a synthetic error result.
        assert_eq!(messages.len(), 2);
        if let Message::User(u) = &messages[1] {
            assert!(matches!(
                &u.content[0],
                ContentBlock::ToolResult { is_error: true, .. }
            ));
        } else {
            panic!("Expected user message with tool result");
        }
    }

    #[test]
    fn test_merge_consecutive_users() {
        let mut messages = vec![
            user_message("hello"),
            user_message("world"),
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text { text: "hi".into() }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
        ];

        merge_consecutive_user_messages(&mut messages);
        assert_eq!(messages.len(), 2); // Two user messages merged into one.
    }

    #[test]
    fn test_strip_empty_blocks() {
        let mut messages = vec![Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![
                ContentBlock::Text {
                    text: "".into(), // empty — should be removed
                },
                ContentBlock::Text {
                    text: "keep me".into(),
                },
            ],
            is_meta: false,
            is_compact_summary: false,
        })];
        strip_empty_blocks(&mut messages);
        if let Message::User(u) = &messages[0] {
            assert_eq!(u.content.len(), 1);
            assert_eq!(u.content[0].as_text(), Some("keep me"));
        }
    }

    #[test]
    fn test_validate_alternation_valid() {
        let messages = vec![
            user_message("hello"),
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text { text: "hi".into() }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
        ];
        assert!(validate_alternation(&messages).is_ok());
    }

    #[test]
    fn test_validate_alternation_invalid() {
        let messages = vec![
            user_message("hello"),
            user_message("world"), // Two users in a row.
        ];
        assert!(validate_alternation(&messages).is_err());
    }

    #[test]
    fn test_remove_empty_messages() {
        let mut messages = vec![
            user_message("keep"),
            Message::User(UserMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![], // empty — should be removed
                is_meta: false,
                is_compact_summary: false,
            }),
            user_message("also keep"),
        ];
        remove_empty_messages(&mut messages);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_cap_document_blocks() {
        let mut messages = vec![Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Document {
                media_type: "application/pdf".into(),
                data: "x".repeat(1000),
                title: Some("big.pdf".into()),
            }],
            is_meta: false,
            is_compact_summary: false,
        })];
        // Cap at 500 bytes — should replace with text.
        cap_document_blocks(&mut messages, 500);
        if let Message::User(u) = &messages[0] {
            assert!(matches!(&u.content[0], ContentBlock::Text { .. }));
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.contains("big.pdf"));
                assert!(text.contains("too large"));
            }
        }
    }

    #[test]
    fn test_cap_document_blocks_within_limit() {
        let mut messages = vec![Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Document {
                media_type: "application/pdf".into(),
                data: "small".into(),
                title: Some("small.pdf".into()),
            }],
            is_meta: false,
            is_compact_summary: false,
        })];
        // Cap at 500 bytes — should keep as-is.
        cap_document_blocks(&mut messages, 500);
        if let Message::User(u) = &messages[0] {
            assert!(matches!(&u.content[0], ContentBlock::Document { .. }));
        }
    }

    #[test]
    fn test_tool_result_pairing_already_paired() {
        let mut messages = vec![
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
            Message::User(UserMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "ok".into(),
                    is_error: false,
                    extra_content: vec![],
                }],
                is_meta: true,
                is_compact_summary: false,
            }),
        ];

        ensure_tool_result_pairing(&mut messages);
        // No change expected — already paired.
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_tool_result_pairing_multiple_orphans() {
        let mut messages = vec![Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![
                ContentBlock::ToolUse {
                    id: "call_a".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "call_b".into(),
                    name: "FileRead".into(),
                    input: serde_json::json!({}),
                },
            ],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })];

        ensure_tool_result_pairing(&mut messages);
        // Should add two synthetic error results (one per orphan).
        assert_eq!(messages.len(), 3);
        for msg in &messages[1..] {
            if let Message::User(u) = msg {
                assert!(matches!(
                    &u.content[0],
                    ContentBlock::ToolResult { is_error: true, .. }
                ));
            } else {
                panic!("Expected user message with tool result");
            }
        }
    }

    #[test]
    fn test_merge_no_consecutive_users() {
        let assistant = Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Text { text: "hi".into() }],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        });
        let mut messages = vec![user_message("hello"), assistant, user_message("bye")];

        merge_consecutive_user_messages(&mut messages);
        assert_eq!(messages.len(), 3); // No change.
    }

    #[test]
    fn test_merge_three_consecutive_users() {
        let mut messages = vec![
            user_message("one"),
            user_message("two"),
            user_message("three"),
        ];

        merge_consecutive_user_messages(&mut messages);
        assert_eq!(messages.len(), 1); // All merged into one.
        if let Message::User(u) = &messages[0] {
            assert_eq!(u.content.len(), 3);
        } else {
            panic!("Expected user message");
        }
    }

    #[test]
    fn test_validate_alternation_with_system_messages() {
        let messages = vec![
            Message::System(SystemMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                subtype: SystemMessageType::Informational,
                content: "system note".into(),
                level: MessageLevel::Info,
            }),
            user_message("hello"),
            Message::System(SystemMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                subtype: SystemMessageType::Informational,
                content: "another note".into(),
                level: MessageLevel::Info,
            }),
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text { text: "hi".into() }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
        ];
        assert!(validate_alternation(&messages).is_ok());
    }

    #[test]
    fn test_validate_alternation_empty_list() {
        let messages: Vec<Message> = vec![];
        assert!(validate_alternation(&messages).is_ok());
    }

    #[test]
    fn test_strip_empty_blocks_on_assistant() {
        let mut messages = vec![Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![
                ContentBlock::Text { text: "".into() },
                ContentBlock::Text {
                    text: "real content".into(),
                },
                ContentBlock::Text { text: "".into() },
            ],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })];
        strip_empty_blocks(&mut messages);
        if let Message::Assistant(a) = &messages[0] {
            assert_eq!(a.content.len(), 1);
            assert_eq!(a.content[0].as_text(), Some("real content"));
        }
    }

    #[test]
    fn test_remove_empty_messages_preserves_system() {
        let mut messages = vec![
            Message::System(SystemMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                subtype: SystemMessageType::Informational,
                content: "".into(), // Empty content but system messages are always kept.
                level: MessageLevel::Info,
            }),
            Message::User(UserMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![], // Empty — should be removed.
                is_meta: false,
                is_compact_summary: false,
            }),
            user_message("keep me"),
        ];
        remove_empty_messages(&mut messages);
        assert_eq!(messages.len(), 2); // System + "keep me".
        assert!(matches!(&messages[0], Message::System(_)));
        assert!(matches!(&messages[1], Message::User(_)));
    }

    #[test]
    fn test_cap_document_blocks_no_title_uses_document() {
        let mut messages = vec![Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Document {
                media_type: "text/plain".into(),
                data: "x".repeat(200),
                title: None,
            }],
            is_meta: false,
            is_compact_summary: false,
        })];
        cap_document_blocks(&mut messages, 100);
        if let Message::User(u) = &messages[0] {
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.contains("document"), "should use fallback name 'document'");
                assert!(text.contains("too large"));
            } else {
                panic!("Expected text block after capping");
            }
        }
    }
}
