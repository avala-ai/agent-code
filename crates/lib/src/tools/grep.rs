//! Grep tool: regex-based content search.
//!
//! Searches file contents using regular expressions. Shells out to
//! `rg` (ripgrep) when available for performance and .gitignore
//! awareness. Falls back to a built-in implementation.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Searches file contents using regular expressions. Powered by ripgrep."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., \"*.rs\", \"*.{ts,tsx}\")"
                },
                "type": {
                    "type": "string",
                    "description": "File type to search (e.g., \"js\", \"py\", \"rust\")"
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case-insensitive search",
                    "default": false
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers in output (content mode only)",
                    "default": true
                },
                "-A": {
                    "type": "integer",
                    "description": "Lines to show after each match (content mode only)"
                },
                "-B": {
                    "type": "integer",
                    "description": "Lines to show before each match (content mode only)"
                },
                "-C": {
                    "type": "integer",
                    "description": "Lines of context around each match (content mode only)"
                },
                "context": {
                    "type": "integer",
                    "description": "Alias for -C"
                },
                "multiline": {
                    "type": "boolean",
                    "description": "Enable multiline matching (pattern can span lines)",
                    "default": false
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode: content (matching lines), files_with_matches (file paths), count (match counts)",
                    "default": "files_with_matches"
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Limit output to first N lines/entries (default: 250, 0 for unlimited)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Skip first N lines/entries before applying head_limit",
                    "default": 0
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

        let search_path = input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.cwd.clone());

        let glob_filter = input.get("glob").and_then(|v| v.as_str());
        let type_filter = input.get("type").and_then(|v| v.as_str());

        let case_insensitive = input
            .get("-i")
            // Also check legacy field name for backwards compat.
            .or_else(|| input.get("case_insensitive"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let show_line_numbers = input.get("-n").and_then(|v| v.as_bool()).unwrap_or(true);

        let after_context = input.get("-A").and_then(|v| v.as_u64());
        let before_context = input.get("-B").and_then(|v| v.as_u64());
        let context = input
            .get("-C")
            .or_else(|| input.get("context"))
            // Also check legacy field name.
            .or_else(|| input.get("context_lines"))
            .and_then(|v| v.as_u64());

        let multiline = input
            .get("multiline")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let output_mode = input
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches");

        let head_limit = input
            .get("head_limit")
            // Also check legacy field name.
            .or_else(|| input.get("max_results"))
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(250);

        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        // Build ripgrep command.
        let mut cmd = Command::new("rg");
        cmd.arg("--color=never");

        // Output mode determines base flags.
        match output_mode {
            "files_with_matches" => {
                cmd.arg("--files-with-matches");
            }
            "count" => {
                cmd.arg("--count");
            }
            "content" => {
                // Content mode: show matching lines.
                if show_line_numbers {
                    cmd.arg("--line-number");
                }
                cmd.arg("--no-heading");
            }
            _ => {
                // Default to files_with_matches for unknown modes.
                cmd.arg("--files-with-matches");
            }
        }

        // Case sensitivity.
        if case_insensitive {
            cmd.arg("-i");
        }

        // Context flags (only meaningful in content mode).
        if output_mode == "content" {
            if let Some(a) = after_context {
                cmd.arg(format!("-A{a}"));
            }
            if let Some(b) = before_context {
                cmd.arg(format!("-B{b}"));
            }
            if let Some(c) = context {
                cmd.arg(format!("-C{c}"));
            }
        }

        // Multiline mode.
        if multiline {
            cmd.arg("--multiline").arg("--multiline-dotall");
        }

        // File type filter.
        if let Some(file_type) = type_filter {
            cmd.arg("--type").arg(file_type);
        }

        // Glob filter.
        if let Some(glob_pat) = glob_filter {
            cmd.arg("--glob").arg(glob_pat);
        }

        cmd.arg(pattern).arg(&search_path);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to run rg: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Apply offset and head_limit.
        let lines: Vec<&str> = stdout.lines().collect();
        let total = lines.len();

        let after_offset = if offset > 0 {
            if offset >= total {
                Vec::new()
            } else {
                lines[offset..].to_vec()
            }
        } else {
            lines
        };

        let effective_limit = if head_limit == 0 {
            after_offset.len() // 0 means unlimited
        } else {
            head_limit
        };

        let truncated = after_offset.len() > effective_limit;
        let display_lines = &after_offset[..after_offset.len().min(effective_limit)];

        let mut result = display_lines.join("\n");
        if truncated {
            result.push_str(&format!(
                "\n\n(Showing {} of {} results. Use a more specific pattern or increase head_limit.)",
                effective_limit,
                after_offset.len()
            ));
        }

        if result.is_empty() {
            result = "No matches found.".to_string();
        }

        // Build summary based on output mode.
        match output_mode {
            "files_with_matches" => Ok(ToolResult::success(format!(
                "Found {total} matching files:\n{result}"
            ))),
            "count" => Ok(ToolResult::success(result)),
            "content" => {
                let num_files = display_lines
                    .iter()
                    .filter_map(|l| l.split(':').next())
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                Ok(ToolResult::success(format!(
                    "Found {total} matches across {num_files} files:\n{result}"
                )))
            }
            _ => Ok(ToolResult::success(result)),
        }
    }
}
