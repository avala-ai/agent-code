//! PowerShell tool for Windows environments.
//!
//! Provides PowerShell command execution on Windows. On non-Windows
//! platforms, this tool is disabled and hidden from the registry.

use async_trait::async_trait;
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Maximum output bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

pub struct PowerShellTool;

#[async_trait]
impl Tool for PowerShellTool {
    fn name(&self) -> &'static str {
        "PowerShell"
    }

    fn description(&self) -> &'static str {
        "Executes a PowerShell command on Windows. Returns stdout and stderr."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The PowerShell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (max 600000)"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_enabled(&self) -> bool {
        cfg!(target_os = "windows")
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

        // Use pwsh (PowerShell Core) if available, fall back to powershell.exe.
        let shell = if which_exists("pwsh") {
            "pwsh"
        } else {
            "powershell"
        };

        let mut cmd = Command::new(shell);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        cmd.current_dir(&ctx.cwd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output =
            tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), cmd.output())
                .await
                .map_err(|_| {
                    ToolError::ExecutionFailed(format!("Command timed out after {timeout_ms}ms"))
                })?
                .map_err(|e| {
                    ToolError::ExecutionFailed(format!("Failed to run PowerShell: {e}"))
                })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("STDERR:\n");
            result.push_str(&stderr);
        }

        // Truncate if too large.
        if result.len() > MAX_OUTPUT_BYTES {
            result.truncate(MAX_OUTPUT_BYTES);
            result.push_str("\n\n(output truncated)");
        }

        let exit_code = output.status.code().unwrap_or(-1);
        if exit_code != 0 {
            result.push_str(&format!("\n\nExit code: {exit_code}"));
        }

        if output.status.success() {
            Ok(ToolResult::success(result))
        } else {
            Ok(ToolResult {
                content: result,
                is_error: true,
            })
        }
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .is_ok_and(|o| o.status.success())
}
