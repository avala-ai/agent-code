//! Plan mode tools: switch between execution and planning modes.
//!
//! Plan mode restricts the agent to read-only tools, preventing
//! mutations while the user reviews and approves a plan.
//! The LLM decides when to enter plan mode based on task complexity.
//!
//! The plan is written to a file under the agent-code data dir. On
//! exit, that file is read back and returned so the user (or parent
//! agent) can review the concrete plan before implementation begins.

use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Enter plan mode (read-only operations only).
pub struct EnterPlanModeTool;

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &'static str {
        "EnterPlanMode"
    }

    fn description(&self) -> &'static str {
        "Switch to plan mode for safe exploration before making changes. \
         Writes a plan file; call ExitPlanMode when the plan is ready for review."
    }

    fn prompt(&self) -> String {
        "Use this tool when a task has genuine ambiguity about the right approach \
         and getting user input before coding would prevent significant rework. \
         In plan mode, only read-only tools are available. Write tools are blocked \
         until ExitPlanMode is called.\n\n\
         When to enter plan mode:\n\
         - Complex tasks requiring multiple file changes\n\
         - Unclear requirements that need investigation first\n\
         - Multiple possible approaches to evaluate\n\
         - Large refactors where the plan should be reviewed\n\
         - When the user asks to \"plan\", \"think through\", or \"design\"\n\n\
         When NOT to enter plan mode:\n\
         - Straightforward fixes with a clear implementation path\n\
         - Single-file edits the user already specified\n\
         - The user wants to start coding immediately\n\n\
         Workflow:\n\
         1. Call EnterPlanMode — you receive a plan file path.\n\
         2. Explore the codebase (FileRead, Grep, Glob, read-only Bash).\n\
         3. Write a concrete plan to the plan file (overwrite the template).\n\
         4. Call ExitPlanMode — the plan content is returned for review.\n\n\
         The plan file should include: Context (why), Approach (what), Critical \
         files (paths), Reuse (existing functions/utilities), Verification \
         (how to test end-to-end), and Risks/open questions."
            .to_string()
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
        let plan_dir = plan_dir();
        let _ = std::fs::create_dir_all(&plan_dir);

        let slug = generate_slug();
        let plan_path = plan_dir.join(format!("{slug}.md"));

        // Create the plan file with a structured template (inspired by
        // well-scoped planning agents: context → approach → files → verify).
        let template = format!(
            "# Plan\n\n\
             Created: {}\n\n\
             ## Context\n\n\
             (why this change is needed)\n\n\
             ## Approach\n\n\
             (recommended approach — not every alternative)\n\n\
             ## Critical files\n\n\
             (paths to modify, and what each needs)\n\n\
             ## Reuse\n\n\
             (existing functions/utilities to reuse, with file paths)\n\n\
             ## Verification\n\n\
             (how to test the changes end to end)\n\n\
             ## Risks / open questions\n\n\
             (anything uncertain)\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"),
        );
        std::fs::write(&plan_path, &template)
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to create plan file: {e}")))?;

        // Session pointer so ExitPlanMode can find the active plan without
        // ToolContext plumbing. Overwritten on every EnterPlanMode call.
        if let Err(e) = set_active_plan_path(&plan_path) {
            return Err(ToolError::ExecutionFailed(format!(
                "Failed to record active plan path: {e}"
            )));
        }

        Ok(ToolResult::success(format!(
            "Entered plan mode. Only read-only tools are available.\n\
             Plan file: {}\n\
             Write your full plan to this file (replace the template sections), \
             then call ExitPlanMode when ready for review.",
            plan_path.display()
        )))
    }
}

