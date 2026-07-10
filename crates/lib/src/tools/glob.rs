//! Glob tool: file pattern matching.
//!
//! Finds files matching glob patterns. Results are sorted by
//! modification time (most recently modified first).

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Finds files matching a glob pattern. Returns paths sorted by modification time."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g., \"**/*.rs\", \"src/**/*.toml\")"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (defaults to cwd)"
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
        let pattern = input
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'pattern' is required".into()))?;

        let base_path = input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.cwd.clone());

        // Resolve the glob pattern relative to the base path.
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{pattern}", base_path.display())
        };

        let entries: Vec<PathBuf> = glob::glob(&full_pattern)
            .map_err(|e| ToolError::InvalidInput(format!("Invalid glob pattern: {e}")))?
            .filter_map(|entry| entry.ok())
            .filter(|p| p.is_file())
            // Confine results to the read scope when one is active (AMR
            // workers). A pattern like `**/*` matches dotfiles under the
            // default glob options, so without this a confined worker could
            // enumerate `.env` or `.git/` that the permission gate forbids as
            // an explicit path argument. No scope set → keeps every match.
            .filter(|p| ctx.permission_checker.read_scope_allows_path(p))
            .collect();

        // Sort by modification time (most recent first).
        let mut entries_with_mtime: Vec<(PathBuf, std::time::SystemTime)> = entries
            .into_iter()
            .filter_map(|p| {
                let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
                Some((p, mtime))
            })
            .collect();

        entries_with_mtime.sort_by_key(|e| std::cmp::Reverse(e.1));

        let total = entries_with_mtime.len();
        let max_results = 500;
        let truncated = total > max_results;

        let result: Vec<String> = entries_with_mtime
            .iter()
            .take(max_results)
            .map(|(p, _)| p.display().to_string())
            .collect();

        let mut output = format!("Found {total} files:\n{}", result.join("\n"));
        if truncated {
            output.push_str(&format!("\n\n(Showing {max_results} of {total} files)"));
        }

        Ok(ToolResult::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionChecker;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn ctx_with_scope(scope: Option<PathBuf>) -> ToolContext {
        let mut checker = PermissionChecker::allow_all();
        if let Some(root) = scope {
            checker = checker.with_read_scope(root);
        }
        ToolContext {
            cwd: PathBuf::from("."),
            cancel: CancellationToken::new(),
            permission_checker: Arc::new(checker),
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

    async fn glob_all(ctx: &ToolContext, root: &std::path::Path) -> String {
        let out = GlobTool
            .call(
                json!({ "pattern": "**/*", "path": root.to_string_lossy() }),
                ctx,
            )
            .await
            .unwrap();
        out.content
    }

    #[tokio::test]
    async fn read_scope_hides_dotfiles_from_recursive_glob() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.py"), "x").unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=sk-abc123").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "token=sk-def456").unwrap();

        // Unconfined: the raw glob surfaces the dotfiles (documents that the
        // scope filter below is load-bearing, not a no-op).
        let unconfined = glob_all(&ctx_with_scope(None), dir.path()).await;
        assert!(unconfined.contains("app.py"));
        assert!(
            unconfined.contains(".env"),
            "default glob returns dotfiles, so the scope filter must remove them"
        );

        // Confined to the scan root: dotfiles are filtered out entirely.
        let confined = glob_all(&ctx_with_scope(Some(dir.path().to_path_buf())), dir.path()).await;
        assert!(
            confined.contains("app.py"),
            "in-scope source is still listed"
        );
        assert!(
            !confined.contains(".env"),
            "a confined worker must not enumerate .env"
        );
        assert!(
            !confined.contains(".git/config") && !confined.contains(".git\\config"),
            "a confined worker must not enumerate .git internals"
        );
    }
}
