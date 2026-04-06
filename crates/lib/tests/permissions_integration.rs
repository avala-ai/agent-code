//! Integration tests for the permission system.
//!
//! Tests PermissionChecker end-to-end with all permission modes,
//! protected directories, rule priority, and glob pattern matching.

use agent_code_lib::config::{PermissionMode, PermissionRule, PermissionsConfig};
use agent_code_lib::permissions::{PermissionChecker, PermissionDecision};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn json_cmd(cmd: &str) -> serde_json::Value {
    serde_json::json!({"command": cmd})
}

fn json_file(path: &str) -> serde_json::Value {
    serde_json::json!({"file_path": path})
}

fn checker_with_mode(mode: PermissionMode) -> PermissionChecker {
    PermissionChecker::from_config(&PermissionsConfig {
        default_mode: mode,
        rules: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Default mode tests
// ---------------------------------------------------------------------------

#[test]
fn allow_mode_allows_all_tool_operations() {
    let checker = checker_with_mode(PermissionMode::Allow);

    assert!(matches!(
        checker.check("Bash", &json_cmd("ls -la")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker.check("FileWrite", &json_file("src/main.rs")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker.check("FileRead", &json_file("README.md")),
        PermissionDecision::Allow
    ));
}

#[test]
fn deny_mode_denies_all_operations() {
    let checker = checker_with_mode(PermissionMode::Deny);

    assert!(matches!(
        checker.check("Bash", &json_cmd("echo hello")),
        PermissionDecision::Deny(_)
    ));
    assert!(matches!(
        checker.check("FileWrite", &json_file("src/lib.rs")),
        PermissionDecision::Deny(_)
    ));
    assert!(matches!(
        checker.check("FileRead", &json_file("Cargo.toml")),
        PermissionDecision::Deny(_)
    ));
}

#[test]
fn ask_mode_returns_ask_decisions() {
    let checker = checker_with_mode(PermissionMode::Ask);

    assert!(matches!(
        checker.check("Bash", &json_cmd("cargo build")),
        PermissionDecision::Ask(_)
    ));
    assert!(matches!(
        checker.check("FileWrite", &json_file("src/lib.rs")),
        PermissionDecision::Ask(_)
    ));
}

#[test]
fn plan_mode_denies_write_tools() {
    let checker = checker_with_mode(PermissionMode::Plan);

    // Write tools denied.
    assert!(matches!(
        checker.check("FileWrite", &json_file("src/lib.rs")),
        PermissionDecision::Deny(_)
    ));
    assert!(matches!(
        checker.check("FileEdit", &json_file("src/main.rs")),
        PermissionDecision::Deny(_)
    ));
    // Bash is also denied in plan mode (it can mutate).
    assert!(matches!(
        checker.check("Bash", &json_cmd("rm -rf /")),
        PermissionDecision::Deny(_)
    ));
}

// ---------------------------------------------------------------------------
// Protected directories
// ---------------------------------------------------------------------------

#[test]
fn protected_directories_always_denied_for_writes() {
    // Even with allow-all mode, writing to protected dirs is blocked.
    let checker = checker_with_mode(PermissionMode::Allow);

    // .git/
    assert!(matches!(
        checker.check("FileWrite", &json_file(".git/config")),
        PermissionDecision::Deny(_)
    ));
    assert!(matches!(
        checker.check("FileEdit", &json_file("repo/.git/HEAD")),
        PermissionDecision::Deny(_)
    ));

    // .husky/
    assert!(matches!(
        checker.check("FileWrite", &json_file(".husky/pre-commit")),
        PermissionDecision::Deny(_)
    ));

    // node_modules/
    assert!(matches!(
        checker.check("FileEdit", &json_file("node_modules/pkg/index.js")),
        PermissionDecision::Deny(_)
    ));
    assert!(matches!(
        checker.check("NotebookEdit", &json_file("project/node_modules/a.ipynb")),
        PermissionDecision::Deny(_)
    ));
}

#[test]
fn protected_directories_always_allowed_for_reads() {
    // Reading protected dirs should succeed regardless of mode.
    let checker = checker_with_mode(PermissionMode::Ask);

    assert!(matches!(
        checker.check("FileRead", &json_file(".git/config")),
        PermissionDecision::Ask(_)
    ));
    // FileRead is not a write tool, so protected dir check does not apply.
    // It falls through to default mode (Ask), not Deny.

    let checker_allow = checker_with_mode(PermissionMode::Allow);
    assert!(matches!(
        checker_allow.check("FileRead", &json_file(".git/HEAD")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker_allow.check("FileRead", &json_file("node_modules/pkg/lib.js")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker_allow.check("FileRead", &json_file(".husky/pre-commit")),
        PermissionDecision::Allow
    ));

    // check_read should always allow reads to protected dirs.
    let checker_deny = checker_with_mode(PermissionMode::Deny);
    assert!(matches!(
        checker_deny.check_read("FileRead", &json_file(".git/config")),
        PermissionDecision::Allow
    ));
}

// ---------------------------------------------------------------------------
// Rule priority: specific rules override default mode
// ---------------------------------------------------------------------------

#[test]
fn specific_rule_overrides_default_mode() {
    // Default mode is Deny, but a specific rule allows git commands.
    let checker = PermissionChecker::from_config(&PermissionsConfig {
        default_mode: PermissionMode::Deny,
        rules: vec![PermissionRule {
            tool: "Bash".into(),
            pattern: Some("git *".into()),
            action: PermissionMode::Allow,
        }],
    });

    // Git commands allowed by rule.
    assert!(matches!(
        checker.check("Bash", &json_cmd("git status")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker.check("Bash", &json_cmd("git push origin main")),
        PermissionDecision::Allow
    ));

    // Non-git commands fall back to Deny.
    assert!(matches!(
        checker.check("Bash", &json_cmd("ls -la")),
        PermissionDecision::Deny(_)
    ));
    // Other tools also fall back to Deny.
    assert!(matches!(
        checker.check("FileWrite", &json_file("src/lib.rs")),
        PermissionDecision::Deny(_)
    ));
}

// ---------------------------------------------------------------------------
// Glob pattern matching in rules
// ---------------------------------------------------------------------------

#[test]
fn glob_pattern_matching_git_star() {
    let checker = PermissionChecker::from_config(&PermissionsConfig {
        default_mode: PermissionMode::Deny,
        rules: vec![PermissionRule {
            tool: "Bash".into(),
            pattern: Some("git *".into()),
            action: PermissionMode::Allow,
        }],
    });

    assert!(matches!(
        checker.check("Bash", &json_cmd("git log --oneline")),
        PermissionDecision::Allow
    ));
    // "git" alone (no space + rest) should not match "git *".
    assert!(matches!(
        checker.check("Bash", &json_cmd("git")),
        PermissionDecision::Deny(_)
    ));
}

#[test]
fn glob_pattern_matching_rs_files() {
    let checker = PermissionChecker::from_config(&PermissionsConfig {
        default_mode: PermissionMode::Deny,
        rules: vec![PermissionRule {
            tool: "FileWrite".into(),
            pattern: Some("*.rs".into()),
            action: PermissionMode::Allow,
        }],
    });

    assert!(matches!(
        checker.check("FileWrite", &json_file("main.rs")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker.check("FileWrite", &json_file("main.py")),
        PermissionDecision::Deny(_)
    ));
}

// ---------------------------------------------------------------------------
// Multiple rules: first match wins
// ---------------------------------------------------------------------------

#[test]
fn first_matching_rule_wins() {
    let checker = PermissionChecker::from_config(&PermissionsConfig {
        default_mode: PermissionMode::Ask,
        rules: vec![
            // Rule 0: allow git commands.
            PermissionRule {
                tool: "Bash".into(),
                pattern: Some("git *".into()),
                action: PermissionMode::Allow,
            },
            // Rule 1: deny ALL bash. This comes after, so git * still allowed.
            PermissionRule {
                tool: "Bash".into(),
                pattern: None,
                action: PermissionMode::Deny,
            },
        ],
    });

    // git commands match rule 0 first -> Allow.
    assert!(matches!(
        checker.check("Bash", &json_cmd("git diff")),
        PermissionDecision::Allow
    ));
    // Non-git bash commands match rule 1 -> Deny.
    assert!(matches!(
        checker.check("Bash", &json_cmd("ls")),
        PermissionDecision::Deny(_)
    ));
    // FileWrite has no matching rule -> falls to default Ask.
    assert!(matches!(
        checker.check("FileWrite", &json_file("src/lib.rs")),
        PermissionDecision::Ask(_)
    ));
}

// ---------------------------------------------------------------------------
// Mixed rules with different tools
// ---------------------------------------------------------------------------

#[test]
fn mixed_rules_different_tools() {
    let checker = PermissionChecker::from_config(&PermissionsConfig {
        default_mode: PermissionMode::Ask,
        rules: vec![
            // Allow Bash for git commands.
            PermissionRule {
                tool: "Bash".into(),
                pattern: Some("git *".into()),
                action: PermissionMode::Allow,
            },
            // Deny FileWrite to node_modules (redundant with protected dirs,
            // but tests that rule matching works for FileWrite too).
            PermissionRule {
                tool: "FileWrite".into(),
                pattern: Some("*node_modules*".into()),
                action: PermissionMode::Deny,
            },
            // Allow FileWrite to *.rs files.
            PermissionRule {
                tool: "FileWrite".into(),
                pattern: Some("*.rs".into()),
                action: PermissionMode::Allow,
            },
        ],
    });

    // Bash + git -> Allow.
    assert!(matches!(
        checker.check("Bash", &json_cmd("git status")),
        PermissionDecision::Allow
    ));
    // Bash + non-git -> Ask (default).
    assert!(matches!(
        checker.check("Bash", &json_cmd("cargo test")),
        PermissionDecision::Ask(_)
    ));

    // FileWrite to .rs -> Allow (rule index 2).
    assert!(matches!(
        checker.check("FileWrite", &json_file("lib.rs")),
        PermissionDecision::Allow
    ));
    // FileWrite to .py -> Ask (no matching rule, default).
    assert!(matches!(
        checker.check("FileWrite", &json_file("script.py")),
        PermissionDecision::Ask(_)
    ));
}

// ---------------------------------------------------------------------------
// Wildcard tool rules
// ---------------------------------------------------------------------------

#[test]
fn wildcard_tool_rule_matches_all_tools() {
    let checker = PermissionChecker::from_config(&PermissionsConfig {
        default_mode: PermissionMode::Deny,
        rules: vec![PermissionRule {
            tool: "*".into(),
            pattern: None,
            action: PermissionMode::Allow,
        }],
    });

    assert!(matches!(
        checker.check("Bash", &json_cmd("ls")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker.check("FileRead", &json_file("a.txt")),
        PermissionDecision::Allow
    ));
    // Protected dirs still blocked for writes even with wildcard allow.
    assert!(matches!(
        checker.check("FileWrite", &json_file(".git/config")),
        PermissionDecision::Deny(_)
    ));
}

// ---------------------------------------------------------------------------
// AcceptEdits mode
// ---------------------------------------------------------------------------

#[test]
fn accept_edits_mode_allows_operations() {
    let checker = checker_with_mode(PermissionMode::AcceptEdits);

    assert!(matches!(
        checker.check("FileWrite", &json_file("src/lib.rs")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker.check("FileEdit", &json_file("src/main.rs")),
        PermissionDecision::Allow
    ));
    assert!(matches!(
        checker.check("Bash", &json_cmd("cargo build")),
        PermissionDecision::Allow
    ));
}
