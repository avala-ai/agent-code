//! Task management tools.
//!
//! Full task lifecycle: create, update, get, list, stop, and read
//! output. Tasks are tracked in the background task manager and
//! persisted to the cache directory for retrieval.
//!
//! Each task carries a [`TaskKind`](crate::services::background::TaskKind)
//! so the model can distinguish a backgrounded shell command from a
//! subagent run, an MCP monitor, etc. `TaskCreate` accepts an optional
//! `kind` argument; `TaskList` and `TaskGet` surface it on every entry.

use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::ToolError;
use crate::services::background::TaskKind;
use crate::tools::{Tool, ToolContext, ToolResult};

static TASK_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Pull a `kind` field out of tool input. Defaults to `LocalShell`
/// for back-compat with the pre-8.13 schema, where the tool only
/// understood shell-style tasks.
fn parse_kind(input: &serde_json::Value) -> Result<TaskKind, ToolError> {
    match input.get("kind").and_then(|v| v.as_str()) {
        None => Ok(TaskKind::LocalShell),
        Some(s) => TaskKind::parse(s).ok_or_else(|| {
            ToolError::InvalidInput(format!(
                "unknown task kind '{s}'. Expected one of: \
                 LocalShell, LocalAgent, LocalWorkflow, MonitorMcp, RemoteAgent, Dream"
            ))
        }),
    }
}

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
                },
                "kind": {
                    "type": "string",
                    "enum": [
                        "LocalShell",
                        "LocalAgent",
                        "LocalWorkflow",
                        "MonitorMcp",
                        "RemoteAgent",
                        "Dream"
                    ],
                    "default": "LocalShell",
                    "description": "What kind of work this task represents. \
                                    Defaults to LocalShell for backward compat."
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

        let kind = parse_kind(&input)?;
        let id = TASK_COUNTER.fetch_add(1, Ordering::Relaxed);

        Ok(ToolResult::success(format!(
            "Task #{id} created [kind: {}]: {description} [{status}]",
            kind.as_str()
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
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        if let Some(mgr) = ctx.task_manager.as_ref()
            && let Some(info) = mgr.get_status(id).await
        {
            return Ok(ToolResult::success(format!(
                "Task #{id} [kind: {}] status: {:?}\n  description: {}\n  output_file: {}",
                info.kind.as_str(),
                info.status,
                info.description,
                info.output_file.display(),
            )));
        }

        Ok(ToolResult::success(format!(
            "Task #{id}: not found in the active task manager. \
             It may have been created by an older binary or already pruned."
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
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let count = TASK_COUNTER.load(Ordering::Relaxed) - 1;
        let mut out = format!("{count} task(s) created this session.");
        if let Some(mgr) = ctx.task_manager.as_ref() {
            let tasks = mgr.list().await;
            if !tasks.is_empty() {
                out.push_str("\n\nActive tasks:\n");
                for info in tasks {
                    out.push_str(&format!(
                        "  {} [kind: {}] {:?}: {}\n",
                        info.id,
                        info.kind.as_str(),
                        info.status,
                        info.description,
                    ));
                }
            }
        }
        Ok(ToolResult::success(out))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_ctx() -> ToolContext {
        use std::sync::Arc;
        ToolContext {
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp")),
            cancel: tokio_util::sync::CancellationToken::new(),
            permission_checker: Arc::new(crate::permissions::PermissionChecker::allow_all()),
            verbose: false,
            plan_mode: false,
            file_cache: None,
            denial_tracker: None,
            task_manager: None,
            subagent_colors: None,
            session_allows: None,
            permission_prompter: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
        }
    }

    #[tokio::test]
    async fn task_create_defaults_to_local_shell() {
        let res = TaskCreateTool
            .call(json!({ "description": "do a thing" }), &empty_ctx())
            .await
            .unwrap();
        assert!(res.content.contains("[kind: LocalShell]"));
    }

    #[tokio::test]
    async fn task_create_accepts_explicit_kind() {
        let res = TaskCreateTool
            .call(
                json!({ "description": "spawn helper", "kind": "LocalAgent" }),
                &empty_ctx(),
            )
            .await
            .unwrap();
        assert!(res.content.contains("[kind: LocalAgent]"));
    }

    #[tokio::test]
    async fn task_create_accepts_snake_case_kind() {
        let res = TaskCreateTool
            .call(
                json!({ "description": "watch", "kind": "monitor_mcp" }),
                &empty_ctx(),
            )
            .await
            .unwrap();
        assert!(res.content.contains("[kind: MonitorMcp]"));
    }

    #[tokio::test]
    async fn task_create_rejects_unknown_kind() {
        let err = TaskCreateTool
            .call(json!({ "description": "x", "kind": "Bogus" }), &empty_ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
