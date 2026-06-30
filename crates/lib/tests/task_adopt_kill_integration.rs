//! An adopted live task (recovered across a restart, with no in-process
//! cancellation handle) must still be killable: `kill()` falls back to
//! signaling the recorded process group by pid. Spawns a real process,
//! so Unix-only.
#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::time::Duration;

use agent_code_lib::services::background::{TaskInfo, TaskKind, TaskManager, TaskStatus};

#[tokio::test]
async fn kill_terminates_an_adopted_live_task_by_pid() {
    let dir = std::env::temp_dir().join(format!("agentcode-adoptkill-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    // Spawn a long sleeper in its OWN process group (mirrors how
    // spawn_command launches tasks), so killpg(pid) reaches it.
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    cmd.process_group(0);
    let mut child = cmd.spawn().expect("spawn sleeper");
    let pid = child.id();

    // Journal it as a Running task owned by a *previous* session — the
    // current process has no cancel handle for it.
    let info = TaskInfo {
        id: "a4242".to_string(),
        description: "orphan".to_string(),
        status: TaskStatus::Running,
        output_file: dir.join("a4242.out"),
        kind: TaskKind::LocalAgent,
        payload: None,
        subagent_color: None,
        notified: false,
        pid: Some(pid),
        started_at: std::time::Instant::now(),
        finished_at: None,
    };
    let path: PathBuf = dir.join("a4242.json");
    std::fs::write(&path, serde_json::to_vec(&info).unwrap()).unwrap();

    // A fresh manager adopts it (process is alive → stays Running).
    let mgr = TaskManager::with_persistence(dir.clone());
    assert!(
        mgr.adopt(None).await >= 1,
        "should adopt the journaled task"
    );
    let adopted = mgr.get_status("a4242").await.expect("adopted");
    assert_eq!(adopted.status, TaskStatus::Running);

    // Kill it: with no cancel handle, kill() must fall back to the pid
    // process-group signal and actually terminate the orphan.
    mgr.kill("a4242").await.expect("kill");

    // The child must exit promptly (killed by signal), not run its full
    // 30s sleep.
    let exited = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(Some(_status)) = child.try_wait() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or(false);

    assert!(exited, "adopted live task was not terminated by kill()");

    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn kill_does_not_signal_stale_pid_of_terminal_task() {
    // A terminal (failed/completed) task loaded by adopt() carries a pid
    // that is stale — the process exited and the OS may have reused the
    // number. kill() must NOT signal it. We prove this by pointing a
    // terminal task's pid at a *live, unrelated* process and confirming
    // kill() leaves it running.
    let dir = std::env::temp_dir().join(format!("agentcode-stalepid-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    cmd.process_group(0);
    let mut bystander = cmd.spawn().expect("spawn bystander");
    let pid = bystander.id();

    // Journal a *terminal* (Failed, unnotified) task whose pid happens to
    // point at the live bystander — the dangerous stale-pid case.
    let info = TaskInfo {
        id: "a9001".to_string(),
        description: "ghost".to_string(),
        status: TaskStatus::Failed("interrupted by restart".to_string()),
        output_file: dir.join("a9001.out"),
        kind: TaskKind::LocalAgent,
        payload: None,
        subagent_color: None,
        notified: false,
        pid: Some(pid),
        started_at: std::time::Instant::now(),
        finished_at: None,
    };
    std::fs::write(dir.join("a9001.json"), serde_json::to_vec(&info).unwrap()).unwrap();

    let mgr = TaskManager::with_persistence(dir.clone());
    assert!(mgr.adopt(None).await >= 1);

    // kill() on the already-terminal task must be a no-op for the process.
    mgr.kill("a9001").await.expect("kill");

    // Give any (erroneous) signal time to land, then confirm the
    // bystander is still alive.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        matches!(bystander.try_wait(), Ok(None)),
        "kill() signalled a stale pid and killed an unrelated process"
    );

    let _ = bystander.kill();
    let _ = std::fs::remove_dir_all(&dir);
}
