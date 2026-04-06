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
    /// Evaluates in order: protected paths, explicit rules, default mode.
    /// The first match wins.
    pub fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Block writes to protected directories regardless of rules.
        if is_write_tool(tool_name)
            && let Some(reason) = check_protected_path(input)
        {
            return PermissionDecision::Deny(reason);
        }

        // Check explicit rules.
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

/// Directories that should never be written to by the agent.
const PROTECTED_DIRS: &[&str] = &[
    ".git/",
    ".git\\",
    ".husky/",
    ".husky\\",
    "node_modules/",
    "node_modules\\",
];

/// Returns true for tools that modify the filesystem.
fn is_write_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "FileWrite" | "FileEdit" | "MultiEdit" | "NotebookEdit"
    )
}

/// Check if the input targets a protected path. Returns the denial reason if so.
fn check_protected_path(input: &serde_json::Value) -> Option<String> {
    let path = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    for dir in PROTECTED_DIRS {
        if path.contains(dir) {
            let dir_name = dir.trim_end_matches(['/', '\\']);
            return Some(format!(
                "Write to {dir_name}/ is blocked. This is a protected directory."
            ));
        }
    }
    None
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
    fn test_protected_dirs_block_writes() {
        let checker = PermissionChecker::allow_all();

        // Writing to .git/ should be denied even with allow_all.
        assert!(matches!(
            checker.check(
                "FileWrite",
                &serde_json::json!({"file_path": ".git/config"})
            ),
            PermissionDecision::Deny(_)
        ));

        // Writing to node_modules/ should be denied.
        assert!(matches!(
            checker.check(
                "FileEdit",
                &serde_json::json!({"file_path": "node_modules/foo/index.js"})
            ),
            PermissionDecision::Deny(_)
        ));

        // Writing to .husky/ should be denied.
        assert!(matches!(
            checker.check(
                "FileWrite",
                &serde_json::json!({"file_path": ".husky/pre-commit"})
            ),
            PermissionDecision::Deny(_)
        ));

        // Reading .git/ should still be allowed.
        assert!(matches!(
            checker.check("FileRead", &serde_json::json!({"file_path": ".git/config"})),
            PermissionDecision::Allow
        ));

        // Writing to normal paths should still work.
        assert!(matches!(
            checker.check(
                "FileWrite",
                &serde_json::json!({"file_path": "src/main.rs"})
            ),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_protected_dirs_helper() {
        assert!(check_protected_path(&serde_json::json!({"file_path": ".git/HEAD"})).is_some());
        assert!(
            check_protected_path(&serde_json::json!({"file_path": "node_modules/pkg/lib.js"}))
                .is_some()
        );
        assert!(check_protected_path(&serde_json::json!({"file_path": "src/lib.rs"})).is_none());
        assert!(check_protected_path(&serde_json::json!({"command": "ls"})).is_none());
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

    #[test]
    fn test_deny_mode_blocks_all_tools() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![],
        });
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Deny(_)
        ));
        assert!(matches!(
            checker.check("FileWrite", &serde_json::json!({"file_path": "src/main.rs"})),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn test_plan_mode_blocks_all_tools() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Plan,
            rules: vec![],
        });
        let decision = checker.check("Bash", &serde_json::json!({"command": "ls"}));
        assert!(matches!(decision, PermissionDecision::Deny(_)));
        if let PermissionDecision::Deny(msg) = decision {
            assert!(msg.contains("Plan mode"));
        }
    }

    #[test]
    fn test_accept_edits_mode_allows_writes() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::AcceptEdits,
            rules: vec![],
        });
        // Write to a non-protected path should be allowed.
        assert!(matches!(
            checker.check("FileWrite", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_wildcard_tool_rule_matches_any_tool() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![PermissionRule {
                tool: "*".into(),
                pattern: None,
                action: PermissionMode::Allow,
            }],
        });
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            checker.check("FileRead", &serde_json::json!({"file_path": "foo.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_check_read_allows_reads_with_deny_default() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![],
        });
        // check_read should allow even when default mode is Deny (no explicit deny rule).
        assert!(matches!(
            checker.check_read("FileRead", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_check_read_blocks_with_explicit_deny_rule() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Allow,
            rules: vec![PermissionRule {
                tool: "FileRead".into(),
                pattern: Some("*.secret".into()),
                action: PermissionMode::Deny,
            }],
        });
        assert!(matches!(
            checker.check_read("FileRead", &serde_json::json!({"file_path": "keys.secret"})),
            PermissionDecision::Deny(_)
        ));
        // Non-matching pattern should still allow.
        assert!(matches!(
            checker.check_read("FileRead", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_matches_input_pattern_with_file_path() {
        let input = serde_json::json!({"file_path": "src/main.rs"});
        assert!(matches_input_pattern("src/*", &input));
        assert!(!matches_input_pattern("test/*", &input));
    }

    #[test]
    fn test_matches_input_pattern_with_pattern_field() {
        let input = serde_json::json!({"pattern": "TODO"});
        assert!(matches_input_pattern("TODO", &input));
        assert!(!matches_input_pattern("FIXME", &input));
    }

    #[test]
    fn test_is_write_tool_classification() {
        assert!(is_write_tool("FileWrite"));
        assert!(is_write_tool("FileEdit"));
        assert!(is_write_tool("MultiEdit"));
        assert!(is_write_tool("NotebookEdit"));
        assert!(!is_write_tool("FileRead"));
        assert!(!is_write_tool("Bash"));
        assert!(!is_write_tool("Grep"));
    }

    #[test]
    fn test_protected_path_windows_backslash() {
        assert!(
            check_protected_path(&serde_json::json!({"file_path": "repo\\.git\\config"}))
                .is_some()
        );
    }

    #[test]
    fn test_protected_path_nested_git_objects() {
        assert!(
            check_protected_path(&serde_json::json!({"file_path": "some/path/.git/objects/foo"}))
                .is_some()
        );
    }
}
