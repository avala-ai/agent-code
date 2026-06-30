//! Surfacing finished background tasks back to the user and the model.
//!
//! When a background task ([`crate::services::background`]) finishes, the
//! interactive loop reports it once and injects a synthetic, `is_meta`
//! message so the model can reference the result on its next turn — the
//! same "tell the agent without polling" pattern used for shell
//! passthrough output. These helpers keep the envelope format in one
//! place so it is unit-testable independently of the REPL.

use crate::llm::message::{ContentBlock, Message, UserMessage};
use crate::services::background::{TaskInfo, TaskStatus};

/// Maximum number of characters of task output embedded in the
/// injected envelope. Truncated on a character boundary so multi-byte
/// output never panics.
const MAX_SNIPPET_CHARS: usize = 4000;

/// Short, stable human label for a task status.
pub fn status_label(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed(_) => "failed",
        TaskStatus::Killed => "killed",
    }
}

/// Character-safe truncation of `s` to at most `max` characters.
fn truncate_chars(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max).collect();
    out.push_str("\n[output truncated]");
    out
}

/// Render the envelope injected into the conversation for a finished
/// task. Tagged XML so the model can parse the id / kind / status.
pub fn completion_envelope(info: &TaskInfo, output: &str) -> String {
    format!(
        "<task id=\"{}\" kind=\"{}\" status=\"{}\">\n{}\n</task>",
        info.id,
        info.kind.as_str(),
        status_label(&info.status),
        truncate_chars(output, MAX_SNIPPET_CHARS),
    )
}

/// Build the synthetic, `is_meta` user message carrying a finished
/// task's result. `is_meta` so it informs the model without appearing
/// as user-authored input or polluting transcripts as a real turn.
pub fn build_completion_message(info: &TaskInfo, output: &str) -> Message {
    Message::User(UserMessage {
        uuid: uuid::Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        content: vec![ContentBlock::Text {
            text: completion_envelope(info, output),
        }],
        is_meta: true,
        is_compact_summary: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::background::TaskKind;
    use std::path::PathBuf;

    fn info(id: &str, status: TaskStatus) -> TaskInfo {
        TaskInfo {
            id: id.to_string(),
            description: "demo".into(),
            status,
            output_file: PathBuf::from("/tmp/x"),
            kind: TaskKind::LocalShell,
            payload: None,
            subagent_color: None,
            notified: false,
            pid: None,
            started_at: std::time::Instant::now(),
            finished_at: None,
        }
    }

    #[test]
    fn envelope_carries_id_kind_status_and_output() {
        let env = completion_envelope(&info("b1", TaskStatus::Completed), "hello world");
        assert!(env.contains("id=\"b1\""));
        assert!(env.contains("kind=\"LocalShell\""));
        assert!(env.contains("status=\"completed\""));
        assert!(env.contains("hello world"));
        assert!(env.contains("</task>"));
    }

    #[test]
    fn failed_status_label_is_failed() {
        let env = completion_envelope(&info("b2", TaskStatus::Failed("boom".into())), "");
        assert!(env.contains("status=\"failed\""));
    }

    #[test]
    fn long_multibyte_output_truncates_without_panic() {
        let big = "é".repeat(MAX_SNIPPET_CHARS + 500);
        let env = completion_envelope(&info("b3", TaskStatus::Completed), &big);
        assert!(env.contains("[output truncated]"));
    }

    #[test]
    fn build_message_is_meta_user_message() {
        let msg = build_completion_message(&info("b4", TaskStatus::Completed), "out");
        match msg {
            Message::User(u) => {
                assert!(u.is_meta, "injected task message must be meta");
                assert!(!u.content.is_empty());
            }
            _ => panic!("expected a user message"),
        }
    }
}
