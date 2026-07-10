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
         In plan mode, only read-only tools are available. FileWrite/FileEdit/Bash \
         are blocked — pass the finished plan markdown to ExitPlanMode via its \
         `plan` argument instead of trying to write the plan file with other tools.\n\n\
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
         2. Explore the codebase (FileRead, Grep, Glob, and other read-only tools).\n\
         3. Call ExitPlanMode with `plan` set to the full markdown plan (this \
            writes the plan file and returns it for review).\n\n\
         The plan should include: Context (why), Approach (what), Critical \
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
             Explore with read-only tools, then call ExitPlanMode with a `plan` \
             argument containing the full markdown plan (that writes this file \
             and returns it for review). Do not use FileWrite/FileEdit while \
             plan mode is active — they are blocked.",
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
         Prefer passing the finished plan markdown via `plan` (written to the \
         plan file atomically). Returns the plan content for review."
    }

    fn prompt(&self) -> String {
        "Call ExitPlanMode when the plan is ready for user review. Pass the \
         full plan as the `plan` string argument — that is the supported way \
         to persist the plan while plan mode is active (FileWrite/FileEdit are \
         blocked). The tool writes `plan` to the plan file (if provided), then \
         returns the file content for approval.\n\n\
         Do not exit with an empty or still-templated plan. If the user requests \
         revisions, re-enter plan mode and call ExitPlanMode again with an \
         updated `plan`."
            .to_string()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "plan": {
                    "type": "string",
                    "description": "Full markdown plan content. When set, written to \
                        the plan file before exit so the plan can be persisted without \
                        FileWrite (which is blocked in plan mode)."
                },
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
        let mut plan_path = if let Some(p) = input.get("plan_path").and_then(|v| v.as_str()) {
            PathBuf::from(p)
        } else {
            match active_plan_path() {
                Some(p) => p,
                None => {
                    return Ok(ToolResult::error(
                        "No active plan file found. Call EnterPlanMode first, then \
                         ExitPlanMode with a `plan` argument — or pass plan_path explicitly.",
                    ));
                }
            }
        };

        // Persist plan content from the tool argument when provided. This is
        // the write path that remains available while plan_mode blocks
        // FileWrite/FileEdit/Bash (query loop sets plan_mode immediately after
        // EnterPlanMode succeeds).
        //
        // Security: only allow writes under the session plan directory so
        // ExitPlanMode cannot be used as an unprompted arbitrary file write
        // (it is classified read-only for executor purposes).
        if let Some(plan_md) = input.get("plan").and_then(|v| v.as_str()) {
            plan_path = ensure_path_under_plan_dir(&plan_path)?;
            if let Some(parent) = plan_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&plan_path, plan_md).map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "Failed to write plan file {}: {e}",
                    plan_path.display()
                ))
            })?;
            let _ = set_active_plan_path(&plan_path);
        }

        if !plan_path.exists() {
            return Ok(ToolResult::error(format!(
                "Plan file does not exist: {}. Call EnterPlanMode first, pass plan content \
                 via the `plan` argument, or pass a valid plan_path.",
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

/// Reject plan_path values that escape the plans directory.
fn ensure_path_under_plan_dir(path: &Path) -> Result<PathBuf, ToolError> {
    let root = plan_dir();
    let _ = std::fs::create_dir_all(&root);
    let root_canon = root
        .canonicalize()
        .map_err(|e| ToolError::ExecutionFailed(format!("plan dir unavailable: {e}")))?;

    // Absolute paths must resolve under root; relative paths are joined under root.
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root_canon.join(path)
    };

    // For non-existent files, canonicalize the parent and re-join the name.
    let checked = if candidate.exists() {
        candidate.canonicalize().map_err(|e| {
            ToolError::ExecutionFailed(format!("invalid plan path {}: {e}", candidate.display()))
        })?
    } else {
        let parent = candidate.parent().unwrap_or(Path::new("."));
        let file = candidate
            .file_name()
            .ok_or_else(|| ToolError::ExecutionFailed("plan path missing file name".into()))?;
        let parent_canon = if parent.exists() {
            parent.canonicalize().map_err(|e| {
                ToolError::ExecutionFailed(format!("invalid plan parent {}: {e}", parent.display()))
            })?
        } else if parent == Path::new("") || parent == Path::new(".") {
            root_canon.clone()
        } else {
            // Refuse creating nested dirs outside the plan root.
            return Err(ToolError::ExecutionFailed(format!(
                "plan path must be inside {}: {}",
                root_canon.display(),
                candidate.display()
            )));
        };
        parent_canon.join(file)
    };

    if !checked.starts_with(&root_canon) {
        return Err(ToolError::ExecutionFailed(format!(
            "plan path must be inside {}: {}",
            root_canon.display(),
            checked.display()
        )));
    }
    Ok(checked)
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

/// Generate a memorable slug for plan files (adjective-noun + unique suffix).
///
/// The UUID suffix avoids collisions when concurrent sessions or tests enter
/// plan mode in the same nanosecond (time-only slugs raced under Windows CI).
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

    let id = uuid::Uuid::new_v4();
    let bytes = id.as_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let adj = adjectives[n % adjectives.len()];
    let noun = nouns[(n / adjectives.len()) % nouns.len()];
    let suffix = &id.simple().to_string()[..8];

    format!("{adj}-{noun}-{suffix}")
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

        // Preferred path: pass plan markdown via ExitPlanMode (works while
        // plan_mode blocks FileWrite).
        let exit = ExitPlanModeTool
            .call(
                json!({
                    "plan_path": path.to_string_lossy(),
                    "plan": body,
                }),
                &ctx,
            )
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
        // File on disk should match what was passed in `plan`.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("Ship subagent types."));
    }

    #[tokio::test]
    async fn exit_with_plan_arg_writes_file_without_prior_filewrite() {
        let ctx = test_ctx();
        let enter = EnterPlanModeTool.call(json!({}), &ctx).await.unwrap();
        let path = plan_path_from_enter(&enter.content);
        // Leave template on disk; only ExitPlanMode writes the real plan.
        let plan = "# Plan\n\n## Context\n\nNeed a writable exit path.\n\n\
                    ## Approach\n\nPass `plan` to ExitPlanMode.\n\n\
                    ## Critical files\n\nplan_mode.rs\n\n## Verification\n\nunit test\n";
        let exit = ExitPlanModeTool
            .call(
                json!({
                    "plan_path": path.to_string_lossy(),
                    "plan": plan,
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!exit.is_error);
        assert!(exit.content.contains("Need a writable exit path."));
        assert!(
            !exit
                .content
                .contains("WARNING: Plan still looks like the template")
        );
    }

    #[tokio::test]
    async fn exit_warns_on_unfilled_template() {
        let ctx = test_ctx();
        // Isolate from concurrent plan-mode tests: unique path under tempfile,
        // not the shared data_dir plan slug which other tests may overwrite.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template-plan.md");
        let template = "# Plan\n\n## Context\n\n(why this change is needed)\n\n\
                        ## Approach\n\n(recommended approach — not every alternative)\n\n\
                        ## Critical files\n\n(paths to modify, and what each needs)\n";
        std::fs::write(&path, template).unwrap();

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
        // Unique non-existent path so concurrent tests cannot create it.
        let missing = std::env::temp_dir().join(format!(
            "agent-code-no-such-plan-{}.md",
            uuid::Uuid::new_v4()
        ));
        let exit = ExitPlanModeTool
            .call(json!({ "plan_path": missing.to_string_lossy() }), &ctx)
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

    #[tokio::test]
    async fn exit_plan_rejects_write_outside_plan_dir() {
        let ctx = test_ctx();
        let outside = std::env::temp_dir().join(format!(
            "agent-code-plan-escape-{}.md",
            uuid::Uuid::new_v4()
        ));
        let exit = ExitPlanModeTool
            .call(
                json!({
                    "plan_path": outside.to_string_lossy(),
                    "plan": "# Plan\n\nshould not land outside plan dir\n",
                }),
                &ctx,
            )
            .await;
        assert!(exit.is_err() || exit.as_ref().unwrap().is_error);
        assert!(!outside.exists(), "must not write outside plan dir");
    }
}
