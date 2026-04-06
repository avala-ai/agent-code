//! Core services layer.
//!
//! Services handle cross-cutting concerns like history compaction,
//! token estimation, MCP server management, and memory persistence.

pub mod background;
pub mod bridge;
pub mod budget;
pub mod cache_tracking;
pub mod compact;
pub mod context_collapse;
pub mod coordinator;
pub mod diagnostics;
pub mod file_cache;
pub mod git;
pub mod git_ops;
pub mod history;
pub mod lsp;
pub mod mcp;
pub mod output_store;
pub mod plugins;
pub mod pricing;
pub mod session;
pub mod session_env;
pub mod shell_passthrough;
pub mod telemetry;
pub mod tokens;
