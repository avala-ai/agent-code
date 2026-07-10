//! Agent tool: spawn subagents for parallel task execution.
//!
//! Launches a new agent with its own query loop, isolated context,
//! and optionally a separate working directory. The subagent runs
//! the same tool set and LLM client but with its own conversation
//! history and permission scope.
//!
//! # Subagent types
//!
//! Built-in types (from [`crate::services::coordinator::AgentRegistry`]):
//! - `general-purpose` — full tool access (default)
//! - `explore` — read-only codebase investigation
//! - `plan` — read-only implementation planning
//!
//! Custom types load from `.agent/agents/*.md` and
//! `~/.config/agent-code/agents/*.md`.
//!
//! # Isolation modes
//!
//! - Default: shares the parent's working directory
//! - `worktree`: creates a temporary git worktree for isolated file changes

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::services::coordinator::{
    AgentDefinition, AgentRegistry, apply_agent_definition, compose_agent_prompt,
};
use crate::services::subagent_colors::SubagentColor;

/// Pull a stable id out of the input, falling back to a fresh uuid.
///
/// Callers (the model, the LocalAgent task executor) may pass a
/// `subagent_id` field through the JSON input to anchor the color
/// to a known id; otherwise we generate one so the assignment is
/// still deterministic across the rest of the call.
fn resolve_subagent_id(input: &serde_json::Value) -> String {
    if let Some(id) = input.get("subagent_id").and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return id.to_string();
    }
    uuid::Uuid::new_v4().to_string()
}

pub struct AgentTool;

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &'static str {
        "Agent"
    }

    fn description(&self) -> &'static str {
        "Launch a subagent to handle a complex task autonomously. The agent \
         runs with its own conversation context and can execute tools in parallel \
         with the main session. Choose a subagent_type (explore, plan, \
         general-purpose) to scope tools and permissions."
    }

    fn prompt(&self) -> String {
        "Launch a subagent for complex, multi-step tasks. Each agent gets its own \
         conversation context and tool access.\n\n\
         **When to use:**\n\
         - Parallel research or code exploration\n\
         - Tasks that would clutter the main conversation\n\
         - Independent subtasks that don't depend on each other\n\n\
         **subagent_type (pick the tightest fit):**\n\
         - `explore` — read-only search/read. Use for \"where is X?\", codebase \
           maps, gathering facts. Prefer this over general-purpose for investigation.\n\
         - `plan` — read-only architecture/planning. Use to design an approach \
           without writing code.\n\
         - `general-purpose` — full tools (default). Use only when the child must \
           edit, run mutating commands, or finish an implementation.\n\
         Custom types from `.agent/agents/*.md` are also accepted.\n\n\
         Provide a clear, complete prompt so the agent can work autonomously. \
         Do not assume the child inherits your recent conversation — put every \
         needed fact in `prompt`."
            .to_string()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["description", "prompt"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short (3-5 word) description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The complete task for the agent to perform"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "Agent type: general-purpose (default), explore \
                        (read-only research), plan (read-only architecture), or a \
                        custom name from .agent/agents/"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override for this agent \
                        (any provider model id, e.g. grok-4, gpt-5.4, claude-sonnet-4)"
                },
                "isolation": {
                    "type": "string",
                    "enum": ["worktree"],
                    "description": "Run in an isolated git worktree"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run the agent in the background"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn max_result_size_chars(&self) -> usize {
        200_000
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'description' is required".into()))?;

        let prompt = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'prompt' is required".into()))?;

        let isolation = input.get("isolation").and_then(|v| v.as_str());
        let model_override = input.get("model").and_then(|v| v.as_str());
        let subagent_type = input
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general-purpose");

        // Resolve the agent type from the registry (built-ins + disk).
        // Unknown types are a hard error so the model cannot silently
        // fall back to full write access when it meant explore/plan.
        let mut registry = AgentRegistry::with_defaults();
        registry.load_from_disk(Some(&ctx.cwd));
        let definition = registry.get(subagent_type).cloned().ok_or_else(|| {
            let known: Vec<&str> = registry.list().iter().map(|d| d.name.as_str()).collect();
            ToolError::InvalidInput(format!(
                "Unknown subagent_type '{subagent_type}'. Known types: {}",
                known.join(", ")
            ))
        })?;

        // Resolve a stable id and assign a color through the shared
        // manager. The id is also used to name a temporary worktree
        // (when isolation is requested) and is propagated to the
        // child via `AGENT_CODE_SUBAGENT_ID` so future renderers can
        // tie tool-call events back to a color.
        let subagent_id = resolve_subagent_id(&input);
        let assigned_color: Option<SubagentColor> = if let Some(mgr) = ctx.subagent_colors.as_ref()
        {
            Some(mgr.assign(&subagent_id).await)
        } else {
            None
        };

        // Determine working directory (worktree isolation if requested).
        let agent_cwd = if isolation == Some("worktree") {
            match create_worktree(&ctx.cwd).await {
                Ok(path) => path,
                Err(e) => {
                    return Ok(ToolResult::error(format!("Failed to create worktree: {e}")));
                }
            }
        } else {
            ctx.cwd.clone()
        };

        // Background mode: register a tracked task, spawn the subagent
        // subprocess detached, and return immediately. The subagent's
        // output is captured to the task's output file and surfaced when
        // it finishes (see `services::task_surface`). Requires a task
        // manager; without one we fall through to synchronous mode.
        let run_in_background = input
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if let Some(tm) = ctx.task_manager.as_ref().filter(|_| run_in_background) {
            let id = spawn_background_agent(
                prompt,
                description,
                &agent_cwd,
                tm,
                &subagent_id,
                assigned_color,
                ctx.active_disk_output_style.as_deref(),
                ctx.agent_limiter.clone(),
                Some(&definition),
                model_override,
            )
            .await;
            return Ok(ToolResult::success(format!(
                "Agent ({description}, type={subagent_type}) started in the background as task {id}. \
                 Its result surfaces automatically when it completes — do not wait on it."
            )));
        }

        // Foreground: spawn the subagent subprocess and await it.
        let mut cmd = build_subagent_command(
            prompt,
            &agent_cwd,
            &subagent_id,
            assigned_color,
            ctx.active_disk_output_style.as_deref(),
            Some(&definition),
            model_override,
        );
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let timeout = std::time::Duration::from_secs(300); // 5 minute timeout.

        let result = tokio::select! {
            r = cmd.output() => {
                match r {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                        let mut content = format!(
                            "Agent ({description}, type={subagent_type}) completed.\n\n"
                        );
                        if !stdout.is_empty() {
                            content.push_str(&stdout);
                        }
                        if !stderr.is_empty() && !output.status.success() {
                            content.push_str(&format!("\nAgent errors:\n{stderr}"));
                        }

                        // Clean up worktree if it was created.
                        if isolation == Some("worktree") {
                            let _ = cleanup_worktree(&agent_cwd).await;
                        }

                        Ok(ToolResult {
                            content,
                            is_error: !output.status.success(),
                        })
                    }
                    Err(e) => Err(ToolError::ExecutionFailed(format!(
                        "Failed to spawn agent: {e}"
                    ))),
                }
            }
            _ = tokio::time::sleep(timeout) => {
                Err(ToolError::Timeout(300_000))
            }
            _ = ctx.cancel.cancelled() => {
                Err(ToolError::Cancelled)
            }
        };

        result
    }
}

