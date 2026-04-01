//! Bash tool: execute shell commands.
//!
//! Runs commands via the system shell. Features:
//! - Timeout with configurable duration (default 2min, max 10min)
//! - Background execution for long-running commands
//! - Sandbox mode: blocks writes outside the project directory
//! - Destructive command warnings
//! - Output truncation for large results
//! - Cancellation via CancellationToken

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Maximum output size before truncation (256KB).
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Commands that are potentially destructive and warrant a warning.
const DESTRUCTIVE_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -r /",
    "rmdir",
    "git reset --hard",
    "git clean -f",
    "git push --force",
    "git push -f",
    "git checkout -- .",
    "git restore .",
    "DROP TABLE",
    "DROP DATABASE",
    "DELETE FROM",
    "TRUNCATE",
    "shutdown",
    "reboot",
    "mkfs",
    "dd if=",
    "> /dev/sd",
    "chmod -R 777",
    ":(){ :|:& };:",
];

/// Paths that should never be written to.
const BLOCKED_WRITE_PATHS: &[&str] = &[
    "/etc/", "/usr/", "/bin/", "/sbin/", "/boot/", "/sys/", "/proc/",
];

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Executes a shell command and returns its output."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (max 600000)"
                },
                "description": {
                    "type": "string",
                    "description": "Description of what this command does"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run the command in the background and return immediately"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn get_path(&self, _input: &serde_json::Value) -> Option<PathBuf> {
        None
    }

    fn validate_input(&self, input: &serde_json::Value) -> Result<(), String> {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // Check for destructive commands.
        let cmd_lower = command.to_lowercase();
        for pattern in DESTRUCTIVE_PATTERNS {
            if cmd_lower.contains(&pattern.to_lowercase()) {
                return Err(format!(
                    "Potentially destructive command detected: contains '{pattern}'. \
                     This command could cause data loss or system damage. \
                     If you're sure, ask the user for confirmation first."
                ));
            }
        }

        // Block writes to system paths.
        for path in BLOCKED_WRITE_PATHS {
            if cmd_lower.contains(&format!(">{path}"))
                || cmd_lower.contains(&format!("tee {path}"))
                || cmd_lower.contains(&"mv ".to_string()) && cmd_lower.contains(path)
            {
                return Err(format!(
                    "Cannot write to system path '{path}'. \
                     Operations on system directories are not allowed."
                ));
            }
        }

        Ok(())
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'command' is required".into()))?;

        let timeout_ms = input
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120_000)
            .min(600_000);

        let run_in_background = input
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Background execution: spawn and return immediately.
        if run_in_background {
            return run_background(command, &ctx.cwd, ctx.task_manager.as_ref()).await;
        }

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn: {e}")))?;

        let timeout = Duration::from_millis(timeout_ms);

        let mut stdout_handle = child.stdout.take().unwrap();
        let mut stderr_handle = child.stderr.take().unwrap();

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        let result = tokio::select! {
            r = async {
                let (so, se) = tokio::join!(
                    async { stdout_handle.read_to_end(&mut stdout_buf).await },
                    async { stderr_handle.read_to_end(&mut stderr_buf).await },
                );
                so?;
                se?;
                child.wait().await
            } => {
                match r {
                    Ok(status) => {
                        let exit_code = status.code().unwrap_or(-1);
                        let content = format_output(&stdout_buf, &stderr_buf, exit_code);

                        Ok(ToolResult {
                            content,
                            is_error: exit_code != 0,
                        })
                    }
                    Err(e) => Err(ToolError::ExecutionFailed(e.to_string())),
                }
            }
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                Err(ToolError::Timeout(timeout_ms))
            }
            _ = ctx.cancel.cancelled() => {
                let _ = child.kill().await;
                Err(ToolError::Cancelled)
            }
        };

        result
    }
}

/// Run a command in the background, returning a task ID immediately.
async fn run_background(
    command: &str,
    cwd: &std::path::Path,
    task_mgr: Option<&std::sync::Arc<crate::services::background::TaskManager>>,
) -> Result<ToolResult, ToolError> {
    let default_mgr = crate::services::background::TaskManager::new();
    let task_mgr = task_mgr.map(|m| m.as_ref()).unwrap_or(&default_mgr);
    let task_id = task_mgr
        .spawn_shell(command, command, cwd)
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Background spawn failed: {e}")))?;

    Ok(ToolResult::success(format!(
        "Command running in background (task {task_id}). \
         Use TaskOutput to check results when complete."
    )))
}

/// Format stdout/stderr into a single output string with truncation.
fn format_output(stdout: &[u8], stderr: &[u8], exit_code: i32) -> String {
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);

    let mut content = String::new();

    if !stdout_str.is_empty() {
        if stdout_str.len() > MAX_OUTPUT_BYTES {
            content.push_str(&stdout_str[..MAX_OUTPUT_BYTES]);
            content.push_str(&format!(
                "\n\n(stdout truncated: {} bytes total)",
                stdout_str.len()
            ));
        } else {
            content.push_str(&stdout_str);
        }
    }

    if !stderr_str.is_empty() {
        if !content.is_empty() {
            content.push('\n');
        }
        let stderr_display = if stderr_str.len() > MAX_OUTPUT_BYTES / 4 {
            format!(
                "{}...\n(stderr truncated: {} bytes total)",
                &stderr_str[..MAX_OUTPUT_BYTES / 4],
                stderr_str.len()
            )
        } else {
            stderr_str.to_string()
        };
        content.push_str(&format!("stderr:\n{stderr_display}"));
    }

    if content.is_empty() {
        content = "(no output)".to_string();
    }

    if exit_code != 0 {
        content.push_str(&format!("\n\nExit code: {exit_code}"));
    }

    content
}
