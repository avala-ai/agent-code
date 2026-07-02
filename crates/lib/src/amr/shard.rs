//! SHARD stage: run the profile's selectors over the tree and emit signals.
//!
//! Deterministic and model-free. Walks the repository (honouring
//! `.gitignore`), applies every selector to each text file, and drops any
//! file that produces zero signals before the expensive MAP stage ever
//! sees it. File order is sorted so a given tree always yields the same
//! signals in the same order, which keeps batching and snapshot tests
//! reproducible.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::AmrError;
use super::profile::Profile;
use super::types::Signal;

/// Inputs to the shard stage.
pub struct ShardInput {
    pub repo_root: PathBuf,
    /// When `Some`, restrict scanning to these repo-relative paths (the
    /// incremental / diff case). When `None`, walk the whole tree.
    pub files: Option<Vec<PathBuf>>,
    /// Skip files larger than this many bytes (minified bundles, blobs).
    pub max_file_bytes: usize,
}

impl ShardInput {
    pub fn full(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            files: None,
            max_file_bytes: 1_000_000,
        }
    }
}

/// Output of the shard stage: the signals plus coverage accounting.
#[derive(Debug, Default)]
pub struct ShardOutput {
    pub signals: Vec<Signal>,
    /// Text files considered (read and classified).
    pub total_files: usize,
    /// Files that produced at least one signal (reach MAP).
    pub scanned_files: usize,
    /// Files considered but dropped for emitting zero signals.
    pub dropped_files: usize,
}

