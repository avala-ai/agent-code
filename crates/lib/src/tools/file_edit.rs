//! FileEdit tool: targeted search-and-replace editing.
//!
//! Performs exact string replacement within a file. The `old_string`
//! must match uniquely (unless `replace_all` is set) to prevent
//! ambiguous edits.
//!
//! Before writing, the tool checks whether the file's modification time
//! has changed since it was last read (via the session file cache). If
//! the file is stale the edit is rejected so the model re-reads first.
//!
//! After a successful edit the tool returns a unified diff of the
//! changes so the model (and user) can see exactly what happened.

use async_trait::async_trait;
use serde_json::json;
use similar::TextDiff;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct FileEditTool;

/// Check whether the file on disk was modified after the cache recorded it.
///
/// Returns `Ok(())` when:
///   - the cache holds an entry and the on-disk mtime still matches, or
///   - there is no cache / no entry (we cannot prove staleness).
///
/// Returns an error message when the mtimes diverge.
async fn check_staleness(path: &Path, ctx: &ToolContext) -> Result<(), String> {
    let cache = match ctx.file_cache.as_ref() {
        Some(c) => c,
        None => return Ok(()),
    };

    let cached_mtime: SystemTime = {
        let guard = cache.lock().await;
        match guard.last_read_mtime(path) {
            Some(t) => t,
            None => return Ok(()), // never read through cache — nothing to compare
        }
    };

    let disk_mtime = tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|m| m.modified().ok());

    if let Some(disk) = disk_mtime
        && disk != cached_mtime
    {
        return Err(format!(
            "File was modified since last read. \
             Please re-read {} before editing.",
            path.display()
        ));
    }

    Ok(())
}

/// Produce a compact unified diff between `old` and `new` text.
///
/// The output uses `---`/`+++` headers with the file path and includes
/// only the changed hunks with a few lines of context.
fn unified_diff(file_path: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();

    // Header
    out.push_str(&format!("--- {file_path}\n"));
    out.push_str(&format!("+++ {file_path}\n"));

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&format!("{hunk}"));
    }

    // If the diff is empty (shouldn't happen because we already verified
    // old_string != new_string), fall back to a note.
    if out.lines().count() <= 2 {
        out.push_str("(no visible diff)\n");
    }

    out
}

/// The dominant line ending in `content`.
///
/// Returns `"\r\n"` when the file uses CRLF at least as often as bare LF,
/// otherwise `"\n"`. Files with no newline default to LF.
fn dominant_eol(content: &str) -> &'static str {
    let crlf = content.matches("\r\n").count();
    let bare_lf = content.matches('\n').count().saturating_sub(crlf);
    if crlf > 0 && crlf >= bare_lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Rewrite the line endings in `text` to `eol` without doubling existing CRLF.
///
/// Model-supplied strings almost always use bare LF; matching them against a
/// CRLF file would fail, and writing them into one would leave mixed endings.
/// Normalizing both the search and replacement text to the file's own ending
/// keeps edits working and the file internally consistent.
fn normalize_eol(text: &str, eol: &str) -> String {
    let lf = text.replace("\r\n", "\n");
    if eol == "\r\n" {
        lf.replace('\n', "\r\n")
    } else {
        lf
    }
}