/// Provider/runtime environment variables passed through to a spawned
/// subagent so it reaches the same provider, base URL, and model.
const SUBAGENT_ENV_PASSTHROUGH: &[&str] = &[
    "AGENT_CODE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "XAI_API_KEY",
    "GOOGLE_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "TOGETHER_API_KEY",
    "AGENT_CODE_API_BASE_URL",
    "AGENT_CODE_MODEL",
];

/// Build the `agent --prompt` subprocess command for a subagent run.
///
/// Sets the program, prompt, working directory, provider env
/// passthrough, and the subagent role/id/color/output-style markers.
/// When `definition` is `Some`, also applies type-specific model,
/// max-turns, read-only plan mode, system-prompt prefix, and
/// permissions/tool-visibility overlays.
/// The caller configures stdio: the foreground path uses `output()`;
/// the background path hands the command to
/// [`crate::services::background::TaskManager::spawn_command`], which
/// pipes stdio and isolates the process group.
pub fn build_subagent_command(
    prompt: &str,
    cwd: &std::path::Path,
    subagent_id: &str,
    color: Option<SubagentColor>,
    disk_output_style: Option<&str>,
    definition: Option<&AgentDefinition>,
    model_override: Option<&str>,
) -> tokio::process::Command {
    let full_prompt = match definition {
        Some(def) => compose_agent_prompt(def, prompt),
        None => prompt.to_string(),
    };

    let agent_binary = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "agent".to_string());

    let mut cmd = tokio::process::Command::new(&agent_binary);
    cmd.arg("--prompt").arg(full_prompt).current_dir(cwd);

    if let Some(def) = definition {
        apply_agent_definition(&mut cmd, def, model_override);
    } else if let Some(model) = model_override {
        cmd.arg("--model")
            .arg(crate::services::coordinator::resolve_model_alias(model));
    }

    for var in SUBAGENT_ENV_PASSTHROUGH {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }

    // Mark the child as a subagent and propagate its id/color so the
    // child renderer and output-style filtering behave correctly.
    cmd.env("AGENT_CODE_SUBAGENT", "1");
    cmd.env("AGENT_CODE_SUBAGENT_ID", subagent_id);
    if let Some(color) = color {
        cmd.env("AGENT_CODE_SUBAGENT_COLOR", color.as_str());
    }
    if let Some(name) = disk_output_style {
        cmd.env("AGENT_CODE_DISK_OUTPUT_STYLE", name);
    }
    if let Some(def) = definition {
        cmd.env("AGENT_CODE_SUBAGENT_TYPE", &def.name);
    }

    cmd
}

