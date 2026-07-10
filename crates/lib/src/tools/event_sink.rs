//! Live tool events (stdout chunks, etc.) for the UI sink path.
//!
//! Tools emit through a cheap unbounded channel so the query loop can
//! forward to [`crate::query::StreamSink`] while tool execution is in
//! flight — without requiring `Arc<dyn StreamSink>` or tool→query crate
//! cycles.

use tokio::sync::mpsc;

/// Events produced by tools during execution.
#[derive(Debug, Clone)]
pub enum ToolEvent {
    /// Streaming stdout/stderr (or other progressive output) for a call.
    Output { call_id: String, chunk: String },
    /// Plan content ready for user approval (ExitPlanMode).
    PlanProposed {
        plan_md: String,
        path: Option<String>,
    },
}

/// Create a tool-event channel. The receiver is drained by the query loop
/// concurrently with `execute_tool_calls`.
pub fn tool_event_channel() -> (ToolEventTx, mpsc::UnboundedReceiver<ToolEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (ToolEventTx(tx), rx)
}

/// Cloneable sender half stored on [`super::ToolContext`].
#[derive(Clone, Debug)]
pub struct ToolEventTx(mpsc::UnboundedSender<ToolEvent>);

impl ToolEventTx {
    /// Emit a progressive output chunk for `call_id`.
    pub fn emit_output(&self, call_id: &str, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let _ = self.0.send(ToolEvent::Output {
            call_id: call_id.to_string(),
            chunk: chunk.to_string(),
        });
    }

    /// Emit a plan-approval payload for the UI modal queue.
    pub fn emit_plan_proposed(&self, plan_md: &str, path: Option<&str>) {
        let _ = self.0.send(ToolEvent::PlanProposed {
            plan_md: plan_md.to_string(),
            path: path.map(str::to_string),
        });
    }
}
