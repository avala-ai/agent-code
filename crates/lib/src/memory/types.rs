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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- MemoryType serde round-trip ----

    #[test]
    fn memory_type_serde_roundtrip_user() {
        let json = serde_json::to_string(&MemoryType::User).unwrap();
        assert_eq!(json, "\"user\"");
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MemoryType::User);
    }

    #[test]
    fn memory_type_serde_roundtrip_feedback() {
        let json = serde_json::to_string(&MemoryType::Feedback).unwrap();
        assert_eq!(json, "\"feedback\"");
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MemoryType::Feedback);
    }

    #[test]
    fn memory_type_serde_roundtrip_project() {
        let json = serde_json::to_string(&MemoryType::Project).unwrap();
        assert_eq!(json, "\"project\"");
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MemoryType::Project);
    }

    #[test]
    fn memory_type_serde_roundtrip_reference() {
        let json = serde_json::to_string(&MemoryType::Reference).unwrap();
        assert_eq!(json, "\"reference\"");
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MemoryType::Reference);
    }

    #[test]
    fn memory_type_rejects_unknown_variant() {
        let result = serde_json::from_str::<MemoryType>("\"unknown\"");
        assert!(result.is_err());
    }

    // ---- MemoryMeta serde round-trip ----

    #[test]
    fn memory_meta_serde_roundtrip_with_type() {
        let meta = MemoryMeta {
            name: "user prefs".into(),
            description: "editor preferences".into(),
            memory_type: Some(MemoryType::User),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: MemoryMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "user prefs");
        assert_eq!(back.description, "editor preferences");
        assert_eq!(back.memory_type, Some(MemoryType::User));
    }

    #[test]
    fn memory_meta_serde_roundtrip_without_type() {
        let meta = MemoryMeta {
            name: "misc".into(),
            description: "untyped memory".into(),
            memory_type: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: MemoryMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "misc");
        assert!(back.memory_type.is_none());
    }

    #[test]
    fn memory_meta_type_field_renamed_in_json() {
        let json = r#"{"name":"test","description":"desc","type":"feedback"}"#;
        let meta: MemoryMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.memory_type, Some(MemoryType::Feedback));
    }

    // ---- memory_age_text ----

    #[test]
    fn memory_age_text_just_now_zero() {
        assert_eq!(memory_age_text(0), "just now");
    }

    #[test]
    fn memory_age_text_just_now_59_seconds() {
        assert_eq!(memory_age_text(59), "just now");
    }

    #[test]
    fn memory_age_text_one_minute() {
        assert_eq!(memory_age_text(60), "1 minutes ago");
    }

    #[test]
    fn memory_age_text_30_minutes() {
        assert_eq!(memory_age_text(1800), "30 minutes ago");
    }

    #[test]
    fn memory_age_text_59_minutes() {
        assert_eq!(memory_age_text(3599), "59 minutes ago");
    }

    #[test]
    fn memory_age_text_one_hour() {
        assert_eq!(memory_age_text(3600), "1 hours ago");
    }

    #[test]
    fn memory_age_text_23_hours() {
        assert_eq!(memory_age_text(23 * 3600), "23 hours ago");
    }

    #[test]
    fn memory_age_text_boundary_just_under_one_day() {
        assert_eq!(memory_age_text(86399), "23 hours ago");
    }

    #[test]
    fn memory_age_text_one_day() {
        assert_eq!(memory_age_text(86400), "1 days ago");
    }

    #[test]
    fn memory_age_text_seven_days() {
        assert_eq!(memory_age_text(7 * 86400), "7 days ago");
    }

    #[test]
    fn memory_age_text_large_value() {
        assert_eq!(memory_age_text(365 * 86400), "365 days ago");
    }

    // ---- staleness_caveat ----

    #[test]
    fn staleness_caveat_none_for_zero() {
        assert!(staleness_caveat(0).is_none());
    }

    #[test]
    fn staleness_caveat_none_for_one_hour() {
        assert!(staleness_caveat(3600).is_none());
    }

    #[test]
    fn staleness_caveat_none_at_boundary() {
        assert!(staleness_caveat(86400).is_none());
    }

    #[test]
    fn staleness_caveat_some_just_over_one_day() {
        let caveat = staleness_caveat(86401);
        assert!(caveat.is_some());
        let text = caveat.unwrap();
        assert!(text.contains("1 days ago"));
        assert!(text.contains("Verify"));
    }

    #[test]
    fn staleness_caveat_some_for_seven_days() {
        let caveat = staleness_caveat(7 * 86400 + 1);
        assert!(caveat.is_some());
        let text = caveat.unwrap();
        assert!(text.contains("7 days ago"));
    }

    #[test]
    fn staleness_caveat_message_format() {
        let caveat = staleness_caveat(2 * 86400 + 1).unwrap();
        assert!(caveat.starts_with("This memory was last updated"));
        assert!(caveat.contains("reflects reality before acting on it"));
    }

    // ---- EXCLUSION_RULES ----

    #[test]
    fn exclusion_rules_is_non_empty() {
        assert!(!EXCLUSION_RULES.is_empty());
    }

    #[test]
    fn exclusion_rules_each_entry_is_non_empty() {
        for rule in EXCLUSION_RULES {
            assert!(!rule.is_empty(), "found empty exclusion rule");
        }
    }

    #[test]
    fn exclusion_rules_has_expected_count() {
        assert_eq!(EXCLUSION_RULES.len(), 5);
    }

    #[test]
    fn exclusion_rules_contains_code_patterns() {
        assert!(EXCLUSION_RULES
            .iter()
            .any(|r| r.contains("Code patterns")));
    }

    #[test]
    fn exclusion_rules_contains_git_history() {
        assert!(EXCLUSION_RULES.iter().any(|r| r.contains("Git history")));
    }
}
