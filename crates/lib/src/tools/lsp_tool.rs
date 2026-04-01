//! LSP tool: query language servers for diagnostics and symbols.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct LspTool;

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &'static str {
        "LSP"
    }

    fn description(&self) -> &'static str {
        "Query a language server for diagnostics, definitions, references, \
         or symbols in the current project."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["diagnostics", "definition", "references", "symbols"],
                    "description": "What to query from the language server"
                },
                "file_path": {
                    "type": "string",
                    "description": "File to query (required for diagnostics/definition/references)"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (1-based, for definition/references)"
                },
                "column": {
                    "type": "integer",
                    "description": "Column number (1-based, for definition/references)"
                },
                "query": {
                    "type": "string",
                    "description": "Symbol name to search (for symbols action)"
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
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'action' is required".into()))?;

        match action {
            "diagnostics" => {
                let file = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::InvalidInput("'file_path' required for diagnostics".into())
                    })?;

                // Shell out to common linters as a fallback when no LSP is connected.
                let ext = std::path::Path::new(file)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");

                let (cmd, args) = match ext {
                    "rs" => ("cargo", vec!["check", "--message-format=short"]),
                    "py" => ("python3", vec!["-m", "py_compile", file]),
                    "js" | "ts" | "tsx" | "jsx" => ("npx", vec!["tsc", "--noEmit", "--pretty"]),
                    "go" => ("go", vec!["vet", "./..."]),
                    "rb" => ("ruby", vec!["-c", file]),
                    _ => {
                        return Ok(ToolResult::success(format!(
                            "No linter available for .{ext} files. \
                             Connect an LSP server for full diagnostics."
                        )));
                    }
                };

                let output = tokio::process::Command::new(cmd)
                    .args(&args)
                    .current_dir(&ctx.cwd)
                    .output()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("{cmd} failed: {e}")))?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = format!("{stdout}{stderr}");

                if combined.trim().is_empty() {
                    Ok(ToolResult::success("No diagnostics found."))
                } else {
                    Ok(ToolResult::success(combined.to_string()))
                }
            }
            "definition" | "references" | "symbols" => Ok(ToolResult::success(format!(
                "LSP '{action}' requires a connected language server. \
                     Configure one in your settings to enable this feature."
            ))),
            other => Err(ToolError::InvalidInput(format!(
                "Unknown action '{other}'. Use: diagnostics, definition, references, symbols"
            ))),
        }
    }
}
