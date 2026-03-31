//! Memory system.
//!
//! Persistent memory across sessions via markdown files with YAML
//! frontmatter. Two layers:
//!
//! - **Project memory**: `.rc/CONTEXT.md` in the project root — loaded
//!   automatically, provides project-specific instructions and context.
//! - **User memory**: `~/.config/rust-code/MEMORY.md` — user-level
//!   preferences, patterns, and learned context.
//!
//! Memory files are injected into the system prompt at session start.
//! The index file (`MEMORY.md`) contains short pointers to individual
//! memory files, which are loaded on demand.

use std::path::{Path, PathBuf};
use tracing::debug;

/// Maximum lines to load from a memory index file.
const MAX_INDEX_LINES: usize = 200;

/// Maximum bytes for a single memory file.
const MAX_MEMORY_FILE_BYTES: usize = 25_000;

/// Loaded memory context ready for injection into the system prompt.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Project-level context (from .rc/CONTEXT.md).
    pub project_context: Option<String>,
    /// User-level memory index (from ~/.config/rust-code/MEMORY.md).
    pub user_memory: Option<String>,
    /// Individual memory files loaded from the index.
    pub memory_files: Vec<MemoryFile>,
}

/// A single loaded memory file.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub name: String,
    pub content: String,
}

impl MemoryContext {
    /// Load all memory context for the current session.
    pub fn load(project_root: Option<&Path>) -> Self {
        let mut ctx = Self::default();

        // Load project-level context.
        if let Some(root) = project_root {
            ctx.project_context = load_project_context(root);
        }

        // Load user-level memory.
        if let Some(memory_dir) = user_memory_dir() {
            let index_path = memory_dir.join("MEMORY.md");
            if index_path.exists() {
                ctx.user_memory = load_memory_file(&index_path);
            }

            // Load individual memory files referenced in the index.
            if let Some(ref index) = ctx.user_memory {
                ctx.memory_files = load_referenced_files(index, &memory_dir);
            }
        }

        ctx
    }

    /// Format memory context for injection into the system prompt.
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
            section.push_str("# User Memory\n\n");
            section.push_str(memory);
            section.push_str("\n\n");
        }

        for file in &self.memory_files {
            section.push_str(&format!("## Memory: {}\n\n", file.name));
            section.push_str(&file.content);
            section.push_str("\n\n");
        }

        section
    }

    /// Check if any memory was loaded.
    pub fn is_empty(&self) -> bool {
        self.project_context.is_none() && self.user_memory.is_none() && self.memory_files.is_empty()
    }
}

/// Load project context from `.rc/CONTEXT.md` or `CONTEXT.md`.
fn load_project_context(project_root: &Path) -> Option<String> {
    // Try .rc/CONTEXT.md first, then CONTEXT.md in root.
    let candidates = [
        project_root.join(".rc").join("CONTEXT.md"),
        project_root.join("CONTEXT.md"),
    ];

    for path in &candidates {
        if let Some(content) = load_memory_file(path) {
            debug!("Loaded project context from {}", path.display());
            return Some(content);
        }
    }

    None
}

/// Load a memory file, respecting size limits.
fn load_memory_file(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;

    if content.is_empty() {
        return None;
    }

    // Truncate to max bytes.
    let truncated = if content.len() > MAX_MEMORY_FILE_BYTES {
        let mut s = content[..MAX_MEMORY_FILE_BYTES].to_string();
        s.push_str("\n\n(truncated)");
        s
    } else {
        content
    };

    // Truncate to max lines for index files.
    let lines: Vec<&str> = truncated.lines().collect();
    if lines.len() > MAX_INDEX_LINES {
        Some(lines[..MAX_INDEX_LINES].join("\n"))
    } else {
        Some(truncated)
    }
}

/// Parse markdown links from an index file and load the referenced files.
///
/// Looks for patterns like `- [Title](filename.md) — description`
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
        if let Some(content) = load_memory_file(&path) {
            files.push(MemoryFile {
                path,
                name: name.to_string(),
                content,
            });
        }
    }

    files
}

/// Get the user memory directory.
fn user_memory_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("rust-code").join("memory"))
}

/// Get the project memory directory for the given project root.
pub fn project_memory_dir(project_root: &Path) -> PathBuf {
    project_root.join(".rc")
}

/// Save a memory file to the user memory directory.
pub fn save_user_memory(filename: &str, content: &str) -> Result<PathBuf, String> {
    let dir = user_memory_dir().ok_or("Could not determine memory directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create memory directory: {e}"))?;

    let path = dir.join(filename);
    std::fs::write(&path, content).map_err(|e| format!("Failed to write memory file: {e}"))?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_load_memory_file_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.md");
        let content = "a\n".repeat(300);
        fs::write(&path, &content).unwrap();

        let loaded = load_memory_file(&path).unwrap();
        assert!(loaded.lines().count() <= MAX_INDEX_LINES);
    }

    #[test]
    fn test_load_referenced_files() {
        let dir = tempfile::tempdir().unwrap();

        // Create a referenced file.
        fs::write(dir.path().join("prefs.md"), "I prefer Rust").unwrap();

        let index = "- [Preferences](prefs.md) — user preferences\n\
                     - [Missing](gone.md) — this doesn't exist";

        let files = load_referenced_files(index, dir.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "Preferences");
        assert_eq!(files[0].content, "I prefer Rust");
    }
}
