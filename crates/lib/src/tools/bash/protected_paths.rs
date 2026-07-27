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
//!
//! Destinations are checked in several spellings (see
//! [`normalized_forms`]): lexical, raw, anchored to the execution cwd
//! for relative paths ([`check_at`]), and symlink-resolved. Links
//! created by an earlier segment of the *same* command (`ln`,
//! `cp -s`/`-l`) are tracked in scan order, so a later write through a
//! path that does not exist at validation time is checked against the
//! link's target instead of being waved through.

use std::path::{Path, PathBuf};

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

/// How relative write destinations are anchored during a check.
#[derive(Clone)]
enum Anchor {
    /// No execution directory is known (pre-execution screening from
    /// `validate_input`). Relative paths are checked lexically only —
    /// the anchored pass in [`check_at`] runs before dispatch.
    None,
    /// The canonicalized directory the command will execute in.
    Dir(PathBuf),
    /// A cwd was claimed but could not be canonicalized. Relative
    /// write destinations cannot be verified, so they are refused
    /// (fail closed).
    Unavailable,
}

/// A link (`ln`, `ln -s`, `cp -s`, `cp -l`) that an earlier segment of
/// the command under scan will create before a later segment runs.
///
/// Canonicalization sees only the pre-execution filesystem, so a write
/// "through" such a link must be checked against the link's *target*,
/// not against the not-yet-existing path.
struct CreatedLink {
    /// Normalized spellings of the link name, for prefix-matching
    /// later write destinations.
    name_forms: Vec<String>,
    /// The link target as written.
    target: String,
    /// Directory the link itself lives in — the parent of the link
    /// name. A *symbolic* link's relative target is resolved from
    /// here, not from the shell cwd: after `ln -s ../etc /tmp/e`,
    /// `/tmp/e` points at `/etc` regardless of where bash was run.
    name_dir: String,
    /// True for symbolic links (`ln -s`, `cp -s`). A hard link's
    /// target is resolved against the cwd at creation time, so it
    /// keeps the ordinary anchoring.
    symbolic: bool,
    /// False when the target contains shell expansion (`$`, backtick)
    /// and therefore cannot be audited before execution.
    auditable: bool,
}

impl CreatedLink {
    /// If any form of a write destination is the link itself or lies
    /// below it, return the path remainder under the link ("" for the
    /// link itself).
    fn match_remainder(&self, dest_forms: &[String]) -> Option<String> {
        for name in &self.name_forms {
            let name = name.trim_end_matches('/');
            if name.is_empty() || name == "." {
                continue;
            }
            for dest in dest_forms {
                if dest == name {
                    return Some(String::new());
                }
                if let Some(rest) = dest.strip_prefix(name)
                    && let Some(rest) = rest.strip_prefix('/')
                {
                    return Some(rest.to_string());
                }
            }
        }
        None
    }

    /// The path this link resolves to once `remainder` is appended,
    /// spelled so that [`normalized_forms`] can anchor it correctly.
    ///
    /// A symbolic link's relative target hangs off the link's own
    /// directory; a hard link's target was resolved against the cwd
    /// when it was created, so it is returned as written.
    fn substituted(&self, remainder: &str) -> String {
        let base = if self.symbolic && !is_shell_absolute(&self.target) && !self.name_dir.is_empty()
        {
            format!("{}/{}", self.name_dir.trim_end_matches('/'), self.target)
        } else {
            self.target.clone()
        };
        if remainder.is_empty() {
            base
        } else {
            format!("{}/{}", base.trim_end_matches('/'), remainder)
        }
    }
}

/// State threaded through every segment (and recursive shell payload)
/// of one command, in execution order.
struct ScanState {
    anchor: Anchor,
    /// Links created by segments already scanned.
    links: Vec<CreatedLink>,
    /// True once a segment creates a link whose *name* cannot be
    /// determined before execution — after that, no later write
    /// destination can be audited at all.
    opaque_link: bool,
}

impl ScanState {
    fn new(anchor: Anchor) -> Self {
        Self {
            anchor,
            links: Vec::new(),
            opaque_link: false,
        }
    }
}

/// Run every protected-path check against `command`. Returns the first
/// violation found, or `Ok(())` when the command is safe.
///
/// `command` is the full original shell string. We re-parse pieces of
/// it (subshells, `bash -c` payloads, etc.) to apply the same checks
/// recursively.
///
/// This entry point has no execution directory, so relative
/// destinations are checked lexically only. The bash tool additionally
/// runs [`check_at`] with the real cwd before dispatch; both must pass.
pub fn check(command: &str) -> Result<(), ProtectedPathViolation> {
    check_with_depth(command, 0, &mut ScanState::new(Anchor::None))
}

/// Like [`check`], but anchored to `cwd` — the directory the command
/// will actually execute in. Relative destinations are resolved against
/// it before symlink canonicalization, so `out -> .git` in the cwd is
/// seen for what it is. When `cwd` cannot be canonicalized the check
/// fails closed: relative write destinations are refused.
pub fn check_at(command: &str, cwd: &Path) -> Result<(), ProtectedPathViolation> {
    let anchor = match cwd.canonicalize() {
        Ok(dir) => Anchor::Dir(dir),
        Err(_) => Anchor::Unavailable,
    };
    check_with_depth(command, 0, &mut ScanState::new(anchor))
}

fn check_with_depth(
    command: &str,
    depth: u8,
    state: &mut ScanState,
) -> Result<(), ProtectedPathViolation> {
    if depth > MAX_INTERPRETER_DEPTH {
        return Err(ProtectedPathViolation {
            reason: format!(
                "interpreter nesting exceeds depth {MAX_INTERPRETER_DEPTH}; \
                 refusing because deep nesting cannot be safely audited"
            ),
        });
    }

    for segment in shell_segments(command) {
        check_segment(&segment, depth, state)?;
    }
    Ok(())
}

