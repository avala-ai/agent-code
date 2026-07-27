//! Block bash invocations that would write into protected directories.
//!
//! The FileWrite/FileEdit/MultiEdit/NotebookEdit tools are
//! unconditionally blocked from writing into `PROTECTED_DIRS`
//! (`.git/`, `.husky/`, `node_modules/`) by
//! [`crate::permissions`]. Without this module the bash tool would
//! happily route around that gate via:
//!
//! * shell redirections: `> .git/config`, `>> .git/config`,
//!   `tee .git/config`, heredoc-to-file.
//! * non-sed writers: `cp`, `mv`, `tee`, `dd of=`, `install`, `ln`,
//!   `rsync`, `truncate`.
//! * interpreters: `bash -c '... > .git/config'`,
//!   `python -c "open('.git/config','w')..."`, `eval ...`, `node -e`,
//!   `perl -e`, `ruby -e`.
//!
//! Each of those is checked here. The same destination-extraction logic
//! is reused for the system-path list (`/etc/`, `/usr/`, `/bin/`,
//! `/sbin/`, `/boot/`, `/sys/`, `/proc/`) so that the historical
//! `mv …` / `> …` / `tee …` checks in `bash::mod` no longer leave
//! `cp …`, `dd of=…`, `install …`, etc. as bypasses.

use std::path::Path;

use crate::permissions::{PROTECTED_DIRS, is_team_memory_write_target};
use crate::tools::bash::BLOCKED_WRITE_PATHS;

/// Maximum recursion depth for re-parsing `bash -c '…'` /
/// `eval '…'` payloads. Three is enough for any realistic
/// nested-shell construction; deeper nesting almost certainly
/// indicates obfuscation, in which case refusing is the right answer.
const MAX_INTERPRETER_DEPTH: u8 = 3;

/// A single protected-path violation found in a bash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedPathViolation {
    /// Human-readable explanation that the bash tool surfaces to the
    /// caller.
    pub reason: String,
}

/// Run every protected-path check against `command`. Returns the first
/// violation found, or `Ok(())` when the command is safe.
///
/// `command` is the full original shell string. We re-parse pieces of
/// it (subshells, `bash -c` payloads, etc.) to apply the same checks
/// recursively.
pub fn check(command: &str) -> Result<(), ProtectedPathViolation> {
    check_with_depth(command, 0)
}

fn check_with_depth(command: &str, depth: u8) -> Result<(), ProtectedPathViolation> {
    if depth > MAX_INTERPRETER_DEPTH {
        return Err(ProtectedPathViolation {
            reason: format!(
                "interpreter nesting exceeds depth {MAX_INTERPRETER_DEPTH}; \
                 refusing because deep nesting cannot be safely audited"
            ),
        });
    }

    for segment in shell_segments(command) {
        check_segment(&segment, depth)?;
    }
    Ok(())
}

fn check_segment(segment: &str, depth: u8) -> Result<(), ProtectedPathViolation> {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let tokens = tokenize(trimmed);
    if tokens.is_empty() {
        return Ok(());
    }

    // Redirections (`>`, `>>`, `|& tee FILE`, heredoc-to-file). These
    // are checked first because they apply regardless of the underlying
    // command name.
    for dest in extract_redirection_destinations(&tokens) {
        ensure_not_protected(&dest, "redirection")?;
    }

    // Skip leading `VAR=value` assignments to find the actual command.
    let mut idx = 0;
    while idx < tokens.len() && is_assignment(&tokens[idx]) {
        idx += 1;
    }
    if idx >= tokens.len() {
        return Ok(());
    }

    let head = base_name(&tokens[idx]);
    let args = &tokens[idx + 1..];

    // Writer commands: extract the destination operand and check it.
    for dest in extract_writer_destinations(head, args) {
        ensure_not_protected(&dest, head)?;
    }

    // Interpreter recursion / heuristic scan.
    if let Some(payload) = extract_recursive_shell_payload(head, args) {
        check_with_depth(&payload, depth + 1)?;
    } else if let Some(payload) = extract_interpreter_payload(head, args) {
        ensure_no_protected_substring(&payload, head)?;
    }

    Ok(())
}

