//! FileEdit tool: targeted search-and-replace editing.
//!
//! Performs exact string replacement within a file. The `old_string`
//! must match uniquely (unless `replace_all` is set) to prevent
//! ambiguous edits.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct FileEditTool;

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &'static str {
        "FileEdit"
    }

    fn description(&self) -> &'static str {
        "Performs exact string replacements in files. The old_string must \
         match uniquely unless replace_all is true."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path", "old_string", "new_string"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text (must differ from old_string)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)",
                    "default": false
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

        let old_string = input
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'old_string' is required".into()))?;

        let new_string = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'new_string' is required".into()))?;

        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if old_string == new_string {
            return Err(ToolError::InvalidInput(
                "old_string and new_string must be different".into(),
            ));
        }

        // Check file size before reading (reject files > 1MB).
        const MAX_EDIT_SIZE: u64 = 1_048_576;
        if let Ok(meta) = tokio::fs::metadata(file_path).await
            && meta.len() > MAX_EDIT_SIZE
        {
            return Err(ToolError::InvalidInput(format!(
                "File too large for editing ({} bytes, max {}). \
                 Consider using Bash with sed/awk for large files.",
                meta.len(),
                MAX_EDIT_SIZE
            )));
        }

        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read {file_path}: {e}")))?;

        let occurrences = content.matches(old_string).count();

        if occurrences == 0 {
            return Err(ToolError::InvalidInput(format!(
                "old_string not found in {file_path}"
            )));
        }

        if occurrences > 1 && !replace_all {
            return Err(ToolError::InvalidInput(format!(
                "old_string has {occurrences} occurrences in {file_path}. \
                 Use replace_all=true to replace all, or provide a more \
                 specific old_string."
            )));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        tokio::fs::write(file_path, &new_content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write {file_path}: {e}")))?;

        let replaced = if replace_all { occurrences } else { 1 };
        Ok(ToolResult::success(format!(
            "Replaced {replaced} occurrence(s) in {file_path}"
        )))
    }
}
