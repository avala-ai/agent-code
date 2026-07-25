//! Shell-command classification for [`PermissionMode::Auto`].
//!
//! Auto mode auto-approves the narrow set of shell invocations that can be
//! *positively proven* read-only, and prompts for everything else. The gate
//! is deliberately closed by default: a command is approved only when every
//! one of the following holds.
//!
//! 1. The command parses cleanly (no tree-sitter error/missing nodes).
//! 2. It contains no command substitution, subshell, process substitution,
//!    redirection, variable expansion or variable assignment — those make the
//!    effective command line unanalyzable at gate time.
//! 3. [`check_parsed_security`] reports no violations and
//!    [`classify_destructive`] reports [`DestructivenessLevel::Safe`].
//! 4. [`classify`] yields a non-empty effect set whose every member is
//!    [`Effect::ReadOnly`].
//! 5. *Every* invocation in the command — each pipeline stage, each `&&`,
//!    `||` and `;` segment — names a binary on [`AUTO_SAFE_COMMANDS`] and
//!    passes that binary's argument rules.
//!
//! Rule 5 is what makes `ls && rm -rf /` prompt: the gate is a conjunction
//! over segments, never a decision about the first one.
//!
//! [`PermissionMode::Auto`]: crate::config::PermissionMode::Auto

use crate::tools::bash::bash_security::{DestructivenessLevel, classify_destructive};
use crate::tools::bash::command_semantics::{Effect, classify};
use crate::tools::bash_parse::{ParsedCommand, check_parsed_security, parse_bash};

/// Binaries Auto mode may run without asking.
///
/// Every entry is also on the classifier's read-only list, so the effect
/// gate and this allowlist agree. The list is intentionally *smaller* than
/// the classifier's: pagers (`less`, `more`) can spawn a shell, `awk` can
/// write files and call `system()`, `env` runs an arbitrary child, `top` is
/// interactive, `ip`/`ifconfig` can reconfigure interfaces, `dig`/`host`/
/// `nslookup` reach the network, and `uniq`/`hostname` write when handed a
/// positional argument. None of those are provably read-only, so none of
/// them are here.
pub const AUTO_SAFE_COMMANDS: &[&str] = &[
    "[",
    "basename",
    "cat",
    "cmp",
    "cut",
    "date",
    "df",
    "diff",
    "dirname",
    "du",
    "echo",
    "egrep",
    "false",
    "fgrep",
    "file",
    "find",
    "free",
    "git",
    "grep",
    "head",
    "id",
    "ll",
    "ls",
    "md5sum",
    "printenv",
    "printf",
    "ps",
    "pwd",
    "readlink",
    "realpath",
    "rg",
    "sha1sum",
    "sha256sum",
    "sort",
    "stat",
    "tail",
    "test",
    "tr",
    "true",
    "type",
    "uname",
    "uptime",
    "vmstat",
    "wc",
    "whereis",
    "which",
    "whoami",
];

/// `git` subcommands that only inspect history and the working tree.
///
/// Anything that can write refs, objects, the index, the working tree, the
/// config, or talk to a remote (`push`, `fetch`, `pull`, `clone`, `commit`,
/// `reset`, `checkout`, `clean`, `config`, `stash`, `tag`, `rebase`, …) is
/// absent and therefore prompts.
const GIT_SAFE_SUBCOMMANDS: &[&str] = &[
    "blame",
    "cat-file",
    "describe",
    "diff",
    "diff-index",
    "diff-tree",
    "grep",
    "log",
    "ls-files",
    "ls-tree",
    "rev-list",
    "rev-parse",
    "shortlog",
    "show",
    "status",
    "whatchanged",
];

/// `git` options that write a file or run an external program even under an
/// otherwise read-only subcommand. Matched exactly or as `flag=value`.
const GIT_UNSAFE_OPTIONS: &[&str] = &[
    "--config-env",
    "--exec-path",
    "--ext-diff",
    "--open-files-in-pager",
    "--output",
    "--output-indicator-new",
    "--pager",
    "--receive-pack",
    "--textconv",
    "--upload-pack",
    "-O",
];

/// `find` primaries that execute a program or modify the filesystem.
const FIND_UNSAFE_OPTIONS: &[&str] = &[
    "-delete", "-exec", "-execdir", "-fls", "-fprint", "-fprint0", "-fprintf", "-ok", "-okdir",
];

/// Per-binary options that turn an otherwise read-only tool into one that
/// writes a file or executes a helper program.
const UNSAFE_OPTIONS: &[(&str, &[&str])] = &[
    ("date", &["-s", "--set"]),
    ("find", FIND_UNSAFE_OPTIONS),
    ("rg", &["--pre", "--hostname-bin"]),
    ("sort", &["-o", "--output", "--compress-program"]),
];