/// Pick the search/replacement pair and its occurrence count.
///
/// Exact matches win: if the raw `old` already appears in `content`, the raw
/// strings are used verbatim so a mixed-ending file isn't disturbed and the
/// exact snippet the caller supplied is the one edited. Only when the raw
/// search finds nothing do we normalize both strings to the file's dominant
/// line ending and retry — the common "model sent LF, file is CRLF" case.
fn select_match<'a>(
    content: &str,
    old: &'a str,
    new: &'a str,
) -> (Cow<'a, str>, Cow<'a, str>, usize) {
    let raw = content.matches(old).count();
    if raw > 0 {
        return (Cow::Borrowed(old), Cow::Borrowed(new), raw);
    }
    let eol = dominant_eol(content);
    let old_n = normalize_eol(old, eol);
    let new_n = normalize_eol(new, eol);
    let occ = content.matches(old_n.as_str()).count();
    (Cow::Owned(old_n), Cow::Owned(new_n), occ)
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &'static str {
        "FileEdit"
    }

    fn description(&self) -> &'static str {
        "Performs exact string replacements in files. The old_string must \
         match uniquely unless replace_all is true."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path", "old_string", "new_string"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text (must differ from old_string)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)",
                    "default": false
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'file_path' is required".into()))?;

        let old_string = input
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'old_string' is required".into()))?;

        let new_string = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'new_string' is required".into()))?;

        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if old_string == new_string {
            return Err(ToolError::InvalidInput(
                "old_string and new_string must be different".into(),
            ));
        }

        let path = Path::new(file_path);

        // Check file size before reading (reject files > 1MB).
        const MAX_EDIT_SIZE: u64 = 1_048_576;
        if let Ok(meta) = tokio::fs::metadata(file_path).await
            && meta.len() > MAX_EDIT_SIZE
        {
            return Err(ToolError::InvalidInput(format!(
                "File too large for editing ({} bytes, max {}). \
                 Consider using Bash with sed/awk for large files.",
                meta.len(),
                MAX_EDIT_SIZE
            )));
        }

        // Staleness check: reject if the file changed since the model last
        // read it, so the model works with up-to-date content.
        if let Err(msg) = check_staleness(path, ctx).await {
            return Err(ToolError::ExecutionFailed(msg));
        }

        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read {file_path}: {e}")))?;

        // Prefer an exact match on the raw strings; only fall back to
        // line-ending normalization when the raw search finds nothing, so an
        // exact match in a mixed-ending file is never skipped (see
        // `select_match`). Any leading BOM is part of `content` and is
        // preserved untouched by the string replacement.
        let (old_cow, new_cow, occurrences) = select_match(&content, old_string, new_string);
        let old_string = old_cow.as_ref();
        let new_string = new_cow.as_ref();

        // The up-front `old_string == new_string` check ran on the raw inputs.
        // Normalizing line endings can make them equal (e.g. old `"a\r\nb"` and
        // new `"a\nb"` on an LF file), which would write an identical file yet
        // report success. Reject that no-op explicitly.
        if old_string == new_string {
            return Err(ToolError::InvalidInput(
                "old_string and new_string are identical after normalizing line \
                 endings, so the edit would make no change"
                    .into(),
            ));
        }

        if occurrences == 0 {
            return Err(ToolError::InvalidInput(format!(
                "old_string not found in {file_path}"
            )));
        }

        if occurrences > 1 && !replace_all {
            return Err(ToolError::InvalidInput(format!(
                "old_string has {occurrences} occurrences in {file_path}. \
                 Use replace_all=true to replace all, or provide a more \
                 specific old_string."
            )));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        tokio::fs::write(file_path, &new_content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write {file_path}: {e}")))?;

        // Invalidate the cache entry so the next read picks up our write.
        if let Some(cache) = ctx.file_cache.as_ref() {
            let mut guard = cache.lock().await;
            guard.invalidate(path);
        }

        // Build a unified diff so the model/user sees exactly what changed.
        let replaced = if replace_all { occurrences } else { 1 };
        let diff = unified_diff(file_path, &content, &new_content);
        Ok(ToolResult::success(format!(
            "Replaced {replaced} occurrence(s) in {file_path}\n\n{diff}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_eol_detects_crlf() {
        assert_eq!(dominant_eol("a\r\nb\r\nc"), "\r\n");
    }

    #[test]
    fn dominant_eol_detects_lf() {
        assert_eq!(dominant_eol("a\nb\nc"), "\n");
    }

    #[test]
    fn dominant_eol_defaults_lf_without_newlines() {
        assert_eq!(dominant_eol("no newline here"), "\n");
    }

    #[test]
    fn dominant_eol_picks_majority_on_mixed() {
        // Two CRLF vs one bare LF -> CRLF wins.
        assert_eq!(dominant_eol("a\r\nb\r\nc\nd"), "\r\n");
    }

    #[test]
    fn normalize_eol_lf_to_crlf_without_doubling() {
        assert_eq!(normalize_eol("x\ny", "\r\n"), "x\r\ny");
        // Already-CRLF input must not become CRCRLF.
        assert_eq!(normalize_eol("x\r\ny", "\r\n"), "x\r\ny");
    }

    #[test]
    fn normalize_eol_crlf_to_lf() {
        assert_eq!(normalize_eol("x\r\ny", "\n"), "x\ny");
    }

    #[test]
    fn normalize_eol_matches_model_lf_against_crlf_file() {
        // A model-supplied LF search string, normalized to the CRLF file's
        // ending, must be found in the file content.
        let file = "let x = 1;\r\nlet y = 2;\r\n";
        let search = normalize_eol("let x = 1;\nlet y = 2;", dominant_eol(file));
        assert!(file.contains(&search));
    }

    #[test]
    fn select_match_prefers_raw_exact_over_normalization() {
        // Mixed-ending file with an exact LF block and a logically identical
        // CRLF block, CRLF dominant. A raw LF `old` must match the LF block
        // exactly (borrowed, no normalization), not be rewritten to CRLF and
        // matched against the other block.
        let content = "a\r\nb\r\nfoo\nbar\r\n";
        let (old, new, occ) = select_match(content, "foo\nbar", "X");
        assert!(
            matches!(old, Cow::Borrowed(_)),
            "raw match must be borrowed"
        );
        assert_eq!(occ, 1);
        assert_eq!(new, "X");
    }

    #[test]
    fn select_match_normalizes_only_on_miss() {
        // No raw match (model sent LF, file is CRLF): fall back to normalized
        // matching, which finds the block.
        let content = "let x = 1;\r\nlet y = 2;\r\n";
        let (old, _new, occ) = select_match(content, "let x = 1;\nlet y = 2;", "z");
        assert!(matches!(old, Cow::Owned(_)), "fallback must be owned");
        assert_eq!(occ, 1);
    }

    #[test]
    fn strings_differing_only_by_eol_collapse_to_equal_on_lf_file() {
        // Regression guard for the no-op case: on an LF file, old `"a\r\nb"`
        // and new `"a\nb"` both normalize to `"a\nb"`, so a real edit must be
        // rejected rather than silently writing an unchanged file.
        let eol = dominant_eol("a\nb\n");
        assert_eq!(normalize_eol("a\r\nb", eol), normalize_eol("a\nb", eol));
    }
}
