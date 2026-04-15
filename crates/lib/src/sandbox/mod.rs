//! Process-level sandboxing for subprocess-spawning tools.
//!
//! Sandboxing wraps [`tokio::process::Command`] with an OS-level isolation
//! mechanism before the child is spawned. The current permission system is
//! policy enforced *inside* the agent process — a compromised tool or a
//! prompt-injection attack gets the agent's full privileges. Sandboxing
//! adds a second layer of defense so that even a compromised subprocess
//! cannot write outside the project directory or read credentials.
//!
//! # Platform support
//!
//! This first slice ships **macOS only** using `sandbox-exec` (Seatbelt).
//! Linux `bwrap` and Windows Low Integrity strategies will land as
//! follow-up PRs behind the same [`SandboxStrategy`] trait.
//!
//! # Wiring
//!
//! A [`SandboxExecutor`] is built once per session from
//! [`crate::config::schema::SandboxConfig`] and threaded into
//! [`crate::tools::ToolContext`]. Subprocess-spawning tools (currently:
//! [`Bash`](crate::tools::Bash)) call [`SandboxExecutor::wrap`] on their
//! `Command` before `.spawn()`. When sandboxing is disabled or the
//! platform has no strategy, `wrap` returns the command unchanged.

pub mod policy;
pub mod seatbelt;

pub use policy::SandboxPolicy;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::config::SandboxConfig;

/// Strategy trait for wrapping a subprocess command with OS-level isolation.
pub trait SandboxStrategy: Send + Sync {
    /// Short name for diagnostics and logging (e.g. `"seatbelt"`, `"noop"`).
    fn name(&self) -> &'static str;

    /// Wrap `cmd` so the spawned child runs inside the sandbox.
    ///
    /// Implementations may build a new [`Command`] that invokes a helper
    /// (e.g. `sandbox-exec`) with the original program as its child. The
    /// returned command must preserve the original working directory and
    /// environment but does **not** need to re-apply stdio — the caller
    /// re-applies piped stdio via [`SandboxExecutor::wrap`].
    fn wrap_command(&self, cmd: Command, policy: &SandboxPolicy) -> Command;
}

/// No-op strategy used when sandboxing is disabled or unavailable.
pub struct NoopStrategy;

impl SandboxStrategy for NoopStrategy {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn wrap_command(&self, cmd: Command, _policy: &SandboxPolicy) -> Command {
        cmd
    }
}

/// Owns the active strategy and resolved policy for a session.
///
/// Construct one at session start via [`SandboxExecutor::from_config`] and
/// thread it into [`crate::tools::ToolContext::sandbox`]. Tools call
/// [`SandboxExecutor::wrap`] before spawning.
pub struct SandboxExecutor {
    strategy: Arc<dyn SandboxStrategy>,
    policy: SandboxPolicy,
    enabled: bool,
    /// Whether per-tool-call bypass (e.g. the `dangerouslyDisableSandbox`
    /// Bash tool parameter) is permitted. Derived from
    /// `security.disable_bypass_permissions == false`.
    allow_bypass: bool,
}

impl SandboxExecutor {
    /// Build an executor from config and the session's project directory.
    ///
    /// If `config.enabled` is false, the returned executor's [`wrap`] is a
    /// no-op. If the selected strategy is unavailable on the current
    /// platform (e.g. `seatbelt` on Linux), falls back to [`NoopStrategy`]
    /// and logs a warning — the caller should still treat this as enabled
    /// for `/sandbox` reporting so the degradation is visible.
    pub fn from_config(config: &SandboxConfig, project_dir: &Path) -> Self {
        Self::from_config_with_bypass(config, project_dir, true)
    }

    /// Build an executor, explicitly setting whether per-call bypass is allowed.
    ///
    /// Call sites with access to the full [`crate::config::Config`] should
    /// prefer [`SandboxExecutor::from_session_config`], which reads the
    /// bypass flag from `security.disable_bypass_permissions`.
    pub fn from_config_with_bypass(
        config: &SandboxConfig,
        project_dir: &Path,
        allow_bypass: bool,
    ) -> Self {
        let policy = SandboxPolicy::from_config(config, project_dir);
        let strategy = pick_strategy(&config.strategy);

        if config.enabled && strategy.name() == "noop" {
            warn!(
                "sandbox enabled in config but no working strategy on this platform; \
                 running without OS-level isolation"
            );
        }

        Self {
            strategy,
            policy,
            enabled: config.enabled,
            allow_bypass,
        }
    }

