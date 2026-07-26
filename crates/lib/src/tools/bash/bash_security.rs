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
        // `env git push -uf …` runs git, so the wrapper chain comes off
        // before the cluster check can see the subcommand. Nothing is
        // checked at all when the chain was only asked to describe
        // itself — `command env --help git push -uf` prints env's help.
        let unquoted: Vec<String> = invocation.iter().map(|t| unquote_token(t)).collect();
        if wrapper_only_reports(&unquoted) {
            continue;
        }
        let unwrapped = unwrapped_argv(invocation);
        // Assignments are collected from both views: `env -S 'FOO=bar
        // git p'` keeps them inside one token until the wrapper is
        // unwrapped, while a plain `env FOO=bar git p` loses them when
        // it is.
        let mut env_pairs = git_env_pairs(cmd, invocation);
        if let Some(unwrapped) = unwrapped.as_deref() {
            env_pairs.extend(git_env_pairs(cmd, unwrapped));
        }
        for tokens in [Some(invocation.as_slice()), unwrapped.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some(reason) = clustered_git_flag(tokens) {
                findings.push(DestructiveFinding {
                    level: DestructivenessLevel::Destructive,
                    reason,
                });
            }
            // Config from the environment never appears in argv, so an
            // alias defined there turns any command word into something
            // this scan cannot read. The variables reach git either as
            // a shell prefix assignment or as an `env` operand, and
            // unwrapping strips the latter, so both are collected.
            if env_defines_opaque_git_alias(&env_pairs)
                && tokens
                    .iter()
                    .any(|t| base_name(&unquote_token(t)).to_lowercase() == "git")
            {
                findings.push(DestructiveFinding {
                    level: DestructivenessLevel::Destructive,
                    reason: "git alias comes from the environment; what it runs cannot be \
                             determined"
                        .to_string(),
                });
            }
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
    if let Some(reason) = scan_git_subcommands(after_git) {
        return Some(reason);
    }
    // Nothing literal. The command token may still be an alias defined
    // in this same command, which only expansion reveals.
    match expand_command_alias(after_git)? {
        AliasExpansion::Tokens(tokens) => scan_git_subcommands(&tokens),
        AliasExpansion::Unresolved => {
            Some("git alias cannot be resolved; what it runs cannot be determined".to_string())
        }
    }
}

fn scan_git_subcommands(after_git: &[String]) -> Option<String> {
    for (idx, token) in after_git.iter().enumerate() {
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
];

/// How many alias hops to follow. Aliases can name other aliases.
const MAX_GIT_ALIAS_DEPTH: usize = 8;

/// Every environment assignment that reaches the command, with shell
/// quoting removed: shell prefixes (`FOO=bar cmd`) and `env` operands
/// (`env FOO=bar cmd`) both set the variable, and the parser keeps
/// them in different places.
fn git_env_pairs(cmd: &ParsedCommand, invocation: &[String]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = cmd
        .assignments
        .iter()
        .map(|(name, value)| (unquote_token(name), unquote_token(value)))
        .collect();
    // Only where a runner would actually apply them. A `NAME=VALUE`
    // string among the operands of an ordinary command is data:
    // `printf '%s\n' 'GIT_CONFIG_KEY_0=alias.p' git` prints two words
    // and sets nothing.
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
            idx += 1;
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
    if name.is_empty() || !(is_env_assignment(word) || name.contains(['$', '`'])) {
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
        let name = name.to_ascii_uppercase();
        let dynamic_value = value.contains('$') || value.contains('`');
        let names_an_alias = value.to_lowercase().starts_with("alias.");
        if name == "GIT_CONFIG_PARAMETERS" {
            return dynamic_value || value.to_lowercase().contains("alias.");
        }
        // A config *file* can define aliases too, and its contents are
        // not in the command at all. `/dev/null` and an empty path are
        // the documented way to ask for no config, and define nothing.
        if matches!(
            name.as_str(),
            "GIT_CONFIG_GLOBAL" | "GIT_CONFIG_SYSTEM" | "GIT_CONFIG"
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
                dynamic_value || names_an_alias
            }
            _ => false,
        }
    })
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
        }) && let Some(name) = alias_key_name(definition)
        {
            opaque.push(name);
        }
        // `-c alias.p=push` arrives as two tokens; `-calias.p=push`
        // as one.
        let body = token.strip_prefix("-c").unwrap_or(token);
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
            && let Some(AliasExpansion::Tokens(expansion)) = expand_command_alias(rest)
            && let Some(shell_form) = expansion.first().and_then(|first| first.strip_prefix('!'))
        {
            // A `!` alias is a shell command, not a git subcommand:
            // `-c "alias.p=!sh -c 'git push -uf'"` runs a shell. Its
            // words go through as payloads so the nested command is
            // scanned rather than read as one opaque token.
            payloads.push(shell_form.to_string());
            payloads.extend(expansion[1..].iter().cloned());
            payloads.push(
                std::iter::once(shell_form.to_string())
                    .chain(expansion[1..].iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
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
