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
pub mod scanner;
pub mod session_notes;
pub mod types;
pub mod writer;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tracing::debug;

const MAX_INDEX_LINES: usize = 200;
const MAX_MEMORY_FILE_BYTES: usize = 25_000;

#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    pub project_context: Option<String>,
    pub user_memory: Option<String>,
    pub memory_files: Vec<MemoryFile>,
    pub surfaced: HashSet<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub name: String,
    pub content: String,
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

/// Load project context by traversing the directory hierarchy.
///
/// Checks (in priority order, lowest to highest):
/// 1. User global: ~/.config/agent-code/CONTEXT.md
/// 2. Project root: CONTEXT.md, .agent/CONTEXT.md
/// 3. Project rules: .agent/rules/*.md (all files concatenated)
/// 4. Project local: CONTEXT.local.md (gitignored overrides)
///
/// Files closer to cwd have higher priority (loaded later, overrides earlier).
fn load_project_context(project_root: &Path) -> Option<String> {
    let mut sections = Vec::new();

    // Layer 1: User global context.
    if let Some(global_path) = dirs::config_dir().map(|d| d.join("agent-code").join("CONTEXT.md"))
        && let Some(content) = load_truncated_file(&global_path)
    {
        debug!("Loaded global context from {}", global_path.display());
        sections.push(content);
    }

    // Layer 2: Project root context.
    for path in &[
        project_root.join("CONTEXT.md"),
        project_root.join(".agent").join("CONTEXT.md"),
    ] {
        if let Some(content) = load_truncated_file(path) {
            debug!("Loaded project context from {}", path.display());
            sections.push(content);
        }
    }

    // Layer 3: Rules directory (all .md files).
    let rules_dir = project_root.join(".agent").join("rules");
    if rules_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&rules_dir)
    {
        let mut rule_files: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "md") && e.path().is_file())
            .collect();
        rule_files.sort_by_key(|e| e.file_name());

        for entry in rule_files {
            if let Some(content) = load_truncated_file(&entry.path()) {
                debug!("Loaded rule from {}", entry.path().display());
                sections.push(content);
            }
        }
    }

    // Layer 4: Local overrides (gitignored).
    let local_path = project_root.join("CONTEXT.local.md");
    if let Some(content) = load_truncated_file(&local_path) {
        debug!("Loaded local context from {}", local_path.display());
        sections.push(content);
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

pub fn project_memory_dir(project_root: &Path) -> PathBuf {
    project_root.join(".agent")
}

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
}