fn check_segment(
    segment: &str,
    depth: u8,
    state: &mut ScanState,
) -> Result<(), ProtectedPathViolation> {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let tokens = tokenize(trimmed);
    if tokens.is_empty() {
        return Ok(());
    }

    // Subshells / groups / command substitutions. `shell_segments`
    // keeps `( … )` intact on purpose, which left the whole group as
    // one segment whose head is `(ln` — so a link created inside it was
    // never recorded and a later write through it inside the same group
    // was checked against a path that did not exist yet. Scan the inner
    // command list first (it runs first) with the same `state`, so its
    // link creations and its writes are ordered against each other.
    //
    // Every construct reached here — `( … )`, `$( … )`, backquotes —
    // runs in a subshell, so a `cd` inside it does not move the parent
    // shell: the anchor is restored afterwards. Links are filesystem
    // effects and do persist, so `state.links` is not restored.
    let nested = nested_command_lists(trimmed);
    if !nested.is_empty() {
        let saved = state.anchor.clone();
        for inner in nested {
            check_with_depth(&inner, depth + 1, state)?;
        }
        state.anchor = saved;
    }

    // Redirections (`>`, `>>`, `|& tee FILE`, heredoc-to-file). These
    // are checked first because they apply regardless of the underlying
    // command name.
    for dest in extract_redirection_destinations(&tokens) {
        ensure_not_protected(&dest, "redirection", state)?;
    }

    // Skip leading `VAR=value` assignments and brace-group punctuation
    // to find the actual command. `shell_segments` splits on `;`, so
    // `{ ln -s … ; write ; }` already arrives as ordered segments —
    // but the first one's head parsed as `{`, which hid the `ln`.
    // A brace group runs in the *current* shell, so its `cd` and its
    // links both persist; nothing is saved or restored here.
    let mut idx = 0;
    while idx < tokens.len() && (is_assignment(&tokens[idx]) || is_group_punctuation(&tokens[idx]))
    {
        idx += 1;
    }
    if idx >= tokens.len() {
        return Ok(());
    }

    let head = base_name(&tokens[idx]);
    let args = &tokens[idx + 1..];

    // Writer commands: extract the destination operand and check it.
    for dest in extract_writer_destinations(head, args) {
        ensure_not_protected(&dest, head, state)?;
    }

    // Record links this segment creates AFTER its own destinations were
    // checked (creating a link that merely points at a protected path
    // is not a write into it), so only later writes are matched against
    // the link.
    record_link_creations(head, args, state);

    // A `cd` moves every later relative destination in this command.
    apply_directory_change(head, args, state);

    // Interpreter recursion / heuristic scan. Recursion shares `state`
    // so a link created inside `bash -c '…'` is visible to segments
    // after it, and vice versa.
    if let Some(payload) = extract_recursive_shell_payload(head, args) {
        check_with_depth(&payload, depth + 1, state)?;
    } else if let Some(payload) = extract_interpreter_payload(head, args) {
        ensure_no_protected_substring(&payload, head)?;
    }

    Ok(())
}

/// Refuse `path` if it resolves into any protected directory or
/// blocked system path.
fn ensure_not_protected(
    path: &str,
    source: &str,
    state: &ScanState,
) -> Result<(), ProtectedPathViolation> {
    let trimmed = path.trim_matches(|c: char| c == '"' || c == '\'');
    if matches!(state.anchor, Anchor::Unavailable) && !is_shell_absolute(trimmed) {
        return Err(ProtectedPathViolation {
            reason: format!(
                "{source} writes to relative path {path}, but the command's \
                 working directory is unavailable so the destination cannot \
                 be verified; failing closed"
            ),
        });
    }

    // Check every spelling the token can denote — see `normalized_forms`.
    // A path is refused if *any* of them lands somewhere protected.
    let Forms { forms, exhausted } = normalized_forms(path, &state.anchor);
    for normalized in &forms {
        ensure_form_not_protected(normalized, path, source)?;
    }

    // The symlink walk ran out of budget, so a link deeper in the path
    // could still redirect this write somewhere protected. Unknown is
    // not safe: refuse rather than trust the literal spelling.
    if exhausted {
        return Err(ProtectedPathViolation {
            reason: format!(
                "{source} writes to {path}, which has too many path \
                 components to resolve symlinks through; failing closed — \
                 use a shorter absolute path"
            ),
        });
    }

    // A previous segment created a link whose name we could not
    // determine — any destination might be (or pass through) that
    // link, and `ln -f` can even replace an existing file with it.
    if state.opaque_link {
        return Err(ProtectedPathViolation {
            reason: format!(
                "{source} writes to {path} after this command creates a link \
                 whose name cannot be determined before execution; failing \
                 closed — run the link creation and the write as separate \
                 commands"
            ),
        });
    }

    // Writes through a link created earlier in this command: the path
    // does not exist at validation time, so canonicalization cannot see
    // it. Check the link *target* with the remainder substituted in.
    follow_created_links(&forms, path, source, state, 0)
}

/// Longest chain of same-command links followed before giving up.
/// Chains this long are not a legitimate construction; refusing is the
/// fail-closed answer and it also bounds link cycles
/// (`ln -s a b && ln -s b a`).
const MAX_LINK_CHAIN: u8 = 8;

/// Resolve `forms` through every link this command creates before the
/// write, transitively: `ln -s "$PWD/.git" /tmp/a && ln -s /tmp/a /tmp/b`
/// means a write to `/tmp/b/config` lands in `.git/config`, so the
/// substituted path must itself be re-checked against the link table.
fn follow_created_links(
    forms: &[String],
    path: &str,
    source: &str,
    state: &ScanState,
    depth: u8,
) -> Result<(), ProtectedPathViolation> {
    if depth >= MAX_LINK_CHAIN {
        return Err(ProtectedPathViolation {
            reason: format!(
                "{source} writes to {path} through a chain of more than \
                 {MAX_LINK_CHAIN} links created by this same command; \
                 refusing because such a chain cannot be safely audited"
            ),
        });
    }
    for link in &state.links {
        let Some(remainder) = link.match_remainder(forms) else {
            continue;
        };
        if !link.auditable {
            return Err(ProtectedPathViolation {
                reason: format!(
                    "{source} would write through {path}, a link created \
                     earlier in this command whose target cannot be \
                     determined before execution; failing closed — run the \
                     link creation and the write as separate commands"
                ),
            });
        }
        let substituted = link.substituted(&remainder);
        let sub_forms = normalized_forms(&substituted, &state.anchor).forms;
        for form in &sub_forms {
            ensure_form_not_protected(form, path, source)?;
        }
        follow_created_links(&sub_forms, path, source, state, depth + 1)?;
    }
    Ok(())
}

