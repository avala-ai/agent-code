//! Model Context Protocol (MCP) client.
//!
//! Connects to MCP servers that provide additional tools, resources,
//! and prompts. Servers can run as:
//!
//! - **stdio**: subprocess communicating via stdin/stdout JSON-RPC
//! - **sse**: HTTP server with Server-Sent Events
//!
//! # Protocol
//!
//! MCP uses JSON-RPC 2.0 over the chosen transport. The client
//! discovers available tools via `tools/list`, then proxies tool
//! calls to the server via `tools/call`.

pub mod client;
pub mod transport;
pub mod types;

pub use client::McpClient;
pub use types::*;
