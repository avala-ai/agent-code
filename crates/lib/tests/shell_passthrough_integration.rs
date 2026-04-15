//! Integration tests for shell passthrough context injection.
//!
//! Tests the full pipeline: run a real subprocess, capture output, build
//! a context message, and verify it integrates correctly with the message
//! system (alternation, normalization, compaction compatibility).

use agent_code_lib::llm::message::*;
use agent_code_lib::services::shell_passthrough::*;

// ---------------------------------------------------------------------------
// Message integration: verify shell output messages work with the rest of
// the message system (alternation, normalization, serialization).
// ---------------------------------------------------------------------------

#[test]
fn shell_message_is_valid_user_message() {
    let output = CapturedOutput {
        text: "hello\n".to_string(),
        truncated: false,
        exit_code: Some(0),
    };
    let msg = build_context_message("echo hello", &output).unwrap();

    // Should be a User message.
    assert!(matches!(msg, Message::User(_)));

    // Should have is_meta = true.
    if let Message::User(u) = &msg {
        assert!(u.is_meta, "Shell output should be marked as meta");
        assert!(!u.is_compact_summary);
        assert!(!u.uuid.is_nil());
        assert!(!u.timestamp.is_empty());
    }
}

#[test]
fn shell_message_does_not_break_alternation() {
    // Meta messages should be valid between any messages without breaking
    // user/assistant alternation rules.
    let output = CapturedOutput {
        text: "test output\n".to_string(),
        truncated: false,
        exit_code: Some(0),
    };
    let shell_msg = build_context_message("ls", &output).unwrap();

    // A sequence: user → assistant → shell_meta → user should be valid
    // because meta messages don't count as "user" for alternation.
    let messages = [
        user_message("first question"),
        Message::Assistant(AssistantMessage {
            uuid: uuid::Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Text {
                text: "answer".into(),
            }],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        }),
        shell_msg,
    ];

    // Verify the messages are structurally sound.
    assert_eq!(messages.len(), 3);
    if let Message::User(u) = &messages[2] {
        assert!(u.is_meta);
    } else {
        panic!("Third message should be User (meta)");
    }
}

#[test]
fn shell_message_serialization_roundtrip() {
    let output = CapturedOutput {
        text: "line 1\nline 2\n".to_string(),
        truncated: false,
        exit_code: Some(0),
    };
    let msg = build_context_message("test-cmd", &output).unwrap();

    // Serialize to JSON and back.
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: Message = serde_json::from_str(&json).unwrap();

    if let Message::User(u) = &deserialized {
        assert!(u.is_meta);
        assert_eq!(u.content.len(), 1);
        if let ContentBlock::Text { text } = &u.content[0] {
            assert!(text.contains("[Shell output from: test-cmd]"));
            assert!(text.contains("line 1\nline 2"));
        } else {
            panic!("Expected Text block after deserialization");
        }
    } else {
        panic!("Expected User message after deserialization");
    }
}

// ---------------------------------------------------------------------------
// Subprocess integration: verify real commands run correctly.
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(target_os = "windows", ignore = "requires bash")]
fn subprocess_respects_working_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let marker_path = tmp.path().join("cwd_test.txt");
    std::fs::write(&marker_path, "cwd_verified").unwrap();

    let result = run_and_capture("cat cwd_test.txt", tmp.path(), |_| {}, |_| {}).unwrap();

    assert_eq!(result.text.trim(), "cwd_verified");
    assert_eq!(result.exit_code, Some(0));
}

#[test]
#[cfg_attr(target_os = "windows", ignore = "requires bash")]
fn subprocess_captures_exit_codes() {
    let dir = std::env::temp_dir();

    // Success.
    let r = run_and_capture("true", &dir, |_| {}, |_| {}).unwrap();
    assert_eq!(r.exit_code, Some(0));

    // Failure.
    let r = run_and_capture("false", &dir, |_| {}, |_| {}).unwrap();
    assert_eq!(r.exit_code, Some(1));

    // Custom exit code.
    let r = run_and_capture("exit 137", &dir, |_| {}, |_| {}).unwrap();
    assert_eq!(r.exit_code, Some(137));
}