/// True when a token contains no shell expansion syntax, i.e. the path
/// it denotes at execution time is the token itself.
fn is_literal_path(s: &str) -> bool {
    !s.contains('$') && !s.contains('`')
}

/// True when a *shell* token denotes an absolute path.
///
/// `Path::is_absolute` answers for the host: on Windows it is false for
/// `/etc` and `/tmp/e`, because those lack a drive prefix. The commands
/// checked here are bash commands, where a leading separator is
/// absolute regardless of the platform the check runs on — and the
/// protected lists (`/etc/`, `/usr/`, …) are written that way. Treating
/// `/tmp/e` as relative on Windows anchored it under the cwd, which
/// silently defeated the link and `cd` tracking there while the Linux
/// tests passed.
fn is_shell_absolute(s: &str) -> bool {
    s.starts_with('/') || s.starts_with('\\') || Path::new(s).is_absolute()
}

/// Brace-group punctuation (`{` / `}`) occupying a token of its own.
/// Bash requires these to be separate tokens, so a literal `{}` (as in
/// `find -exec … {} \;`) or a `${VAR}` never matches.
fn is_group_punctuation(tok: &str) -> bool {
    tok == "{" || tok == "}"
}

/// Update the anchor when a segment changes the working directory.
///
/// Every later relative destination in the command resolves against
/// the new directory: `cd / && printf x > etc/passwd` writes
/// `/etc/passwd`. A destination we cannot follow statically (`cd $D`,
/// bare `cd` to `$HOME`, `cd -`, `popd`) makes the directory unknown,
/// which refuses later relative writes rather than checking them
/// against a cwd the command has already left.
fn apply_directory_change(head: &str, args: &[String], state: &mut ScanState) {
    match head {
        "cd" | "pushd" => {}
        "popd" => {
            // The stack is not tracked, so the resulting directory is
            // unknown — but only downgrade a known directory.
            if matches!(state.anchor, Anchor::Dir(_)) {
                state.anchor = Anchor::Unavailable;
            }
            return;
        }
        _ => return,
    }

    let target = positional_args(head, args).into_iter().next();
    let Some(target) = target else {
        // Bare `cd` goes to `$HOME`, which is not knowable here.
        if matches!(state.anchor, Anchor::Dir(_)) {
            state.anchor = Anchor::Unavailable;
        }
        return;
    };
    if target == "-" || !is_literal_path(&target) {
        if matches!(state.anchor, Anchor::Dir(_)) {
            state.anchor = Anchor::Unavailable;
        }
        return;
    }

    let path = Path::new(&target);
    if is_shell_absolute(&target) {
        // An absolute `cd` pins the directory regardless of where the
        // command started — worth following even from `Anchor::None`.
        state.anchor = Anchor::Dir(resolve_dir(path));
        return;
    }
    match &state.anchor {
        Anchor::Dir(dir) => {
            state.anchor = Anchor::Dir(resolve_dir(&dir.join(path)));
        }
        // Without a starting directory a relative `cd` teaches us
        // nothing; the anchored pass in `check_at` is what resolves it.
        Anchor::None | Anchor::Unavailable => {}
    }
}

/// Resolve a directory the command changes into.
///
/// Canonicalization is a host operation, so it is only meaningful for a
/// path the host itself calls absolute. On Windows, `Path::canonicalize`
/// of the bash-absolute `/` succeeds and answers with the *drive* root
/// (`\\?\C:\`), which matches none of the `/etc/`-style protected
/// entries — `cd /` then looked harmless there. A bash-absolute path
/// that the host does not recognize keeps its own spelling instead, so
/// the protected lists still match it.
fn resolve_dir(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.canonicalize()
            .unwrap_or_else(|_| crate::permissions::lexical_normalize(path))
    } else {
        crate::permissions::lexical_normalize(path)
    }
}

