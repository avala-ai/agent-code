//! ApplyPatch tool: multi-hunk, multi-file patch application.
//!
//! Accepts a free-form patch body in a compact "Begin Patch" dialect
//! (compatible with common coding-agent patch formats) and applies
//! updates/adds/deletes under the working directory.
//!
//! This is the engine-track tool tracked as issue #407 — separate from
//! the modern TUI workstream.

use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::permissions::{PermissionChecker, PermissionDecision};

/// Apply a multi-file patch in one tool call.
pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "ApplyPatch"
    }

    fn description(&self) -> &'static str {
        "Apply a multi-hunk patch to one or more files. Prefer this over \
         multiple FileEdit calls when changing several locations or files. \
         Patch dialect:\n\
         *** Begin Patch\n\
         *** Update File: relative/or/absolute/path\n\
         @@\n\
          context\n\
         -old line\n\
         +new line\n\
         *** Add File: path\n\
         +line1\n\
         +line2\n\
         *** Delete File: path\n\
         *** End Patch"
    }

    fn prompt(&self) -> String {
        "Use ApplyPatch for multi-hunk or multi-file edits. Each file section \
         starts with `*** Update File:`, `*** Add File:`, or `*** Delete File:`. \
         Hunks use unified-diff style lines (` `, `-`, `+`) optionally preceded \
         by `@@`. Always include enough context lines so the old block is unique \
         in the file."
            .to_string()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["patch"],
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Full patch body (Begin Patch … End Patch)"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        // Can delete files via *** Delete File:
        true
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        let patch = input.get("patch").and_then(|v| v.as_str())?;
        parse_patch(patch)
            .ok()
            .and_then(|ops| ops.into_iter().next().map(|o| o.path))
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        checker: &PermissionChecker,
    ) -> PermissionDecision {
        let patch = match input.get("patch").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return PermissionDecision::Deny("ApplyPatch requires 'patch'".into());
            }
        };
        let ops = match parse_patch(patch) {
            Ok(o) => o,
            Err(e) => return PermissionDecision::Deny(e),
        };
        for op in &ops {
            let path_str = op.path.to_string_lossy();
            let synthetic = json!({ "file_path": path_str });
            match checker.check(self.name(), &synthetic) {
                PermissionDecision::Allow => {}
                other => return other,
            }
        }
        PermissionDecision::Allow
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let patch = input
            .get("patch")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'patch' is required".into()))?;

        let ops = parse_patch(patch).map_err(ToolError::InvalidInput)?;
        if ops.is_empty() {
            return Err(ToolError::InvalidInput(
                "patch contained no file operations".into(),
            ));
        }

        let mut report = Vec::new();
        for op in ops {
            let abs = resolve_path(&ctx.cwd, &op.path);
            match op.kind {
                OpKind::Update { hunks } => {
                    let before = tokio::fs::read_to_string(&abs).await.map_err(|e| {
                        ToolError::ExecutionFailed(format!("read {}: {e}", abs.display()))
                    })?;
                    let after = apply_hunks(&before, &hunks).map_err(|e| {
                        ToolError::ExecutionFailed(format!("{}: {e}", abs.display()))
                    })?;
                    if let Some(parent) = abs.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|e| {
                            ToolError::ExecutionFailed(format!(
                                "create dir {}: {e}",
                                parent.display()
                            ))
                        })?;
                    }
                    tokio::fs::write(&abs, &after).await.map_err(|e| {
                        ToolError::ExecutionFailed(format!("write {}: {e}", abs.display()))
                    })?;
                    report.push(format!("updated {}", op.path.display()));
                }
                OpKind::Add { content } => {
                    if let Some(parent) = abs.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|e| {
                            ToolError::ExecutionFailed(format!(
                                "create dir {}: {e}",
                                parent.display()
                            ))
                        })?;
                    }
                    tokio::fs::write(&abs, &content).await.map_err(|e| {
                        ToolError::ExecutionFailed(format!("write {}: {e}", abs.display()))
                    })?;
                    report.push(format!("added {}", op.path.display()));
                }
                OpKind::Delete => {
                    if abs.exists() {
                        tokio::fs::remove_file(&abs).await.map_err(|e| {
                            ToolError::ExecutionFailed(format!("delete {}: {e}", abs.display()))
                        })?;
                        report.push(format!("deleted {}", op.path.display()));
                    } else {
                        report.push(format!("skip delete (missing) {}", op.path.display()));
                    }
                }
            }
        }

        Ok(ToolResult::success(format!(
            "ApplyPatch OK ({}):\n{}",
            report.len(),
            report.join("\n")
        )))
    }
}

#[derive(Debug)]
enum OpKind {
    Update { hunks: Vec<Hunk> },
    Add { content: String },
    Delete,
}

#[derive(Debug)]
struct FileOp {
    path: PathBuf,
    kind: OpKind,
}

