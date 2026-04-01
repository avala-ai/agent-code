//! Session environment setup.
//!
//! Initializes the runtime environment at session start:
//! - Detects project root (git root or cwd)
//! - Loads memory context
//! - Sets up safe environment variables
//! - Configures signal handlers
//! - Prepares the session state

use std::path::{Path, PathBuf};

/// Resolved session environment.
#[derive(Debug, Clone)]
pub struct SessionEnvironment {
    /// Original working directory at startup.
    pub original_cwd: PathBuf,
    /// Detected project root (git root or cwd).
    pub project_root: PathBuf,
    /// Whether we're inside a git repository.
    pub is_git_repo: bool,
    /// Current git branch (if in a repo).
    pub git_branch: Option<String>,
    /// Platform identifier.
    pub platform: String,
    /// Default shell.
    pub shell: String,
    /// Whether the session is interactive (has a TTY).
    pub is_interactive: bool,
    /// Terminal width in columns.
    pub terminal_width: u16,
}

impl SessionEnvironment {
    /// Detect and build the session environment.
    pub async fn detect() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let project_root = detect_project_root(&cwd).await;
        let is_git = crate::services::git::is_git_repo(&cwd).await;
        let branch = if is_git {
            crate::services::git::current_branch(&cwd).await
        } else {
            None
        };

        let terminal_width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);

        Self {
            original_cwd: cwd,
            project_root,
            is_git_repo: is_git,
            git_branch: branch,
            platform: std::env::consts::OS.to_string(),
            shell: detect_shell(),
            is_interactive: atty_check(),
            terminal_width,
        }
    }
}

/// Walk up from cwd to find the project root (git root or first dir with config).
async fn detect_project_root(cwd: &Path) -> PathBuf {
    // Try git root first.
    if let Some(root) = crate::services::git::repo_root(cwd).await {
        return PathBuf::from(root);
    }

    // Look for project markers.
    let markers = [
        ".rc",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
    ];
    let mut dir = cwd.to_path_buf();
    loop {
        for marker in &markers {
            if dir.join(marker).exists() {
                return dir;
            }
        }
        if !dir.pop() {
            break;
        }
    }

    cwd.to_path_buf()
}

fn detect_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string())
}

fn atty_check() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: isatty is a standard POSIX function with no preconditions.
        unsafe { libc::isatty(0) != 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}
