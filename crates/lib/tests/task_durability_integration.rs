//! Durability: a background task journaled by one `TaskManager` is
//! recovered by a fresh manager pointed at the same directory — the
//! "adopt after restart" path. Spawns a real shell task, so Unix-only.
#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use agent_code_lib::services::background::{TaskManager, TaskStatus};

async fn wait_terminal(mgr: &TaskManager, id: &str, timeout_ms: u64) {
    let start = std::time::Instant::now();
    loop {
        match mgr.get_status(id).await {
            Some(info) if !matches!(info.status, TaskStatus::Running) => return,
            _ => {}
        }
        assert!(
            start.elapsed().as_millis() < timeout_ms as u128,
            "task {id} did not finish in time"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn completed_task_is_adopted_by_a_fresh_manager_and_surfaces() {
    let dir = std::env::temp_dir().join(format!("agentcode-dur-{}", uuid::Uuid::new_v4()));

    // Session 1: spawn a task and let it finish. We never drain it, so
    // its journal records an un-notified Completed task.
    let id = {
        let mgr = TaskManager::with_persistence(dir.clone());
        let id = mgr
            .spawn_shell("echo durable-output", "demo", Path::new("."))
            .await
            .expect("spawn");
        wait_terminal(&mgr, &id, 5_000).await;
        id
    };

    // Session 2: a brand-new manager over the same directory adopts the
    // leftover task and surfaces its completion exactly once.
    let mgr2 = TaskManager::with_persistence(dir.clone());
    assert!(
        mgr2.adopt(None).await >= 1,
        "expected to adopt the leftover task"
    );

    let recovered = mgr2.get_status(&id).await.expect("task recovered");
    assert_eq!(recovered.status, TaskStatus::Completed);

    let drained = mgr2.drain_completions().await;
    assert!(
        drained.iter().any(|t| t.id == id),
        "adopted completion should surface"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
