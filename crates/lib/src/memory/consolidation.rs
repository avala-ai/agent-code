//! Memory consolidation ("dreaming").
//!
//! Background process that reviews memory files and consolidates
//! them: merging duplicates, resolving contradictions, converting
//! relative dates to absolute, pruning stale entries, and keeping
//! the index under limits.
//!
//! Uses a lock file to prevent concurrent consolidation across
//! multiple agent sessions.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

/// Minimum hours between consolidation runs.
const MIN_HOURS_BETWEEN_RUNS: u64 = 24;

/// Lock file name within the memory directory.
const LOCK_FILE: &str = ".consolidate-lock";

/// Check if consolidation should run.
pub fn should_consolidate(memory_dir: &Path) -> bool {
    let lock_path = memory_dir.join(LOCK_FILE);

    // If lock doesn't exist, we've never consolidated.
    let modified = match std::fs::metadata(&lock_path)
        .ok()
        .and_then(|m| m.modified().ok())
    {
        Some(t) => t,
        None => return true, // Never run before.
    };

    let elapsed = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);

    elapsed.as_secs() >= MIN_HOURS_BETWEEN_RUNS * 3600
}

/// Try to acquire the consolidation lock.
/// Returns the lock path if acquired, None if another process holds it.
pub fn try_acquire_lock(memory_dir: &Path) -> Option<PathBuf> {
    let lock_path = memory_dir.join(LOCK_FILE);

    // Check for existing lock.
    if lock_path.exists()
        && let Ok(content) = std::fs::read_to_string(&lock_path)
    {
        let pid_str = content.trim();
        if let Ok(pid) = pid_str.parse::<u32>() {
            // Check if the holding process is still alive.
            if is_process_alive(pid) {
                // Check if lock is stale (> 1 hour).
                if let Ok(meta) = std::fs::metadata(&lock_path)
                    && let Ok(modified) = meta.modified()
                {
                    let age = SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or(Duration::ZERO);
                    if age.as_secs() < 3600 {
                        return None; // Lock is fresh and holder is alive.
                    }
                }
            }
            // Process is dead or lock is stale — reclaim.
        }
    }

    // Write our PID to the lock file.
    let pid = std::process::id();
    if std::fs::write(&lock_path, pid.to_string()).is_err() {
        return None;
    }

    // Verify we actually hold the lock (race protection).
    if let Ok(content) = std::fs::read_to_string(&lock_path)
        && content.trim() == pid.to_string()
    {
        return Some(lock_path);
    }

    None // Lost the race.
}

/// Release the consolidation lock by updating its mtime to now.
/// This marks the consolidation as complete (mtime = last consolidated time).
pub fn release_lock(lock_path: &Path) {
    // Rewrite the file to update mtime to now.
    let _ = std::fs::write(lock_path, std::process::id().to_string());
}

/// Roll back the lock on failure (rewind mtime so next session retries).
pub fn rollback_lock(lock_path: &Path) {
    // Delete the lock file so next check sees "never consolidated".
    let _ = std::fs::remove_file(lock_path);
}

/// Build the consolidation prompt for the dream agent.
pub fn build_consolidation_prompt(memory_dir: &Path) -> String {
    let mut prompt = String::from(
        "You are a memory consolidation agent. Review and improve the memory \
         directory. Work in four phases:\n\n\
         Phase 1 — Orient:\n\
         - List the memory directory contents\n\
         - Read MEMORY.md to understand the current index\n\
         - Skim existing files to avoid creating duplicates\n\n\
         Phase 2 — Identify issues:\n\
         - Find duplicate or near-duplicate memories\n\
         - Find contradictions between memory files\n\
         - Find memories with relative dates (convert to absolute)\n\
         - Find memories about things derivable from code (delete these)\n\n\
         Phase 3 — Consolidate:\n\
         - Merge duplicates into single files\n\
         - Delete contradicted facts at the source\n\
         - Update vague descriptions with specific ones\n\
         - Remove memories about code patterns, git history, or debugging\n\n\
         Phase 4 — Prune and index:\n\
         - Update MEMORY.md to stay under 200 lines\n\
         - Remove pointers to deleted files\n\
         - Shorten verbose index entries (detail belongs in topic files)\n\
         - Resolve contradictions between index and files\n\n\
         Be aggressive about pruning. Less memory is better than stale memory.\n",
    );

    prompt.push_str(&format!("\nMemory directory: {}\n", memory_dir.display()));

    prompt
}