/// True when `command` is provably read-only and may run without a prompt
/// under [`PermissionMode::Auto`](crate::config::PermissionMode::Auto).
///
/// Fails closed: any parse failure, any construct the gate cannot analyse,
/// and any unrecognised binary returns `false`.
pub fn is_auto_safe_shell_command(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }
    let Some(parsed) = parse_bash(command) else {
        return false;
    };
    is_auto_safe_parsed(&parsed)
}

fn is_auto_safe_parsed(parsed: &ParsedCommand) -> bool {
    // A command that did not parse cleanly cannot be reasoned about.
    if parsed.has_parse_error {
        return false;
    }

    // Command substitution, subshells and process substitution hide the
    // command line that will actually run; redirection writes to a path;
    // an assignment can steer the child (`PATH=`, `LD_PRELOAD=`, `IFS=`).
    if !parsed.substitutions.is_empty()
        || parsed.has_subshell
        || parsed.has_process_substitution
        || !parsed.redirections.is_empty()
        || !parsed.assignments.is_empty()
    {
        return false;
    }

    // Reuse the existing shell-safety checks rather than re-deriving them.
    if !check_parsed_security(parsed).is_empty() {
        return false;
    }
    if classify_destructive(parsed) != DestructivenessLevel::Safe {
        return false;
    }

    // Effect gate: an unknown command yields `Mutating`, and an empty set
    // (no command at all) is not provably anything.
    let effects = classify(parsed);
    if effects.is_empty() || effects.iter().any(|e| *e != Effect::ReadOnly) {
        return false;
    }

    if parsed.command_texts.is_empty() {
        return false;
    }
    parsed.command_texts.iter().all(|seg| segment_is_safe(seg))
}

/// Validate one invocation (`ls -la`, `git diff HEAD~1`, …).
fn segment_is_safe(segment: &str) -> bool {
    // `$` and a backtick mean expansion or substitution: the token the shell
    // ends up running is not the token we are looking at.
    if segment.contains('$') || segment.contains('`') {
        return false;
    }

    let tokens: Vec<String> = segment.split_whitespace().map(normalize_token).collect();
    let Some((head, args)) = tokens.split_first() else {
        return false;
    };

    let base = base_name(head);
    if !AUTO_SAFE_COMMANDS.contains(&base) {
        return false;
    }
    // Belt and braces: the shared classifier must independently agree that
    // this *invocation* is read-only. It is classified per segment rather
    // than by binary name, because for several binaries the arguments decide
    // — `git status` reads, `git push` does not; `find -name` reads,
    // `find -delete` does not. A name-only check would clear both.
    match parse_bash(segment) {
        Some(parsed) => {
            let effects = classify(&parsed);
            if effects.is_empty() || effects.iter().any(|e| *e != Effect::ReadOnly) {
                return false;
            }
        }
        None => return false,
    }

    if let Some((_, unsafe_opts)) = UNSAFE_OPTIONS.iter().find(|(name, _)| *name == base)
        && args.iter().any(|a| matches_option(a, unsafe_opts))
    {
        return false;
    }

    if base == "git" {
        return git_args_are_safe(args);
    }

    true
}

/// `git` needs subcommand-level analysis: the classifier only sees the name
/// `git` and calls it read-only, which is true of `git status` and false of
/// `git push`.
fn git_args_are_safe(args: &[String]) -> bool {
    // The subcommand must come first. A leading option is a global switch
    // (`-c core.pager=…`, `-C dir`, `--exec-path=…`) that can redirect what
    // git executes, so refuse rather than try to skip over it.
    let Some(subcommand) = args.first() else {
        return false;
    };
    if !GIT_SAFE_SUBCOMMANDS.contains(&subcommand.as_str()) {
        return false;
    }
    !args[1..]
        .iter()
        .any(|a| matches_option(a, GIT_UNSAFE_OPTIONS))
}

/// True when `arg` is `flag`, or `flag=value`, for any flag in `flags`.
fn matches_option(arg: &str, flags: &[&str]) -> bool {
    flags.iter().any(|flag| {
        arg == *flag
            || arg
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
    })
}

/// Strip quoting and escaping so `-'delete'` and `-dele\te` compare equal to
/// `-delete`. Without this an option denylist is trivially evaded.
fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| *c != '\'' && *c != '"' && *c != '\\')
        .collect()
}

