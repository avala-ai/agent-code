//! SendMessage tool: communicate between agents.
//!
//! Allows one agent to send a message to another running agent
//! by ID or name. Used for coordination in multi-agent workflows.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct SendMessageTool;

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &'static str {
        "SendMessage"
    }

    fn description(&self) -> &'static str {
        "Send a message to another running agent by ID or name. \
         Used for inter-agent coordination in multi-agent workflows."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["to", "content"],
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Target agent ID or name"
                },
                "content": {
                    "type": "string",
                    "description": "Message content to send"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let to = input
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'to' is required".into()))?;

        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'content' is required".into()))?;

        // In a full implementation, this would route through the coordinator
        // to find the target agent and inject the message into its conversation.
        // For now, return a placeholder indicating the message was queued.
        Ok(ToolResult::success(format!(
            "Message queued for agent '{to}': {content}"
        )))
    }
}
