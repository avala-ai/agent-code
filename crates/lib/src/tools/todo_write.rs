//! TodoWrite tool: manage a structured todo list.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct TodoWriteTool;

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "TodoWrite"
    }

    fn description(&self) -> &'static str {
        "Write or update a structured todo list for tracking work items."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["todos"],
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "List of todo items",
                    "items": {
                        "type": "object",
                        "required": ["id", "content", "status"],
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier"
                            },
                            "content": {
                                "type": "string",
                                "description": "Todo description"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "done"],
                                "description": "Current status"
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["high", "medium", "low"],
                                "description": "Priority level"
                            }
                        }
                    }
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true // Todos are metadata, not file changes.
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let todos = input
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidInput("'todos' array is required".into()))?;

        let mut lines = Vec::new();
        for todo in todos {
            let id = todo.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let content = todo.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let status = todo
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");
            let marker = match status {
                "done" => "[x]",
                "in_progress" => "[~]",
                _ => "[ ]",
            };
            lines.push(format!("{marker} {id}: {content}"));
        }

        Ok(ToolResult::success(format!(
            "Todo list ({} items):\n{}",
            todos.len(),
            lines.join("\n")
        )))
    }
}
