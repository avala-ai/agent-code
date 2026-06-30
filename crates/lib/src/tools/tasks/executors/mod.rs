//! Per-kind [`TaskExecutor`](super::executor::TaskExecutor)
//! implementations.
//!
//! Each kind lives in its own file so the wiring stays narrow.
//! `LocalShell` is the only fully-functional executor today —
//! `LocalAgent` re-uses the existing `Agent` tool subprocess path,
//! `RemoteAgent` re-uses `RemoteTrigger`, and the rest are
//! placeholders that raise `NotImplemented` until their phase ships.

mod dream;
mod local_agent;
mod local_shell;
mod local_workflow;
mod monitor_mcp;
mod remote_agent;

pub use dream::DreamExecutor;
pub use local_agent::LocalAgentExecutor;
pub use local_shell::LocalShellExecutor;
pub use local_workflow::{LocalWorkflowExecutor, resolve_workflow_prompt};
pub use monitor_mcp::MonitorMcpExecutor;
pub use remote_agent::RemoteAgentExecutor;