    /// Build an executor from the top-level [`crate::config::Config`],
    /// honoring the enterprise `security.disable_bypass_permissions` flag.
    pub fn from_session_config(config: &crate::config::Config, project_dir: &Path) -> Self {
        Self::from_config_with_bypass(
            &config.sandbox,
            project_dir,
            !config.security.disable_bypass_permissions,
        )
    }

    /// Strategy name for diagnostics (e.g. `/sandbox` command output).
    pub fn strategy_name(&self) -> &'static str {
        self.strategy.name()
    }

    /// Whether sandboxing is active (config enabled *and* a real strategy is selected).
    pub fn is_active(&self) -> bool {
        self.enabled && self.strategy.name() != "noop"
    }

    /// Access the resolved policy for diagnostics.
    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// Whether a tool call may request per-call bypass (e.g. the Bash tool's
    /// `dangerouslyDisableSandbox` parameter).
    ///
    /// Returns `false` when `security.disable_bypass_permissions = true`.
    pub fn allow_bypass(&self) -> bool {
        self.allow_bypass
    }

    /// Wrap `cmd` with the active strategy, reapplying piped stdio.
    ///
    /// If the executor is disabled or the strategy is a no-op, returns
    /// `cmd` unchanged. Callers should use this before `.spawn()`.
    pub fn wrap(&self, cmd: Command) -> Command {
        if !self.is_active() {
            return cmd;
        }
        debug!(
            strategy = self.strategy.name(),
            project_dir = %self.policy.project_dir.display(),
            "wrapping subprocess with sandbox"
        );
        let mut wrapped = self.strategy.wrap_command(cmd, &self.policy);
        // Strategies do not re-apply stdio (tokio hides it); force piped
        // stdio so the caller can still read stdout/stderr of the wrapped
        // child process.
        wrapped
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        wrapped
    }

    /// A "disabled" executor for tests and default construction.
    pub fn disabled() -> Self {
        Self {
            strategy: Arc::new(NoopStrategy),
            policy: SandboxPolicy {
                project_dir: PathBuf::from("."),
                allowed_write_paths: Vec::new(),
                forbidden_paths: Vec::new(),
                allow_network: true,
            },
            enabled: false,
            allow_bypass: true,
        }
    }
}

fn pick_strategy(requested: &str) -> Arc<dyn SandboxStrategy> {
    match requested {
        "none" => Arc::new(NoopStrategy),
        "seatbelt" => make_seatbelt_or_noop(),
        "auto" | "" => auto_detect(),
        other => {
            warn!("unknown sandbox strategy {other:?}; falling back to noop");
            Arc::new(NoopStrategy)
        }
    }
}

fn auto_detect() -> Arc<dyn SandboxStrategy> {
    if cfg!(target_os = "macos") {
        make_seatbelt_or_noop()
    } else {
        Arc::new(NoopStrategy)
    }
}

fn make_seatbelt_or_noop() -> Arc<dyn SandboxStrategy> {
    if cfg!(target_os = "macos") && sandbox_exec_available() {
        Arc::new(seatbelt::SeatbeltStrategy)
    } else {
        Arc::new(NoopStrategy)
    }
}