/// Run the full consolidation pipeline via LLM.
pub async fn run_consolidation(
    memory_dir: &Path,
    lock_path: &Path,
    llm: Arc<dyn crate::llm::provider::Provider>,
    model: &str,
) {
    let prompt = build_consolidation_prompt(memory_dir);

    // Build a manifest of all current memory files.
    let manifest = super::extraction::build_memory_manifest_public(memory_dir);
    let full_prompt = format!(
        "{prompt}\n\n{manifest}\n\n\
         Analyze these memory files. For each action you want to take, output a JSON \
         line with one of these formats:\n\
         To delete a file: {{\"action\": \"delete\", \"filename\": \"file.md\"}}\n\
         To update a file: {{\"action\": \"update\", \"filename\": \"file.md\", \
         \"name\": \"Name\", \"description\": \"desc\", \"type\": \"user\", \
         \"content\": \"new content\"}}\n\
         To update the index: {{\"action\": \"reindex\"}}\n\n\
         Output ONLY JSON lines, nothing else. If no changes needed, output nothing."
    );

    let request = crate::llm::provider::ProviderRequest {
        messages: vec![crate::llm::message::user_message(&full_prompt)],
        system_prompt: "You are a memory consolidation agent. You merge, prune, and \
                        organize memory files. Be aggressive about removing stale or \
                        duplicate content. Output only JSON lines."
            .to_string(),
        tools: vec![],
        model: model.to_string(),
        max_tokens: 4096,
        temperature: Some(0.0),
        enable_caching: false,
        tool_choice: Default::default(),
        metadata: None,
        // Background consolidation: not user-cancellable, passes a fresh token.
        cancel: tokio_util::sync::CancellationToken::new(),
    };

    let result = match llm.stream(&request).await {
        Ok(mut rx) => {
            let mut output = String::new();
            while let Some(event) = rx.recv().await {
                if let crate::llm::stream::StreamEvent::TextDelta(text) = event {
                    output.push_str(&text);
                }
            }
            output
        }
        Err(e) => {
            tracing::debug!("Memory consolidation skipped (API error): {e}");
            rollback_lock(lock_path);
            return;
        }
    };

    // Process actions.
    let mut actions_taken = 0;
    for line in result.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            let action = entry.get("action").and_then(|v| v.as_str()).unwrap_or("");

            match action {
                "delete" => {
                    if let Some(filename) = entry.get("filename").and_then(|v| v.as_str()) {
                        let path = memory_dir.join(filename);
                        if path.exists() {
                            if let Err(e) = std::fs::remove_file(&path) {
                                warn!("Failed to delete memory file {filename}: {e}");
                            } else {
                                info!("Consolidation: deleted {filename}");
                                actions_taken += 1;
                            }
                        }
                    }
                }
                "update" => {
                    let filename = entry
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown.md");
                    let name = entry
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let description = entry
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let mem_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("user");
                    let content = entry.get("content").and_then(|v| v.as_str()).unwrap_or("");

                    if !content.is_empty() {
                        let memory_type = match mem_type {
                            "feedback" => Some(super::types::MemoryType::Feedback),
                            "project" => Some(super::types::MemoryType::Project),
                            "reference" => Some(super::types::MemoryType::Reference),
                            _ => Some(super::types::MemoryType::User),
                        };

                        let meta = super::types::MemoryMeta {
                            name: name.to_string(),
                            description: description.to_string(),
                            memory_type,
                        };

                        match super::writer::write_memory(memory_dir, filename, &meta, content) {
                            Ok(_) => {
                                info!("Consolidation: updated {filename}");
                                actions_taken += 1;
                            }
                            Err(e) => {
                                warn!("Failed to update memory file {filename}: {e}");
                            }
                        }
                    }
                }
                "reindex" => {
                    // Rebuild the index from existing files.
                    if let Err(e) = super::writer::rebuild_index(memory_dir) {
                        warn!("Failed to rebuild memory index: {e}");
                    } else {
                        info!("Consolidation: reindexed MEMORY.md");
                        actions_taken += 1;
                    }
                }
                _ => {}
            }
        }
    }

    if actions_taken > 0 {
        info!("Memory consolidation complete: {actions_taken} actions taken");
    } else {
        info!("Memory consolidation: no changes needed");
    }

    release_lock(lock_path);
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks if process exists without sending a signal.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid; // Suppress unused variable warning on non-Unix.
        true // Assume alive on non-Unix.
    }
}
