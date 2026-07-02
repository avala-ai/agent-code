//! Incremental cache: pay for the diff, not a full pass.
//!
//! The expensive stage is MAP (one LLM worker per shard). To avoid re-running
//! it on unchanged code, the cache persists the MAP-stage findings keyed by
//! the file they were attributed to, plus the commit they were computed at.
//! On an incremental rerun the orchestrator asks git which files changed
//! since that commit, re-runs MAP only over those, carries the cached
//! findings for every unchanged file forward, and re-runs the cheap REDUCE
//! over the union.
//!
//! ```text
//!   base_commit ─┐
//!   findings_by_file (from last run)
//!                │
//!   HEAD ───────▶ git diff ──▶ changed files ──▶ re-MAP (diff only)
//!                                                      │
//!   cached[unchanged files] ────────────┬─────────────┘
//!                                        ▼
//!                                    REDUCE (all)
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use super::types::Finding;

/// Where the cache lives, relative to the repo root. Matches the repo's
/// existing `.agent/` project-config convention.
pub const CACHE_RELPATH: &str = ".agent/amr/cache.json";

/// Persisted incremental-scan state.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanCache {
    /// Commit the cached findings were computed at.
    #[serde(default)]
    pub base_commit: Option<String>,
    /// MAP-stage findings, keyed by the repo-relative file they touch.
    #[serde(default)]
    pub findings_by_file: BTreeMap<String, Vec<Finding>>,
}

impl ScanCache {
    /// Load the cache for `repo_root`, or an empty cache if none exists or
    /// it cannot be parsed (a stale/corrupt cache is never fatal).
    pub fn load(repo_root: &Path) -> Self {
        let path = repo_root.join(CACHE_RELPATH);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist the cache under `repo_root`, creating `.agent/amr/` as needed.
    pub fn save(&self, repo_root: &Path) -> std::io::Result<()> {
        let path = repo_root.join(CACHE_RELPATH);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        std::fs::write(path, text)
    }

    /// All cached findings flattened, in file order.
    pub fn all_findings(&self) -> Vec<Finding> {
        self.findings_by_file.values().flatten().cloned().collect()
    }

    /// Index a fresh set of findings by their `file`, replacing any prior
    /// entries for those files.
    pub fn index_findings(&mut self, findings: &[Finding]) {
        let mut grouped: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
        for f in findings {
            grouped.entry(f.file.clone()).or_default().push(f.clone());
        }
        for (file, fs) in grouped {
            self.findings_by_file.insert(file, fs);
        }
    }
}

/// Merge cached findings with a fresh set computed over `changed` files.
///
/// Cached findings attributed to a changed file are discarded (they were
/// just recomputed); every other cached finding is carried forward. The
/// result is the full pre-reduce finding set the REDUCE stage sees.
pub fn merge_incremental(
    cached: &ScanCache,
    changed: &BTreeSet<PathBuf>,
    fresh: &[Finding],
) -> Vec<Finding> {
    let changed_str: BTreeSet<String> = changed
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let mut out: Vec<Finding> = cached
        .findings_by_file
        .iter()
        .filter(|(file, _)| !changed_str.contains(*file))
        .flat_map(|(_, fs)| fs.clone())
        .collect();
    out.extend(fresh.iter().cloned());
    out
}

/// SHA-256 of arbitrary bytes as lowercase hex. Used for shard content
/// hashing and stable dedup keys.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for byte in digest {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

/// The current `HEAD` commit of the repo at `repo_root`, if it is a git repo.
pub fn head_commit(repo_root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// True only when `repo_root` is a git repo with a completely clean working
/// tree (no staged, unstaged, or untracked changes). Incremental caching keys
/// on the HEAD commit, so a scan of a dirty tree must not be cached under
/// HEAD — a later clean `--incremental` run would otherwise treat the changed
/// files as unchanged and skip them.
pub fn worktree_is_clean(repo_root: &Path) -> bool {
    match Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["status", "--porcelain"])
        .output()
    {
        Ok(out) => out.status.success() && out.stdout.is_empty(),
        Err(_) => false,
    }
}

/// Repo-relative files that changed since `base` (committed diff plus
/// modified/untracked working-tree files). `None` if git is unavailable
/// or `base` is unknown, which signals the caller to fall back to a full scan.
pub fn changed_files_since(repo_root: &Path, base: &str) -> Option<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();

    let diff = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["diff", "--name-only", &format!("{base}..HEAD")])
        .output()
        .ok()?;
    if !diff.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&diff.stdout).lines() {
        if !line.trim().is_empty() {
            files.insert(PathBuf::from(line.trim()));
        }
    }

    // Uncommitted changes and untracked files (porcelain: XY <path>).
    if let Ok(status) = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["status", "--porcelain"])
        .output()
    {
        for line in String::from_utf8_lossy(&status.stdout).lines() {
            if line.len() > 3 {
                files.insert(PathBuf::from(line[3..].trim()));
            }
        }
    }

    Some(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amr::types::Severity;

    fn finding(id: &str, file: &str) -> Finding {
        Finding {
            id: id.into(),
            cwe: None,
            file: file.into(),
            line_range: None,
            severity: Severity::P1,
            confidence: 0.8,
            title: "t".into(),
            root_cause: String::new(),
            exploit_preconditions: String::new(),
            evidence: String::new(),
            selector_id: None,
            shard_id: None,
        }
    }

    #[test]
    fn cache_roundtrips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ScanCache {
            base_commit: Some("abc123".into()),
            ..Default::default()
        };
        cache.index_findings(&[finding("f1", "a.py"), finding("f2", "a.py")]);
        cache.save(dir.path()).unwrap();

        let loaded = ScanCache::load(dir.path());
        assert_eq!(loaded.base_commit.as_deref(), Some("abc123"));
        assert_eq!(loaded.all_findings().len(), 2);
    }

    #[test]
    fn missing_cache_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ScanCache::load(dir.path());
        assert!(cache.base_commit.is_none());
        assert!(cache.all_findings().is_empty());
    }

    #[test]
    fn merge_carries_unchanged_and_replaces_changed() {
        let mut cached = ScanCache::default();
        cached.index_findings(&[finding("old_a", "a.py"), finding("old_b", "b.py")]);

        let changed: BTreeSet<PathBuf> = [PathBuf::from("a.py")].into_iter().collect();
        let fresh = vec![finding("new_a", "a.py")];

        let merged = merge_incremental(&cached, &changed, &fresh);
        let ids: BTreeSet<_> = merged.iter().map(|f| f.id.clone()).collect();
        // a.py's old finding dropped, new one added; b.py carried forward.
        assert!(ids.contains("new_a"));
        assert!(ids.contains("old_b"));
        assert!(!ids.contains("old_a"));
    }

    #[test]
    fn sha256_is_stable_and_distinct() {
        assert_eq!(sha256_hex(b"hello"), sha256_hex(b"hello"));
        assert_ne!(sha256_hex(b"hello"), sha256_hex(b"world"));
        assert_eq!(sha256_hex(b"hello").len(), 64);
    }
}
