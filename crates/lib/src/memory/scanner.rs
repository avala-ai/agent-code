//! Memory file scanner and relevance-based selection.
//!
//! Scans the memory directory for .md files, reads frontmatter
//! headers, and selects relevant memories based on description
//! matching. Caps at 200 files, returns newest-first.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::types::MemoryMeta;

/// Maximum memory files to scan.
const MAX_MEMORY_FILES: usize = 200;

/// Maximum memories to surface per turn.
const MAX_RELEVANT_PER_TURN: usize = 5;

/// Maximum frontmatter lines to read per file.
const MAX_FRONTMATTER_LINES: usize = 30;

/// A scanned memory file header (metadata only, not full content).
#[derive(Debug, Clone)]
pub struct MemoryHeader {
    pub filename: String,
    pub path: PathBuf,
    pub modified: SystemTime,
    pub meta: Option<MemoryMeta>,
}

/// Scan the memory directory for all .md files (excluding MEMORY.md).
/// Returns headers sorted by modification time (newest first), capped at 200.
pub fn scan_memory_files(memory_dir: &Path) -> Vec<MemoryHeader> {
    if !memory_dir.is_dir() {
        return Vec::new();
    }

    let mut headers: Vec<MemoryHeader> = std::fs::read_dir(memory_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.is_file()
                && path.extension().is_some_and(|e| e == "md")
                && path.file_name().is_some_and(|n| n != "MEMORY.md")
        })
        .filter_map(|entry| {
            let path = entry.path();
            let modified = entry.metadata().ok()?.modified().ok()?;
            let meta = read_frontmatter_only(&path);
            let filename = path.file_name()?.to_str()?.to_string();
            Some(MemoryHeader {
                filename,
                path,
                modified,
                meta,
            })
        })
        .collect();

    // Sort newest first.
    headers.sort_by(|a, b| b.modified.cmp(&a.modified));

    // Cap at max.
    headers.truncate(MAX_MEMORY_FILES);

    headers
}

/// Read only the YAML frontmatter from a memory file (first 30 lines).
fn read_frontmatter_only(path: &Path) -> Option<MemoryMeta> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return None;
    }

    let after_first = &trimmed[3..];
    let closing = after_first
        .lines()
        .take(MAX_FRONTMATTER_LINES)
        .position(|line| line.trim() == "---")?;

    let yaml_lines: Vec<&str> = after_first.lines().take(closing).collect();
    let yaml = yaml_lines.join("\n");

    parse_simple_yaml(&yaml)
}

