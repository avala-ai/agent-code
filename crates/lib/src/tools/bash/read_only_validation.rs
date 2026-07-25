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

    let mut effects: Vec<Effect> = classify(cmd)
        .into_iter()
        .filter(|e| !matches!(e, Effect::ReadOnly))
        .collect();

    // Any output redirection or process substitution is mutating
    // regardless of the underlying command name. Without this
    // adjustment `echo foo > file` and `awk '{}' > out` would be
    // classified as read-only because `echo` and `awk` are on the
    // read-only list.
    if has_output_redirection(cmd) && !effects.contains(&Effect::Mutating) {
        effects.push(Effect::Mutating);
    }

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

/// Detect any output redirection (`>`, `>>`, `&>`, `2>`, heredoc-to-file
/// from `<<<`, or process substitution `>(…)`) in the parsed command.
///
/// File-descriptor duplications such as `2>&1` are NOT output
/// redirections to a path and must not be flagged here.
fn has_output_redirection(cmd: &ParsedCommand) -> bool {
    if cmd.has_process_substitution {
        return true;
    }
    contains_unquoted_output_redirect(&cmd.raw)
}

/// True if `raw` contains a `>` outside of single/double quotes that
/// is acting as an output redirection (i.e. not part of `2>&1`,
/// arrows, comparison operators, etc.).
fn contains_unquoted_output_redirect(raw: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut prev: Option<char> = None;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            prev = Some(c);
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '>' => {
                // Skip `2>&1`-style merges: `>` followed by `&` and a
                // digit is a duplication, not a file write.
                if prev == Some('-') {
                    // Heredoc bodies, arrows in conditionals (`->`),
                    // etc. — not a redirect.
                    prev = Some(c);
                    continue;
                }
                if let Some(&next) = chars.peek()
                    && next == '&'
                {
                    // `>&` (file descriptor duplication, e.g. `2>&1`).
                    // Not a write to a path.
                    chars.next();
                    prev = Some('&');
                    continue;
                }
                return true;
            }
            _ => {}
        }
        prev = Some(c);
    }
    false
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
}
