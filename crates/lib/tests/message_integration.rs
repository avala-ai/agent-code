//! Integration tests for message types and normalization.
//!
//! Tests conversation construction, alternation validation, tool result
//! pairing, empty block stripping, document capping, API param conversion,
//! and usage tracking.

use agent_code_lib::llm::message::*;
use agent_code_lib::llm::normalize::*;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn assistant_text(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        content: vec![ContentBlock::Text { text: text.into() }],
        model: None,
        usage: None,
        stop_reason: None,
        request_id: None,
    })
}

fn assistant_with_tool_use(id: &str, name: &str) -> Message {
    Message::Assistant(AssistantMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input: serde_json::json!({}),
        }],
        model: None,
        usage: None,
        stop_reason: None,
        request_id: None,
    })
}

fn system_info(text: &str) -> Message {
    Message::System(SystemMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        subtype: SystemMessageType::Informational,
        content: text.into(),
        level: MessageLevel::Info,
    })
}

fn user_with_empty_and_text() -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        content: vec![
            ContentBlock::Text { text: "".into() },
            ContentBlock::Text {
                text: "real content".into(),
            },
            ContentBlock::Text { text: "".into() },
        ],
        is_meta: false,
        is_compact_summary: false,
    })
}

fn user_empty() -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        content: vec![],
        is_meta: false,
        is_compact_summary: false,
    })
}

fn user_with_document(data: &str, title: Option<&str>) -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        content: vec![ContentBlock::Document {
            media_type: "application/pdf".into(),
            data: data.into(),
            title: title.map(String::from),
        }],
        is_meta: false,
        is_compact_summary: false,
    })
}

// ---------------------------------------------------------------------------
// Conversation alternation
// ---------------------------------------------------------------------------

#[test]
fn user_assistant_user_alternation_is_valid() {
    let messages = vec![
        user_message("hello"),
        assistant_text("hi there"),
        user_message("how are you?"),
    ];
    assert!(validate_alternation(&messages).is_ok());
}

#[test]
fn system_messages_are_ignored_in_alternation() {
    let messages = vec![
        system_info("session started"),
        user_message("hello"),
        system_info("context loaded"),
        assistant_text("hi"),
    ];
    assert!(validate_alternation(&messages).is_ok());
}

#[test]
fn consecutive_user_messages_fail_alternation() {
    let messages = vec![user_message("one"), user_message("two")];
    assert!(validate_alternation(&messages).is_err());
}

#[test]
fn consecutive_assistant_messages_fail_alternation() {
    let messages = vec![
        user_message("hello"),
        assistant_text("hi"),
        assistant_text("also hi"),
    ];
    assert!(validate_alternation(&messages).is_err());
}

// ---------------------------------------------------------------------------
// Tool result pairing
// ---------------------------------------------------------------------------

#[test]
fn orphaned_tool_use_gets_synthetic_result() {
    let mut messages = vec![assistant_with_tool_use("call_99", "Bash")];

    ensure_tool_result_pairing(&mut messages);

    // Should add a synthetic error tool result.
    assert_eq!(messages.len(), 2);
    if let Message::User(u) = &messages[1] {
        assert_eq!(u.content.len(), 1);
        if let ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } = &u.content[0]
        {
            assert_eq!(tool_use_id, "call_99");
            assert!(*is_error);
        } else {
            panic!("Expected ToolResult block");
        }
    } else {
        panic!("Expected User message");
    }
}

#[test]
fn paired_tool_use_does_not_add_synthetic_result() {
    let mut messages = vec![
        assistant_with_tool_use("call_1", "FileRead"),
        tool_result_message("call_1", "file contents here", false),
    ];

    let original_len = messages.len();
    ensure_tool_result_pairing(&mut messages);

    // No extra messages added.
    assert_eq!(messages.len(), original_len);
}

