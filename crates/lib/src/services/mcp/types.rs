//! MCP protocol types.

use serde::{Deserialize, Serialize};

/// Configuration for an MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Transport type.
    pub transport: McpTransport,
    /// Human-readable server name.
    pub name: String,
    /// Optional environment variables to set for the server process.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

impl McpServerConfig {
    /// Stable digest of everything that decides *what process or endpoint*
    /// this server name resolves to: the transport and its command, args,
    /// url and environment.
    ///
    /// Durable permission grants for `mcp__{server}__{tool}` embed this.
    /// The server name is mutable configuration — a branch switch or an
    /// updated project settings file can repoint `[mcp_servers.foo]` at a
    /// different command or URL — and without the binding in the key an
    /// approval given for one server would silently suppress the prompt
    /// while the input was dispatched to a replacement external process.
    ///
    /// A digest, not the values: commands, URLs and env values carry
    /// credentials (tokens in an SSE url, API keys in `env`), and the
    /// grant file lives in the config directory, where secrets must never
    /// be written.
    pub fn binding_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        let mut part = |bytes: &[u8]| {
            // Length-prefix every field so adjacent values cannot blur
            // into each other ("ab"+"c" vs "a"+"bc").
            h.update((bytes.len() as u64).to_le_bytes());
            h.update(bytes);
        };
        match &self.transport {
            McpTransport::Stdio { command, args } => {
                part(b"stdio");
                part(command.as_bytes());
                for a in args {
                    part(a.as_bytes());
                }
            }
            McpTransport::Sse { url } => {
                part(b"sse");
                part(url.as_bytes());
            }
        }
        // Sorted: `HashMap` iteration order is not stable, and a
        // fingerprint that changed between runs would re-prompt forever.
        part(b"|env|");
        let mut env: Vec<(&String, &String)> = self.env.iter().collect();
        env.sort();
        for (k, v) in env {
            part(k.as_bytes());
            part(v.as_bytes());
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Transport configuration for connecting to an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpTransport {
    /// Subprocess communicating via stdin/stdout.
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// HTTP server with Server-Sent Events.
    #[serde(rename = "sse")]
    Sse { url: String },
}

/// A tool exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name (may be prefixed with server name).
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// JSON Schema for the tool's input.
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// A resource exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// Result from calling an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

/// Content block in an MCP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource { resource: McpResource },
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Connection status of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    Error(String),
}
