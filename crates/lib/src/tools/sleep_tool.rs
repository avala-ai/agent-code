//! Sleep tool: pause execution for a specified duration.
//!
//! Useful for polling loops, waiting for external processes,
//! or rate-limiting operations.

use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Maximum sleep duration: 5 minutes.
const MAX_SLEEP_MS: u64 = 300_000;

pub struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &'static str {
        "Sleep"
    }

    fn description(&self) -> &'static str {
        "Pause execution for a specified number of milliseconds (max 5 minutes)."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["duration_ms"],
            "properties": {
                "duration_ms": {
                    "type": "integer",
                    "description": "Duration to sleep in milliseconds (max 300000)"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let ms = input
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::InvalidInput("'duration_ms' is required".into()))?;

        let ms = ms.min(MAX_SLEEP_MS);

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {
                Ok(ToolResult::success(format!("Slept for {ms}ms")))
            }
            _ = ctx.cancel.cancelled() => {
                Err(ToolError::Cancelled)
            }
        }
    }
}