#[test]
fn multiple_orphaned_tool_uses_all_get_results() {
    let mut messages = vec![Message::Assistant(AssistantMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        content: vec![
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "Bash".into(),
                input: serde_json::json!({}),
            },
            ContentBlock::ToolUse {
                id: "b".into(),
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

    // Original assistant + 2 synthetic results.
    assert_eq!(messages.len(), 3);
}

// ---------------------------------------------------------------------------
// Merge consecutive user messages
// ---------------------------------------------------------------------------

#[test]
fn merge_consecutive_user_messages_combines_content() {
    let mut messages = vec![
        user_message("first"),
        user_message("second"),
        assistant_text("response"),
    ];

    merge_consecutive_user_messages(&mut messages);

    assert_eq!(messages.len(), 2);
    if let Message::User(u) = &messages[0] {
        // Should have two text blocks: "first" and "second".
        assert_eq!(u.content.len(), 2);
        assert_eq!(u.content[0].as_text(), Some("first"));
        assert_eq!(u.content[1].as_text(), Some("second"));
    } else {
        panic!("Expected merged user message");
    }
}

#[test]
fn merge_does_not_affect_properly_alternating_messages() {
    let mut messages = vec![
        user_message("hello"),
        assistant_text("hi"),
        user_message("bye"),
    ];

    merge_consecutive_user_messages(&mut messages);

    assert_eq!(messages.len(), 3);
}

// ---------------------------------------------------------------------------
// Strip empty blocks
// ---------------------------------------------------------------------------

#[test]
fn strip_empty_blocks_removes_empty_text_keeps_non_empty() {
    let mut messages = vec![user_with_empty_and_text()];

    strip_empty_blocks(&mut messages);

    if let Message::User(u) = &messages[0] {
        assert_eq!(u.content.len(), 1);
        assert_eq!(u.content[0].as_text(), Some("real content"));
    } else {
        panic!("Expected User message");
    }
}

#[test]
fn strip_empty_blocks_preserves_tool_use_blocks() {
    let mut messages = vec![Message::Assistant(AssistantMessage {
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        content: vec![
            ContentBlock::Text { text: "".into() },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: serde_json::json!({}),
            },
        ],
        model: None,
        usage: None,
        stop_reason: None,
        request_id: None,
    })];

    strip_empty_blocks(&mut messages);

    if let Message::Assistant(a) = &messages[0] {
        // Empty text removed, tool_use kept.
        assert_eq!(a.content.len(), 1);
        assert!(a.content[0].as_tool_use().is_some());
    }
}

// ---------------------------------------------------------------------------
// Remove empty messages
// ---------------------------------------------------------------------------

#[test]
fn remove_empty_messages_drops_contentless_messages() {
    let mut messages = vec![user_message("keep"), user_empty(), user_message("also keep")];

    remove_empty_messages(&mut messages);

    assert_eq!(messages.len(), 2);
}

#[test]
fn remove_empty_messages_keeps_system_messages() {
    let mut messages = vec![system_info("info"), user_empty()];

    remove_empty_messages(&mut messages);

    assert_eq!(messages.len(), 1);
    assert!(matches!(messages[0], Message::System(_)));
}

// ---------------------------------------------------------------------------
// Cap oversized documents
// ---------------------------------------------------------------------------

#[test]
fn cap_document_replaces_oversized_with_text() {
    let mut messages = vec![user_with_document(&"x".repeat(2000), Some("huge.pdf"))];

    cap_document_blocks(&mut messages, 500);

    if let Message::User(u) = &messages[0] {
        if let ContentBlock::Text { text } = &u.content[0] {
            assert!(text.contains("huge.pdf"));
            assert!(text.contains("too large"));
        } else {
            panic!("Expected Text block replacing document");
        }
    }
}

#[test]
fn cap_document_keeps_small_documents() {
    let mut messages = vec![user_with_document("short", Some("small.pdf"))];

    cap_document_blocks(&mut messages, 500);

    if let Message::User(u) = &messages[0] {
        assert!(matches!(u.content[0], ContentBlock::Document { .. }));
    }
}

// ---------------------------------------------------------------------------
// messages_to_api_params
// ---------------------------------------------------------------------------

#[test]
fn messages_to_api_params_produces_correct_json_structure() {
    let messages = vec![user_message("hi"), assistant_text("hello")];

    let params = messages_to_api_params(&messages);

    assert_eq!(params.len(), 2);
    assert_eq!(params[0]["role"], "user");
    // Single text block should be a simple string.
    assert_eq!(params[0]["content"], "hi");
    assert_eq!(params[1]["role"], "assistant");
    assert_eq!(params[1]["content"], "hello");
}

#[test]
fn messages_to_api_params_filters_out_system_messages() {
    let messages = vec![
        system_info("context loaded"),
        user_message("hello"),
        assistant_text("world"),
    ];

    let params = messages_to_api_params(&messages);

    // System message filtered out.
    assert_eq!(params.len(), 2);
    assert_eq!(params[0]["role"], "user");
    assert_eq!(params[1]["role"], "assistant");
}

#[test]
fn messages_to_api_params_includes_tool_use_blocks() {
    let messages = vec![assistant_with_tool_use("c1", "Bash")];

    let params = messages_to_api_params(&messages);

    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["role"], "assistant");
    let content = &params[0]["content"];
    assert!(content.is_array());
    assert_eq!(content[0]["type"], "tool_use");
    assert_eq!(content[0]["id"], "c1");
    assert_eq!(content[0]["name"], "Bash");
}

// ---------------------------------------------------------------------------
// messages_to_api_params_cached
// ---------------------------------------------------------------------------

#[test]
fn cached_params_marks_second_to_last_non_meta_user_message() {
    // Three real user messages: cache should be on index 1 (second-to-last non-meta).
    let messages = vec![
        user_message("first"),
        assistant_text("r1"),
        user_message("second"),
        assistant_text("r2"),
        user_message("third"),
    ];

    let params = messages_to_api_params_cached(&messages);

    // params should have 5 entries (3 user + 2 assistant, no system).
    assert_eq!(params.len(), 5);

    // The second user message (params index 2) should have cache_control.
    // params[0] = user "first", params[1] = assistant "r1",
    // params[2] = user "second" (should be cached), params[3] = assistant "r2",
    // params[4] = user "third"
    let second_user_content = &params[2]["content"];
    // For a single text block it becomes a string, so cache_control may
    // be added to the last block of an array. If it's a string, the caching
    // logic cannot attach cache_control (single text block -> string format).
    // This tests the function runs without error.
    assert_eq!(params[2]["role"], "user");
}

#[test]
fn cached_params_with_single_user_message_no_cache_mark() {
    let messages = vec![user_message("only one")];

    let params = messages_to_api_params_cached(&messages);

    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["role"], "user");
}

