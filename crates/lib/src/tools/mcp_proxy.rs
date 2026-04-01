//! MCP proxy tool: bridges MCP server tools into the local tool system.
//!
//! Each MCP tool discovered from a server is wrapped as a local `Tool`
//! implementation that proxies calls through the MCP client.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::services::mcp::{McpClient, McpTool};

/// A tool backed by an MCP server. Proxies `call()` to the server
/// via `tools/call` JSON-RPC and converts the response.
pub struct McpProxyTool {
    /// The MCP tool metadata (name, description, schema).
    definition: McpTool,
    /// Qualified name: `mcp__{server}__{tool}` for uniqueness.
    qualified_name: String,
    /// The MCP client connection (shared across all tools from this server).
    client: Arc<Mutex<McpClient>>,
    /// Original server name for display.
    server_name: String,
}

impl McpProxyTool {
    pub fn new(definition: McpTool, server_name: &str, client: Arc<Mutex<McpClient>>) -> Self {
        let qualified_name = format!(
            "mcp__{}__{}",
            normalize_name(server_name),
            normalize_name(&definition.name),
        );
        Self {
            definition,
            qualified_name,
            client,
            server_name: server_name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for McpProxyTool {
    fn name(&self) -> &'static str {
        // Leak the string to get a &'static str. This is fine because
        // MCP tools live for the duration of the session.
        Box::leak(self.qualified_name.clone().into_boxed_str())
    }

    fn description(&self) -> &'static str {
        let desc = self
            .definition
            .description
            .clone()
            .unwrap_or_else(|| format!("MCP tool from {}", self.server_name));
        Box::leak(desc.into_boxed_str())
    }

    fn input_schema(&self) -> serde_json::Value {
        self.definition.input_schema.clone()
    }

    fn is_read_only(&self) -> bool {
        false // We can't know — assume mutation is possible.
    }

    fn is_concurrency_safe(&self) -> bool {
        false // MCP servers may have internal state.
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let client = self.client.lock().await;

        let result = client
            .call_tool(&self.definition.name, input)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("MCP call failed: {e}")))?;

        // Convert MCP response to our ToolResult.
        let content = result
            .content
            .iter()
            .filter_map(|c| match c {
                crate::services::mcp::McpContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolResult {
            content: if content.is_empty() {
                "(no output)".to_string()
            } else {
                content
            },
            is_error: result.is_error,
        })
    }
}

/// Normalize a name for use in qualified tool names (lowercase, replace spaces/special chars).
fn normalize_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Create proxy tools from all tools discovered on an MCP server.
pub fn create_proxy_tools(
    server_name: &str,
    mcp_tools: &[McpTool],
    client: Arc<Mutex<McpClient>>,
) -> Vec<Arc<dyn Tool>> {
    mcp_tools
        .iter()
        .map(|t| {
            Arc::new(McpProxyTool::new(t.clone(), server_name, client.clone())) as Arc<dyn Tool>
        })
        .collect()
}
