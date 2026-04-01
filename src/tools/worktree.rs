//! Worktree tools: manage isolated git worktrees for safe parallel work.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Enter a new git worktree for isolated file changes.
pub struct EnterWorktreeTool;

#[async_trait]
impl Tool for EnterWorktreeTool {
    fn name(&self) -> &'static str {
        "EnterWorktree"
    }

    fn description(&self) -> &'static str {
        "Create and enter a git worktree for isolated file changes. \
         Changes in the worktree don't affect the main working directory."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "branch": {
                    "type": "string",
                    "description": "Branch name for the worktree (auto-generated if omitted)"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let branch = input
            .get("branch")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                format!(
                    "worktree-{}",
                    uuid::Uuid::new_v4()
                        .to_string()
                        .split('-')
                        .next()
                        .unwrap_or("tmp")
                )
            });

        let worktree_path = std::env::temp_dir().join(format!("agent-wt-{branch}"));

        let output = tokio::process::Command::new("git")
            .args(["worktree", "add", "-b", &branch])
            .arg(&worktree_path)
            .current_dir(&ctx.cwd)
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("git worktree failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Ok(ToolResult::error(format!(
                "Worktree creation failed: {stderr}"
            )));
        }

        Ok(ToolResult::success(format!(
            "Worktree created at {} on branch '{branch}'.\n\
             Use this path as the working directory for isolated changes.",
            worktree_path.display()
        )))
    }
}

/// Exit and optionally clean up a git worktree.
pub struct ExitWorktreeTool;

#[async_trait]
impl Tool for ExitWorktreeTool {
    fn name(&self) -> &'static str {
        "ExitWorktree"
    }

    fn description(&self) -> &'static str {
        "Leave the current worktree. If no changes were made, the \
         worktree is automatically removed."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["worktree_path"],
            "properties": {
                "worktree_path": {
                    "type": "string",
                    "description": "Path to the worktree to exit"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let wt_path = input
            .get("worktree_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'worktree_path' is required".into()))?;

        let wt = PathBuf::from(wt_path);

        // Check if there are uncommitted changes.
        let status = tokio::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&wt)
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("git status failed: {e}")))?;

        let has_changes = !String::from_utf8_lossy(&status.stdout).trim().is_empty();

        if has_changes {
            Ok(ToolResult::success(format!(
                "Worktree at {wt_path} has uncommitted changes. \
                 Commit or stash them before removing the worktree."
            )))
        } else {
            // Clean removal.
            let _ = tokio::process::Command::new("git")
                .args(["worktree", "remove", wt_path])
                .current_dir(&ctx.cwd)
                .output()
                .await;

            Ok(ToolResult::success(format!(
                "Worktree at {wt_path} removed (no changes detected)."
            )))
        }
    }
}
