//! Message normalization and validation utilities.
//!
//! Ensures messages conform to API requirements before sending:
//! - Tool use / tool result pairing
//! - Content block ordering
//! - Empty message handling

use super::message::*;

/// Repair tool_use / tool_result pairing so strict providers accept
/// the history.
///
/// Malformed histories (crash mid-turn, imported sessions, permissive
/// upstream models) show up three ways, all rejected with a 400 by
/// strict OpenAI-compatible and local backends:
///
/// - a `tool_use` with no answering `tool_result` → a synthetic error
///   result is appended (long-standing behavior);
/// - a `tool_result` with no preceding `tool_use` for its id
///   (out-of-order or truly orphaned) → the block is dropped;
/// - several `tool_result`s for the same id → the first is kept, the
///   rest are dropped (the first is the one the model actually
///   continued from).
///
/// Blocks emptied by the drops are cleaned up by the
/// `remove_empty_messages` step that follows in the pipeline.
pub fn ensure_tool_result_pairing(messages: &mut Vec<Message>) {
    use std::collections::HashSet;

    // IDs of tool_use blocks seen so far in the walk (a result is only
    // valid when its call precedes it), and IDs already answered.
    let mut seen_use_ids: HashSet<String> = HashSet::new();
    let mut answered_ids: HashSet<String> = HashSet::new();
    // Preserves emission order for the synthetic results appended below.
    let mut pending_tool_ids: Vec<String> = Vec::new();

    for msg in messages.iter_mut() {
        match msg {
            Message::Assistant(a) => {
                for block in &a.content {
                    if let ContentBlock::ToolUse { id, .. } = block
                        && seen_use_ids.insert(id.clone())
                    {
                        pending_tool_ids.push(id.clone());
                    }
                }
            }
            Message::User(u) => {
                u.content.retain(|block| {
                    let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                        return true;
                    };
                    if !seen_use_ids.contains(tool_use_id) {
                        // Out-of-order or orphaned result — no call to
                        // pair with; keeping it guarantees a 400.
                        return false;
                    }
                    // Keep only the first result per id.
                    answered_ids.insert(tool_use_id.clone())
                });
            }
            _ => {}
        }
    }

    // Any calls still unanswered get synthetic error results.
    pending_tool_ids.retain(|id| !answered_ids.contains(id));
    for id in pending_tool_ids {
        messages.push(tool_result_message(
            &id,
            "(tool execution was interrupted)",
            true,
        ));
    }
}

