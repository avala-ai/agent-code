//! Permission system.
//!
//! Controls which tool operations are allowed. Checks are run
//! before every tool execution. The system supports three modes:
//!
//! - `Allow` — execute without asking
//! - `Deny` — block with a reason
//! - `Ask` — prompt the user interactively
//!
//! Rules can be configured per-tool and per-pattern (e.g., allow
//! `Bash` for `git *` commands, deny `FileWrite` outside the project).

pub mod tracking;

use std::path::{Path, PathBuf};

use crate::config::{PermissionMode, PermissionRule, PermissionsConfig};

/// Decision from a permission check.
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// Tool execution is allowed.
    Allow,
    /// Tool execution is denied with a reason.
    Deny(String),
    /// User should be prompted with this message.
    Ask(String),
}

/// Checks permissions for tool operations based on configured rules.
pub struct PermissionChecker {
    /// Live default mode. Interior mutability so a UI can switch
    /// AcceptEdits / Plan / Ask mid-turn without rebuilding the checker
    /// or waiting on the query-engine mutex (M0 mid-turn mode).
    default_mode: std::sync::RwLock<PermissionMode>,
    rules: Vec<PermissionRule>,
    /// Project root used for runtime checks (e.g. team-memory).
    /// `None` means "no project root known" — runtime checks that
    /// require it become best-effort.
    project_root: Option<PathBuf>,
    /// When set, read-only tools may only touch paths inside this root.
    /// `None` (the default) leaves reads unrestricted, preserving the
    /// interactive agent's behavior. Set for confined workers such as the
    /// AMR security-scan map phase, so a prompt injection in scanned code
    /// cannot read files (e.g. `~/.ssh`) outside the scan target.
    read_scope: Option<PathBuf>,
}

impl PermissionChecker {
    /// Create from configuration.
    pub fn from_config(config: &PermissionsConfig) -> Self {
        Self {
            default_mode: std::sync::RwLock::new(config.default_mode),
            rules: config.rules.clone(),
            project_root: None,
            read_scope: None,
        }
    }

    /// Create a checker that allows everything (for testing or bypass mode).
    pub fn allow_all() -> Self {
        Self {
            default_mode: std::sync::RwLock::new(PermissionMode::Allow),
            rules: Vec::new(),
            project_root: None,
            read_scope: None,
        }
    }

    /// Current default permission mode (lock-friendly read).
    pub fn default_mode(&self) -> PermissionMode {
        self.default_mode
            .read()
            .map(|g| *g)
            .unwrap_or(PermissionMode::Ask)
    }

    /// Update the default mode live (mid-turn Shift+Tab, etc.).
    pub fn set_default_mode(&self, mode: PermissionMode) {
        if let Ok(mut g) = self.default_mode.write() {
            *g = mode;
        }
    }

    /// Builder: pin the project root used for runtime path checks
    /// (currently the team-memory write protection). The model's
    /// write tools refuse any path that resolves inside
    /// `<project_root>/.agent/team-memory/`.
    #[must_use]
    pub fn with_project_root(mut self, project_root: PathBuf) -> Self {
        self.project_root = Some(project_root);
        self
    }

    /// Builder: confine read-only tools to `root`. Reads of paths that
    /// resolve outside `root` are denied. Used by sandboxed workers (AMR
    /// scan map phase) so scanned code cannot exfiltrate local files.
    #[must_use]
    pub fn with_read_scope(mut self, root: PathBuf) -> Self {
        self.read_scope = Some(root);
        self
    }

