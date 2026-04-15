//! Linux sandbox strategy using `bwrap` (bubblewrap).
//!
//! Builds an argv that namespace-isolates the child process:
//! - user, IPC, UTS, PID, cgroup namespaces are always unshared
//! - network namespace is unshared only when `allow_network` is false
//! - the host filesystem is bind-mounted read-only at `/`
//! - `/dev` and `/proc` are overlaid with clean views
//! - the project directory and every `allowed_write_paths` entry are
//!   rw-bind-mounted on top of the read-only base
//! - `--die-with-parent` ensures the sandbox dies with the agent
//!
//! Forbidden-path masking (`~/.ssh` etc.) is deferred to a follow-up
//! PR — bwrap does not support the seatbelt `subpath` deny model
//! directly and needs per-file handling. For now, forbidden paths are
//! logged but not enforced by this strategy; callers that need secret
//! masking should rely on the in-process permission system until the
//! follow-up lands.

use std::ffi::OsString;
use std::path::Path;

use tokio::process::Command;

use super::{SandboxPolicy, SandboxStrategy};

/// Linux bubblewrap strategy. See module docs.
pub struct BwrapStrategy;

impl SandboxStrategy for BwrapStrategy {
    fn name(&self) -> &'static str {
        "bwrap"
    }

    fn wrap_command(&self, cmd: Command, policy: &SandboxPolicy) -> Command {
        let std_cmd = cmd.as_std();
        let program = std_cmd.get_program().to_os_string();
        let args: Vec<OsString> = std_cmd.get_args().map(|a| a.to_os_string()).collect();
        let current_dir = std_cmd.get_current_dir().map(Path::to_path_buf);
        let envs: Vec<(OsString, Option<OsString>)> = std_cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();

        let bwrap_args = build_args(policy, current_dir.as_deref(), &program, &args);

        let mut wrapped = Command::new("bwrap");
        wrapped.args(bwrap_args);
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
        wrapped
    }
}

/// Build the argv passed to `bwrap` (everything after the `bwrap` program).
///
/// Extracted so unit tests can assert the exact shape without spawning.
pub(super) fn build_args(
    policy: &SandboxPolicy,
    chdir: Option<&Path>,
    program: &OsString,
    program_args: &[OsString],
) -> Vec<OsString> {
    let mut out: Vec<OsString> = Vec::new();

    // Namespace isolation. We deliberately do NOT unshare the network by
    // default — most coding-agent bash calls need outbound HTTPS for git,
    // package managers, and network-aware tools. Users who need strict
    // isolation set `allow_network = false` in config.
    push_flag(&mut out, "--unshare-user");
    push_flag(&mut out, "--unshare-ipc");
    push_flag(&mut out, "--unshare-uts");
    push_flag(&mut out, "--unshare-pid");
    push_flag(&mut out, "--unshare-cgroup");
    if !policy.allow_network {
        push_flag(&mut out, "--unshare-net");
    }

    // Die with parent — without this, a bwrap-wrapped child can outlive
    // the agent if the agent crashes mid-turn.
    push_flag(&mut out, "--die-with-parent");

    // Read-only view of the entire host filesystem. Subsequent --bind and
    // overlay operations shadow specific paths on top of this.
    push_flag(&mut out, "--ro-bind");
    push_path(&mut out, Path::new("/"));
    push_path(&mut out, Path::new("/"));

    // Clean /dev and /proc overlays — the ro-bind above would otherwise
    // expose the host's kernel-managed filesystems as they were at snapshot.
    push_flag(&mut out, "--dev");
    push_path(&mut out, Path::new("/dev"));
    push_flag(&mut out, "--proc");
    push_path(&mut out, Path::new("/proc"));

    // Read-write mount for the project directory. This is the only
    // writable location by default.
    bind_rw(&mut out, &policy.project_dir);
    for p in &policy.allowed_write_paths {
        bind_rw(&mut out, p);
    }

    // Working directory inside the namespace.
    if let Some(dir) = chdir {
        push_flag(&mut out, "--chdir");
        push_path(&mut out, dir);
    }

    // `--` separates bwrap's flags from the program to exec. Without it,
    // bwrap would try to interpret the program as one of its own flags.
    push_flag(&mut out, "--");
    out.push(program.clone());
    out.extend(program_args.iter().cloned());

    out
}

fn bind_rw(out: &mut Vec<OsString>, path: &Path) {
    // `--bind` is rw by default in bwrap. We emit both the raw path and
    // its canonicalized form when they differ, mirroring the seatbelt
    // strategy's symlink handling — on Linux, /var/run -> /run and
    // similar resolutions can otherwise leave the child unable to write
    // to its own working directory.
    push_flag(out, "--bind");
    push_path(out, path);
    push_path(out, path);
    if let Ok(canonical) = std::fs::canonicalize(path)
        && canonical != path
    {
        push_flag(out, "--bind");
        push_path(out, &canonical);
        push_path(out, &canonical);
    }
}

fn push_flag(out: &mut Vec<OsString>, flag: &str) {
    out.push(OsString::from(flag));
}

