//! Glob tool: file pattern matching.
//!
//! Finds files matching glob patterns. Results are sorted by
//! modification time (most recently modified first).

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Finds files matching a glob pattern. Returns paths sorted by modification time."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g., \"**/*.rs\", \"src/**/*.toml\")"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (defaults to cwd)"
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
        let pattern = input
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'pattern' is required".into()))?;

        let base_path = input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.cwd.clone());

        // Resolve the glob pattern relative to the base path.
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{pattern}", base_path.display())
        };

        let entries: Vec<PathBuf> = glob::glob(&full_pattern)
            .map_err(|e| ToolError::InvalidInput(format!("Invalid glob pattern: {e}")))?
            .filter_map(|entry| entry.ok())
            .filter(|p| p.is_file())
            .collect();

        // Sort by modification time (most recent first).
        let mut entries_with_mtime: Vec<(PathBuf, std::time::SystemTime)> = entries
            .into_iter()
            .filter_map(|p| {
                let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
                Some((p, mtime))
            })
            .collect();

        entries_with_mtime.sort_by_key(|e| std::cmp::Reverse(e.1));

        let total = entries_with_mtime.len();
        let max_results = 500;
        let truncated = total > max_results;

        let result: Vec<String> = entries_with_mtime
            .iter()
            .take(max_results)
            .map(|(p, _)| p.display().to_string())
            .collect();

        let mut output = format!("Found {total} files:\n{}", result.join("\n"));
        if truncated {
            output.push_str(&format!("\n\n(Showing {max_results} of {total} files)"));
        }

        Ok(ToolResult::success(output))
    }
}
