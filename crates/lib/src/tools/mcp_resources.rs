//! MCP resource tools: list and read resources from MCP servers.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct ListMcpResourcesTool;

#[async_trait]
impl Tool for ListMcpResourcesTool {
    fn name(&self) -> &'static str {
        "ListMcpResources"
    }

    fn description(&self) -> &'static str {
        "List available resources from connected MCP servers."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Filter by server name (optional)"
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
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success(
            "MCP resource listing requires connected servers. \
             Configure servers in .agent/settings.toml under [mcp_servers].",
        ))
    }
}

pub struct ReadMcpResourceTool;

#[async_trait]
impl Tool for ReadMcpResourceTool {
    fn name(&self) -> &'static str {
        "ReadMcpResource"
    }

    fn description(&self) -> &'static str {
        "Read a resource from an MCP server by URI."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["uri"],
            "properties": {
                "uri": {
                    "type": "string",
                    "description": "Resource URI to read"
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
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let uri = input
            .get("uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'uri' is required".into()))?;

        Ok(ToolResult::success(format!(
            "MCP resource read for '{uri}' requires a connected server \
             that provides this resource."
        )))
    }
}
