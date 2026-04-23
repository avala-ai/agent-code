//! Memory system — 3-layer architecture.
//!
//! **Layer 1 — Index (always loaded):**
//! MEMORY.md contains one-line pointers to topic files. Capped at
//! 200 lines / 25KB. Always in the system prompt.
//!
//! **Layer 2 — Topic files (on-demand):**
//! Individual .md files with YAML frontmatter. Loaded selectively
//! based on relevance to the current conversation.
//!
//! **Layer 3 — Transcripts (never loaded, only grepped):**
//! Past session logs. Not loaded into context.
//!
//! # Write discipline
//!
//! 1. Write the memory file with frontmatter
//! 2. Update MEMORY.md index with a one-line pointer
//!
//! Never dump content into the index.

pub mod consolidation;
pub mod extraction;
pub mod scanner;
pub mod session_notes;
pub mod types;
pub mod writer;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tracing::debug;

const MAX_INDEX_LINES: usize = 200;
const MAX_MEMORY_FILE_BYTES: usize = 25_000;

/// Persistent context loaded at session start.
///
/// Contains project-level context (`AGENTS.md`), user-level memory
/// (`~/.config/agent-code/memory/`), and individual memory files.
/// Injected into the system prompt so the agent has context across sessions.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Project-level instructions from `AGENTS.md` in the repo root.
    pub project_context: Option<String>,
    /// User-level memory index from `MEMORY.md`.
    pub user_memory: Option<String>,
    /// Individual memory files linked from the index.
    pub memory_files: Vec<MemoryFile>,
    /// Paths already surfaced in this session (to avoid duplicates).
    pub surfaced: HashSet<PathBuf>,
}

/// A single memory file with metadata.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Absolute path to the memory file.
    pub path: PathBuf,
    /// Memory name from frontmatter.
    pub name: String,
    /// File content (truncated at 25KB).
    pub content: String,
    /// Optional staleness indicator.
    pub staleness: Option<String>,
}

impl MemoryContext {
    pub fn load(project_root: Option<&Path>) -> Self {
        let mut ctx = Self::default();
        if let Some(root) = project_root {
            ctx.project_context = load_project_context(root);
        }
        if let Some(memory_dir) = user_memory_dir() {
            let index_path = memory_dir.join("MEMORY.md");
            if index_path.exists() {
                ctx.user_memory = load_truncated_file(&index_path);
            }
            if let Some(ref index) = ctx.user_memory {
                ctx.memory_files = load_referenced_files(index, &memory_dir);
            }
        }
        ctx
    }

    pub fn load_relevant(&mut self, recent_text: &str) {
        let Some(memory_dir) = user_memory_dir() else {
            return;
        };
        let headers = scanner::scan_memory_files(&memory_dir);
        let relevant = scanner::select_relevant(&headers, recent_text, &self.surfaced);
        for path in relevant {
            if let Some(file) = load_memory_file_with_staleness(&path) {
                self.surfaced.insert(path);
                self.memory_files.push(file);
            }
        }
    }

    pub fn to_system_prompt_section(&self) -> String {
        let mut section = String::new();
        if let Some(ref project) = self.project_context
            && !project.is_empty()
        {
            section.push_str("# Project Context\n\n");
            section.push_str(project);
            section.push_str("\n\n");
        }
        if let Some(ref memory) = self.user_memory
            && !memory.is_empty()
        {
            section.push_str("# Memory Index\n\n");
            section.push_str(memory);
            section.push_str("\n\n");
            section.push_str(
                "_Memory is a hint, not truth. Verify against current state \
                     before acting on remembered facts._\n\n",
            );
        }
        for file in &self.memory_files {
            section.push_str(&format!("## Memory: {}\n\n", file.name));
            if let Some(ref warning) = file.staleness {
                section.push_str(&format!("_{warning}_\n\n"));
            }
            section.push_str(&file.content);
            section.push_str("\n\n");
        }
        section
    }

    pub fn is_empty(&self) -> bool {
        self.project_context.is_none() && self.user_memory.is_none() && self.memory_files.is_empty()
    }
}

/// Walk from the git repo root down to `start` (inclusive) and return
/// every `AGENTS.md` / `.agent/AGENTS.md` / `CLAUDE.md` /
/// `.claude/CLAUDE.md` that exists, ordered outermost→innermost.
///
/// "Git repo root" is the nearest ancestor containing `.git`. If no
/// `.git` is found, the walk stops at `start` itself — we never escape
/// the session dir to load config from random parent dirs.
fn hierarchical_project_files(start: &Path) -> Vec<PathBuf> {
    // Walk ancestors once to locate the repo root (nearest dir with
    // `.git`). `.git` can be a directory (normal checkout) or a file
    // (submodules / worktrees). We accept either.
    let mut repo_root: Option<&Path> = None;
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            repo_root = Some(dir);
            break;
        }
    }

    // Build the walk range: every directory from repo_root down to
    // start inclusive. Using strip_prefix + component iteration keeps
    // this deterministic regardless of OS path-separator.
    let top: &Path = repo_root.unwrap_or(start);
    let mut dirs: Vec<PathBuf> = vec![top.to_path_buf()];
    if let Ok(rel) = start.strip_prefix(top) {
        let mut cursor = top.to_path_buf();
        for seg in rel.components() {
            cursor.push(seg);
            if cursor != *top {
                dirs.push(cursor.clone());
            }
        }
    }

    let mut files = Vec::new();
    for dir in &dirs {
        // Primary names at each level. AGENTS.md first so it wins
        // the `is_file()` race for callers that stop at first hit.
        for leaf in &[
            "AGENTS.md",
            ".agent/AGENTS.md",
            "CLAUDE.md",
            ".claude/CLAUDE.md",
        ] {
            let p = dir.join(leaf);
            if p.is_file() {
                files.push(p);
            }
        }
    }
    files
}

