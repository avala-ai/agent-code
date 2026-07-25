//! Refuse mutating bash commands when the active permission profile
//! demands read-only operation.
//!
//! Used by [`crate::tools::bash::BashTool`] when running under a
//! permission profile (e.g. plan mode) that should never let a shell
//! pipeline mutate state. The check runs after `bash_parse` has built a
//! [`ParsedCommand`] but before the command is dispatched.

use crate::config::PermissionMode;
use crate::tools::bash::command_semantics::{Effect, classify};
use crate::tools::bash_parse::ParsedCommand;

/// A coarse summary of what the active permission policy permits.
///
/// Wraps [`PermissionMode`] so callers do not have to reason about the
/// individual config-level variants. `Allow` and `AcceptEdits` map to
/// [`PermissionProfile::Permissive`]; `Plan` and `Deny` map to
/// [`PermissionProfile::ReadOnly`]; `Ask` and `Auto` map to
/// [`PermissionProfile::Prompted`] — under `Auto` a mutating command is
/// not refused outright, it is put in front of the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionProfile {
    /// Anything goes (subject to the regular destructive-pattern checks).
    Permissive,
    /// Tool calls are allowed but require user confirmation.
    Prompted,
    /// Only read-only commands may run; mutating/network/privileged
    /// commands must be refused before exec.
    ReadOnly,
}

impl From<PermissionMode> for PermissionProfile {
    fn from(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Allow | PermissionMode::AcceptEdits => PermissionProfile::Permissive,
            PermissionMode::Ask | PermissionMode::Auto => PermissionProfile::Prompted,
            PermissionMode::Deny | PermissionMode::Plan => PermissionProfile::ReadOnly,
        }
    }
}

/// Reason a command was rejected as not-read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyViolation {
    /// Effects that triggered the rejection.
    pub effects: Vec<Effect>,
    /// User-facing message.
    pub message: String,
}