#[derive(Debug, Clone)]
struct Hunk {
    /// Lines of the old block (context + removals), without `-`/` ` prefixes
    /// for removals we store the raw body; see apply_hunks.
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn parse_patch(patch: &str) -> Result<Vec<FileOp>, String> {
    let mut ops = Vec::new();
    let mut lines = patch.lines().peekable();

    // Optional envelope.
    while let Some(line) = lines.peek() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("*** Begin Patch") {
            lines.next();
            continue;
        }
        break;
    }

    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("*** End Patch") {
            continue;
        }

        if let Some(path) = t.strip_prefix("*** Update File:") {
            let path = PathBuf::from(path.trim());
            let hunks = parse_update_hunks(&mut lines)?;
            ops.push(FileOp {
                path,
                kind: OpKind::Update { hunks },
            });
        } else if let Some(path) = t.strip_prefix("*** Add File:") {
            let path = PathBuf::from(path.trim());
            let content = parse_add_body(&mut lines);
            ops.push(FileOp {
                path,
                kind: OpKind::Add { content },
            });
        } else if let Some(path) = t.strip_prefix("*** Delete File:") {
            let path = PathBuf::from(path.trim());
            ops.push(FileOp {
                path,
                kind: OpKind::Delete,
            });
        } else if t.starts_with("--- ") {
            // Unified-diff header path: --- a/foo  /  +++ b/foo
            let old = t.trim_start_matches("--- ").trim();
            let old = old
                .strip_prefix("a/")
                .unwrap_or(old)
                .split_whitespace()
                .next()
                .unwrap_or(old);
            // Expect +++ line
            let plus = lines.next().unwrap_or("").trim().to_string();
            let new = plus
                .trim_start_matches("+++ ")
                .trim()
                .strip_prefix("b/")
                .unwrap_or_else(|| plus.trim_start_matches("+++ ").trim());
            let new = new.split_whitespace().next().unwrap_or(new);
            let path = if new != "/dev/null" && !new.is_empty() {
                PathBuf::from(new)
            } else {
                PathBuf::from(old)
            };
            if new == "/dev/null" {
                ops.push(FileOp {
                    path,
                    kind: OpKind::Delete,
                });
                // drain hunks until next file header
                drain_until_file_header(&mut lines);
            } else if old == "/dev/null" {
                let content = parse_unified_add_body(&mut lines);
                ops.push(FileOp {
                    path,
                    kind: OpKind::Add { content },
                });
            } else {
                let hunks = parse_update_hunks(&mut lines)?;
                ops.push(FileOp {
                    path,
                    kind: OpKind::Update { hunks },
                });
            }
        } else {
            return Err(format!("unrecognized patch line: {t}"));
        }
    }

    Ok(ops)
}

fn drain_until_file_header(lines: &mut std::iter::Peekable<std::str::Lines<'_>>) {
    while let Some(l) = lines.peek() {
        let t = l.trim();
        if t.starts_with("*** ") || t.starts_with("--- ") {
            break;
        }
        lines.next();
    }
}

fn parse_add_body(lines: &mut std::iter::Peekable<std::str::Lines<'_>>) -> String {
    let mut out = String::new();
    while let Some(l) = lines.peek() {
        let t = l.trim_end();
        if t.starts_with("*** ") || t.starts_with("--- ") {
            break;
        }
        let body = lines.next().unwrap();
        let content = if let Some(rest) = body.strip_prefix('+') {
            rest
        } else if body.starts_with("@@") {
            continue;
        } else {
            body
        };
        out.push_str(content);
        out.push('\n');
    }
    out
}

fn parse_unified_add_body(lines: &mut std::iter::Peekable<std::str::Lines<'_>>) -> String {
    parse_add_body(lines)
}

