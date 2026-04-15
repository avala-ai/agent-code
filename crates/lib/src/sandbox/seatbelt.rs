//! macOS sandbox strategy using `sandbox-exec` (Seatbelt).
//!
//! Builds an inline SBPL profile that denies everything by default, allows
//! broad reads (so tools can introspect the system), and grants writes only
//! to the project directory plus any explicitly allowed paths. Forbidden
//! read paths are denied after the broad read rule.
//!
//! Note: `sandbox-exec` is documented as deprecated on newer macOS versions
//! but remains functional. A future follow-up will add an Endpoint Security
//! based strategy; this ships today.

use std::path::Path;

use tokio::process::Command;

use super::{SandboxPolicy, SandboxStrategy};

/// macOS Seatbelt strategy. See module docs.
pub struct SeatbeltStrategy;

impl SandboxStrategy for SeatbeltStrategy {
    fn name(&self) -> &'static str {
        "seatbelt"
    }

    fn wrap_command(&self, cmd: Command, policy: &SandboxPolicy) -> Command {
        let profile = build_profile(policy);

        // Extract the program and args from the existing Command. We rebuild
        // via sandbox-exec rather than mutating in place because tokio's
        // Command does not expose its program field for replacement.
        let std_cmd = cmd.as_std();
        let program = std_cmd.get_program().to_os_string();
        let args: Vec<_> = std_cmd.get_args().map(|a| a.to_os_string()).collect();
        let current_dir = std_cmd.get_current_dir().map(Path::to_path_buf);
        let envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = std_cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();

        let mut wrapped = Command::new("sandbox-exec");
        wrapped.arg("-p").arg(profile);
        wrapped.arg(program);
        wrapped.args(args);
        if let Some(dir) = current_dir {
            wrapped.current_dir(dir);
        }
        for (k, v) in envs {
            match v {
                Some(val) => {
                    wrapped.env(k, val);
                }
                None => {
                    wrapped.env_remove(k);
                }
            }
        }
        // Preserve piped stdio from the original command — tokio does not
        // expose a getter, so the caller must re-apply stdio configuration
        // after wrapping. See [`super::wrap_with_sandbox`] for the helper
        // that handles this.
        wrapped
    }
}

/// Build an SBPL profile string for the given policy.
///
/// The generated profile:
/// - denies all operations by default
/// - imports `system.sb` for minimal OS compatibility
/// - allows `process-fork`, `process-exec*`, `signal`, and `sysctl-read`
///   so wrapped shells can run commands and inspect basic system state
/// - allows reads everywhere (tools need to see the filesystem)
/// - allows writes to the project directory and each allowed-write path
/// - denies reads to every forbidden path (overriding the broad read rule)
/// - conditionally allows network
pub(super) fn build_profile(policy: &SandboxPolicy) -> String {
    let mut profile = String::new();
    profile.push_str("(version 1)\n");
    profile.push_str("(deny default)\n");
    profile.push_str("(import \"system.sb\")\n");
    profile.push_str("(allow process-fork)\n");
    profile.push_str("(allow process-exec*)\n");
    profile.push_str("(allow signal)\n");
    profile.push_str("(allow sysctl-read)\n");
    profile.push_str("(allow file-read*)\n");

    // Writable: project dir first, then explicit allow list.
    push_subpath_allow(&mut profile, &policy.project_dir);
    for p in &policy.allowed_write_paths {
        push_subpath_allow(&mut profile, p);
    }

    // Forbidden reads override the broad allow above.
    for p in &policy.forbidden_paths {
        push_subpath_deny_read(&mut profile, p);
    }

    if policy.allow_network {
        profile.push_str("(allow network*)\n");
    }

    profile
}

fn push_subpath_allow(profile: &mut String, path: &Path) {
    for variant in path_variants(path) {
        let escaped = escape_sbpl(&variant.display().to_string());
        profile.push_str(&format!("(allow file-write* (subpath \"{escaped}\"))\n"));
    }
}

fn push_subpath_deny_read(profile: &mut String, path: &Path) {
    for variant in path_variants(path) {
        let escaped = escape_sbpl(&variant.display().to_string());
        profile.push_str(&format!("(deny file-read* (subpath \"{escaped}\"))\n"));
    }
}

/// Return both the raw path and its canonicalized form (if different).
///
/// On macOS, common paths like `/tmp` and `/var/folders/...` are symlinks
/// into `/private/...`, and Seatbelt `subpath` rules match the real path,
/// not the symlink. Emitting both forms makes the profile behave correctly
/// regardless of which form a child process happens to open.
fn path_variants(path: &Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![path.to_path_buf()];
    if let Ok(canonical) = std::fs::canonicalize(path)
        && canonical != path
    {
        out.push(canonical);
    }
    out
}

