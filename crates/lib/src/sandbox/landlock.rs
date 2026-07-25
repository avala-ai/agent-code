//! Native Linux sandbox strategy using Landlock + seccomp.
//!
//! Unlike [`super::bwrap::BwrapStrategy`], this strategy needs no external
//! helper binary: it confines the child in-process via two kernel LSM
//! features that any unprivileged process can apply to itself.
//!
//! - **Filesystem confinement (Landlock).** The child gets read+execute on
//!   the whole filesystem (mirroring bwrap's read-only bind of `/`) but may
//!   only write inside the project directory, each `allowed_write_paths`
//!   entry, and a minimal device set (`/dev/null`, `/dev/tty`). Any write
//!   outside that set is denied by the kernel.
//! - **Network kill (seccomp).** When the policy denies networking, a
//!   seccomp-BPF filter makes `socket(2)` return `EPERM` for the `AF_INET`
//!   and `AF_INET6` domains while leaving `AF_UNIX` (and every other
//!   syscall) untouched, so local IPC and normal tooling keep working.
//!
//! # How it is applied
//!
//! The Landlock ruleset and the compiled BPF program are built in the parent
//! (inside [`LandlockStrategy::wrap_command`]) so that the post-fork
//! [`pre_exec`](std::os::unix::process::CommandExt::pre_exec) closure only
//! has to issue the two enforcement syscalls (`restrict_self()` and
//! `apply_filter`) — no heap allocation happens between `fork` and `exec`.
//! The restrictions persist across the following `execve`, so the executed
//! program runs already confined.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreated, RulesetCreatedAttr, RulesetError,
};
use seccompiler::{
    BackendError, BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition,
    SeccompFilter, SeccompRule, TargetArch,
};
use tokio::process::Command;
use tracing::warn;

use super::{SandboxPolicy, SandboxStrategy};

/// Landlock ABI we build rules against. V1 (Linux 5.13) is the lowest common
/// denominator and covers the read/write/execute rights we need; combined
/// with [`CompatLevel::BestEffort`] the same code degrades gracefully on
/// kernels that only support an even older/narrower feature set.
const RULESET_ABI: ABI = ABI::V1;

/// Native Linux Landlock + seccomp strategy. See module docs.
pub struct LandlockStrategy;

