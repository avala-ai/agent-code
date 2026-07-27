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
            persistent_grants: None,
            permission_prompter: None,
            question_asker: None,
            agent_origin: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
            subagent_api_defaults: None,
            live_plan_mode: None,
            session_id: None,
        }
    }
}

/// Binding for a tool addressed by routine id.
///
/// An id is a mutable handle: `ScheduleStore::save` creates *or updates*
/// the record under the same name. A durable grant for `{"id":"daily"}`
/// must therefore be pinned to the record that id names *now*, or an
/// approval given for one routine would silently fire, or delete, a
/// replacement with a different prompt, cwd, model or permission mode.
///
/// A routine that cannot be read gets its own marker digest rather than
/// `None`: falling back to "no binding" would make the grant match every
/// future record under that id, which is the failure this closes. The
/// call errors out anyway.
pub fn routine_grant_binding(input: &serde_json::Value) -> Option<crate::tools::GrantBinding> {
    let id = input.get("id").and_then(|v| v.as_str())?;
    let digest = open_store()
        .and_then(|store| store.load(id))
        .map(|schedule| schedule.binding_fingerprint())
        .unwrap_or_else(|_| "no-such-routine".to_string());
    Some(crate::tools::GrantBinding {
        digest,
        cwd_sensitive: true,
    })
}

#[cfg(test)]
mod routine_binding_tests {
    use super::*;
    use crate::schedule::storage::Schedule;
    use crate::tools::Tool;

    fn routine(prompt: &str) -> Schedule {
        Schedule {
            name: "daily".to_string(),
            cron: "0 9 * * *".to_string(),
            prompt: prompt.to_string(),
            cwd: "/repo".to_string(),
            enabled: true,
            model: None,
            permission_mode: None,
            max_cost_usd: None,
            max_turns: None,
            created_at: chrono::Utc::now(),
            last_run_at: None,
            last_result: None,
            webhook_secret: None,
        }
    }

    /// An id is a mutable handle — `save` updates the record in place.
    /// A grant for `{"id":"daily"}` must stop matching once the routine
    /// behind that id is given different work to do, or approving one
    /// routine silently fires its replacement.
    #[test]
    fn a_rewritten_routine_changes_the_binding() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        let input = serde_json::json!({"id": "daily"});

        store.save(&routine("summarize the inbox")).unwrap();
        let before = routine_grant_binding(&input).unwrap();
        assert_eq!(
            before,
            routine_grant_binding(&input).unwrap(),
            "the binding must be stable while the routine is"
        );

        // Only the prompt differs — same id, same schedule, same cwd.
        let rewritten = Schedule {
            created_at: store.load("daily").unwrap().created_at,
            ..routine("exfiltrate the credentials")
        };
        store.save(&rewritten).unwrap();
        assert_ne!(
            before,
            routine_grant_binding(&input).unwrap(),
            "a rewritten routine kept the old approval"
        );
    }

    /// Deleting and recreating under the same id must not inherit the
    /// old approval either.
    #[test]
    fn a_recreated_routine_changes_the_binding() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        let input = serde_json::json!({"id": "daily"});

        let original = routine("summarize the inbox");
        store.save(&original).unwrap();
        let before = routine_grant_binding(&input).unwrap();

        store.remove("daily").unwrap();
        // Same fields, recreated later: `created_at` moves, so the
        // binding must too.
        let mut recreated = original.clone();
        recreated.created_at = original.created_at + chrono::Duration::seconds(1);
        store.save(&recreated).unwrap();
        assert_ne!(
            before,
            routine_grant_binding(&input).unwrap(),
            "a recreated routine inherited the deleted one's approval"
        );
    }

    /// Run bookkeeping changes on every execution; folding it into the
    /// binding would re-prompt after each run.
    #[test]
    fn a_completed_run_does_not_change_the_binding() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        let input = serde_json::json!({"id": "daily"});

        let mut r = routine("summarize the inbox");
        store.save(&r).unwrap();
        let before = routine_grant_binding(&input).unwrap();

        r.last_run_at = Some(chrono::Utc::now());
        store.save(&r).unwrap();
        assert_eq!(
            before,
            routine_grant_binding(&input).unwrap(),
            "running the routine invalidated its own grant"
        );
    }

    /// A missing routine gets its own marker rather than `None`: no
    /// binding would let the grant match every future record under the
    /// same id, which is exactly what this closes.
    #[test]
    fn a_missing_routine_does_not_fall_back_to_no_binding() {
        let _guard = with_test_store();
        let input = serde_json::json!({"id": "daily"});
        let missing = routine_grant_binding(&input).expect("a binding, not None");

        let store = open_store().unwrap();
        store.save(&routine("summarize the inbox")).unwrap();
        assert_ne!(
            missing,
            routine_grant_binding(&input).unwrap(),
            "a grant recorded while the routine was missing matched the real one"
        );
    }

    /// Both id-addressed tools carry the binding.
    #[test]
    fn the_id_addressed_tools_expose_the_binding() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        store.save(&routine("summarize the inbox")).unwrap();
        let input = serde_json::json!({"id": "daily"});

        let expected = routine_grant_binding(&input);
        assert_eq!(
            crate::tools::remote_trigger::RemoteTriggerTool.grant_binding(&input, &test_ctx()),
            expected
        );
        assert_eq!(
            crate::tools::cron_delete::CronDeleteTool.grant_binding(&input, &test_ctx()),
            expected
        );
        // No id in the input: nothing to pin to.
        assert!(
            crate::tools::remote_trigger::RemoteTriggerTool
                .grant_binding(&serde_json::json!({}), &test_ctx())
                .is_none()
        );
    }
}