/// True if `sandbox-exec` is resolvable on `$PATH`.
fn sandbox_exec_available() -> bool {
    // Probe via `which`-style $PATH walk. We avoid `std::process::Command`
    // here to keep this synchronous and not spawn a child at detect time.
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        if dir.join("sandbox-exec").is_file() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config(enabled: bool, strategy: &str) -> SandboxConfig {
        SandboxConfig {
            enabled,
            strategy: strategy.to_string(),
            allowed_write_paths: vec![],
            forbidden_paths: vec![],
            allow_network: false,
        }
    }

    #[test]
    fn pick_strategy_none_is_noop() {
        assert_eq!(pick_strategy("none").name(), "noop");
    }

    #[test]
    fn pick_strategy_empty_is_auto() {
        // Empty string should behave the same as "auto".
        assert_eq!(pick_strategy("").name(), auto_detect().name());
    }

    #[test]
    fn pick_strategy_auto_matches_auto_detect() {
        assert_eq!(pick_strategy("auto").name(), auto_detect().name());
    }

    #[test]
    fn pick_strategy_unknown_is_noop() {
        assert_eq!(pick_strategy("martian").name(), "noop");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn auto_detect_off_macos_is_noop() {
        assert_eq!(auto_detect().name(), "noop");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn auto_detect_on_macos_picks_seatbelt() {
        // macOS CI and dev machines always have sandbox-exec on $PATH.
        assert_eq!(auto_detect().name(), "seatbelt");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn pick_strategy_seatbelt_matches_make_seatbelt() {
        assert_eq!(pick_strategy("seatbelt").name(), "seatbelt");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn pick_strategy_seatbelt_off_macos_is_noop() {
        // Selecting seatbelt on Linux should silently degrade to noop
        // rather than crashing at config-load time.
        assert_eq!(pick_strategy("seatbelt").name(), "noop");
    }

    #[test]
    fn disabled_executor_is_inactive() {
        let exec = SandboxExecutor::disabled();
        assert!(!exec.is_active());
        assert_eq!(exec.strategy_name(), "noop");
        assert!(exec.allow_bypass());
    }

    #[test]
    fn disabled_executor_wrap_is_identity_program() {
        // We cannot compare Commands directly, but we can verify that
        // wrapping an echo command still targets echo (not sandbox-exec)
        // when the executor is disabled.
        let exec = SandboxExecutor::disabled();
        let cmd = Command::new("echo");
        let wrapped = exec.wrap(cmd);
        let program = wrapped.as_std().get_program();
        assert_eq!(program, "echo");
    }

    #[test]
    fn from_config_disabled_is_inactive_regardless_of_strategy() {
        let cfg = sample_config(false, "seatbelt");
        let exec = SandboxExecutor::from_config(&cfg, std::path::Path::new("/tmp"));
        assert!(!exec.is_active());
    }

    #[test]
    fn from_config_strategy_none_is_inactive_even_when_enabled() {
        let cfg = sample_config(true, "none");
        let exec = SandboxExecutor::from_config(&cfg, std::path::Path::new("/tmp"));
        assert!(!exec.is_active());
        assert_eq!(exec.strategy_name(), "noop");
    }

    #[test]
    fn from_config_policy_contains_project_dir() {
        let cfg = sample_config(false, "auto");
        let exec = SandboxExecutor::from_config(&cfg, std::path::Path::new("/work/repo"));
        assert_eq!(exec.policy().project_dir, PathBuf::from("/work/repo"));
    }

    #[test]
    fn from_config_with_bypass_respects_flag() {
        let cfg = sample_config(true, "auto");
        let allowed =
            SandboxExecutor::from_config_with_bypass(&cfg, std::path::Path::new("/tmp"), true);
        let denied =
            SandboxExecutor::from_config_with_bypass(&cfg, std::path::Path::new("/tmp"), false);
        assert!(allowed.allow_bypass());
        assert!(!denied.allow_bypass());
    }

    #[test]
    fn from_session_config_honors_disable_bypass_permissions() {
        let base = crate::config::Config {
            sandbox: sample_config(true, "none"),
            ..Default::default()
        };

        let mut denied_cfg = base.clone();
        denied_cfg.security.disable_bypass_permissions = true;
        let denied =
            SandboxExecutor::from_session_config(&denied_cfg, std::path::Path::new("/tmp"));
        assert!(!denied.allow_bypass());

        let mut allowed_cfg = base;
        allowed_cfg.security.disable_bypass_permissions = false;
        let allowed =
            SandboxExecutor::from_session_config(&allowed_cfg, std::path::Path::new("/tmp"));
        assert!(allowed.allow_bypass());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn active_seatbelt_wrap_replaces_program() {
        let cfg = sample_config(true, "seatbelt");
        let exec = SandboxExecutor::from_config(&cfg, std::path::Path::new("/tmp"));
        if !exec.is_active() {
            eprintln!("skipping: seatbelt unavailable");
            return;
        }
        let wrapped = exec.wrap(Command::new("echo"));
        let std_cmd = wrapped.as_std();
        assert_eq!(std_cmd.get_program(), "sandbox-exec");
        // After `-p <profile>`, the original program is argv[3].
        let args: Vec<_> = std_cmd.get_args().collect();
        assert_eq!(args.first().map(|a| a.to_str().unwrap()), Some("-p"));
        // "echo" appears after the profile string.
        assert!(args.iter().any(|a| a.to_str() == Some("echo")));
    }

    #[test]
    fn noop_strategy_returns_command_untouched() {
        // Guard against accidental NoopStrategy behavior changes.
        let cmd = Command::new("cat");
        let policy = SandboxPolicy {
            project_dir: PathBuf::from("/tmp"),
            allowed_write_paths: vec![],
            forbidden_paths: vec![],
            allow_network: false,
        };
        let wrapped = NoopStrategy.wrap_command(cmd, &policy);
        assert_eq!(wrapped.as_std().get_program(), "cat");
    }
}