/// Last path component of a command word, so `/usr/bin/ls` is still `ls`.
fn base_name(raw: &str) -> &str {
    raw.rsplit('/').next().unwrap_or(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The safety invariant: nothing Auto approves may be anything other than
    /// read-only according to the shared classifier.
    ///
    /// This used to compare `classify_single(name)` against the allowlist,
    /// which only held while the classifier judged by binary name. It is now
    /// argument-aware — `git` is read-only or not depending on the
    /// subcommand — so the invariant is asserted over whole invocations,
    /// which is what Auto actually approves.
    #[test]
    fn nothing_auto_approves_is_non_read_only() {
        for cmd in [
            "ls -la",
            "cat foo.rs",
            "git status",
            "git diff",
            "git log --oneline -20",
            "grep -r foo src/",
            "rg pattern",
            "find . -name '*.rs'",
            "wc -l file",
            "head -n 20 file",
            "sort file",
            "uniq file",
            "echo hi",
            "pwd",
            "cat a.txt | grep foo | wc -l",
            "git status && git diff",
        ] {
            if !is_auto_safe_shell_command(cmd) {
                continue; // not approved; the invariant says nothing about it
            }
            let parsed = parse_bash(cmd).expect("approved command must parse");
            let effects = classify(&parsed);
            assert!(
                !effects.is_empty() && effects.iter().all(|e| *e == Effect::ReadOnly),
                "auto approved {cmd} but the classifier reports {effects:?}"
            );
        }
    }

    /// Every binary on the allowlist must have at least one invocation the
    /// classifier agrees is read-only — otherwise the entry is dead weight
    /// that can never be approved, which usually means a stale name.
    #[test]
    fn every_allowlisted_binary_has_a_read_only_invocation() {
        for name in AUTO_SAFE_COMMANDS {
            let probe = match *name {
                "git" => "git status".to_string(),
                "find" => "find . -name x".to_string(),
                other => format!("{other} --help"),
            };
            let Some(parsed) = parse_bash(&probe) else {
                panic!("{name}: probe {probe:?} did not parse");
            };
            let effects = classify(&parsed);
            assert!(
                effects.iter().all(|e| *e == Effect::ReadOnly),
                "{name} is on the auto allowlist but no read-only invocation \
                 exists — probe {probe:?} classified {effects:?}"
            );
        }
    }

    #[test]
    fn safe_reads_are_auto_approved() {
        for cmd in [
            "ls -la",
            "cat foo.rs",
            "git status",
            "git diff",
            "grep -r foo src/",
            "rg pattern",
            "wc -l file",
            "head -n 20 file",
            "tail -f log.txt",
            "find . -name '*.rs'",
            "echo hi",
            "pwd",
            "git log --oneline -20",
            "cat a.txt | grep foo | wc -l",
            "git status && git diff",
        ] {
            assert!(is_auto_safe_shell_command(cmd), "{cmd} should be auto-safe");
        }
    }

    #[test]
    fn git_write_and_network_subcommands_are_not_auto_safe() {
        for cmd in [
            "git push",
            "git push origin main",
            "git reset --hard",
            "git commit -m x",
            "git checkout main",
            "git clean -fd",
            "git config user.email x@y.z",
            "git fetch",
            "git pull",
            "git stash",
            "git tag v1",
            "git -c core.pager=sh status",
            "git -C /tmp status",
            "git diff --output=/tmp/x",
            "git grep -O sh foo",
        ] {
            assert!(
                !is_auto_safe_shell_command(cmd),
                "{cmd} must not be allowed"
            );
        }
    }

    #[test]
    fn find_execution_primaries_are_not_auto_safe() {
        for cmd in [
            "find . -delete",
            "find . -exec rm {} ;",
            "find . -execdir sh -c x ;",
            "find . -ok rm {} ;",
            "find . -fprintf /tmp/out %p",
            // Quoting and escaping must not slip the primary past the gate.
            "find . -'delete'",
            "find . -dele\\te",
        ] {
            assert!(
                !is_auto_safe_shell_command(cmd),
                "{cmd} must not be allowed"
            );
        }
    }

    #[test]
    fn writing_options_on_read_only_tools_are_not_auto_safe() {
        for cmd in [
            "sort -o out.txt in.txt",
            "rg --pre ./x pattern",
            "date -s now",
        ] {
            assert!(
                !is_auto_safe_shell_command(cmd),
                "{cmd} must not be allowed"
            );
        }
    }

    #[test]
    fn expansion_and_substitution_are_not_auto_safe() {
        for cmd in [
            "echo $HOME",
            "ls $(pwd)",
            "ls `pwd`",
            "cat \"$FILE\"",
            "(ls)",
            "diff <(ls a) <(ls b)",
        ] {
            assert!(
                !is_auto_safe_shell_command(cmd),
                "{cmd} must not be allowed"
            );
        }
    }

    #[test]
    fn commands_outside_the_allowlist_are_not_auto_safe() {
        for cmd in [
            "awk '{print}' f",
            "env ls",
            "less f",
            "more f",
            "dig example.com",
            "ip link show",
            "uniq a b",
            "hostname newname",
            "./script.sh",
            "cargo build",
            "make",
        ] {
            assert!(
                !is_auto_safe_shell_command(cmd),
                "{cmd} must not be allowed"
            );
        }
    }

    #[test]
    fn empty_and_unparseable_commands_are_not_auto_safe() {
        for cmd in ["", "   ", "ls |&& ((( \"unterminated", "if then fi else"] {
            assert!(
                !is_auto_safe_shell_command(cmd),
                "{cmd:?} must not be allowed"
            );
        }
    }
}