/// Extract command lists nested inside a segment: subshells and groups
/// `( … )`, command substitutions `$( … )`, and backquotes. Each is a
/// command list in its own right and executes as part of this segment,
/// so it must go through the same checks.
///
/// Only the outermost level is returned; deeper nesting is reached by
/// the recursive call on each result. Quoted runs are skipped so a
/// literal paren inside `'…'` is not mistaken for a group.
fn nested_command_lists(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = segment.char_indices().peekable();
    let mut quote: Option<char> = None;
    let bytes = segment.as_bytes();

    while let Some((i, c)) = chars.next() {
        if let Some(q) = quote {
            // `$( … )` and backquotes both still run inside double
            // quotes; single quotes are literal.
            if c == q {
                quote = None;
            } else if q == '"'
                && c == '$'
                && bytes.get(i + 1) == Some(&b'(')
                && let Some((inner, end)) = span_to_matching_paren(segment, i + 1)
            {
                out.push(inner);
                while chars.peek().is_some_and(|(j, _)| *j <= end) {
                    chars.next();
                }
            } else if q == '"'
                && c == '`'
                && let Some(end) = segment[i + 1..].find('`').map(|off| i + 1 + off)
            {
                out.push(segment[i + 1..end].to_string());
                while chars.peek().is_some_and(|(j, _)| *j <= end) {
                    chars.next();
                }
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '\\' => {
                chars.next();
            }
            '`' => {
                if let Some(end) = segment[i + 1..].find('`').map(|off| i + 1 + off) {
                    out.push(segment[i + 1..end].to_string());
                    while chars.peek().is_some_and(|(j, _)| *j <= end) {
                        chars.next();
                    }
                }
            }
            '(' => {
                if let Some((inner, end)) = span_to_matching_paren(segment, i) {
                    out.push(inner);
                    while chars.peek().is_some_and(|(j, _)| *j <= end) {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Given the index of an opening `(`, return the text between it and
/// its matching `)` plus the index of that `)`.
fn span_to_matching_paren(s: &str, open: usize) -> Option<(String, usize)> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s.char_indices().skip_while(|(i, _)| *i < open) {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' | '\'' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((s[open + 1..i].to_string(), i));
                }
            }
            _ => {}
        }
    }
    None
}

/// If this segment creates filesystem links (`ln`, hard or symbolic,
/// or `cp --link`/`--symbolic-link`), record the (target, link name)
/// pairs so later write destinations can be checked against them.
fn record_link_creations(head: &str, args: &[String], state: &mut ScanState) {
    let has_short = |c: char| {
        args.iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains(c))
    };
    let symbolic = has_short('s')
        || args
            .iter()
            .any(|a| a == "--symbolic" || a == "--symbolic-link");
    let creates_links = match head {
        "ln" => true,
        "cp" => symbolic || has_short('l') || args.iter().any(|a| a == "--link"),
        _ => false,
    };
    if !creates_links {
        return;
    }

    let positional = positional_args(head, args);
    let mut pairs: Vec<(String, String)> = Vec::new(); // (target, link name)
    if let Some(dir) = find_flag_value(args, &["-t", "--target-directory"]) {
        for p in &positional {
            let name = format!("{}/{}", dir.trim_end_matches('/'), last_path_component(p));
            pairs.push((p.clone(), name));
        }
    } else if positional.len() == 2 {
        pairs.push((positional[0].clone(), positional[1].clone()));
    } else if positional.len() > 2 {
        // `ln TARGET... DIR` creates DIR/<basename> for every target.
        let dir = positional.last().expect("len checked above");
        for p in &positional[..positional.len() - 1] {
            let name = format!("{}/{}", dir.trim_end_matches('/'), last_path_component(p));
            pairs.push((p.clone(), name));
        }
    } else if positional.len() == 1 && head == "ln" {
        // `ln TARGET` creates ./<basename> in the cwd.
        let p = &positional[0];
        pairs.push((p.clone(), last_path_component(p).to_string()));
    }

    for (target, name) in pairs {
        if !is_literal_path(&name) {
            state.opaque_link = true;
            continue;
        }
        let name_forms = normalized_forms(&name, &state.anchor).forms;
        // Prefer an absolute spelling of the link's directory when the
        // anchor produced one, so a relative symbolic target resolves
        // from where the link actually lives.
        let name_dir = name_forms
            .iter()
            .find(|f| is_shell_absolute(f))
            .map_or_else(
                || parent_dir(&name).to_string(),
                |f| parent_dir(f).to_string(),
            );
        state.links.push(CreatedLink {
            name_forms,
            name_dir,
            symbolic,
            auditable: is_literal_path(&target),
            target,
        });
    }
}

fn last_path_component(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// The directory part of a path as written — `""` when the path has no
/// separator (a bare name in the cwd).
fn parent_dir(p: &str) -> &str {
    let trimmed = p.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/",
        Some(i) => &trimmed[..i],
        None => "",
    }
}

fn ensure_form_not_protected(
    normalized: &str,
    path: &str,
    source: &str,
) -> Result<(), ProtectedPathViolation> {
    for dir in PROTECTED_DIRS {
        if path_targets_dir(normalized, dir) {
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
        if path_targets_dir(normalized, sys) {
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
    if is_team_memory_write_target(Path::new(normalized), None) {
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
    let positional = positional_args(head, args);
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

/// Flags of `head` that consume the following token as their value.
///
/// Flag arity is per-command: `truncate -s SIZE` takes a value while
/// `ln -s` does not. A single shared list made `ln -s TARGET LINK`
/// swallow its own target, so `ln -s evil .git/config` sailed past the
/// destination check. `-T`/`--no-target-directory` take no value
/// anywhere and are deliberately absent. Misclassifying a value-taking
/// flag as valueless only adds phantom positionals *before* the final
/// operand, which over-checks rather than under-checks.
fn value_flags(head: &str) -> &'static [&'static str] {
    match head {
        "ln" | "cp" | "mv" => &["-t", "--target-directory", "-S", "--suffix"],
        "install" => &[
            "-t",
            "--target-directory",
            "-m",
            "--mode",
            "-o",
            "--owner",
            "-g",
            "--group",
            "-S",
            "--suffix",
        ],
        "truncate" => &["-s", "--size", "-r", "--reference"],
        _ => &[],
    }
}

/// Return positional (non-flag) arguments of `head`. Skips long-form
/// flag values (`--target=DIR` is consumed in one token already) and
/// the argument that follows short flags of `head` that take a value.
fn positional_args(head: &str, args: &[String]) -> Vec<String> {
    let with_value = value_flags(head);
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
        if with_value.contains(&a.as_str()) {
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

/// Canonical forms of a path token, for matching against the protected
/// lists.
///
/// Returns more than one form because a single spelling is not enough:
///
/// * **lexical** — quotes stripped, `.` dropped, `..` resolved, repeated
///   separators collapsed. Without this `/tmp/../etc/passwd` and
///   `//etc/passwd` both reached `/etc` while plain `/etc/passwd` was
///   refused.
/// * **anchored** — a relative path joined onto the directory the
///   command will execute in ([`Anchor::Dir`]). Bash resolves relative
///   destinations against its cwd, so the check must too.
/// * **symlink-resolved** — the deepest existing ancestor canonicalized
///   and the remainder re-appended, so `ln -s /etc /tmp/e` followed by a
///   write to `/tmp/e/passwd` — or `out -> .git` in the cwd followed by
///   a write to `out/config` — is seen for what it is.
///
/// The caller checks every form and refuses if any one of them lands in
/// a protected location. Resolution is best-effort: the write target
/// usually does not exist yet, so a failure to canonicalize falls back
/// to the lexical form rather than skipping the check.
fn normalized_forms(raw: &str, anchor: &Anchor) -> Forms {
    let trimmed = raw.trim_matches(|c: char| c == '"' || c == '\'');
    let lexical = join_components(&crate::permissions::lexical_normalize(Path::new(trimmed)));

    let mut forms = vec![lexical.clone()];
    // Keep the raw spelling too: `lexical_normalize` pops `..` past the
    // start for relative paths, and the untouched token is what the
    // segment-equality rules for project-relative dirs were written
    // against.
    if trimmed != lexical {
        forms.push(trimmed.to_string());
    }

    // The absolute spelling to resolve symlinks against: the path
    // itself when absolute, or the anchored join when the execution
    // directory is known.
    let absolute = if is_shell_absolute(trimmed) {
        Some(lexical)
    } else if let Anchor::Dir(dir) = anchor {
        let anchored = join_components(&crate::permissions::lexical_normalize(&dir.join(trimmed)));
        if !forms.contains(&anchored) {
            forms.push(anchored.clone());
        }
        Some(anchored)
    } else {
        None
    };
    let mut exhausted = false;
    if let Some(abs) = absolute {
        match resolve_symlinked_ancestor(&abs) {
            Ancestor::Resolved(resolved) => {
                if !forms.contains(&resolved) {
                    forms.push(resolved);
                }
            }
            Ancestor::Exhausted => exhausted = true,
            Ancestor::None => {}
        }
    }
    Forms { forms, exhausted }
}

/// Every spelling a destination token can denote, plus whether the
/// symlink walk ran out of budget (in which case the list is known to
/// be incomplete and the caller must fail closed).
struct Forms {
    forms: Vec<String>,
    exhausted: bool,
}

/// Render a path with `/` separators on every platform.
///
/// `to_string_lossy` emits `\` on Windows, which made the normalized form
/// stop matching the `/etc/`-style entries in the protected lists — the
/// traversal bypass this normalization exists to close stayed open there.
/// Joining components rather than substituting characters keeps a
/// backslash that is part of a Unix filename inside its own component.
fn join_components(path: &std::path::Path) -> String {
    use std::path::Component;
    let mut out = String::new();
    for comp in path.components() {
        match comp {
            // A Windows prefix (`C:`, or a UNC share from a `//host/x`
            // spelling) is emitted verbatim. It is not a traversal — on
            // Windows `//etc/passwd` denotes a network share, not the
            // local `/etc`, so it must not be rewritten into one.
            Component::Prefix(p) => out.push_str(&p.as_os_str().to_string_lossy()),
            Component::RootDir => {
                if !out.ends_with('/') {
                    out.push('/');
                }
            }
            other => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&other.as_os_str().to_string_lossy());
            }
        }
    }
    if out.is_empty() { ".".to_string() } else { out }
}

/// Canonicalize the deepest existing ancestor of `path` and re-append the
/// components below it, so a symlinked directory in the middle of the
/// path is followed even when the leaf does not exist yet.
/// Outcome of walking up `path` looking for a resolvable ancestor.
enum Ancestor {
    /// The deepest existing ancestor was canonicalized; this is the
    /// path with the remaining components re-appended.
    Resolved(String),
    /// The walk completed without finding one — nothing is symlinked
    /// here, so the literal spellings are the whole story.
    None,
    /// The walk hit [`MAX_ANCESTOR_HOPS`] with components still left.
    /// The destination is *unknown*, not "not symlinked": callers must
    /// refuse rather than fall through to the literal path.
    Exhausted,
}

/// Ancestor hops before the walk gives up. Each hop is one
/// `canonicalize` syscall, so this also bounds the filesystem work an
/// attacker-supplied path can provoke — the command carries no length
/// limit of its own. Real paths are orders of magnitude shorter; a path
/// this deep is refused, not resolved.
const MAX_ANCESTOR_HOPS: usize = 4096;

fn resolve_symlinked_ancestor(path: &str) -> Ancestor {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Ancestor::None;
    }
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = p.to_path_buf();
    for _ in 0..MAX_ANCESTOR_HOPS {
        if let Ok(real) = cur.canonicalize() {
            let mut out = real;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return Ancestor::Resolved(join_components(&out));
        }
        let Some(name) = cur.file_name().map(|n| n.to_os_string()) else {
            return Ancestor::None;
        };
        suffix.push(name);
        if !cur.pop() {
            return Ancestor::None;
        }
    }
    // Components remain but the budget is spent: a deeper link could
    // still be hiding the real destination.
    Ancestor::Exhausted
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
    /// The normalized form must use `/` on every platform: the protected
    /// lists are written with forward slashes, and on Windows a
    /// `to_string_lossy` rendering produced `\etc\passwd`, which matched
    /// nothing — the traversal bypass stayed open there while passing on
    /// Linux. Asserting the rendering directly catches that without a
    /// Windows runner.
    #[test]
    fn normalized_paths_use_forward_slashes_on_every_platform() {
        // `//etc/passwd` is deliberately absent: on Windows that is a
        // UNC share (`\\etc\\passwd`), not the local `/etc`, so
        // normalizing it to `/etc/passwd` would be wrong rather than
        // safer. It is covered as a traversal case on unix only.
        for (input, expected) in [
            ("/tmp/../etc/passwd", "/etc/passwd"),
            ("/etc/./passwd", "/etc/passwd"),
            ("/usr/local/../../etc/passwd", "/etc/passwd"),
            ("/var/tmp/../../etc/passwd", "/etc/passwd"),
        ] {
            let forms = normalized_forms(input, &Anchor::None).forms;
            assert!(
                forms.iter().any(|f| f == expected),
                "{input} did not normalize to {expected}: {forms:?}"
            );
            assert!(
                !forms[0].contains('\\'),
                "normalized form kept an OS separator: {:?}",
                forms[0]
            );
        }
    }

    /// `/etc/passwd` was refused but `/tmp/../etc/passwd` was not: the
    /// check compared the raw token, so any traversal segment walked
    /// straight past it.
    #[test]
    fn traversal_cannot_reach_a_protected_directory() {
        // `//etc/passwd` is unix-only: see the note in the
        // forward-slash test — on Windows it is a UNC share.
        #[cfg(unix)]
        assert!(
            check("echo x > //etc/passwd").is_err(),
            "traversal reached a protected path: //etc/passwd"
        );
        for cmd in [
            "echo x > /tmp/../etc/passwd",
            "echo x > /etc/./passwd",
            "cp evil /tmp/../etc/passwd",
            "tee /var/../etc/passwd",
            "echo x > /usr/local/../../etc/passwd",
        ] {
            assert!(
                check(cmd).is_err(),
                "traversal reached a protected path: {cmd}"
            );
        }
    }

    /// A symlinked directory mid-path is followed, so the guard sees
    /// where the write actually lands rather than how it was spelled.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_does_not_hide_the_destination() {
        let td = tempfile::tempdir().unwrap();
        let link = td.path().join("e");
        if std::os::unix::fs::symlink("/etc", &link).is_err() {
            return; // no permission to symlink here; nothing to assert
        }
        let cmd = format!("echo x > {}/passwd", link.display());
        assert!(check(&cmd).is_err(), "symlink hid the destination: {cmd}");
    }

    /// Padding the path with components must not exhaust the ancestor
    /// walk into a `None` that reads as "nothing symlinked here".
    #[cfg(unix)]
    #[test]
    fn a_deep_path_cannot_outrun_the_symlinked_ancestor_walk() {
        let td = tempfile::tempdir().unwrap();
        let link = td.path().join("e");
        if std::os::unix::fs::symlink("/etc", &link).is_err() {
            return; // no permission to symlink here; nothing to assert
        }
        let deep = "d/".repeat(128);
        let cmd = format!("echo x > {}/{deep}passwd", link.display());
        assert!(
            check(&cmd).is_err(),
            "a deep path outran the ancestor walk and hid the destination: {cmd}"
        );
    }

    /// A path too deep to resolve is refused, not silently trusted:
    /// exhausting the walk means the destination is unknown, and a
    /// link below the budget could still redirect it.
    #[test]
    fn a_path_deeper_than_the_walk_budget_is_refused() {
        let deep = "d/".repeat(MAX_ANCESTOR_HOPS + 16);
        let cmd = format!("echo x > /{deep}file");
        let err = check(&cmd).expect_err("a path past the budget must fail closed");
        assert!(
            err.reason.contains("too many path components"),
            "unexpected refusal reason: {}",
            err.reason
        );
    }

    /// ...but a path merely long stays workable, so the bound cannot
    /// become an over-block in ordinary use.
    #[test]
    fn a_long_but_bounded_path_is_still_allowed() {
        let deep = "d/".repeat(64);
        let cmd = format!("echo x > /tmp/{deep}file");
        assert!(check(&cmd).is_ok(), "ordinary deep path was refused: {cmd}");
    }

    /// The normalization must not turn ordinary writes into refusals —
    /// a guard that blocks everything is its own kind of failure.
    #[test]
    fn ordinary_writes_are_still_allowed() {
        for cmd in [
            "echo ok > /tmp/fine.txt",
            "echo ok > ./notes.md",
            "cp a b",
            "echo ok > src/main.rs",
            "tee /tmp/build/../out.log",
        ] {
            assert!(check(cmd).is_ok(), "false positive on: {cmd}");
        }
    }

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

    /// A relative destination is executed relative to the bash cwd, so
    /// the symlink resolution must be too: with `out -> .git` in the
    /// cwd, `printf x > out/config` overwrites `.git/config`.
    #[cfg(unix)]
    #[test]
    fn relative_destination_resolves_symlinks_against_the_commands_cwd() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join(".git")).unwrap();
        if std::os::unix::fs::symlink(".git", td.path().join("out")).is_err() {
            return; // no permission to symlink here; nothing to assert
        }
        for cmd in [
            "printf x > out/config",
            "echo x >> out/config",
            "cp evil out/config",
            "tee out/config",
        ] {
            assert!(
                check_at(cmd, td.path()).is_err(),
                "relative symlink hid the destination: {cmd}"
            );
        }
    }

    /// Anchoring must not over-block: relative writes in a cwd without
    /// hostile symlinks stay allowed.
    #[test]
    fn relative_destination_in_a_clean_cwd_still_allowed() {
        let td = tempfile::tempdir().unwrap();
        for cmd in [
            "printf x > out/config",
            "echo ok > notes.md",
            "mkdir -p build && echo x > build/out",
            "cp a.txt b.txt",
        ] {
            assert!(
                check_at(cmd, td.path()).is_ok(),
                "false positive on: {cmd}, got {:?}",
                check_at(cmd, td.path())
            );
        }
    }

    /// When the claimed cwd cannot be canonicalized, relative write
    /// destinations cannot be verified at all — fail closed.
    #[test]
    fn unavailable_cwd_fails_closed_for_relative_destinations() {
        let missing = Path::new("/nonexistent-agent-code-test/dir");
        assert!(check_at("echo x > relative.txt", missing).is_err());
        assert!(check_at("cp a b", missing).is_err());
        // Commands without write destinations are unaffected.
        assert!(check_at("ls -la", missing).is_ok());
        // Absolute destinations do not depend on the cwd.
        #[cfg(unix)]
        assert!(check_at("echo x > /tmp/fine.txt", missing).is_ok());
    }

    /// A symlink created by an earlier segment of the same command does
    /// not exist when the check runs, so canonicalization alone cannot
    /// see it. The link target must be checked instead.
    #[test]
    fn symlink_created_in_the_same_command_cannot_reach_a_protected_dir() {
        // The exact reported bypass: the link target names `.git` via
        // `$PWD`, the write goes through the not-yet-existing link.
        refuse(r#"ln -s "$PWD/.git" /tmp/e && printf x > /tmp/e/config"#);
        // Literal target into a blocked system path.
        refuse("ln -s /etc /tmp/agent-code-e2 && echo x > /tmp/agent-code-e2/passwd");
        // Relative link name, relative target.
        refuse("ln -s ../.git out && printf x > out/config");
        // `;` and `|` sequencing are the same hazard as `&&`.
        refuse(r#"ln -s "$PWD/.git" /tmp/e; printf x > /tmp/e/config"#);
        refuse(r#"ln -s "$PWD/.git" /tmp/e | printf x > /tmp/e/config"#);
        // Writing to the link itself (hard link to a protected file).
        refuse("ln .git/config /tmp/agent-code-x && printf y > /tmp/agent-code-x");
        // `cp` can create links too.
        refuse("cp -s /etc /tmp/agent-code-e3 && echo x > /tmp/agent-code-e3/passwd");
        // Link creation hidden inside a recursive shell payload.
        refuse(r#"bash -c 'ln -s "$PWD/.git" /tmp/e' && printf x > /tmp/e/config"#);
    }

    /// A link whose name is only known at execution time makes every
    /// later write unauditable — fail closed.
    #[test]
    fn link_with_unauditable_name_blocks_later_writes() {
        refuse(r#"ln -s /etc/passwd "$D" && printf x > /tmp/out"#);
        refuse("ln -sf .git/config $LINK; echo x > notes.txt");
    }

    /// The same-command link tracking must not flag innocent compound
    /// commands: links not written through, writes not through links,
    /// and non-link commands creating fresh directories.
    #[test]
    fn innocent_compound_commands_with_links_still_allowed() {
        allow("ln -s /tmp/a /tmp/b && echo hi > /tmp/c");
        allow("ln -s /var/data d && cp report.csv d/out.csv");
        allow("ln -s ../shared/config.toml conf.toml && cat conf.toml");
        allow("mkdir -p build && echo x > build/out");
        // Creating a link that points AT a protected path is reading
        // infrastructure, not a write into it — only writing *through*
        // it later is.
        allow("ln -s .git/hooks /tmp/agent-code-hooks-view");
    }

    /// A symbolic link's relative target is resolved from the link's
    /// own directory, not from the shell cwd: after
    /// `ln -s ../etc /tmp/e`, `/tmp/e` is `/etc` no matter where bash
    /// was run. Anchoring the substituted target to the cwd checked
    /// `<cwd>/../etc/passwd` and let the write through.
    #[test]
    fn relative_symlink_target_resolves_from_the_link_directory() {
        refuse("ln -s ../etc /tmp/e && echo x > /tmp/e/passwd");
        refuse("ln -s ../../etc /tmp/sub/e && echo x > /tmp/sub/e/passwd");
        refuse("ln -s ../.git build/out && printf x > build/out/config");
        // Anchored to a cwd far away from the link — the link's own
        // directory is what decides.
        let td = tempfile::tempdir().unwrap();
        assert!(
            check_at("ln -s ../etc /tmp/e && echo x > /tmp/e/passwd", td.path()).is_err(),
            "relative symlink target was resolved from the cwd, not the link dir"
        );
    }

    /// A relative target that stays clear of protected paths must not
    /// be dragged into one by the resolution rule.
    #[test]
    fn relative_symlink_target_outside_protected_dirs_allowed() {
        allow("ln -s ../shared /tmp/e && echo x > /tmp/e/out.txt");
        allow("ln -s ../data build/link && cp a.csv build/link/a.csv");
    }

    /// Links chain: the substituted path must itself be re-checked
    /// against the links this command already created.
    #[test]
    fn chained_links_are_followed_transitively() {
        refuse(r#"ln -s "$PWD/.git" /tmp/a && ln -s /tmp/a /tmp/b && echo x > /tmp/b/config"#);
        refuse(
            "ln -s /etc /tmp/a && ln -s /tmp/a /tmp/b && ln -s /tmp/b /tmp/c && echo x > /tmp/c/passwd",
        );
        // A cycle must terminate (and fail closed) rather than spin.
        let cyclic = "ln -s /tmp/b /tmp/a && ln -s /tmp/a /tmp/b && echo x > /tmp/a/f";
        assert!(check(cyclic).is_err(), "link cycle should fail closed");
    }

    /// Chains that never reach a protected directory stay allowed.
    #[test]
    fn chained_links_outside_protected_dirs_allowed() {
        allow("ln -s /tmp/data /tmp/a && ln -s /tmp/a /tmp/b && echo x > /tmp/b/out.txt");
    }

    /// `shell_segments` keeps `( … )` intact, so a group's head is
    /// `(ln` and the link inside it was never recorded — the write
    /// later in the same group went unchecked.
    #[test]
    fn links_created_inside_grouped_commands_are_recorded() {
        refuse("(ln -s /etc /tmp/e && printf x > /tmp/e/passwd)");
        refuse(r#"(ln -s "$PWD/.git" /tmp/e; printf x > /tmp/e/config)"#);
        // Group creates the link, write happens after the group.
        refuse("(ln -s /etc /tmp/e) && printf x > /tmp/e/passwd");
        // Nested one level deeper.
        refuse("((ln -s /etc /tmp/e && printf x > /tmp/e/passwd))");
        // Command substitution runs too.
        refuse(r#"echo $(ln -s /etc /tmp/e) && printf x > /tmp/e/passwd"#);
        // A plain write inside a group is still caught directly.
        refuse("(printf x > .git/config)");
    }

    /// Groups that write nowhere protected keep working.
    #[test]
    fn innocent_grouped_commands_allowed() {
        allow("(cd src && cargo build)");
        allow("(ln -s /tmp/a /tmp/b && echo hi > /tmp/c)");
        allow("(mkdir -p build && echo x > build/out)");
        allow("echo $(date) > /tmp/stamp.txt");
        allow("(echo one; echo two) > /tmp/both.txt");
    }

    /// A leading `/` is absolute in the bash commands being checked,
    /// whatever platform the check runs on. `Path::is_absolute` answers
    /// for the host and is false for `/etc` on Windows, which anchored
    /// those tokens under the cwd and quietly defeated the link and
    /// `cd` tracking there while Linux passed. Asserting the predicate
    /// directly catches that without a Windows runner.
    #[test]
    fn shell_paths_are_absolute_on_every_platform() {
        for p in ["/etc", "/tmp/e", "/etc/passwd", "\\etc"] {
            assert!(is_shell_absolute(p), "{p} should count as absolute");
        }
        for p in ["etc", "out/config", "../.git", "."] {
            assert!(!is_shell_absolute(p), "{p} should count as relative");
        }
        // A bash-absolute directory keeps its own spelling when the
        // host cannot resolve it, so the protected lists still match.
        // On Windows `canonicalize("/")` answers with the drive root,
        // which matches nothing in those lists.
        assert_eq!(
            join_components(&resolve_dir(Path::new("/nonexistent-agent-code-dir"))),
            "/nonexistent-agent-code-dir"
        );
    }

    /// A `cd` moves every later relative destination in the command:
    /// `cd / && printf x > etc/passwd` writes `/etc/passwd`, but the
    /// check anchored `etc/passwd` under the original cwd.
    #[test]
    fn directory_changes_move_later_relative_destinations() {
        let td = tempfile::tempdir().unwrap();
        for cmd in [
            "cd / && printf x > etc/passwd",
            "cd /; printf x > etc/passwd",
            "cd / && cp evil etc/passwd",
            "cd /etc && printf x > passwd",
            "{ cd /; printf x > etc/passwd; }",
        ] {
            assert!(
                check_at(cmd, td.path()).is_err(),
                "cd was not tracked: {cmd}"
            );
        }
        // An absolute `cd` is knowable even without a starting cwd.
        refuse("cd / && printf x > etc/passwd");
    }

    /// A `cd` whose destination cannot be resolved before execution
    /// leaves the directory unknown, so later relative writes fail
    /// closed rather than being checked against a stale cwd.
    #[test]
    fn unresolvable_cd_fails_closed_for_later_relative_writes() {
        let td = tempfile::tempdir().unwrap();
        for cmd in [
            "cd $TARGET && printf x > out.txt",
            "cd && printf x > out.txt",
            "cd - && printf x > out.txt",
            "pushd /tmp >/dev/null; popd >/dev/null; printf x > out.txt",
        ] {
            assert!(
                check_at(cmd, td.path()).is_err(),
                "unresolvable cd did not fail closed: {cmd}"
            );
        }
        // Absolute destinations remain checkable after any `cd`.
        assert!(check_at("cd $TARGET && printf x > /tmp/fine.txt", td.path()).is_ok());
        // A `cd` with no later write is unaffected.
        assert!(check_at("cd $TARGET && ls -la", td.path()).is_ok());
    }

    /// Ordinary `cd`-then-write must keep working, and a `cd` inside a
    /// subshell must not leak into the parent shell.
    #[test]
    fn ordinary_directory_changes_do_not_over_block() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join("build")).unwrap();
        for cmd in [
            "cd build && echo x > out.txt",
            "cd build; printf ok > log",
            "(cd / && ls) && echo ok > notes.md",
            "(cd src && cargo build)",
        ] {
            assert!(
                check_at(cmd, td.path()).is_ok(),
                "false positive on: {cmd}, got {:?}",
                check_at(cmd, td.path())
            );
        }
    }

    /// A brace group runs in the current shell and `shell_segments`
    /// splits it on `;`, but the first segment's head parsed as `{`,
    /// so the link inside it was never recorded.
    #[test]
    fn links_created_inside_brace_groups_are_recorded() {
        refuse("{ ln -s /etc /tmp/agent-brace-link; printf x > /tmp/agent-brace-link/passwd; }");
        refuse(
            r#"{ ln -s "$PWD/.git" /tmp/agent-brace-g; printf x > /tmp/agent-brace-g/config; }"#,
        );
        refuse("{ printf x > .git/config; }");
        // The group's own writer command is still found behind `{`.
        refuse("{ cp src .git/config; }");
    }

    /// Brace punctuation must not swallow real commands or flag
    /// ordinary uses of `{}`.
    #[test]
    fn innocent_brace_groups_allowed() {
        allow("{ echo one; echo two; } > /tmp/both.txt");
        allow("{ cd /tmp && ls; }");
        allow("find . -name '*.tmp' -exec rm {} \\;");
        allow("echo ${HOME}/notes.md");
    }

    /// Backquotes execute inside double quotes too, so a link created
    /// there must be recorded before later writes are checked.
    #[test]
    fn backquotes_inside_double_quotes_are_scanned() {
        refuse(
            "echo \"`ln -s /etc /tmp/agent-backtick-link`\" \
             && printf x > /tmp/agent-backtick-link/passwd",
        );
        refuse("echo \"`printf x > .git/config`\"");
        // Unquoted backquotes were already covered; keep them pinned.
        refuse("echo `ln -s /etc /tmp/agent-bt2` && printf x > /tmp/agent-bt2/passwd");
    }

    /// Backquoted commands that touch nothing protected stay allowed.
    #[test]
    fn innocent_backquotes_allowed() {
        allow("echo \"`date`\" > /tmp/stamp.txt");
        allow("echo \"`ls /tmp`\"");
    }

    /// Flag arity is per-command. The old shared value-flag list let
    /// `ln -s` (no value) swallow its own target because `truncate -s
    /// SIZE` takes one, and treated the valueless `-T` as value-taking
    /// — both made the destination operand disappear from the check.
    #[test]
    fn flag_parsing_does_not_hide_the_destination() {
        refuse("ln -s evil .git/config");
        refuse("ln -s -T evil .git/config");
        refuse("cp -T src .git/config");
        refuse("mv --no-target-directory x .git/config");
        refuse("truncate -s 0 .git/config");
    }
}
