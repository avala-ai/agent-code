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

/// On-disk path of the incremental cache for `repo_root`.
///
/// The cache lives under the user cache directory, keyed by the canonical
/// repo path — NOT inside the scanned repository. A scanned repo therefore
/// cannot supply a poisoned cache (one that sets `base_commit` to skip every
/// file), and scans never write into the target tree.
fn cache_path(repo_root: &Path) -> Option<PathBuf> {
    let canon = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let key = sha256_hex(canon.to_string_lossy().as_bytes());
    Some(
        dirs::cache_dir()?
            .join("agent-code")
            .join("amr")
            .join(format!("{key}.json")),
    )
}

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
        let Some(path) = cache_path(repo_root) else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist the cache for `repo_root` under the user cache directory.
    pub fn save(&self, repo_root: &Path) -> std::io::Result<()> {
        let Some(path) = cache_path(repo_root) else {
            return Ok(());
        };
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

    // git reports paths relative to the repository TOP-LEVEL, but the shard
    // stage and cache keys are relative to `repo_root`, which may be a
    // SUBDIRECTORY of the git repo (e.g. `security-scan ./services/api` in a
    // monorepo). Translate every path back to repo_root-relative and drop
    // anything outside repo_root; otherwise a changed file is never re-scanned
    // (repo_root.join(top_level_path) points at a nonexistent nested path) and
    // its stale cached finding is carried forward — a false-clean gate.
    let prefix = git_show_prefix(repo_root);

    // NUL-delimited so paths with spaces/newlines/quotes and rename records
    // survive intact; line-based parsing silently corrupts them, which would
    // let a changed file be skipped and reported clean.
    let diff = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["diff", "--name-only", "-z", &format!("{base}..HEAD")])
        .output()
        .ok()?;
    if !diff.status.success() {
        return None;
    }
    files.extend(strip_prefix_paths(parse_nul_paths(&diff.stdout), &prefix));

    // Uncommitted + untracked changes (porcelain v1, NUL-delimited).
    if let Ok(status) = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["status", "--porcelain=v1", "-z"])
        .output()
        && status.status.success()
    {
        files.extend(strip_prefix_paths(parse_status_z(&status.stdout), &prefix));
    }

    Some(files)
}

/// `repo_root`'s path relative to its git top-level as a `/`-terminated,
/// forward-slashed prefix (`git rev-parse --show-prefix`). Empty when
/// `repo_root` is itself the top-level (or not in a git repo). git always
/// emits forward slashes here and in diff/status output, so prefix stripping
/// is a plain string operation on every platform.
fn git_show_prefix(repo_root: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--show-prefix"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim_end_matches('\n').to_string())
        .unwrap_or_default()
}