fn push_path(out: &mut Vec<OsString>, path: &Path) {
    out.push(OsString::from(path));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_policy() -> SandboxPolicy {
        SandboxPolicy {
            project_dir: PathBuf::from("/work/repo"),
            allowed_write_paths: vec![
                PathBuf::from("/tmp/agent-cache"),
                PathBuf::from("/var/build-output"),
            ],
            forbidden_paths: vec![],
            allow_network: false,
        }
    }

    fn args_with(policy: &SandboxPolicy, chdir: Option<&Path>) -> Vec<String> {
        build_args(
            policy,
            chdir,
            &OsString::from("bash"),
            &[OsString::from("-c"), OsString::from("echo hi")],
        )
        .into_iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
    }

    fn contains_sequence(haystack: &[String], needle: &[&str]) -> bool {
        haystack
            .windows(needle.len())
            .any(|w| w.iter().map(String::as_str).eq(needle.iter().copied()))
    }

    #[test]
    fn argv_unshares_standard_namespaces() {
        let policy = test_policy();
        let args = args_with(&policy, None);
        for ns in [
            "--unshare-user",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-pid",
            "--unshare-cgroup",
        ] {
            assert!(
                args.iter().any(|a| a == ns),
                "expected {ns} in argv: {args:?}"
            );
        }
    }

    #[test]
    fn argv_unshares_network_only_when_denied() {
        let mut policy = test_policy();
        policy.allow_network = false;
        let denied = args_with(&policy, None);
        assert!(denied.iter().any(|a| a == "--unshare-net"));

        policy.allow_network = true;
        let allowed = args_with(&policy, None);
        assert!(!allowed.iter().any(|a| a == "--unshare-net"));
    }

    #[test]
    fn argv_mounts_root_read_only() {
        let args = args_with(&test_policy(), None);
        assert!(contains_sequence(&args, &["--ro-bind", "/", "/"]));
    }

    #[test]
    fn argv_overlays_dev_and_proc() {
        let args = args_with(&test_policy(), None);
        assert!(contains_sequence(&args, &["--dev", "/dev"]));
        assert!(contains_sequence(&args, &["--proc", "/proc"]));
    }

    #[test]
    fn argv_rw_binds_project_dir() {
        let args = args_with(&test_policy(), None);
        assert!(contains_sequence(
            &args,
            &["--bind", "/work/repo", "/work/repo"]
        ));
    }

    #[test]
    fn argv_rw_binds_allowed_paths() {
        let args = args_with(&test_policy(), None);
        assert!(contains_sequence(
            &args,
            &["--bind", "/tmp/agent-cache", "/tmp/agent-cache"]
        ));
        assert!(contains_sequence(
            &args,
            &["--bind", "/var/build-output", "/var/build-output"]
        ));
    }

    #[test]
    fn argv_sets_die_with_parent() {
        let args = args_with(&test_policy(), None);
        assert!(args.iter().any(|a| a == "--die-with-parent"));
    }

    #[test]
    fn argv_passes_chdir_when_provided() {
        let args = args_with(&test_policy(), Some(Path::new("/work/repo")));
        assert!(contains_sequence(&args, &["--chdir", "/work/repo"]));
    }

    #[test]
    fn argv_terminates_with_double_dash_and_program() {
        let args = args_with(&test_policy(), None);
        // Everything after `--` is the program and its args.
        let dash_idx = args
            .iter()
            .position(|a| a == "--")
            .expect("argv must contain `--`");
        assert_eq!(args[dash_idx + 1], "bash");
        assert_eq!(args[dash_idx + 2], "-c");
        assert_eq!(args[dash_idx + 3], "echo hi");
    }

    #[test]
    fn argv_handles_empty_allow_and_forbid_lists() {
        let policy = SandboxPolicy {
            project_dir: PathBuf::from("/work/repo"),
            allowed_write_paths: vec![],
            forbidden_paths: vec![],
            allow_network: false,
        };
        let args = args_with(&policy, None);
        // Project-dir bind must still be present.
        assert!(contains_sequence(
            &args,
            &["--bind", "/work/repo", "/work/repo"]
        ));
    }

    #[test]
    fn strategy_name_is_bwrap() {
        assert_eq!(BwrapStrategy.name(), "bwrap");
    }

    #[test]
    fn wrap_command_sets_bwrap_as_program() {
        let policy = test_policy();
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg("echo hi");
        let wrapped = BwrapStrategy.wrap_command(cmd, &policy);
        assert_eq!(wrapped.as_std().get_program(), "bwrap");
    }

    #[test]
    fn wrap_command_preserves_current_dir() {
        let policy = test_policy();
        let mut cmd = Command::new("bash");
        cmd.current_dir("/work/repo");
        let wrapped = BwrapStrategy.wrap_command(cmd, &policy);
        assert_eq!(
            wrapped.as_std().get_current_dir(),
            Some(Path::new("/work/repo"))
        );
    }

    #[test]
    fn wrap_command_preserves_env_vars() {
        let policy = test_policy();
        let mut cmd = Command::new("bash");
        cmd.env("MY_VAR", "hello").env_remove("SECRET");
        let wrapped = BwrapStrategy.wrap_command(cmd, &policy);
        let envs: std::collections::HashMap<_, _> = wrapped
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();
        assert_eq!(
            envs.get(&OsString::from("MY_VAR")).and_then(|v| v.clone()),
            Some(OsString::from("hello"))
        );
        assert_eq!(envs.get(&OsString::from("SECRET")), Some(&None));
    }
}
