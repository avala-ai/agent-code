//! Output persistence for large tool results.
//!
//! When a tool produces output larger than the inline threshold,
//! it's persisted to disk and a reference is returned instead.
//! This prevents large outputs from bloating the context window.

use std::path::PathBuf;

/// Maximum inline output size (64KB). Larger results are persisted.
const INLINE_THRESHOLD: usize = 64 * 1024;

/// Persist large output to disk and return a summary reference.
///
/// If the content is under the threshold, returns it unchanged.
/// Otherwise, writes to the output store and returns a truncated
/// version with a file path reference.
pub fn persist_if_large(content: &str, _tool_name: &str, tool_use_id: &str) -> String {
    if content.len() <= INLINE_THRESHOLD {
        return content.to_string();
    }

    let store_dir = output_store_dir();
    let _ = std::fs::create_dir_all(&store_dir);

    let filename = format!("{tool_use_id}.txt");
    let path = store_dir.join(&filename);

    match std::fs::write(&path, content) {
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
