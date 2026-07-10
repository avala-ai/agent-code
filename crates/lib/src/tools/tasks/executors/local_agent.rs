//! `LocalAgent` executor — runs a subagent through the existing
//! [`AgentTool`](crate::tools::agent::AgentTool).
//!
//! We deliberately do not reimplement subagent spawning here. The
//! executor builds the JSON input the `Agent` tool already accepts
//! and forwards the call. That keeps worktree isolation, environment
//! plumbing, and the timeout semantics in one place.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use crate::permissions::PermissionChecker;
use crate::services::background::{TaskKind, TaskPayload, TaskStatus};
use crate::tools::agent::AgentTool;
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};
use crate::tools::{Tool, ToolContext};

pub struct LocalAgentExecutor;

#[async_trait]
impl TaskExecutor for LocalAgentExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::LocalAgent
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        let (subagent_kind, prompt) = match payload {
            TaskPayload::LocalAgent {
                subagent_kind,
                prompt,
                ..
            } => (subagent_kind.clone(), prompt.clone()),
            other => {
                return Err(TaskError::PayloadMismatch {
                    expected: TaskKind::LocalAgent,
                    actual: other.kind(),
                });
            }
        };

        if prompt.trim().is_empty() {
            return Err(TaskError::InvalidPayload(
                "LocalAgent payload requires a non-empty prompt".into(),
            ));
        }

        // Map our payload onto the input shape the Agent tool already
        // understands. Reuse, don't reimplement. Generate a stable
        // subagent id up front so the color assignment ties together
        // the queue entry, the AgentTool call, and any future
        // resume / fork follow-up.
        //
        // When `subagent_kind` matches a registered agent type
        // (explore / plan / general-purpose / custom disk agents),
        // pass it as `subagent_type` so the Agent tool applies the
        // right tool set and permissions. Otherwise treat it as a
        // free-form description only.
        let kind = subagent_kind.unwrap_or_else(|| "subagent".to_string());
        let mut registry = crate::services::coordinator::AgentRegistry::with_defaults();
        registry.load_from_disk(Some(&ctx.cwd));
        let known_type = registry.get(&kind).is_some();
        let description = kind.clone();
        let subagent_id = uuid::Uuid::new_v4().to_string();

        // Pre-assign a color so the queue entry we register can show
        // it immediately. AgentTool's own assign() call will be a
        // cheap idempotent no-op for the same id.
        let assigned_color = if let Some(mgr) = ctx.subagent_colors.as_ref() {
            Some(mgr.assign(&subagent_id).await)
        } else {
            None
        };

        // Register a queue entry so `/tasks` and `TaskList` can see
        // this subagent run. We transition the status when the
        // AgentTool call returns.
        let task_id = if let Some(tm) = ctx.task_manager.as_ref() {
            Some(
                tm.register_with_color(
                    &description,
                    TaskKind::LocalAgent,
                    TaskPayload::LocalAgent {
                        subagent_kind: Some(description.clone()),
                        prompt: prompt.clone(),
                        parent_session: None,
                    },
                    assigned_color,
                )
                .await,
            )
        } else {
            None
        };

        let mut input = json!({
            "description": description,
            "prompt": prompt,
            // Anchor AgentTool's assignment to the same id so the
            // colour doesn't bounce between two slots.
            "subagent_id": subagent_id,
        });
        if known_type {
            input
                .as_object_mut()
                .expect("object")
                .insert("subagent_type".into(), json!(kind));
        }

        // Build a minimal tool context. The Agent tool spawns a child
        // `agent` subprocess so it does not reach into permissions or
        // the file cache for itself — defaults are fine. Pass the
        // colour manager through so AgentTool sees the same shared
        // assignment table.
        let tool_ctx = ToolContext {
            cwd: ctx.cwd.clone(),
            cancel: ctx.cancel.clone(),
            permission_checker: Arc::new(PermissionChecker::allow_all()),
            verbose: false,
            plan_mode: false,
            file_cache: None,
            denial_tracker: None,
            task_manager: ctx.task_manager.clone(),
            subagent_colors: ctx.subagent_colors.clone(),
            session_allows: None,
            permission_prompter: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
        };

        let outcome = AgentTool.call(input, &tool_ctx).await;

        // Drive the queue entry to a terminal state regardless of
        // outcome, so `/tasks` doesn't show a perpetually-running row.
        if let (Some(tm), Some(id)) = (ctx.task_manager.as_ref(), task_id.as_ref()) {
            let status = match &outcome {
                Ok(r) if !r.is_error => TaskStatus::Completed,
                Ok(_) => TaskStatus::Failed("agent reported error".into()),
                Err(crate::error::ToolError::Cancelled) => TaskStatus::Killed,
                Err(e) => TaskStatus::Failed(e.to_string()),
            };
            let _ = tm.set_status(id, status).await;
        }

        match outcome {
            Ok(result) => Ok(TaskResult {
                output: result.content,
                is_error: result.is_error,
            }),
            Err(crate::error::ToolError::Cancelled) => Err(TaskError::Cancelled),
            Err(e) => Err(TaskError::ExecutionFailed(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn rejects_empty_prompt() {
        let exec = LocalAgentExecutor;
        let ctx = TaskContext::new(PathBuf::from("/tmp"));
        let payload = TaskPayload::LocalAgent {
            subagent_kind: None,
            prompt: "   ".into(),
            parent_session: None,
        };
        let err = exec.execute(&payload, &ctx).await.unwrap_err();
        assert!(matches!(err, TaskError::InvalidPayload(_)));
    }

    #[tokio::test]
    async fn rejects_wrong_payload_kind() {
        let exec = LocalAgentExecutor;
        let ctx = TaskContext::new(PathBuf::from("/tmp"));
        let payload = TaskPayload::Dream { note: None };
        let err = exec.execute(&payload, &ctx).await.unwrap_err();
        assert!(matches!(err, TaskError::PayloadMismatch { .. }));
    }
}