/// Run every selector over the selected files and collect signals.
pub fn collect_signals(profile: &Profile, input: &ShardInput) -> Result<ShardOutput, AmrError> {
    let files = match &input.files {
        Some(list) => {
            let mut v = list.clone();
            v.sort();
            v.dedup();
            v
        }
        None => walk_repo(&input.repo_root)?,
    };

    let mut out = ShardOutput::default();
    let root_canon = input.repo_root.canonicalize().ok();
    for rel in &files {
        // Hidden files (`.env`, `.git/config`, dotdirs) are outside the scan
        // scope. `walk_repo` already drops them via `.hidden(true)`, but the
        // incremental path takes its file list from `git diff`/`status`, which
        // reports changed hidden files too. Filter here so a secret in e.g.
        // `.env` is never read into a selector signal and thus never embedded
        // in a MAP prompt — mirroring the permission read-scope that denies
        // hidden paths to worker tool reads.
        if path_has_hidden_component(rel) {
            out.dropped_files += 1;
            continue;
        }
        let abs = input.repo_root.join(rel);
        // `symlink_metadata` does not follow the final component: a repo whose
        // file is a symlink to a local secret (e.g. `~/.ssh/id_rsa`) must not
        // be read into a selector signal.
        let Ok(meta) = std::fs::symlink_metadata(&abs) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            out.dropped_files += 1;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        // Defense in depth: skip anything that resolves outside the scan root
        // (e.g. via a symlinked parent directory in the path).
        if let (Some(root), Ok(canon)) = (root_canon.as_ref(), abs.canonicalize())
            && !canon.starts_with(root)
        {
            out.dropped_files += 1;
            continue;
        }
        out.total_files += 1;
        if meta.len() as usize > input.max_file_bytes {
            out.dropped_files += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(&abs) else {
            out.dropped_files += 1;
            continue;
        };
        if looks_binary(&bytes) {
            out.dropped_files += 1;
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            out.dropped_files += 1;
            continue;
        };

        let mut file_signals = Vec::new();
        for selector in &profile.selectors {
            file_signals.extend(selector.scan_text(rel, &text));
        }
        if file_signals.is_empty() {
            out.dropped_files += 1;
        } else {
            file_signals.sort_by(|a, b| {
                let ao = a.byte_range.map(|r| r.0).unwrap_or(0);
                let bo = b.byte_range.map(|r| r.0).unwrap_or(0);
                ao.cmp(&bo).then_with(|| a.selector_id.cmp(&b.selector_id))
            });
            out.scanned_files += 1;
            out.signals.extend(file_signals);
        }
    }
    Ok(out)
}

/// Walk the repository, returning sorted repo-relative paths of files that
/// git would track (respecting `.gitignore` and skipping hidden/`.git`).
fn walk_repo(repo_root: &Path) -> Result<Vec<PathBuf>, AmrError> {
    let mut files = Vec::new();
    for entry in WalkBuilder::new(repo_root)
        .standard_filters(true)
        .hidden(true)
        .git_ignore(true)
        .parents(true)
        .build()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(repo_root) {
            files.push(rel.to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

/// True if any component of `rel` begins with `.` (a dotfile or dotdir such as
/// `.env`, `.git/config`, or `config/.secret`). Hidden files are outside the
/// scan scope; the permission read-scope enforces the same rule for worker
/// reads, and this keeps the deterministic shard stage consistent with it on
/// the incremental path (where the file list comes from git, not `walk_repo`).
fn path_has_hidden_component(rel: &Path) -> bool {
    rel.components().any(
        |c| matches!(c, std::path::Component::Normal(os) if os.to_string_lossy().starts_with('.')),
    )
}

/// Heuristic binary detection: a NUL byte in the first 8 KiB.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amr::profile::security_profile;
    use std::fs;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn collect_drops_clean_files_and_keeps_vulnerable_ones() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "clean.py", "def add(a, b):\n    return a + b\n");
        write(
            dir.path(),
            "vuln.py",
            "import os\ndef h(r):\n    os.system('x' + r)\n",
        );
        write(dir.path(), "notes.md", "# just docs\n");

        let profile = security_profile();
        let out = collect_signals(&profile, &ShardInput::full(dir.path())).unwrap();

        assert!(out.total_files >= 3);
        assert_eq!(out.scanned_files, 1, "only vuln.py should reach MAP");
        assert!(out.dropped_files >= 2);
        assert!(
            out.signals
                .iter()
                .all(|s| s.file.as_path() == Path::new("vuln.py"))
        );
    }

    #[test]
    fn incremental_restricts_to_given_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.py", "eval(x)\n");
        write(dir.path(), "b.py", "eval(y)\n");

        let profile = security_profile();
        let input = ShardInput {
            repo_root: dir.path().to_path_buf(),
            files: Some(vec![PathBuf::from("b.py")]),
            max_file_bytes: 1_000_000,
        };
        let out = collect_signals(&profile, &input).unwrap();
        assert_eq!(out.total_files, 1);
        assert!(
            out.signals
                .iter()
                .all(|s| s.file.as_path() == Path::new("b.py"))
        );
    }

    #[test]
    fn incremental_list_skips_hidden_secret_files() {
        // Regression: the incremental path takes its file list from git, which
        // reports changed hidden files like `.env`. Those must be dropped
        // before any read so a credential never lands in a selector signal
        // (and thus a MAP prompt). A full walk already skips them.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".env", "api_key = \"sk-secretvalue123\"\n");
        write(dir.path(), ".git/config", "token = \"sk-secretvalue456\"\n");
        write(dir.path(), "app.py", "eval(x)\n");

        let profile = security_profile();
        let input = ShardInput {
            repo_root: dir.path().to_path_buf(),
            files: Some(vec![
                PathBuf::from(".env"),
                PathBuf::from(".git/config"),
                PathBuf::from("app.py"),
            ]),
            max_file_bytes: 1_000_000,
        };
        let out = collect_signals(&profile, &input).unwrap();
        // Only app.py is read; the hidden files are dropped, not scanned.
        assert_eq!(out.total_files, 1, "hidden files are not even read");
        assert!(
            out.signals
                .iter()
                .all(|s| s.file.as_path() == Path::new("app.py")),
            "no signal may carry a hidden-file path or its secret contents"
        );
        assert!(
            !out.signals
                .iter()
                .any(|s| s.evidence.contains("sk-secretvalue")),
            "no secret from a hidden file leaks into a signal"
        );
    }

    #[test]
    fn oversized_and_binary_files_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        // A file with a NUL byte is treated as binary.
        fs::write(dir.path().join("blob.py"), b"eval(\0x)\n").unwrap();
        let profile = security_profile();
        let out = collect_signals(&profile, &ShardInput::full(dir.path())).unwrap();
        assert_eq!(out.scanned_files, 0);
    }

    #[test]
    fn output_is_deterministic_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "z.py", "os.system(a)\n");
        write(dir.path(), "a.py", "eval(b)\n");
        let profile = security_profile();
        let run1 = collect_signals(&profile, &ShardInput::full(dir.path())).unwrap();
        let run2 = collect_signals(&profile, &ShardInput::full(dir.path())).unwrap();
        assert_eq!(run1.signals, run2.signals);
    }
}
