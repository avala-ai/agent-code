//! Permission system.
//!
//! Controls which tool operations are allowed. Checks are run
//! before every tool execution. The system supports three modes:
//!
//! - `Allow` — execute without asking
//! - `Deny` — block with a reason
//! - `Ask` — prompt the user interactively
//!
//! Rules can be configured per-tool and per-pattern (e.g., allow
//! `Bash` for `git *` commands, deny `FileWrite` outside the project).

pub mod tracking;

use crate::config::{PermissionMode, PermissionRule, PermissionsConfig};

/// Decision from a permission check.
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// Tool execution is allowed.
    Allow,
    /// Tool execution is denied with a reason.
    Deny(String),
    /// User should be prompted with this message.
    Ask(String),
}

/// Checks permissions for tool operations based on configured rules.
pub struct PermissionChecker {
    default_mode: PermissionMode,
    rules: Vec<PermissionRule>,
}

impl PermissionChecker {
    /// Create from configuration.
    pub fn from_config(config: &PermissionsConfig) -> Self {
        Self {
            default_mode: config.default_mode,
            rules: config.rules.clone(),
        }
    }

    /// Create a checker that allows everything (for testing or bypass mode).
    pub fn allow_all() -> Self {
        Self {
            default_mode: PermissionMode::Allow,
            rules: Vec::new(),
        }
    }

    /// Check whether a tool operation is permitted.
    ///
    /// Evaluates rules in order. The first matching rule wins.
    /// If no rule matches, the default mode is applied.
    pub fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Check explicit rules first.
        for rule in &self.rules {
            if !matches_tool(&rule.tool, tool_name) {
                continue;
            }

            if let Some(ref pattern) = rule.pattern
                && !matches_input_pattern(pattern, input)
            {
                continue;
            }

            return mode_to_decision(rule.action, tool_name);
        }

        // Fall back to default mode.
        mode_to_decision(self.default_mode, tool_name)
    }

    /// Check for read-only operations (always allowed).
    pub fn check_read(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Read operations use a relaxed check — only explicit deny rules block.
        for rule in &self.rules {
            if !matches_tool(&rule.tool, tool_name) {
                continue;
            }
            if let Some(ref pattern) = rule.pattern
                && !matches_input_pattern(pattern, input)
            {
                continue;
            }
            if matches!(rule.action, PermissionMode::Deny) {
                return PermissionDecision::Deny(format!("Denied by rule for {tool_name}"));
            }
        }
        PermissionDecision::Allow
    }
}

fn matches_tool(rule_tool: &str, tool_name: &str) -> bool {
    rule_tool == "*" || rule_tool.eq_ignore_ascii_case(tool_name)
}

fn matches_input_pattern(pattern: &str, input: &serde_json::Value) -> bool {
    // Match against common input fields: command, file_path, pattern.
    let input_str = input
        .get("command")
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("pattern"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    glob_match(pattern, input_str)
}

/// Simple glob matching (supports `*` and `?`).
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    glob_match_inner(&pattern_chars, &text_chars)
}

fn glob_match_inner(pattern: &[char], text: &[char]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // '*' matches zero or more characters.
            glob_match_inner(&pattern[1..], text)
                || (!text.is_empty() && glob_match_inner(pattern, &text[1..]))
        }
        (Some('?'), Some(_)) => glob_match_inner(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p == t => glob_match_inner(&pattern[1..], &text[1..]),
        _ => false,
    }
}

fn mode_to_decision(mode: PermissionMode, tool_name: &str) -> PermissionDecision {
    match mode {
        PermissionMode::Allow | PermissionMode::AcceptEdits => PermissionDecision::Allow,
        PermissionMode::Deny => {
            PermissionDecision::Deny(format!("Default mode denies {tool_name}"))
        }
        PermissionMode::Ask => PermissionDecision::Ask(format!("Allow {tool_name} to execute?")),
        PermissionMode::Plan => {
            PermissionDecision::Deny("Plan mode: only read-only operations allowed".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("git *", "git status"));
        assert!(glob_match("git *", "git push --force"));
        assert!(!glob_match("git *", "rm -rf /"));
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("??", "ab"));
        assert!(!glob_match("??", "abc"));
    }

    #[test]
    fn test_allow_all() {
        let checker = PermissionChecker::allow_all();
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_rule_matching() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Ask,
            rules: vec![
                PermissionRule {
                    tool: "Bash".into(),
                    pattern: Some("git *".into()),
                    action: PermissionMode::Allow,
                },
                PermissionRule {
                    tool: "Bash".into(),
                    pattern: Some("rm *".into()),
                    action: PermissionMode::Deny,
                },
            ],
        });

        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "git status"})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "rm -rf /"})),
            PermissionDecision::Deny(_)
        ));
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Ask(_)
        ));
    }
}
