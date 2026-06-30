//! End-to-end test for the cron + remote-trigger tool layer.
//!
//! Walks through the canonical lifecycle a model would drive — create
//! a routine, list it, attempt to trigger it, then delete it — using
//! a temp-dir storage backend so nothing escapes the test sandbox.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use agent_code_lib::permissions::PermissionChecker;
use agent_code_lib::tools::cron_create::CronCreateTool;
use agent_code_lib::tools::cron_delete::CronDeleteTool;
use agent_code_lib::tools::cron_list::CronListTool;
use agent_code_lib::tools::cron_support::SCHEDULES_DIR_ENV;
use agent_code_lib::tools::remote_trigger::RemoteTriggerTool;
use agent_code_lib::tools::{Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Serialize env-var access across the integration tests so the two
/// tests don't trample each other's storage directory. Async-aware
/// because the tests hold the guard across `await` points.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn ctx() -> ToolContext {
    ToolContext {
        cwd: PathBuf::from("."),
        cancel: CancellationToken::new(),
        permission_checker: Arc::new(PermissionChecker::allow_all()),
        verbose: false,
        plan_mode: false,
        file_cache: None,
        denial_tracker: None,
        task_manager: None,
        subagent_colors: None,
        session_allows: None,
        permission_prompter: None,
        sandbox: None,
        active_disk_output_style: None,
        agent_limiter: None,
    }
}

#[tokio::test]
async fn lifecycle_create_list_trigger_delete() {
    let _guard = env_lock().lock().await;
    // Hermetic storage so this test doesn't touch the real config dir.
    let tmp = TempDir::new().unwrap();
    // SAFETY: env access is serialized via `env_lock()`.
    unsafe {
        std::env::set_var(SCHEDULES_DIR_ENV, tmp.path());
    }

    // 1. Create.
    let create_res = CronCreateTool
        .call(
            json!({
                "cron_expression": "*/10 * * * *",
                "prompt": "integration smoke",
                "name": "integration-routine"
            }),
            &ctx(),
        )
        .await
        .expect("create succeeds");
    assert!(
        !create_res.is_error,
        "create errored: {}",
        create_res.content
    );
    let created: serde_json::Value = serde_json::from_str(&create_res.content).unwrap();
    assert_eq!(created["id"].as_str(), Some("integration-routine"));
    assert!(created["next_run_at"].is_string());

    // 2. List sees the new routine.
    let list_res = CronListTool.call(json!({}), &ctx()).await.unwrap();
    let listed: serde_json::Value = serde_json::from_str(&list_res.content).unwrap();
    assert_eq!(listed["count"].as_u64(), Some(1));
    let routines = listed["routines"].as_array().unwrap();
    assert_eq!(routines[0]["id"].as_str(), Some("integration-routine"));
    assert_eq!(routines[0]["enabled"].as_bool(), Some(true));

    // 3. Trigger — exercises the input validation path. The full run
    //    spawns a subprocess that needs a real LLM provider, so we
    //    don't expect success here. We do expect that the tool finds
    //    the routine and proceeds to spawn (or returns Timeout/error
    //    rather than InvalidInput "no routine").
    let trigger = RemoteTriggerTool
        .call(
            json!({
                "id": "integration-routine",
                "timeout_seconds": 1
            }),
            &ctx(),
        )
        .await;
    match trigger {
        // Subprocess produced output (likely an auth error) — what
        // matters is that we got past the routine-lookup check.
        Ok(result) => {
            assert!(
                !result.content.contains("No routine with id"),
                "trigger should not surface routine-missing error: {}",
                result.content
            );
        }
        // Timeout or generic execution failure are both acceptable —
        // both prove the spawn path was reached.
        Err(e) => {
            let msg = e.to_string();
            assert!(
                !msg.contains("'integration-routine'") || !msg.contains("does not"),
                "unexpected error from trigger: {msg}"
            );
        }
    }

    // 4. Delete.
    let delete_res = CronDeleteTool
        .call(json!({"id": "integration-routine"}), &ctx())
        .await
        .unwrap();
    assert!(!delete_res.is_error);
    let deleted: serde_json::Value = serde_json::from_str(&delete_res.content).unwrap();
    assert_eq!(deleted["deleted"].as_bool(), Some(true));

    // 5. List is empty again.
    let final_list = CronListTool.call(json!({}), &ctx()).await.unwrap();
    let final_parsed: serde_json::Value = serde_json::from_str(&final_list.content).unwrap();
    assert_eq!(final_parsed["count"].as_u64(), Some(0));

    // SAFETY: cleanup matches the set above.
    unsafe {
        std::env::remove_var(SCHEDULES_DIR_ENV);
    }
}

#[tokio::test]
async fn trigger_missing_routine_is_invalid_input() {
    let _guard = env_lock().lock().await;
    let tmp = TempDir::new().unwrap();
    // SAFETY: env access is serialized via `env_lock()`.
    unsafe {
        std::env::set_var(SCHEDULES_DIR_ENV, tmp.path());
    }

    let err = RemoteTriggerTool
        .call(json!({"id": "never-existed"}), &ctx())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        agent_code_lib::error::ToolError::InvalidInput(_)
    ));

    unsafe {
        std::env::remove_var(SCHEDULES_DIR_ENV);
    }
}
