//! Plan mode tools: switch between execution and planning modes.
//!
//! Plan mode restricts the agent to read-only tools, preventing
//! mutations while the user reviews and approves a plan.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Enter plan mode (read-only operations only).
pub struct EnterPlanModeTool;

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &'static str {
        "EnterPlanMode"
    }

    fn description(&self) -> &'static str {
        "Switch to plan mode. Only read-only tools will be available until \
         ExitPlanMode is called. Use this when you need to think through \
         an approach before making changes."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
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
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Plan mode is tracked in AppState; the query engine enforces it.
        Ok(ToolResult::success(
            "Entered plan mode. Only read-only tools are available. \
             Use ExitPlanMode when ready to execute.",
        ))
    }
}

/// Exit plan mode (re-enable all tools).
pub struct ExitPlanModeTool;

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &'static str {
        "ExitPlanMode"
    }

    fn description(&self) -> &'static str {
        "Exit plan mode and re-enable all tools for execution."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
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
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success(
            "Exited plan mode. All tools are now available.",
        ))
    }
}
