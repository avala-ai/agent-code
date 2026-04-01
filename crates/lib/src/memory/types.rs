//! Memory type system and frontmatter schema.
//!
//! Memories are categorized into four types, each with specific
//! save criteria and staleness characteristics.

use serde::{Deserialize, Serialize};

/// Memory types — closed set, validated at parse time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// User profile: role, preferences, knowledge.
    User,
    /// Guidance: what to do/avoid, validated approaches.
    Feedback,
    /// Project context: deadlines, decisions, incidents.
    Project,
    /// Pointers to external systems (Linear, Grafana, Slack).
    Reference,
}

/// Parsed frontmatter from a memory file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMeta {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub memory_type: Option<MemoryType>,
}

/// What should NOT be stored as memory.
/// These are derivable from the codebase and storing them
/// creates stale/contradictory state.
pub const EXCLUSION_RULES: &[&str] = &[
    "Code patterns, conventions, architecture, file paths — derivable from code",
    "Git history, recent changes — use git log / git blame",
    "Debugging solutions — the fix is in the code, commit message has context",
    "Anything already in project AGENTS.md",
    "Ephemeral task details or current conversation context",
];

/// Calculate human-readable age for a memory file.
pub fn memory_age_text(modified_secs_ago: u64) -> String {
    if modified_secs_ago < 60 {
        "just now".to_string()
    } else if modified_secs_ago < 3600 {
        format!("{} minutes ago", modified_secs_ago / 60)
    } else if modified_secs_ago < 86400 {
        format!("{} hours ago", modified_secs_ago / 3600)
    } else {
        format!("{} days ago", modified_secs_ago / 86400)
    }
}

/// Generate a staleness warning if the memory is older than 1 day.
pub fn staleness_caveat(modified_secs_ago: u64) -> Option<String> {
    if modified_secs_ago > 86400 {
        Some(format!(
            "This memory was last updated {}. Verify it still \
             reflects reality before acting on it.",
            memory_age_text(modified_secs_ago)
        ))
    } else {
        None
    }
}