    /// Confine a read-only tool call to [`Self::read_scope`]. With no scope
    /// set (the default) reads are always allowed, so the interactive
    /// agent is unaffected. With a scope set, a path argument that resolves
    /// outside the scope is denied; a call with no explicit path (e.g. a
    /// `Grep`/`Glob` that defaults to the working directory) is allowed.
    pub fn check_read_scope(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> PermissionDecision {
        let Some(ref scope) = self.read_scope else {
            return PermissionDecision::Allow;
        };
        // Validate EVERY path-bearing argument, not just the first. Different
        // read tools read different fields (`FileRead` uses `file_path`,
        // `Grep`/`Glob` use `path`), and a call may set several — checking
        // only one lets `{"file_path": <in-scope>, "path": "/etc"}` slip the
        // out-of-scope path past the gate.
        let mut checked_any = false;
        for key in ["file_path", "path"] {
            if let Some(p) = input
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                checked_any = true;
                if !read_scope_allows(scope, p) {
                    return PermissionDecision::Deny(format!(
                        "read outside the scan scope is not allowed: {p}"
                    ));
                }
            }
        }
        // No explicit path: `Grep`/`Glob` default to the process working
        // directory, which must itself be in scope.
        if !checked_any {
            let cwd = std::env::current_dir().unwrap_or_default();
            if !read_scope_allows(scope, &cwd.to_string_lossy()) {
                return PermissionDecision::Deny(format!(
                    "read outside the scan scope is not allowed: {}",
                    cwd.display()
                ));
            }
        }

        // A `Glob` `pattern` is joined onto the search root, so an absolute
        // pattern (`/etc/**`) or one containing `..` (`../.ssh/*`) escapes the
        // scope even when the `path` argument is in-scope. This applies ONLY
        // to `Glob`: `Grep`'s `pattern` is a content regex, not a path, so
        // regexes like `../` or `/admin` are legitimate and must not be blocked.
        if tool_name == "Glob"
            && let Some(pattern) = input.get("pattern").and_then(|v| v.as_str())
            && glob_pattern_escapes(pattern)
        {
            return PermissionDecision::Deny(format!(
                "glob pattern may not escape the scan scope: {pattern}"
            ));
        }

        PermissionDecision::Allow
    }

    /// True when a read scope is configured (AMR worker confinement is
    /// active). Recursive tools use this to decide whether to constrain the
    /// files their traversal reaches.
    pub fn has_read_scope(&self) -> bool {
        self.read_scope.is_some()
    }

    /// Whether `path` may be read under the active read scope. With no scope
    /// set (the interactive agent) this is always true. With a scope set it
    /// applies the same containment + hidden-file rules as
    /// [`Self::check_read_scope`].
    ///
    /// The permission gate validates a tool's path *arguments*, but a
    /// recursive `Grep`/`Glob` reaches descendants that were never arguments
    /// (e.g. `.env` under an in-scope root). Those tools call this to filter
    /// their own results back down to the scope, so recursion cannot exfiltrate
    /// a hidden or out-of-scope file the gate would have denied as an argument.
    pub fn read_scope_allows_path(&self, path: &Path) -> bool {
        match &self.read_scope {
            None => true,
            Some(scope) => read_scope_allows(scope, &path.to_string_lossy()),
        }
    }

    /// Check whether a tool operation is permitted.
    ///
    /// Evaluates in order: protected paths, explicit rules, default mode.
    /// The first match wins.
    pub fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Block writes to protected directories regardless of rules.
        if is_write_tool(tool_name) {
            if let Some(reason) = check_protected_path(input) {
                return PermissionDecision::Deny(reason);
            }
            if let Some(reason) = self.check_team_memory_target(input) {
                return PermissionDecision::Deny(reason);
            }
        }

        // Check explicit rules.
        for rule in &self.rules {
            if !matches_tool(&rule.tool, tool_name) {
                continue;
            }

            if let Some(ref pattern) = rule.pattern
                && !matches_input_pattern(pattern, input)
            {
                continue;
            }

            return mode_to_decision(rule.action, tool_name);
        }

