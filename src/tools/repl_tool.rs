//! REPL tool: execute code in a Python or Node.js REPL.
//!
//! Spawns an interpreter subprocess, sends code, and captures output.
//! Useful for data exploration, quick calculations, and testing
//! code snippets without writing files.

use async_trait::async_trait;
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct ReplTool;

#[async_trait]
impl Tool for ReplTool {
    fn name(&self) -> &'static str {
        "REPL"
    }

    fn description(&self) -> &'static str {
        "Execute code in a Python or Node.js interpreter and return the output."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["language", "code"],
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["python", "node"],
                    "description": "Interpreter to use"
                },
                "code": {
                    "type": "string",
                    "description": "Code to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 30000)",
                    "default": 30000
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

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let language = input
            .get("language")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'language' is required".into()))?;

        let code = input
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'code' is required".into()))?;

        let timeout_ms = input
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(30_000)
            .min(120_000);

        let (cmd, flag) = match language {
            "python" => ("python3", "-c"),
            "node" => ("node", "-e"),
            other => {
                return Err(ToolError::InvalidInput(format!(
                    "Unsupported language '{other}'. Use 'python' or 'node'."
                )));
            }
        };

        let mut child = Command::new(cmd)
            .arg(flag)
            .arg(code)
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "Failed to start {language} interpreter: {e}. \
                     Make sure '{cmd}' is installed and in PATH."
                ))
            })?;

        let mut stdout_handle = child.stdout.take().unwrap();
        let mut stderr_handle = child.stderr.take().unwrap();
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        let timeout = Duration::from_millis(timeout_ms);

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
                        let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
                        let stderr = String::from_utf8_lossy(&stderr_buf).to_string();
                        let exit_code = status.code().unwrap_or(-1);

                        let mut content = String::new();
                        if !stdout.is_empty() {
                            content.push_str(&stdout);
                        }
                        if !stderr.is_empty() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(&stderr);
                        }
                        if content.is_empty() {
                            content = "(no output)".to_string();
                        }

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
