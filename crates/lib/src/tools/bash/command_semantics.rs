//! Coarse classification of bash commands by side effect.
//!
//! Given a [`ParsedCommand`], return the set of [`Effect`]s that any of
//! its component commands trigger. A single shell pipeline can have
//! multiple effects (for example `curl url | tee file` is both
//! [`Effect::Network`] and [`Effect::Mutating`]).
//!
//! Classification is **argument-aware**. A command name on its own is
//! not enough to decide whether an invocation is read-only: `git status`
//! reads while `git push` writes to a remote, `find -name` reads while
//! `find -delete` destroys, and `env` runs whatever follows it. Names
//! whose effect depends on their arguments are routed through
//! [`classify_arg_sensitive`] before the name tables are consulted;
//! [`FILE_READERS`] now means "read-only *whatever* the arguments are".
//!
//! This module is intentionally conservative: when a command name is not
//! recognised, or when an argument-aware rule cannot decide, the result
//! is [`Effect::Mutating`] so callers err on the side of caution.

use std::collections::BTreeSet;

use crate::tools::bash_parse::{ParsedCommand, base_name, is_env_assignment, unquote_token};

/// Coarse-grained effect categories used by the safety pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effect {
    /// Reads state without mutating anything (cat, ls, grep, ...).
    ReadOnly,
    /// Mutates the filesystem or other local state.
    Mutating,
    /// Touches the network (curl, wget, ssh, ...).
    Network,
    /// Requires elevated privileges (sudo, su, doas).
    Privileged,
}

/// How deep the `env`-style wrapper recursion may go before giving up
/// and failing closed.
const MAX_WRAPPER_DEPTH: usize = 8;

/// Compute the union of effects for every command in `parsed`.
pub fn classify(parsed: &ParsedCommand) -> Vec<Effect> {
    let mut set: BTreeSet<Effect> = BTreeSet::new();

    for argv in effective_invocations(parsed) {
        for effect in classify_invocation(argv) {
            set.insert(effect);
        }
    }

    // Any output redirection or process substitution writes to the
    // filesystem regardless of the command name: `echo foo > file` and
    // `awk '{}' > out` are mutating even though `echo` and `awk` read.
    if !set.contains(&Effect::Mutating) && writes_via_redirection(parsed) {
        set.insert(Effect::Mutating);
    }

    set.into_iter().collect()
}

/// Convenience helper: true if [`Effect::ReadOnly`] is the only effect.
pub fn is_read_only(parsed: &ParsedCommand) -> bool {
    let effects = classify(parsed);
    !effects.is_empty() && effects.iter().all(|e| *e == Effect::ReadOnly)
}

/// The argument vectors to classify. Prefers the parser's per-command
/// argv; falls back to bare command names for a [`ParsedCommand`] built
/// without them (e.g. the raw-only fallback used when parsing fails).
fn effective_invocations(parsed: &ParsedCommand) -> Vec<&[String]> {
    if parsed.invocations.is_empty() {
        parsed.commands.iter().map(std::slice::from_ref).collect()
    } else {
        parsed.invocations.iter().map(Vec::as_slice).collect()
    }
}

/// Classify a bare command name with no arguments known.
///
/// For argument-sensitive binaries this is the "no arguments" case, not
/// a verdict on every invocation of that name: `classify_single("git")`
/// describes `git` on its own, and says nothing about `git push`. Use
/// [`classify_invocation`] whenever the arguments are available.
pub fn classify_single(raw_name: &str) -> Vec<Effect> {
    classify_invocation(std::slice::from_ref(&raw_name.to_string()))
}

/// Classify one full invocation: command name followed by arguments.
pub fn classify_invocation(argv: &[String]) -> Vec<Effect> {
    classify_at_depth(argv, 0)
}

