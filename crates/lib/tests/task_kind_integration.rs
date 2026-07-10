//! Integration tests for the kind-tagged task model (Phase 8.13).
//!
//! These exercise the serde round-trip of [`TaskKind`] /
//! [`TaskPayload`] / [`TaskInfo`], the executor registry, and the
//! `TaskCreate` / `TaskList` tools when they cooperate through a
//! shared [`TaskManager`].

use std::sync::Arc;
use std::time::Duration;

use agent_code_lib::permissions::PermissionChecker;
use agent_code_lib::services::background::{TaskInfo, TaskKind, TaskManager, TaskPayload};
use agent_code_lib::tools::tasks::executors::{
    DreamExecutor, LocalAgentExecutor, LocalShellExecutor, LocalWorkflowExecutor,
    MonitorMcpExecutor, RemoteAgentExecutor,
};
use agent_code_lib::tools::tasks::{
    TaskCreateTool, TaskExecutorRegistry, TaskListTool, default_registry,
};
use agent_code_lib::tools::{Tool, ToolContext};

fn ctx_with_manager(mgr: Arc<TaskManager>) -> ToolContext {
    ToolContext {
        cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp")),
        cancel: tokio_util::sync::CancellationToken::new(),
        permission_checker: Arc::new(PermissionChecker::allow_all()),
        verbose: false,
        plan_mode: false,
        file_cache: None,
        denial_tracker: None,
        task_manager: Some(mgr),
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

// ---- TaskKind / TaskPayload round-trip per variant ----

#[test]
fn task_kind_round_trips_through_serde_for_every_variant() {
    for kind in [
        TaskKind::LocalShell,
        TaskKind::LocalAgent,
        TaskKind::LocalWorkflow,
        TaskKind::MonitorMcp,
        TaskKind::RemoteAgent,
        TaskKind::Dream,
    ] {
        let json = serde_json::to_string(&kind).expect("serialize");
        let back: TaskKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, back, "round-trip lost data for {kind:?}");
    }
}

#[test]
fn task_payload_local_shell_round_trips() {
    let p = TaskPayload::LocalShell {
        command: "echo hi".into(),
        cwd: std::path::PathBuf::from("/tmp"),
    };
    let s = serde_json::to_string(&p).unwrap();
    let back: TaskPayload = serde_json::from_str(&s).unwrap();
    assert!(matches!(back, TaskPayload::LocalShell { .. }));
    assert_eq!(back.kind(), TaskKind::LocalShell);
}

#[test]
fn task_payload_local_agent_round_trips() {
    let p = TaskPayload::LocalAgent {
        subagent_kind: Some("research".into()),
        prompt: "investigate".into(),
        parent_session: Some("sess_abc".into()),
    };
    let s = serde_json::to_string(&p).unwrap();
    let back: TaskPayload = serde_json::from_str(&s).unwrap();
    match back {
        TaskPayload::LocalAgent {
            subagent_kind,
            prompt,
            parent_session,
        } => {
            assert_eq!(subagent_kind.as_deref(), Some("research"));
            assert_eq!(prompt, "investigate");
            assert_eq!(parent_session.as_deref(), Some("sess_abc"));
        }
        other => panic!("expected LocalAgent, got {other:?}"),
    }
}

#[test]
fn task_payload_local_workflow_round_trips() {
    let p = TaskPayload::LocalWorkflow {
        workflow: "ship".into(),
        args: serde_json::json!({"branch": "main"}),
    };
    let s = serde_json::to_string(&p).unwrap();
    let back: TaskPayload = serde_json::from_str(&s).unwrap();
    match back {
        TaskPayload::LocalWorkflow { workflow, args } => {
            assert_eq!(workflow, "ship");
            assert_eq!(args["branch"], "main");
        }
        other => panic!("expected LocalWorkflow, got {other:?}"),
    }
}

#[test]
fn task_payload_monitor_mcp_round_trips() {
    let p = TaskPayload::MonitorMcp {
        server_name: "linear".into(),
        expected_tool: Some("create_issue".into()),
        timeout: Duration::from_secs(60),
    };
    let s = serde_json::to_string(&p).unwrap();
    let back: TaskPayload = serde_json::from_str(&s).unwrap();
    match back {
        TaskPayload::MonitorMcp {
            server_name,
            expected_tool,
            timeout,
        } => {
            assert_eq!(server_name, "linear");
            assert_eq!(expected_tool.as_deref(), Some("create_issue"));
            assert_eq!(timeout, Duration::from_secs(60));
        }
        other => panic!("expected MonitorMcp, got {other:?}"),
    }
}

#[test]
fn task_payload_remote_agent_round_trips() {
    let p = TaskPayload::RemoteAgent {
        routine_id: "weekly_retro".into(),
        timeout: Some(Duration::from_secs(900)),
    };
    let s = serde_json::to_string(&p).unwrap();
    let back: TaskPayload = serde_json::from_str(&s).unwrap();
    assert_eq!(back.kind(), TaskKind::RemoteAgent);
}

#[test]
fn task_payload_dream_round_trips() {
    let p = TaskPayload::Dream {
        note: Some("clean stale caches".into()),
    };
    let s = serde_json::to_string(&p).unwrap();
    let back: TaskPayload = serde_json::from_str(&s).unwrap();
    assert_eq!(back.kind(), TaskKind::Dream);
}

// ---- Legacy compatibility ----

#[test]
fn legacy_task_record_without_kind_field_deserializes_as_local_shell() {
    // Pre-8.13 schema: no `kind`, no `payload`. The defaults must
    // kick in so older state files keep working.
    let raw = r#"{
        "id": "b7",
        "description": "ls -la",
        "status": "running",
        "output_file": "/tmp/agent-tasks/b7.out"
    }"#;
    let info: TaskInfo = serde_json::from_str(raw).expect("legacy record should parse");
    assert_eq!(info.kind, TaskKind::LocalShell);
    assert!(info.payload.is_none());
}

