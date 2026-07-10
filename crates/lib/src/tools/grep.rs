//! Grep tool: regex-based content search.
//!
//! Searches file contents using regular expressions. Shells out to
//! `rg` (ripgrep) when available for performance and .gitignore
//! awareness. Falls back to a built-in implementation.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Searches file contents using regular expressions. Powered by ripgrep."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., \"*.rs\", \"*.{ts,tsx}\")"
                },
                "type": {
                    "type": "string",
                    "description": "File type to search (e.g., \"js\", \"py\", \"rust\")"
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case-insensitive search",
                    "default": false
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers in output (content mode only)",
                    "default": true
                },
                "-A": {
                    "type": "integer",
                    "description": "Lines to show after each match (content mode only)"
                },
                "-B": {
                    "type": "integer",
                    "description": "Lines to show before each match (content mode only)"
                },
                "-C": {
                    "type": "integer",
                    "description": "Lines of context around each match (content mode only)"
                },
                "context": {
                    "type": "integer",
                    "description": "Alias for -C"
                },
                "multiline": {
                    "type": "boolean",
                    "description": "Enable multiline matching (pattern can span lines)",
                    "default": false
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode: content (matching lines), files_with_matches (file paths), count (match counts)",
                    "default": "files_with_matches"
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Limit output to first N lines/entries (default: 250, 0 for unlimited)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Skip first N lines/entries before applying head_limit",
                    "default": 0
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

        let search_path = input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.cwd.clone());

        let glob_filter = input.get("glob").and_then(|v| v.as_str());
        let type_filter = input.get("type").and_then(|v| v.as_str());

        let case_insensitive = input
            .get("-i")
            // Also check legacy field name for backwards compat.
            .or_else(|| input.get("case_insensitive"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let show_line_numbers = input.get("-n").and_then(|v| v.as_bool()).unwrap_or(true);

        let after_context = input.get("-A").and_then(|v| v.as_u64());
        let before_context = input.get("-B").and_then(|v| v.as_u64());
        let context = input
            .get("-C")
            .or_else(|| input.get("context"))
            // Also check legacy field name.
            .or_else(|| input.get("context_lines"))
            .and_then(|v| v.as_u64());

        let multiline = input
            .get("multiline")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let output_mode = input
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches");

        let head_limit = input
            .get("head_limit")
            // Also check legacy field name.
            .or_else(|| input.get("max_results"))
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(250);

        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        // Build ripgrep command.
        let mut cmd = Command::new("rg");
        cmd.arg("--color=never");

        // Output mode determines base flags.
        match output_mode {
            "files_with_matches" => {
                cmd.arg("--files-with-matches");
            }
            "count" => {
                cmd.arg("--count");
            }
            "content" => {
                // Content mode: show matching lines.
                if show_line_numbers {
                    cmd.arg("--line-number");
                }
                cmd.arg("--no-heading");
            }
            _ => {
                // Default to files_with_matches for unknown modes.
                cmd.arg("--files-with-matches");
            }
        }

        // Case sensitivity.
        if case_insensitive {
            cmd.arg("-i");
        }

        // Context flags (only meaningful in content mode).
        if output_mode == "content" {
            if let Some(a) = after_context {
                cmd.arg(format!("-A{a}"));
            }
            if let Some(b) = before_context {
                cmd.arg(format!("-B{b}"));
            }
            if let Some(c) = context {
                cmd.arg(format!("-C{c}"));
            }
        }

        // Multiline mode.
        if multiline {
            cmd.arg("--multiline").arg("--multiline-dotall");
        }

        // File type filter.
        if let Some(file_type) = type_filter {
            cmd.arg("--type").arg(file_type);
        }

        // Glob filter.
        if let Some(glob_pat) = glob_filter {
            cmd.arg("--glob").arg(glob_pat);
        }
        // Re-assert the hidden-file skip AFTER the user glob. ripgrep normally
        // skips dotfiles, but a `--glob '.env*'` re-includes them; ripgrep gives
        // precedence to the LAST matching glob, so appending these exclusions
        // stops a confined worker from whitelisting `.env`/dotfiles to defeat
        // the read-scope. Gated on the scope so interactive greps keep the
        // ability to target dotfiles explicitly.
        for excl in rg_hidden_exclude_globs(ctx.permission_checker.has_read_scope()) {
            cmd.arg(excl);
        }

        // `--` terminates option parsing: the user-controlled `pattern` (and
        // the `search_path`) are positional operands, never flags. Without it a
        // confined worker could pass `pattern: "-uu"` to re-enable ripgrep's
        // hidden/ignored search and exfiltrate `.env`, defeating the read-scope
        // confinement. It also lets a legitimate pattern begin with `-`.
        cmd.arg("--").arg(pattern).arg(&search_path);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = match cmd.output().await {
            Ok(out) => out,
            Err(_) => {
                // Fallback to grep if rg is not installed.
                let mut fallback = Command::new("grep");
                fallback.arg("-r").arg("--color=never");
                if show_line_numbers && output_mode == "content" {
                    fallback.arg("-n");
                }
                if case_insensitive {
                    fallback.arg("-i");
                }
                if output_mode == "files_with_matches" {
                    fallback.arg("-l");
                } else if output_mode == "count" {
                    fallback.arg("-c");
                }
                if let Some(glob_pat) = glob_filter {
                    fallback.arg("--include").arg(glob_pat);
                }
                // ripgrep skips hidden files/dirs by default; plain `grep -r`
                // does not. When confined to a read scope (AMR workers), match
                // ripgrep so a recursive grep of the scan root cannot return
                // secrets from `.env` or `.git/`. These excludes are appended
                // AFTER any user `--include` so that on a grep which resolves a
                // conflicting include/exclude by last-match, a worker-supplied
                // `--include=.env*` still cannot re-include a hidden file.
                for excl in fallback_hidden_excludes(ctx.permission_checker.has_read_scope()) {
                    fallback.arg(excl);
                }
                // Same option-injection guard as the ripgrep path above.
                fallback.arg("--").arg(pattern).arg(&search_path);
                fallback.stdout(Stdio::piped()).stderr(Stdio::piped());
                fallback.output().await.map_err(|e| {
                    ToolError::ExecutionFailed(format!(
                        "Neither rg nor grep available: {e}. Install ripgrep: brew install ripgrep"
                    ))
                })?
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Apply offset and head_limit.
        let lines: Vec<&str> = stdout.lines().collect();
        let total = lines.len();

        let after_offset = if offset > 0 {
            if offset >= total {
                Vec::new()
            } else {
                lines[offset..].to_vec()
            }
        } else {
            lines
        };

        let effective_limit = if head_limit == 0 {
            after_offset.len() // 0 means unlimited
        } else {
            head_limit
        };

        let truncated = after_offset.len() > effective_limit;
        let display_lines = &after_offset[..after_offset.len().min(effective_limit)];

        let mut result = display_lines.join("\n");
        if truncated {
            result.push_str(&format!(
                "\n\n(Showing {} of {} results. Use a more specific pattern or increase head_limit.)",
                effective_limit,
                after_offset.len()
            ));
        }

        if result.is_empty() {
            result = "No matches found.".to_string();
        }

        // Build summary based on output mode.
        match output_mode {
            "files_with_matches" => Ok(ToolResult::success(format!(
                "Found {total} matching files:\n{result}"
            ))),
            "count" => Ok(ToolResult::success(result)),
            "content" => {
                let num_files = display_lines
                    .iter()
                    .filter_map(|l| l.split(':').next())
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                Ok(ToolResult::success(format!(
                    "Found {total} matches across {num_files} files:\n{result}"
                )))
            }
            _ => Ok(ToolResult::success(result)),
        }
    }
}

/// Extra `grep` args that make the non-ripgrep fallback skip hidden files and
/// directories (`.env`, `.git/…`), matching ripgrep's default. `--exclude`
/// takes priority over `--include`, so this also neutralizes a user glob that
/// tries to whitelist a dotfile. Applied only when the call is confined to a
/// read scope (AMR workers); an unconfined, interactive grep keeps searching
/// hidden files as before.
fn fallback_hidden_excludes(has_read_scope: bool) -> &'static [&'static str] {
    if has_read_scope {
        &["--exclude-dir=.*", "--exclude=.*"]
    } else {
        &[]
    }
}

/// Trailing ripgrep `--glob` exclusions that re-assert the default dotfile skip
/// even when the user's `--glob` whitelists a hidden file (`--glob '.env*'`).
/// ripgrep honors the last matching glob, so these are appended after the user
/// glob. Applied only under a read scope (AMR workers).
fn rg_hidden_exclude_globs(has_read_scope: bool) -> &'static [&'static str] {
    if has_read_scope {
        &["--glob=!.*", "--glob=!**/.*/**"]
    } else {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionChecker;
    use crate::tools::ToolContext;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn hidden_excludes_only_apply_under_a_read_scope() {
        // Unconfined interactive grep is unchanged.
        assert!(fallback_hidden_excludes(false).is_empty());
        assert!(rg_hidden_exclude_globs(false).is_empty());
        // A confined worker skips hidden files/dirs, matching ripgrep's default
        // so the `grep -r` fallback cannot leak `.env` / `.git/` secrets.
        let confined = fallback_hidden_excludes(true);
        assert!(confined.contains(&"--exclude-dir=.*"));
        assert!(confined.contains(&"--exclude=.*"));
        // And the ripgrep path re-asserts the dotfile skip after a user glob.
        assert!(rg_hidden_exclude_globs(true).contains(&"--glob=!.*"));
    }

    fn ctx_with(scope: Option<PathBuf>) -> ToolContext {
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
            question_asker: None,
            agent_origin: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
        }
    }

    fn test_ctx() -> ToolContext {
        ctx_with(None)
    }

    #[tokio::test]
    async fn pattern_that_looks_like_a_flag_is_searched_literally() {
        // Regression for option injection: a `pattern` beginning with `-` must
        // be a positional regex, never parsed as an rg/grep flag. If it were,
        // `-uu` / `--hidden` would re-enable hidden-file search and defeat the
        // AMR read-scope confinement.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "harmless --hidden token\n").unwrap();

        let out = GrepTool
            .call(
                json!({
                    "pattern": "--hidden",
                    "path": dir.path().to_string_lossy(),
                    "output_mode": "content",
                }),
                &test_ctx(),
            )
            .await
            .unwrap();

        assert!(!out.is_error, "flag-like pattern must not error out");
        assert!(
            out.content.contains("--hidden"),
            "the pattern was treated as a literal regex (found the matching line), not a flag"
        );
    }

    #[tokio::test]
    async fn read_scope_blocks_glob_whitelisting_hidden_files() {
        // Regression: `--glob '.env*'` re-includes dotfiles in ripgrep, which
        // would let a confined worker exfiltrate `.env` despite the read scope.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join(".env"), "SECRET=sk-LEAK\n").unwrap();
        std::fs::write(root.join("app.py"), "x = \"sk-OK\"\n").unwrap();

        let input = json!({
            "pattern": "sk-",
            "path": root.to_string_lossy(),
            "glob": ".env*",
            "output_mode": "content",
        });

        // Confined: the glob cannot whitelist the hidden file.
        let confined = GrepTool
            .call(input.clone(), &ctx_with(Some(root.clone())))
            .await
            .unwrap()
            .content;
        assert!(
            !confined.contains("sk-LEAK"),
            "a read-scoped worker must not read .env via a glob whitelist"
        );

        // Interactive (no scope): the explicit glob still targets the dotfile.
        let open = GrepTool.call(input, &test_ctx()).await.unwrap().content;
        assert!(
            open.contains("sk-LEAK"),
            "without a scope, an explicit glob keeps targeting dotfiles"
        );
    }
}
