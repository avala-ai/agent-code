//! The subagent execution limiter, wired through `TaskManager::spawn_command`,
//! bounds how many background tasks run concurrently: spawns past the cap
//! queue and start as running ones finish. Spawns real `sleep` processes,
//! so Unix-only.
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_code_lib::services::agent_control::AgentExecutionLimiter;
use agent_code_lib::services::background::{
    TaskInfo, TaskKind, TaskManager, TaskPayload, TaskStatus,
};

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

#[tokio::test]
async fn killed_while_queued_task_is_never_started() {
    // Cap of 1. Task A holds the only slot; task B queues behind it.
    // Killing B while queued must discard it — it must never spawn its
    // process (which would create a sentinel file).
    let mgr = TaskManager::new();
    let limiter = Arc::new(AgentExecutionLimiter::new(1));

    let a = mgr
        .spawn_command(
            sleep_cmd("1.0"),
            "holder",
            TaskKind::LocalAgent,
            agent_payload(),
            None,
            Some(limiter.clone()),
        )
        .await;

    let sentinel =
        std::env::temp_dir().join(format!("agentcode-queuekill-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::remove_file(&sentinel);
    let mut b_cmd = tokio::process::Command::new("bash");
    b_cmd.arg("-c").arg(format!("touch {}", sentinel.display()));
    let b = mgr
        .spawn_command(
            b_cmd,
            "queued",
            TaskKind::LocalAgent,
            agent_payload(),
            None,
            Some(limiter.clone()),
        )
        .await;

    // Kill B while it's queued behind A.
    tokio::time::sleep(Duration::from_millis(150)).await;
    mgr.kill(&b).await.unwrap();

    // Let A finish (frees the slot) and give a discarded-vs-run margin.
    wait_all_done(&mgr, &[a], Duration::from_secs(10)).await;
    tokio::time::sleep(Duration::from_millis(600)).await;

    assert!(
        !sentinel.exists(),
        "killed queued task still ran its process"
    );
    assert_eq!(mgr.get_status(&b).await.unwrap().status, TaskStatus::Killed);
    let _ = std::fs::remove_file(&sentinel);
}

#[tokio::test]
async fn adopt_reserves_a_slot_for_a_live_subagent() {
    use std::os::unix::process::CommandExt;

    let dir = std::env::temp_dir().join(format!("agentcode-adoptcap-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    // A live process standing in for a subagent that survived a restart.
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    cmd.process_group(0);
    let mut child = cmd.spawn().unwrap();
    let pid = child.id();

    let info = TaskInfo {
        id: "a1".to_string(),
        description: "orphan subagent".to_string(),
        status: TaskStatus::Running,
        output_file: dir.join("a1.out"),
        kind: TaskKind::LocalAgent,
        payload: None,
        subagent_color: None,
        notified: false,
        pid: Some(pid),
        started_at: std::time::Instant::now(),
        finished_at: None,
    };
    std::fs::write(dir.join("a1.json"), serde_json::to_vec(&info).unwrap()).unwrap();

    let mgr = TaskManager::with_persistence(dir.clone());
    let limiter = Arc::new(AgentExecutionLimiter::new(2));
    assert_eq!(mgr.adopt(Some(limiter.clone())).await, 1);

    // One of the two slots is reserved for the adopted live subagent.
    assert_eq!(
        limiter.available(),
        1,
        "adopted subagent did not reserve a slot"
    );

    // When the adopted process exits, the watcher releases its slot.
    // (Reap the child each poll: as our own child it would otherwise
    // linger as a zombie whose pid still looks alive. In production the
    // adopted pid is an orphan reaped by init.)
    let _ = child.kill();
    let mut released = false;
    for _ in 0..60 {
        let _ = child.try_wait();
        if limiter.available() == 2 {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(released, "slot not released after the adopted process died");

    let _ = std::fs::remove_dir_all(&dir);
}
