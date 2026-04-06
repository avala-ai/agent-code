//! Shell passthrough: capture subprocess output and build context messages.
//!
//! The `!` prefix in the REPL runs a shell command directly, streams output
//! to the terminal, captures it into a buffer, and injects the result into
//! conversation history as a `UserMessage` with `is_meta: true`.
//!
//! This module extracts the capture and message-building logic so it can be
//! tested independently of the REPL event loop.

use crate::llm::message::{ContentBlock, Message, UserMessage};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use uuid::Uuid;

/// Maximum bytes captured from shell output before truncation.
pub const MAX_CAPTURE_BYTES: usize = 50 * 1024; // 50KB

/// Result of capturing shell command output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedOutput {
    /// The captured text (may be truncated).
    pub text: String,
    /// Whether the output was truncated at `MAX_CAPTURE_BYTES`.
    pub truncated: bool,
    /// The exit code of the process, if available.
    pub exit_code: Option<i32>,
}

/// Capture text into a buffer with a size limit.
///
/// Reads lines from `reader`, calls `on_line` for each (e.g., to print to
/// the terminal), and appends to the buffer until `MAX_CAPTURE_BYTES` is reached.
pub fn capture_lines<R: Read>(
    reader: R,
    buffer: &mut String,
    truncated: &mut bool,
    mut on_line: impl FnMut(&str),
) {
    for line in BufReader::new(reader).lines() {
        match line {
            Ok(line) => {
                on_line(&line);
                if !*truncated {
                    if buffer.len() + line.len() + 1 > MAX_CAPTURE_BYTES {
                        *truncated = true;
                    } else {
                        buffer.push_str(&line);
                        buffer.push('\n');
                    }
                }
            }
            Err(_) => break,
        }
    }
}

/// Run a shell command, capture its output, and return the result.
///
/// Output is captured up to `MAX_CAPTURE_BYTES`. The `on_stdout` and
/// `on_stderr` callbacks are called for each line as it arrives (for
/// real-time terminal streaming).
pub fn run_and_capture(
    cmd: &str,
    cwd: &Path,
    mut on_stdout: impl FnMut(&str),
    mut on_stderr: impl FnMut(&str),
) -> Result<CapturedOutput, String> {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("bash error: {e}"))?;

    let mut captured = String::new();
    let mut truncated = false;

    if let Some(stdout) = child.stdout.take() {
        capture_lines(stdout, &mut captured, &mut truncated, &mut on_stdout);
    }

    if let Some(stderr) = child.stderr.take() {
        capture_lines(stderr, &mut captured, &mut truncated, &mut on_stderr);
    }

    let exit_code = child.wait().ok().and_then(|s| s.code());

    Ok(CapturedOutput {
        text: captured,
        truncated,
        exit_code,
    })
}