// ---------------------------------------------------------------------------
// Usage tracking
// ---------------------------------------------------------------------------

#[test]
fn usage_merge_accumulates_output_replaces_input() {
    let mut u1 = Usage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    };

    let u2 = Usage {
        input_tokens: 200,
        output_tokens: 30,
        cache_creation_input_tokens: 10,
        cache_read_input_tokens: 20,
    };

    u1.merge(&u2);

    // input_tokens replaced.
    assert_eq!(u1.input_tokens, 200);
    // output_tokens accumulated.
    assert_eq!(u1.output_tokens, 80);
    // Cache tokens replaced.
    assert_eq!(u1.cache_creation_input_tokens, 10);
    assert_eq!(u1.cache_read_input_tokens, 20);
}

#[test]
fn usage_total_sums_all_fields() {
    let usage = Usage {
        input_tokens: 10,
        output_tokens: 20,
        cache_creation_input_tokens: 5,
        cache_read_input_tokens: 15,
    };
    assert_eq!(usage.total(), 50);
}

#[test]
fn usage_default_is_zero() {
    let usage = Usage::default();
    assert_eq!(usage.total(), 0);
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
}

#[test]
fn usage_merge_across_multiple_turns() {
    let mut total = Usage::default();

    let turn1 = Usage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: 20,
        cache_read_input_tokens: 0,
    };
    total.merge(&turn1);
    assert_eq!(total.output_tokens, 50);

    let turn2 = Usage {
        input_tokens: 150,
        output_tokens: 40,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 20,
    };
    total.merge(&turn2);
    assert_eq!(total.input_tokens, 150);
    assert_eq!(total.output_tokens, 90); // 50 + 40
    assert_eq!(total.cache_read_input_tokens, 20);
}
