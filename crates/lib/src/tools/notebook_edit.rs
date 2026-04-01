//! NotebookEdit tool: edit Jupyter notebook cells.
//!
//! Supports editing code cells, markdown cells, and outputs
//! in `.ipynb` files.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct NotebookEditTool;

#[async_trait]
impl Tool for NotebookEditTool {
    fn name(&self) -> &'static str {
        "NotebookEdit"
    }

    fn description(&self) -> &'static str {
        "Edit cells in Jupyter notebooks (.ipynb files). Can replace cell \
         content, insert new cells, or delete cells."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path", "edit"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the .ipynb file"
                },
                "edit": {
                    "type": "object",
                    "description": "The edit to apply",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["replace", "insert", "delete"],
                            "description": "Edit action"
                        },
                        "cell_index": {
                            "type": "integer",
                            "description": "Index of the cell to edit (0-based)"
                        },
                        "cell_type": {
                            "type": "string",
                            "enum": ["code", "markdown"],
                            "description": "Cell type (for insert)"
                        },
                        "content": {
                            "type": "string",
                            "description": "New cell content"
                        }
                    }
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

        let edit = input
            .get("edit")
            .ok_or_else(|| ToolError::InvalidInput("'edit' is required".into()))?;

        let action = edit
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'edit.action' is required".into()))?;

        // Read and parse the notebook.
        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read {file_path}: {e}")))?;

        let mut notebook: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| ToolError::ExecutionFailed(format!("Invalid notebook JSON: {e}")))?;

        let cells = notebook
            .get_mut("cells")
            .and_then(|v| v.as_array_mut())
            .ok_or_else(|| ToolError::ExecutionFailed("Notebook has no 'cells' array".into()))?;

        match action {
            "replace" => {
                let idx = edit
                    .get("cell_index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        ToolError::InvalidInput("'cell_index' required for replace".into())
                    })? as usize;

                let new_content =
                    edit.get("content")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            ToolError::InvalidInput("'content' required for replace".into())
                        })?;

                if idx >= cells.len() {
                    return Err(ToolError::InvalidInput(format!(
                        "Cell index {idx} out of range (notebook has {} cells)",
                        cells.len()
                    )));
                }

                // Replace the cell source.
                let source_lines: Vec<serde_json::Value> = new_content
                    .lines()
                    .map(|l| serde_json::Value::String(format!("{l}\n")))
                    .collect();
                cells[idx]["source"] = serde_json::Value::Array(source_lines);

                // Clear outputs for code cells.
                if cells[idx].get("cell_type").and_then(|v| v.as_str()) == Some("code") {
                    cells[idx]["outputs"] = serde_json::Value::Array(vec![]);
                    cells[idx]["execution_count"] = serde_json::Value::Null;
                }
            }
            "insert" => {
                let idx = edit
                    .get("cell_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(cells.len() as u64) as usize;

                let cell_type = edit
                    .get("cell_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("code");

                let new_content = edit.get("content").and_then(|v| v.as_str()).unwrap_or("");

                let source_lines: Vec<serde_json::Value> = new_content
                    .lines()
                    .map(|l| serde_json::Value::String(format!("{l}\n")))
                    .collect();

                let new_cell = if cell_type == "code" {
                    json!({
                        "cell_type": "code",
                        "source": source_lines,
                        "metadata": {},
                        "outputs": [],
                        "execution_count": null
                    })
                } else {
                    json!({
                        "cell_type": "markdown",
                        "source": source_lines,
                        "metadata": {}
                    })
                };

                let idx = idx.min(cells.len());
                cells.insert(idx, new_cell);
            }
            "delete" => {
                let idx = edit
                    .get("cell_index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        ToolError::InvalidInput("'cell_index' required for delete".into())
                    })? as usize;

                if idx >= cells.len() {
                    return Err(ToolError::InvalidInput(format!(
                        "Cell index {idx} out of range (notebook has {} cells)",
                        cells.len()
                    )));
                }

                cells.remove(idx);
            }
            other => {
                return Err(ToolError::InvalidInput(format!(
                    "Unknown action '{other}'. Use 'replace', 'insert', or 'delete'."
                )));
            }
        }

        // Write the modified notebook.
        let output = serde_json::to_string_pretty(&notebook)
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to serialize: {e}")))?;

        tokio::fs::write(file_path, &output)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write {file_path}: {e}")))?;

        Ok(ToolResult::success(format!(
            "Notebook {file_path}: {action} applied ({} cells total)",
            notebook["cells"].as_array().map(|c| c.len()).unwrap_or(0)
        )))
    }
}
