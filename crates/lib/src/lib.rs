//! agent-code-lib: the complete agent engine as a library.
#![allow(dead_code, clippy::new_without_default, clippy::len_without_is_empty)]
//!
//! Contains LLM providers, tools, query engine, memory, permissions,
//! and all services. The CLI binary is a thin wrapper over this.

pub mod config;
pub mod error;
pub mod hooks;
pub mod llm;
pub mod memory;
pub mod permissions;
pub mod query;
pub mod services;
pub mod skills;
pub mod state;
pub mod tools;