/// Load project context by traversing the directory hierarchy.
///
/// Checks (in priority order, lowest to highest):
/// 1. User global: ~/.config/agent-code/AGENTS.md
/// 2. Hierarchical project context: every `AGENTS.md` (or `CLAUDE.md`
///    compat) from the git repo root down to the session cwd, in
///    outermost-to-innermost order. Deeper files are loaded last so
///    their contents override earlier layers in the composed prompt.
///    `.agent/AGENTS.md` at each level is also honored.
/// 3. Project rules: .agent/rules/*.md AND .claude/rules/*.md
/// 4. Project local: AGENTS.local.md / CLAUDE.local.md (gitignored)
///
/// CLAUDE.md is supported for compatibility with existing projects.
/// If both AGENTS.md and CLAUDE.md exist, both are loaded (AGENTS.md first).
fn load_project_context(project_root: &Path) -> Option<String> {
    let mut sections = Vec::new();

    // Layer 1: User global context.
    for name in &["AGENTS.md", "CLAUDE.md"] {
        if let Some(global_path) = dirs::config_dir().map(|d| d.join("agent-code").join(name))
            && let Some(content) = load_truncated_file(&global_path)
        {
            debug!("Loaded global context from {}", global_path.display());
            sections.push(content);
        }
    }

    // Layer 2: Hierarchical project context.
    //
    // Walk from the git repo root down to `project_root` (typically the
    // session cwd). Load every `AGENTS.md` / `.agent/AGENTS.md` /
    // `CLAUDE.md` / `.claude/CLAUDE.md` seen along the way so an
    // `AGENTS.md` in a monorepo sub-package actually takes effect when
    // the agent is invoked from that subdir. Outermost-first ordering
    // lets deeper (more specific) files override broader ones.
    for path in hierarchical_project_files(project_root) {
        if let Some(content) = load_truncated_file(&path) {
            debug!("Loaded project context from {}", path.display());
            sections.push(content);
        }
    }

    // Layer 3: Rules directories (both .agent/ and .claude/ for compat).
    for rules_dir in &[
        project_root.join(".agent").join("rules"),
        project_root.join(".claude").join("rules"),
    ] {
        if rules_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(rules_dir)
        {
            let mut rule_files: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    e.path().extension().is_some_and(|ext| ext == "md") && e.path().is_file()
                })
                .collect();
            rule_files.sort_by_key(|e| e.file_name());

            for entry in rule_files {
                if let Some(content) = load_truncated_file(&entry.path()) {
                    debug!("Loaded rule from {}", entry.path().display());
                    sections.push(content);
                }
            }
        }
    }

    // Layer 4: Local overrides (gitignored).
    for name in &["AGENTS.local.md", "CLAUDE.local.md"] {
        let local_path = project_root.join(name);
        if let Some(content) = load_truncated_file(&local_path) {
            debug!("Loaded local context from {}", local_path.display());
            sections.push(content);
        }
    }

    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

fn load_truncated_file(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    if content.is_empty() {
        return None;
    }

    let mut result = content.clone();
    let mut was_byte_truncated = false;

    if result.len() > MAX_MEMORY_FILE_BYTES {
        if let Some(pos) = result[..MAX_MEMORY_FILE_BYTES].rfind('\n') {
            result.truncate(pos);
        } else {
            result.truncate(MAX_MEMORY_FILE_BYTES);
        }
        was_byte_truncated = true;
    }

    let lines: Vec<&str> = result.lines().collect();
    let was_line_truncated = lines.len() > MAX_INDEX_LINES;
    if was_line_truncated {
        result = lines[..MAX_INDEX_LINES].join("\n");
    }

    if was_byte_truncated || was_line_truncated {
        result.push_str("\n\n(truncated)");
    }

    Some(result)
}

fn load_memory_file_with_staleness(path: &Path) -> Option<MemoryFile> {
    let content = load_truncated_file(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let staleness = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|modified| {
            let age = std::time::SystemTime::now().duration_since(modified).ok()?;
            types::staleness_caveat(age.as_secs())
        });

    Some(MemoryFile {
        path: path.to_path_buf(),
        name,
        content,
        staleness,
    })
}