fn parse_update_hunks(
    lines: &mut std::iter::Peekable<std::str::Lines<'_>>,
) -> Result<Vec<Hunk>, String> {
    let mut hunks = Vec::new();
    let mut cur_old = Vec::new();
    let mut cur_new = Vec::new();
    let mut in_hunk = false;

    let flush = |old: &mut Vec<String>, new: &mut Vec<String>, hunks: &mut Vec<Hunk>| {
        if !old.is_empty() || !new.is_empty() {
            hunks.push(Hunk {
                old_lines: std::mem::take(old),
                new_lines: std::mem::take(new),
            });
        }
    };

    while let Some(l) = lines.peek() {
        let raw = *l;
        let t = raw.trim_end();
        if t.starts_with("*** ") || t.starts_with("--- ") {
            break;
        }
        let line = lines.next().unwrap();

        if line.starts_with("@@") {
            if in_hunk {
                flush(&mut cur_old, &mut cur_new, &mut hunks);
            }
            in_hunk = true;
            continue;
        }

        // Implicit single hunk when no @@ is present.
        if !in_hunk
            && (line.starts_with(' ')
                || line.starts_with('+')
                || line.starts_with('-')
                || line == "+")
        {
            in_hunk = true;
        }

        if !in_hunk {
            if line.trim().is_empty() {
                continue;
            }
            return Err(format!("expected hunk line, got: {line}"));
        }

        if let Some(rest) = line.strip_prefix('-') {
            cur_old.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('+') {
            cur_new.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix(' ') {
            cur_old.push(rest.to_string());
            cur_new.push(rest.to_string());
        } else if line.trim().is_empty() {
            // blank context
            cur_old.push(String::new());
            cur_new.push(String::new());
        } else {
            // Treat as context without prefix.
            cur_old.push(line.to_string());
            cur_new.push(line.to_string());
        }
    }
    flush(&mut cur_old, &mut cur_new, &mut hunks);
    if hunks.is_empty() {
        return Err("update file section had no hunks".into());
    }
    Ok(hunks)
}

fn apply_hunks(before: &str, hunks: &[Hunk]) -> Result<String, String> {
    let mut content = before.to_string();
    // Normalize to \n for matching; preserve original EOL style on write.
    let uses_crlf = content.contains("\r\n");
    if uses_crlf {
        content = content.replace("\r\n", "\n");
    }

    for (i, hunk) in hunks.iter().enumerate() {
        let old_block = join_lines(&hunk.old_lines);
        let new_block = join_lines(&hunk.new_lines);
        if old_block.is_empty() && new_block.is_empty() {
            continue;
        }
        if old_block.is_empty() {
            // Pure insertion at EOF if no old context.
            if !content.ends_with('\n') && !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&new_block);
            continue;
        }
        let matches: Vec<_> = content.match_indices(&old_block).collect();
        if matches.is_empty() {
            return Err(format!(
                "hunk {} old block not found ({} lines). Re-read the file and retry.",
                i + 1,
                hunk.old_lines.len()
            ));
        }
        if matches.len() > 1 {
            return Err(format!(
                "hunk {} old block matched {} times; add more context.",
                i + 1,
                matches.len()
            ));
        }
        let (idx, _) = matches[0];
        content.replace_range(idx..idx + old_block.len(), &new_block);
    }

    if uses_crlf {
        content = content.replace('\n', "\r\n");
    }
    Ok(content)
}

fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut s = lines.join("\n");
    // Hunks from diffs usually imply a trailing newline when the last
    // changed line is a full line. Keep a trailing newline if the file
    // block was non-empty.
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionChecker;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn ctx(cwd: PathBuf) -> ToolContext {
        ToolContext {
            cwd,
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
            tool_events: None,
            active_call_id: None,
        }
    }

    #[test]
    fn parse_update_and_add() {
        let patch = "\
*** Begin Patch
*** Update File: src/a.rs
@@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
 }
*** Add File: src/b.rs
+pub fn hi() {}
*** End Patch
";
        let ops = parse_patch(patch).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0].kind, OpKind::Update { .. }));
        assert!(matches!(ops[1].kind, OpKind::Add { .. }));
    }

    #[test]
    fn apply_hunk_replaces_unique_block() {
        let before = "fn main() {\n    println!(\"old\");\n}\n";
        let hunks = vec![Hunk {
            old_lines: vec![
                "fn main() {".into(),
                "    println!(\"old\");".into(),
                "}".into(),
            ],
            new_lines: vec![
                "fn main() {".into(),
                "    println!(\"new\");".into(),
                "}".into(),
            ],
        }];
        let after = apply_hunks(before, &hunks).unwrap();
        assert!(after.contains("println!(\"new\")"));
        assert!(!after.contains("println!(\"old\")"));
    }

    #[tokio::test]
    async fn apply_patch_updates_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "hello world\n").unwrap();
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-hello world\n+hello rust\n*** End Patch\n",
            path.display()
        );
        let tool = ApplyPatchTool;
        let result = tool
            .call(json!({ "patch": patch }), &ctx(dir.path().to_path_buf()))
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.content);
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, "hello rust\n");
    }

    #[tokio::test]
    async fn apply_patch_adds_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let add_path = "new.txt";
        let del_path = dir.path().join("gone.txt");
        std::fs::write(&del_path, "x\n").unwrap();
        let patch = format!(
            "*** Begin Patch\n\
             *** Add File: {add_path}\n\
             +alpha\n\
             +beta\n\
             *** Delete File: {}\n\
             *** End Patch\n",
            del_path.display()
        );
        let tool = ApplyPatchTool;
        let result = tool
            .call(json!({ "patch": patch }), &ctx(dir.path().to_path_buf()))
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(add_path)).unwrap(),
            "alpha\nbeta\n"
        );
        assert!(!del_path.exists());
    }
}
