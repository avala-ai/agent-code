//! Tool system.
//!
//! Tools are the primary way the agent interacts with the environment.
//! Each tool implements the `Tool` trait and is registered in the
//! `ToolRegistry` for dispatch by name.
//!
//! # Architecture
//!
//! - `Tool` trait — defines the interface for all tools
//! - `ToolRegistry` — collects tools and dispatches by name
//! - `ToolExecutor` — manages concurrent/serial tool execution
//! - Individual tool modules — concrete implementations
//!
//! # Tool execution flow
//!
//! 1. Input validation (schema check)
//! 2. Permission check (allow/ask/deny)
//! 3. Tool execution (`call`)
//! 4. Result mapping (to API format)

pub mod agent;
pub mod ask_user;
pub mod bash;
pub mod executor;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob;
pub mod grep;
pub mod registry;
pub mod web_fetch;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::permissions::{PermissionChecker, PermissionDecision};

/// The core trait that all tools must implement.
///
/// Tools are the bridge between the LLM's intentions and the local
/// environment. Each tool defines its input schema (for the LLM),
/// permission requirements, concurrency behavior, and execution logic.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name used in API tool_use blocks.
    fn name(&self) -> &'static str;

    /// Human-readable description sent to the LLM.
    fn description(&self) -> &'static str;

    /// System prompt instructions for this tool.
    fn prompt(&self) -> String {
        self.description().to_string()
    }

    /// JSON Schema for the tool's input parameters.
    fn input_schema(&self) -> serde_json::Value;

    /// Execute the tool with validated input.
    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, crate::error::ToolError>;

    /// Whether this tool only reads state (no mutations).
    fn is_read_only(&self) -> bool {
        false
    }

    /// Whether this tool can safely run concurrently with other tools.
    /// Read-only tools are typically concurrency-safe.
    fn is_concurrency_safe(&self) -> bool {
        self.is_read_only()
    }

    /// Whether this tool is destructive (deletes data, force-pushes, etc.).
    fn is_destructive(&self) -> bool {
        false
    }

    /// Whether this tool is currently enabled in the environment.
    fn is_enabled(&self) -> bool {
        true
    }

    /// Maximum result size in characters before truncation.
    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    /// Check permissions for executing this tool with the given input.
    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        checker: &PermissionChecker,
    ) -> PermissionDecision {
        if self.is_read_only() {
            PermissionDecision::Allow
        } else {
            checker.check(self.name(), input)
        }
    }

    /// Validate tool input before execution.
    fn validate_input(&self, _input: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// Extract a file path from the input, if applicable (for permission matching).
    fn get_path(&self, _input: &serde_json::Value) -> Option<PathBuf> {
        None
    }
}

/// Context passed to every tool during execution.
pub struct ToolContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Cancellation token for cooperative cancellation.
    pub cancel: CancellationToken,
    /// Permission checker instance.
    pub permission_checker: Arc<PermissionChecker>,
    /// Whether to produce verbose output.
    pub verbose: bool,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The main output content.
    pub content: String,
    /// Whether the result represents an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful result.
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error result.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Schema information for a tool, used when building API requests.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
}

impl<T: Tool + ?Sized> From<&T> for ToolSchema {
    fn from(tool: &T) -> Self {
        Self {
            name: tool.name(),
            description: tool.description(),
            input_schema: tool.input_schema(),
        }
    }
}
