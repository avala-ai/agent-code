//! MCP client: high-level interface for MCP server interactions.
//!
//! Manages the connection lifecycle, tool discovery, and tool
//! execution for a single MCP server.

use std::collections::HashMap;
use tracing::{debug, info, warn};

use super::transport::McpTransportConnection;
use super::types::*;

/// Client for a single MCP server connection.
pub struct McpClient {
    /// Server configuration.
    config: McpServerConfig,
    /// Transport connection.
    transport: Option<McpTransportConnection>,
    /// Discovered tools.
    tools: Vec<McpTool>,
    /// Discovered resources.
    resources: Vec<McpResource>,
    /// Connection status.
    status: McpConnectionStatus,
}

impl McpClient {
    /// Create a new client for the given server configuration.
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            transport: None,
            tools: Vec::new(),
            resources: Vec::new(),
            status: McpConnectionStatus::Disconnected,
        }
    }

    /// Get the server name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Get the connection status.
    pub fn status(&self) -> &McpConnectionStatus {
        &self.status
    }

    /// Get discovered tools.
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Get discovered resources.
    pub fn resources(&self) -> &[McpResource] {
        &self.resources
    }

    /// Connect to the MCP server and perform initialization.
    pub async fn connect(&mut self) -> Result<(), String> {
        self.status = McpConnectionStatus::Connecting;

        let transport = match &self.config.transport {
            McpTransport::Stdio { command, args } => {
                McpTransportConnection::connect_stdio(command, args, &self.config.env).await?
            }
            McpTransport::Sse { url } => {
                McpTransportConnection::connect_sse(url).await?
            }
        };

        // Initialize the connection.
        let init_result = transport
            .request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "resources": {}
                    },
                    "clientInfo": {
                        "name": "rc",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            )
            .await?;

        debug!("MCP server initialized: {:?}", init_result);

        // Send initialized notification.
        transport.notify("notifications/initialized", None).await?;

        self.transport = Some(transport);
        self.status = McpConnectionStatus::Connected;

        // Discover tools and resources.
        self.discover_tools().await?;
        self.discover_resources().await?;

        info!(
            "MCP server '{}' connected: {} tools, {} resources",
            self.config.name,
            self.tools.len(),
            self.resources.len()
        );

        Ok(())
    }

    /// Discover available tools from the server.
    async fn discover_tools(&mut self) -> Result<(), String> {
        let transport = self
            .transport
            .as_ref()
            .ok_or("Not connected")?;

        let result = transport.request("tools/list", None).await?;

        if let Some(tools) = result.get("tools").and_then(|v| v.as_array()) {
            self.tools = tools
                .iter()
                .filter_map(|t| serde_json::from_value(t.clone()).ok())
                .collect();
        }

        Ok(())
    }

    /// Discover available resources from the server.
    async fn discover_resources(&mut self) -> Result<(), String> {
        let transport = self
            .transport
            .as_ref()
            .ok_or("Not connected")?;

        match transport.request("resources/list", None).await {
            Ok(result) => {
                if let Some(resources) = result.get("resources").and_then(|v| v.as_array()) {
                    self.resources = resources
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                }
            }
            Err(e) => {
                // Resources are optional — server may not support them.
                debug!("MCP server '{}' doesn't support resources: {e}", self.config.name);
            }
        }

        Ok(())
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        let transport = self
            .transport
            .as_ref()
            .ok_or("Not connected")?;

        let result = transport
            .request(
                "tools/call",
                Some(serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments,
                })),
            )
            .await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Invalid tool result: {e}"))
    }

    /// Read a resource from the MCP server.
    pub async fn read_resource(&self, uri: &str) -> Result<String, String> {
        let transport = self
            .transport
            .as_ref()
            .ok_or("Not connected")?;

        let result = transport
            .request(
                "resources/read",
                Some(serde_json::json!({ "uri": uri })),
            )
            .await?;

        // Extract text content from the response.
        if let Some(contents) = result.get("contents").and_then(|v| v.as_array()) {
            let text: Vec<&str> = contents
                .iter()
                .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                .collect();
            Ok(text.join("\n"))
        } else {
            Ok(result.to_string())
        }
    }

    /// Disconnect from the MCP server.
    pub async fn disconnect(&mut self) {
        if let Some(transport) = self.transport.take() {
            transport.shutdown().await;
        }
        self.tools.clear();
        self.resources.clear();
        self.status = McpConnectionStatus::Disconnected;
    }
}
