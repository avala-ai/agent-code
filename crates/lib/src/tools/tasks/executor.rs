//! Per-kind task executor trait and registry.
//!
//! Each [`TaskKind`] has exactly one [`TaskExecutor`] registered in the
//! [`TaskExecutorRegistry`]. The registry mirrors
//! [`crate::tools::registry::ToolRegistry`] in spirit: dynamic
//! `Arc<dyn TaskExecutor>` dispatch keyed by kind. New kinds plug in
//! by implementing `TaskExecutor` and calling
//! [`TaskExecutorRegistry::register`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::services::background::{TaskKind, TaskManager, TaskPayload};

/// Outcome of a successful executor run.
///
/// The `output` field is what `TaskOutput` shows to the model. The
/// distinction between `Ok` and `Err` of `Result<TaskResult, _>` is
/// "did the executor crash?", not "did the task succeed?". A
/// command that returns exit code 1 still produces a `TaskResult` —
/// `is_error` flags it.
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub output: String,
    pub is_error: bool,
}

impl TaskResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    pub fn failure(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

/// Errors a [`TaskExecutor`] can raise.
#[derive(Debug, Error)]
pub enum TaskError {
    /// The executor for this kind is intentionally a stub — the
    /// caller should pick a different kind or wait for a later phase.
    #[error("Task kind '{0}' is not yet implemented")]
    NotImplemented(&'static str),

    /// The payload variant did not match the registered executor's
    /// kind. Callers should normally never see this — registry
    /// dispatch keys by kind.
    #[error("Task payload mismatch: expected {expected:?}, got {actual:?}")]
    PayloadMismatch {
        expected: TaskKind,
        actual: TaskKind,
    },

    /// The payload for this kind was missing required fields.
    #[error("Invalid task payload: {0}")]
    InvalidPayload(String),

    /// Anything else.
    #[error("Task execution failed: {0}")]
    ExecutionFailed(String),

    /// The work was cancelled before it finished.
    #[error("Task cancelled")]
    Cancelled,
}

/// Shared context handed to every executor run.
///
/// Built by the dispatch site so executors do not have to thread
/// configuration plumbing through their own constructors. `cwd` is
/// the working directory the task was launched from; `task_manager`
/// lets executors register sub-records (e.g. a `LocalAgent` run that
/// in turn spawns shell tasks).
pub struct TaskContext {
    pub cwd: PathBuf,
    pub cancel: CancellationToken,
    pub task_manager: Option<Arc<TaskManager>>,
    /// Per-session color assignments for spawned subagents. Optional
    /// so freshly-built test contexts that don't exercise color
    /// assignment can still be constructed via [`Self::new`].
    pub subagent_colors: Option<Arc<crate::services::subagent_colors::SubagentColorManager>>,
    /// Mirror of `security.disable_skill_shell_execution`. When `true`,
    /// fenced shell blocks in a skill/workflow template are stripped
    /// before the prompt is handed to a subagent (see
    /// [`crate::skills::Skill::expand_safe`]). Defaults to `false` so a
    /// dispatch site that hasn't wired config yet preserves today's
    /// behavior; the production driver threads the real flag in.
    pub disable_skill_shell: bool,
}

impl TaskContext {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            cancel: CancellationToken::new(),
            task_manager: None,
            subagent_colors: None,
            disable_skill_shell: false,
        }
    }
}

/// Implemented by anything that can execute one [`TaskKind`].
///
/// The trait is `Send + Sync` so executors can sit behind an
/// `Arc<dyn TaskExecutor>` in the registry exactly the way tools do.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Which kind this executor handles.
    fn kind(&self) -> TaskKind;

    /// Run the work described by `payload`.
    async fn execute(
        &self,
        payload: &TaskPayload,
        ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError>;
}

/// Registry of executors, keyed by [`TaskKind`].
///
/// At construction time the registry is empty; call
/// [`default_registry`] for one wired up with the in-tree executors.
#[derive(Default)]
pub struct TaskExecutorRegistry {
    executors: HashMap<TaskKind, Arc<dyn TaskExecutor>>,
}

impl TaskExecutorRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) the executor for one kind.
    pub fn register(&mut self, executor: Arc<dyn TaskExecutor>) {
        self.executors.insert(executor.kind(), executor);
    }

    /// Look up the executor for `kind`.
    pub fn get(&self, kind: TaskKind) -> Option<Arc<dyn TaskExecutor>> {
        self.executors.get(&kind).cloned()
    }

    /// Dispatch a payload to the right executor in one call. Errors
    /// when no executor is registered for the payload's kind.
    pub async fn execute(
        &self,
        payload: &TaskPayload,
        ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        let kind = payload.kind();
        let executor = self.get(kind).ok_or_else(|| {
            TaskError::ExecutionFailed(format!(
                "no executor registered for kind '{}'",
                kind.as_str()
            ))
        })?;
        executor.execute(payload, ctx).await
    }
}

/// Build the default registry: one executor per [`TaskKind`].
///
/// `LocalShell` and `LocalAgent` reuse the existing tool path so we do
/// not duplicate logic. `LocalWorkflow` resolves a skill slug and runs
/// it as a subagent through the same path. The remaining kinds are
/// stubbed — `MonitorMcp` and `Dream` raise `NotImplemented`,
/// `RemoteAgent` is a thin wrapper over the existing `RemoteTrigger`
/// cron path that lands fully in 8.14.
pub fn default_registry() -> TaskExecutorRegistry {
    use super::executors::{
        DreamExecutor, LocalAgentExecutor, LocalShellExecutor, LocalWorkflowExecutor,
        MonitorMcpExecutor, RemoteAgentExecutor,
    };

    let mut registry = TaskExecutorRegistry::new();
    registry.register(Arc::new(LocalShellExecutor));
    registry.register(Arc::new(LocalAgentExecutor));
    registry.register(Arc::new(LocalWorkflowExecutor));
    registry.register(Arc::new(MonitorMcpExecutor));
    registry.register(Arc::new(RemoteAgentExecutor));
    registry.register(Arc::new(DreamExecutor));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(TaskKind);

    #[async_trait]
    impl TaskExecutor for Fake {
        fn kind(&self) -> TaskKind {
            self.0
        }
        async fn execute(
            &self,
            _payload: &TaskPayload,
            _ctx: &TaskContext,
        ) -> Result<TaskResult, TaskError> {
            Ok(TaskResult::success(format!("ran {:?}", self.0)))
        }
    }

    #[test]
    fn registry_keys_by_kind() {
        let mut r = TaskExecutorRegistry::new();
        r.register(Arc::new(Fake(TaskKind::LocalShell)));
        r.register(Arc::new(Fake(TaskKind::LocalAgent)));
        assert_eq!(
            r.get(TaskKind::LocalShell).unwrap().kind(),
            TaskKind::LocalShell
        );
        assert_eq!(
            r.get(TaskKind::LocalAgent).unwrap().kind(),
            TaskKind::LocalAgent
        );
        assert!(r.get(TaskKind::Dream).is_none());
    }

    #[test]
    fn default_registry_has_one_executor_per_kind() {
        let r = default_registry();
        for kind in [
            TaskKind::LocalShell,
            TaskKind::LocalAgent,
            TaskKind::LocalWorkflow,
            TaskKind::MonitorMcp,
            TaskKind::RemoteAgent,
            TaskKind::Dream,
        ] {
            let exec = r
                .get(kind)
                .unwrap_or_else(|| panic!("missing executor for {kind:?}"));
            assert_eq!(exec.kind(), kind);
        }
    }
}
