//! FileWrite tool: create or overwrite files.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &'static str {
        "FileWrite"
    }

    fn description(&self) -> &'static str {
        "Writes content to a file, creating it if needed or overwriting if it exists."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path", "content"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'file_path' is required".into()))?;

        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'content' is required".into()))?;

        let path = PathBuf::from(file_path);

        // Create parent directories if needed.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "Failed to create directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write {file_path}: {e}")))?;

        let lines = content.lines().count();
        let bytes = content.len();
        Ok(ToolResult::success(format!(
            "Wrote {lines} lines ({bytes} bytes) to {file_path}"
        )))
    }
}
