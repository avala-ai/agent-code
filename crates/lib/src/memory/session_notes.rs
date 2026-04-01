//! Per-session working notes.
//!
//! Maintains a structured markdown file for each session with
//! sections for current state, task spec, files, workflow, errors,
//! and key results. Used as a cheap compaction source — when context
//! needs shrinking, session notes serve as the summary without
//! requiring an expensive API call.

use std::path::PathBuf;

/// Session notes template sections.
const TEMPLATE: &str = "\
# Session Notes

## Current State
_What is actively being worked on right now?_

## Task
_What did the user ask to do?_

## Key Files
_Important files and their relevance_

## Workflow
_Commands run and their results_

## Errors
_Errors encountered and how they were fixed_

## Learnings
_What worked well? What didn't?_

## Results
_Key outputs or deliverables_

## Log
_Step by step, what was attempted and done_
";

/// Get the path for this session's notes file.
pub fn session_notes_path(session_id: &str) -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("agent-code").join("session-notes");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join(format!("{session_id}.md")))
}

/// Initialize session notes with the template if the file doesn't exist.
pub fn init_session_notes(session_id: &str) -> Option<PathBuf> {
    let path = session_notes_path(session_id)?;
    if !path.exists() {
        let _ = std::fs::write(&path, TEMPLATE);
    }
    Some(path)
}

/// Read existing session notes content.
pub fn read_session_notes(session_id: &str) -> Option<String> {
    let path = session_notes_path(session_id)?;
    std::fs::read_to_string(path).ok()
}

/// Check if session notes have meaningful content (not just the template).
pub fn has_content(session_id: &str) -> bool {
    match read_session_notes(session_id) {
        Some(content) => {
            // Check if any section has been filled in (non-italic content).
            content
                .lines()
                .any(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('_'))
        }
        None => false,
    }
}

/// Build a prompt for updating session notes from recent conversation.
pub fn build_update_prompt(session_id: &str, recent_messages: &str) -> String {
    let existing = read_session_notes(session_id).unwrap_or_else(|| TEMPLATE.to_string());

    format!(
        "Update the session notes below based on the recent conversation. \
         Fill in sections that apply, leave others with their placeholder text. \
         Be concise — these notes are for quick reference, not detailed logs. \
         Keep the markdown structure intact.\n\n\
         Current notes:\n```\n{existing}\n```\n\n\
         Recent conversation:\n{recent_messages}\n\n\
         Write the updated session notes (full markdown document):"
    )
}

/// Clean up old session notes (older than 7 days).
pub fn cleanup_old_notes() {
    let dir = match dirs::config_dir() {
        Some(d) => d.join("agent-code").join("session-notes"),
        None => return,
    };

    if !dir.is_dir() {
        return;
    }

    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 24 * 60 * 60);

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