fn load_referenced_files(index: &str, base_dir: &Path) -> Vec<MemoryFile> {
    let mut files = Vec::new();
    let link_re = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();

    for captures in link_re.captures_iter(index) {
        let name = captures.get(1).map(|m| m.as_str()).unwrap_or("");
        let filename = captures.get(2).map(|m| m.as_str()).unwrap_or("");
        if filename.is_empty() || !filename.ends_with(".md") {
            continue;
        }
        let path = base_dir.join(filename);
        if let Some(mut file) = load_memory_file_with_staleness(&path) {
            file.name = name.to_string();
            files.push(file);
        }
    }
    files
}

fn user_memory_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("memory"))
}

/// Returns the project-level memory directory (`.agent/` in the project root).
pub fn project_memory_dir(project_root: &Path) -> PathBuf {
    project_root.join(".agent")
}

/// Returns the user-level memory directory, creating it if needed.
pub fn ensure_memory_dir() -> Option<PathBuf> {
    let dir = user_memory_dir()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_truncated_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.md");
        std::fs::write(&path, "a\n".repeat(300)).unwrap();
        let loaded = load_truncated_file(&path).unwrap();
        assert!(loaded.contains("truncated"));
    }

    #[test]
    fn test_load_referenced_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("prefs.md"), "I prefer Rust").unwrap();
        let index = "- [Preferences](prefs.md) — prefs\n- [Missing](gone.md) — gone";
        let files = load_referenced_files(index, dir.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "Preferences");
    }

    // ---- hierarchical_project_files ----

    /// Build a fake repo layout:
    ///   tmp/
    ///     .git/  (dir, repo root marker)
    ///     AGENTS.md
    ///     packages/
    ///       sub/
    ///         AGENTS.md
    ///         nested/
    ///           AGENTS.md
    fn make_nested_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "root").unwrap();
        let sub = root.join("packages").join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("AGENTS.md"), "sub").unwrap();
        let nested = sub.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("AGENTS.md"), "nested").unwrap();
        dir
    }

    #[test]
    fn hierarchical_walks_from_cwd_up_to_git_root() {
        let tmp = make_nested_repo();
        let start = tmp.path().join("packages").join("sub").join("nested");
        let files = hierarchical_project_files(&start);
        // Should find 3 AGENTS.md: root, sub, nested
        let names: Vec<_> = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();
        assert_eq!(names, vec!["root", "sub", "nested"]);
    }

    #[test]
    fn hierarchical_stops_at_git_root_does_not_escape() {
        let tmp = make_nested_repo();
        // Write an AGENTS.md in the *parent* of the temp dir. The walk
        // must not reach it — we must stop at `.git`.
        // (Skipping this in practice because tmpdir's parent may not be
        // writable; instead, verify the walk only returns files within
        // the repo root.)
        let start = tmp.path().join("packages").join("sub");
        let files = hierarchical_project_files(&start);
        for p in &files {
            assert!(p.starts_with(tmp.path()), "walk escaped repo root: {p:?}");
        }
    }

    #[test]
    fn hierarchical_ordering_is_outermost_first() {
        let tmp = make_nested_repo();
        let start = tmp.path().join("packages").join("sub").join("nested");
        let files = hierarchical_project_files(&start);
        // Content of the first file must be "root" (outermost).
        let first = std::fs::read_to_string(&files[0]).unwrap();
        assert_eq!(first, "root");
        let last = std::fs::read_to_string(files.last().unwrap()).unwrap();
        assert_eq!(last, "nested");
    }

    #[test]
    fn hierarchical_without_git_stays_at_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "x").unwrap();
        let files = hierarchical_project_files(dir.path());
        // No .git anywhere — walk should stop at start itself, not
        // climb into parent filesystem dirs.
        assert!(!files.is_empty());
        for p in &files {
            assert!(p.starts_with(dir.path()));
        }
    }

    #[test]
    fn hierarchical_handles_missing_intermediate_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        // Only root and deepest have AGENTS.md; intermediate doesn't.
        std::fs::write(root.join("AGENTS.md"), "root").unwrap();
        let mid = root.join("a").join("b");
        std::fs::create_dir_all(&mid).unwrap();
        std::fs::write(mid.join("AGENTS.md"), "deep").unwrap();
        let files = hierarchical_project_files(&mid);
        let names: Vec<_> = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();
        assert_eq!(names, vec!["root", "deep"]);
    }

    #[test]
    fn hierarchical_includes_dot_agent_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        let dotagent = root.join(".agent");
        std::fs::create_dir(&dotagent).unwrap();
        std::fs::write(dotagent.join("AGENTS.md"), "from-.agent").unwrap();
        let files = hierarchical_project_files(root);
        let contents: Vec<_> = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();
        assert!(
            contents.iter().any(|c| c == "from-.agent"),
            "expected .agent/AGENTS.md to be picked up, got {contents:?}"
        );
    }
}