/// Decide whether `cmd` may run under `profile`.
///
/// Returns `Ok(())` when the command is allowed under the profile.
/// Returns `Err` when the profile is read-only and the command would
/// mutate state, touch the network, or escalate privileges.
pub fn validate_read_only(
    cmd: &ParsedCommand,
    profile: &PermissionProfile,
) -> Result<(), ReadOnlyViolation> {
    if !matches!(profile, PermissionProfile::ReadOnly) {
        return Ok(());
    }

    // `classify` is argument-aware and already accounts for output
    // redirection, `env`-wrapped commands, mutating `git` subcommands
    // and the rest, so the read-only decision is exactly "no effect
    // other than ReadOnly survives".
    let effects: Vec<Effect> = classify(cmd)
        .into_iter()
        .filter(|e| !matches!(e, Effect::ReadOnly))
        .collect();

    if effects.is_empty() {
        return Ok(());
    }

    let names: Vec<String> = effects.iter().map(|e| format!("{e:?}")).collect();
    Err(ReadOnlyViolation {
        effects,
        message: format!(
            "Read-only profile refuses command with effects: {}. \
             Switch to a more permissive profile to run mutating commands.",
            names.join(", ")
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash_parse::parse_bash;

    fn parse(s: &str) -> ParsedCommand {
        parse_bash(s).unwrap()
    }

    #[test]
    fn permissive_profile_allows_everything() {
        let cmd = parse("rm -rf /tmp/foo");
        assert!(validate_read_only(&cmd, &PermissionProfile::Permissive).is_ok());
    }

    #[test]
    fn prompted_profile_allows_everything_at_validation_layer() {
        // The prompt happens elsewhere; this layer only blocks read-only.
        let cmd = parse("rm -rf /tmp/foo");
        assert!(validate_read_only(&cmd, &PermissionProfile::Prompted).is_ok());
    }

    #[test]
    fn read_only_profile_allows_reads() {
        let cmd = parse("ls -la");
        assert!(validate_read_only(&cmd, &PermissionProfile::ReadOnly).is_ok());
    }

    #[test]
    fn read_only_profile_blocks_writes() {
        let cmd = parse("rm foo");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_blocks_network() {
        let cmd = parse("curl https://example.com");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Network));
    }

    #[test]
    fn read_only_profile_blocks_privileged() {
        let cmd = parse("sudo ls");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Privileged));
    }

    #[test]
    fn from_permission_mode_maps_correctly() {
        assert_eq!(
            PermissionProfile::from(PermissionMode::Allow),
            PermissionProfile::Permissive
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::AcceptEdits),
            PermissionProfile::Permissive
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::Ask),
            PermissionProfile::Prompted
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::Plan),
            PermissionProfile::ReadOnly
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::Deny),
            PermissionProfile::ReadOnly
        );
    }

    #[test]
    fn read_only_profile_rejects_chained_writes() {
        let cmd = parse("ls && rm foo");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_rejects_output_redirection() {
        // `echo` is on the read-only list, but `echo foo > file` writes
        // to the filesystem and must be classified as mutating.
        let cmd = parse("echo foo > file");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_rejects_append_redirection() {
        let cmd = parse("printf x >> log.txt");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_rejects_cat_to_file() {
        let cmd = parse("cat src > dst");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_rejects_awk_to_file() {
        let cmd = parse("awk '{}' > out");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_rejects_process_substitution() {
        // `>(...)` is an output process substitution.
        let cmd = parse("tee >(cat) < input");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_allows_stderr_merge() {
        // `2>&1` is a file-descriptor duplication, not a path write.
        let cmd = parse("ls 2>&1");
        assert!(validate_read_only(&cmd, &PermissionProfile::ReadOnly).is_ok());
    }

    /// Every command here escaped the read-only profile before the
    /// classifier became argument-aware: the binary name alone
    /// (`git`, `env`, `awk`, `less`, `sort`, `ip`, `dig`, `find`, …)
    /// was enough to be judged read-only.
    #[test]
    fn read_only_profile_refuses_known_escapes() {
        for cmd in [
            // Arbitrary execution through a wrapper or a program-text
            // escape hatch.
            "env rm -rf /tmp/x",
            "env -S 'rm -rf /tmp/x'",
            "awk 'BEGIN{system(\"rm -rf /tmp/x\")}'",
            "awk '{print > \"/tmp/pwned\"}' /etc/hostname",
            "awk -f /tmp/prog.awk /etc/hostname",
            "less /etc/passwd",
            "more /etc/passwd",
            "top",
            // Arbitrary file writes.
            "sort -o /tmp/pwned /etc/hostname",
            "sort --output=/tmp/pwned /etc/hostname",
            "uniq /etc/hostname /tmp/pwned",
            "find . -delete",
            "find . -name '*.rs' -exec rm {} ;",
            "find . -fprintf /tmp/pwned %p",
            // System / network configuration.
            "ip link set eth0 down",
            "ip addr add 10.0.0.1/24 dev eth0",
            "ifconfig eth0 down",
            "hostname pwned",
            "date -s 12:00",
            "ss -K dst 1.2.3.4",
            // Network egress.
            "dig example.com",
            "host example.com",
            "nslookup example.com",
            // Destructive or remote-mutating git.
            "git push",
            "git push origin main",
            "git commit -m x",
            "git reset --hard",
            "git clean -fd",
            "git checkout main",
            "git fetch",
            "git pull",
            "git config user.email x@example.com",
            "git stash",
            "git branch -D main",
            "git tag -d v1",
            "git -c core.pager=sh status",
            "git -C /tmp status",
            "git --git-dir=/tmp/.git status",
            "git diff --ext-diff",
            "git log --textconv",
        ] {
            let parsed = parse(cmd);
            assert!(
                validate_read_only(&parsed, &PermissionProfile::ReadOnly).is_err(),
                "read-only profile must refuse: {cmd}"
            );
        }
    }

    /// Plan mode has to stay useful: these must keep working.
    #[test]
    fn read_only_profile_still_allows_inspection_commands() {
        for cmd in [
            "git status",
            "git diff",
            "git log --oneline -5",
            "git show HEAD",
            "git branch",
            "git remote -v",
            "ls -la",
            "cat f",
            "grep -r x src/",
            "rg pat",
            "find . -name '*.rs'",
            "awk '{print $1}' f",
            "sort f",
            "uniq f",
            "head -20 f",
            "tail -f f",
            "wc -l f",
            "hostname",
            "pwd",
            "echo hi",
            "stat f",
            "du -sh .",
            "diff a b",
            "cat a | grep b | wc -l",
        ] {
            let parsed = parse(cmd);
            assert!(
                validate_read_only(&parsed, &PermissionProfile::ReadOnly).is_ok(),
                "read-only profile must allow: {cmd}"
            );
        }
    }

    #[test]
    fn read_only_profile_sees_through_quote_and_backslash_evasion() {
        for cmd in [
            "find . -'delete'",
            "find . -dele\\te",
            "find . '-delete'",
            "sort --'output'=/tmp/pwned f",
            "sort --outp\\ut=/tmp/pwned f",
        ] {
            let parsed = parse(cmd);
            assert!(
                validate_read_only(&parsed, &PermissionProfile::ReadOnly).is_err(),
                "quoting must not defeat option matching: {cmd}"
            );
        }
    }

    #[test]
    fn env_wrapping_is_classified_by_the_wrapped_command() {
        // Refused: `env` runs whatever follows it.
        for cmd in [
            "env rm -rf x",
            "env git push",
            "env curl https://example.com",
        ] {
            let parsed = parse(cmd);
            assert!(
                validate_read_only(&parsed, &PermissionProfile::ReadOnly).is_err(),
                "env must be classified by the wrapped command: {cmd}"
            );
        }
        // Allowed by design: recursion resolves to a read-only command.
        for cmd in [
            "env ls",
            "env FOO=bar ls -la",
            "env -i ls",
            "env -u HOME ls",
        ] {
            let parsed = parse(cmd);
            assert!(
                validate_read_only(&parsed, &PermissionProfile::ReadOnly).is_ok(),
                "env wrapping a read-only command should stay allowed: {cmd}"
            );
        }
        // `env` with no command prints the environment; it must not panic.
        let parsed = parse("env");
        assert!(validate_read_only(&parsed, &PermissionProfile::ReadOnly).is_ok());
        let parsed = parse("env FOO=bar");
        assert!(validate_read_only(&parsed, &PermissionProfile::ReadOnly).is_ok());
    }

    #[test]
    fn deny_mode_profile_refuses_the_same_escapes() {
        // `PermissionMode::Deny` maps to the same ReadOnly profile as
        // plan mode, so the fix has to cover it too.
        let profile = PermissionProfile::from(PermissionMode::Deny);
        assert_eq!(profile, PermissionProfile::ReadOnly);
        for cmd in ["git push", "env rm -rf /tmp/x", "sort -o /tmp/pwned f"] {
            let parsed = parse(cmd);
            assert!(
                validate_read_only(&parsed, &profile).is_err(),
                "deny mode must refuse: {cmd}"
            );
        }
    }
}
