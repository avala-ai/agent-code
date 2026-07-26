//! Destructive-command detection for shell pipelines.
//!
//! Walks the [`ParsedCommand`] AST and the original command string and
//! reports a [`DestructivenessLevel`] plus a list of human-readable
//! reasons. Used by the Bash tool's `validate_input` step and by tests
//! that want to assert "this command should not be executable".
//!
//! The patterns here intentionally mirror the historical inline list
//! that lived in `bash.rs` so that a refactor does not change which
//! invocations are blocked.

use crate::tools::bash_parse::{ParsedCommand, base_name, unquote_token};

/// Severity of a destructive-command finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DestructivenessLevel {
    /// No destructive markers found.
    Safe,
    /// Mildly risky, e.g. would require user confirmation in a polished
    /// UX but not an outright block. Currently unused by callers but
    /// reserved so the API can grow without breaking changes.
    Risky,
    /// Should be blocked unless the caller explicitly opted out via
    /// `dangerouslyDisableSandbox`.
    Destructive,
}

/// Reasons a command was classified as risky or destructive.
#[derive(Debug, Clone)]
pub struct DestructiveFinding {
    pub level: DestructivenessLevel,
    pub reason: String,
}

/// Substrings that mark a command as destructive when present anywhere
/// in the lower-cased command line. Order is significant only for
/// determining which reason is reported first.
pub(crate) const DESTRUCTIVE_PATTERNS: &[&str] = &[
    // Filesystem destruction.
    "rm -rf",
    "rm -r /",
    "rm -fr",
    "rmdir",
    "shred",
    // Git destructive operations.
    "git reset --hard",
    "git clean -f",
    "git clean -d",
    "git push --force",
    "git push -f",
    "git checkout -- .",
    "git checkout -f",
    "git restore .",
    "git branch -d",
    "git branch --delete --force",
    "git stash drop",
    "git stash clear",
    "git rebase --abort",
    // Database operations.
    "drop table",
    "drop database",
    "drop schema",
    "delete from",
    "truncate",
    // System operations.
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "init 0",
    "init 6",
    "mkfs",
    "dd if=",
    "dd of=/dev",
    "> /dev/sd",
    "wipefs",
    // Permission escalation.
    "chmod -r 777",
    "chmod 777",
    "chown -r",
    // Process/system danger.
    "kill -9",
    "killall",
    "pkill -9",
    // Fork bomb.
    ":(){ :|:& };:",
    // Package destruction.
    "npm publish",
    "cargo publish",
    // Container cleanup.
    "docker system prune -a",
    "docker volume prune",
    // Kubernetes.
    "kubectl delete namespace",
    "kubectl delete --all",
    // Infrastructure.
    "terraform destroy",
    "pulumi destroy",
];

/// Base commands that are destructive when they appear as a pipeline
/// segment, regardless of their arguments.
const DESTRUCTIVE_PIPELINE_BASES: &[&str] = &[
    "rm", "shred", "dd", "mkfs", "wipefs", "shutdown", "reboot", "halt",
];

/// Classify the destructiveness of a parsed command.
///
/// Both the parsed AST and the original raw string are consulted so
/// that we catch:
/// - Substring patterns that appear inside arguments (e.g. SQL).
/// - Pipeline segments whose head command is intrinsically destructive.
/// - `&&`/`;`-chained subcommands that match destructive patterns.
pub fn classify_destructive(cmd: &ParsedCommand) -> DestructivenessLevel {
    let findings = find_destructive(cmd);
    findings
        .iter()
        .map(|f| f.level)
        .max()
        .unwrap_or(DestructivenessLevel::Safe)
}

