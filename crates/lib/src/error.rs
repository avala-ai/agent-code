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
    Http(String),

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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Error Display formatting ----

    #[test]
    fn error_display_llm_variant_delegates_to_llm_error() {
        let err: Error = LlmError::StreamInterrupted.into();
        assert_eq!(err.to_string(), "Stream interrupted");
    }

    #[test]
    fn error_display_tool_variant_delegates_to_tool_error() {
        let err: Error = ToolError::Cancelled.into();
        assert_eq!(err.to_string(), "Operation cancelled");
    }

    #[test]
    fn error_display_permission_variant_delegates_to_permission_error() {
        let err: Error = PermissionError::DeniedByRule("no writes".into()).into();
        assert_eq!(err.to_string(), "Permission denied by rule: no writes");
    }

    #[test]
    fn error_display_config_variant_delegates_to_config_error() {
        let err: Error = ConfigError::InvalidValue("bad timeout".into()).into();
        assert_eq!(err.to_string(), "Invalid config value: bad timeout");
    }

    #[test]
    fn error_display_io_variant() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: Error = io_err.into();
        assert_eq!(err.to_string(), "file missing");
    }

    #[test]
    fn error_display_other_variant() {
        let err = Error::Other("something went wrong".into());
        assert_eq!(err.to_string(), "something went wrong");
    }

    // ---- From conversions ----

    #[test]
    fn from_llm_error_to_error() {
        let llm = LlmError::Http("connection reset".into());
        let err: Error = llm.into();
        assert!(matches!(err, Error::Llm(_)));
    }

    #[test]
    fn from_tool_error_to_error() {
        let tool = ToolError::NotFound("bash".into());
        let err: Error = tool.into();
        assert!(matches!(err, Error::Tool(_)));
    }

    #[test]
    fn from_permission_error_to_error() {
        let perm = PermissionError::DeniedByRule("rule_1".into());
        let err: Error = perm.into();
        assert!(matches!(err, Error::Permission(_)));
    }

    #[test]
    fn from_config_error_to_error() {
        let cfg = ConfigError::FileError("not found".into());
        let err: Error = cfg.into();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn from_io_error_to_error() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
    }

    // ---- LlmError Display ----

    #[test]
    fn llm_error_display_http() {
        let err = LlmError::Http("timeout".into());
        assert_eq!(err.to_string(), "HTTP request failed: timeout");
    }

    #[test]
    fn llm_error_display_api() {
        let err = LlmError::Api {
            status: 429,
            body: "too many requests".into(),
        };
        assert_eq!(err.to_string(), "API error (status 429): too many requests");
    }

    #[test]
    fn llm_error_display_rate_limited() {
        let err = LlmError::RateLimited {
            retry_after_ms: 5000,
        };
        assert_eq!(err.to_string(), "Rate limited, retry after 5000ms");
    }

    #[test]
    fn llm_error_display_stream_interrupted() {
        let err = LlmError::StreamInterrupted;
        assert_eq!(err.to_string(), "Stream interrupted");
    }

    #[test]
    fn llm_error_display_invalid_response() {
        let err = LlmError::InvalidResponse("missing field".into());
        assert_eq!(err.to_string(), "Invalid response: missing field");
    }

    #[test]
    fn llm_error_display_auth_error() {
        let err = LlmError::AuthError("invalid key".into());
        assert_eq!(err.to_string(), "Authentication failed: invalid key");
    }

    #[test]
    fn llm_error_display_context_overflow() {
        let err = LlmError::ContextOverflow { tokens: 200000 };
        assert_eq!(err.to_string(), "Context window exceeded (200000 tokens)");
    }

    // ---- ToolError Display ----

    #[test]
    fn tool_error_display_permission_denied() {
        let err = ToolError::PermissionDenied("read /etc/shadow".into());
        assert_eq!(err.to_string(), "Permission denied: read /etc/shadow");
    }

    #[test]
    fn tool_error_display_execution_failed() {
        let err = ToolError::ExecutionFailed("exit code 1".into());
        assert_eq!(err.to_string(), "Tool execution failed: exit code 1");
    }

    #[test]
    fn tool_error_display_invalid_input() {
        let err = ToolError::InvalidInput("expected JSON".into());
        assert_eq!(err.to_string(), "Invalid input: expected JSON");
    }

    #[test]
    fn tool_error_display_not_found() {
        let err = ToolError::NotFound("custom_tool".into());
        assert_eq!(err.to_string(), "Tool not found: custom_tool");
    }

    #[test]
    fn tool_error_display_io() {
        let io = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe");
        let err = ToolError::Io(io);
        assert_eq!(err.to_string(), "IO error: broken pipe");
    }

    #[test]
    fn tool_error_display_cancelled() {
        let err = ToolError::Cancelled;
        assert_eq!(err.to_string(), "Operation cancelled");
    }

    #[test]
    fn tool_error_display_timeout() {
        let err = ToolError::Timeout(30000);
        assert_eq!(err.to_string(), "Timeout after 30000ms");
    }

    #[test]
    fn tool_error_from_io_error() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let err: ToolError = io.into();
        assert!(matches!(err, ToolError::Io(_)));
    }

    // ---- PermissionError Display ----

    #[test]
    fn permission_error_display_denied_by_rule() {
        let err = PermissionError::DeniedByRule("no shell access".into());
        assert_eq!(
            err.to_string(),
            "Permission denied by rule: no shell access"
        );
    }

    #[test]
    fn permission_error_display_user_denied() {
        let err = PermissionError::UserDenied {
            tool: "Bash".into(),
            reason: "looks dangerous".into(),
        };
        assert_eq!(
            err.to_string(),
            "User denied permission for Bash: looks dangerous"
        );
    }

    // ---- ConfigError Display ----

    #[test]
    fn config_error_display_file_error() {
        let err = ConfigError::FileError("config.toml not found".into());
        assert_eq!(err.to_string(), "Config file error: config.toml not found");
    }

    #[test]
    fn config_error_display_invalid_value() {
        let err = ConfigError::InvalidValue("timeout must be positive".into());
        assert_eq!(
            err.to_string(),
            "Invalid config value: timeout must be positive"
        );
    }

    #[test]
    fn config_error_from_toml_de_error() {
        // Trigger a real TOML parse error.
        let bad_toml = "key = [unclosed";
        let toml_err = toml::from_str::<toml::Value>(bad_toml).unwrap_err();
        let err: ConfigError = toml_err.into();
        assert!(matches!(err, ConfigError::ParseError(_)));
        let display = err.to_string();
        assert!(display.starts_with("TOML parse error:"));
    }

    #[test]
    fn config_error_parse_error_propagates_to_top_level() {
        let bad_toml = "= missing key";
        let toml_err = toml::from_str::<toml::Value>(bad_toml).unwrap_err();
        let config_err: ConfigError = toml_err.into();
        let top_err: Error = config_err.into();
        assert!(matches!(top_err, Error::Config(ConfigError::ParseError(_))));
    }

    // ---- Result alias ----

    #[test]
    fn result_alias_ok() {
        let r: Result<i32> = Ok(42);
        assert!(r.is_ok());
    }

    #[test]
    fn result_alias_err() {
        let r: Result<i32> = Err(Error::Other("oops".into()));
        assert!(r.is_err());
    }
}
