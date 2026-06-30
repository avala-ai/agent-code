//! The subagent execution limiter, wired through `TaskManager::spawn_command`,
//! bounds how many background tasks run concurrently: spawns past the cap
//! queue and start as running ones finish. Spawns real `sleep` processes,
//! so Unix-only.
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_code_lib::services::agent_control::AgentExecutionLimiter;
use agent_code_lib::services::background::{TaskKind, TaskManager, TaskPayload, TaskStatus};

fn sleep_cmd(secs: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sleep");
    cmd.arg(secs);
    cmd
}

fn agent_payload() -> TaskPayload {
    TaskPayload::LocalAgent {
        subagent_kind: Some("test".into()),
        prompt: "noop".into(),
        parent_session: None,
    }
}

async fn wait_all_done(mgr: &TaskManager, ids: &[String], timeout: Duration) {
    let start = Instant::now();
    loop {
        let mut all_done = true;
        for id in ids {
            match mgr.get_status(id).await {
                Some(info) if matches!(info.status, TaskStatus::Running) => all_done = false,
                Some(_) => {}
                None => all_done = false,
            }
        }
        if all_done {
            return;
        }
        assert!(start.elapsed() < timeout, "tasks did not finish in time");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn limiter_serializes_spawns_beyond_the_cap() {
    // Cap of 1: three ~0.4s tasks must run one at a time, so total
    // wall-clock is well above a single task's duration (which is what
    // it would be if they all ran at once).
    let mgr = TaskManager::new();
    let limiter = Arc::new(AgentExecutionLimiter::new(1));

    let start = Instant::now();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let id = mgr
            .spawn_command(
                sleep_cmd("0.4"),
                "sleeper",
                TaskKind::LocalAgent,
                agent_payload(),
                None,
                Some(limiter.clone()),
            )
            .await;
        ids.push(id);
    }

    wait_all_done(&mgr, &ids, Duration::from_secs(15)).await;
    let elapsed = start.elapsed();

    // Serialized: ~1.2s. Parallel (no cap): ~0.4s. A 1.0s floor cleanly
    // distinguishes the two without being tight enough to flake.
    assert!(
        elapsed >= Duration::from_millis(1_000),
        "tasks did not serialize under the cap (elapsed {elapsed:?}); limiter not applied?"
    );
}

#[tokio::test]
async fn unbounded_when_no_limiter() {
    // Same three tasks with no limiter run concurrently and finish in
    // roughly one task's duration.
    let mgr = TaskManager::new();

    let start = Instant::now();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let id = mgr
            .spawn_command(
                sleep_cmd("0.4"),
                "sleeper",
                TaskKind::LocalAgent,
                agent_payload(),
                None,
                None,
            )
            .await;
        ids.push(id);
    }

    wait_all_done(&mgr, &ids, Duration::from_secs(15)).await;
    assert!(
        start.elapsed() < Duration::from_millis(1_000),
        "uncapped tasks did not run concurrently"
    );
}