/// Like [`classify_destructive`] but returns every finding so callers
/// can present a useful message to the user.
pub fn find_destructive(cmd: &ParsedCommand) -> Vec<DestructiveFinding> {
    let mut findings = Vec::new();
    let raw_lower = cmd.raw.to_lowercase();

    // Pattern scan over the entire lowered string.
    for pattern in DESTRUCTIVE_PATTERNS {
        if raw_lower.contains(pattern) {
            findings.push(DestructiveFinding {
                level: DestructivenessLevel::Destructive,
                reason: format!("contains '{pattern}'"),
            });
        }
    }

    // Normalized scan. The raw text can be spelled to dodge a substring
    // match while running exactly the same thing: `'git' push --force`
    // and `ch\\mod 777 /etc` both reached the shell unflagged. Rebuild
    // each invocation from its tokens with the shell quoting removed and
    // scan that too. Additive — anything the raw scan catches is still
    // caught.
    for invocation in &cmd.invocations {
        let normalized = invocation
            .iter()
            .map(|t| unquote_token(t))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        for pattern in DESTRUCTIVE_PATTERNS {
            if normalized.contains(pattern) {
                findings.push(DestructiveFinding {
                    level: DestructivenessLevel::Destructive,
                    reason: format!("contains '{pattern}'"),
                });
            }
        }
    }

    // Pipeline scan: any segment whose head is intrinsically destructive
    // is flagged even if the whole-string pattern scan did not catch it.
    // The head is unquoted and de-pathed first, so `/bin/rm` and `'rm'`
    // are both recognised as `rm`.
    for segment in cmd.raw.split('|') {
        let trimmed = segment.trim();
        let head = trimmed.split_whitespace().next().unwrap_or("");
        let base_owned = base_name(&unquote_token(head)).to_lowercase();
        let base = base_owned.as_str();
        if DESTRUCTIVE_PIPELINE_BASES.contains(&base) {
            findings.push(DestructiveFinding {
                level: DestructivenessLevel::Destructive,
                reason: format!("destructive command '{base}' in pipe"),
            });
        }
    }

    // Chained subcommand scan (&& or ;).
    for segment in raw_lower.split("&&").flat_map(|s| s.split(';')) {
        let trimmed = segment.trim();
        for pattern in DESTRUCTIVE_PATTERNS {
            if trimmed.contains(pattern) {
                findings.push(DestructiveFinding {
                    level: DestructivenessLevel::Destructive,
                    reason: format!("chain segment contains '{pattern}'"),
                });
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    /// The pattern scan ran over the raw text, so re-spelling a command
    /// without changing what it runs walked past it. Both of these were
    /// accepted by `BashTool::validate_input` in every mode.
    #[test]
    fn quoting_and_escaping_cannot_hide_a_destructive_command() {
        for cmd in [
            "'git' push --force",
            "\"git\" push --force",
            "ch\\mod 777 /etc",
            "'chmod' 777 /etc",
            "'rm' -rf /tmp/x",
            "/bin/rm -rf /tmp/x",
            "'shutdown' -h now",
        ] {
            let parsed = parse_bash(cmd).expect("parses");
            assert_eq!(
                classify_destructive(&parsed),
                DestructivenessLevel::Destructive,
                "re-spelling hid a destructive command: {cmd}"
            );
        }
    }

    /// ANSI-C (`$'…'`) and locale (`$"…"`) quoting are decoded by bash
    /// before the command runs: `$'git' push --force` executes `git
    /// push --force`. `unquote_token` used to keep the `$`, so the
    /// normalized scan compared against `$git push --force` and every
    /// one of these walked past `BashTool::validate_input`.
    #[test]
    fn ansi_c_quoting_cannot_hide_a_destructive_command() {
        for cmd in [
            "$'git' push --force",
            "$\"git\" push --force",
            "$'rm' -rf /tmp/x",
            "$'r\\x6d' -rf /tmp/x",
            "$'ch'mod 777 /etc",
            "$'sh'utdown -h now",
            "env $'git' push --force",
            "nohup $'rm' -rf /tmp/x",
        ] {
            let parsed = parse_bash(cmd).expect("parses");
            assert_eq!(
                classify_destructive(&parsed),
                DestructivenessLevel::Destructive,
                "re-spelling hid a destructive command: {cmd}"
            );
        }
    }

    /// Known limitation, pre-dating this change: the scan cannot tell a
    /// command from a string that merely mentions one, so searching for a
    /// dangerous pattern is refused. Pinned here so the behaviour is
    /// documented rather than discovered, and so a later fix has a test
    /// to flip. Over-blocking, i.e. failing in the safe direction.
    #[test]
    fn a_literal_argument_mentioning_a_pattern_is_still_flagged() {
        for cmd in [
            "grep -r 'rm -rf' docs/",
            "grep 'shutdown' log.txt",
            "echo 'rm -rf /'",
        ] {
            let parsed = parse_bash(cmd).expect("parses");
            assert_eq!(
                classify_destructive(&parsed),
                DestructivenessLevel::Destructive,
                "behaviour changed for: {cmd}"
            );
        }
    }

    /// The normalization must not start flagging ordinary work — a guard
    /// that refuses everything is its own kind of failure.
    #[test]
    fn ordinary_commands_stay_safe() {
        for cmd in [
            "ls -la",
            "git status",
            "git push",
            "git commit -m 'wip'",
            "cargo build",
            "chmod 644 file.txt",
            "echo hi | grep h",
            // Legitimate ANSI-C quoting must not trip the decode.
            "echo $'hello\\nworld'",
            "grep $'\\t' notes.txt",
            "printf $'%s\\n' one two",
            "echo \"$100 reward\"",
            "echo $HOME",
        ] {
            let parsed = parse_bash(cmd).expect("parses");
            assert_eq!(
                classify_destructive(&parsed),
                DestructivenessLevel::Safe,
                "false positive on: {cmd}"
            );
        }
    }

    use super::*;
    use crate::tools::bash_parse::parse_bash;

    fn classify_str(s: &str) -> DestructivenessLevel {
        let mut p = parse_bash(s).unwrap_or_default();
        p.raw = s.to_string();
        classify_destructive(&p)
    }

    #[test]
    fn safe_commands_classify_safe() {
        assert_eq!(classify_str("ls -la"), DestructivenessLevel::Safe);
        assert_eq!(classify_str("git status"), DestructivenessLevel::Safe);
        assert_eq!(classify_str("cargo test"), DestructivenessLevel::Safe);
    }

    #[test]
    fn rm_rf_is_destructive() {
        assert_eq!(
            classify_str("rm -rf /tmp/foo"),
            DestructivenessLevel::Destructive
        );
    }

    #[test]
    fn force_push_is_destructive() {
        assert_eq!(
            classify_str("git push --force origin main"),
            DestructivenessLevel::Destructive
        );
    }

    #[test]
    fn drop_table_is_destructive() {
        assert_eq!(
            classify_str("psql -c 'DROP TABLE users'"),
            DestructivenessLevel::Destructive
        );
    }

    #[test]
    fn chained_destructive_detected() {
        assert_eq!(
            classify_str("echo ok && git reset --hard HEAD~1"),
            DestructivenessLevel::Destructive
        );
    }

    #[test]
    fn piped_rm_is_destructive() {
        assert_eq!(
            classify_str("find . -name old | rm -rf"),
            DestructivenessLevel::Destructive
        );
    }

    #[test]
    fn fork_bomb_is_destructive() {
        assert_eq!(
            classify_str(":(){ :|:& };:"),
            DestructivenessLevel::Destructive
        );
    }

    #[test]
    fn semicolon_chain_with_truncate_detected() {
        assert_eq!(
            classify_str("echo a; psql -c 'TRUNCATE users'"),
            DestructivenessLevel::Destructive
        );
    }
}
