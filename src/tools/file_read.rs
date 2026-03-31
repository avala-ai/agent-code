//! FileRead tool: read file contents with optional line ranges.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &'static str {
        "FileRead"
    }

    fn description(&self) -> &'static str {
        "Reads a file from the filesystem. Returns contents with line numbers."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read"
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

        let offset = input
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;

        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000) as usize;

        let path = std::path::Path::new(file_path);

        // Handle binary/special file types.
        match path.extension().and_then(|e| e.to_str()) {
            Some("pdf") => {
                return Ok(ToolResult::success(format!(
                    "(PDF file: {file_path} — use a PDF extraction tool for contents)"
                )));
            }
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp") => {
                let meta = tokio::fs::metadata(file_path).await.ok();
                let size = meta.map(|m| m.len()).unwrap_or(0);
                return Ok(ToolResult::success(format!(
                    "(Image file: {file_path}, {size} bytes)"
                )));
            }
            Some("wasm" | "exe" | "dll" | "so" | "dylib" | "o" | "a") => {
                let meta = tokio::fs::metadata(file_path).await.ok();
                let size = meta.map(|m| m.len()).unwrap_or(0);
                return Ok(ToolResult::success(format!(
                    "(Binary file: {file_path}, {size} bytes)"
                )));
            }
            _ => {}
        }

        // Try to read as text; if it fails (binary content), report the file type.
        let content = match tokio::fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(e) => {
                // May be binary — try to read size at least.
                if let Ok(meta) = tokio::fs::metadata(file_path).await {
                    return Ok(ToolResult::success(format!(
                        "(Binary or unreadable file: {file_path}, {} bytes: {e})",
                        meta.len()
                    )));
                }
                return Err(ToolError::ExecutionFailed(format!(
                    "Failed to read {file_path}: {e}"
                )));
            }
        };

        // Apply line range and add line numbers (1-indexed).
        let lines: Vec<&str> = content.lines().collect();
        let start = (offset.saturating_sub(1)).min(lines.len());
        let end = (start + limit).min(lines.len());

        let mut output = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_num = start + i + 1;
            output.push_str(&format!("{line_num}\t{line}\n"));
        }

        if output.is_empty() {
            output = "(empty file)".to_string();
        }

        Ok(ToolResult::success(output))
    }
}