/// Replace `tool_use` inputs that are not JSON objects.
///
/// Providers require tool-call arguments to be an object; some models
/// emit `null` or the arguments as a JSON-encoded *string*. A string
/// that parses to an object is adopted (the arguments were merely
/// double-encoded); anything else non-object becomes `{}` so the
/// request is not rejected outright.
pub fn sanitize_tool_use_input(messages: &mut [Message]) {
    for msg in messages.iter_mut() {
        let Message::Assistant(a) = msg else { continue };
        for block in a.content.iter_mut() {
            let ContentBlock::ToolUse { input, .. } = block else {
                continue;
            };
            if input.is_object() {
                continue;
            }
            if let serde_json::Value::String(s) = &input
                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
                && parsed.is_object()
            {
                *input = parsed;
                continue;
            }
            *input = serde_json::json!({});
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

    fn assistant_with_use(id: &str, input: serde_json::Value) -> Message {
        Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "Bash".into(),
                input,
            }],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })
    }

    fn result_content(msg: &Message) -> Vec<(&str, &str)> {
        match msg {
            Message::User(u) => u
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => Some((tool_use_id.as_str(), content.as_str())),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }

    #[test]
    fn orphan_result_with_no_preceding_use_is_dropped() {
        let mut messages = vec![
            user_message("hi"),
            // Result arrives before any tool_use with this id exists.
            tool_result_message("never_called", "stale", false),
            assistant_with_use("call_1", serde_json::json!({})),
            tool_result_message("call_1", "ok", false),
        ];
        ensure_tool_result_pairing(&mut messages);
        let all_results: Vec<_> = messages.iter().flat_map(result_content).collect();
        assert_eq!(
            all_results,
            vec![("call_1", "ok")],
            "orphan result must be dropped, valid pair preserved"
        );
    }

    #[test]
    fn out_of_order_result_before_its_use_is_dropped_then_synthesized() {
        let mut messages = vec![
            // Result precedes its own tool_use — invalid ordering.
            tool_result_message("call_1", "too early", false),
            assistant_with_use("call_1", serde_json::json!({})),
        ];
        ensure_tool_result_pairing(&mut messages);
        let all_results: Vec<_> = messages.iter().flat_map(result_content).collect();
        // The early result is dropped; the now-unanswered call gets a
        // synthetic error result appended.
        assert_eq!(all_results.len(), 1);
        assert_eq!(all_results[0].0, "call_1");
        assert_ne!(all_results[0].1, "too early");
    }

    #[test]
    fn duplicate_results_keep_only_the_first() {
        let mut messages = vec![
            assistant_with_use("call_1", serde_json::json!({})),
            tool_result_message("call_1", "first", false),
            tool_result_message("call_1", "second", false),
            tool_result_message("call_1", "third", false),
        ];
        ensure_tool_result_pairing(&mut messages);
        let all_results: Vec<_> = messages.iter().flat_map(result_content).collect();
        assert_eq!(
            all_results,
            vec![("call_1", "first")],
            "only the first result per id survives"
        );
    }

    #[test]
    fn duplicate_result_inside_one_message_is_deduplicated() {
        let mut messages = vec![
            assistant_with_use("call_1", serde_json::json!({})),
            Message::User(UserMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "first".into(),
                        is_error: false,
                        extra_content: vec![],
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "second".into(),
                        is_error: false,
                        extra_content: vec![],
                    },
                ],
                is_meta: true,
                is_compact_summary: false,
            }),
        ];
        ensure_tool_result_pairing(&mut messages);
        let all_results: Vec<_> = messages.iter().flat_map(result_content).collect();
        assert_eq!(all_results, vec![("call_1", "first")]);
    }

    #[test]
    fn sanitize_null_input_becomes_empty_object() {
        let mut messages = vec![assistant_with_use("call_1", serde_json::Value::Null)];
        sanitize_tool_use_input(&mut messages);
        let Message::Assistant(a) = &messages[0] else {
            panic!("expected assistant");
        };
        let ContentBlock::ToolUse { input, .. } = &a.content[0] else {
            panic!("expected tool_use");
        };
        assert_eq!(input, &serde_json::json!({}));
    }

    #[test]
    fn sanitize_stringified_object_is_recovered() {
        let mut messages = vec![assistant_with_use(
            "call_1",
            serde_json::Value::String(r#"{"command":"ls"}"#.into()),
        )];
        sanitize_tool_use_input(&mut messages);
        let Message::Assistant(a) = &messages[0] else {
            panic!("expected assistant");
        };
        let ContentBlock::ToolUse { input, .. } = &a.content[0] else {
            panic!("expected tool_use");
        };
        assert_eq!(input, &serde_json::json!({"command": "ls"}));
    }

    #[test]
    fn sanitize_non_object_values_become_empty_object() {
        for bad in [
            serde_json::json!("not json at all"),
            serde_json::json!([1, 2, 3]),
            serde_json::json!(42),
            serde_json::json!("[1,2]"), // parses, but not to an object
        ] {
            let mut messages = vec![assistant_with_use("call_1", bad)];
            sanitize_tool_use_input(&mut messages);
            let Message::Assistant(a) = &messages[0] else {
                panic!("expected assistant");
            };
            let ContentBlock::ToolUse { input, .. } = &a.content[0] else {
                panic!("expected tool_use");
            };
            assert_eq!(input, &serde_json::json!({}));
        }
    }

    #[test]
    fn well_formed_history_passes_through_unchanged() {
        let original = vec![
            user_message("run ls"),
            assistant_with_use("call_1", serde_json::json!({"command": "ls"})),
            tool_result_message("call_1", "file.txt", false),
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
        ];
        let mut messages = original.clone();
        sanitize_tool_use_input(&mut messages);
        ensure_tool_result_pairing(&mut messages);
        assert_eq!(messages.len(), original.len());
        assert_eq!(
            serde_json::to_string(&messages).unwrap(),
            serde_json::to_string(&original).unwrap(),
            "valid history must not be altered"
        );
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
                assert!(
                    text.contains("document"),
                    "should use fallback name 'document'"
                );
                assert!(text.contains("too large"));
            } else {
                panic!("Expected text block after capping");
            }
        }
    }
}