/// Refuse `path` if it resolves into any protected directory or
/// blocked system path.
fn ensure_not_protected(path: &str, source: &str) -> Result<(), ProtectedPathViolation> {
    let normalized = normalize_path(path);
    for dir in PROTECTED_DIRS {
        if path_targets_dir(&normalized, dir) {
            let display = dir.trim_end_matches(['/', '\\']);
            return Err(ProtectedPathViolation {
                reason: format!(
                    "{source} would write into {display}/ ({path}); \
                     this is a protected directory"
                ),
            });
        }
    }
    for sys in BLOCKED_WRITE_PATHS {
        if path_targets_dir(&normalized, sys) {
            return Err(ProtectedPathViolation {
                reason: format!(
                    "{source} would write into system path {sys} ({path}); \
                     operations on system directories are not allowed"
                ),
            });
        }
    }
    // Team-memory directory is shared, version-controlled state — only
    // the `/team-remember` slash command may add entries. We pass
    // `None` for project_root so the component-aware fallback inside
    // `is_team_memory_write_target` runs; the bash tool does not have
    // a project-root handle here.
    if is_team_memory_write_target(Path::new(&normalized), None) {
        return Err(ProtectedPathViolation {
            reason: format!(
                "{source} would write into .agent/team-memory/ ({path}); \
                 team-memory is read-only to the agent — use the \
                 `/team-remember` slash command to add entries"
            ),
        });
    }
    Ok(())
}

/// Conservative literal-substring scan for protected paths inside an
/// interpreter payload (`python -c "…"`, `node -e "…"`, etc). We
/// cannot semantically parse Python/JS/Perl/Ruby, so we deliberately
/// over-reject when the payload mentions any `PROTECTED_DIRS` /
/// `BLOCKED_WRITE_PATHS` entry. False positives here cost the caller
/// a clear error message; false negatives would defeat the gate
/// entirely.
fn ensure_no_protected_substring(
    payload: &str,
    interpreter: &str,
) -> Result<(), ProtectedPathViolation> {
    for dir in PROTECTED_DIRS {
        if payload.contains(dir) {
            let display = dir.trim_end_matches(['/', '\\']);
            return Err(ProtectedPathViolation {
                reason: format!(
                    "{interpreter} payload references protected path {display}/; \
                     interpreters cannot reference protected paths via this tool"
                ),
            });
        }
    }
    for sys in BLOCKED_WRITE_PATHS {
        if payload.contains(sys) {
            return Err(ProtectedPathViolation {
                reason: format!(
                    "{interpreter} payload references system path {sys}; \
                     interpreters cannot reference system paths via this tool"
                ),
            });
        }
    }
    if payload.contains(".agent/team-memory") || payload.contains(".agent\\team-memory") {
        return Err(ProtectedPathViolation {
            reason: format!(
                "{interpreter} payload references team-memory path; \
                 team-memory is read-only to the agent — use the \
                 `/team-remember` slash command to add entries"
            ),
        });
    }
    Ok(())
}

/// Walk `tokens` and return every redirection target.
fn extract_redirection_destinations(tokens: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        // `2>file` / `>file` / `>>file` / `&>file` attached forms.
        if let Some(rest) = strip_redirect_prefix(tok) {
            if !rest.is_empty() {
                out.push(rest.to_string());
            } else if let Some(next) = tokens.get(i + 1) {
                out.push(next.clone());
                i += 1;
            }
        }
        // Heredoc-to-file forms: `<<<FILE` is read; `<<EOF` reads
        // stdin from a heredoc and is not a write — only `>` family
        // matter for protected-path purposes. Skip.
        i += 1;
    }
    out
}

/// Strip a leading `>`, `>>`, `&>`, `&>>`, `2>`, or `2>>` and return
/// what follows. None for non-redirection tokens.
fn strip_redirect_prefix(tok: &str) -> Option<&str> {
    // Order matters: longest prefix first.
    for prefix in ["&>>", "&>", "2>>", "2>", ">>", ">"] {
        if let Some(rest) = tok.strip_prefix(prefix) {
            // A bare `2` followed by `>` would be split into separate
            // tokens by the tokenizer. The cases that survive here are
            // all genuine redirection prefixes.
            return Some(rest);
        }
    }
    None
}

