//! Unified error types for the application.
//!
//! Each subsystem defines specific error variants that compose into
//! the top-level `Error` enum. Tool errors and API errors are recoverable
//! within the agent loop; other errors propagate to the caller.

use thiserror::Error;

/// Top-level error type.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Tool(#[from] ToolError),

    #[error(transparent)]
    Permission(#[from] PermissionError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

/// LLM API errors.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error (status {status}): {body}")]
    Api { status: u16, body: String },

    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("Stream interrupted")]
    StreamInterrupted,

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Authentication failed: {0}")]
    AuthError(String),

    #[error("Context window exceeded ({tokens} tokens)")]
    ContextOverflow { tokens: usize },
}

/// Tool execution errors.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Tool execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Timeout after {0}ms")]
    Timeout(u64),
}

/// Permission system errors.
#[derive(Debug, Error)]
pub enum PermissionError {
    #[error("Permission denied by rule: {0}")]
    DeniedByRule(String),

    #[error("User denied permission for {tool}: {reason}")]
    UserDenied { tool: String, reason: String },
}

/// Configuration errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config file error: {0}")]
    FileError(String),

    #[error("Invalid config value: {0}")]
    InvalidValue(String),

    #[error("TOML parse error: {0}")]
    ParseError(#[from] toml::de::Error),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
