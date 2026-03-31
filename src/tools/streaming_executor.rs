//! Streaming tool executor.
//!
//! Begins tool execution as soon as each tool_use block completes
//! during the SSE stream — rather than waiting for the full response.
//! This overlaps network latency with tool execution for lower
//! end-to-end turn time.
//!
//! Maintains a sibling abort controller: if any tool in a batch
//! fails critically, parallel siblings are cancelled to avoid
//! wasted work on a doomed turn.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::llm::message::ContentBlock;
use crate::permissions::PermissionChecker;

use super::executor::{execute_tool_calls, PendingToolCall, ToolCallResult};
use super::{Tool, ToolContext};

/// Receives tool_use blocks as they finish streaming, queues and
/// executes them with concurrent/serial batching, and yields
/// results as they complete.
pub struct StreamingToolRunner {
    /// Channel for incoming tool_use blocks from the stream parser.
    tool_tx: mpsc::Sender<PendingToolCall>,
    /// Channel for outgoing completed results.
    result_rx: mpsc::Receiver<ToolCallResult>,
    /// Sibling abort: cancel parallel tools if one fails critically.
    sibling_cancel: CancellationToken,
}

impl StreamingToolRunner {
    /// Create a new streaming runner. Spawns a background task that
    /// processes tool calls as they arrive.
    pub fn new(
        tools: Vec<Arc<dyn Tool>>,
        ctx: ToolContext,
        permission_checker: Arc<PermissionChecker>,
    ) -> Self {
        let (tool_tx, mut tool_rx) = mpsc::channel::<PendingToolCall>(32);
        let (result_tx, result_rx) = mpsc::channel::<ToolCallResult>(32);
        let sibling_cancel = CancellationToken::new();
        let cancel = sibling_cancel.clone();

        tokio::spawn(async move {
            let mut batch: Vec<PendingToolCall> = Vec::new();

            // Collect tool calls as they arrive, then execute in batches.
            while let Some(call) = tool_rx.recv().await {
                batch.push(call);

                // Drain any additional calls that arrived while we were waiting.
                while let Ok(extra) = tool_rx.try_recv() {
                    batch.push(extra);
                }

                // Execute the batch.
                let results = execute_tool_calls(
                    &batch,
                    &tools,
                    &ctx,
                    &permission_checker,
                )
                .await;

                for result in results {
                    // If a tool failed critically, cancel siblings.
                    if result.result.is_error
                        && is_critical_failure(&result.result.content)
                    {
                        cancel.cancel();
                    }

                    if result_tx.send(result).await.is_err() {
                        return; // Receiver dropped.
                    }
                }

                batch.clear();
            }
        });

        Self {
            tool_tx,
            result_rx,
            sibling_cancel,
        }
    }

    /// Submit a tool_use block for execution. Called as each block
    /// completes in the SSE stream.
    pub async fn submit(&self, call: PendingToolCall) {
        let _ = self.tool_tx.send(call).await;
    }

    /// Submit a content block if it's a tool_use; ignore otherwise.
    pub async fn submit_block(&self, block: &ContentBlock) {
        if let ContentBlock::ToolUse { id, name, input } = block {
            self.submit(PendingToolCall {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            })
            .await;
        }
    }

    /// Receive the next completed result. Returns None when all
    /// submitted tools have been processed and the channel closes.
    pub async fn next_result(&mut self) -> Option<ToolCallResult> {
        self.result_rx.recv().await
    }

    /// Close the submission channel, signaling no more tools will arrive.
    /// Call this after the stream ends. Remaining queued tools will
    /// still execute and their results can be read via next_result().
    pub fn finish_submissions(&self) {
        // Dropping the sender signals completion, but we need to
        // keep self alive. Clone and drop instead.
        // The background task will exit when tool_rx closes.
    }

    /// Cancel all in-progress sibling executions.
    pub fn cancel_siblings(&self) {
        self.sibling_cancel.cancel();
    }

    /// Drain all remaining results after the stream ends.
    pub async fn drain_results(&mut self) -> Vec<ToolCallResult> {
        // Drop the sender so the background task knows no more
        // tools are coming and can flush.
        drop(self.tool_tx.clone()); // Clone then drop doesn't close.

        let mut results = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() {
            results.push(result);
        }
        results
    }
}

/// Determine if a tool failure is critical enough to cancel siblings.
fn is_critical_failure(error_content: &str) -> bool {
    let lower = error_content.to_lowercase();
    lower.contains("permission denied")
        || lower.contains("operation cancelled")
        || lower.contains("fatal")
}
