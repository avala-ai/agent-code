//! Git integration utilities.
//!
//! Helpers for interacting with git repositories — status, diff,
//! log, blame, and branch operations. All operations shell out to
//! the git CLI for maximum compatibility.

use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Check if the given directory is inside a git repository.
pub async fn is_git_repo(cwd: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Get the root of the current git repository.
pub async fn repo_root(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Resolve the canonical repository root, following worktree links.
///
/// If the current directory is inside a worktree, this returns the
/// path to the main repository (not the worktree checkout).
pub async fn canonical_root(cwd: &Path) -> Option<String> {
    // git rev-parse --git-common-dir gives the shared .git directory.
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(cwd)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return repo_root(cwd).await;
    }

    let common_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // If the common dir ends with "/.git", the parent is the canonical root.
    if common_dir.ends_with("/.git") || common_dir.ends_with("\\.git") {
        let root = common_dir
            .strip_suffix("/.git")
            .or_else(|| common_dir.strip_suffix("\\.git"))
            .unwrap_or(&common_dir);
        Some(root.to_string())
    } else if common_dir == ".git" {
        // Not a worktree — use regular root.
        repo_root(cwd).await
    } else {
        // Absolute path to shared git dir.
        let path = std::path::Path::new(&common_dir);
        path.parent().map(|p| p.display().to_string())
    }
}

/// Check if the repository is a shallow clone.
pub async fn is_shallow(cwd: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-shallow-repository"])
        .current_dir(cwd)
        .output()
        .await
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Check if the current directory is inside a worktree (not the main checkout).
pub async fn is_worktree(cwd: &Path) -> bool {
    let toplevel = repo_root(cwd).await;
    let canonical = canonical_root(cwd).await;
    match (toplevel, canonical) {
        (Some(t), Some(c)) => t != c,
        _ => false,
    }
}

/// Get the current branch name.
pub async fn current_branch(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(cwd)
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() {
            None
        } else {
            Some(branch)
        }
    } else {
        None
    }
}

/// Get the default/main branch name.
pub async fn default_branch(cwd: &Path) -> String {
    // Try common conventions.
    for name in &["main", "master"] {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .current_dir(cwd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        if output.map(|s| s.success()).unwrap_or(false) {
            return name.to_string();
        }
    }
    "main".to_string()
}

/// Get git status (short format).
pub async fn status(cwd: &Path) -> Result<String, String> {
    run_git(cwd, &["status", "--short"]).await
}

/// Get staged and unstaged diff.
pub async fn diff(cwd: &Path) -> Result<String, String> {
    let staged = run_git(cwd, &["diff", "--cached"])
        .await
        .unwrap_or_default();
    let unstaged = run_git(cwd, &["diff"]).await.unwrap_or_default();

    let mut result = String::new();
    if !staged.is_empty() {
        result.push_str("=== Staged changes ===\n");
        result.push_str(&staged);
    }
    if !unstaged.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("=== Unstaged changes ===\n");
        result.push_str(&unstaged);
    }
    if result.is_empty() {
        result = "(no changes)".to_string();
    }
    Ok(result)
}

/// Get recent commit log.
pub async fn log(cwd: &Path, count: usize) -> Result<String, String> {
    run_git(cwd, &["log", "--oneline", &format!("-{count}")]).await
}

/// Get blame for a file (abbreviated).
pub async fn blame(cwd: &Path, file: &str) -> Result<String, String> {
    run_git(cwd, &["blame", "--line-porcelain", file]).await
}

/// Get the diff between the current branch and the default branch.
pub async fn diff_from_base(cwd: &Path) -> Result<String, String> {
    let base = default_branch(cwd).await;
    run_git(cwd, &["diff", &format!("{base}...HEAD")]).await
}

/// Parse a unified diff into structured hunks.
pub fn parse_diff(diff_text: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current_file: Option<DiffFile> = None;
    let mut current_hunk: Option<DiffHunk> = None;

    for line in diff_text.lines() {
        if line.starts_with("diff --git") {
            // Save previous file.
            if let Some(mut file) = current_file.take() {
                if let Some(hunk) = current_hunk.take() {
                    file.hunks.push(hunk);
                }
                files.push(file);
            }

            // Extract file path from "diff --git a/path b/path".
            let path = line.split(" b/").nth(1).unwrap_or("unknown").to_string();

            current_file = Some(DiffFile {
                path,
                hunks: Vec::new(),
            });
        } else if line.starts_with("@@") {
            if let Some(ref mut file) = current_file
                && let Some(hunk) = current_hunk.take()
            {
                file.hunks.push(hunk);
            }
            current_hunk = Some(DiffHunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current_hunk {
            let kind = match line.chars().next() {
                Some('+') => DiffLineKind::Added,
                Some('-') => DiffLineKind::Removed,
                _ => DiffLineKind::Context,
            };
            hunk.lines.push(DiffLine {
                kind,
                content: line.to_string(),
            });
        }
    }

    // Save last file.
    if let Some(mut file) = current_file {
        if let Some(hunk) = current_hunk {
            file.hunks.push(hunk);
        }
        files.push(file);
    }

    files
}

/// A file in a parsed diff.
#[derive(Debug, Clone)]
pub struct DiffFile {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
}

/// A hunk within a diff file.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Added,
    Removed,
    Context,
}

impl DiffFile {
    /// Count added and removed lines.
    pub fn stats(&self) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;
        for hunk in &self.hunks {
            for line in &hunk.lines {
                match line.kind {
                    DiffLineKind::Added => added += 1,
                    DiffLineKind::Removed => removed += 1,
                    DiffLineKind::Context => {}
                }
            }
        }
        (added, removed)
    }
}

/// Run a git command and return stdout.
async fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| format!("git command failed: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git error: {stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_diff() {
        let diff = "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
+    println!(\"added\");
 }
";
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].hunks.len(), 1);

        let (added, removed) = files[0].stats();
        assert_eq!(added, 2);
        assert_eq!(removed, 1);
    }

    #[test]
    fn test_parse_diff_multiple_files() {
        let diff = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,1 +1,1 @@
-old
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,1 +1,2 @@
 keep
+added
";
        let files = parse_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[1].path, "b.rs");
    }

    #[test]
    fn test_parse_diff_empty() {
        let files = parse_diff("");
        assert!(files.is_empty());
    }

    #[test]
    fn test_diff_line_kinds() {
        assert!(matches!(DiffLineKind::Added, DiffLineKind::Added));
        assert!(matches!(DiffLineKind::Removed, DiffLineKind::Removed));
        assert!(matches!(DiffLineKind::Context, DiffLineKind::Context));
    }

    #[tokio::test]
    async fn test_is_git_repo_in_repo() {
        // This test runs inside the agent-code repo itself.
        // Create a directory that IS a git repo.
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(is_git_repo(dir.path()).await);
    }

    #[tokio::test]
    async fn test_is_git_repo_not_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(dir.path()).await);
    }

    #[tokio::test]
    async fn test_repo_root() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        let root = repo_root(dir.path()).await;
        assert!(root.is_some());
    }

    #[tokio::test]
    async fn test_current_branch_new_repo() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        // New repo with no commits may not have a branch.
        let _branch = current_branch(dir.path()).await;
        // Just ensure it doesn't panic.
    }

    #[tokio::test]
    async fn test_current_branch_with_commit() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        std::fs::write(dir.path().join("f.txt"), "hi").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let branch = current_branch(dir.path()).await;
        assert!(branch.is_some());
    }

    #[tokio::test]
    async fn test_status_and_diff() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        std::fs::write(dir.path().join("f.txt"), "v1").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        // Modify file.
        std::fs::write(dir.path().join("f.txt"), "v2").unwrap();

        let st = status(dir.path()).await.unwrap();
        assert!(st.contains("f.txt"));

        let d = diff(dir.path()).await.unwrap();
        assert!(d.contains("v1") || d.contains("v2"));
    }

    #[tokio::test]
    async fn test_is_shallow_and_worktree() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        // Fresh init is not shallow and not a worktree.
        assert!(!is_shallow(dir.path()).await);
        assert!(!is_worktree(dir.path()).await);
    }
}
