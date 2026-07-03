//! Startup process hardening.
//!
//! The agent holds API keys and other secrets in memory. A few cheap,
//! platform-level steps close common local vectors:
//!
//! - **Library injection** — clear `LD_PRELOAD` / `LD_AUDIT` (Linux) and the
//!   `DYLD_*` insertion variables (macOS) so a hostile environment can't load
//!   arbitrary code into the process.
//! - **Core dumps** — set `RLIMIT_CORE` to 0 so a crash never writes memory
//!   (which may contain secrets) to disk.
//! - **Ptrace attach** — on Linux, mark the process non-dumpable
//!   (`PR_SET_DUMPABLE = 0`) so another local process of the same user cannot
//!   attach a debugger and read memory.
//!
//! Call [`harden_process`] once, as early as possible in `main`, before any
//! secret is loaded. Set `AGENT_CODE_DISABLE_HARDENING=1` to skip it (e.g. for
//! debugging with a preloaded allocator or a profiler).

/// Apply best-effort process hardening. No-op on unsupported platforms and
/// when `AGENT_CODE_DISABLE_HARDENING` is set to a truthy value.
pub fn harden_process() {
    if disabled_by_env() {
        return;
    }
    clear_injection_env();
    #[cfg(unix)]
    unix::apply();
}

fn disabled_by_env() -> bool {
    matches!(
        std::env::var("AGENT_CODE_DISABLE_HARDENING")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Remove environment variables that can inject code into this process or its
/// children. Safe on every platform (a missing var is simply ignored).
fn clear_injection_env() {
    const INJECTION_VARS: &[&str] = &[
        // Linux/glibc dynamic-linker hooks.
        "LD_PRELOAD",
        "LD_AUDIT",
        // macOS dyld insertion hooks.
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "DYLD_FRAMEWORK_PATH",
    ];
    for var in INJECTION_VARS {
        if std::env::var_os(var).is_some() {
            // SAFETY: called at the very start of `main`, before any threads
            // are spawned, so there is no concurrent env access.
            unsafe { std::env::remove_var(var) };
        }
    }
}

#[cfg(unix)]
mod unix {
    /// Disable core dumps and, on Linux, block ptrace-attach.
    pub fn apply() {
        disable_core_dumps();
        #[cfg(target_os = "linux")]
        set_non_dumpable();
    }

    fn disable_core_dumps() {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` is a valid, fully-initialized rlimit; the call only
        // reads it. Failure is non-fatal and intentionally ignored.
        unsafe {
            libc::setrlimit(libc::RLIMIT_CORE, &limit);
        }
    }

    #[cfg(target_os = "linux")]
    fn set_non_dumpable() {
        // SAFETY: prctl with PR_SET_DUMPABLE takes an int arg; 0 marks the
        // process non-dumpable. Failure is non-fatal and ignored.
        unsafe {
            libc::prctl(libc::PR_SET_DUMPABLE, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harden_process_is_safe_to_call() {
        // Smoke test: hardening must never panic or abort the process.
        harden_process();
    }
}
