//! `LocalWorkflow` executor — runs a named skill/workflow as a subagent.
//!
//! A `LocalWorkflow` payload names a skill slug and optional args. The
//! executor resolves the skill from the registry, expands its template,
//! and runs the result as a subagent through the existing
//! [`AgentTool`](crate::tools::agent::AgentTool) — the same path the
//! `LocalAgent` executor uses, so worktree isolation, env plumbing, and
//! timeout semantics stay in one place. The skill resolution is the only
//! new piece, and it is factored into [`resolve_workflow_prompt`] so it
//! can be tested without spawning a subprocess.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use crate::permissions::PermissionChecker;
use crate::services::background::{TaskKind, TaskPayload, TaskStatus};
use crate::skills::SkillRegistry;
use crate::tools::agent::AgentTool;
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};
use crate::tools::{Tool, ToolContext};

pub struct LocalWorkflowExecutor;

#[async_trait]
impl TaskExecutor for LocalWorkflowExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::LocalWorkflow
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        let (workflow, args) = match payload {
            TaskPayload::LocalWorkflow { workflow, args } => (workflow.clone(), args.clone()),
            other => {
                return Err(TaskError::PayloadMismatch {
                    expected: TaskKind::LocalWorkflow,
                    actual: other.kind(),
                });
            }
        };

        // Resolve the skill slug to a concrete prompt (the only
        // workflow-specific step; subagent execution is reused below).
        let registry = SkillRegistry::load_all(Some(ctx.cwd.as_path()));
        let prompt = resolve_workflow_prompt(&workflow, &args, &registry, ctx.disable_skill_shell)?;

        // From here this mirrors the LocalAgent executor: register a queue
        // entry, run the expanded prompt as a subagent, drive the status.
        let subagent_id = uuid::Uuid::new_v4().to_string();
        let assigned_color = if let Some(mgr) = ctx.subagent_colors.as_ref() {
            Some(mgr.assign(&subagent_id).await)
        } else {
            None
        };

        let task_id = if let Some(tm) = ctx.task_manager.as_ref() {
            Some(
                tm.register_with_color(
                    &workflow,
                    TaskKind::LocalWorkflow,
                    TaskPayload::LocalWorkflow {
                        workflow: workflow.clone(),
                        args: args.clone(),
                    },
                    assigned_color,
                )
                .await,
            )
        } else {
            None
        };

        let input = json!({
            "description": workflow,
            "prompt": prompt,
            "subagent_id": subagent_id,
        });

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
            question_asker: None,
            agent_origin: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
        };

        let outcome = AgentTool.call(input, &tool_ctx).await;

        if let (Some(tm), Some(id)) = (ctx.task_manager.as_ref(), task_id.as_ref()) {
            // Persist the subagent's output to the registered task's
            // file *before* marking it terminal. Completion surfacing
            // and `TaskOutput` read that file; without this the workflow
            // would surface/inject empty output despite producing content
            // (the subagent runs in-process, so nothing else writes it).
            let captured = match &outcome {
                Ok(r) => r.content.clone(),
                Err(e) => e.to_string(),
            };
            let _ = tm.write_output(id, &captured).await;

            let status = match &outcome {
                Ok(r) if !r.is_error => TaskStatus::Completed,
                Ok(_) => TaskStatus::Failed("workflow reported error".into()),
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

/// Resolve a workflow slug + args to the prompt a subagent should run.
///
/// Errors (as `InvalidPayload`) when the slug is empty, unknown, or
/// expands to nothing — so a caller gets a clear message rather than a
/// silently-empty subagent. Pure, so it can be tested without spawning.
///
/// `disable_shell` mirrors `security.disable_skill_shell_execution`:
/// when set, fenced shell blocks in the template are stripped before the
/// prompt reaches the autonomous subagent.
pub fn resolve_workflow_prompt(
    workflow: &str,
    args: &serde_json::Value,
    registry: &SkillRegistry,
    disable_shell: bool,
) -> Result<String, TaskError> {
    if workflow.trim().is_empty() {
        return Err(TaskError::InvalidPayload(
            "LocalWorkflow payload requires a workflow slug".into(),
        ));
    }

    let skill = registry
        .find(workflow)
        .ok_or_else(|| TaskError::InvalidPayload(format!("unknown workflow/skill '{workflow}'")))?;

    let args_str = workflow_args_to_str(args);
    let prompt = skill.expand_safe(args_str.as_deref(), disable_shell);
    if prompt.trim().is_empty() {
        return Err(TaskError::InvalidPayload(format!(
            "workflow/skill '{workflow}' expanded to an empty prompt"
        )));
    }
    Ok(prompt)
}

/// Coerce the free-form `args` value into the single `{{arg}}` string a
/// skill template expects. `null`/`""` → no args; a JSON string is used
/// verbatim; anything else is stringified.
fn workflow_args_to_str(args: &serde_json::Value) -> Option<String> {
    match args {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) if s.is_empty() => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn rejects_wrong_payload_kind() {
        let exec = LocalWorkflowExecutor;
        let ctx = TaskContext::new(PathBuf::from("/tmp"));
        let payload = TaskPayload::Dream { note: None };
        let err = exec.execute(&payload, &ctx).await.unwrap_err();
        assert!(matches!(err, TaskError::PayloadMismatch { .. }));
    }

    #[test]
    fn empty_slug_is_invalid() {
        let reg = SkillRegistry::load_bundled_only();
        let err =
            resolve_workflow_prompt("   ", &serde_json::Value::Null, &reg, false).unwrap_err();
        assert!(matches!(err, TaskError::InvalidPayload(_)));
    }

    #[test]
    fn unknown_slug_is_invalid() {
        let reg = SkillRegistry::load_bundled_only();
        let err = resolve_workflow_prompt(
            "definitely-not-a-real-skill",
            &serde_json::Value::Null,
            &reg,
            false,
        )
        .unwrap_err();
        match err {
            TaskError::InvalidPayload(msg) => assert!(msg.contains("unknown"), "{msg}"),
            other => panic!("expected InvalidPayload, got {other:?}"),
        }
    }

    #[test]
    fn known_bundled_skill_resolves_to_a_prompt() {
        let reg = SkillRegistry::load_bundled_only();
        let name = reg
            .all()
            .first()
            .expect("at least one bundled skill")
            .name
            .clone();
        let prompt = resolve_workflow_prompt(&name, &serde_json::Value::Null, &reg, false).unwrap();
        assert!(
            !prompt.trim().is_empty(),
            "skill '{name}' gave empty prompt"
        );
    }

    #[test]
    fn disable_shell_strips_fenced_blocks_from_workflow_prompt() {
        // A registry holding one skill whose body has a fenced bash
        // block. With the security flag off the block survives; with it
        // on, the block is replaced by the disabled notice before the
        // prompt would reach a subagent.
        let skill = crate::skills::Skill {
            name: "deploy".into(),
            metadata: Default::default(),
            body: "Run this:\n```bash\nrm -rf /\n```\nThen report.".into(),
            source: PathBuf::from("test"),
        };
        let reg = SkillRegistry::from_skills(vec![skill]);

        let allowed =
            resolve_workflow_prompt("deploy", &serde_json::Value::Null, &reg, false).unwrap();
        assert!(allowed.contains("rm -rf /"), "shell kept when allowed");

        let stripped =
            resolve_workflow_prompt("deploy", &serde_json::Value::Null, &reg, true).unwrap();
        assert!(
            !stripped.contains("rm -rf /"),
            "shell must be stripped when disabled: {stripped:?}"
        );
        assert!(
            stripped.contains("Shell execution disabled"),
            "stripped prompt should carry the disabled notice: {stripped:?}"
        );
        // Non-shell prose is preserved either way.
        assert!(stripped.contains("Then report."));
    }

    #[test]
    fn skill_arg_substitution() {
        let skill = crate::skills::Skill {
            name: "echo-arg".into(),
            metadata: Default::default(),
            body: "Do the thing with {{arg}}.".into(),
            source: PathBuf::from("test"),
        };
        assert_eq!(
            skill.expand(Some("the widget")),
            "Do the thing with the widget."
        );
    }

    #[test]
    fn args_coercion() {
        assert_eq!(workflow_args_to_str(&serde_json::Value::Null), None);
        assert_eq!(workflow_args_to_str(&serde_json::json!("")), None);
        assert_eq!(
            workflow_args_to_str(&serde_json::json!("hi")),
            Some("hi".to_string())
        );
        assert_eq!(
            workflow_args_to_str(&serde_json::json!(42)),
            Some("42".to_string())
        );
    }
}