        // Fall back to default mode (may have been updated mid-turn).
        mode_to_decision(self.default_mode(), tool_name)
    }

    /// Check for read-only operations (always allowed).
    pub fn check_read(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Read operations use a relaxed check — only explicit deny rules block.
        for rule in &self.rules {
            if !matches_tool(&rule.tool, tool_name) {
                continue;
            }
            if let Some(ref pattern) = rule.pattern
                && !matches_input_pattern(pattern, input)
            {
                continue;
            }
            if matches!(rule.action, PermissionMode::Deny) {
                return PermissionDecision::Deny(format!("Denied by rule for {tool_name}"));
            }
        }
        PermissionDecision::Allow
    }

    /// If this write targets `<project_root>/.agent/team-memory/...`,
    /// return a denial reason. Team memory is shared, version-controlled
    /// state — only the `/team-remember` slash command may add entries.
    /// The model's own write tools must never silently mutate it.
    fn check_team_memory_target(&self, input: &serde_json::Value) -> Option<String> {
        let raw = input.get("file_path").and_then(|v| v.as_str())?;
        if raw.is_empty() {
            return None;
        }
        if is_team_memory_write_target(Path::new(raw), self.project_root.as_deref()) {
            return Some(
                "Write to team-memory is blocked. The `.agent/team-memory/` directory \
                 is read-only to the agent — use the `/team-remember` slash command \
                 to add entries."
                    .into(),
            );
        }
        None
    }
}

/// True if `target` resolves inside any project's team-memory directory
/// (`<project_root>/.agent/team-memory/`).
///
/// Two-pronged: when `project_root` is provided, canonicalize and
/// compare prefixes (handles symlinks and `..`). Independently, do a
/// component-aware substring check on the raw path so we still refuse
/// obvious team-memory writes when the project root is unknown
/// (test environments, scheduled executors, allow-all checker).
pub fn is_team_memory_write_target(target: &Path, project_root: Option<&Path>) -> bool {
    if let Some(root) = project_root {
        let team_dir = root.join(".agent").join("team-memory");
        if path_is_inside(target, &team_dir) {
            return true;
        }
    }
    // Component check on the raw input as a fallback. Catches the
    // common path shape `.../.agent/team-memory/...` regardless of
    // whether the parent dirs exist on disk.
    contains_team_memory_components(target)
}

/// Returns true when `path`, after light normalization, lives under
/// `dir`. Tries the canonical form first (resolves symlinks); falls
/// back to lexical comparison when canonicalization fails — e.g. the
/// target file does not exist yet, which is the common case for a
/// would-be `FileWrite`.
fn path_is_inside(path: &Path, dir: &Path) -> bool {
    if let (Ok(p), Ok(d)) = (path.canonicalize(), dir.canonicalize())
        && p.starts_with(&d)
    {
        return true;
    }

    // Lexical fallback. Anchor relative paths against the dir's parent so
    // a relative `.agent/team-memory/foo.md` still resolves against the
    // project root.
    let abs_path: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(parent) = dir.parent().and_then(|p| p.parent()) {
        // dir is `<root>/.agent/team-memory`; parent.parent() is `<root>`.
        parent.join(path)
    } else {
        path.to_path_buf()
    };
    let normalized = lexical_normalize(&abs_path);
    let dir_norm = lexical_normalize(dir);
    normalized.starts_with(&dir_norm)
}

/// Lexically normalize a path: collapse `.` and `..` components without
/// touching the filesystem. Sufficient for prefix comparisons against a
/// known directory.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True when `path`'s components contain `.agent` immediately followed
/// by `team-memory`. Used as the project-root-less fallback so the
/// invariant holds in test environments and for `allow_all` checkers.
fn contains_team_memory_components(path: &Path) -> bool {
    let mut prev_was_dot_agent = false;
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy();
        if prev_was_dot_agent && s == "team-memory" {
            return true;
        }
        prev_was_dot_agent = s == ".agent";
    }
    false
}

fn matches_tool(rule_tool: &str, tool_name: &str) -> bool {
    rule_tool == "*" || rule_tool.eq_ignore_ascii_case(tool_name)
}

fn matches_input_pattern(pattern: &str, input: &serde_json::Value) -> bool {
    // Match against common input fields: command, file_path, pattern.
    let input_str = input
        .get("command")
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("pattern"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    glob_match(pattern, input_str)
}

/// Simple glob matching (supports `*` and `?`).
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    glob_match_inner(&pattern_chars, &text_chars)
}