/// Exit plan mode (re-enable all tools).
pub struct ExitPlanModeTool;

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &'static str {
        "ExitPlanMode"
    }

    fn description(&self) -> &'static str {
        "Exit plan mode and re-enable all tools for execution. \
         Reads the active plan file and returns its content for review. \
         Call this after the plan file is complete."
    }

    fn prompt(&self) -> String {
        "Call ExitPlanMode only after you have written a complete plan to the \
         plan file created by EnterPlanMode. The tool reads that file from disk \
         and returns its content so the user can approve or request changes.\n\n\
         Do not exit plan mode with an empty or still-templated plan. If the \
         user requests revisions, stay in plan mode, update the plan file, and \
         call ExitPlanMode again."
            .to_string()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "plan_path": {
                    "type": "string",
                    "description": "Optional path to the plan file. Defaults to the \
                        plan created by the most recent EnterPlanMode call."
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
        let plan_path = if let Some(p) = input.get("plan_path").and_then(|v| v.as_str()) {
            PathBuf::from(p)
        } else {
            match active_plan_path() {
                Some(p) => p,
                None => {
                    return Ok(ToolResult::error(
                        "No active plan file found. Call EnterPlanMode first, write the plan, \
                         then ExitPlanMode — or pass plan_path explicitly.",
                    ));
                }
            }
        };

        if !plan_path.exists() {
            return Ok(ToolResult::error(format!(
                "Plan file does not exist: {}. Call EnterPlanMode first or pass a valid plan_path.",
                plan_path.display()
            )));
        }

        let content = std::fs::read_to_string(&plan_path).map_err(|e| {
            ToolError::ExecutionFailed(format!(
                "Failed to read plan file {}: {e}",
                plan_path.display()
            ))
        })?;

        let mut notes = Vec::new();
        if looks_like_template(&content) {
            notes.push(
                "WARNING: Plan still looks like the template (placeholder sections remain). \
                 Prefer filling every section before implementing.",
            );
        }
        if content.trim().len() < 80 {
            notes.push(
                "WARNING: Plan content is very short. Consider adding more concrete steps \
                 and file paths before implementing.",
            );
        }

        let mut result =
            String::from("Exited plan mode. All tools are now available for execution.\n\n");
        result.push_str(&format!("Plan file: {}\n\n", plan_path.display()));
        if !notes.is_empty() {
            for n in &notes {
                result.push_str(n);
                result.push('\n');
            }
            result.push('\n');
        }
        result.push_str("--- BEGIN PLAN ---\n");
        result.push_str(content.trim_end());
        result.push_str("\n--- END PLAN ---\n");
        result.push_str(
            "\nPresent this plan to the user for approval before making code changes. \
             If they request revisions, re-enter plan mode, update the plan file, and \
             exit again.",
        );

        // Clear the active pointer so a stale plan is not reused later.
        let _ = clear_active_plan_path();

        Ok(ToolResult::success(result))
    }
}

fn plan_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agent-code")
        .join("plans")
}

fn active_plan_pointer() -> PathBuf {
    plan_dir().join(".active-plan")
}

fn set_active_plan_path(path: &Path) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(plan_dir());
    std::fs::write(active_plan_pointer(), path.to_string_lossy().as_bytes())
}