/// Build a context injection message from captured shell output.
///
/// Returns `None` if the captured text is empty (nothing to inject).
pub fn build_context_message(cmd: &str, output: &CapturedOutput) -> Option<Message> {
    if output.text.is_empty() {
        return None;
    }

    let suffix = if output.truncated {
        "\n[output truncated at 50KB]"
    } else {
        ""
    };
    let context_text = format!("[Shell output from: {cmd}]\n{}{suffix}", output.text);

    Some(Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        content: vec![ContentBlock::Text { text: context_text }],
        is_meta: true,
        is_compact_summary: false,
    }))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ── capture_lines tests ──────────────────────────────────────────

    #[test]
    fn captures_all_lines_under_limit() {
        let input = "line one\nline two\nline three\n";
        let reader = Cursor::new(input);
        let mut buffer = String::new();
        let mut truncated = false;
        let mut printed = Vec::new();

        capture_lines(reader, &mut buffer, &mut truncated, |line| {
            printed.push(line.to_string());
        });

        assert_eq!(buffer, "line one\nline two\nline three\n");
        assert!(!truncated);
        assert_eq!(printed, vec!["line one", "line two", "line three"]);
    }

    #[test]
    fn truncates_at_limit() {
        // Create input that exceeds MAX_CAPTURE_BYTES.
        let long_line = "x".repeat(1024);
        let lines: Vec<String> = (0..60).map(|_| long_line.clone()).collect();
        let input = lines.join("\n") + "\n";
        let reader = Cursor::new(input);
        let mut buffer = String::new();
        let mut truncated = false;

        capture_lines(reader, &mut buffer, &mut truncated, |_| {});

        assert!(truncated);
        assert!(buffer.len() <= MAX_CAPTURE_BYTES);
        // Should have captured some lines but not all 60.
        let captured_lines: Vec<&str> = buffer.lines().collect();
        assert!(captured_lines.len() < 60);
        assert!(captured_lines.len() > 0);
    }

    #[test]
    fn empty_input_produces_empty_buffer() {
        let reader = Cursor::new("");
        let mut buffer = String::new();
        let mut truncated = false;

        capture_lines(reader, &mut buffer, &mut truncated, |_| {});

        assert!(buffer.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn single_line_no_newline() {
        let reader = Cursor::new("just one line");
        let mut buffer = String::new();
        let mut truncated = false;

        capture_lines(reader, &mut buffer, &mut truncated, |_| {});

        assert_eq!(buffer, "just one line\n");
        assert!(!truncated);
    }

    #[test]
    fn on_line_called_for_every_line_even_after_truncation() {
        // Verify that the callback is always called (for terminal streaming)
        // even after truncation kicks in.
        let long_line = "x".repeat(MAX_CAPTURE_BYTES + 100);
        let input = format!("{long_line}\nsecond line\n");
        let reader = Cursor::new(input);
        let mut buffer = String::new();
        let mut truncated = false;
        let mut callback_count = 0;

        capture_lines(reader, &mut buffer, &mut truncated, |_| {
            callback_count += 1;
        });

        // Both lines should trigger the callback for terminal streaming.
        assert_eq!(callback_count, 2);
        assert!(truncated);
    }

    #[test]
    fn truncation_boundary_exact() {
        // Fill buffer to exactly MAX_CAPTURE_BYTES - 1, then add a line
        // that would push it over.
        let fill_len = MAX_CAPTURE_BYTES - 10;
        let fill = "a".repeat(fill_len);
        let overflow = "b".repeat(20);
        let input = format!("{fill}\n{overflow}\n");
        let reader = Cursor::new(input);
        let mut buffer = String::new();
        let mut truncated = false;

        capture_lines(reader, &mut buffer, &mut truncated, |_| {});

        // First line should be captured (fill_len + 1 newline = MAX - 9).
        // Second line (20 + 1) would exceed MAX, so truncation should fire.
        assert!(truncated);
        assert!(buffer.contains(&fill));
        assert!(!buffer.contains(&overflow));
    }

    // ── build_context_message tests ──────────────────────────────────

    #[test]
    fn builds_message_with_header() {
        let output = CapturedOutput {
            text: "hello world\n".to_string(),
            truncated: false,
            exit_code: Some(0),
        };

        let msg = build_context_message("echo hello", &output).unwrap();
        if let Message::User(u) = &msg {
            assert!(u.is_meta);
            assert!(!u.is_compact_summary);
            assert_eq!(u.content.len(), 1);
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.starts_with("[Shell output from: echo hello]\n"));
                assert!(text.contains("hello world"));
                assert!(!text.contains("[output truncated"));
            } else {
                panic!("Expected Text block");
            }
        } else {
            panic!("Expected User message");
        }
    }

    #[test]
    fn builds_message_with_truncation_suffix() {
        let output = CapturedOutput {
            text: "partial output\n".to_string(),
            truncated: true,
            exit_code: Some(0),
        };

        let msg = build_context_message("big-cmd", &output).unwrap();
        if let Message::User(u) = &msg {
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.contains("[output truncated at 50KB]"));
                assert!(text.starts_with("[Shell output from: big-cmd]\n"));
            } else {
                panic!("Expected Text block");
            }
        } else {
            panic!("Expected User message");
        }
    }

    #[test]
    fn empty_output_returns_none() {
        let output = CapturedOutput {
            text: String::new(),
            truncated: false,
            exit_code: Some(0),
        };

        assert!(build_context_message("true", &output).is_none());
    }

    #[test]
    fn preserves_multiline_output() {
        let output = CapturedOutput {
            text: "line 1\nline 2\nline 3\n".to_string(),
            truncated: false,
            exit_code: Some(0),
        };

        let msg = build_context_message("seq 3", &output).unwrap();
        if let Message::User(u) = &msg {
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.contains("line 1\nline 2\nline 3\n"));
            } else {
                panic!("Expected Text block");
            }
        } else {
            panic!("Expected User message");
        }
    }

    #[test]
    fn special_characters_in_command_preserved() {
        let output = CapturedOutput {
            text: "ok\n".to_string(),
            truncated: false,
            exit_code: Some(0),
        };

        let msg = build_context_message("grep -r 'foo bar' .", &output).unwrap();
        if let Message::User(u) = &msg {
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.contains("[Shell output from: grep -r 'foo bar' .]"));
            } else {
                panic!("Expected Text block");
            }
        } else {
            panic!("Expected User message");
        }
    }

    // ── run_and_capture tests (real subprocess) ─────────────────────

    #[test]
    fn captures_stdout_from_echo() {
        let dir = std::env::temp_dir();
        let result = run_and_capture("echo hello_e2e", &dir, |_| {}, |_| {}).unwrap();

        assert_eq!(result.text, "hello_e2e\n");
        assert!(!result.truncated);
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn captures_stderr() {
        let dir = std::env::temp_dir();
        let result = run_and_capture("echo err_msg >&2", &dir, |_| {}, |_| {}).unwrap();

        assert!(result.text.contains("err_msg"));
        assert!(!result.truncated);
    }

    #[test]
    fn captures_mixed_stdout_stderr() {
        let dir = std::env::temp_dir();
        let result = run_and_capture(
            "echo stdout_line && echo stderr_line >&2",
            &dir,
            |_| {},
            |_| {},
        )
        .unwrap();

        assert!(result.text.contains("stdout_line"));
        assert!(result.text.contains("stderr_line"));
    }

    #[test]
    fn captures_multiline_output() {
        let dir = std::env::temp_dir();
        let result = run_and_capture("printf 'a\\nb\\nc\\n'", &dir, |_| {}, |_| {}).unwrap();

        assert_eq!(result.text, "a\nb\nc\n");
        assert!(!result.truncated);
    }

    #[test]
    fn handles_empty_output_command() {
        let dir = std::env::temp_dir();
        let result = run_and_capture("true", &dir, |_| {}, |_| {}).unwrap();

        assert!(result.text.is_empty());
        assert!(!result.truncated);
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn captures_nonzero_exit_code() {
        let dir = std::env::temp_dir();
        let result = run_and_capture("exit 42", &dir, |_| {}, |_| {}).unwrap();

        assert_eq!(result.exit_code, Some(42));
    }

    #[test]
    fn respects_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("marker.txt"), "found_it").unwrap();

        let result = run_and_capture("cat marker.txt", tmp.path(), |_| {}, |_| {}).unwrap();

        assert!(result.text.contains("found_it"));
    }

    #[test]
    fn callbacks_receive_all_lines() {
        let dir = std::env::temp_dir();
        let mut stdout_lines = Vec::new();
        let mut stderr_lines = Vec::new();

        let _ = run_and_capture(
            "echo out1 && echo out2 && echo err1 >&2",
            &dir,
            |line| stdout_lines.push(line.to_string()),
            |line| stderr_lines.push(line.to_string()),
        );

        assert_eq!(stdout_lines, vec!["out1", "out2"]);
        assert_eq!(stderr_lines, vec!["err1"]);
    }

    #[test]
    fn truncates_large_output() {
        let dir = std::env::temp_dir();
        // Generate >50KB of output: 1000 lines of 100 chars each = 100KB.
        let result = run_and_capture(
            "for i in $(seq 1 1000); do printf '%0100d\\n' $i; done",
            &dir,
            |_| {},
            |_| {},
        )
        .unwrap();

        assert!(result.truncated);
        assert!(result.text.len() <= MAX_CAPTURE_BYTES);
        // Should have captured some but not all 1000 lines.
        let line_count = result.text.lines().count();
        assert!(line_count > 100);
        assert!(line_count < 1000);
    }

    #[test]
    fn invalid_command_returns_error_output() {
        let dir = std::env::temp_dir();
        let result = run_and_capture(
            "nonexistent_command_xyz123 2>&1 || true",
            &dir,
            |_| {},
            |_| {},
        )
        .unwrap();

        // The command fails but bash itself runs fine.
        // stderr should contain the error message.
        assert!(
            result.text.contains("not found") || result.text.contains("No such file"),
            "Expected error message, got: {}",
            result.text
        );
    }

    // ── End-to-end: run_and_capture + build_context_message ──────────

    #[test]
    fn full_pipeline_echo_to_context_message() {
        let dir = std::env::temp_dir();
        let result = run_and_capture("echo integration_test_marker", &dir, |_| {}, |_| {}).unwrap();
        let msg = build_context_message("echo integration_test_marker", &result).unwrap();

        if let Message::User(u) = &msg {
            assert!(u.is_meta);
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.starts_with("[Shell output from: echo integration_test_marker]"));
                assert!(text.contains("integration_test_marker"));
            } else {
                panic!("Expected Text block");
            }
        } else {
            panic!("Expected User message");
        }
    }

    #[test]
    fn full_pipeline_empty_command_no_message() {
        let dir = std::env::temp_dir();
        let result = run_and_capture("true", &dir, |_| {}, |_| {}).unwrap();
        let msg = build_context_message("true", &result);

        assert!(msg.is_none());
    }

    #[test]
    fn full_pipeline_truncated_output_has_suffix() {
        let dir = std::env::temp_dir();
        let result = run_and_capture(
            "for i in $(seq 1 2000); do printf '%0100d\\n' $i; done",
            &dir,
            |_| {},
            |_| {},
        )
        .unwrap();

        assert!(result.truncated);

        let msg = build_context_message("big-output", &result).unwrap();
        if let Message::User(u) = &msg {
            if let ContentBlock::Text { text } = &u.content[0] {
                assert!(text.contains("[output truncated at 50KB]"));
                assert!(text.starts_with("[Shell output from: big-output]"));
            } else {
                panic!("Expected Text block");
            }
        } else {
            panic!("Expected User message");
        }
    }
}