fn glob_match_inner(pattern: &[char], text: &[char]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // '*' matches zero or more characters.
            glob_match_inner(&pattern[1..], text)
                || (!text.is_empty() && glob_match_inner(pattern, &text[1..]))
        }
        (Some('?'), Some(_)) => glob_match_inner(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p == t => glob_match_inner(&pattern[1..], &text[1..]),
        _ => false,
    }
}

/// Directories that should never be written to by the agent.
///
/// Crate-visible so the Bash tool can apply the same gate to shell
/// invocations that route around the FileEdit/FileWrite/MultiEdit/
/// NotebookEdit tools (e.g. `cp src .git/config`, `printf evil >
/// .git/config`, `bash -c '... > .git/config'`). Keep the constant in
/// a single place so adding a new protected directory updates every
/// surface at once.
/// True when `raw` resolves inside `scope`. Canonicalizes both sides so a
/// symlink or `..` traversal cannot escape the scope for an existing file;
/// a non-existent path falls back to the unresolved absolute path (the read
/// will fail anyway, so the exfiltration risk is only for real files).
/// True if a `Glob` pattern would escape the scan root when joined onto it.
///
/// A `Glob` pattern is appended to the search root, so an absolute pattern
/// (`/etc/**`) or one with a `..` traversal (`../.ssh/*`) reads outside the
/// scope even when the `path` argument is in-scope. This must be
/// platform-independent: `Path::is_absolute()` is not (a Unix-style `/etc/**`
/// is not "absolute" on Windows, and `Path::components()` on Linux does not
/// split on `\`), so a scan running on Windows would otherwise miss `/etc/**`.
fn glob_pattern_escapes(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    // Leading `/` or `\` — absolute or UNC on some platform.
    let leading_sep = matches!(bytes.first(), Some(b'/' | b'\\'));
    // Windows drive prefix like `C:` (with or without a following separator).
    let drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    // A `..` segment under EITHER separator, regardless of host platform.
    let has_parent = pattern.split(['/', '\\']).any(|seg| seg == "..");
    Path::new(pattern).is_absolute() || leading_sep || drive_prefix || has_parent
}

fn read_scope_allows(scope: &Path, raw: &str) -> bool {
    let target = Path::new(raw);
    let abs_target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(target))
            .unwrap_or_else(|_| target.to_path_buf())
    };
    let canon_target = abs_target.canonicalize().unwrap_or(abs_target);
    let canon_scope = scope.canonicalize().unwrap_or_else(|_| scope.to_path_buf());
    if !canon_target.starts_with(&canon_scope) {
        return false;
    }
    // Deny hidden files/dirs beneath the root (`.env`, `.git/…`, other
    // dotfiles). The shard walk excludes them from the scan, and they
    // commonly hold local secrets, so a prompt-injected worker must not be
    // able to read them just because they sit inside the scan root.
    if let Ok(rel) = canon_target.strip_prefix(&canon_scope) {
        let hidden = rel.components().any(|c| {
            matches!(c, std::path::Component::Normal(name)
                if name.to_string_lossy().starts_with('.'))
        });
        if hidden {
            return false;
        }
    }
    true
}

pub(crate) const PROTECTED_DIRS: &[&str] = &[
    ".git/",
    ".git\\",
    ".husky/",
    ".husky\\",
    "node_modules/",
    "node_modules\\",
];

/// Returns true for tools that modify the filesystem.
fn is_write_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "FileWrite" | "FileEdit" | "MultiEdit" | "NotebookEdit" | "ApplyPatch"
    )
}

/// Check if the input targets a protected path. Returns the denial reason if so.
fn check_protected_path(input: &serde_json::Value) -> Option<String> {
    let path = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    for dir in PROTECTED_DIRS {
        if path.contains(dir) {
            let dir_name = dir.trim_end_matches(['/', '\\']);
            return Some(format!(
                "Write to {dir_name}/ is blocked. This is a protected directory."
            ));
        }
    }
    None
}

