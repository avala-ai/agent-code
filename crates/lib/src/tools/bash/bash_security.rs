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
    ParsedCommand, base_name, env_split_string, is_env_assignment, parse_bash, unquote_token,
    unwrapped_argv,
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
    //
    // Under a head whose operands are text the quoting is the user's
    // meaning, not a disguise: `printf '%s\n' 'git' push --force`
    // prints four words. The raw scan still reads those invocations,
    // so this only declines to *add* a match the text does not have.
    for invocation in &cmd.invocations {
        // `env printf …` prints just as `printf …` does, so the
        // wrapper comes off before the head is read — and the
        // wrapper's own view is not scanned as executable operands.
        // A wrapper asked to describe itself runs none of what follows,
        // so those words are operands of a command that never starts.
        if wrapper_only_reports(
            &invocation
                .iter()
                .map(|t| unquote_token(t))
                .collect::<Vec<_>>(),
        ) {
            continue;
        }
        let unwrapped = unwrapped_argv(invocation);
        let invocation = unwrapped.as_ref().unwrap_or(invocation);
        let head_is_data = invocation.first().is_some_and(|t| {
            DATA_COMMANDS.contains(&base_name(&unquote_token(t)).to_lowercase().as_str())
        });
        if head_is_data {
            continue;
        }
        let normalized = invocation
            .iter()
            .map(|t| unquote_token(t).replace(' ', "\u{0}"))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        // The masking above answers "is this one argument or several".
        // A multi-word pattern living *inside* one argument is the
        // other question, and it needs its spaces: `psql -c DROP\
        // TABLE\ users` is one token that runs a whole statement.
        // Scanning each token on its own asks it without letting a
        // pattern span two arguments.
        let per_token: Vec<String> = invocation
            .iter()
            .map(|t| unquote_token(t).to_lowercase())
            .collect();
        for pattern in DESTRUCTIVE_PATTERNS {
            if normalized.contains(pattern) || per_token.iter().any(|t| t.contains(pattern)) {
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
        // `env git push -uf …` runs git, so the wrapper chain comes off
        // before the cluster check can see the subcommand. Nothing is
        // checked at all when the chain was only asked to describe
        // itself — `command env --help git push -uf` prints env's help.
        let unquoted: Vec<String> = invocation.iter().map(|t| unquote_token(t)).collect();
        if wrapper_only_reports(&unquoted) {
            continue;
        }
        // When the chain resolves, the wrapped view is the only
        // accurate one: with `env` as the head, `env printf … git push
        // --force` would read a printed word as a command.
        let unwrapped = unwrapped_argv(invocation);
        {
            let tokens = unwrapped.as_deref().unwrap_or(invocation.as_slice());
            if let Some(reason) = clustered_git_flag(tokens) {
                findings.push(DestructiveFinding {
                    level: DestructivenessLevel::Destructive,
                    reason,
                });
            }
        }
    }

    // `$"…"` is not just double quoting: bash translates the string
    // through the locale catalogue first, so `$"cat"` can arrive as
    // any other command entirely. What runs is decided outside the
    // command text, which is the same answer as any other
    // unresolvable input — refuse rather than read the untranslated
    // source as if it were the result.
    if shell_text_facts(&cmd.raw).locale_quote && translation_can_reach_a_command(cmd) {
        findings.push(DestructiveFinding {
            level: DestructivenessLevel::Destructive,
            reason: "locale-translated string ($\"…\"); what it runs cannot be determined"
                .to_string(),
        });
    }

    // Git configuration handed over through the environment never
    // appears in the argv, so an alias defined there turns the command
    // word into something this scan cannot read. Done per statement:
    // a prefix assignment belongs to the command it precedes, and
    // `GIT_CONFIG_GLOBAL=… /bin/true; git status` must not tar the
    // second statement with the first one's environment.
    //
    // Environment that outlives its statement is carried forward.
    // Only a *command prefix* (`FOO=bar cmd`) is scoped to one command
    // by the shell; everything else is shell state, so the walk keeps
    // the state a shell would: each variable's latest value, and which
    // names carry the export attribute. That attribute sticks, so a
    // later bare assignment to an exported name still reaches git.
    //
    // The walk models a plain sequence of statements. Where the shell
    // stops being one — a branch that may not run, a heredoc whose
    // body is data rather than code, a `set` whose options this cannot
    // resolve — the state after it is unknown, and a git command
    // downstream of an unknown environment is refused rather than
    // read against a state that may be wrong in either direction.
    let statements = shell_statements(&cmd.raw);
    if state_is_unresolvable(&cmd.raw, &statements) && carries_git_config(&statements) {
        findings.push(DestructiveFinding {
            level: DestructivenessLevel::Destructive,
            reason: "git configuration is set where the shell state cannot be followed; what it \
                     runs cannot be determined"
                .to_string(),
        });
    }
    let mut shell_vars: Vec<(String, String)> = Vec::new();
    let mut exported: Vec<String> = Vec::new();
    let mut allexport = false;
    // Once git's configuration has been pointed somewhere unreadable,
    // a later assignment cannot argue it back to safety: this walk
    // cannot know that the later one is the value git ends up with —
    // a subshell keeps its own copy, a branch may not run. So opacity
    // sticks for the rest of the command.
    let mut config_ever_opaque = false;
    for statement in statements {
        let Some(parsed) = parse_bash(&statement) else {
            continue;
        };
        if let Some(state) = allexport_transition(&parsed) {
            allexport = state;
        }
        // A subshell keeps its own environment, so what it assigns
        // never reaches the parent's later commands.
        if !statement.trim_start().starts_with('(') {
            apply_statement_env(
                &statement,
                &parsed,
                allexport,
                &mut shell_vars,
                &mut exported,
            );
        }
        let carried: Vec<(String, String)> = exported
            .iter()
            .filter_map(|name| {
                shell_vars
                    .iter()
                    .rev()
                    .find(|(known, _)| known == name)
                    .cloned()
            })
            .collect();
        config_ever_opaque = config_ever_opaque || env_defines_opaque_git_alias(&carried);
        // A launcher inherits the environment, so `GIT_CONFIG_GLOBAL=…
        // bash -c 'git p'` configures the git inside the payload. The
        // payload is one token here, so tokens are searched word by
        // word rather than whole.
        let runs_git = parsed.invocations.iter().any(|invocation| {
            // A head whose operands are text runs nothing they name:
            // `GIT_CONFIG_GLOBAL=… printf '%s\n' git` prints the word.
            let head_is_data = invocation.first().is_some_and(|t| {
                DATA_COMMANDS.contains(&base_name(&unquote_token(t)).to_lowercase().as_str())
            });
            if head_is_data {
                return false;
            }
            [
                Some(invocation.as_slice()),
                unwrapped_argv(invocation).as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|tokens| {
                tokens.iter().enumerate().any(|(index, token)| {
                    split_alias_value(&unquote_token(token))
                        .iter()
                        .any(|word| base_name(word).to_lowercase() == "git")
                        // `git --version` prints and exits without
                        // dispatching anything, so no alias of any
                        // origin can run.
                        && !dispatches_no_subcommand(
                            &tokens[index + 1..]
                                .iter()
                                .map(|t| unquote_token(t))
                                .collect::<Vec<_>>(),
                        )
                })
            })
        });
        if !runs_git {
            continue;
        }
        let mut pairs: Vec<(String, String)> = carried.clone();
        pairs.extend(
            parsed
                .assignments
                .iter()
                .map(|(name, value)| (unquote_token(name), unquote_token(value))),
        );
        for invocation in &parsed.invocations {
            pairs.extend(runner_env_pairs(invocation));
        }
        // A `GIT_CONFIG…` word the positional walk did not account for
        // is the end of the argument: the walk models one option
        // grammar, and `timeout 5 env GIT_CONFIG_GLOBAL=… git p`,
        // `env -S'FOO=x' GIT_CONFIG_GLOBAL=… git p` and `env --uns X
        // GIT_CONFIG_GLOBAL=… git p` each hand git the variable
        // through a spelling the walk does not follow. Rather than
        // model every one, an unaccounted mention is unknown — except
        // under a head whose operands are text, where it is data.
        let unaccounted = statement_mentions_unaccounted_git_config(&parsed, &pairs);
        if config_ever_opaque || env_defines_opaque_git_alias(&pairs) || unaccounted {
            findings.push(DestructiveFinding {
                level: DestructivenessLevel::Destructive,
                reason: "git configuration comes from the environment; what it runs cannot be \
                         determined"
                    .to_string(),
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

/// Git subcommands paired with the short flags that make them
/// destructive. The text patterns only match a flag written on its
/// own (`git push -f`), but short options cluster: `git push -uf`
/// forces the same update and `git clean -df` deletes the same files.
/// Each entry is the subcommand, its destructive short flags, and the
/// long spellings of the same thing — `git push -u --force` never
/// writes `git push --force` adjacently, so the text patterns miss it.
const DESTRUCTIVE_GIT_FLAGS: &[(&str, &[char], &[&str])] = &[
    (
        "push",
        &['f'],
        &["force", "force-with-lease", "force-if-includes"],
    ),
    ("clean", &['f', 'd'], &["force"]),
    ("checkout", &['f'], &["force"]),
    ("branch", &['D'], &["delete", "force"]),
];

/// A destructive short flag reached through a cluster, e.g. the `f` in
/// `git push -uf origin main`.
///
/// `git` is looked for at any position, the same way shell payloads
/// are: `timeout 30 git push -uf …`, `sudo git push -uf …` and
/// `stdbuf -oL git clean -df` all run git behind a runner that no
/// list of wrappers will ever fully cover. A head whose operands are
/// text keeps its arguments inert, so `echo git push -uf` is not a
/// force push.
/// Every `git` token is tried, not just the first: an inert operand
/// spelled `git` sits in front of the real one in
/// `find … -exec echo git \; -exec git push -uf … \;`.
fn clustered_git_flag(invocation: &[String]) -> Option<String> {
    let tokens: Vec<String> = invocation.iter().map(|t| unquote_token(t)).collect();
    let head_is_data = tokens
        .first()
        .is_some_and(|t| DATA_COMMANDS.contains(&base_name(t).to_lowercase().as_str()));
    for (idx, token) in tokens.iter().enumerate() {
        if base_name(token).to_lowercase() != "git" {
            continue;
        }
        if head_is_data && !in_command_position(&tokens, idx) {
            continue;
        }
        if let Some(reason) = destructive_git_subcommand(&tokens[idx + 1..]) {
            return Some(reason);
        }
    }
    None
}

/// The first destructive subcommand + cluster pair in the tokens after
/// a `git`.
///
/// The subcommand is found by name rather than by counting past git's
/// global options: several of them take a separate operand
/// (`--git-dir DIR`, `--work-tree DIR`, `--namespace NAME`,
/// `-C dir`…), and mistaking one operand for the subcommand is what
/// let `git --git-dir .git push -uf …` through. Matching the name
/// needs no option grammar at all, and an operand that happens to be
/// spelled `push` only costs an over-block.
fn destructive_git_subcommand(after_git: &[String]) -> Option<String> {
    // `git --html-path push -uf` prints a path and exits — no
    // subcommand is dispatched, so nothing after the global runs.
    if dispatches_no_subcommand(after_git) {
        return None;
    }
    // The text patterns want `git reset --hard` written adjacently, so
    // any global between the two hides it. Rebuilding the invocation
    // from the command word — `git --no-pager reset --hard` becomes
    // `git reset --hard` — hands every git pattern the spelling it
    // expects, current and future, with no second table to keep in
    // step.
    if let Some(index) = git_command_index(after_git) {
        let canonical = format!("git {}", after_git[index..].join(" ")).to_lowercase();
        for pattern in DESTRUCTIVE_PATTERNS {
            if pattern.starts_with("git ") && canonical.contains(pattern) {
                return Some(format!("contains '{pattern}'"));
            }
        }
    }
    // From the command word onwards, so a global's operand is not read
    // as the subcommand: `git -C push grep -f …` greps in a directory
    // named `push`. When the command word cannot be located the whole
    // tail is scanned instead, which over-matches rather than missing.
    let (tail, only_command_word) = match git_command_index(after_git) {
        Some(index) => (&after_git[index..], true),
        None => (after_git, false),
    };
    if let Some(reason) = scan_git_subcommands(tail, only_command_word) {
        return Some(reason);
    }
    // Nothing literal. The command token may still be an alias defined
    // in this same command, which only expansion reveals.
    match expand_command_alias(after_git)? {
        AliasExpansion::Tokens(tokens) => scan_git_subcommands(&tokens, false),
        AliasExpansion::Unresolved => {
            Some("git alias cannot be resolved; what it runs cannot be determined".to_string())
        }
    }
}

/// With `only_command_word`, just the first token is treated as the
/// subcommand — `git grep push -f …` searches for the word `push`.
/// Without it the command word could not be located, so every token is
/// tried, which over-matches rather than missing.
fn scan_git_subcommands(after_git: &[String], only_command_word: bool) -> Option<String> {
    let candidates = if only_command_word {
        1
    } else {
        after_git.len()
    };
    for (idx, token) in after_git.iter().enumerate().take(candidates) {
        let subcommand = token.to_lowercase();
        let Some((_, flags, longs)) = DESTRUCTIVE_GIT_FLAGS
            .iter()
            .find(|(name, _, _)| *name == subcommand)
        else {
            continue;
        };
        for token in &after_git[idx + 1..] {
            // Past `--` everything is a pathspec, however it is
            // spelled: `git clean -n -- -dir` is a dry run against a
            // file named `-dir`, not a `-d` cluster.
            if token == "--" {
                break;
            }
            let Some(cluster) = token.strip_prefix('-') else {
                continue;
            };
            if let Some(long) = cluster.strip_prefix('-') {
                // `--force` need not sit next to the subcommand.
                // `--no-force` turns it back off, so only the
                // affirmative spelling counts.
                //
                // Git accepts any unambiguous abbreviation, so
                // `--force-w` is `--force-with-lease`. A written name
                // that merely *starts* a destructive option counts;
                // when the abbreviation is ambiguous git refuses the
                // command outright, so flagging it costs nothing.
                let name = long.split('=').next().unwrap_or(long).to_lowercase();
                if !name.is_empty()
                    && let Some(full) = longs.iter().find(|full| full.starts_with(&name))
                {
                    return Some(format!("git {subcommand} with '--{full}'"));
                }
                continue;
            }
            // `git branch -D` is the destructive spelling; `-d`
            // refuses to delete an unmerged branch. The text scan
            // lowercases and so flags both, and this must not go the
            // other way and miss the upper-case one.
            if let Some(found) = cluster
                .chars()
                .find(|c| flags.contains(c) || (flags.contains(&'D') && *c == 'd'))
            {
                return Some(format!("git {subcommand} with '-{found}'"));
            }
        }
    }
    None
}

/// Git global options that take a separate operand, so the token
/// after them is not the command.
const GIT_GLOBAL_OPERAND_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
    "--config-env",
];

/// Git globals that print something and exit without dispatching a
/// subcommand, so nothing after them runs.
/// Bare `--exec-path` belongs here too: only the attached
/// `--exec-path=<path>` form configures a command that then runs.
const GIT_REPORT_ONLY_GLOBALS: &[&str] = &[
    "--html-path",
    "--man-path",
    "--info-path",
    "--version",
    "--help",
    "--exec-path",
    // Git documents the short forms in its own usage line.
    "-v",
    "-h",
];

/// How many alias hops to follow. Aliases can name other aliases.
const MAX_GIT_ALIAS_DEPTH: usize = 8;

/// The command's statements, split on the separators that end one:
/// `;`, `&&`, `||`, `|`, `&` and newlines. Quoted separators are not
/// separators, so a `;` inside an argument keeps its statement whole,
/// and neither are separators nested inside `$( … )`, backticks or a
/// subshell — `GIT_CONFIG_GLOBAL=$(printf /tmp/g; true) git p` is one
/// statement whose assignment and command belong together.
///
/// A prefix assignment applies to the statement it introduces and no
/// other, so environment questions are answered per statement rather
/// than over the whole script.
fn shell_statements(raw: &str) -> Vec<String> {
    /// One level of nesting. Quoting restarts inside a substitution,
    /// so the contexts have to stack rather than toggle: in
    /// `"$(printf "/tmp/g;")"` the inner quote closes the inner
    /// string, not the outer one, and the `;` is nested throughout.
    enum Context {
        Single,
        Double,
        /// `$( … )` / `<( … )`: the result joins the word around it.
        Substitution,
        /// `( … )`: a compound command whose `)` ends the word.
        Subshell,
        Brace,
        Backtick,
    }
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut stack: Vec<Context> = Vec::new();
    let mut at_word_start = true;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        let word_start = at_word_start;
        at_word_start = c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(');
        // A separator inside a comment separates nothing: bash reads
        // to the end of the line and forgets it.
        if word_start
            && c == '#'
            && !matches!(stack.last(), Some(Context::Single | Context::Double))
        {
            while chars.peek().is_some_and(|next| *next != '\n') {
                chars.next();
            }
            continue;
        }
        if matches!(stack.last(), Some(Context::Single)) {
            current.push(c);
            if c == '\'' {
                stack.pop();
            }
            continue;
        }
        let in_double = matches!(stack.last(), Some(Context::Double));
        match c {
            '\\' => {
                current.push(c);
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            '\'' if !in_double => {
                stack.push(Context::Single);
                current.push(c);
            }
            '"' => {
                if in_double {
                    stack.pop();
                } else {
                    stack.push(Context::Double);
                }
                current.push(c);
            }
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                stack.push(Context::Substitution);
                current.push('$');
                current.push('(');
            }
            '`' => {
                if matches!(stack.last(), Some(Context::Backtick)) {
                    stack.pop();
                } else {
                    stack.push(Context::Backtick);
                }
                current.push(c);
            }
            '<' | '>' if !in_double && chars.peek() == Some(&'(') => {
                chars.next();
                stack.push(Context::Substitution);
                current.push(c);
                current.push('(');
            }
            '(' if !in_double => {
                stack.push(Context::Subshell);
                current.push(c);
            }
            '{' if !in_double => {
                stack.push(Context::Brace);
                current.push(c);
            }
            ')' if matches!(
                stack.last(),
                Some(Context::Substitution | Context::Subshell)
            ) =>
            {
                // A subshell's `)` ends the word, so a `#` after it
                // starts a comment; a substitution's result joins the
                // word and does not.
                if matches!(stack.pop(), Some(Context::Subshell)) {
                    at_word_start = true;
                }
                current.push(c);
            }
            '}' if matches!(stack.last(), Some(Context::Brace)) => {
                stack.pop();
                current.push(c);
            }
            ';' | '\n' | '|' | '&' if stack.is_empty() => {
                // Consume the second character of `&&` and `||`.
                if (c == '|' || c == '&') && chars.peek() == Some(&c) {
                    chars.next();
                }
                statements.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    statements.push(current);
    statements
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// Builtins that set a variable for everything that follows rather
/// than for one command.
/// `set` is deliberately absent: its trailing operands become
/// positional parameters, so `set -- FOO=bar` assigns nothing.
///
/// Only `export` exports on its own. The declaration builtins make a
/// shell variable that a child never sees unless `-x` is given, so
/// they are listed separately.
const EXPORTING_BUILTINS: &[&str] = &["export"];

/// Builtins that declare a variable but export it only with `-x`.
const DECLARING_BUILTINS: &[&str] = &["declare", "typeset", "readonly", "local"];

/// The head a statement runs, with builtin wrappers and their options
/// (`command -p export …`) skipped.
fn effective_head(words: &[String]) -> Option<String> {
    words
        .iter()
        .map(|word| base_name(word))
        .find(|word| !BUILTIN_PREFIXES.contains(&word.as_str()) && !word.starts_with('-'))
}

/// True when the head assigns a shell variable, whether or not it
/// exports one.
fn declares_variables(words: &[String]) -> bool {
    effective_head(words).is_some_and(|head| {
        EXPORTING_BUILTINS.contains(&head.as_str()) || DECLARING_BUILTINS.contains(&head.as_str())
    })
}

/// True when `invocation`'s head declares *and* exports: `export …`,
/// or a declaration builtin given `-x`.
fn exports_variables(words: &[String]) -> bool {
    let Some(head) = effective_head(words) else {
        return false;
    };
    if EXPORTING_BUILTINS.contains(&head.as_str()) {
        return true;
    }
    DECLARING_BUILTINS.contains(&head.as_str())
        && words.iter().any(|word| {
            word.strip_prefix('-')
                .is_some_and(|cluster| !cluster.starts_with('-') && cluster.contains('x'))
        })
}

/// Environment variables that decide where git finds its
/// configuration, and so what aliases it has.
const GIT_CONFIG_SELECTORS: &[&str] = &[
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "HOME",
    "XDG_CONFIG_HOME",
];

/// True when a locale-translated string in this command could decide
/// what runs.
///
/// Under a head whose operands are text — `printf '%s\n' $"hello"`,
/// `grep $"message" log.txt` — the executable is known whatever the
/// catalogue says, and the translation is only the data being printed
/// or matched. That exemption does not extend to a token carrying a
/// substitution, which runs a command of its own.
///
/// Anything the invocations do not account for (a heredoc delimiter,
/// a command that did not parse) keeps the refusal, since it cannot
/// be shown to be data.
fn translation_can_reach_a_command(cmd: &ParsedCommand) -> bool {
    let accounted: Vec<bool> = cmd
        .invocations
        .iter()
        .map(|invocation| {
            // `env printf …` prints just as `printf …` does, so the
            // wrapper comes off before the head is read.
            let unwrapped = unwrapped_argv(invocation);
            let invocation = unwrapped.as_ref().unwrap_or(invocation);
            // A head that is itself a translation cannot vouch for
            // anything: `$"cat" file.txt` reads as `cat` only until
            // the catalogue says otherwise.
            let head_is_data = invocation.first().is_some_and(|t| {
                DATA_COMMANDS.contains(&base_name(&unquote_token(t)).to_lowercase().as_str())
                    && !shell_text_facts(t).locale_quote
            });
            let has_substitution = invocation
                .iter()
                .any(|token| token.contains("$(") || token.contains('`'));
            // The parser can split `$\"…\"` into a `$` token and a
            // string token, so the invocation is read as one piece of
            // text rather than token by token.
            let carries_translation = shell_text_facts(&invocation.join("")).locale_quote;
            carries_translation && (!head_is_data || has_substitution)
        })
        .collect();
    if accounted.iter().any(|reaches| *reaches) {
        return true;
    }
    // No invocation's translation can reach a command. If none of
    // them carries one at all, the translation the text pass saw lies
    // somewhere the invocations do not cover — a heredoc delimiter, a
    // command that did not parse — and that cannot be shown to be
    // data.
    !cmd.invocations
        .iter()
        .any(|invocation| shell_text_facts(&invocation.join("")).locale_quote)
}

/// What a quote-aware pass over the command text found.
///
/// One pass answers both questions, so a construct either scanner
/// would have missed — a substitution that restarts quoting, a
/// comment — cannot be handled correctly by one and not the other.
struct ShellTextFacts {
    /// `&&`, `||` or a heredoc as an actual shell construct.
    control_operator: bool,
    /// A locale-translated `$"…"` string, whose content the catalogue
    /// decides rather than the command text.
    locale_quote: bool,
}

/// Read `raw` the way the shell reads it: quotes nest, a substitution
/// restarts quoting inside itself, and a `#` beginning a word starts a
/// comment.
fn shell_text_facts(raw: &str) -> ShellTextFacts {
    enum Context {
        Single,
        /// `$'…'`, where a backslash escapes the closing quote.
        AnsiC,
        Double,
        /// `$( … )` and `<( … )`, whose result joins the surrounding
        /// word.
        Substitution,
        /// `( … )`, a compound command that ends the word.
        Subshell,
        Brace,
        Backtick,
    }
    let mut facts = ShellTextFacts {
        control_operator: false,
        locale_quote: false,
    };
    let text: Vec<char> = raw.chars().collect();
    let mut stack: Vec<Context> = Vec::new();
    let mut at_word_start = true;
    // Heredocs declared on the current line, in the order their
    // bodies will follow it.
    let mut pending_heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        // The line has ended and a heredoc was declared on it: its
        // body is data the command receives, so it is skipped whole.
        if c == '\n' && !pending_heredocs.is_empty() {
            for (delimiter, strip_tabs) in std::mem::take(&mut pending_heredocs) {
                i = skip_heredoc_body(&text, i, &delimiter, strip_tabs);
            }
            at_word_start = true;
            i += 1;
            continue;
        }
        let word_start = at_word_start;
        // A closing `)` does not end the word when it closes a
        // substitution: bash joins what follows onto it, so the `#` in
        // `echo $(true)#x` is a literal character.
        at_word_start = c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(');
        let peek = text.get(i + 1).copied();
        if matches!(stack.last(), Some(Context::Single)) {
            if c == '\'' {
                stack.pop();
            }
            i += 1;
            continue;
        }
        // Inside `$'…'` a backslash escapes the next character, so an
        // escaped quote does not close the literal.
        if matches!(stack.last(), Some(Context::AnsiC)) {
            match c {
                '\\' => i += 1,
                '\'' => {
                    stack.pop();
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        let in_double = matches!(stack.last(), Some(Context::Double));
        match c {
            '\\' => i += 1,
            '\'' if !in_double => stack.push(Context::Single),
            '"' => {
                if in_double {
                    stack.pop();
                } else {
                    stack.push(Context::Double);
                }
            }
            // `$(` restarts quoting inside the substitution; `$"`
            // opens a translated string, but only where a `$` is
            // special — inside double quotes it is literal text.
            '$' => match peek {
                Some('(') => {
                    i += 1;
                    stack.push(Context::Substitution);
                }
                Some('\'') if !in_double => {
                    i += 1;
                    stack.push(Context::AnsiC);
                }
                Some('"') if !in_double => facts.locale_quote = true,
                _ => {}
            },
            '`' => {
                if matches!(stack.last(), Some(Context::Backtick)) {
                    stack.pop();
                } else {
                    stack.push(Context::Backtick);
                }
            }
            // A heredoc body is data the shell hands to a command, not
            // syntax: nothing in it is translated or executed, so it
            // is skipped whole.
            // A heredoc: `<<` but not `<<<`, which is a here-string
            // with no body at all. The declaration line keeps being
            // scanned — commands can follow the redirect — and only
            // the body, which is data rather than syntax, is skipped
            // when the line ends.
            '<' if !in_double && peek == Some('<') => {
                // The whole run of `<` is consumed at once, or the
                // tail of `<<<` would be read as another `<<`.
                let mut run = 0;
                while text.get(i + run) == Some(&'<') {
                    run += 1;
                }
                if run == 2 {
                    facts.control_operator = true;
                    let (word, strip_tabs, translated, next) =
                        read_heredoc_delimiter(&text, i + run);
                    // A translated delimiter is decided by the
                    // catalogue, so where the body ends — and what
                    // between here and there is code — is unknown.
                    // Only an active `$\"` counts: inside other
                    // quoting those are literal characters.
                    if translated {
                        facts.locale_quote = true;
                    }
                    let delimiter = unquote_token(&word);
                    if !delimiter.is_empty() {
                        pending_heredocs.push((delimiter, strip_tabs));
                    }
                    i = next.saturating_sub(1);
                } else {
                    // `<<<` is a here-string: one line, no body.
                    i += run - 1;
                }
            }
            // `<( … )` and `>( … )` are process substitutions: the
            // result is a pathname that joins the surrounding word,
            // exactly as `$( … )` does.
            '<' | '>' if !in_double && peek == Some('(') => {
                i += 1;
                stack.push(Context::Substitution);
            }
            '(' if !in_double => stack.push(Context::Subshell),
            '{' if !in_double => stack.push(Context::Brace),
            ')' if matches!(
                stack.last(),
                Some(Context::Substitution | Context::Subshell)
            ) =>
            {
                // A substitution's result joins the word around it; a
                // subshell is a compound command, and what follows its
                // `)` starts a new word — so `(true)# …` is a comment
                // while `$(true)#x` is not.
                if matches!(stack.pop(), Some(Context::Subshell)) {
                    at_word_start = true;
                }
            }
            '}' if matches!(stack.last(), Some(Context::Brace)) => {
                stack.pop();
            }
            '#' if word_start && !in_double => {
                while text.get(i + 1).is_some_and(|next| *next != '\n') {
                    i += 1;
                }
            }
            '&' | '|' if !in_double && peek == Some(c) => {
                facts.control_operator = true;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    facts
}

/// Read a heredoc's delimiter word, given the index just past its
/// `<<`. Returns the word as written, whether the operator was `<<-`
/// (which strips leading tabs from the terminator), whether the word
/// contains an *active* `$\"…\"` translation, and the index just past
/// the word.
fn read_heredoc_delimiter(text: &[char], mut i: usize) -> (String, bool, bool, usize) {
    let strip_tabs = text.get(i) == Some(&'-');
    if strip_tabs {
        i += 1;
    }
    while text.get(i).is_some_and(|c| *c == ' ' || *c == '\t') {
        i += 1;
    }
    // The word is read whole and then unquoted the way the shell
    // unquotes it: `<<$'EOF'` ends at a line reading `EOF`, not
    // `$EOF`. Quotes inside the word do not end it.
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut translated = false;
    while let Some(&c) = text.get(i) {
        match quote {
            Some(open) => {
                // Inside double quotes a backslash escapes the next
                // character, so `"X\""` stays quoted through it.
                if c == '\\' && open == '"' {
                    word.push(c);
                    if let Some(&escaped) = text.get(i + 1) {
                        word.push(escaped);
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                if c == open {
                    quote = None;
                }
                word.push(c);
            }
            None => {
                // A backslash escapes the next character, whitespace
                // included: `<<EO\ F` ends at a line reading `EO F`.
                if c == '\\' {
                    word.push(c);
                    if let Some(&escaped) = text.get(i + 1) {
                        word.push(escaped);
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                if c.is_whitespace() || matches!(c, ';' | '&' | '|' | ')' | '<' | '>') {
                    break;
                }
                if matches!(c, '\'' | '"') {
                    quote = Some(c);
                }
                if c == '$' && text.get(i + 1) == Some(&'"') {
                    translated = true;
                }
                word.push(c);
            }
        }
        i += 1;
    }
    (word, strip_tabs, translated, i)
}

/// Skip a heredoc body, given the index of the newline that ends its
/// declaration line. Returns the index of the newline ending the
/// terminator line, or the last index when the body is unterminated.
///
/// The terminator is a line holding the delimiter and nothing else;
/// `<<-` allows leading *tabs* before it, and nothing else does — so
/// an indented line is body text.
fn skip_heredoc_body(text: &[char], mut i: usize, delimiter: &str, strip_tabs: bool) -> usize {
    while i < text.len() {
        let line_start = i + 1;
        let mut line_end = line_start;
        while text.get(line_end).is_some_and(|c| *c != '\n') {
            line_end += 1;
        }
        let line: String = text[line_start.min(text.len())..line_end.min(text.len())]
            .iter()
            .collect();
        let candidate = if strip_tabs {
            line.trim_start_matches('\t')
        } else {
            line.as_str()
        };
        if candidate == delimiter {
            return line_end.saturating_sub(1);
        }
        if line_end >= text.len() {
            return text.len();
        }
        i = line_end;
    }
    text.len()
}

/// True when the command does something the statement walk cannot
/// follow: a branch whose statements may not run, a heredoc whose body
/// is data rather than statements, or a `set` carrying operands whose
/// effect on `allexport` cannot be read off.
///
/// Only structure is judged here. Whether it matters is
/// [`carries_git_config`]'s question, so an ordinary `git status &&
/// git log` is untouched.
fn state_is_unresolvable(raw: &str, statements: &[String]) -> bool {
    if shell_text_facts(raw).control_operator {
        return true;
    }
    statements.iter().any(|statement| {
        let words = split_alias_value(statement);
        if words.first().map(|word| base_name(word)).as_deref() != Some("set") {
            return false;
        }
        let mut index = 1;
        while let Some(word) = words.get(index) {
            let Some(cluster) = word.strip_prefix(['-', '+']) else {
                return true;
            };
            // `-o NAME` names a shell option; the name is its operand,
            // not a positional argument.
            index += if cluster == "o" { 2 } else { 1 };
        }
        false
    })
}

/// True when some statement assigns a variable that selects git's
/// configuration *and* is a candidate for outliving its statement —
/// a bare assignment or a declaration, not a command prefix, which the
/// shell scopes to the command it introduces.
fn carries_git_config(statements: &[String]) -> bool {
    statements.iter().any(|statement| {
        let Some(parsed) = parse_bash(statement) else {
            return false;
        };
        let words = split_alias_value(statement);
        if !parsed.invocations.is_empty() && !declares_variables(&words) {
            return false;
        }
        let mut assigned: Vec<(String, String)> = Vec::new();
        for (name, value) in &parsed.assignments {
            push_assignment(&mut assigned, &format!("{name}={}", unquote_token(value)));
        }
        for word in &words {
            push_assignment(&mut assigned, word);
        }
        assigned.iter().any(|(name, _)| {
            GIT_CONFIG_SELECTORS.contains(&name.as_str()) || name.starts_with("GIT_CONFIG")
        })
    })
}

/// Whether a statement switches `allexport` on or off — `set -a` /
/// `set -o allexport` and their `+` counterparts, which turn it back
/// off. `None` when the statement says nothing about it.
fn allexport_transition(parsed: &ParsedCommand) -> Option<bool> {
    // `set +a -a` is one command with two toggles; the last one wins.
    let mut state = None;
    for invocation in &parsed.invocations {
        let mut words = invocation.iter().map(|t| unquote_token(t));
        if words.next().map(|w| base_name(&w)).as_deref() != Some("set") {
            continue;
        }
        let rest: Vec<String> = words.collect();
        for (index, word) in rest.iter().enumerate() {
            // `set -a -- +a` ends option parsing: `+a` is a positional
            // argument, not a toggle.
            if word == "--" {
                break;
            }
            let Some(cluster) = word.strip_prefix(['-', '+']) else {
                // The first operand ends option processing.
                break;
            };
            let on = word.starts_with('-');
            if cluster == "o" {
                if rest.get(index + 1).is_some_and(|next| next == "allexport") {
                    state = Some(on);
                }
                continue;
            }
            if !cluster.starts_with(['-', '+']) && cluster.contains('a') {
                state = Some(on);
            }
        }
    }
    state
}

/// Apply one statement to the shell state a later git would inherit:
/// each variable's latest value, and which names carry the export
/// attribute.
///
/// A command prefix (`FOO=bar cmd`) is scoped to that command by the
/// shell, so it changes nothing here. A bare `FOO=bar` statement makes
/// a shell variable, which a child sees only once the name is
/// exported — by `export`, by a declaration builtin with `-x`, or by
/// `allexport`. The attribute sticks to the name, so a later plain
/// assignment to an already-exported variable reaches git too.
fn apply_statement_env(
    statement: &str,
    parsed: &ParsedCommand,
    allexport: bool,
    shell_vars: &mut Vec<(String, String)>,
    exported: &mut Vec<String>,
) {
    // `command -v export FOO=bar` prints a description of `export` and
    // exports nothing.
    if parsed.invocations.iter().any(|invocation| {
        wrapper_only_reports(
            &invocation
                .iter()
                .map(|t| unquote_token(t))
                .collect::<Vec<_>>(),
        )
    }) {
        return;
    }
    let words = split_alias_value(statement);
    // `export FOO=bar` parses as a declaration rather than a command,
    // so the statement's own first word answers for that spelling;
    // `command export FOO=bar` arrives as an invocation instead.
    let exports = exports_variables(&words)
        || parsed.invocations.iter().any(|invocation| {
            exports_variables(
                &invocation
                    .iter()
                    .map(|t| unquote_token(t))
                    .collect::<Vec<_>>(),
            )
        });
    // A declaration builtin assigns whether or not it exports: after
    // `declare FOO=/tmp/g`, a later `export FOO` sends that value on.
    // `-x` decides the attribute, not whether the value is recorded.
    let declares = exports
        || declares_variables(&words)
        || parsed.invocations.iter().any(|invocation| {
            declares_variables(
                &invocation
                    .iter()
                    .map(|t| unquote_token(t))
                    .collect::<Vec<_>>(),
            )
        });
    let mut assigned: Vec<(String, String)> = Vec::new();
    if declares || parsed.invocations.is_empty() {
        for (name, value) in &parsed.assignments {
            push_assignment(&mut assigned, &format!("{name}={}", unquote_token(value)));
        }
    }
    if declares {
        for invocation in &parsed.invocations {
            for token in invocation {
                push_assignment(&mut assigned, &unquote_token(token));
            }
        }
    }
    // `export FOO` with no value exports a variable assigned earlier,
    // so the name alone carries the attribute. A declaration without
    // `-x` grants nothing, hence `exports` rather than `declares`.
    if exports {
        for word in words.iter().skip(1) {
            if !word.contains('=')
                && !word.starts_with(['-', '+'])
                && is_env_assignment(&format!("{word}=x"))
                && !exported.contains(word)
            {
                exported.push(word.clone());
            }
        }
    }
    for (name, value) in assigned {
        match shell_vars.iter_mut().find(|(known, _)| *known == name) {
            Some(slot) => slot.1 = value,
            None => shell_vars.push((name.clone(), value)),
        }
        if (exports || allexport) && !exported.contains(&name) {
            exported.push(name);
        }
    }
}

/// True when a statement names a `GIT_CONFIG…` variable that the
/// positional walk did not collect, so what git will be configured
/// with cannot be said.
///
/// The exception is a head whose operands are text: `printf '%s\n'
/// 'GIT_CONFIG_KEY_0=alias.p' git` prints the string and sets nothing.
fn statement_mentions_unaccounted_git_config(
    parsed: &ParsedCommand,
    collected: &[(String, String)],
) -> bool {
    parsed.invocations.iter().any(|invocation| {
        let head_is_data = invocation.first().is_some_and(|t| {
            DATA_COMMANDS.contains(&base_name(&unquote_token(t)).to_lowercase().as_str())
        });
        if head_is_data {
            return false;
        }
        invocation.iter().skip(1).any(|token| {
            split_alias_value(&unquote_token(token)).iter().any(|word| {
                let name = word.split('=').next().unwrap_or(word);
                name.starts_with("GIT_CONFIG") && !collected.iter().any(|(seen, _)| seen == name)
            })
        })
    })
}

/// Short options of `env` that take a separate operand, so the token
/// after them is not the command and not an assignment.
const ENV_OPERAND_OPTIONS: &[&str] = &["-u", "--unset", "-C", "--chdir"];

/// The environment assignments a runner invocation applies, with shell
/// quoting removed.
///
/// Only where a runner would actually apply them. A `NAME=VALUE`
/// string among the operands of an ordinary command is data:
/// `printf '%s\n' 'GIT_CONFIG_KEY_0=alias.p' git` prints two words and
/// sets nothing.
fn runner_env_pairs(invocation: &[String]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let head = invocation
        .first()
        .map(|t| base_name(&unquote_token(t)).to_lowercase());
    if !head.is_some_and(|h| COMMAND_RUNNER_WRAPPERS.contains(&h.as_str())) {
        return pairs;
    }
    let mut idx = 1;
    while let Some(token) = invocation.get(idx) {
        let unquoted = unquote_token(token);
        // `env -S 'FOO=bar git p'` packs the assignments into one
        // word, and the wrapper parser consumes them rather than
        // handing them back, so neither view lists them separately.
        // Split that payload the way env does.
        if let Some(payload) = split_string_payload(&unquoted, invocation.get(idx + 1)) {
            // env's own splitting rules, not an approximation of them:
            // an unquoted `\_` separates words while a quoted one is a
            // literal space, and getting that backwards merges the
            // assignments into a single meaningless one.
            let words = env_split_string(&payload).unwrap_or_else(|| split_alias_value(&payload));
            for word in words {
                push_assignment(&mut pairs, &word);
            }
            idx += 2;
            continue;
        }
        if unquoted.starts_with('-') {
            // An option with a separate operand takes the next token
            // with it — `env -u X GIT_CONFIG_GLOBAL=… git p` would
            // otherwise read `X` as the command and stop.
            idx += if ENV_OPERAND_OPTIONS.contains(&unquoted.as_str()) {
                2
            } else {
                1
            };
            continue;
        }
        if !push_assignment(&mut pairs, &unquoted) {
            // Another wrapper: its own operands are still environment
            // for whatever runs at the end of the chain, so keep
            // walking. `env env FOO=bar git p` sets FOO.
            if COMMAND_RUNNER_WRAPPERS.contains(&base_name(&unquoted).to_lowercase().as_str()) {
                idx += 1;
                continue;
            }
            // The command word: later operands belong to it.
            break;
        }
        idx += 1;
    }
    pairs
}

/// Record `word` as an environment assignment. A name carrying an
/// expansion counts too — `GIT_CONFIG_KEY_$I=alias.p` names a real
/// config key once the shell is done with it, and only the classifier
/// downstream can decide what that means.
fn push_assignment(pairs: &mut Vec<(String, String)>, word: &str) -> bool {
    let Some((name, value)) = word.split_once('=') else {
        return false;
    };
    // `NAME+=value` appends, and appending to an unset variable just
    // sets it. The `+` is part of the operator, not of the name.
    let name = name.strip_suffix('+').unwrap_or(name);
    let canonical = format!("{name}={value}");
    if name.is_empty() || !(is_env_assignment(&canonical) || name.contains(['$', '`'])) {
        return false;
    }
    pairs.push((name.to_string(), unquote_token(value)));
    true
}

/// The operand of `env -S` / `--split-string`, in any of its
/// spellings — including `-S` reached through a short-option cluster
/// (`-vS'…'`), where the payload is whatever follows the `S`.
fn split_string_payload(token: &str, next: Option<&String>) -> Option<String> {
    if let Some(long) = token.strip_prefix("--") {
        let (name, inline) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (long, None),
        };
        if name.is_empty() || !"split-string".starts_with(name) {
            return None;
        }
        return inline
            .map(str::to_string)
            .or_else(|| next.map(|t| unquote_token(t)));
    }
    let cluster = token.strip_prefix('-')?;
    let position = cluster.find('S')?;
    let attached = &cluster[position + 1..];
    if attached.is_empty() {
        next.map(|t| unquote_token(t))
    } else {
        Some(attached.to_string())
    }
}

/// True when a prefix assignment hands git configuration through the
/// environment in a way that could define an alias:
/// `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p GIT_CONFIG_VALUE_0='push
/// --force' git p` runs a force push that nothing in the argv shows.
///
/// Only alias-defining or unreadable keys count; ordinary env config
/// (`GIT_CONFIG_KEY_0=user.name`) says nothing about what runs.
fn env_defines_opaque_git_alias(assignments: &[(String, String)]) -> bool {
    assignments.iter().any(|(name, value)| {
        // Environment names are case-sensitive: `home=/tmp/h` sets an
        // unrelated variable that git never reads.
        let name = name.clone();
        let dynamic_value = value.contains('$') || value.contains('`');
        let names_an_alias = value.to_lowercase().starts_with("alias.");
        // A name the shell still has to expand could be any variable,
        // including one that configures git: `env "$K=/tmp/g" git p`
        // sets whatever `K` holds.
        if name.contains(['$', '`']) {
            return true;
        }
        if name == "GIT_CONFIG_PARAMETERS" {
            // Each parameter is `'key'='value'`; the key is what
            // decides, and `user.alias.foo` is not an alias.
            return dynamic_value
                || split_alias_value(value)
                    .iter()
                    .flat_map(|pair| pair.split('=').next())
                    .any(config_key_is_opaque);
        }
        // A config *file* can define aliases too, and its contents are
        // not in the command at all. `/dev/null` and an empty path are
        // the documented way to ask for no config, and define nothing.
        // `HOME` and `XDG_CONFIG_HOME` select where git looks for its
        // ordinary config, so redirecting them supplies aliases just
        // as naming a config file does.
        if matches!(
            name.as_str(),
            "GIT_CONFIG_GLOBAL" | "GIT_CONFIG_SYSTEM" | "GIT_CONFIG" | "HOME" | "XDG_CONFIG_HOME"
        ) {
            return !matches!(value.as_str(), "/dev/null" | "");
        }
        // A name the shell still has to expand cannot be ruled out:
        // `GIT_CONFIG_KEY_$I=alias.p` is `GIT_CONFIG_KEY_0` by the time
        // git reads it, and `$K=alias.p` could be anything at all.
        if name.contains(['$', '`']) {
            return names_an_alias || dynamic_value || name.starts_with("GIT_CONFIG");
        }
        match name.strip_prefix("GIT_CONFIG_KEY_") {
            Some(index) if index.chars().all(|c| c.is_ascii_digit()) => {
                dynamic_value || names_an_alias || config_key_is_opaque(value)
            }
            _ => false,
        }
    })
}

/// Config keys that can make git run something the command text does
/// not show: an alias *is* a command, and an include names a file that
/// can define one. Every carrier — `-c`, `--config-env`,
/// `GIT_CONFIG_KEY_<n>`, `GIT_CONFIG_PARAMETERS` — is asked this one
/// question, so a new carrier cannot be given a weaker rule by
/// accident.
fn config_key_is_opaque(key: &str) -> bool {
    let key = key.trim_matches(|c| c == '\'' || c == '"').to_lowercase();
    ["alias.", "include.", "includeif."]
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

/// The alias name a config key defines, if it defines one:
/// `alias.p=A` yields `p`. Section names are case-insensitive.
fn alias_key_name(definition: &str) -> Option<String> {
    const PREFIX: &str = "alias.";
    if !definition
        .get(..PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(PREFIX))
    {
        return None;
    }
    let name = definition[PREFIX.len()..].split('=').next()?;
    (!name.is_empty()).then(|| name.to_lowercase())
}

/// Index of the git command word — the first token that is neither a
/// global option nor the operand of one. `None` when a global makes
/// git print and exit, or when no command word follows.
fn git_command_index(tokens: &[String]) -> Option<usize> {
    let mut idx = 0;
    while let Some(token) = tokens.get(idx) {
        if !token.starts_with('-') {
            return Some(idx);
        }
        if GIT_REPORT_ONLY_GLOBALS.contains(&token.as_str()) {
            return None;
        }
        idx += if GIT_GLOBAL_OPERAND_OPTIONS.contains(&token.as_str()) {
            2
        } else {
            1
        };
    }
    None
}

/// True when a leading global stops git before it dispatches anything,
/// so no later token is a command however it is spelled.
fn dispatches_no_subcommand(tokens: &[String]) -> bool {
    let mut idx = 0;
    while let Some(token) = tokens.get(idx) {
        if !token.starts_with('-') {
            return false;
        }
        if GIT_REPORT_ONLY_GLOBALS.contains(&token.as_str()) {
            return true;
        }
        idx += if GIT_GLOBAL_OPERAND_OPTIONS.contains(&token.as_str()) {
            2
        } else {
            1
        };
    }
    false
}

/// Outcome of resolving the git command token through inline aliases.
enum AliasExpansion {
    /// The fully expanded token list.
    Tokens(Vec<String>),
    /// The chain was still unwinding when the budget ran out, so what
    /// git would run is unknown.
    Unresolved,
}

/// Split an alias value the way git does before scanning it: words
/// separated by unquoted whitespace, with quotes and backslashes
/// removed. `alias.p=push "--force"` runs `git push --force`, so
/// leaving the quotes in the word would hide the option.
fn split_alias_value(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    current.push(c);
                }
            }
            Some(_) => match c {
                '"' => quote = None,
                '\\' => match chars.next() {
                    Some(escaped) => current.push(escaped),
                    None => break,
                },
                _ => current.push(c),
            },
            None => match c {
                '\'' | '"' => {
                    started = true;
                    quote = Some(c);
                }
                '\\' => {
                    started = true;
                    match chars.next() {
                        Some(escaped) => current.push(escaped),
                        None => break,
                    }
                }
                c if c.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => {
                    started = true;
                    current.push(c);
                }
            },
        }
    }
    if started {
        words.push(current);
    }
    words
}

/// The tokens with the git *command* replaced by what an inline
/// `-c alias.NAME=VALUE` defines it as: `git -c alias.p=push p -uf …`
/// runs `git push -uf …`, and the alias name alone matches no
/// subcommand. `None` when nothing expanded.
///
/// Only the command token is rewritten. Substituting every matching
/// token would turn the pathspec in `git -c 'alias.p=push --force'
/// status p` into a force push that never runs.
///
/// Only aliases defined in the same command are resolvable — one from
/// the user's config is invisible here, and the raw scans remain the
/// only cover for that.
fn expand_command_alias(tokens: &[String]) -> Option<AliasExpansion> {
    let mut aliases: Vec<(String, Vec<String>)> = Vec::new();
    // Aliases whose value cannot be read from the command text:
    // `--config-env=alias.p=A` takes it from the environment, and a
    // value carrying an expansion is decided at run time.
    let mut opaque: Vec<String> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if let Some(definition) = token.strip_prefix("--config-env=").or_else(|| {
            (token == "--config-env")
                .then(|| tokens.get(i + 1))?
                .map(String::as_str)
        }) {
            // The key's own spelling can be expanded, and then which
            // command it binds to is decided by the shell, not here.
            if definition.contains(['$', '`']) {
                return Some(AliasExpansion::Unresolved);
            }
            // An include names a file that can define any alias, so
            // which command word it binds to is unknowable here.
            if config_key_is_opaque(definition) && alias_key_name(definition).is_none() {
                return Some(AliasExpansion::Unresolved);
            }
            if let Some(name) = alias_key_name(definition) {
                opaque.push(name);
            }
        }
        // `-c alias.p=push` arrives as two tokens; `-calias.p=push`
        // as one.
        let body = token.strip_prefix("-c").unwrap_or(token);
        // An include supplied the same way pulls in aliases this scan
        // cannot read.
        if config_key_is_opaque(body) && alias_key_name(body).is_none() {
            return Some(AliasExpansion::Unresolved);
        }
        // Git config section and key names are case-insensitive, so
        // `Alias.p` defines the same alias as `alias.p`.
        const PREFIX: &str = "alias.";
        if !body
            .get(..PREFIX.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(PREFIX))
        {
            continue;
        }
        let definition = &body[PREFIX.len()..];
        if let Some((name, value)) = definition.split_once('=')
            && !name.is_empty()
        {
            if name.contains(['$', '`']) {
                // Expansion decides which command the alias binds to,
                // so no command word in this invocation can be ruled
                // out: `git -c alias.${X:+p}='push --force' p` defines
                // `p` only once the shell is done.
                return Some(AliasExpansion::Unresolved);
            }
            if value.contains('$') || value.contains('`') {
                // Run-time expansion decides the value.
                opaque.push(name.to_lowercase());
            }
            aliases.push((name.to_lowercase(), split_alias_value(value)));
        }
    }
    if aliases.is_empty() && opaque.is_empty() {
        return None;
    }
    // Git takes the last `-c` value for a repeated key, so lookups run
    // backwards: `-c alias.p=status -c 'alias.p=push --force'` defines
    // `p` as the force push.
    let lookup = |name: &str| {
        aliases
            .iter()
            .rev()
            .find(|(alias, _)| alias == name)
            .map(|(_, expansion)| expansion.clone())
    };

    let idx = git_command_index(tokens)?;
    let command = tokens.get(idx)?.to_lowercase();
    // A command that an opaque definition names runs something this
    // check cannot read, so it must not be treated as safe.
    if opaque.contains(&command) {
        return Some(AliasExpansion::Unresolved);
    }
    let mut expansion = lookup(&command)?;
    // An alias can name another alias. Follow the chain, refusing to
    // revisit a name so a cycle cannot spin.
    let mut seen = vec![command];
    let mut resolved = false;
    for _ in 0..MAX_GIT_ALIAS_DEPTH {
        let Some(head) = expansion.first().map(|t| t.to_lowercase()) else {
            resolved = true;
            break;
        };
        if opaque.contains(&head) {
            return Some(AliasExpansion::Unresolved);
        }
        if seen.contains(&head) || lookup(&head).is_none() {
            resolved = true;
            break;
        }
        let next = lookup(&head)?;
        seen.push(head);
        expansion.splice(0..1, next);
    }
    if !resolved {
        // Still unwinding when the budget ran out: what git ends up
        // running is unknown, and unknown must not mean allowed.
        return Some(AliasExpansion::Unresolved);
    }
    expansion.extend_from_slice(&tokens[idx + 1..]);
    Some(AliasExpansion::Tokens(expansion))
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
/// Interpreters that can run a command are deliberately absent, even
/// though their operands look like text: `awk 'BEGIN{system(ARGV[1] …
/// )}' git push -uf` executes what follows, and `sed`'s `e` and the
/// pagers' shell escapes do the same. Their arguments are not data.
const DATA_COMMANDS: &[&str] = &[
    "echo", "printf", "cat", "tee", "grep", "egrep", "fgrep", "rg", "ag", "tr", "cut", "paste",
    "sort", "uniq", "head", "tail", "wc", "fold", "column", "diff", "comm", "jq", "yq", "strings",
    "logger",
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
        } else if base == "eval" && in_command_position(tokens, i) {
            // Bash consumes an option terminator before evaluating, so
            // `eval -- "'rm' x"` runs `rm x`. Leaving the `--` in place
            // would put it in the head position of the re-parse and
            // hide the command behind it.
            let rest = match rest.split_first() {
                Some((first, tail)) if first == "--" => tail,
                _ => rest,
            };
            if !rest.is_empty() {
                payloads.push(rest.join(" "));
            }
        } else if base == "git"
            // A `git` among the operands of a data command is a word
            // being printed, alias definitions and all.
            && (!head_is_data || in_command_position(tokens, i))
            && let Some(AliasExpansion::Tokens(expansion)) = expand_command_alias(rest)
            && let Some(first) = expansion.first()
        {
            match first.strip_prefix('!') {
                // A `!` alias is a shell command, not a git
                // subcommand: `-c "alias.p=!sh -c 'git push -uf'"`
                // runs a shell. Its words go through as payloads so
                // the nested command is scanned rather than read as
                // one opaque token.
                Some(shell_form) => {
                    payloads.push(shell_form.to_string());
                    payloads.extend(expansion[1..].iter().cloned());
                    payloads.push(
                        std::iter::once(shell_form.to_string())
                            .chain(expansion[1..].iter().cloned())
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
                // An ordinary alias expands to a git command, which
                // has to face every destructive rule rather than only
                // the flag table: `-c 'alias.p=reset --hard' p` runs
                // `git reset --hard`.
                None => payloads.push(format!("git {}", expansion.join(" "))),
            }
        }
    }
    payloads
}

/// True when the wrapper was asked to describe something instead of
/// running it: `env --help bash -c '…'` prints env's help and exits,
/// `command -v bash -c '…'` prints a path. The trailing words are
/// then operands of a wrapper that never executes them.
///
/// Only the option *immediately* after a wrapper counts. Later
/// options can be operands of earlier ones — `env -u --help bash -c
/// '…'` unsets a variable named `--help` and then runs the payload —
/// and re-deriving which is which would mean a second copy of env's
/// option grammar. So the walk stops at the first option it cannot
/// attribute, keeping the scan, which at worst over-blocks a wrapper
/// that would have printed help.
///
/// The whole leading wrapper chain is walked, not just its first
/// entry: `command env --help git push -uf` runs `env`, which prints
/// help and exits. Only the leading chain — a wrapper name later in
/// the argv is an operand, and letting one suppress the scan would
/// hand every command a way to opt out.
fn wrapper_only_reports(tokens: &[String]) -> bool {
    let mut idx = 0;
    while let Some(wrapper) = tokens.get(idx).map(|t| base_name(t).to_lowercase()) {
        if !COMMAND_RUNNER_WRAPPERS.contains(&wrapper.as_str()) {
            return false;
        }
        let Some(next) = tokens.get(idx + 1) else {
            return false;
        };
        let describes = match wrapper.as_str() {
            // `command -v`/`-V` describe; `env -v` is verbose and
            // still runs the command, so it must not count here.
            "command" => next
                .strip_prefix('-')
                .is_some_and(|cluster| !cluster.starts_with('-') && cluster.contains(['v', 'V'])),
            _ => next == "--help" || next == "--version",
        };
        if describes {
            return true;
        }
        if next.starts_with('-') {
            // An option this walk cannot attribute — its operand may
            // be the next token. Stop rather than guess.
            return false;
        }
        idx += 1;
    }
    false
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
            // `--help` as the operand of `-u` names a variable to
            // unset; env still runs the payload.
            "env -u --help bash -c \"'git' push --force\"",
            "env -u --version bash -c \"'rm' -rf /tmp/x\"",
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
            // Bash consumes `--` before evaluating the rest.
            "eval -- \"'rm' victim.txt\"",
            "eval -- \"'git' push --force\"",
            // Short options cluster: the text patterns only see a flag
            // written on its own.
            "'git' push -uf origin main",
            "git push -uf origin main",
            "git clean -df",
            "git checkout -qf main",
            "git branch -Dq old",
            "git -C /tmp/repo push -uf origin main",
            "bash -c \"'git' push -uf origin main\"",
            // Runners in front of git: the cluster check needs the
            // unwrapped view to see the subcommand at all.
            "env git push -uf origin main",
            "nohup git clean -df",
            "command git checkout -qf main",
            "env -u PATH git push -uf origin main",
            // Runners no wrapper list covers.
            "timeout 30 git push -uf origin main",
            "sudo git push -uf origin main",
            "stdbuf -oL git clean -df",
            "nice -n 5 git checkout -qf main",
            // An inert `git` operand before the executable one.
            "find . -maxdepth 0 -exec echo git \\; -exec git push -uf origin main \\;",
            // Global options that take a separate operand.
            "git --git-dir .git push -uf origin main",
            "git --work-tree /tmp/repo clean -df",
            "git --namespace ns push -uf origin main",
            // A long force option that is not adjacent to the
            // subcommand.
            "git push -u --force origin main",
            "git push --verbose --force origin main",
            "git push --quiet --force-with-lease origin main",
            "git branch --quiet --delete old",
            // Aliases defined in the same command.
            "git -c alias.p=push p -uf origin main",
            "git -c alias.p='push --force' p origin main",
            // Git takes the last value for a repeated key.
            "git -c alias.p=status -c 'alias.p=push --force' p origin main",
            // An alias that names another alias.
            "git -c alias.p=q -c 'alias.q=push -uf' p origin main",
            // Quotes inside an alias value are git's, not part of the
            // word.
            "git -c 'alias.p=push \"--force\"' p origin main",
            "git -c 'alias.p=push \\-\\-force' p origin main",
            // A `!` alias is a shell command, not a git subcommand.
            "git -c \"alias.p=!sh -c 'git push -uf origin main'\" p",
            "git -c \"alias.p=!git push -uf origin main\" p",
            // Config section names are case-insensitive.
            "git -c \"Alias.p=!git push -uf origin main\" p",
            "git -c 'ALIAS.p=push -uf' p origin main",
            "git -c 'alias.P=push -uf' P origin main",
            // An alias whose value comes from the environment or from
            // run-time expansion cannot be read here: unknown, not
            // safe.
            "A=push git --config-env=alias.p=A p -uf origin main",
            "git --config-env alias.p=A p -uf origin main",
            "git -c 'alias.p=$CMD' p -uf origin main",
            // Config handed to git through the environment never shows
            // in the argv at all.
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p GIT_CONFIG_VALUE_0='push --force' \
             git p origin main",
            "GIT_CONFIG_PARAMETERS='alias.p=push --force' git p origin main",
            // The same variables as `env` operands rather than shell
            // prefix assignments, and with the key itself quoted.
            "env GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p \
             GIT_CONFIG_VALUE_0='push --force' git p origin main",
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0='alias.p' \
             GIT_CONFIG_VALUE_0='push --force' git p origin main",
            // `env -S` keeps the assignments inside one token until
            // the wrapper is unwrapped.
            "env -S \"GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p \
             GIT_CONFIG_VALUE_0='push --force' git p origin main\"",
            // A key name the shell has still to expand names a real
            // config key once it is done.
            "env \"GIT_CONFIG_KEY_$I=alias.p\" GIT_CONFIG_COUNT=1 \
             GIT_CONFIG_VALUE_0='push --force' git p origin main",
            // `-S` reached through a cluster, and env's own `\\_` word
            // separator rather than a plain space.
            "env -vS\"GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p \
             GIT_CONFIG_VALUE_0='push --force' git p origin main\"",
            "env -S 'GIT_CONFIG_COUNT=1\\_GIT_CONFIG_KEY_0=alias.p\\_\
             GIT_CONFIG_VALUE_0=\"push\\_--force\"\\_git\\_p\\_origin\\_main'",
            // An `env` behind another wrapper still sets the variable.
            "env env GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p \
             GIT_CONFIG_VALUE_0='push -uf' git p origin main",
            // A config file can define aliases the command never shows.
            "GIT_CONFIG_GLOBAL=/tmp/g git p origin main",
            "env GIT_CONFIG_SYSTEM=/tmp/g git p origin main",
            // An option operand must not be mistaken for the command.
            "env -u X GIT_CONFIG_GLOBAL=/tmp/g git p origin main",
            "env -C /tmp GIT_CONFIG_GLOBAL=/tmp/g git p origin main",
            // Spellings the positional walk does not follow: an
            // unaccounted mention is unknown, not safe.
            "timeout 5 env GIT_CONFIG_GLOBAL=/tmp/g git p",
            "env -S'FOO=x' GIT_CONFIG_GLOBAL=/tmp/g git p",
            "env --uns X GIT_CONFIG_GLOBAL=/tmp/g git p",
            "sudo env GIT_CONFIG_GLOBAL=/tmp/g git p",
            // A separator inside a substitution does not end the
            // statement, so the assignment and the command stay
            // together.
            "GIT_CONFIG_GLOBAL=$(printf /tmp/g; true) git p",
            // Quoting nests: the inner quote closes the inner string,
            // and the `;` is inside the substitution throughout.
            "GIT_CONFIG_GLOBAL=\"$(printf \"/tmp/g;\")\" git p",
            "GIT_CONFIG_GLOBAL=\"`printf \"/tmp/g;\"`\" git p",
            // A launcher inherits the environment, so the git inside
            // the payload is configured by it too.
            "GIT_CONFIG_GLOBAL=/tmp/g bash -c 'git p'",
            "GIT_CONFIG_GLOBAL=/tmp/g sh -c \"git p origin main\"",
            // An alias name the shell still has to expand binds to a
            // command word only at run time.
            "git -c alias.${PATH:+p}='push --force' p",
            "A='push --force' git --config-env=alias.$NAME=A p",
            // Environment that outlives its statement reaches a later
            // git.
            "export GIT_CONFIG_GLOBAL=/tmp/g; git p",
            "export GIT_CONFIG_GLOBAL=/tmp/g\ngit p",
            "export GIT_CONFIG_GLOBAL=/tmp/g && git p",
            // Appending to an unset variable just sets it.
            "export GIT_CONFIG_GLOBAL+=/tmp/g; git p",
            "set -a; GIT_CONFIG_GLOBAL+=/tmp/g; git p",
            // `set -a` exports every later assignment.
            "set -a; GIT_CONFIG_GLOBAL=/tmp/g; git p",
            "set -o allexport; GIT_CONFIG_GLOBAL=/tmp/g; git p",
            // A declaration builtin exports with `-x`.
            "declare -x GIT_CONFIG_GLOBAL=/tmp/g; git p",
            "typeset -x GIT_CONFIG_GLOBAL=/tmp/g; git p",
            // The last toggle in one `set` wins.
            "set +a -a; GIT_CONFIG_GLOBAL=/tmp/g; git p",
            // The export attribute sticks to the name.
            "export GIT_CONFIG_GLOBAL=/dev/null; GIT_CONFIG_GLOBAL=/tmp/g; git p",
            "GIT_CONFIG_GLOBAL=/tmp/g; export GIT_CONFIG_GLOBAL; git p",
            // A declaration keeps its value for a later export.
            "declare GIT_CONFIG_GLOBAL=/tmp/g; export GIT_CONFIG_GLOBAL; git p",
            // `--` ends option parsing, so the later `+a` is an
            // argument rather than a toggle.
            "set -a -- +a; GIT_CONFIG_GLOBAL=/tmp/g; git p",
            // An operand ends `set` option processing too.
            "set -a x +a; GIT_CONFIG_GLOBAL=/tmp/g; git p",
            // A branch that may not run leaves the state unknown.
            "GIT_CONFIG_GLOBAL=/tmp/g; export GIT_CONFIG_GLOBAL; \
             false && GIT_CONFIG_GLOBAL=/dev/null; git p",
            // A heredoc body is data, not statements.
            "export GIT_CONFIG_GLOBAL=/tmp/g; cat <<EOF\nGIT_CONFIG_GLOBAL=/dev/null\nEOF\ngit p",
            // `HOME` and `XDG_CONFIG_HOME` choose where git reads its
            // config, aliases and all.
            "HOME=/tmp/h git p",
            "XDG_CONFIG_HOME=/tmp/h git p",
            // A later assignment cannot argue an opaque config back to
            // safety: a subshell keeps its own copy, and a `#` in the
            // middle of a word is not a comment.
            "export GIT_CONFIG_GLOBAL=/tmp/g; (GIT_CONFIG_GLOBAL=/dev/null); git p",
            "export GIT_CONFIG_GLOBAL=/tmp/g; false#x && GIT_CONFIG_GLOBAL=/dev/null; git p",
            // A builtin wrapper does not stop the export.
            "command export GIT_CONFIG_GLOBAL=/tmp/g; git p",
            "builtin export GIT_CONFIG_GLOBAL=/tmp/g && git p",
            // An include directive pulls in a file that can define
            // aliases.
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=include.path \
             GIT_CONFIG_VALUE_0=/tmp/g git p",
            // A wrapper's own options do not stop the export.
            "command -p export GIT_CONFIG_GLOBAL=/tmp/g; git p",
            "command -- export GIT_CONFIG_GLOBAL=/tmp/g; git p",
            // Every carrier of an include gets the same answer.
            "git -c include.path=/tmp/g p",
            "git --config-env=include.path=VAR p",
            "GIT_CONFIG_PARAMETERS=\"'include.path'='/tmp/g'\" git p",
            // An alias that expands to a destructive operation with no
            // flag cluster of its own.
            "git -c 'alias.p=reset --hard' p",
            "git -c 'alias.p=stash clear' p",
            // A global between git and its subcommand hides the
            // adjacency the text patterns look for.
            "git --no-pager reset --hard HEAD~1",
            "git -c core.foo=bar reset --hard HEAD~1",
            "git --git-dir .git stash clear",
            // A whole statement inside one token, its spaces escaped
            // rather than quoted.
            "psql -c DROP\\ TABLE\\ users",
            "mysql -e DELETE\\ FROM\\ users",
            // The locale catalogue decides what a `$\"…\"` string is,
            // so even an innocuous-looking one is unknown.
            "$\"cat\" file.txt",
            // A substitution restarts quoting, so the translation
            // inside it is one too.
            "echo \"$($\"cat\" /tmp/file)\"",
            // What follows a closing `)` joins the same word, so this
            // `#` is a character rather than a comment.
            "echo $(true)#x; $\"cat\" /tmp/file",
            // A process substitution's pathname joins the word too.
            "true <(true)#x; $\"cat\"",
            "true >(true)#x; $\"cat\"",
            // A command after the heredoc redirect still runs.
            "cat <<EOF; $\"safe\"\nbody\nEOF",
            // The delimiter is unquoted the way the shell unquotes it.
            "cat <<$'EOF'\nbody\nEOF\n$\"safe\"",
            // An escaped space belongs to the delimiter.
            "cat <<EO\\ F\nbody\nEO F\n$\"safe\" -rf /tmp/x",
            // A translated delimiter decides where the body ends, so
            // what lies between is unknown.
            "cat <<$\"SAFE\"\nr\"\"m -rf /tmp/x\nSAFE",
            // An assignment name the shell has still to expand could
            // be any variable at all.
            "env \"$K=/tmp/g\" git p",
            // An interpreter that can run a command does not make its
            // operands data.
            "awk 'BEGIN{system(ARGV[1] \" \" ARGV[2] \" \" ARGV[3]); exit}' git push -uf",
            // A chain longer than the hop budget is unknown, not safe.
            "git -c alias.a1=a2 -c alias.a2=a3 -c alias.a3=a4 -c alias.a4=a5 \
             -c alias.a5=a6 -c alias.a6=a7 -c alias.a7=a8 -c alias.a8=a9 \
             -c alias.a9=a10 -c 'alias.a10=push -uf' a1 origin main",
            // Unambiguous abbreviations of a long option.
            "git push --quiet --force-w origin main",
            "git push --quiet --forc origin main",
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
            // A `$\"` inside quotes is ordinary text, not a
            // translation.
            "echo \"$\"",
            "echo '$\"cat\"'",
            // A comment is never evaluated.
            "echo ok # $\"translation example\"",
            // A subshell's `)` ends the word, so this `#` does start
            // a comment.
            "(true)# $\"cat\"",
            // A heredoc body is data: nothing in it is translated.
            "cat <<'EOF'\n$\"cat\"\nEOF",
            // A `$\"` protected by other quoting is a literal
            // delimiter, not a translation.
            "cat <<'$\"SAFE\"'\nbody\n$\"SAFE\"",
            "cat <<$'$\"SAFE\"'\nbody\n$\"SAFE\"",
            // An escaped quote keeps the delimiter quoted through it.
            "cat <<\"X\\\"$\"SAFE\nbody\nX\"$SAFE",
            // A translation that is only data cannot change what runs.
            "printf '%s\\n' $\"hello\"",
            "grep $\"message\" log.txt",
            // A wrapper does not change whose operands those are.
            "env printf '%s\\n' $\"hello\"",
            "command grep $\"message\" log.txt",
            // An escaped quote inside `$'…'` keeps the literal open.
            "touch $'X\\'$\"SAFE'",
            "cat <<EOF\n$\"cat\"\nEOF",
            // A separator inside a comment separates nothing.
            "export GIT_CONFIG_GLOBAL=/tmp/g # comment; git status",
            "export GIT_CONFIG_GLOBAL=/tmp/g; (true)# comment; git status",
            // A here-string has no body and no ambiguity.
            "export GIT_CONFIG_GLOBAL=/dev/null; cat <<< x",
            // Indentation makes a line body text, not the terminator.
            "cat <<'EOF'\n  EOF\n$\"cat\"\nEOF",
            // Environment names are case-sensitive.
            "home=/tmp/h git status",
            "git_config_global=/tmp/g git status",
            // A report-only git call dispatches nothing.
            "GIT_CONFIG_GLOBAL=/tmp/g git --version",
            "HOME=/tmp git --html-path",
            "GIT_CONFIG_GLOBAL=/tmp/g git -v",
            "GIT_CONFIG_GLOBAL=/tmp/g git -h",
            // An unmodelled runner in front of a lowercase variable.
            "timeout 1 env git_config_global=/tmp/g git status",
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
            "eval -- 'echo hi'",
            // Clusters without a destructive flag stay allowed.
            "git push -u origin main",
            "git push -qu origin main",
            "git clean -n",
            "git checkout -b feature",
            "git -C /tmp/repo push -u origin main",
            "git log -p",
            "env git push -u origin main",
            "env --help git push -uf origin main",
            // Past `--` a dash-prefixed word is a pathspec.
            "git clean -n -- -dir",
            "git checkout main -- -file",
            // `--no-force` turns the option back off.
            "git push --no-force origin main",
            // An alias that expands to something harmless.
            "git -c alias.s=status s",
            // An alias name reused as an operand of another
            // subcommand is not the command token.
            "git -c 'alias.p=push --force' status p",
            // A global that prints and exits dispatches no subcommand,
            // whether what follows is an alias or spelled out.
            "git -c 'alias.p=push -uf' --html-path p",
            "git -c 'alias.p=push -uf' --man-path p",
            "git --html-path push -uf",
            "git --info-path clean -df",
            // Bare `--exec-path` prints and exits; only the attached
            // form configures a command that then runs.
            "git --exec-path push -uf",
            // A `!` alias that runs something harmless.
            "git -c \"alias.p=!echo hi\" p",
            // Ordinary env config defines no alias.
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=user.name GIT_CONFIG_VALUE_0=me git commit",
            "env GIT_CONFIG_KEY_0=user.name GIT_CONFIG_VALUE_0=me git commit",
            // Asking for no config at all defines nothing.
            "GIT_CONFIG_GLOBAL=/dev/null git status",
            "GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git log -p",
            // A prefix assignment belongs to the statement it
            // introduces, not to a later one.
            "GIT_CONFIG_GLOBAL=/tmp/g /bin/true; git status",
            // A prefix-scoped assignment on a data command: the `git`
            // here is a word to print, not a command.
            "GIT_CONFIG_GLOBAL=/tmp/g printf '%s\\n' git",
            // A wrapped data command still only prints.
            "env printf '%s\\n' 'git' push --force",
            "command echo 'git' push --force",
            // An operator inside quoted data branches nowhere.
            "export GIT_CONFIG_GLOBAL=/dev/null; printf '%s\\n' 'a && b'; git status",
            "export GIT_CONFIG_GLOBAL=/dev/null; echo '<<EOF'; git status",
            // A global's operand is not the subcommand: this greps in
            // a directory that happens to be named `push`.
            "git -C push grep -f ../patterns",
            "git --git-dir clean status",
            // A subcommand's own argument is not another subcommand.
            "git grep push -f ../patterns",
            "git log branch -1",
            // A wrapper that only describes itself runs none of this.
            "env --help 'git' push --force",
            "command -v 'git' push --force",
            // A key that merely contains `alias.` does not define one.
            "GIT_CONFIG_PARAMETERS=\"'user.alias.foo'='x'\" git status",
            // `command -v` describes the builtin, it does not run it.
            "command -v export GIT_CONFIG_GLOBAL=/tmp/g; git status",
            // `set` operands become positional parameters, not
            // environment.
            "set -- GIT_CONFIG_GLOBAL=/tmp/g; git status",
            // Printing an alias example is not defining one.
            "printf '%s\\n' git -c 'alias.p=reset --hard' p",
            // An unexported shell variable never reaches git.
            "GIT_CONFIG_GLOBAL=/tmp/g; git status",
            "GIT_CONFIG_GLOBAL+=/tmp/g\ngit status",
            // A declaration without `-x` is not exported.
            "declare GIT_CONFIG_GLOBAL=/tmp/g; git status",
            "local GIT_CONFIG_GLOBAL=/tmp/g; git status",
            // Auto-export switched back off.
            "set -a; set +a; GIT_CONFIG_GLOBAL=/tmp/g; git status",
            "set -o allexport; set +o allexport; GIT_CONFIG_GLOBAL=/tmp/g; git status",
            // A data command's operands are text, however they are
            // quoted.
            "printf '%s\\n' 'git' push --force",
            "grep -r 'rm' -rf docs/",
            "GIT_CONFIG_GLOBAL=/tmp/g /bin/true && git log -p",
            // An assignment-looking operand of a command that sets
            // nothing is data.
            "printf '%s\\n' 'GIT_CONFIG_KEY_0=alias.p' git",
            "echo GIT_CONFIG_KEY_0=alias.p git",
            // Describe-and-exit deeper in a wrapper chain.
            "command env --help git push -uf origin main",
            // A git token among the operands of a data command. Only
            // spellings the historical text scan does not already
            // refuse — `printf … git clean -df` still trips the
            // literal `git clean -d` pattern, which predates this.
            "echo git push -uf origin main",
            "printf '%s\\n' git push -uf origin main",
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