fn classify_at_depth(argv: &[String], depth: usize) -> Vec<Effect> {
    let Some(raw_name) = argv.first() else {
        return Vec::new();
    };
    if depth > MAX_WRAPPER_DEPTH {
        return vec![Effect::Mutating];
    }

    let base = base_name(&unquote_token(raw_name));
    let args = &argv[1..];

    if let Some(effects) = classify_arg_sensitive(&base, args, depth) {
        return effects;
    }

    let base = base.as_str();
    let mut effects = Vec::new();

    if PRIVILEGED.contains(&base) {
        effects.push(Effect::Privileged);
    }
    if NETWORK.contains(&base) {
        effects.push(Effect::Network);
    }
    if FILE_WRITERS.contains(&base) {
        effects.push(Effect::Mutating);
    }
    if FILE_READERS.contains(&base) {
        effects.push(Effect::ReadOnly);
    }

    if effects.is_empty() {
        // Unknown commands are conservatively treated as mutating; this
        // means the safety pipeline cannot be silently bypassed by an
        // exotic binary name.
        effects.push(Effect::Mutating);
    }

    effects
}

/// Effects for the binaries whose classification depends on arguments.
/// `None` means "not argument-sensitive, use the name tables".
fn classify_arg_sensitive(base: &str, args: &[String], depth: usize) -> Option<Vec<Effect>> {
    let effects = match base {
        "git" => git_effects(&normalise(args)),
        "env" => return Some(env_effects(args, depth)),
        "awk" | "gawk" | "mawk" | "nawk" => awk_effects(&normalise(args)),
        // Interactive pagers with shell escapes (`!cmd`, `v` to open an
        // editor). They have no place in an automated read-only path.
        "less" | "more" | "pg" => vec![Effect::Mutating],
        // Interactive by default and able to signal processes; only the
        // batch mode is a pure read.
        "top" => flag(
            !has_flag(&normalise(args), 'b', &["--batch-mode"]),
            Effect::Mutating,
        ),
        "sort" => sort_effects(&normalise(args)),
        "find" => flag(
            args_contain(&normalise(args), FIND_EXEC_OPTS),
            Effect::Mutating,
        ),
        "uniq" => uniq_effects(&normalise(args)),
        "hostname" => hostname_effects(&normalise(args)),
        "ip" => ip_effects(&normalise(args)),
        "ifconfig" => ifconfig_effects(&normalise(args)),
        // DNS lookups leave the machine.
        "dig" | "host" | "nslookup" | "drill" | "delv" => vec![Effect::Network],
        // `date -s` sets the system clock.
        "date" => flag(
            args_contain(&normalise(args), &["-s", "--set"]),
            Effect::Mutating,
        ),
        // `ss -K` destroys live sockets.
        "ss" => flag(
            has_flag(&normalise(args), 'K', &["--kill"]),
            Effect::Mutating,
        ),
        // `rg --pre` / `--hostname-bin` run an arbitrary program.
        "rg" | "ripgrep" => flag(
            args_contain(&normalise(args), &["--pre", "--hostname-bin"]),
            Effect::Mutating,
        ),
        _ => return None,
    };
    Some(effects)
}

/// `effect` when `triggered` holds, `ReadOnly` otherwise.
fn flag(triggered: bool, effect: Effect) -> Vec<Effect> {
    if triggered {
        vec![effect]
    } else {
        vec![Effect::ReadOnly]
    }
}

/// Strip one layer of shell quoting from every argument so option
/// matching cannot be evaded with `-'delete'` or `-dele\te`.
fn normalise(args: &[String]) -> Vec<String> {
    args.iter().map(|a| unquote_token(a)).collect()
}

/// The option name of a token, dropping any `=value` suffix.
fn opt_name(tok: &str) -> &str {
    tok.split('=').next().unwrap_or(tok)
}

fn is_option(tok: &str) -> bool {
    tok.starts_with('-') && tok.len() > 1
}

fn args_contain(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|a| names.contains(&opt_name(a)))
}

/// True when any argument carries `short` — on its own or bundled into a
/// short-option cluster such as `-tK` — or is one of `long`.
fn has_flag(args: &[String], short: char, long: &[&str]) -> bool {
    args.iter().any(|a| {
        if a.starts_with("--") {
            long.contains(&opt_name(a))
        } else {
            is_option(a) && a.chars().skip(1).any(|c| c == short)
        }
    })
}