fn active_plan_path() -> Option<PathBuf> {
    let raw = std::fs::read_to_string(active_plan_pointer()).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn clear_active_plan_path() -> std::io::Result<()> {
    let ptr = active_plan_pointer();
    if ptr.exists() {
        std::fs::remove_file(ptr)?;
    }
    Ok(())
}

/// Detect an unmodified (or barely modified) plan template.
fn looks_like_template(content: &str) -> bool {
    const PLACEHOLDERS: &[&str] = &[
        "(why this change is needed)",
        "(recommended approach",
        "(paths to modify",
        "(existing functions/utilities",
        "(how to test the changes",
        "(anything uncertain)",
        // legacy template phrases
        "(describe what needs to be accomplished)",
        "(outline the steps)",
        "(list files and what changes each needs)",
    ];
    let hits = PLACEHOLDERS.iter().filter(|p| content.contains(*p)).count();
    hits >= 2
}

/// Generate a memorable slug for plan files (adjective-noun).
fn generate_slug() -> String {
    let adjectives = [
        "brave", "calm", "dark", "eager", "fair", "golden", "hidden", "iron", "jade", "keen",
        "light", "mystic", "noble", "ocean", "proud", "quick", "rapid", "silent", "true", "vivid",
    ];
    let nouns = [
        "anchor", "beacon", "cedar", "dawn", "ember", "falcon", "grove", "harbor", "island",
        "jewel", "kernel", "lantern", "meadow", "nexus", "orbit", "peak", "quill", "river",
        "spark", "tower",
    ];

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();

    let adj = adjectives[(now as usize) % adjectives.len()];
    let noun = nouns[((now as usize) / adjectives.len()) % nouns.len()];

    format!("{adj}-{noun}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionChecker;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("/tmp"),
            cancel: CancellationToken::new(),
            permission_checker: Arc::new(PermissionChecker::allow_all()),
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
        }
    }

    fn plan_path_from_enter(content: &str) -> PathBuf {
        content
            .lines()
            .find_map(|l| l.strip_prefix("Plan file: ").map(PathBuf::from))
            .expect("enter result should include 'Plan file: …'")
    }

    #[test]
    fn looks_like_template_detects_placeholders() {
        let template = "# Plan\n\n## Context\n\n(why this change is needed)\n\n\
                        ## Approach\n\n(recommended approach — not every alternative)\n";
        assert!(looks_like_template(template));
        assert!(!looks_like_template(
            "# Plan\n\n## Context\n\nNeed auth for the API.\n\n## Approach\n\nAdd JWT middleware.\n"
        ));
    }

    #[tokio::test]
    async fn enter_then_exit_returns_plan_content() {
        let ctx = test_ctx();
        let enter = EnterPlanModeTool
            .call(json!({}), &ctx)
            .await
            .expect("enter");
        assert!(!enter.is_error);
        assert!(enter.content.contains("Plan file:"));

        // Always use the path from the enter result (not the shared pointer)
        // so concurrent tests cannot redirect the exit read.
        let path = plan_path_from_enter(&enter.content);
        assert!(path.exists());

        let body = "# Plan\n\n## Context\n\nShip subagent types.\n\n## Approach\n\n\
             Wire Agent tool to AgentRegistry.\n\n## Critical files\n\n\
             crates/lib/src/tools/agent.rs\n\n## Verification\n\ncargo test -p agent-code-lib\n";
        std::fs::write(&path, body).unwrap();

        let exit = ExitPlanModeTool
            .call(json!({ "plan_path": path.to_string_lossy() }), &ctx)
            .await
            .expect("exit");
        assert!(!exit.is_error, "exit error: {}", exit.content);
        assert!(exit.content.contains("--- BEGIN PLAN ---"));
        assert!(
            exit.content.contains("Ship subagent types."),
            "exit content: {}",
            exit.content
        );
        assert!(exit.content.contains("Wire Agent tool to AgentRegistry."));
        assert!(
            !exit
                .content
                .contains("WARNING: Plan still looks like the template"),
            "filled plan should not warn: {}",
            exit.content
        );
    }

    #[tokio::test]
    async fn exit_warns_on_unfilled_template() {
        let ctx = test_ctx();
        let enter = EnterPlanModeTool.call(json!({}), &ctx).await.unwrap();
        let path = plan_path_from_enter(&enter.content);
        let exit = ExitPlanModeTool
            .call(json!({ "plan_path": path.to_string_lossy() }), &ctx)
            .await
            .unwrap();
        assert!(!exit.is_error);
        assert!(
            exit.content
                .contains("WARNING: Plan still looks like the template"),
            "exit content: {}",
            exit.content
        );
    }

    #[tokio::test]
    async fn exit_missing_plan_path_errors() {
        let ctx = test_ctx();
        let exit = ExitPlanModeTool
            .call(
                json!({ "plan_path": "/tmp/agent-code-no-such-plan-file.md" }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(exit.is_error);
        assert!(exit.content.contains("does not exist"));
    }

    #[test]
    fn looks_like_template_legacy_phrases() {
        let legacy = "# Plan\n\n(describe what needs to be accomplished)\n\
                      (outline the steps)\n(list files and what changes each needs)\n";
        assert!(looks_like_template(legacy));
    }
}