impl SandboxStrategy for LandlockStrategy {
    fn name(&self) -> &'static str {
        "landlock"
    }

    fn wrap_command(&self, cmd: Command, policy: &SandboxPolicy) -> Command {
        // Pull program/args/cwd/env out of the incoming command. tokio's
        // Command doesn't allow mutating the program in place, so — like
        // bwrap.rs — we read the parts and rebuild a fresh std Command that
        // carries the pre_exec hook.
        let std_cmd = cmd.as_std();
        let program = std_cmd.get_program().to_os_string();
        let args: Vec<OsString> = std_cmd.get_args().map(|a| a.to_os_string()).collect();
        let current_dir = std_cmd.get_current_dir().map(Path::to_path_buf);
        let envs: Vec<(OsString, Option<OsString>)> = std_cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();

        // Writable set: project dir + configured writable paths + a minimal
        // device set that normal tooling needs (redirects to /dev/null, a
        // controlling terminal at /dev/tty).
        let mut write_paths: Vec<PathBuf> =
            Vec::with_capacity(policy.allowed_write_paths.len() + 3);
        write_paths.push(policy.project_dir.clone());
        write_paths.extend(policy.allowed_write_paths.iter().cloned());
        write_paths.push(PathBuf::from("/dev/null"));
        write_paths.push(PathBuf::from("/dev/tty"));

        // Build the Landlock ruleset in the PARENT. `create()` allocates the
        // ruleset fd and every rule fd here; the child only calls
        // `restrict_self()`.
        let ruleset = match build_ruleset(&write_paths) {
            Ok(rs) => Some(rs),
            Err(e) => {
                warn!(
                    "landlock: failed to build filesystem ruleset ({e}); \
                       running this command without filesystem confinement"
                );
                None
            }
        };

        // Compile the seccomp network-kill filter in the PARENT (only when
        // the policy denies networking). The child merely loads it.
        let seccomp = if policy.allow_network {
            None
        } else {
            match build_network_kill_filter() {
                Ok(prog) => Some(prog),
                Err(e) => {
                    warn!(
                        "landlock: failed to compile seccomp network filter ({e}); \
                           network access will NOT be blocked for this command"
                    );
                    None
                }
            }
        };

        let mut builder = std::process::Command::new(&program);
        builder.args(&args);
        if let Some(dir) = &current_dir {
            builder.current_dir(dir);
        }
        for (k, v) in &envs {
            match v {
                Some(val) => {
                    builder.env(k, val);
                }
                None => {
                    builder.env_remove(k);
                }
            }
        }

        // Move the prepared ruleset + BPF program into the child hook. The
        // ruleset is consumed by `restrict_self`, so it lives in an Option we
        // `take()` (the FnMut bound forbids moving a captured value out
        // directly).
        let mut ruleset = ruleset;
        // SAFETY: the closure runs in the forked child after `fork(2)` and
        // before `execve(2)`. The child is single-threaded at that point, so
        // the two enforcement syscalls are async-signal-safe to issue here;
        // no allocation is performed because the ruleset and BPF program were
        // built in the parent.
        unsafe {
            builder.pre_exec(move || {
                if let Some(rs) = ruleset.take() {
                    rs.restrict_self().map_err(|e| {
                        io::Error::other(format!("landlock restrict_self failed: {e}"))
                    })?;
                }
                if let Some(prog) = &seccomp {
                    seccompiler::apply_filter(prog).map_err(|e| {
                        io::Error::other(format!("seccomp apply_filter failed: {e}"))
                    })?;
                }
                Ok(())
            });
        }

        Command::from(builder)
    }
}

/// Build a Landlock ruleset that allows read+execute everywhere and write
/// only under `write_paths`.
///
/// Landlock grants an access to a file if *any* enclosing rule allows it, so
/// the broad read rule on `/` composes with the narrow write rules: a file
/// under the project dir is readable (via `/`) and writable (via its own
/// rule), while a file elsewhere is readable but not writable.
///
/// The ruleset is created here (in the parent) but NOT enforced — enforcement
/// happens when the child calls [`RulesetCreated::restrict_self`].
fn build_ruleset(write_paths: &[PathBuf]) -> Result<RulesetCreated, RulesetError> {
    let mut created = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        // Handle every filesystem access right so anything not explicitly
        // granted below is denied.
        .handle_access(AccessFs::from_all(RULESET_ABI))?
        .create()?;

    // Read + execute across the entire filesystem (mirrors bwrap's ro-bind of
    // `/`). `from_read` covers Execute + ReadFile + ReadDir.
    if let Ok(root) = PathFd::new("/") {
        created = created.add_rule(PathBeneath::new(root, AccessFs::from_read(RULESET_ABI)))?;
    }

    // Read + write + execute on each writable path. Paths that cannot be
    // opened (e.g. a configured path that doesn't exist on this host, or a
    // missing /dev/tty) are skipped best-effort rather than failing the whole
    // sandbox.
    for path in write_paths {
        match PathFd::new(path) {
            Ok(fd) => {
                created =
                    created.add_rule(PathBeneath::new(fd, AccessFs::from_all(RULESET_ABI)))?;
            }
            Err(_) => {
                // Not fatal: nothing to grant on a path that isn't there.
            }
        }
    }

    Ok(created)
}

