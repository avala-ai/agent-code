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
pub mod bash_parse;
pub mod executor;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob;
pub mod grep;
pub mod lsp_tool;
pub mod mcp_proxy;
pub mod mcp_resources;
pub mod monitor;
pub mod multi_edit;
pub mod notebook_edit;
pub mod plan_mode;
pub mod plugin_exec;
pub mod powershell;
pub mod registry;
pub mod repl_tool;
pub mod send_message;
pub mod skill_tool;
pub mod sleep_tool;
pub mod tasks;
pub mod todo_write;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod worktree;

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

/// Permission prompt response from the UI layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResponse {
    AllowOnce,
    AllowSession,
    Deny,
}

/// Trait for prompting the user for permission decisions.
/// Implemented by the CLI's UI layer; the lib engine uses this abstraction.
pub trait PermissionPrompter: Send + Sync {
    fn ask(
        &self,
        tool_name: &str,
        description: &str,
        input_preview: Option<&str>,
    ) -> PermissionResponse;
}

/// Default prompter that always allows (for non-interactive/testing).
pub struct AutoAllowPrompter;
impl PermissionPrompter for AutoAllowPrompter {
    fn ask(&self, _: &str, _: &str, _: Option<&str>) -> PermissionResponse {
        PermissionResponse::AllowOnce
    }
}

/// Context passed to every tool during execution.
///
/// Provides the working directory, cancellation token, permission
/// checker, file cache, and other shared state. Created by the
/// executor before each tool call.
pub struct ToolContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Cancellation token for cooperative cancellation.
    pub cancel: CancellationToken,
    /// Permission checker instance.
    pub permission_checker: Arc<PermissionChecker>,
    /// Whether to produce verbose output.
    pub verbose: bool,
    /// Plan mode: only read-only tools allowed.
    pub plan_mode: bool,
    /// File content cache for avoiding redundant reads.
    pub file_cache: Option<Arc<tokio::sync::Mutex<crate::services::file_cache::FileCache>>>,
    /// Permission denial tracker for reporting.
    pub denial_tracker:
        Option<Arc<tokio::sync::Mutex<crate::permissions::tracking::DenialTracker>>>,
    /// Shared background task manager.
    pub task_manager: Option<Arc<crate::services::background::TaskManager>>,
    /// Tools allowed for the rest of the session (via "Allow for session" prompt).
    pub session_allows: Option<Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>>,
    /// Permission prompter for interactive approval.
    pub permission_prompter: Option<Arc<dyn PermissionPrompter>>,
    /// Process-level sandbox executor.
    ///
    /// `None` means sandboxing is unavailable for this context
    /// (e.g. parallel read-only retry paths); subprocess-spawning tools
    /// should treat `None` as "pass through unchanged". The main query
    /// loop populates this from [`crate::config::SandboxConfig`].
    pub sandbox: Option<Arc<crate::sandbox::SandboxExecutor>>,
}

/// Result of a tool execution.
///
/// Contains the output text and whether it represents an error.
/// Injected into the conversation as a `ToolResult` content block.
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
