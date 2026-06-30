//! End-to-end coverage of the background-task surfacing path that the
//! interactive REPL relies on: spawn a real shell task, wait for it to
//! finish, drain its completion exactly once, render the injected
//! envelope, and confirm a killed task never surfaces.
//!
//! These exercise real `bash`/`echo`/`sleep` subprocesses and Unix
//! process-group kill, so the suite is Unix-only.
#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use agent_code_lib::services::background::{TaskManager, TaskStatus};
use agent_code_lib::services::task_surface;

async fn wait_terminal(mgr: &TaskManager, id: &str, timeout_ms: u64) -> TaskStatus {
    let start = std::time::Instant::now();
    loop {
        match mgr.get_status(id).await {
            Some(info) if !matches!(info.status, TaskStatus::Running) => return info.status,
            _ => {}
        }
        if start.elapsed().as_millis() as u64 > timeout_ms {
            panic!("task {id} did not reach a terminal state in {timeout_ms}ms");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn shell_task_surfaces_once_with_output_envelope() {
    let mgr = TaskManager::new();
    let id = mgr
        .spawn_shell("echo surfaced-output", "demo", Path::new("."))
        .await
        .expect("spawn");

    assert_eq!(wait_terminal(&mgr, &id, 5_000).await, TaskStatus::Completed);

    // First drain surfaces the task; the rendered envelope carries the
    // captured output for injection into the conversation.
    let drained = mgr.drain_completions().await;
    assert_eq!(drained.len(), 1, "exactly one completion should surface");
    let info = &drained[0];
    assert_eq!(info.id, id);

    let output = mgr.read_output(&id).await.unwrap_or_default();
    let envelope = task_surface::completion_envelope(info, &output);
    assert!(envelope.contains("surfaced-output"), "envelope: {envelope}");
    assert!(envelope.contains("status=\"completed\""));

    // Second drain is empty: the `notified` flag de-duplicates.
    assert!(
        mgr.drain_completions().await.is_empty(),
        "a completion must never surface twice"
    );
}

#[tokio::test]
async fn killed_task_does_not_surface() {
    let mgr = TaskManager::new();
    let id = mgr
        .spawn_shell("sleep 5", "sleeper", Path::new("."))
        .await
        .expect("spawn");

    // Let it start, then kill it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    mgr.kill(&id).await.expect("kill");

    assert_eq!(wait_terminal(&mgr, &id, 5_000).await, TaskStatus::Killed);
    assert!(
        mgr.drain_completions().await.is_empty(),
        "killed tasks are user-initiated and must not be surfaced"
    );
}
