//! Shared helpers for the cron-management tools.
//!
//! Centralizes the [`ScheduleStore`] opener so tests can redirect the
//! storage directory via the `AGENT_CODE_SCHEDULES_DIR` environment
//! variable. In normal operation the store opens at the platform's
//! default config dir, matching the rest of the schedule subsystem.

use std::path::PathBuf;

use crate::schedule::ScheduleStore;

/// Environment variable that, when set, overrides the schedules directory.
/// Used by tests to keep storage hermetic. Not part of the public CLI
/// surface — operators should rely on the default config directory.
pub const SCHEDULES_DIR_ENV: &str = "AGENT_CODE_SCHEDULES_DIR";

/// Open the schedule store, honoring [`SCHEDULES_DIR_ENV`] when set.
pub fn open_store() -> Result<ScheduleStore, String> {
    if let Ok(dir) = std::env::var(SCHEDULES_DIR_ENV) {
        ScheduleStore::open_at(PathBuf::from(dir))
    } else {
        ScheduleStore::open()
    }
}

#[cfg(test)]
pub use test_helpers::*;

#[cfg(test)]
mod test_helpers {
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::SCHEDULES_DIR_ENV;
    use crate::permissions::PermissionChecker;
    use crate::tools::ToolContext;

    /// Serializes test access to the env-var override so concurrent
    /// tests don't trample each other's storage directories. Returned
    /// guard restores prior state on drop.
    pub struct TestStoreGuard {
        _tmp: TempDir,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for TestStoreGuard {
        fn drop(&mut self) {
            // Safe in single-threaded test (mutex held above).
            // SAFETY: env access is serialized via the global mutex
            // guarded by `_lock`.
            unsafe {
                if let Some(ref prev) = self.prev {
                    std::env::set_var(SCHEDULES_DIR_ENV, prev);
                } else {
                    std::env::remove_var(SCHEDULES_DIR_ENV);
                }
            }
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Set the schedules directory to a fresh temp dir for the
    /// duration of the returned guard. Use at the top of each test
    /// that touches the schedule store.
    pub fn with_test_store() -> TestStoreGuard {
        let lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().expect("temp dir");
        let prev = std::env::var(SCHEDULES_DIR_ENV).ok();
        // SAFETY: env access is serialized via `lock`.
        unsafe {
            std::env::set_var(SCHEDULES_DIR_ENV, tmp.path());
        }
        TestStoreGuard {
            _tmp: tmp,
            prev,
            _lock: lock,
        }
    }

    /// Build a minimal [`ToolContext`] suitable for unit tests that
    /// don't exercise permission prompts or sandboxing.
    pub fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("."),
            cancel: CancellationToken::new(),
            permission_checker: std::sync::Arc::new(PermissionChecker::allow_all()),
            verbose: false,
            plan_mode: false,
            file_cache: None,
            denial_tracker: None,
            task_manager: None,
            subagent_colors: None,
            session_allows: None,
            permission_prompter: None,
            question_asker: None,
            agent_origin: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
        }
    }
}