/// Spawn a subagent as a tracked background task and return its id.
///
/// Registers a `LocalAgent` queue entry, runs the subagent subprocess
/// detached with its output captured to the task's output file, and
/// returns immediately. The completion is surfaced by the interactive
/// loop (toast + result injection). Shared by the Agent tool's
/// `run_in_background` path and the REPL `&` prefix.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_background_agent(
    prompt: &str,
    description: &str,
    cwd: &std::path::Path,
    task_manager: &std::sync::Arc<crate::services::background::TaskManager>,
    subagent_id: &str,
    color: Option<SubagentColor>,
    disk_output_style: Option<&str>,
    limiter: Option<std::sync::Arc<crate::services::agent_control::AgentExecutionLimiter>>,
    definition: Option<&AgentDefinition>,
    model_override: Option<&str>,
) -> crate::services::background::TaskId {
    use crate::services::background::{TaskKind, TaskPayload};

    let cmd = build_subagent_command(
        prompt,
        cwd,
        subagent_id,
        color,
        disk_output_style,
        definition,
        model_override,
    );
    let payload = TaskPayload::LocalAgent {
        subagent_kind: Some(
            definition
                .map(|d| d.name.clone())
                .unwrap_or_else(|| description.to_string()),
        ),
        prompt: prompt.to_string(),
        parent_session: None,
    };
    task_manager
        .spawn_command(
            cmd,
            description,
            TaskKind::LocalAgent,
            payload,
            color,
            limiter,
        )
        .await
}

/// Spawn an already-resolved workflow/skill prompt as a background task.
///
/// Mirrors [`spawn_background_agent`] — same subprocess runner, output
/// capture, killable process group, and concurrency limiter — but tags
/// the task as [`TaskKind::LocalWorkflow`] and records a `LocalWorkflow`
/// payload (the originating skill slug + args) so `/tasks` and the
/// completion surface label it as a workflow run rather than a free-form
/// subagent. The caller is responsible for resolving the slug to
/// `prompt` (see `resolve_workflow_prompt`).
#[allow(clippy::too_many_arguments)]
pub async fn spawn_background_workflow(
    workflow: &str,
    args: serde_json::Value,
    prompt: &str,
    description: &str,
    cwd: &std::path::Path,
    task_manager: &std::sync::Arc<crate::services::background::TaskManager>,
    subagent_id: &str,
    color: Option<SubagentColor>,
    disk_output_style: Option<&str>,
    limiter: Option<std::sync::Arc<crate::services::agent_control::AgentExecutionLimiter>>,
) -> crate::services::background::TaskId {
    use crate::services::background::{TaskKind, TaskPayload};

    let cmd = build_subagent_command(
        prompt,
        cwd,
        subagent_id,
        color,
        disk_output_style,
        None,
        None,
    );
    let payload = TaskPayload::LocalWorkflow {
        workflow: workflow.to_string(),
        args,
    };
    task_manager
        .spawn_command(
            cmd,
            description,
            TaskKind::LocalWorkflow,
            payload,
            color,
            limiter,
        )
        .await
}

/// Create a temporary git worktree for isolated execution.
async fn create_worktree(base_cwd: &PathBuf) -> Result<PathBuf, String> {
    let branch_name = format!(
        "agent-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("tmp")
    );
    let worktree_path = std::env::temp_dir().join(format!("agent-wt-{branch_name}"));

    let output = tokio::process::Command::new("git")
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&worktree_path)
        .current_dir(base_cwd)
        .output()
        .await
        .map_err(|e| format!("git worktree add failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {stderr}"));
    }

    Ok(worktree_path)
}

