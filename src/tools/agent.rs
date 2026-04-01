//! Agent tool: spawn subagents for parallel task execution.
//!
//! Launches a new agent with its own query loop, isolated context,
//! and optionally a separate working directory. The subagent runs
//! the same tool set and LLM client but with its own conversation
//! history and permission scope.
//!
//! # Isolation modes
//!
//! - Default: shares the parent's working directory
//! - `worktree`: creates a temporary git worktree for isolated file changes

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct AgentTool;

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &'static str {
        "Agent"
    }

    fn description(&self) -> &'static str {
        "Launch a subagent to handle a complex task autonomously. The agent \
         runs with its own conversation context and can execute tools in parallel \
         with the main session."
    }

    fn prompt(&self) -> String {
        "Launch a subagent for complex, multi-step tasks. Each agent gets its own \
         conversation context and tool access. Use for:\n\
         - Parallel research or code exploration\n\
         - Tasks that would clutter the main conversation\n\
         - Independent subtasks that don't depend on each other\n\n\
         Provide a clear, complete prompt so the agent can work autonomously."
            .to_string()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["description", "prompt"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short (3-5 word) description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The complete task for the agent to perform"
                },
                "model": {
                    "type": "string",
                    "enum": ["sonnet", "opus", "haiku"],
                    "description": "Optional model override for this agent"
                },
                "isolation": {
                    "type": "string",
                    "enum": ["worktree"],
                    "description": "Run in an isolated git worktree"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run the agent in the background"
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

    fn max_result_size_chars(&self) -> usize {
        200_000
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'description' is required".into()))?;

        let prompt = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'prompt' is required".into()))?;

        let isolation = input.get("isolation").and_then(|v| v.as_str());

        // Determine working directory (worktree isolation if requested).
        let agent_cwd = if isolation == Some("worktree") {
            match create_worktree(&ctx.cwd).await {
                Ok(path) => path,
                Err(e) => {
                    return Ok(ToolResult::error(format!("Failed to create worktree: {e}")));
                }
            }
        } else {
            ctx.cwd.clone()
        };

        // Run the subagent as a Bash tool call to ourselves (rc --prompt).
        // This gives full isolation — separate process, separate context.
        let rc_binary = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "agent".to_string());

        let mut cmd = tokio::process::Command::new(&rc_binary);
        cmd.arg("--prompt")
            .arg(prompt)
            .current_dir(&agent_cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Pass through environment so the subagent uses the same provider.
        for var in &[
            "AGENT_CODE_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "XAI_API_KEY",
            "AGENT_CODE_API_BASE_URL",
            "AGENT_CODE_MODEL",
        ] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let timeout = std::time::Duration::from_secs(300); // 5 minute timeout.

        let result = tokio::select! {
            r = cmd.output() => {
                match r {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                        let mut content = format!("Agent ({description}) completed.\n\n");
                        if !stdout.is_empty() {
                            content.push_str(&stdout);
                        }
                        if !stderr.is_empty() && !output.status.success() {
                            content.push_str(&format!("\nAgent errors:\n{stderr}"));
                        }

                        // Clean up worktree if it was created.
                        if isolation == Some("worktree") {
                            let _ = cleanup_worktree(&agent_cwd).await;
                        }

                        Ok(ToolResult {
                            content,
                            is_error: !output.status.success(),
                        })
                    }
                    Err(e) => Err(ToolError::ExecutionFailed(format!(
                        "Failed to spawn agent: {e}"
                    ))),
                }
            }
            _ = tokio::time::sleep(timeout) => {
                Err(ToolError::Timeout(300_000))
            }
            _ = ctx.cancel.cancelled() => {
                Err(ToolError::Cancelled)
            }
        };

        result
    }
}

/// Create a temporary git worktree for isolated execution.
async fn create_worktree(base_cwd: &PathBuf) -> Result<PathBuf, String> {
    let branch_name = format!(
        "agent-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("tmp")
    );
    let worktree_path = std::env::temp_dir().join(format!("rc-worktree-{branch_name}"));

    let output = tokio::process::Command::new("git")
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&worktree_path)
        .current_dir(base_cwd)
        .output()
        .await
        .map_err(|e| format!("git worktree add failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {stderr}"));
    }

    Ok(worktree_path)
}

/// Clean up a temporary worktree.
async fn cleanup_worktree(worktree_path: &PathBuf) -> Result<(), String> {
    // Check if any changes were made.
    let status = tokio::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output()
        .await
        .map_err(|e| format!("git status failed: {e}"))?;

    let has_changes = !String::from_utf8_lossy(&status.stdout).trim().is_empty();

    if !has_changes {
        // No changes — remove the worktree.
        let _ = tokio::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .output()
            .await;
    }
    // If there are changes, leave the worktree for the user to inspect.

    Ok(())
}