fn escape_sbpl(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_policy() -> SandboxPolicy {
        SandboxPolicy {
            project_dir: PathBuf::from("/work/repo"),
            allowed_write_paths: vec![
                PathBuf::from("/tmp"),
                PathBuf::from("/Users/test/.cache/agent-code"),
            ],
            forbidden_paths: vec![PathBuf::from("/Users/test/.ssh")],
            allow_network: false,
        }
    }

    #[test]
    fn profile_denies_by_default() {
        let p = build_profile(&test_policy());
        assert!(p.contains("(deny default)"));
    }

    #[test]
    fn profile_allows_reads_broadly() {
        let p = build_profile(&test_policy());
        assert!(p.contains("(allow file-read*)"));
    }

    #[test]
    fn profile_allows_project_writes() {
        let p = build_profile(&test_policy());
        assert!(p.contains("(allow file-write* (subpath \"/work/repo\"))"));
    }

    #[test]
    fn profile_allows_extra_write_paths() {
        let p = build_profile(&test_policy());
        assert!(p.contains("(allow file-write* (subpath \"/tmp\"))"));
        assert!(p.contains("(allow file-write* (subpath \"/Users/test/.cache/agent-code\"))"));
    }

    #[test]
    fn profile_denies_forbidden_paths() {
        let p = build_profile(&test_policy());
        assert!(p.contains("(deny file-read* (subpath \"/Users/test/.ssh\"))"));
    }

    #[test]
    fn profile_skips_network_when_disabled() {
        let p = build_profile(&test_policy());
        assert!(!p.contains("network*"));
    }

    #[test]
    fn profile_allows_network_when_enabled() {
        let mut policy = test_policy();
        policy.allow_network = true;
        let p = build_profile(&policy);
        assert!(p.contains("(allow network*)"));
    }

    #[test]
    fn profile_escapes_double_quotes_in_paths() {
        let policy = SandboxPolicy {
            project_dir: PathBuf::from("/weird\"path"),
            allowed_write_paths: vec![],
            forbidden_paths: vec![],
            allow_network: false,
        };
        let p = build_profile(&policy);
        assert!(p.contains("\"/weird\\\"path\""));
    }

    #[test]
    fn profile_empty_allow_and_forbid_lists_still_builds() {
        let policy = SandboxPolicy {
            project_dir: PathBuf::from("/work/repo"),
            allowed_write_paths: vec![],
            forbidden_paths: vec![],
            allow_network: false,
        };
        let p = build_profile(&policy);
        // Must still allow project-dir writes even with empty extra lists.
        assert!(p.contains("(allow file-write* (subpath \"/work/repo\"))"));
        // And still deny default.
        assert!(p.contains("(deny default)"));
    }

    #[test]
    fn profile_multiple_forbidden_paths_all_appear() {
        let policy = SandboxPolicy {
            project_dir: PathBuf::from("/work/repo"),
            allowed_write_paths: vec![],
            forbidden_paths: vec![
                PathBuf::from("/Users/test/.ssh"),
                PathBuf::from("/Users/test/.aws"),
                PathBuf::from("/Users/test/.gnupg"),
            ],
            allow_network: false,
        };
        let p = build_profile(&policy);
        assert!(p.contains("/Users/test/.ssh"));
        assert!(p.contains("/Users/test/.aws"));
        assert!(p.contains("/Users/test/.gnupg"));
    }

    #[test]
    fn profile_contains_process_and_signal_allows() {
        // Regression guard: subprocess execution inside the sandbox
        // requires these rules to be present. Breaking them would make
        // bash unable to fork/exec/signal its children.
        let p = build_profile(&test_policy());
        assert!(p.contains("(allow process-fork)"));
        assert!(p.contains("(allow process-exec*)"));
        assert!(p.contains("(allow signal)"));
    }

    #[test]
    fn profile_imports_system_sb() {
        let p = build_profile(&test_policy());
        assert!(p.contains("(import \"system.sb\")"));
    }

    // ──────────────────────────────────────────────────────────────
    //  SeatbeltStrategy argv / cwd / env preservation
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn wrap_command_sets_sandbox_exec_as_program() {
        let policy = test_policy();
        let cmd = Command::new("bash");
        let wrapped = SeatbeltStrategy.wrap_command(cmd, &policy);
        assert_eq!(wrapped.as_std().get_program(), "sandbox-exec");
    }

    #[test]
    fn wrap_command_prepends_profile_flag() {
        let policy = test_policy();
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg("echo hi");
        let wrapped = SeatbeltStrategy.wrap_command(cmd, &policy);
        let args: Vec<_> = wrapped
            .as_std()
            .get_args()
            .map(|a| a.to_os_string())
            .collect();
        // argv: -p, <profile>, bash, -c, "echo hi"
        assert_eq!(args[0], "-p");
        assert!(args[1].to_str().unwrap().contains("(deny default)"));
        assert_eq!(args[2], "bash");
        assert_eq!(args[3], "-c");
        assert_eq!(args[4], "echo hi");
    }

    #[test]
    fn wrap_command_preserves_current_dir() {
        let policy = test_policy();
        let mut cmd = Command::new("bash");
        cmd.current_dir("/work/repo");
        let wrapped = SeatbeltStrategy.wrap_command(cmd, &policy);
        assert_eq!(
            wrapped.as_std().get_current_dir(),
            Some(std::path::Path::new("/work/repo"))
        );
    }

    #[test]
    fn wrap_command_preserves_env_vars() {
        let policy = test_policy();
        let mut cmd = Command::new("bash");
        cmd.env("MY_VAR", "hello").env("OTHER_VAR", "world");
        let wrapped = SeatbeltStrategy.wrap_command(cmd, &policy);
        let envs: std::collections::HashMap<_, _> = wrapped
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();
        assert_eq!(
            envs.get(&std::ffi::OsString::from("MY_VAR"))
                .and_then(|v| v.clone()),
            Some("hello".into())
        );
        assert_eq!(
            envs.get(&std::ffi::OsString::from("OTHER_VAR"))
                .and_then(|v| v.clone()),
            Some("world".into())
        );
    }

    #[test]
    fn wrap_command_preserves_env_removals() {
        let policy = test_policy();
        let mut cmd = Command::new("bash");
        cmd.env_remove("SECRET");
        let wrapped = SeatbeltStrategy.wrap_command(cmd, &policy);
        let envs: std::collections::HashMap<_, _> = wrapped
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();
        // env_remove stores a None value under the key.
        assert_eq!(envs.get(&std::ffi::OsString::from("SECRET")), Some(&None));
    }
}
