//! Core services layer.
//!
//! Services handle cross-cutting concerns like history compaction,
//! token estimation, MCP server management, and memory persistence.

pub mod background;
pub mod bridge;
pub mod compact;
pub mod coordinator;
pub mod diagnostics;
pub mod git;
pub mod history;
pub mod lsp;
pub mod mcp;
pub mod plugins;
pub mod session;
pub mod tokens;