// ---------------------------------------------------------------------
// git
// ---------------------------------------------------------------------

/// Global options that cannot redirect execution or run a program.
/// Anything else before the subcommand (notably `-c`, `-C`,
/// `--exec-path`, `--git-dir`, `--work-tree`) fails closed.
const GIT_SAFE_GLOBAL_OPTS: &[&str] = &[
    "--no-pager",
    "-P",
    "--no-optional-locks",
    "--literal-pathspecs",
    "--icase-pathspecs",
    "--glob-pathspecs",
    "--noglob-pathspecs",
    "--no-replace-objects",
];

/// Subcommand options that can invoke a program of the caller's choice.
const GIT_PROGRAM_OPTS: &[&str] = &[
    "--ext-diff",
    "--pager",
    "--textconv",
    "--upload-pack",
    "--receive-pack",
    "--exec",
    "-O",
    "--output",
    "--output-directory",
    "--to-cmd",
    "--cc-cmd",
];

/// Subcommands that only inspect repository state.
const GIT_READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "status",
    "diff",
    "diff-tree",
    "diff-files",
    "diff-index",
    "log",
    "show",
    "blame",
    "annotate",
    "rev-parse",
    "rev-list",
    "ls-files",
    "ls-tree",
    "cat-file",
    "describe",
    "shortlog",
    "whatchanged",
    "grep",
    "name-rev",
    "merge-base",
    "show-branch",
    "check-ignore",
    "check-attr",
    "count-objects",
    "var",
    "version",
];

/// Options of `git branch` / `git tag` that only list. Any other flag,
/// or any positional argument, means the invocation creates, renames or
/// deletes a ref.
const GIT_BRANCH_LIST_OPTS: &[&str] = &[
    "-a",
    "--all",
    "-r",
    "--remotes",
    "-v",
    "-vv",
    "--verbose",
    "-l",
    "--list",
    "-q",
    "--quiet",
    "--color",
    "--no-color",
    "--column",
    "--no-column",
    "--sort",
    "--format",
    "--show-current",
];

const GIT_TAG_LIST_OPTS: &[&str] = &[
    "-l",
    "--list",
    "-n",
    "--sort",
    "--format",
    "--color",
    "--no-color",
];

fn git_effects(args: &[String]) -> Vec<Effect> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if arg == "--" {
            i += 1;
            break;
        }
        if !is_option(arg) {
            break;
        }
        if !GIT_SAFE_GLOBAL_OPTS.contains(&opt_name(arg)) {
            // `-c key=val`, `-C dir`, `--git-dir`, `--exec-path`, … all
            // redirect where and how git runs.
            return vec![Effect::Mutating];
        }
        i += 1;
    }

    let Some(sub) = args.get(i) else {
        // Bare `git` (or only global options): nothing to reason about.
        return vec![Effect::Mutating];
    };
    let rest = &args[i + 1..];

    if args_contain(rest, GIT_PROGRAM_OPTS) {
        return vec![Effect::Mutating];
    }

    match sub.as_str() {
        "push" | "pull" | "fetch" | "clone" => vec![Effect::Mutating, Effect::Network],
        "ls-remote" => vec![Effect::Network],
        "branch" => list_only(rest, GIT_BRANCH_LIST_OPTS),
        "tag" => list_only(rest, GIT_TAG_LIST_OPTS),
        "remote" => match rest.first().map(String::as_str) {
            None => vec![Effect::ReadOnly],
            Some("-v" | "--verbose" | "get-url") => vec![Effect::ReadOnly],
            Some("show") => vec![Effect::Network],
            _ => vec![Effect::Mutating],
        },
        "stash" => match rest.first().map(String::as_str) {
            Some("list" | "show") => vec![Effect::ReadOnly],
            _ => vec![Effect::Mutating],
        },
        "reflog" => match rest.first().map(String::as_str) {
            None | Some("show") => vec![Effect::ReadOnly],
            _ => vec![Effect::Mutating],
        },
        s if GIT_READ_ONLY_SUBCOMMANDS.contains(&s) => vec![Effect::ReadOnly],
        _ => vec![Effect::Mutating],
    }
}

