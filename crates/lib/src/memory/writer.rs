//! Memory write discipline.
//!
//! Enforces the two-step write pattern:
//! 1. Write the memory file with proper frontmatter
//! 2. Update MEMORY.md index with a one-line pointer
//!
//! Prevents entropy by never dumping content into the index.

use std::path::{Path, PathBuf};

use super::types::{MemoryMeta, MemoryType};

/// Maximum index line length.
const MAX_INDEX_LINE_CHARS: usize = 150;

/// Maximum index lines before truncation.
const MAX_INDEX_LINES: usize = 200;

/// Write a memory file and update the index atomically.
///
/// Returns the path of the written memory file.
pub fn write_memory(
    memory_dir: &Path,
    filename: &str,
    meta: &MemoryMeta,
    content: &str,
) -> Result<PathBuf, String> {
    let _ = std::fs::create_dir_all(memory_dir);

    // Step 1: Write the memory file with frontmatter.
    let type_str = match &meta.memory_type {
        Some(MemoryType::User) => "user",
        Some(MemoryType::Feedback) => "feedback",
        Some(MemoryType::Project) => "project",
        Some(MemoryType::Reference) => "reference",
        None => "user",
    };

    let file_content = format!(
        "---\nname: {}\ndescription: {}\ntype: {}\n---\n\n{}",
        meta.name, meta.description, type_str, content
    );

    let file_path = memory_dir.join(filename);
    std::fs::write(&file_path, &file_content)
        .map_err(|e| format!("Failed to write memory file: {e}"))?;

    // Step 2: Update MEMORY.md index.
    update_index(memory_dir, filename, &meta.name, &meta.description)?;

    Ok(file_path)
}

/// Update the MEMORY.md index with a pointer to a memory file.
/// If an entry for this filename already exists, replace it.
fn update_index(
    memory_dir: &Path,
    filename: &str,
    name: &str,
    description: &str,
) -> Result<(), String> {
    let index_path = memory_dir.join("MEMORY.md");

    let existing = std::fs::read_to_string(&index_path).unwrap_or_default();

    // Build the new index line (under 150 chars).
    let mut line = format!("- [{}]({}) — {}", name, filename, description);
    if line.len() > MAX_INDEX_LINE_CHARS {
        line.truncate(MAX_INDEX_LINE_CHARS - 3);
        line.push_str("...");
    }

    // Replace existing entry for this filename, or append.
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|l| !l.contains(&format!("({})", filename)))
        .map(|l| l.to_string())
        .collect();

    lines.push(line);

    // Enforce max lines.
    if lines.len() > MAX_INDEX_LINES {
        lines.truncate(MAX_INDEX_LINES);
    }

    let new_index = lines.join("\n") + "\n";
    std::fs::write(&index_path, new_index).map_err(|e| format!("Failed to update index: {e}"))?;

    Ok(())
}

/// Remove a memory file and its index entry.
pub fn delete_memory(memory_dir: &Path, filename: &str) -> Result<(), String> {
    let file_path = memory_dir.join(filename);
    if file_path.exists() {
        std::fs::remove_file(&file_path).map_err(|e| format!("Failed to delete: {e}"))?;
    }

    // Remove from index.
    let index_path = memory_dir.join("MEMORY.md");
    if let Ok(existing) = std::fs::read_to_string(&index_path) {
        let filtered: Vec<&str> = existing
            .lines()
            .filter(|l| !l.contains(&format!("({})", filename)))
            .collect();
        let _ = std::fs::write(&index_path, filtered.join("\n") + "\n");
    }

    Ok(())
}

/// Rebuild MEMORY.md from the actual files in the memory directory.
/// Scans all .md files (except MEMORY.md itself), reads their frontmatter,
/// and regenerates the index.
pub fn rebuild_index(memory_dir: &Path) -> Result<(), String> {
    let headers = super::scanner::scan_memory_files(memory_dir);
    let index_path = memory_dir.join("MEMORY.md");

    let mut lines = Vec::new();
    for h in &headers {
        let name = h
            .meta
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or(&h.filename);
        let desc = h
            .meta
            .as_ref()
            .map(|m| m.description.as_str())
            .unwrap_or("");

        let mut line = format!("- [{}]({}) — {}", name, h.filename, desc);
        if line.len() > MAX_INDEX_LINE_CHARS {
            line.truncate(MAX_INDEX_LINE_CHARS - 3);
            line.push_str("...");
        }
        lines.push(line);
    }

    if lines.len() > MAX_INDEX_LINES {
        lines.truncate(MAX_INDEX_LINES);
    }

    let content = lines.join("\n") + "\n";
    std::fs::write(&index_path, content).map_err(|e| format!("Failed to write index: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta() -> MemoryMeta {
        MemoryMeta {
            name: "Test Memory".to_string(),
            description: "A test memory file".to_string(),
            memory_type: Some(MemoryType::User),
        }
    }

    #[test]
    fn test_write_memory_creates_file_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        let path = write_memory(dir.path(), "test.md", &meta, "Hello world").unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("name: Test Memory"));
        assert!(content.contains("type: user"));
        assert!(content.contains("Hello world"));

        // Index should exist and contain a pointer.
        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        assert!(index.contains("[Test Memory](test.md)"));
    }

    #[test]
    fn test_write_memory_updates_existing_index_entry() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        write_memory(dir.path(), "test.md", &meta, "version 1").unwrap();

        let meta2 = MemoryMeta {
            name: "Updated".to_string(),
            description: "Updated description".to_string(),
            memory_type: Some(MemoryType::Feedback),
        };
        write_memory(dir.path(), "test.md", &meta2, "version 2").unwrap();

        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        // Should have only one entry for test.md (replaced, not duplicated).
        assert_eq!(index.matches("test.md").count(), 1);
        assert!(index.contains("[Updated](test.md)"));
    }

    #[test]
    fn test_delete_memory() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        write_memory(dir.path(), "test.md", &meta, "content").unwrap();

        delete_memory(dir.path(), "test.md").unwrap();

        assert!(!dir.path().join("test.md").exists());
        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        assert!(!index.contains("test.md"));
    }

    #[test]
    fn test_delete_nonexistent_memory() {
        let dir = tempfile::tempdir().unwrap();
        // Should not error even if file doesn't exist.
        assert!(delete_memory(dir.path(), "nope.md").is_ok());
    }

    #[test]
    fn test_rebuild_index() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        write_memory(dir.path(), "one.md", &meta, "first").unwrap();

        let meta2 = MemoryMeta {
            name: "Second".to_string(),
            description: "Second file".to_string(),
            memory_type: Some(MemoryType::Project),
        };
        write_memory(dir.path(), "two.md", &meta2, "second").unwrap();

        // Corrupt the index.
        std::fs::write(dir.path().join("MEMORY.md"), "garbage").unwrap();

        // Rebuild should restore it.
        rebuild_index(dir.path()).unwrap();
        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        assert!(index.contains("one.md"));
        assert!(index.contains("two.md"));
    }

    #[test]
    fn test_index_line_length_cap() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MemoryMeta {
            name: "A".repeat(200),
            description: "B".repeat(200),
            memory_type: Some(MemoryType::User),
        };
        write_memory(dir.path(), "long.md", &meta, "content").unwrap();

        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        for line in index.lines() {
            assert!(line.len() <= MAX_INDEX_LINE_CHARS + 3); // +3 for "..."
        }
    }
}
