//! Query source tagging.
//!
//! Every API call is tagged with its originating source so costs
//! and errors can be attributed correctly.

/// Identifies where an API call originated from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum QuerySource {
    /// Interactive REPL main thread.
    ReplMainThread,
    /// Subagent execution.
    SubAgent { agent_id: String },
    /// History compaction.
    Compact,
    /// Session memory summarization.
    SessionMemory,
    /// Hook execution.
    Hook { event: String },
    /// Background task.
    BackgroundTask { task_id: String },
    /// One-shot CLI invocation.
    OneShot,
}

impl QuerySource {
    /// Whether this source should retry on transient errors.
    pub fn should_retry_on_overload(&self) -> bool {
        match self {
            Self::ReplMainThread | Self::OneShot => true,
            Self::SubAgent { .. } | Self::BackgroundTask { .. } => false,
            Self::Compact | Self::SessionMemory | Self::Hook { .. } => true,
        }
    }

    /// String label for telemetry.
    pub fn label(&self) -> String {
        match self {
            Self::ReplMainThread => "repl_main".to_string(),
            Self::SubAgent { agent_id } => format!("subagent_{agent_id}"),
            Self::Compact => "compact".to_string(),
            Self::SessionMemory => "session_memory".to_string(),
            Self::Hook { event } => format!("hook_{event}"),
            Self::BackgroundTask { task_id } => format!("bg_task_{task_id}"),
            Self::OneShot => "oneshot".to_string(),
        }
    }
}