#[test]
fn task_kind_default_is_local_shell() {
    assert_eq!(TaskKind::default(), TaskKind::LocalShell);
}

// ---- Executor registry dispatch ----

#[test]
fn registry_returns_right_executor_per_kind() {
    let r = default_registry();
    assert_eq!(
        r.get(TaskKind::LocalShell).unwrap().kind(),
        TaskKind::LocalShell
    );
    assert_eq!(
        r.get(TaskKind::LocalAgent).unwrap().kind(),
        TaskKind::LocalAgent
    );
    assert_eq!(
        r.get(TaskKind::LocalWorkflow).unwrap().kind(),
        TaskKind::LocalWorkflow
    );
    assert_eq!(
        r.get(TaskKind::MonitorMcp).unwrap().kind(),
        TaskKind::MonitorMcp
    );
    assert_eq!(
        r.get(TaskKind::RemoteAgent).unwrap().kind(),
        TaskKind::RemoteAgent
    );
    assert_eq!(r.get(TaskKind::Dream).unwrap().kind(), TaskKind::Dream);
}

#[test]
fn empty_registry_has_no_executor_for_any_kind() {
    let r = TaskExecutorRegistry::new();
    assert!(r.get(TaskKind::LocalShell).is_none());
}

#[test]
fn registry_register_and_lookup_known_executors() {
    let mut r = TaskExecutorRegistry::new();
    r.register(Arc::new(LocalShellExecutor));
    r.register(Arc::new(LocalAgentExecutor));
    r.register(Arc::new(LocalWorkflowExecutor));
    r.register(Arc::new(MonitorMcpExecutor));
    r.register(Arc::new(RemoteAgentExecutor));
    r.register(Arc::new(DreamExecutor));
    for kind in [
        TaskKind::LocalShell,
        TaskKind::LocalAgent,
        TaskKind::LocalWorkflow,
        TaskKind::MonitorMcp,
        TaskKind::RemoteAgent,
        TaskKind::Dream,
    ] {
        assert_eq!(r.get(kind).unwrap().kind(), kind);
    }
}

// ---- TaskList / TaskCreate end-to-end ----

#[tokio::test]
async fn task_create_local_agent_then_task_list_surfaces_the_kind() {
    let mgr = Arc::new(TaskManager::new());

    // Register a LocalAgent task directly (TaskCreate today is a thin
    // book-keeping surface — it allocates an id but does not push to
    // the manager). The manager's queue is what TaskList reads.
    let id = mgr
        .register(
            "investigate flaky test",
            TaskKind::LocalAgent,
            TaskPayload::LocalAgent {
                subagent_kind: Some("research".into()),
                prompt: "trace the leak".into(),
                parent_session: None,
            },
        )
        .await;
    assert!(id.starts_with('a'), "LocalAgent ids prefix with 'a'");

    // TaskCreate exists for the model's progress-tracking flow; ensure
    // it accepts the kind and reports it back.
    let create_res = TaskCreateTool
        .call(
            serde_json::json!({
                "description": "investigate flaky test",
                "kind": "LocalAgent",
            }),
            &ctx_with_manager(mgr.clone()),
        )
        .await
        .unwrap();
    assert!(create_res.content.contains("[kind: LocalAgent]"));

    // TaskList should now show the queued LocalAgent task.
    let list_res = TaskListTool
        .call(serde_json::json!({}), &ctx_with_manager(mgr.clone()))
        .await
        .unwrap();
    assert!(
        list_res.content.contains("[kind: LocalAgent]"),
        "expected kind label in TaskList output, got: {}",
        list_res.content
    );
    assert!(list_res.content.contains(&id));
}

#[tokio::test]
async fn task_manager_register_uses_kind_specific_id_prefix() {
    let mgr = TaskManager::new();
    let shell_id = mgr
        .register(
            "tail logs",
            TaskKind::LocalShell,
            TaskPayload::LocalShell {
                command: "tail -f x".into(),
                cwd: std::path::PathBuf::from("/tmp"),
            },
        )
        .await;
    let monitor_id = mgr
        .register(
            "watch linear",
            TaskKind::MonitorMcp,
            TaskPayload::MonitorMcp {
                server_name: "linear".into(),
                expected_tool: None,
                timeout: Duration::from_secs(30),
            },
        )
        .await;
    let dream_id = mgr
        .register(
            "background cleanup",
            TaskKind::Dream,
            TaskPayload::Dream { note: None },
        )
        .await;
    assert!(shell_id.starts_with('b'));
    assert!(monitor_id.starts_with('m'));
    assert!(dream_id.starts_with('d'));
}
