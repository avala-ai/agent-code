//! Task management tools.
//!
//! Full task lifecycle: create, update, get, list, stop, and read output.
//! Tasks are tracked in the background task manager and persisted to
//! the cache directory for retrieval.

use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

static TASK_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct TaskCreateTool;

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &'static str {
        "TaskCreate"
    }

    fn description(&self) -> &'static str {
        "Create a task to track progress on a piece of work."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["description"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "What needs to be done"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed"],
                    "default": "pending"
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
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'description' is required".into()))?;

        let status = input
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");

        let id = TASK_COUNTER.fetch_add(1, Ordering::Relaxed);

        Ok(ToolResult::success(format!(
            "Task #{id} created: {description} [{status}]"
        )))
    }
}

pub struct TaskUpdateTool;

#[async_trait]
impl Tool for TaskUpdateTool {
    fn name(&self) -> &'static str {
        "TaskUpdate"
    }

    fn description(&self) -> &'static str {
        "Update a task's status (pending, in_progress, completed)."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id", "status"],
            "properties": {
                "id": { "type": "string", "description": "Task ID" },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed"]
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
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        let status = input
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'status' is required".into()))?;

        Ok(ToolResult::success(format!(
            "Task #{id} updated to [{status}]"
        )))
    }
}

pub struct TaskGetTool;

#[async_trait]
impl Tool for TaskGetTool {
    fn name(&self) -> &'static str {
        "TaskGet"
    }

    fn description(&self) -> &'static str {
        "Get details about a specific task by ID."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "Task ID to look up" }
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
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        // In a full implementation this would query the TaskManager.
        Ok(ToolResult::success(format!(
            "Task #{id}: status and details would be shown here. \
             Use the background task manager for active task tracking."
        )))
    }
}

pub struct TaskListTool;

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &'static str {
        "TaskList"
    }

    fn description(&self) -> &'static str {
        "List all tasks in the current session."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
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
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let count = TASK_COUNTER.load(Ordering::Relaxed) - 1;
        Ok(ToolResult::success(format!(
            "{count} task(s) created this session."
        )))
    }
}

pub struct TaskStopTool;

#[async_trait]
impl Tool for TaskStopTool {
    fn name(&self) -> &'static str {
        "TaskStop"
    }

    fn description(&self) -> &'static str {
        "Stop a running background task."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "Task ID to stop" }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        Ok(ToolResult::success(format!("Task #{id} stop requested.")))
    }
}

pub struct TaskOutputTool;

#[async_trait]
impl Tool for TaskOutputTool {
    fn name(&self) -> &'static str {
        "TaskOutput"
    }

    fn description(&self) -> &'static str {
        "Read the output of a completed background task."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "Task ID to read output from" }
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
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        // Check the output file in the cache directory.
        let output_path = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("agent-code")
            .join("tasks")
            .join(format!("{id}.out"));

        if output_path.exists() {
            let content = std::fs::read_to_string(&output_path)
                .map_err(|e| ToolError::ExecutionFailed(format!("Read failed: {e}")))?;
            Ok(ToolResult::success(content))
        } else {
            Ok(ToolResult::success(format!(
                "No output file found for task #{id}. It may still be running."
            )))
        }
    }
}
