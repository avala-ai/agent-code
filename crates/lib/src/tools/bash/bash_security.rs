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

use crate::tools::bash_parse::{
    ParsedCommand, base_name, parse_bash, unquote_token, unwrapped_argv,
};

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

/// How deep to follow `bash -c` / `eval` payloads. Mirrors the
/// interpreter-depth cap in `protected_paths`.
const MAX_SHELL_SCAN_DEPTH: u8 = 3;

/// Like [`classify_destructive`] but returns every finding so callers
/// can present a useful message to the user.
pub fn find_destructive(cmd: &ParsedCommand) -> Vec<DestructiveFinding> {
    find_destructive_depth(cmd, 0)
}

fn find_destructive_depth(cmd: &ParsedCommand, depth: u8) -> Vec<DestructiveFinding> {
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
    //
    // A space inside one token is data, not an argument boundary —
    // `printf '%s\n' 'git push' --force` prints two strings, it does
    // not push. Mask intra-token spaces so a pattern can only match
    // across real argv boundaries.
    for invocation in &cmd.invocations {
        let normalized = invocation
            .iter()
            .map(|t| unquote_token(t).replace(' ', "\u{0}"))
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

    // Head scan: any invocation whose head is intrinsically destructive
    // is flagged even if the pattern scans did not catch it. The parsed
    // invocations are quote-aware, so `'keep|rm'` stays one argument
    // instead of minting a synthetic pipe segment, while a disguised
    // head (`/bin/rm`, `'rm'`, `$'rm'`) still normalizes to `rm`.
    for invocation in &cmd.invocations {
        let Some(head) = invocation.first() else {
            continue;
        };
        let base_owned = base_name(&unquote_token(head)).to_lowercase();
        let base = base_owned.as_str();
        if DESTRUCTIVE_PIPELINE_BASES.contains(&base) {
            findings.push(DestructiveFinding {
                level: DestructivenessLevel::Destructive,
                reason: format!("destructive command '{base}'"),
            });
        }
    }

    // Historical raw pipeline scan, kept byte-for-byte: an exact,
    // unnormalized head match per `|` segment. The invocation scan
    // above sees only what parses as a command, and this one also
    // fires on text mentions (heredoc bodies, quoted arguments), which
    // over-blocks — the safe direction — so removing it would narrow
    // the check.
    for segment in cmd.raw.split('|') {
        let trimmed = segment.trim();
        let base = trimmed.split_whitespace().next().unwrap_or("");
        if DESTRUCTIVE_PIPELINE_BASES.contains(&base) {
            findings.push(DestructiveFinding {
                level: DestructivenessLevel::Destructive,
                reason: format!("destructive command '{base}' in pipe"),
            });
        }
    }

    // Without a trustworthy parse there are no invocations to walk, so
    // fall back to splitting the raw text on `|`. Unquoting the head can
    // over-match inside what was really a quoted argument, but with no
    // parse to say otherwise that is the safe direction to fail.
    if cmd.invocations.is_empty() || cmd.has_parse_error {
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

    // Recursive shell payloads. `bash -c "'git' push --force"` hands
    // its literal argument to another shell, where intra-token spaces
    // become command boundaries again — so the payload gets the whole
    // scan, not the inert-data treatment. One unquote layer is exactly
    // the text the inner shell receives.
    for invocation in &cmd.invocations {
        let unquoted: Vec<String> = invocation.iter().map(|t| unquote_token(t)).collect();
        // `env bash -c …` and `command bash -c …` run the inner shell,
        // so the wrapper chain has to come off before the head means
        // anything. Reuses the wrapper resolution the parser already
        // models rather than a second copy of it. When it resolves,
        // the wrapped view is the *only* accurate one — scanning the
        // wrapper's own argv as well would read `env` as the head and
        // lose which command the operands belong to.
        if wrapper_only_reports(&unquoted) {
            continue;
        }
        let unwrapped = unwrapped_argv(invocation);
        let tokens = unwrapped.as_ref().unwrap_or(&unquoted);
        for payload in shell_payloads(tokens) {
            // Out of budget with a shell payload still in hand: what
            // it runs is unknown, and an unknown nested command cannot
            // be waved through. Refusing here over-blocks deeply
            // nested but innocent commands — the safe direction, and
            // the same call `protected_paths` makes at its own cap.
            if depth >= MAX_SHELL_SCAN_DEPTH {
                findings.push(DestructiveFinding {
                    level: DestructivenessLevel::Destructive,
                    reason: format!(
                        "nested shell payload exceeds scan depth {MAX_SHELL_SCAN_DEPTH}; \
                         what it runs cannot be determined"
                    ),
                });
                continue;
            }
            let inner = parse_bash(&payload).unwrap_or_else(|| ParsedCommand {
                raw: payload.clone(),
                ..ParsedCommand::default()
            });
            findings.extend(find_destructive_depth(&inner, depth + 1));
        }
    }

    findings
}

/// Commands that take a command string and run it in a fresh shell.
/// Not only the POSIX family: `fish -c` and `csh -c` interpret a
/// command string just as readily, and an interpreter that is present
/// on the host is a usable one.
const SHELL_LAUNCHERS: &[&str] = &[
    "bash", "rbash", "sh", "zsh", "dash", "ash", "ksh", "mksh", "pdksh", "yash", "busybox", "fish",
    "csh", "tcsh",
];

/// Builtins that run the command word after them, so a builtin behind
/// one is still in command position.
const BUILTIN_PREFIXES: &[&str] = &["command", "builtin", "exec", "nohup", "setsid", "time"];

/// Commands whose operands are text to print, match or transform —
/// never a program to run. A shell name among *their* arguments is
/// data: `printf '%s\n' bash -c "'git' push --force"` prints four
/// words.
///
/// Only this direction is listed. An unrecognised head keeps counting
/// as a possible runner, so `firejail bash -c '…'` and every other
/// runner nobody enumerated still recurse.
const DATA_COMMANDS: &[&str] = &[
    "echo", "printf", "cat", "tee", "grep", "egrep", "fgrep", "rg", "ag", "sed", "awk", "tr",
    "cut", "paste", "sort", "uniq", "head", "tail", "wc", "fold", "column", "diff", "comm", "jq",
    "yq", "less", "more", "strings", "logger",
];

/// The literal command strings an invocation may hand to another
/// shell.
///
/// A shell name is looked for at *any* position, not just the head.
/// Anything that runs another program can sit in front of one —
/// `timeout 10 bash -c '…'`, `sudo bash -c '…'`, `xargs bash -c '…'`,
/// `nice`, `stdbuf`, `chroot` — and enumerating those runners is a
/// list that is always one entry short. What the payload needs is a
/// shell to interpret it, and the shell has to be named in the argv,
/// so keying on that instead holds for runners nobody listed.
///
/// A launcher only counts when it is actually asked to interpret a
/// command string — some later token is a `-c` option. Without that,
/// the name is inert data (`printf '%s\n' bash "'git' push --force"`
/// prints two words) or names a script to run, and re-parsing what
/// follows would refuse harmless commands.
///
/// Given a `-c`, every remaining token is a candidate rather than only
/// the operand of that flag. Locating the operand exactly would mean
/// trusting bash's full option grammar — `-o option`, `-O shopt`,
/// `--rcfile FILE` and clusters all take operands, and one
/// misunderstood spelling (`bash -O extglob -c '…'`) silently skips
/// the payload. An option's own operand (`extglob`, `posix`) parses as
/// a harmless bare word, so the looser rule costs nothing; the
/// positional parameters after a `-c` string are re-parsed too, which
/// can only over-block.
///
/// `eval` instead concatenates everything after it, which is how bash
/// evaluates it — but only in command position. It is a builtin, so
/// unlike an external shell no runner can launch it, and an `eval`
/// sitting in argv data (`echo eval "'git' push --force"`) evaluates
/// nothing.
///
/// `exec` is deliberately not a launcher: it replaces the shell with
/// whatever it is given, so `exec bash -c '…'` is already covered by
/// the shell name sitting later in the argv, while `exec printf …`
/// passes plain argv data that no shell will interpret.
fn shell_payloads(tokens: &[String]) -> Vec<String> {
    let mut payloads = Vec::new();
    let head_is_data = tokens
        .first()
        .is_some_and(|t| DATA_COMMANDS.contains(&base_name(t).to_lowercase().as_str()));
    for (i, token) in tokens.iter().enumerate() {
        let base = base_name(token).to_lowercase();
        let rest = &tokens[i + 1..];
        if SHELL_LAUNCHERS.contains(&base.as_str()) {
            if head_is_data && !in_command_position(tokens, i) {
                continue;
            }
            let fish = base == "fish";
            if rest.iter().any(|t| takes_command_string(t, fish)) {
                payloads.extend(rest.iter().cloned());
                // `--command=CMD` carries the command inside the option
                // token, where a parser reads it as an assignment and
                // drops it. Hand over the operand on its own too.
                payloads.extend(rest.iter().filter_map(|t| inline_operand(t, fish)));
            }
        } else if base == "eval" && !rest.is_empty() && in_command_position(tokens, i) {
            payloads.push(rest.join(" "));
        }
    }
    payloads
}

/// True when the wrapper was asked to describe something instead of
/// running it: `env --help bash -c '…'` prints env's help and exits,
/// `command -v bash -c '…'` prints a path. The trailing words are
/// then operands of a wrapper that never executes them.
///
/// Only the wrapper's own leading options are inspected. A `--help`
/// after the wrapped command name belongs to that command and says
/// nothing about whether it runs.
fn wrapper_only_reports(tokens: &[String]) -> bool {
    let Some(wrapper) = tokens.first().map(|t| base_name(t).to_lowercase()) else {
        return false;
    };
    if !COMMAND_RUNNER_WRAPPERS.contains(&wrapper.as_str()) {
        return false;
    }
    tokens[1..]
        .iter()
        .take_while(|t| t.starts_with('-'))
        .any(|token| match wrapper.as_str() {
            // `command -v`/`-V` describe; `env -v` is verbose and
            // still runs the command, so it must not count here.
            "command" => token
                .strip_prefix('-')
                .is_some_and(|cluster| !cluster.starts_with('-') && cluster.contains(['v', 'V'])),
            _ => token == "--help" || token == "--version",
        })
}

/// Wrappers whose job is to run the command word after them.
const COMMAND_RUNNER_WRAPPERS: &[&str] = &["env", "command", "nohup", "setsid", "builtin"];

/// True when nothing before `i` can turn the token into plain data:
/// every earlier token is an option, an assignment, or a builtin that
/// runs the word after it.
fn in_command_position(tokens: &[String], i: usize) -> bool {
    tokens[..i].iter().all(|t| {
        t.starts_with('-')
            || t.contains('=')
            || BUILTIN_PREFIXES.contains(&base_name(t).to_lowercase().as_str())
    })
}

/// True for an option that makes a shell read its commands from an
/// operand: `-c`, a cluster containing it (`-lc`, `-ec`), and the long
/// spelling `--command`.
///
/// `fish` additionally runs `-C` / `--init-command=COMMANDS` at
/// startup, so those count only for fish — in bash `-C` is `noclobber`
/// and the operand after it is a script name, not a command string.
fn takes_command_string(token: &str, fish: bool) -> bool {
    let longs: &[&str] = if fish {
        &["command", "init-command"]
    } else {
        &["command"]
    };
    match token.strip_prefix("--") {
        Some(long) => long.split('=').next().is_some_and(|name| {
            !name.is_empty() && longs.iter().any(|full| full.starts_with(name))
        }),
        None => token
            .strip_prefix('-')
            .is_some_and(|cluster| cluster.contains('c') || (fish && cluster.contains('C'))),
    }
}

/// The operand written inside a command-string option
/// (`--command=CMD`), if there is one.
fn inline_operand(token: &str, fish: bool) -> Option<String> {
    if !takes_command_string(token, fish) {
        return None;
    }
    token
        .split_once('=')
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
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
            // Bash drops a NUL and the rest of the quoted segment, so
            // these run `git push --force` / `rm -rf`.
            "$'git\\x00junk' push --force",
            "$'rm\\0junk' -rf /tmp/x",
            "$'git\\c@junk' push --force",
            // Disguised heads behind a pipe or chain: the parsed
            // invocation scan must normalize them, not the raw text.
            "true | /bin/rm x",
            "true | $'rm' x",
            "echo hi && $'rm' x",
            // A literal payload handed to another shell is re-parsed
            // there, so its spaces are command boundaries after all.
            "bash -c \"'git' push --force\"",
            "sh -c \"'rm' -rf /tmp/x\"",
            "bash -lc \"'git' push --force\"",
            "eval \"'git' push --force\"",
            "bash -c \"bash -c \\\"'git' push --force\\\"\"",
            // Operand-taking shell options must not move the payload
            // out of view.
            "bash -O extglob -c \"'git' push --force\"",
            "bash -o posix -c \"'rm' -rf /tmp/x\"",
            "bash --rcfile /dev/null -c \"'git' push --force\"",
            // Wrapper chains run the inner shell.
            "env bash -c \"'git' push --force\"",
            "command bash -c \"'rm' -rf /tmp/x\"",
            "nohup sh -c \"'git' push --force\"",
            "env -u PATH bash -c \"'git' push --force\"",
            // `env -v` is verbose, not a describe-and-exit option: the
            // command still runs.
            "env -v bash -c \"'git' push --force\"",
            // `exec` replaces the shell with what follows it.
            "exec bash -c \"'git' push --force\"",
            "exec -a login bash -c \"'rm' -rf /tmp/x\"",
            // Runners in front of a shell: the shell is named in the
            // argv wherever it sits, so no list of runners is needed.
            "timeout 10 bash -c \"'git' push --force\"",
            "sudo bash -c \"'rm' -rf /tmp/x\"",
            "nice -n 5 sh -c \"'git' push --force\"",
            "xargs bash -c \"'rm' -rf /tmp/x\"",
            "stdbuf -oL bash -c \"'git' push --force\"",
            // Non-POSIX shells interpret a command string too.
            "fish -c \"'git' push --force\"",
            "csh -c \"'rm' -rf /tmp/x\"",
            "tcsh -c \"'git' push --force\"",
            "busybox sh -c \"'rm' -rf /tmp/x\"",
            // fish's long spelling of `-c`.
            "fish --command \"'git' push --force\"",
            "fish --command=\"'git' push --force\"",
            // `rbash` is restricted bash, and still takes `-c`.
            "rbash -c \"'git' push --force\"",
            // fish runs `-C` / `--init-command` during startup.
            "fish -C \"'git' push --force\" /dev/null",
            "fish --init-command=\"'rm' -rf /tmp/x\" /dev/null",
            // `eval` in command position, including behind a builtin
            // that runs the word after it.
            "eval \"'rm' -rf /tmp/x\"",
            "command eval \"'git' push --force\"",
            // Out of scan budget with a shell payload still in hand:
            // unknown, so refused rather than allowed.
            "bash -c \"bash -c \\\"bash -c \\\\\\\"bash -c 'ls'\\\\\\\"\\\"\"",
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
            // `\c3` is control byte 0x13, not `s` — this only prints a
            // control character and must not read as `shutdown`.
            "printf %s $'\\c3hutdown'",
            // A `|` inside a quoted argument is not a pipe boundary:
            // the segment after it must not be unquoted into a head.
            "printf '%s\\n' 'keep|rm'",
            "grep 'foo|dd' notes.txt",
            // A space inside a quoted argument is data, not an argument
            // boundary: this prints two strings, it does not push.
            "printf '%s\\n' 'git push' --force",
            "printf '%s' 'git clean' -f",
            // Shell payloads recurse without over-blocking: the inner
            // command gets the same data-vs-boundary treatment.
            "bash -c 'echo hello world'",
            "bash -c \"printf '%s\\n' 'git push' --force\"",
            "eval 'echo hi'",
            // Shell options and their operands are not payloads.
            "bash -O extglob -c 'ls'",
            "bash -o posix -c 'echo hi'",
            "env bash -c 'cargo build'",
            "bash script.sh",
            "timeout 10 bash -c 'cargo test'",
            // Naming a shell without handing it a command is not a
            // payload.
            "which bash",
            "man bash",
            // `exec` of a normal binary passes argv data, not a
            // command string for a shell to interpret.
            "exec printf '%s\\n' \"'git' push --force\"",
            // A launcher name as inert data, with no `-c` asking any
            // shell to interpret anything.
            "printf '%s\\n' bash \"'git' push --force\"",
            "echo sh zsh fish",
            // `eval` as argv data evaluates nothing — it is a builtin,
            // so no runner can launch it from there.
            "echo eval \"'git' push --force\"",
            "printf '%s\\n' eval \"'rm' -rf /tmp/x\"",
            // A shell name and a `-c` as ordinary data for a command
            // whose operands are text.
            "printf '%s\\n' bash -c \"'git' push --force\"",
            "echo bash -c \"'rm' -rf /tmp/x\"",
            // `-C` is noclobber in bash, not an init command.
            "bash -C safe.sh \"'git' push --force\"",
            // A data command behind a wrapper: the wrapped view is the
            // one that says which command the operands belong to.
            "env printf '%s\\n' bash -c \"'git' push --force\"",
            "command echo bash -c \"'rm' -rf /tmp/x\"",
            // A wrapper asked to describe rather than run.
            "env --help bash -c \"'git' push --force\"",
            "env --version bash -c \"'rm' -rf /tmp/x\"",
            "command -v bash -c \"'git' push --force\"",
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