/// Clean up a temporary worktree.
async fn cleanup_worktree(worktree_path: &PathBuf) -> Result<(), String> {
    // Check if any changes were made.
    let status = tokio::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output()
        .await
        .map_err(|e| format!("git status failed: {e}"))?;

    let has_changes = !String::from_utf8_lossy(&status.stdout).trim().is_empty();

    if !has_changes {
        // No changes — remove the worktree.
        let _ = tokio::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .output()
            .await;
    }
    // If there are changes, leave the worktree for the user to inspect.

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn build_subagent_command_sets_prompt_role_and_id() {
        let cmd = build_subagent_command(
            "do the thing",
            std::path::Path::new("/tmp"),
            "sid-1",
            None,
            None,
            None,
            None,
        );
        let std_cmd = cmd.as_std();

        let args: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a == "--prompt"), "args: {args:?}");
        assert!(args.iter().any(|a| a == "do the thing"), "args: {args:?}");

        let envs: HashMap<String, String> = std_cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert_eq!(
            envs.get("AGENT_CODE_SUBAGENT").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            envs.get("AGENT_CODE_SUBAGENT_ID").map(String::as_str),
            Some("sid-1")
        );
        // No color / output style passed → those env vars are absent.
        assert!(!envs.contains_key("AGENT_CODE_SUBAGENT_COLOR"));
        assert!(!envs.contains_key("AGENT_CODE_DISK_OUTPUT_STYLE"));
        assert!(!envs.contains_key("AGENT_CODE_SUBAGENT_TYPE"));
    }

    #[test]
    fn build_subagent_command_propagates_output_style() {
        let cmd = build_subagent_command(
            "p",
            std::path::Path::new("/tmp"),
            "sid",
            None,
            Some("concise"),
            None,
            None,
        );
        let envs: HashMap<String, String> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert_eq!(
            envs.get("AGENT_CODE_DISK_OUTPUT_STYLE").map(String::as_str),
            Some("concise")
        );
    }

    #[test]
    fn build_subagent_command_applies_explore_type() {
        let registry = AgentRegistry::with_defaults();
        let def = registry.get("explore").expect("built-in explore");
        let cmd = build_subagent_command(
            "find auth code",
            std::path::Path::new("/tmp"),
            "sid-explore",
            None,
            None,
            Some(def),
            Some("grok-4"),
        );
        let std_cmd = cmd.as_std();
        let args: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        // System prompt from the explore definition is prefixed.
        let prompt_idx = args.iter().position(|a| a == "--prompt").expect("prompt");
        let prompt = &args[prompt_idx + 1];
        assert!(
            prompt.contains("exploration agent") || prompt.contains("find auth code"),
            "prompt should include definition system prompt + user task: {prompt}"
        );
        assert!(prompt.contains("find auth code"), "prompt: {prompt}");

        // Read-only types force plan permission mode.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--permission-mode" && w[1] == "plan"),
            "explore must run under plan mode: {args:?}"
        );
        // max_turns from definition.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--max-turns" && w[1] == "20"),
            "explore max_turns=20: {args:?}"
        );
        // model override wins.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--model" && w[1] == "grok-4"),
            "model override: {args:?}"
        );
        // Tool visibility overlay written for include_tools.
        assert!(
            args.iter().any(|a| a == "--permissions-overlay"),
            "include_tools should produce a permissions overlay: {args:?}"
        );

        let envs: HashMap<String, String> = std_cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert_eq!(
            envs.get("AGENT_CODE_SUBAGENT_TYPE").map(String::as_str),
            Some("explore")
        );
    }

    #[test]
    fn build_subagent_command_model_override_without_definition() {
        let cmd = build_subagent_command(
            "p",
            std::path::Path::new("/tmp"),
            "sid",
            None,
            None,
            None,
            Some("gpt-5.4"),
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--model" && w[1] == "gpt-5.4"),
            "args: {args:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_background_workflow_tags_localworkflow_kind_and_payload() {
        use crate::services::background::{TaskKind, TaskManager, TaskPayload};

        let tm = std::sync::Arc::new(TaskManager::new());
        let id = spawn_background_workflow(
            "review",
            serde_json::json!("src/main.rs"),
            "Review the code in src/main.rs",
            "/review src/main.rs",
            std::path::Path::new("."),
            &tm,
            "wf-sid-1",
            None,
            None,
            None,
        )
        .await;

        // The task is registered with the workflow kind and payload
        // immediately, regardless of how the (test-binary) child exits.
        let info = tm
            .list()
            .await
            .into_iter()
            .find(|t| t.id == id)
            .expect("workflow task registered");
        assert_eq!(info.kind, TaskKind::LocalWorkflow);
        match info.payload {
            Some(TaskPayload::LocalWorkflow { workflow, args }) => {
                assert_eq!(workflow, "review");
                assert_eq!(args, serde_json::json!("src/main.rs"));
            }
            other => panic!("expected LocalWorkflow payload, got {other:?}"),
        }

        // Don't leave the child running past the test.
        let _ = tm.kill(&id).await;
    }
}