/// Read-only only when every argument is a listing flag: a positional
/// argument to `git branch`/`git tag` creates a ref.
fn list_only(args: &[String], allowed: &[&str]) -> Vec<Effect> {
    let ok = args
        .iter()
        .all(|a| is_option(a) && allowed.contains(&opt_name(a)));
    flag(!ok, Effect::Mutating)
}

// ---------------------------------------------------------------------
// env
// ---------------------------------------------------------------------

/// `env` executes whatever follows it, so classify the wrapped command.
/// Bare `env` (or `env` with assignments only) just prints the
/// environment and stays read-only; an option we cannot account for —
/// notably `-S`/`--split-string`, which hides a whole command line —
/// fails closed.
fn env_effects(args: &[String], depth: usize) -> Vec<Effect> {
    let norm = normalise(args);
    let mut i = 0;
    while let Some(arg) = norm.get(i) {
        if arg == "--" {
            i += 1;
            break;
        }
        if is_option(arg) {
            let takes_value = match opt_name(arg) {
                "-i" | "--ignore-environment" | "-0" | "--null" | "-v" | "--debug" => false,
                "-u" | "--unset" | "-C" | "--chdir" => !arg.contains('='),
                _ => return vec![Effect::Mutating],
            };
            i += if takes_value { 2 } else { 1 };
            continue;
        }
        if is_env_assignment(arg) {
            i += 1;
            continue;
        }
        break;
    }

    if i >= args.len() {
        return vec![Effect::ReadOnly];
    }
    classify_at_depth(&args[i..], depth + 1)
}

// ---------------------------------------------------------------------
// awk
// ---------------------------------------------------------------------

/// awk program fragments that shell out or write files.
const AWK_UNSAFE_MARKERS: &[&str] = &[
    "system(", "close(", "print>", "printf>", ">>", ">\"", ">'", "|getline", "|\"", "\"|", "|'",
    "'|", "|&",
];

fn awk_effects(args: &[String]) -> Vec<Effect> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if arg == "--" {
            i += 1;
            break;
        }
        if !is_option(arg) {
            break;
        }
        // `-f progfile` / `-E progfile`: the program lives in a file we
        // cannot inspect here.
        if arg.starts_with("-f") || arg.starts_with("-E") || opt_name(arg) == "--file" {
            return vec![Effect::Mutating];
        }
        let attached = arg.len() > 2 || arg.contains('=');
        let takes_value = matches!(
            opt_name(arg),
            "-v" | "--assign" | "-F" | "--field-separator" | "-e" | "--source"
        );
        i += if takes_value && !attached { 2 } else { 1 };
    }

    let program = &args[i.min(args.len())..];
    if program.is_empty() {
        // No program text to reason about.
        return vec![Effect::Mutating];
    }
    let unsafe_program = program.iter().any(|tok| {
        let compact: String = tok.chars().filter(|c| !c.is_whitespace()).collect();
        AWK_UNSAFE_MARKERS.iter().any(|m| compact.contains(m))
    });
    flag(unsafe_program, Effect::Mutating)
}

// ---------------------------------------------------------------------
// sort / find / uniq / hostname / ip / ifconfig
// ---------------------------------------------------------------------

/// `find` actions that run a program or delete files.
const FIND_EXEC_OPTS: &[&str] = &[
    "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprintf", "-fls", "-fprint", "-fprint0",
];

/// `sort -o FILE` / `--output=FILE` writes a file. `-o` may also appear
/// inside a bundled short-option cluster (`-uo out`).
fn sort_effects(args: &[String]) -> Vec<Effect> {
    for arg in args {
        if arg == "--" {
            break;
        }
        let writes = if arg.starts_with("--") {
            opt_name(arg) == "--output"
        } else {
            is_option(arg) && arg.chars().skip(1).any(|c| c == 'o')
        };
        if writes {
            return vec![Effect::Mutating];
        }
    }
    vec![Effect::ReadOnly]
}

