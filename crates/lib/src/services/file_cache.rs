//! File state cache.
//!
//! Caches file contents to avoid redundant reads during a session.
//! Files are evicted when their modification time changes or the
//! cache reaches its size limit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Maximum cache size in bytes (50MB).
const MAX_CACHE_BYTES: usize = 50 * 1024 * 1024;

/// Cached state of a single file.
#[derive(Debug, Clone)]
struct CachedFile {
    content: String,
    modified: SystemTime,
    size: usize,
}

/// In-memory cache of recently read files.
pub struct FileCache {
    entries: HashMap<PathBuf, CachedFile>,
    total_bytes: usize,
}

impl FileCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
        }
    }

    /// Read a file, returning cached content if still fresh.
    pub fn read(&mut self, path: &Path) -> Result<String, String> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Check if cached entry is still valid.
        if let Some(cached) = self.entries.get(&canonical)
            && let Ok(meta) = std::fs::metadata(&canonical)
            && let Ok(modified) = meta.modified()
            && modified == cached.modified
        {
            return Ok(cached.content.clone());
        }

        // Read fresh.
        let content =
            std::fs::read_to_string(&canonical).map_err(|e| format!("read error: {e}"))?;

        let meta = std::fs::metadata(&canonical).map_err(|e| format!("metadata error: {e}"))?;

        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let size = content.len();

        // Evict if over budget.
        while self.total_bytes + size > MAX_CACHE_BYTES && !self.entries.is_empty() {
            // Remove the first entry (arbitrary eviction).
            if let Some(key) = self.entries.keys().next().cloned()
                && let Some(evicted) = self.entries.remove(&key)
            {
                self.total_bytes -= evicted.size;
            }
        }

        self.total_bytes += size;
        self.entries.insert(
            canonical,
            CachedFile {
                content: content.clone(),
                modified,
                size,
            },
        );

        Ok(content)
    }

    /// Invalidate the cache entry for a file (e.g., after a write).
    pub fn invalidate(&mut self, path: &Path) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if let Some(evicted) = self.entries.remove(&canonical) {
            self.total_bytes -= evicted.size;
        }
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    /// Number of cached files.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Total cached bytes.
    pub fn bytes(&self) -> usize {
        self.total_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_read_and_invalidate() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        let mut cache = FileCache::new();

        // First read — cache miss.
        let content = cache.read(&file).unwrap();
        assert_eq!(content, "hello");
        assert_eq!(cache.len(), 1);

        // Second read — cache hit (same content).
        let content2 = cache.read(&file).unwrap();
        assert_eq!(content2, "hello");

        // Invalidate.
        cache.invalidate(&file);
        assert_eq!(cache.len(), 0);
    }
}
