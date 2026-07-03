//! Startup process hardening.
//!
//! The agent holds API keys and other secrets in memory. A few cheap,
//! platform-level steps close common local vectors:
//!
//! - **Library injection** — if the process was launched with `LD_PRELOAD`,
//!   `LD_AUDIT`, or `DYLD_INSERT_LIBRARIES`, the dynamic loader has *already*
//!   mapped the injected library in before any of our code runs, so merely
//!   deleting the variable is not enough. On unix we strip those variables and
//!   **re-exec** the binary, giving the loader a clean environment so the
//!   injected library is never mapped into the process that later loads
//!   secrets. Other search-path variables (`DYLD_LIBRARY_PATH`, ...) are
//!   cleared but do not warrant a re-exec.
//! - **Core dumps** — set `RLIMIT_CORE` to 0 so a crash never writes memory
//!   (which may contain secrets) to disk.
//! - **Ptrace attach** — on Linux, mark the process non-dumpable
//!   (`PR_SET_DUMPABLE = 0`) so another local process of the same user cannot
//!   attach a debugger and read memory.
//!
//! [`harden_process`] MUST be called from a synchronous `main`, before the
//! async runtime (or any other threads) start: it mutates the process
//! environment and may re-exec, both of which are only sound while the process
//! is single-threaded. Set `AGENT_CODE_DISABLE_HARDENING=1` to skip it (e.g.
//! for debugging with a preloaded allocator or a profiler).

/// Variables whose presence means arbitrary code was injected by the loader —
/// they justify a re-exec into a clean environment.
const INJECTION_VARS: &[&str] = &["LD_PRELOAD", "LD_AUDIT", "DYLD_INSERT_LIBRARIES"];

/// Additional loader search-path variables that are cleared defensively but do
/// not, on their own, indicate code injection.
const SEARCH_PATH_VARS: &[&str] = &["DYLD_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH"];

/// Sentinel marking a process that already re-exec'd, so we never loop.
const REEXEC_SENTINEL: &str = "AGENT_CODE_HARDENED";

/// Apply best-effort process hardening. No-op on unsupported platforms and
/// when `AGENT_CODE_DISABLE_HARDENING` is set to a truthy value.
///
/// Call from a synchronous `main` before any threads exist — see module docs.
pub fn harden_process() {
    if disabled_by_env() {
        return;
    }
    // Escape an already-mapped injected library first (may not return).
    #[cfg(unix)]
    reexec_without_injection();
    clear_injection_env();
    #[cfg(unix)]
    unix::apply();
}

fn disabled_by_env() -> bool {
    is_truthy(
        std::env::var("AGENT_CODE_DISABLE_HARDENING")
            .ok()
            .as_deref(),
    )
}

fn is_truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1") | Some("true") | Some("yes"))
}

/// Remove code-injection and loader search-path variables from this process's
/// environment. Safe when a variable is absent (it is simply skipped).
fn clear_injection_env() {
    for var in INJECTION_VARS.iter().chain(SEARCH_PATH_VARS) {
        if std::env::var_os(var).is_some() {
            // SAFETY: called from a synchronous `main` before any threads are
            // spawned, so there is no concurrent access to the environment.
            unsafe { std::env::remove_var(var) };
        }
    }
}

/// If the loader injected a library via `LD_PRELOAD`/`LD_AUDIT`/
/// `DYLD_INSERT_LIBRARIES`, strip those variables and re-exec so the child
/// starts without the injected code mapped in. Guarded by a sentinel so it
/// runs at most once. Returns normally when there is nothing to escape or when
/// re-exec fails (best effort — the variables have still been stripped).
#[cfg(unix)]
fn reexec_without_injection() {
    use std::os::unix::process::CommandExt;

    // Already re-exec'd once — proceed without looping.
    if std::env::var_os(REEXEC_SENTINEL).is_some() {
        return;
    }
    let injected = INJECTION_VARS.iter().any(|v| std::env::var_os(v).is_some());
    if !injected {
        return;
    }

    // SAFETY: single-threaded at startup (see module docs). Strip the injection
    // vars so the re-exec'd process inherits a clean environment, and mark it
    // so the fresh process does not re-exec again.
    unsafe {
        for v in INJECTION_VARS {
            std::env::remove_var(v);
        }
        std::env::set_var(REEXEC_SENTINEL, "1");
    }

    let Ok(exe) = std::env::current_exe() else {
        return; // cannot re-exec; vars are already stripped as a fallback
    };
    // `exec` replaces the process image and only returns on failure; on success
    // the loader runs again with the cleaned environment.
    let _ = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec();
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
    fn optout_parsing() {
        assert!(is_truthy(Some("1")));
        assert!(is_truthy(Some("true")));
        assert!(is_truthy(Some("yes")));
        assert!(!is_truthy(Some("0")));
        assert!(!is_truthy(Some("")));
        assert!(!is_truthy(None));
    }

    #[test]
    fn injection_and_search_vars_are_disjoint() {
        for v in INJECTION_VARS {
            assert!(!SEARCH_PATH_VARS.contains(v), "{v} listed twice");
        }
    }

    #[test]
    #[cfg(unix)]
    fn unix_apply_does_not_panic() {
        // The rlimit/prctl path must be safe to run (it affects this test
        // process only). Does not mutate the environment or re-exec.
        unix::apply();
    }
}
