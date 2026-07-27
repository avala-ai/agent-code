//! MCP proxy tool: bridges MCP server tools into the local tool system.
//!
//! Each MCP tool discovered from a server is wrapped as a local `Tool`
//! implementation that proxies calls through the MCP client.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{GrantBinding, Tool, ToolContext, ToolResult};
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
    /// Digest of the server's transport configuration, captured at
    /// registration. Durable grants key on it so an approval does not
    /// follow the server *name* onto a different command or endpoint.
    binding: GrantBinding,
}

impl McpProxyTool {
    pub fn new(
        definition: McpTool,
        server_name: &str,
        client: Arc<Mutex<McpClient>>,
        binding: GrantBinding,
    ) -> Self {
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
            binding,
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

    fn grant_binding(&self) -> Option<GrantBinding> {
        Some(self.binding.clone())
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
///
/// `binding` is [`crate::services::mcp::McpServerConfig::binding_fingerprint`]
/// for the server these tools came from; it enters the durable grant key
/// so a saved approval cannot carry over to a server that has since been
/// repointed at a different command or URL.
pub fn create_proxy_tools(
    server_name: &str,
    mcp_tools: &[McpTool],
    client: Arc<Mutex<McpClient>>,
    binding: &GrantBinding,
) -> Vec<Arc<dyn Tool>> {
    mcp_tools
        .iter()
        .map(|t| {
            Arc::new(McpProxyTool::new(
                t.clone(),
                server_name,
                client.clone(),
                binding.clone(),
            )) as Arc<dyn Tool>
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::mcp::{McpServerConfig, McpTransport};

    fn server(command: &str) -> McpServerConfig {
        McpServerConfig {
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: vec![],
            },
            name: "foo".to_string(),
            env: std::collections::HashMap::new(),
        }
    }

    fn proxy(digest: &str) -> McpProxyTool {
        let definition = McpTool {
            name: "query".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let client = Arc::new(Mutex::new(McpClient::new(server("/usr/bin/foo-mcp"))));
        McpProxyTool::new(
            definition,
            "foo",
            client,
            GrantBinding {
                digest: digest.to_string(),
                cwd_sensitive: true,
            },
        )
    }

    /// The permission system only ever sees the flattened
    /// `mcp__foo__query`, which is mutable configuration. The proxy must
    /// surface the server it was registered against, or a durable grant
    /// cannot tell one binding from another.
    #[test]
    fn a_proxy_reports_the_binding_it_was_registered_with() {
        let original =
            server("/usr/bin/foo-mcp").binding_fingerprint(std::path::Path::new("/test-cwd"));
        let swapped =
            server("/tmp/evil-mcp").binding_fingerprint(std::path::Path::new("/test-cwd"));
        assert_ne!(original, swapped, "precondition: the bindings differ");

        let tool = proxy(&original);
        assert_eq!(tool.name(), "mcp__foo__query");
        assert_eq!(
            tool.grant_binding().map(|b| b.digest),
            Some(original.clone())
        );
        assert_ne!(proxy(&swapped).grant_binding(), tool.grant_binding());
    }
}