fn mode_to_decision(mode: PermissionMode, tool_name: &str) -> PermissionDecision {
    match mode {
        PermissionMode::Allow | PermissionMode::AcceptEdits => PermissionDecision::Allow,
        PermissionMode::Deny => {
            PermissionDecision::Deny(format!("Default mode denies {tool_name}"))
        }
        PermissionMode::Ask => PermissionDecision::Ask(format!("Allow {tool_name} to execute?")),
        PermissionMode::Plan => {
            PermissionDecision::Deny("Plan mode: only read-only operations allowed".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_scope_confines_explicit_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("in.txt"), "x").unwrap();
        let checker = PermissionChecker::allow_all().with_read_scope(dir.path().to_path_buf());

        // In-scope read is allowed.
        let inside = serde_json::json!({
            "file_path": dir.path().join("in.txt").to_string_lossy()
        });
        assert!(matches!(
            checker.check_read_scope("FileRead", &inside),
            PermissionDecision::Allow
        ));

        // Out-of-scope FileRead and out-of-scope Grep path are denied.
        let outside = serde_json::json!({"file_path": "/etc/hostname"});
        assert!(matches!(
            checker.check_read_scope("FileRead", &outside),
            PermissionDecision::Deny(_)
        ));
        let grep_out = serde_json::json!({"path": "/etc", "pattern": "root"});
        assert!(matches!(
            checker.check_read_scope("Grep", &grep_out),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn read_scope_denies_missing_path_that_defaults_outside() {
        // A no-path Grep/Glob defaults to the process cwd; with a scope that
        // is not the cwd, that resolves outside the scope and is denied.
        let dir = tempfile::tempdir().unwrap();
        let checker = PermissionChecker::allow_all().with_read_scope(dir.path().to_path_buf());
        let no_path = serde_json::json!({"pattern": "foo"});
        assert!(matches!(
            checker.check_read_scope("Grep", &no_path),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn glob_pattern_escape_detection_is_platform_independent() {
        // Escaping patterns — must be caught on every host OS (on Windows a
        // Unix-style "/etc/**" is not `is_absolute()`, hence the explicit
        // leading-separator / drive-prefix / dotdot checks).
        for esc in [
            "/etc/**",
            "\\\\server\\share\\*",
            "C:\\Windows\\*",
            "c:/Windows/*",
            "../.ssh/*",
            "..\\secrets\\*",
            "src/../../../etc/*",
        ] {
            assert!(
                glob_pattern_escapes(esc),
                "{esc} must be flagged as escaping"
            );
        }
        // In-scope relative patterns — must be allowed.
        for ok in ["**/*.rs", "src/**/*.toml", "*.py", "a/b/c.txt"] {
            assert!(!glob_pattern_escapes(ok), "{ok} must be allowed");
        }
    }

    #[test]
    fn read_scope_denies_escaping_glob_patterns() {
        // Scope = cwd so the effective-root check passes and the pattern is
        // what must be caught.
        let cwd = std::env::current_dir().unwrap();
        let checker = PermissionChecker::allow_all().with_read_scope(cwd);
        // Absolute pattern escapes.
        let glob_abs = serde_json::json!({"pattern": "/etc/**"});
        assert!(matches!(
            checker.check_read_scope("Glob", &glob_abs),
            PermissionDecision::Deny(_)
        ));
        // A `..` traversal escapes even with an in-scope path.
        let glob_dotdot = serde_json::json!({"path": ".", "pattern": "../.ssh/*"});
        assert!(matches!(
            checker.check_read_scope("Glob", &glob_dotdot),
            PermissionDecision::Deny(_)
        ));
        // A plain relative pattern with no path defaults to the in-scope cwd.
        let glob_rel = serde_json::json!({"pattern": "**/*.rs"});
        assert!(matches!(
            checker.check_read_scope("Glob", &glob_rel),
            PermissionDecision::Allow
        ));

        // Grep's `pattern` is a content regex, not a path: a regex that looks
        // like a path must not be blocked when the search path is in-scope.
        let grep_regex = serde_json::json!({"path": ".", "pattern": "../ or /admin"});
        assert!(matches!(
            checker.check_read_scope("Grep", &grep_regex),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn no_read_scope_allows_any_path() {
        let checker = PermissionChecker::allow_all();
        let outside = serde_json::json!({"file_path": "/etc/hostname"});
        assert!(matches!(
            checker.check_read_scope("FileRead", &outside),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn read_scope_checks_every_path_field() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("in.txt"), "x").unwrap();
        let checker = PermissionChecker::allow_all().with_read_scope(dir.path().to_path_buf());
        // An in-scope `file_path` must not excuse an out-of-scope `path` — a
        // Grep/Glob actually reads `path`, so both fields are validated.
        let mixed = serde_json::json!({
            "file_path": dir.path().join("in.txt").to_string_lossy(),
            "path": "/etc",
            "pattern": ".*"
        });
        assert!(matches!(
            checker.check_read_scope("Grep", &mixed),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn read_scope_denies_hidden_and_git_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/app.py"), "x").unwrap();
        let checker = PermissionChecker::allow_all().with_read_scope(dir.path().to_path_buf());

        // In-scope but hidden/secret files are denied.
        for f in [".env", ".git/config"] {
            let input = serde_json::json!({"file_path": dir.path().join(f).to_string_lossy()});
            assert!(
                matches!(
                    checker.check_read_scope("FileRead", &input),
                    PermissionDecision::Deny(_)
                ),
                "{f} must be denied"
            );
        }
        // A normal source file is allowed.
        let ok = serde_json::json!({"file_path": dir.path().join("src/app.py").to_string_lossy()});
        assert!(matches!(
            checker.check_read_scope("FileRead", &ok),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn read_scope_allows_path_filters_hidden_and_out_of_scope() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/app.py"), "x").unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]").unwrap();

        // No scope: every path is allowed (interactive agent unaffected).
        let open = PermissionChecker::allow_all();
        assert!(!open.has_read_scope());
        assert!(open.read_scope_allows_path(&dir.path().join(".env")));

        // Scoped: in-scope source allowed; hidden descendants and out-of-scope
        // paths denied — the filter recursive tools apply to their results.
        let scoped = PermissionChecker::allow_all().with_read_scope(dir.path().to_path_buf());
        assert!(scoped.has_read_scope());
        assert!(scoped.read_scope_allows_path(&dir.path().join("src/app.py")));
        assert!(!scoped.read_scope_allows_path(&dir.path().join(".env")));
        assert!(!scoped.read_scope_allows_path(&dir.path().join(".git/config")));
        assert!(!scoped.read_scope_allows_path(std::path::Path::new("/etc/passwd")));
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("git *", "git status"));
        assert!(glob_match("git *", "git push --force"));
        assert!(!glob_match("git *", "rm -rf /"));
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("??", "ab"));
        assert!(!glob_match("??", "abc"));
    }

    #[test]
    fn test_allow_all() {
        let checker = PermissionChecker::allow_all();
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_protected_dirs_block_writes() {
        let checker = PermissionChecker::allow_all();

        // Writing to .git/ should be denied even with allow_all.
        assert!(matches!(
            checker.check(
                "FileWrite",
                &serde_json::json!({"file_path": ".git/config"})
            ),
            PermissionDecision::Deny(_)
        ));

        // Writing to node_modules/ should be denied.
        assert!(matches!(
            checker.check(
                "FileEdit",
                &serde_json::json!({"file_path": "node_modules/foo/index.js"})
            ),
            PermissionDecision::Deny(_)
        ));

        // Writing to .husky/ should be denied.
        assert!(matches!(
            checker.check(
                "FileWrite",
                &serde_json::json!({"file_path": ".husky/pre-commit"})
            ),
            PermissionDecision::Deny(_)
        ));

        // Reading .git/ should still be allowed.
        assert!(matches!(
            checker.check("FileRead", &serde_json::json!({"file_path": ".git/config"})),
            PermissionDecision::Allow
        ));

        // Writing to normal paths should still work.
        assert!(matches!(
            checker.check(
                "FileWrite",
                &serde_json::json!({"file_path": "src/main.rs"})
            ),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_protected_dirs_helper() {
        assert!(check_protected_path(&serde_json::json!({"file_path": ".git/HEAD"})).is_some());
        assert!(
            check_protected_path(&serde_json::json!({"file_path": "node_modules/pkg/lib.js"}))
                .is_some()
        );
        assert!(check_protected_path(&serde_json::json!({"file_path": "src/lib.rs"})).is_none());
        assert!(check_protected_path(&serde_json::json!({"command": "ls"})).is_none());
    }

    #[test]
    fn test_rule_matching() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Ask,
            rules: vec![
                PermissionRule {
                    tool: "Bash".into(),
                    pattern: Some("git *".into()),
                    action: PermissionMode::Allow,
                },
                PermissionRule {
                    tool: "Bash".into(),
                    pattern: Some("rm *".into()),
                    action: PermissionMode::Deny,
                },
            ],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });

        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "git status"})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "rm -rf /"})),
            PermissionDecision::Deny(_)
        ));
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Ask(_)
        ));
    }

    #[test]
    fn test_deny_mode_blocks_all_tools() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Deny(_)
        ));
        assert!(matches!(
            checker.check(
                "FileWrite",
                &serde_json::json!({"file_path": "src/main.rs"})
            ),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn test_plan_mode_blocks_all_tools() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Plan,
            rules: vec![],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        let decision = checker.check("Bash", &serde_json::json!({"command": "ls"}));
        assert!(matches!(decision, PermissionDecision::Deny(_)));
        if let PermissionDecision::Deny(msg) = decision {
            assert!(msg.contains("Plan mode"));
        }
    }

    #[test]
    fn test_accept_edits_mode_allows_writes() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::AcceptEdits,
            rules: vec![],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        // Write to a non-protected path should be allowed.
        assert!(matches!(
            checker.check("FileWrite", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_wildcard_tool_rule_matches_any_tool() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![PermissionRule {
                tool: "*".into(),
                pattern: None,
                action: PermissionMode::Allow,
            }],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            checker.check("FileRead", &serde_json::json!({"file_path": "foo.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_check_read_allows_reads_with_deny_default() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        // check_read should allow even when default mode is Deny (no explicit deny rule).
        assert!(matches!(
            checker.check_read("FileRead", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_check_read_blocks_with_explicit_deny_rule() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Allow,
            rules: vec![PermissionRule {
                tool: "FileRead".into(),
                pattern: Some("*.secret".into()),
                action: PermissionMode::Deny,
            }],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        assert!(matches!(
            checker.check_read("FileRead", &serde_json::json!({"file_path": "keys.secret"})),
            PermissionDecision::Deny(_)
        ));
        // Non-matching pattern should still allow.
        assert!(matches!(
            checker.check_read("FileRead", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_matches_input_pattern_with_file_path() {
        let input = serde_json::json!({"file_path": "src/main.rs"});
        assert!(matches_input_pattern("src/*", &input));
        assert!(!matches_input_pattern("test/*", &input));
    }

    #[test]
    fn test_matches_input_pattern_with_pattern_field() {
        let input = serde_json::json!({"pattern": "TODO"});
        assert!(matches_input_pattern("TODO", &input));
        assert!(!matches_input_pattern("FIXME", &input));
    }

    #[test]
    fn test_is_write_tool_classification() {
        assert!(is_write_tool("FileWrite"));
        assert!(is_write_tool("FileEdit"));
        assert!(is_write_tool("MultiEdit"));
        assert!(is_write_tool("NotebookEdit"));
        assert!(is_write_tool("ApplyPatch"));
        assert!(!is_write_tool("FileRead"));
        assert!(!is_write_tool("Bash"));
        assert!(!is_write_tool("Grep"));
    }

    #[test]
    fn test_protected_path_windows_backslash() {
        assert!(
            check_protected_path(&serde_json::json!({"file_path": "repo\\.git\\config"})).is_some()
        );
    }

    #[test]
    fn test_protected_path_nested_git_objects() {
        assert!(
            check_protected_path(&serde_json::json!({"file_path": "some/path/.git/objects/foo"}))
                .is_some()
        );
    }

    // ---- team-memory write protection ----

    fn assert_write_denied(checker: &PermissionChecker, tool: &str, file_path: &str) {
        let dec = checker.check(tool, &serde_json::json!({"file_path": file_path}));
        match dec {
            PermissionDecision::Deny(msg) => {
                assert!(
                    msg.contains("team-memory"),
                    "expected team-memory denial for {tool} {file_path}, got: {msg}"
                );
            }
            other => panic!("expected Deny for {tool} {file_path} (team-memory), got {other:?}"),
        }
    }

    #[test]
    fn team_memory_blocks_all_write_tools_with_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agent").join("team-memory")).unwrap();

        let checker = PermissionChecker::allow_all().with_project_root(root.to_path_buf());

        // Absolute path to a team-memory file.
        let target = root
            .join(".agent")
            .join("team-memory")
            .join("foo.md")
            .to_string_lossy()
            .to_string();
        assert_write_denied(&checker, "FileWrite", &target);
        assert_write_denied(&checker, "FileEdit", &target);
        assert_write_denied(&checker, "MultiEdit", &target);
        assert_write_denied(&checker, "NotebookEdit", &target);

        // Relative path — same answer.
        assert_write_denied(&checker, "FileWrite", ".agent/team-memory/relative.md");
    }

    #[test]
    fn team_memory_block_holds_without_project_root() {
        // Even without a project root pinned (test envs, allow_all
        // checker), the component-aware fallback still refuses the
        // obvious team-memory path shape.
        let checker = PermissionChecker::allow_all();
        assert_write_denied(&checker, "FileWrite", ".agent/team-memory/foo.md");
        assert_write_denied(
            &checker,
            "FileEdit",
            "/work/myproj/.agent/team-memory/deploy.md",
        );
    }

    #[test]
    fn team_memory_block_does_not_match_lookalikes() {
        let checker = PermissionChecker::allow_all();
        // `team-memory` outside `.agent/` is NOT team memory.
        let dec = checker.check(
            "FileWrite",
            &serde_json::json!({"file_path": "team-memory/foo.md"}),
        );
        assert!(matches!(dec, PermissionDecision::Allow));
        // `.agent` without a `team-memory` child is normal config.
        let dec = checker.check(
            "FileWrite",
            &serde_json::json!({"file_path": ".agent/memory/foo.md"}),
        );
        assert!(matches!(dec, PermissionDecision::Allow));
    }

    #[test]
    fn team_memory_block_rejects_traversal_with_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agent").join("team-memory")).unwrap();
        let checker = PermissionChecker::allow_all().with_project_root(root.to_path_buf());

        // `<root>/foo/../.agent/team-memory/x.md` lexically normalizes
        // to a team-memory write — must be denied.
        let traversal = root
            .join("foo")
            .join("..")
            .join(".agent")
            .join("team-memory")
            .join("x.md")
            .to_string_lossy()
            .to_string();
        assert_write_denied(&checker, "FileWrite", &traversal);
    }

    #[test]
    fn team_memory_block_does_not_affect_reads() {
        let checker = PermissionChecker::allow_all();
        let dec = checker.check_read(
            "FileRead",
            &serde_json::json!({"file_path": ".agent/team-memory/foo.md"}),
        );
        assert!(matches!(dec, PermissionDecision::Allow));
    }

    #[test]
    fn set_default_mode_is_visible_to_check() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Ask,
            rules: vec![],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        // Bash under Ask → Ask decision
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Ask(_)
        ));
        checker.set_default_mode(PermissionMode::Plan);
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Deny(_)
        ));
        checker.set_default_mode(PermissionMode::Allow);
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            PermissionDecision::Allow
        ));
    }
}
