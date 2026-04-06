//! # agent-code-lib
//!
//! The complete AI coding agent engine as a reusable library.
//!
//! This crate contains everything needed to build an AI coding agent:
//! LLM providers, tools, the query engine, memory, permissions, and
//! all supporting services. The `agent` CLI binary is a thin wrapper
//! over this library.
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`config`] | Configuration loading and schema (TOML, layered merge) |
//! | [`error`] | Unified error types (`LlmError`, `ToolError`, `ConfigError`) |
//! | [`hooks`] | Lifecycle hooks (pre/post tool use, session events) |
//! | [`llm`] | LLM communication: streaming client, message types, providers, retry |
//! | [`memory`] | Persistent context: project (AGENTS.md) and user memory |
//! | [`permissions`] | Permission system: rules, modes, protected directories |
//! | [`query`] | Agent loop: call LLM → execute tools → compact → repeat |
//! | [`services`] | Cross-cutting: tokens, compaction, sessions, git, MCP, plugins, diagnostics |
//! | [`skills`] | Custom workflow loading from markdown files |
//! | [`state`] | Session state: messages, usage tracking, cost |
//! | [`tools`] | 32 built-in tools and the `Tool` trait for custom tools |
//!
//! ## Quick Example
//!
//! ```rust,no_run
//! use agent_code_lib::config::Config;
//! use agent_code_lib::tools::registry::ToolRegistry;
//! use agent_code_lib::state::AppState;
//!
//! let config = Config::default();
//! let tools = ToolRegistry::default_tools();
//! let state = AppState::new(config);
//! // state.messages, state.total_cost_usd, etc. are now available
//! ```
//!
//! ## Adding a Custom Tool
//!
//! Implement the [`tools::Tool`] trait:
//!
//! ```rust,ignore
//! use async_trait::async_trait;
//! use agent_code_lib::tools::{Tool, ToolContext, ToolResult};
//!
//! struct MyTool;
//!
//! #[async_trait]
//! impl Tool for MyTool {
//!     fn name(&self) -> &'static str { "MyTool" }
//!     fn description(&self) -> &'static str { "Does something useful" }
//!     fn input_schema(&self) -> serde_json::Value {
//!         serde_json::json!({"type": "object", "properties": {}})
//!     }
//!     fn is_read_only(&self) -> bool { true }
//!     async fn call(
//!         &self,
//!         input: serde_json::Value,
//!         ctx: &ToolContext,
//!     ) -> Result<ToolResult, agent_code_lib::error::ToolError> {
//!         Ok(ToolResult { content: "done".into(), is_error: false })
//!     }
//! }
//! ```
//!
//! Then register it: `registry.register(Arc::new(MyTool));`

#![allow(dead_code, clippy::new_without_default, clippy::len_without_is_empty)]

pub mod config;
pub mod error;
pub mod hooks;
pub mod llm;
pub mod memory;
pub mod permissions;
pub mod query;
pub mod schedule;
pub mod services;
pub mod skills;
pub mod state;
pub mod tools;
