//! Core services layer.
//!
//! Services handle cross-cutting concerns like history compaction,
//! token estimation, MCP server management, and memory persistence.

pub mod compact;
pub mod mcp;
pub mod tokens;