/// `uniq INPUT OUTPUT` writes its second positional argument.
fn uniq_effects(args: &[String]) -> Vec<Effect> {
    let mut positionals = 0;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if arg == "--" {
            positionals += args.len() - i - 1;
            break;
        }
        if arg.starts_with("--") {
            let takes_value = matches!(
                opt_name(arg),
                "--skip-fields" | "--skip-chars" | "--check-chars" | "--group" | "--all-repeated"
            );
            i += if takes_value && !arg.contains('=') {
                2
            } else {
                1
            };
            continue;
        }
        if is_option(arg) {
            let cluster: Vec<char> = arg.chars().skip(1).collect();
            let takes_value = cluster.iter().any(|c| matches!(c, 'f' | 's' | 'w'));
            let attached = cluster.iter().any(char::is_ascii_digit);
            i += if takes_value && !attached { 2 } else { 1 };
            continue;
        }
        positionals += 1;
        i += 1;
    }
    flag(positionals >= 2, Effect::Mutating)
}

/// `hostname NEWNAME` (or `-F file`) sets the system hostname.
fn hostname_effects(args: &[String]) -> Vec<Effect> {
    for arg in args {
        if is_option(arg) {
            if matches!(opt_name(arg), "-F" | "--file" | "-b" | "--boot") {
                return vec![Effect::Mutating];
            }
            continue;
        }
        return vec![Effect::Mutating];
    }
    vec![Effect::ReadOnly]
}

/// Objects and listing verbs of `ip`. Anything else — an interface
/// name, an address, `set`, `add`, `del` — means the invocation is
/// configuring the network, not reading it.
const IP_LISTING_WORDS: &[&str] = &[
    "link",
    "l",
    "addr",
    "address",
    "a",
    "route",
    "r",
    "ro",
    "neigh",
    "neighbor",
    "neighbour",
    "n",
    "rule",
    "maddr",
    "mroute",
    "tunnel",
    "netns",
    "addrlabel",
    "tcp_metrics",
    "vrf",
    "stats",
    "show",
    "list",
    "lst",
    "sh",
    "s",
    "ls",
];

fn ip_effects(args: &[String]) -> Vec<Effect> {
    for arg in args {
        if is_option(arg) {
            continue;
        }
        if !IP_LISTING_WORDS.contains(&arg.as_str()) {
            return vec![Effect::Mutating];
        }
    }
    vec![Effect::ReadOnly]
}

/// A bare `ifconfig` (or `ifconfig -a`) lists interfaces; anything
/// positional (`ifconfig eth0 down`) configures them, and the two are
/// not reliably separable, so any positional fails closed.
fn ifconfig_effects(args: &[String]) -> Vec<Effect> {
    flag(args.iter().any(|a| !is_option(a)), Effect::Mutating)
}

// ---------------------------------------------------------------------
// redirection
// ---------------------------------------------------------------------

/// Detect any output redirection (`>`, `>>`, `&>`, `2>`, heredoc-to-file
/// from `<<<`, or process substitution `>(…)`) in the parsed command.
///
/// File-descriptor duplications such as `2>&1` are NOT output
/// redirections to a path and must not be flagged here.
pub fn writes_via_redirection(cmd: &ParsedCommand) -> bool {
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

/// Commands that read filesystem or system state without mutating it,
/// **whatever arguments they are given**.
///
/// A name belongs here only when no argument can turn it into a write,
/// a network call, or a way to run another program. Names whose effect
/// depends on their arguments (`git`, `env`, `awk`, `find`, `sort`,
/// `uniq`, `hostname`, `ip`, `ifconfig`, `top`, `date`, `ss`, `rg`) are
/// handled by [`classify_arg_sensitive`]; `less`, `more` and the DNS
/// clients are never read-only.
const FILE_READERS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "ls",
    "ll",
    "grep",
    "egrep",
    "fgrep",
    "du",
    "df",
    "stat",
    "file",
    "wc",
    "cut",
    "tr",
    "echo",
    "printf",
    "true",
    "false",
    "pwd",
    "which",
    "whereis",
    "type",
    "id",
    "whoami",
    "uname",
    "printenv",
    "ps",
    "uptime",
    "free",
    "vmstat",
    "lsof",
    "netstat",
    "diff",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "basename",
    "dirname",
    "readlink",
    "realpath",
    "test",
    "[",
];