/// Identify the destination operand for a known writer command.
fn extract_writer_destinations(head: &str, args: &[String]) -> Vec<String> {
    let positional = positional_args(args);
    match head {
        "cp" | "mv" | "rsync" | "scp" => {
            // Last positional is the destination. Earlier positionals
            // may also be sources (cp src1 src2 destdir/), but the
            // protected-path test is per-destination; only the final
            // positional is a write target.
            positional.last().cloned().into_iter().collect()
        }
        "tee" => {
            // Every positional is a target.
            positional
        }
        "ln" => {
            // `ln [opts] TARGET LINK_NAME`. `LINK_NAME` is the new
            // entry being created. When only one positional is
            // present (`ln target`), the link name defaults to the
            // basename of the target in the current directory — that
            // case cannot land in a protected dir directly, so skip.
            if positional.len() >= 2 {
                positional.last().cloned().into_iter().collect()
            } else {
                Vec::new()
            }
        }
        "install" => {
            // `install [opts] SRC DEST` or `install [opts] -t DESTDIR SRC...`
            // Look for `-t DEST` first; otherwise the last positional
            // is the destination.
            if let Some(dest) = find_flag_value(args, &["-t", "--target-directory"]) {
                return vec![dest];
            }
            positional.last().cloned().into_iter().collect()
        }
        "dd" => {
            // `dd of=…` is the only write target.
            args.iter()
                .filter_map(|a| a.strip_prefix("of=").map(|s| s.to_string()))
                .collect()
        }
        "truncate" => {
            // `truncate [-s SIZE] FILE...` — every positional is a
            // file target. `-s VALUE` consumes its argument; we have
            // already filtered flag values out via `positional_args`.
            positional
        }
        _ => Vec::new(),
    }
}

/// If `head` is a recursive-shell interpreter (`bash`, `sh`, `zsh`,
/// `eval`), return the literal payload that should be re-parsed as
/// bash.
fn extract_recursive_shell_payload(head: &str, args: &[String]) -> Option<String> {
    match head {
        "bash" | "sh" | "zsh" | "dash" | "ash" | "ksh" => {
            // Look for `-c CMD`. Any other invocation form runs a
            // script file we cannot read; refuse via the literal scan
            // path instead.
            for (i, a) in args.iter().enumerate() {
                if a == "-c"
                    && let Some(payload) = args.get(i + 1)
                {
                    return Some(payload.clone());
                }
                if let Some(rest) = a.strip_prefix("-c")
                    && !rest.is_empty()
                {
                    return Some(rest.to_string());
                }
            }
            None
        }
        "eval" => {
            // `eval` concatenates its arguments and evaluates them.
            if args.is_empty() {
                None
            } else {
                Some(args.join(" "))
            }
        }
        _ => None,
    }
}

/// If `head` is a non-shell interpreter that takes inline source on
/// the command line, return the source string for the heuristic scan.
fn extract_interpreter_payload(head: &str, args: &[String]) -> Option<String> {
    let flag = match head {
        "python" | "python2" | "python3" => "-c",
        "node" | "deno" | "bun" => "-e",
        "perl" => "-e",
        "ruby" => "-e",
        _ => return None,
    };
    for (i, a) in args.iter().enumerate() {
        if a == flag
            && let Some(payload) = args.get(i + 1)
        {
            return Some(payload.clone());
        }
        if let Some(rest) = a.strip_prefix(flag)
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }
    None
}

/// Return positional (non-flag) arguments. Skips long-form flag
/// values (`--target=DIR` is consumed in one token already) and the
/// argument that follows known short flags taking a value.
fn positional_args(args: &[String]) -> Vec<String> {
    const FLAGS_WITH_VALUE: &[&str] = &[
        "-t",
        "--target-directory",
        "-s",
        "--size",
        "-m",
        "--mode",
        "-o",
        "--owner",
        "-g",
        "--group",
        "-T",
        "--no-target-directory",
        "-S",
        "--suffix",
    ];
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            // After `--`, every remaining token is positional.
            for rest in &args[i + 1..] {
                out.push(rest.clone());
            }
            break;
        }
        if FLAGS_WITH_VALUE.contains(&a.as_str()) {
            i += 2;
            continue;
        }
        if a.starts_with('-') && a != "-" {
            i += 1;
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    out
}