/// Simple YAML parser for memory frontmatter (key: value pairs).
fn parse_simple_yaml(yaml: &str) -> Option<MemoryMeta> {
    let mut name = String::new();
    let mut description = String::new();
    let mut memory_type = None;

    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            match key {
                "name" => name = value.to_string(),
                "description" => description = value.to_string(),
                "type" => {
                    memory_type = match value {
                        "user" => Some(super::types::MemoryType::User),
                        "feedback" => Some(super::types::MemoryType::Feedback),
                        "project" => Some(super::types::MemoryType::Project),
                        "reference" => Some(super::types::MemoryType::Reference),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
    }

    if name.is_empty() && description.is_empty() {
        return None;
    }

    Some(MemoryMeta {
        name,
        description,
        memory_type,
    })
}

/// Select the most relevant memories for a given conversation context.
///
/// Uses keyword matching on memory descriptions against the user's
/// recent messages. Returns up to MAX_RELEVANT_PER_TURN file paths.
pub fn select_relevant(
    headers: &[MemoryHeader],
    recent_text: &str,
    already_surfaced: &std::collections::HashSet<PathBuf>,
) -> Vec<PathBuf> {
    if headers.is_empty() || recent_text.is_empty() {
        return Vec::new();
    }

    let words: Vec<&str> = recent_text
        .split_whitespace()
        .filter(|w| w.len() > 3) // Skip short words.
        .collect();

    let mut scored: Vec<(&MemoryHeader, usize)> = headers
        .iter()
        .filter(|h| !already_surfaced.contains(&h.path))
        .map(|h| {
            let desc = h
                .meta
                .as_ref()
                .map(|m| format!("{} {}", m.name, m.description))
                .unwrap_or_else(|| h.filename.clone())
                .to_lowercase();

            let score: usize = words
                .iter()
                .filter(|w| desc.contains(&w.to_lowercase()))
                .count();

            (h, score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.truncate(MAX_RELEVANT_PER_TURN);

    scored.iter().map(|(h, _)| h.path.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_memory_file(dir: &std::path::Path, name: &str, frontmatter: &str, body: &str) {
        let content = format!("---\n{frontmatter}\n---\n\n{body}");
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn test_scan_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let headers = scan_memory_files(dir.path());
        assert!(headers.is_empty());
    }

    #[test]
    fn test_scan_finds_md_files() {
        let dir = tempfile::tempdir().unwrap();
        write_memory_file(
            dir.path(),
            "prefs.md",
            "name: Preferences\ndescription: User prefs\ntype: user",
            "I prefer Rust",
        );
        write_memory_file(
            dir.path(),
            "project.md",
            "name: Project\ndescription: Project info\ntype: project",
            "Working on agent-code",
        );
        // Non-md file should be ignored.
        std::fs::write(dir.path().join("notes.txt"), "not a memory").unwrap();
        // MEMORY.md should be ignored.
        std::fs::write(dir.path().join("MEMORY.md"), "index").unwrap();

        let headers = scan_memory_files(dir.path());
        assert_eq!(headers.len(), 2);
    }

    #[test]
    fn test_scan_parses_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        write_memory_file(
            dir.path(),
            "test.md",
            "name: My Memory\ndescription: test description\ntype: feedback",
            "content here",
        );

        let headers = scan_memory_files(dir.path());
        assert_eq!(headers.len(), 1);
        let meta = headers[0].meta.as_ref().unwrap();
        assert_eq!(meta.name, "My Memory");
        assert_eq!(meta.description, "test description");
        assert!(matches!(
            meta.memory_type,
            Some(super::super::types::MemoryType::Feedback)
        ));
    }

    #[test]
    fn test_select_relevant_by_keyword() {
        let dir = tempfile::tempdir().unwrap();
        write_memory_file(
            dir.path(),
            "rust.md",
            "name: Rust Prefs\ndescription: rust programming preferences\ntype: user",
            "I like Rust",
        );
        write_memory_file(
            dir.path(),
            "python.md",
            "name: Python Prefs\ndescription: python programming preferences\ntype: user",
            "I like Python",
        );

        let headers = scan_memory_files(dir.path());
        let surfaced = std::collections::HashSet::new();

        // Search for "rust" should find rust.md.
        let results = select_relevant(&headers, "tell me about rust programming", &surfaced);
        assert!(!results.is_empty());
        assert!(results.iter().any(|p| p.to_str().unwrap().contains("rust")));
    }

    #[test]
    fn test_select_relevant_skips_surfaced() {
        let dir = tempfile::tempdir().unwrap();
        write_memory_file(
            dir.path(),
            "rust.md",
            "name: Rust\ndescription: rust preferences\ntype: user",
            "content",
        );

        let headers = scan_memory_files(dir.path());
        let mut surfaced = std::collections::HashSet::new();
        surfaced.insert(dir.path().join("rust.md"));

        // Already surfaced — should not be returned.
        let results = select_relevant(&headers, "rust programming", &surfaced);
        assert!(results.is_empty());
    }

    #[test]
    fn test_select_relevant_max_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            write_memory_file(
                dir.path(),
                &format!("topic{i}.md"),
                &format!("name: Topic {i}\ndescription: matching keyword stuff\ntype: user"),
                "content about keyword",
            );
        }

        let headers = scan_memory_files(dir.path());
        let surfaced = std::collections::HashSet::new();
        let results = select_relevant(&headers, "keyword matching stuff topic", &surfaced);
        assert!(results.len() <= MAX_RELEVANT_PER_TURN);
    }
}
