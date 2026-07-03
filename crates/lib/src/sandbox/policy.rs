//! Resolved sandbox policy used at spawn time.
//!
//! A [`SandboxPolicy`] is derived from the user's `[sandbox]` config section
//! by resolving tildes and making paths absolute. It is what a
//! [`SandboxStrategy`](super::SandboxStrategy) consumes when wrapping a
//! subprocess — the raw config is never passed into strategy code.

use std::path::{Path, PathBuf};

use crate::config::SandboxConfig;

/// A concrete sandbox policy with paths resolved to absolute form.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// Project root — always writable inside the sandbox.
    pub project_dir: PathBuf,
    /// Additional writable paths (e.g. `/tmp`, `~/.cache/agent-code`).
    pub allowed_write_paths: Vec<PathBuf>,
    /// Paths that must never be readable (e.g. `~/.ssh`, `~/.aws`).
    pub forbidden_paths: Vec<PathBuf>,
    /// Whether network access is permitted inside the sandbox.
    pub allow_network: bool,
}

impl SandboxPolicy {
    /// Build a policy from a [`SandboxConfig`] and a project directory.
    ///
    /// Path entries have `~` expanded via `$HOME` and are left as-is if
    /// absolute. Relative paths are joined onto `project_dir`.
    pub fn from_config(config: &SandboxConfig, project_dir: &Path) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let expand = |s: &String| expand_path(s.as_str(), &home, project_dir);

        Self {
            project_dir: project_dir.to_path_buf(),
            allowed_write_paths: config.allowed_write_paths.iter().map(expand).collect(),
            forbidden_paths: config.forbidden_paths.iter().map(expand).collect(),
            allow_network: config.allow_network,
        }
    }
}

fn expand_path(raw: &str, home: &Option<PathBuf>, project_dir: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    if raw == "~"
        && let Some(home) = home
    {
        return home.clone();
    }

    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        project_dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tilde_to_home() {
        let home = Some(PathBuf::from("/Users/alice"));
        let project = PathBuf::from("/work/repo");
        assert_eq!(
            expand_path("~/.cache/agent-code", &home, &project),
            PathBuf::from("/Users/alice/.cache/agent-code")
        );
        assert_eq!(
            expand_path("~", &home, &project),
            PathBuf::from("/Users/alice")
        );
    }

    #[test]
    fn absolute_paths_unchanged() {
        let home = Some(PathBuf::from("/Users/alice"));
        let project = PathBuf::from("/work/repo");
        assert_eq!(expand_path("/tmp", &home, &project), PathBuf::from("/tmp"));
    }

    #[test]
    fn relative_paths_join_project_dir() {
        let home = Some(PathBuf::from("/Users/alice"));
        let project = PathBuf::from("/work/repo");
        assert_eq!(
            expand_path("target", &home, &project),
            PathBuf::from("/work/repo/target")
        );
    }

    #[test]
    fn missing_home_leaves_tilde() {
        // When $HOME is unset, tilde paths fall through to the absolute/relative rules.
        let home: Option<PathBuf> = None;
        let project = PathBuf::from("/work/repo");
        // Stripping ~/ fails without $HOME, so we treat "~/foo" as a relative path.
        assert_eq!(
            expand_path("~/foo", &home, &project),
            PathBuf::from("/work/repo/~/foo")
        );
    }

    #[test]
    fn from_config_resolves_paths() {
        // Use a guaranteed-present $HOME via the shared cross-test env
        // guard. The guard holds a process-wide mutex while alive, so
        // concurrent tests in `tools::brief` / `tools::config_tool`
        // that also mutate `HOME` can't observe our temporary
        // `/Users/test` override.
        let _g = crate::test_support::EnvGuard::set_str("HOME", "/Users/test");
        let cfg = SandboxConfig {
            enabled: true,
            strategy: "seatbelt".to_string(),
            allowed_write_paths: vec!["/tmp".to_string(), "~/.cache/agent-code".to_string()],
            forbidden_paths: vec!["~/.ssh".to_string()],
            allow_network: false,
            fail_closed: true,
        };
        let policy = SandboxPolicy::from_config(&cfg, Path::new("/work/repo"));
        assert_eq!(policy.project_dir, PathBuf::from("/work/repo"));
        assert_eq!(
            policy.allowed_write_paths,
            vec![
                PathBuf::from("/tmp"),
                PathBuf::from("/Users/test/.cache/agent-code"),
            ]
        );
        assert_eq!(
            policy.forbidden_paths,
            vec![PathBuf::from("/Users/test/.ssh")]
        );
        assert!(!policy.allow_network);
    }
}