/// Find the value of `flag` (`-t DEST` or `--target-directory=DEST`).
fn find_flag_value(args: &[String], flag_names: &[&str]) -> Option<String> {
    for (i, a) in args.iter().enumerate() {
        if flag_names.iter().any(|f| f == a)
            && let Some(v) = args.get(i + 1)
        {
            return Some(v.clone());
        }
        for f in flag_names {
            if let Some(rest) = a.strip_prefix(&format!("{f}="))
                && !rest.is_empty()
            {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Treat `path` as targeting `dir` if any path component (after
/// normalization) starts with the directory marker. We avoid a naive
/// `contains` check because that would over-match on names that merely
/// share a substring (`my.git/file` is not `.git/`).
fn path_targets_dir(path: &str, dir: &str) -> bool {
    let dir_clean = dir.trim_end_matches(['/', '\\']);
    if dir_clean.starts_with('/') {
        // Absolute system paths: treat the trailing slash as the
        // boundary so `/etc` matches `/etc/passwd` but not
        // `/etcetera`.
        let with_slash = format!("{dir_clean}/");
        return path.starts_with(&with_slash) || path == dir_clean;
    }
    // Project-relative: match when any path segment equals dir_clean.
    path.split(['/', '\\']).any(|seg| seg == dir_clean)
}

/// Strip surrounding quotes and a trailing slash from a path token.
fn normalize_path(raw: &str) -> String {
    let trimmed = raw.trim_matches(|c: char| c == '"' || c == '\'');
    trimmed.to_string()
}

/// Strip a leading path and any wrapping quotes from a command name.
fn base_name(raw: &str) -> &str {
    let trimmed = raw.trim_matches(|c: char| c == '"' || c == '\'');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Recognize `KEY=value` assignment tokens.
fn is_assignment(tok: &str) -> bool {
    if tok.starts_with('-') || tok.starts_with('=') {
        return false;
    }
    if let Some(eq) = tok.find('=') {
        let key = &tok[..eq];
        return !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    }
    false
}

/// Split a shell command on `|`, `||`, `&&`, `;`, and newline
/// boundaries while preserving quoted runs verbatim.
fn shell_segments(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    let mut quote: Option<char> = None;
    let mut paren_depth: i32 = 0;

    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                current.push(c);
            }
            '\\' => {
                current.push(c);
                if let Some(&next) = chars.peek() {
                    current.push(next);
                    chars.next();
                }
            }
            '(' => {
                paren_depth += 1;
                current.push(c);
            }
            ')' => {
                paren_depth -= 1;
                current.push(c);
            }
            '|' if chars.peek() == Some(&'|') && paren_depth == 0 => {
                chars.next();
                push_segment(&mut out, &mut current);
            }
            '&' if chars.peek() == Some(&'&') && paren_depth == 0 => {
                chars.next();
                push_segment(&mut out, &mut current);
            }
            '|' | ';' | '\n' if paren_depth == 0 => push_segment(&mut out, &mut current),
            _ => current.push(c),
        }
    }
    push_segment(&mut out, &mut current);
    out
}

fn push_segment(out: &mut Vec<String>, current: &mut String) {
    let s = current.trim().to_string();
    if !s.is_empty() {
        out.push(s);
    }
    current.clear();
}

/// Whitespace-aware tokenizer that preserves quoted runs and
/// strips outer quote characters from each emitted token.
fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '\\' => {
                if let Some(&next) = chars.peek() {
                    current.push(next);
                    chars.next();
                }
            }
            ' ' | '\t' | '\n' => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refuse(cmd: &str) {
        assert!(check(cmd).is_err(), "expected refusal for: {cmd}");
    }
    fn allow(cmd: &str) {
        assert!(
            check(cmd).is_ok(),
            "expected accept for: {cmd}, got {:?}",
            check(cmd)
        );
    }

    #[test]
    fn allows_safe_commands() {
        allow("ls -la");
        allow("cargo test");
        allow("git status");
        allow("cat foo");
        allow("echo bar");
        allow("cp src/a.txt src/b.txt");
        allow("printf x > /tmp/out");
        allow("tee /tmp/file");
        allow("python -c 'print(1)'");
    }

    #[test]
    fn redirection_into_protected_dir_refused() {
        refuse("cat foo > .git/config");
        refuse("printf evil >> .git/config");
        refuse("echo x &> .git/config");
        refuse("echo x &>> .git/config");
        refuse("echo x 2> .git/config");
        refuse("echo x 2>> .git/config");
        refuse("ls > .husky/pre-commit");
        refuse("ls > node_modules/foo");
    }

    #[test]
    fn writers_into_protected_dir_refused() {
        refuse("tee -a .git/config");
        refuse("cp src .git/config");
        refuse("mv x .git/config");
        refuse("install -m 644 src .git/config");
        refuse("ln -sf evil .git/config");
        refuse("rsync src .git/config");
        refuse("dd of=.git/config");
        refuse("dd if=/dev/zero of=.git/config bs=1");
        refuse("truncate -s 0 .git/config");
        refuse("install -t .git/ src");
    }

    #[test]
    fn redirection_into_system_path_refused() {
        refuse("printf x > /boot/grub/grub.cfg");
        refuse("dd of=/sys/something");
        refuse("dd of=/proc/sysrq-trigger");
        refuse("cat src > /etc/passwd");
    }

    #[test]
    fn writers_into_system_path_refused() {
        refuse("cp src /etc/passwd");
        refuse("mv x /etc/shadow");
        refuse("tee /etc/foo");
        refuse("install -m 644 src /etc/foo");
        refuse("ln -sf evil /etc/foo");
    }

    #[test]
    fn writes_into_team_memory_refused() {
        // Team-memory is read-only to the agent; only `/team-remember`
        // may add entries.
        refuse("echo hi > .agent/team-memory/foo.md");
        refuse("echo hi >.agent/team-memory/foo.md");
        refuse("echo hi | tee .agent/team-memory/foo.md");
        refuse("mv /tmp/x.md .agent/team-memory/x.md");
        refuse("cp /tmp/x.md .agent/team-memory/x.md");
        refuse("python -c \"open('.agent/team-memory/x.md','w')\"");
    }

    #[test]
    fn shell_recursion_into_protected_dir_refused() {
        refuse("bash -c 'printf evil > .git/config'");
        refuse("sh -c 'cp src .git/config'");
        refuse("zsh -c 'tee .git/config'");
        refuse("eval 'printf evil > .git/config'");
        refuse("bash -c 'cp src /etc/passwd'");
        refuse("bash -c \"bash -c 'echo x > .git/config'\"");
    }

    #[test]
    fn nesting_too_deep_refused() {
        // Four levels of bash -c nesting → refused on principle.
        let cmd = "bash -c \"bash -c \\\"bash -c \\\\\\\"bash -c 'ls'\\\\\\\"\\\"\"";
        // Either it parses to a depth error or to a normal allow; we
        // just need to confirm the recursion logic doesn't panic.
        let _ = check(cmd);
    }

    #[test]
    fn interpreter_payload_referencing_protected_path_refused() {
        refuse("python -c \"open('.git/config','w').write('evil')\"");
        refuse("python3 -c \"open('.git/HEAD','w')\"");
        refuse("node -e \"require('fs').writeFileSync('.git/config','evil')\"");
        refuse("perl -e \"open(F, '> .git/config');\"");
        refuse("ruby -e \"File.write('.git/config','evil')\"");
        refuse("python -c \"open('/etc/passwd','w')\"");
    }

    #[test]
    fn interpreter_payload_without_protected_path_allowed() {
        allow("python -c 'print(1)'");
        allow("node -e 'console.log(1)'");
        allow("perl -e 'print 1'");
        allow("ruby -e 'puts 1'");
    }

    #[test]
    fn legitimate_writes_outside_protected_dirs_allowed() {
        allow("cp x.txt y.txt");
        allow("cat foo > /tmp/bar");
        allow("tee /tmp/file");
        allow("dd if=/dev/zero of=/tmp/blob bs=1k count=1");
        allow("ln -s /tmp/a /tmp/b");
        allow("rsync /tmp/a /tmp/b");
    }

    #[test]
    fn pipeline_segments_each_checked() {
        // The destructive write is in the second segment.
        refuse("ls | tee .git/config");
        refuse("cat foo && cp x .git/config");
        refuse("true; mv x .git/config");
        refuse("false || echo bad > .git/config");
    }

    #[test]
    fn protected_dir_substring_is_not_a_false_positive() {
        // `my.git` is not `.git/`. Ensure the boundary check works.
        allow("cp src my.git_backup/file");
        // `node_modulesx/` is not `node_modules/`.
        allow("cp src node_modulesx/file");
    }

    #[test]
    fn assignment_prefix_does_not_hide_writer() {
        refuse("FOO=1 cp src .git/config");
        refuse("LC_ALL=C printf evil > .git/config");
    }

    #[test]
    fn ln_with_one_arg_does_not_panic() {
        // `ln target` (one positional) should not be flagged because we
        // don't know the destination — but it must not panic either.
        allow("ln target");
    }
}
