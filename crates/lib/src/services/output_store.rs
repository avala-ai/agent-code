//! Output persistence for large tool results.
//!
//! When a tool produces output larger than the inline threshold,
//! it's persisted to disk and a reference is returned instead.
//! This prevents large outputs from bloating the context window.

use std::path::{Path, PathBuf};

use crate::services::secret_masker;

/// Maximum inline output size (64KB). Larger results are persisted.
const INLINE_THRESHOLD: usize = 64 * 1024;

/// Persist large output to disk and return a summary reference.
///
/// If the content is under the threshold, returns it unchanged.
/// Otherwise, writes to the output store and returns a truncated
/// version with a file path reference.
pub fn persist_if_large(content: &str, tool_name: &str, tool_use_id: &str) -> String {
    persist_if_large_in(&output_store_dir(), content, tool_name, tool_use_id)
}

/// Variant of [`persist_if_large`] that writes into an explicit
/// directory. Used by tests to avoid touching the real cache dir.
pub(crate) fn persist_if_large_in(
    store_dir: &Path,
    content: &str,
    _tool_name: &str,
    tool_use_id: &str,
) -> String {
    if content.len() <= INLINE_THRESHOLD {
        return content.to_string();
    }

    let _ = std::fs::create_dir_all(store_dir);

    let filename = format!("{tool_use_id}.txt");
    let path = store_dir.join(&filename);

    // Mask secrets at the persistence boundary. The returned preview
    // is kept unmasked for in-memory agent use; only the on-disk copy
    // is sanitized.
    let persisted = secret_masker::mask(content);

    match std::fs::write(&path, &persisted) {
        Ok(()) => {
            let preview = &content[..INLINE_THRESHOLD.min(content.len())];
            format!(
                "{preview}\n\n(Output truncated. Full result ({} bytes) saved to {})",
                content.len(),
                path.display()
            )
        }
        Err(_) => {
            // Can't persist — truncate inline.
            let preview = &content[..INLINE_THRESHOLD.min(content.len())];
            format!(
                "{preview}\n\n(Output truncated: {} bytes total)",
                content.len()
            )
        }
    }
}

/// Read a persisted output by tool_use_id.
pub fn read_persisted(tool_use_id: &str) -> Option<String> {
    let path = output_store_dir().join(format!("{tool_use_id}.txt"));
    std::fs::read_to_string(path).ok()
}

/// Clean up old persisted outputs (older than 24 hours).
pub fn cleanup_old_outputs() {
    let dir = output_store_dir();
    if !dir.is_dir() {
        return;
    }

    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && let Ok(modified) = meta.modified()
                && modified < cutoff
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

fn output_store_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-code")
        .join("tool-results")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_if_large_passes_small_content_through_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let small = "tiny output with api_key=irrelevant_for_small_content";
        // Smaller than the threshold — returned unchanged, nothing written.
        let out = persist_if_large_in(dir.path(), small, "Bash", "tool-1");
        assert_eq!(out, small);
        assert!(
            std::fs::read_dir(dir.path())
                .map(|mut it| it.next().is_none())
                .unwrap_or(true),
            "small content should not write to disk",
        );
    }

    #[test]
    fn persist_if_large_masks_secrets_on_disk_but_not_in_preview() {
        let dir = tempfile::tempdir().unwrap();
        let aws_key = "AKIAIOSFODNN7EXAMPLE";
        // Build a payload larger than INLINE_THRESHOLD with a secret
        // embedded well before the truncation point.
        let mut content = String::with_capacity(INLINE_THRESHOLD + 1024);
        content.push_str("prefix noise ");
        content.push_str(aws_key);
        content.push_str(" more noise ");
        while content.len() <= INLINE_THRESHOLD {
            content.push_str("filler ");
        }

        let preview = persist_if_large_in(dir.path(), &content, "Bash", "tool-big");

        // Preview (in-memory return) keeps the raw secret so the agent
        // can still reason about it in the current turn.
        assert!(
            preview.contains(aws_key),
            "preview should keep raw secret for in-memory use",
        );
        assert!(preview.contains("Output truncated"));

        // On-disk copy must have the secret scrubbed.
        let disk_path = dir.path().join("tool-big.txt");
        assert!(disk_path.exists(), "persisted file not created");
        let on_disk = std::fs::read_to_string(&disk_path).unwrap();
        assert!(
            !on_disk.contains(aws_key),
            "raw secret found on disk: {on_disk}",
        );
        assert!(on_disk.contains("[REDACTED:aws_access_key]"));
    }

    #[test]
    fn persist_if_large_redacts_generic_credential_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let secret = "supersecretproductiontoken1234567890";
        let assignment = format!("DATABASE_PASSWORD={secret}");
        let mut content = assignment.clone();
        while content.len() <= INLINE_THRESHOLD {
            content.push_str(" padding padding padding padding padding ");
        }

        let _ = persist_if_large_in(dir.path(), &content, "Bash", "tool-db");

        let disk_path = dir.path().join("tool-db.txt");
        let on_disk = std::fs::read_to_string(&disk_path).unwrap();
        assert!(!on_disk.contains(secret));
        assert!(on_disk.contains("[REDACTED:credential]"));
    }
}