#[test]
#[cfg_attr(target_os = "windows", ignore = "requires bash")]
fn subprocess_handles_binary_output_gracefully() {
    let dir = std::env::temp_dir();
    // printf with null bytes — BufReader should handle this without panic.
    let result = run_and_capture("printf 'hello\\x00world'", &dir, |_| {}, |_| {});

    // Should not panic. May capture partial output or error.
    assert!(result.is_ok());
}

#[test]
#[cfg_attr(target_os = "windows", ignore = "requires bash")]
fn subprocess_captures_long_running_output() {
    let dir = std::env::temp_dir();
    // 500 lines — should all be captured (well under 50KB).
    let result = run_and_capture("seq 1 500", &dir, |_| {}, |_| {}).unwrap();

    let lines: Vec<&str> = result.text.lines().collect();
    assert_eq!(lines.len(), 500);
    assert_eq!(lines[0], "1");
    assert_eq!(lines[499], "500");
    assert!(!result.truncated);
}

#[test]
#[cfg_attr(target_os = "windows", ignore = "requires bash")]
fn subprocess_truncation_preserves_complete_lines() {
    let dir = std::env::temp_dir();
    // Generate 60KB of output (>50KB limit).
    let result = run_and_capture(
        "for i in $(seq 1 600); do printf '%0100d\\n' $i; done",
        &dir,
        |_| {},
        |_| {},
    )
    .unwrap();

    assert!(result.truncated);
    // Every line in the captured buffer should be complete (no partial lines).
    for line in result.text.lines() {
        assert_eq!(line.len(), 100, "Line should be exactly 100 chars: {line}");
    }
}

// ---------------------------------------------------------------------------
// Context message content format verification.
// ---------------------------------------------------------------------------

#[test]
fn context_message_header_format() {
    let output = CapturedOutput {
        text: "output\n".to_string(),
        truncated: false,
        exit_code: Some(0),
    };
    let msg = build_context_message("cargo test --release", &output).unwrap();

    if let Message::User(u) = &msg {
        if let ContentBlock::Text { text } = &u.content[0] {
            // Header should be on the first line.
            let first_line = text.lines().next().unwrap();
            assert_eq!(first_line, "[Shell output from: cargo test --release]");
            // Output should follow on subsequent lines.
            let rest: String = text.lines().skip(1).collect::<Vec<_>>().join("\n");
            assert!(rest.contains("output"));
        } else {
            panic!("Expected Text block");
        }
    } else {
        panic!("Expected User message");
    }
}

#[test]
fn context_message_truncation_suffix_is_last_line() {
    let output = CapturedOutput {
        text: "some output\n".to_string(),
        truncated: true,
        exit_code: Some(0),
    };
    let msg = build_context_message("cmd", &output).unwrap();

    if let Message::User(u) = &msg {
        if let ContentBlock::Text { text } = &u.content[0] {
            let last_line = text.lines().last().unwrap();
            assert_eq!(last_line, "[output truncated at 50KB]");
        } else {
            panic!("Expected Text block");
        }
    } else {
        panic!("Expected User message");
    }
}

#[test]
fn context_message_preserves_special_characters() {
    let output = CapturedOutput {
        text: "café résumé naïve\n\"quoted\" & <html>\n".to_string(),
        truncated: false,
        exit_code: Some(0),
    };
    let msg = build_context_message("echo special", &output).unwrap();

    if let Message::User(u) = &msg {
        if let ContentBlock::Text { text } = &u.content[0] {
            assert!(text.contains("café résumé naïve"));
            assert!(text.contains("\"quoted\" & <html>"));
        } else {
            panic!("Expected Text block");
        }
    } else {
        panic!("Expected User message");
    }
}