/// Compile a seccomp-BPF program that denies `socket(2)` for the `AF_INET`
/// and `AF_INET6` domains (returning `EPERM`) while allowing every other
/// syscall — including `socket(AF_UNIX, ...)` for local IPC.
///
/// The filter is *surgical*: it inspects `socket`'s first argument (the
/// address family / domain) rather than blocking the syscall wholesale, so
/// Unix-domain sockets, `socketpair`, and unrelated syscalls keep working.
fn build_network_kill_filter() -> Result<BpfProgram, BackendError> {
    // socket(int domain, int type, int protocol) — `domain` is arg 0, a
    // 32-bit int.
    let deny_inet = SeccompRule::new(vec![SeccompCondition::new(
        0,
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::Eq,
        libc::AF_INET as u64,
    )?])?;
    let deny_inet6 = SeccompRule::new(vec![SeccompCondition::new(
        0,
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::Eq,
        libc::AF_INET6 as u64,
    )?])?;

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    // The two rules are ORed: a socket() call whose domain is AF_INET or
    // AF_INET6 matches and gets EPERM; any other domain falls through to the
    // mismatch action (Allow).
    rules.insert(libc::SYS_socket, vec![deny_inet, deny_inet6]);

    let filter = SeccompFilter::new(
        rules,
        // Default (mismatch) action: allow. Non-socket syscalls and
        // socket() calls for other domains (e.g. AF_UNIX) are permitted.
        SeccompAction::Allow,
        // On-match action: refuse INET/INET6 socket creation with EPERM.
        SeccompAction::Errno(libc::EPERM as u32),
        target_arch()?,
    )?;

    filter.try_into()
}

/// Resolve the running architecture into a seccompiler [`TargetArch`].
///
/// Errors (with [`BackendError::InvalidTargetArch`]) on architectures
/// seccompiler doesn't support, which the caller treats as "no network
/// filter" rather than a hard failure.
fn target_arch() -> Result<TargetArch, BackendError> {
    TargetArch::try_from(std::env::consts::ARCH)
}

/// Probe whether Landlock (ABI ≥ v1) is enforceable on the running kernel.
///
/// Returns `false` when the kernel lacks Landlock or has it disabled. This
/// only *creates* a ruleset (which requires no privileges and does not
/// restrict the calling process) with [`CompatLevel::HardRequirement`], so a
/// success means the V1 feature set is actually available — not merely
/// silently downgraded to a no-op.
pub fn landlock_available() -> bool {
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(RULESET_ABI))
        .and_then(|r| r.create())
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_name_is_landlock() {
        assert_eq!(LandlockStrategy.name(), "landlock");
    }

    #[test]
    fn network_kill_filter_compiles() {
        // The BPF program must compile on any supported arch; on an
        // unsupported arch we accept a clean error instead of a panic.
        match build_network_kill_filter() {
            Ok(prog) => assert!(!prog.is_empty(), "compiled filter must be non-empty"),
            Err(e) => eprintln!("skipping: seccomp unavailable on this arch: {e}"),
        }
    }

    #[test]
    fn ruleset_builds_for_project_dir() {
        // Building the ruleset must succeed on a Landlock-capable kernel;
        // skip cleanly where Landlock is unavailable.
        if !landlock_available() {
            eprintln!("skipping: Landlock not available on this kernel");
            return;
        }
        let paths = vec![PathBuf::from("/dev/null")];
        assert!(build_ruleset(&paths).is_ok());
    }

    #[test]
    fn wrap_command_preserves_program_and_env() {
        let policy = SandboxPolicy {
            project_dir: PathBuf::from("/work/repo"),
            allowed_write_paths: vec![],
            forbidden_paths: vec![],
            allow_network: false,
        };
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg("echo hi").env("MY_VAR", "hello");
        let wrapped = LandlockStrategy.wrap_command(cmd, &policy);
        let std_cmd = wrapped.as_std();
        // Program is preserved (not replaced with a helper binary — the
        // confinement is applied via pre_exec, not a wrapper process).
        assert_eq!(std_cmd.get_program(), "bash");
        let args: Vec<_> = std_cmd.get_args().collect();
        assert_eq!(args, ["-c", "echo hi"]);
        let envs: std::collections::HashMap<_, _> = std_cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();
        assert_eq!(
            envs.get(&OsString::from("MY_VAR")).and_then(|v| v.clone()),
            Some(OsString::from("hello"))
        );
    }
}
