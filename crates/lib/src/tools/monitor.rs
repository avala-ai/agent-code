//! Background-task monitor tool.
//!
//! Lets the agent check on a long-running task it started with `!cmd &`
//! or with another background-spawning mechanism. Returns status +
//! tail of output in one call, which is cheaper than separate
//! `TaskGet` + `TaskOutput` calls and lets the agent loop on progress
//! (e.g. "check the build every 30s until it completes").

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::services::background::TaskStatus;

/// Maximum number of bytes of tail output returned per call. The agent
/// uses this to poll, so multi-MB tails would bloat context fast.
const TAIL_CAP_BYTES: usize = 8 * 1024;

pub struct MonitorTool;

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &'static str {
        "Monitor"
    }

    fn description(&self) -> &'static str {
        "Check on a background task: returns status, elapsed time, and the \
         tail of stdout+stderr (last ~8 KB). Use to poll long-running \
         tasks without blocking the agent loop. Pair with a shell `!cmd &` \
         invocation or with tasks kicked off by the agent itself."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Task ID returned when the background task was spawned"
                },
                "tail_bytes": {
                    "type": "integer",
                    "description": "Max bytes of output to return (capped at 8192). \
                                    Default is the cap.",
                    "minimum": 1
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        let tail_bytes = input
            .get("tail_bytes")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(TAIL_CAP_BYTES))
            .unwrap_or(TAIL_CAP_BYTES);

        let task_manager = ctx.task_manager.as_ref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "Monitor tool requires a task manager in context (not wired up)".into(),
            )
        })?;

        let info = task_manager.get_status(id).await.ok_or_else(|| {
            ToolError::NotFound(format!("Task '{id}' not found — was it actually spawned?"))
        })?;

        // Human-readable status line.
        let status_line = match &info.status {
            TaskStatus::Running => "running".to_string(),
            TaskStatus::Completed => "completed".to_string(),
            TaskStatus::Failed(msg) => format!("failed: {msg}"),
            TaskStatus::Killed => "killed".to_string(),
        };

        let elapsed = info
            .finished_at
            .unwrap_or_else(std::time::Instant::now)
            .saturating_duration_since(info.started_at);
        let elapsed_secs = elapsed.as_secs();

        // Tail of output.
        let tail = tail_of_file(&info.output_file, tail_bytes);

        let body = format!(
            "Task: {id}\n\
             Status: {status_line}\n\
             Elapsed: {elapsed_secs}s\n\
             Output file: {}\n\
             ─── tail ({} bytes) ───\n\
             {tail}",
            info.output_file.display(),
            tail.len(),
        );

        Ok(ToolResult::success(body))
    }
}

/// Read the last `max_bytes` of a file as UTF-8-lossy text. On read
/// error returns a short diagnostic instead of erroring the tool —
/// monitor is best-effort and should keep working even if the output
/// file is mid-rotation or momentarily unreadable.
fn tail_of_file(path: &std::path::Path, max_bytes: usize) -> String {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return format!("[output unreadable: {e}]"),
    };
    let start = bytes.len().saturating_sub(max_bytes);
    // Align to a UTF-8 char boundary so we don't split a multibyte char.
    let aligned = align_to_char_boundary(&bytes, start);
    String::from_utf8_lossy(&bytes[aligned..]).to_string()
}

/// Walk forward from `offset` until we're on a UTF-8 start byte
/// (`0b0xxxxxxx` or `0b11xxxxxx`). Prevents splitting a multibyte
/// character when slicing the tail.
fn align_to_char_boundary(bytes: &[u8], offset: usize) -> usize {
    let mut i = offset;
    while i < bytes.len() && (bytes[i] & 0b1100_0000) == 0b1000_0000 {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_to_char_boundary_passes_ascii_through() {
        let s = b"hello world";
        assert_eq!(align_to_char_boundary(s, 3), 3);
        assert_eq!(align_to_char_boundary(s, 0), 0);
        assert_eq!(align_to_char_boundary(s, s.len()), s.len());
    }

    #[test]
    fn align_to_char_boundary_walks_past_continuation_bytes() {
        // "é" is 0xC3 0xA9 — two bytes.
        let s = "hello é".as_bytes(); // ends: 0x20 0xC3 0xA9
        let boundary = s.len() - 1; // points at 0xA9 (continuation)
        // Should walk forward to s.len() (past the continuation).
        assert_eq!(align_to_char_boundary(s, boundary), s.len());
    }

    #[test]
    fn tail_of_file_returns_last_n_bytes_ascii() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, "a".repeat(100)).unwrap();
        let tail = tail_of_file(&path, 10);
        assert_eq!(tail.len(), 10);
        assert!(tail.chars().all(|c| c == 'a'));
    }

    #[test]
    fn tail_of_file_handles_smaller_than_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.txt");
        std::fs::write(&path, "hi").unwrap();
        let tail = tail_of_file(&path, 1024);
        assert_eq!(tail, "hi");
    }

    #[test]
    fn tail_of_file_survives_missing_file() {
        let tail = tail_of_file(std::path::Path::new("/nonexistent/file"), 100);
        assert!(tail.starts_with("[output unreadable"));
    }

    #[test]
    fn tail_of_file_does_not_split_multibyte_chars() {
        // File ends with "héllo" where é is 2 bytes; request a tail
        // that cuts mid-char — should align forward and not panic.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf8.txt");
        std::fs::write(&path, "abcdefhéllo").unwrap(); // 12 bytes
        let tail = tail_of_file(&path, 4); // would cut mid-é if naive
        // Tail should be valid UTF-8 ending with "llo".
        assert!(tail.ends_with("llo"));
    }
}