/// Rebase git's top-level-relative paths onto `repo_root` by stripping
/// `prefix`. Paths that do not start with `prefix` are outside the scan root
/// (a sibling subdirectory in a monorepo) and are dropped.
fn strip_prefix_paths(paths: Vec<PathBuf>, prefix: &str) -> Vec<PathBuf> {
    if prefix.is_empty() {
        return paths;
    }
    paths
        .into_iter()
        .filter_map(|p| {
            p.to_string_lossy()
                .strip_prefix(prefix)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .collect()
}

/// Split NUL-delimited git output (`--name-only -z`) into paths.
fn parse_nul_paths(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Parse `git status --porcelain=v1 -z` output. Each record is `XY <path>`;
/// a rename/copy (`R`/`C`) is followed by its original path as the next NUL
/// field, which is included so a renamed file is always re-scanned.
fn parse_status_z(bytes: &[u8]) -> Vec<PathBuf> {
    let text = String::from_utf8_lossy(bytes);
    let mut tokens = text.split('\0').filter(|t| !t.is_empty());
    let mut out = Vec::new();
    while let Some(entry) = tokens.next() {
        if entry.len() < 4 {
            continue;
        }
        let status_code = &entry[..2];
        let path = &entry[3..];
        if !path.is_empty() {
            out.push(PathBuf::from(path));
        }
        if (status_code.contains('R') || status_code.contains('C'))
            && let Some(orig) = tokens.next()
            && !orig.is_empty()
        {
            out.push(PathBuf::from(orig));
        }
    }
    out
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
        // Redirect the user cache dir so the test never touches the real one.
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::test_support::EnvGuard::set_many(&[
            ("HOME", home.path()),
            ("XDG_CACHE_HOME", home.path()),
        ]);
        let repo = tempfile::tempdir().unwrap();
        let mut cache = ScanCache {
            base_commit: Some("abc123".into()),
            ..Default::default()
        };
        cache.index_findings(&[finding("f1", "a.py"), finding("f2", "a.py")]);
        cache.save(repo.path()).unwrap();

        let loaded = ScanCache::load(repo.path());
        assert_eq!(loaded.base_commit.as_deref(), Some("abc123"));
        assert_eq!(loaded.all_findings().len(), 2);
    }

    #[test]
    fn missing_cache_loads_as_empty() {
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::test_support::EnvGuard::set_many(&[
            ("HOME", home.path()),
            ("XDG_CACHE_HOME", home.path()),
        ]);
        let repo = tempfile::tempdir().unwrap();
        let cache = ScanCache::load(repo.path());
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

    #[test]
    fn parse_nul_paths_preserves_special_chars() {
        let paths: Vec<String> = parse_nul_paths(b"a b.py\0c\td.py\0")
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(paths, vec!["a b.py".to_string(), "c\td.py".to_string()]);
    }

    #[test]
    fn changed_files_since_relativizes_a_subdirectory_scan_root() {
        // Regression: when the scan root is a SUBDIRECTORY of the git repo,
        // git reports top-level-relative paths (`sub/a.py`). They must be
        // rebased to the scan root (`a.py`) — matching the shard walk and cache
        // keys — and changes outside the scan root must be dropped. Otherwise
        // an incremental subdir scan silently skips changed files.
        fn git(dir: &Path, args: &[&str]) {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        }
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@t.co"]);
        git(root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        std::fs::write(root.join("sub/a.py"), "x=1\n").unwrap();
        std::fs::write(root.join("sub/deep/c.py"), "z=1\n").unwrap();
        std::fs::write(root.join("top.py"), "y=1\n").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "base"]);
        let base = head_commit(root).unwrap();

        // Change files inside the subdir (committed diff) plus one outside it.
        std::fs::write(root.join("sub/a.py"), "x=2\n").unwrap();
        std::fs::write(root.join("sub/deep/c.py"), "z=2\n").unwrap();
        std::fs::write(root.join("top.py"), "y=2\n").unwrap();
        git(root, &["commit", "-aqm", "change"]);

        let subroot = root.join("sub");
        let changed = changed_files_since(&subroot, &base).unwrap();

        // Rebased to the scan root, nested path preserved, sibling dropped.
        assert!(
            changed.contains(&PathBuf::from("a.py")),
            "sub/a.py -> a.py; got {changed:?}"
        );
        assert!(
            changed.contains(&PathBuf::from("deep/c.py")),
            "sub/deep/c.py -> deep/c.py; got {changed:?}"
        );
        assert!(
            !changed
                .iter()
                .any(|p| p.to_string_lossy().contains("top.py")),
            "a change outside the scan root is excluded; got {changed:?}"
        );
        assert!(
            !changed
                .iter()
                .any(|p| p.to_string_lossy().starts_with("sub/")),
            "no top-level-relative paths leak through; got {changed:?}"
        );
    }

    #[test]
    fn parse_status_z_handles_spaces_and_renames() {
        // Untracked with a space, a modified file, and a rename (new + orig).
        let bytes = b"?? a b.py\0 M src/x.py\0R  new.py\0old.py\0";
        let paths: Vec<String> = parse_status_z(bytes)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(paths.contains(&"a b.py".to_string()));
        assert!(paths.contains(&"src/x.py".to_string()));
        assert!(paths.contains(&"new.py".to_string()));
        assert!(
            paths.contains(&"old.py".to_string()),
            "rename origin included"
        );
    }
}