/// Commands that mutate filesystem or other local state.
const FILE_WRITERS: &[&str] = &[
    "cp", "mv", "rm", "rmdir", "mkdir", "touch", "chmod", "chown", "chgrp", "ln", "tee", "dd",
    "shred", "truncate", "install", "patch", "sed", "tar", "gzip", "gunzip", "zip", "unzip", "xz",
    "bzip2", "bunzip2",
];

/// Commands that touch the network.
const NETWORK: &[&str] = &[
    "curl",
    "wget",
    "nc",
    "ncat",
    "ssh",
    "scp",
    "rsync",
    "sftp",
    "ftp",
    "git-remote-https",
    "telnet",
];

/// Commands that require elevated privileges.
const PRIVILEGED: &[&str] = &["sudo", "su", "doas", "pkexec"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash_parse::parse_bash;

    fn classify_str(cmd: &str) -> Vec<Effect> {
        let parsed = parse_bash(cmd).expect("parse");
        classify(&parsed)
    }

    fn read_only(cmd: &str) -> bool {
        classify_str(cmd) == vec![Effect::ReadOnly]
    }

    #[test]
    fn read_only_commands_are_read_only() {
        assert_eq!(classify_str("ls -la"), vec![Effect::ReadOnly]);
        assert_eq!(classify_str("grep TODO src/lib.rs"), vec![Effect::ReadOnly]);
        assert_eq!(classify_str("cat README.md"), vec![Effect::ReadOnly]);
    }

    #[test]
    fn file_writers_are_mutating() {
        assert!(classify_str("cp a b").contains(&Effect::Mutating));
        assert!(classify_str("rm -rf dir").contains(&Effect::Mutating));
        assert!(classify_str("mkdir foo").contains(&Effect::Mutating));
    }

    #[test]
    fn network_commands_flagged() {
        assert!(classify_str("curl https://example.com").contains(&Effect::Network));
        assert!(classify_str("wget http://x").contains(&Effect::Network));
        assert!(classify_str("ssh host").contains(&Effect::Network));
    }

    #[test]
    fn privileged_commands_flagged() {
        assert!(classify_str("sudo apt install foo").contains(&Effect::Privileged));
        assert!(classify_str("doas reboot").contains(&Effect::Privileged));
    }

    #[test]
    fn pipeline_unions_effects() {
        let effects = classify_str("curl https://example.com | tee out.txt");
        assert!(effects.contains(&Effect::Network));
        assert!(effects.contains(&Effect::Mutating));
    }

    #[test]
    fn unknown_command_is_mutating() {
        let effects = classify_single("some-unknown-binary");
        assert_eq!(effects, vec![Effect::Mutating]);
    }

    #[test]
    fn is_read_only_helper() {
        let parsed = parse_bash("ls -la").unwrap();
        assert!(is_read_only(&parsed));
        let parsed = parse_bash("rm foo").unwrap();
        assert!(!is_read_only(&parsed));
        let parsed = parse_bash("cat a | grep b").unwrap();
        assert!(is_read_only(&parsed));
    }

    #[test]
    fn empty_command_has_no_effects() {
        let parsed = ParsedCommand::default();
        assert!(classify(&parsed).is_empty());
    }

    #[test]
    fn classify_falls_back_to_names_without_argv() {
        // A ParsedCommand built without `invocations` (the raw-only
        // fallback used when tree-sitter fails) still classifies.
        let parsed = ParsedCommand {
            raw: "rm -rf /".into(),
            commands: vec!["rm".into()],
            ..ParsedCommand::default()
        };
        assert_eq!(classify(&parsed), vec![Effect::Mutating]);
    }

    #[test]
    fn redirection_is_mutating_in_the_classifier() {
        assert!(classify_str("echo hi > f").contains(&Effect::Mutating));
        assert!(classify_str("cat a >> b").contains(&Effect::Mutating));
        assert!(read_only("ls 2>&1"));
    }

    // --- git ---------------------------------------------------------

    #[test]
    fn git_read_only_subcommands() {
        for cmd in [
            "git status",
            "git diff",
            "git diff --stat HEAD~1",
            "git log --oneline -5",
            "git show HEAD",
            "git blame src/lib.rs",
            "git rev-parse HEAD",
            "git ls-files",
            "git describe --tags",
            "git cat-file -p HEAD",
            "git branch",
            "git branch -a",
            "git remote -v",
            "git stash list",
            "git tag",
            "git --no-pager log",
        ] {
            assert!(read_only(cmd), "expected read-only: {cmd}");
        }
    }

    #[test]
    fn git_mutating_subcommands() {
        for cmd in [
            "git commit -m x",
            "git reset --hard",
            "git clean -fd",
            "git checkout main",
            "git merge feature",
            "git rebase main",
            "git config user.name x",
            "git apply patch.diff",
            "git stash",
            "git stash drop",
            "git tag -d v1",
            "git branch -D old",
            "git branch newbranch",
            "git remote add origin url",
            "git reflog expire --all",
            "git",
        ] {
            assert!(
                classify_str(cmd).contains(&Effect::Mutating),
                "expected mutating: {cmd}"
            );
        }
    }

    #[test]
    fn git_network_subcommands() {
        for cmd in [
            "git push",
            "git push origin main",
            "git fetch",
            "git pull",
            "git clone https://example.com/x",
            "git ls-remote origin",
        ] {
            assert!(
                classify_str(cmd).contains(&Effect::Network),
                "expected network: {cmd}"
            );
        }
    }

    #[test]
    fn git_execution_redirecting_globals_are_mutating() {
        for cmd in [
            "git -c core.pager=sh status",
            "git -C /tmp status",
            "git --git-dir=/tmp/.git status",
            "git --work-tree=/tmp status",
            "git --exec-path=/tmp status",
        ] {
            assert_eq!(
                classify_str(cmd),
                vec![Effect::Mutating],
                "expected mutating: {cmd}"
            );
        }
    }

    #[test]
    fn git_program_invoking_options_are_mutating() {
        for cmd in [
            "git diff --ext-diff",
            "git log --textconv",
            "git show --pager=sh",
            "git format-patch -O order",
            "git diff --output=/tmp/x",
        ] {
            assert!(
                classify_str(cmd).contains(&Effect::Mutating),
                "expected mutating: {cmd}"
            );
        }
    }

    // --- env ---------------------------------------------------------

    #[test]
    fn env_classifies_the_wrapped_command() {
        assert!(classify_str("env rm -rf /tmp/x").contains(&Effect::Mutating));
        assert!(read_only("env ls"));
        assert!(read_only("env FOO=bar ls -la"));
        assert!(read_only("env -i ls"));
        assert!(read_only("env -u HOME ls"));
        assert!(classify_str("env git push").contains(&Effect::Network));
    }

    #[test]
    fn bare_env_is_read_only_and_does_not_panic() {
        assert!(read_only("env"));
        assert!(read_only("env FOO=bar"));
    }

    #[test]
    fn env_split_string_fails_closed() {
        assert_eq!(
            classify_str("env -S 'rm -rf /tmp/x'"),
            vec![Effect::Mutating]
        );
    }

    #[test]
    fn nested_env_wrappers_terminate() {
        let deep = format!("{} ls", "env ".repeat(20));
        assert!(classify_str(deep.trim()).contains(&Effect::Mutating));
    }

    // --- awk ---------------------------------------------------------

    #[test]
    fn awk_shell_escapes_are_mutating() {
        for cmd in [
            "awk 'BEGIN{system(\"rm -rf /tmp/x\")}'",
            "awk '{print > \"/tmp/out\"}' f",
            "awk '{printf \"%s\", $0 >> \"/tmp/out\"}' f",
            "awk '{close(\"x\")}' f",
            "awk '{print | \"sh\"}' f",
            "awk '{\"id\" | getline v}' f",
            "awk -f prog.awk f",
        ] {
            assert!(
                classify_str(cmd).contains(&Effect::Mutating),
                "expected mutating: {cmd}"
            );
        }
    }

    #[test]
    fn plain_awk_programs_stay_read_only() {
        assert!(read_only("awk '{print $1}' f"));
        assert!(read_only("awk -F: '{print $1}' /etc/passwd"));
        assert!(read_only("awk '$1 > 5 {print}' f"));
    }

    // --- find / sort / uniq / hostname -------------------------------

    #[test]
    fn find_actions_are_mutating() {
        for cmd in [
            "find . -delete",
            "find . -name x -exec rm {} ;",
            "find . -execdir sh -c x ;",
            "find . -ok rm {} ;",
            "find . -fprintf /tmp/x %p",
        ] {
            assert!(
                classify_str(cmd).contains(&Effect::Mutating),
                "expected mutating: {cmd}"
            );
        }
        assert!(read_only("find . -name '*.rs'"));
        assert!(read_only("find . -type f -printf '%p'"));
    }

    #[test]
    fn quote_and_backslash_evasion_is_normalised() {
        assert!(classify_str("find . -'delete'").contains(&Effect::Mutating));
        assert!(classify_str("find . -dele\\te").contains(&Effect::Mutating));
        assert!(classify_str("sort --'output'=/tmp/x f").contains(&Effect::Mutating));
        assert!(classify_str("sort --outp\\ut=/tmp/x f").contains(&Effect::Mutating));
    }

    #[test]
    fn sort_output_writes() {
        assert!(classify_str("sort -o /tmp/pwned /etc/hostname").contains(&Effect::Mutating));
        assert!(classify_str("sort --output=/tmp/x f").contains(&Effect::Mutating));
        assert!(read_only("sort f"));
        assert!(read_only("sort -u -k1,1 f"));
    }

    #[test]
    fn uniq_second_positional_writes() {
        assert!(classify_str("uniq in out").contains(&Effect::Mutating));
        assert!(read_only("uniq f"));
        assert!(read_only("uniq -c f"));
        assert!(read_only("uniq -f 2 f"));
    }

    #[test]
    fn hostname_with_positional_writes() {
        assert!(classify_str("hostname newname").contains(&Effect::Mutating));
        assert!(classify_str("hostname -F /tmp/name").contains(&Effect::Mutating));
        assert!(read_only("hostname"));
        assert!(read_only("hostname -f"));
    }

    // --- network / interactive ---------------------------------------

    #[test]
    fn dns_clients_are_network() {
        for cmd in [
            "dig example.com",
            "host example.com",
            "nslookup example.com",
        ] {
            assert_eq!(classify_str(cmd), vec![Effect::Network], "{cmd}");
        }
    }

    #[test]
    fn pagers_with_shell_escapes_are_mutating() {
        assert_eq!(classify_str("less /etc/passwd"), vec![Effect::Mutating]);
        assert_eq!(classify_str("more /etc/passwd"), vec![Effect::Mutating]);
    }

    #[test]
    fn ip_and_ifconfig_configuration_is_mutating() {
        for cmd in [
            "ip link set eth0 down",
            "ip addr add 10.0.0.1/24 dev eth0",
            "ip route del default",
            "ifconfig eth0 down",
        ] {
            assert!(
                classify_str(cmd).contains(&Effect::Mutating),
                "expected mutating: {cmd}"
            );
        }
        assert!(read_only("ip addr"));
        assert!(read_only("ip -br link show"));
        assert!(read_only("ifconfig -a"));
    }

    #[test]
    fn interactive_and_side_effecting_readers() {
        assert_eq!(classify_str("top"), vec![Effect::Mutating]);
        assert!(read_only("top -b -n 1"));
        assert!(classify_str("date -s 12:00").contains(&Effect::Mutating));
        assert!(read_only("date +%s"));
        assert!(classify_str("ss -K dst 1.2.3.4").contains(&Effect::Mutating));
        assert!(read_only("ss -tulpn"));
        assert!(classify_str("rg --pre ./evil.sh pat").contains(&Effect::Mutating));
        assert!(read_only("rg pat"));
    }
}
