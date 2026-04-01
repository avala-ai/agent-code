//! Telemetry and observability attributes.
//!
//! Structured attributes attached to API calls and tool executions
//! for performance monitoring and debugging. These are local-only
//! (no data is sent externally) and can be consumed by logging,
//! tracing, or exported via OpenTelemetry.

use std::collections::HashMap;
use std::time::Instant;

/// Structured attributes for a single API call or tool execution.
#[derive(Debug, Clone, Default)]
pub struct TelemetrySpan {
    pub attributes: HashMap<String, String>,
    pub start_time: Option<Instant>,
    pub end_time: Option<Instant>,
}

impl TelemetrySpan {
    pub fn new() -> Self {
        Self {
            start_time: Some(Instant::now()),
            ..Default::default()
        }
    }

    pub fn set(&mut self, key: &str, value: impl ToString) {
        self.attributes.insert(key.to_string(), value.to_string());
    }

    pub fn finish(&mut self) {
        self.end_time = Some(Instant::now());
    }

    pub fn duration_ms(&self) -> Option<u64> {
        match (self.start_time, self.end_time) {
            (Some(start), Some(end)) => Some(end.duration_since(start).as_millis() as u64),
            _ => None,
        }
    }
}

/// Common attribute keys.
pub mod attrs {
    pub const MODEL: &str = "model";
    pub const TOOL_NAME: &str = "tool.name";
    pub const TOOL_USE_ID: &str = "tool.use_id";
    pub const INPUT_TOKENS: &str = "tokens.input";
    pub const OUTPUT_TOKENS: &str = "tokens.output";
    pub const CACHE_READ_TOKENS: &str = "tokens.cache_read";
    pub const CACHE_WRITE_TOKENS: &str = "tokens.cache_write";
    pub const COST_USD: &str = "cost.usd";
    pub const TURN_NUMBER: &str = "turn.number";
    pub const SESSION_ID: &str = "session.id";
    pub const EFFORT: &str = "effort";
    pub const THINKING_MODE: &str = "thinking.mode";
    pub const TTFT_MS: &str = "ttft.ms";
    pub const DURATION_MS: &str = "duration.ms";
    pub const IS_ERROR: &str = "is_error";
    pub const ERROR_TYPE: &str = "error.type";
    pub const PERMISSION_DECISION: &str = "permission.decision";
}

/// Build telemetry attributes for an API call.
pub fn api_call_span(model: &str, turn: usize, session_id: &str) -> TelemetrySpan {
    let mut span = TelemetrySpan::new();
    span.set(attrs::MODEL, model);
    span.set(attrs::TURN_NUMBER, turn);
    span.set(attrs::SESSION_ID, session_id);
    span
}

/// Build telemetry attributes for a tool execution.
pub fn tool_span(tool_name: &str, tool_use_id: &str) -> TelemetrySpan {
    let mut span = TelemetrySpan::new();
    span.set(attrs::TOOL_NAME, tool_name);
    span.set(attrs::TOOL_USE_ID, tool_use_id);
    span
}

/// Record usage into a span.
pub fn record_usage(span: &mut TelemetrySpan, usage: &crate::llm::message::Usage) {
    span.set(attrs::INPUT_TOKENS, usage.input_tokens);
    span.set(attrs::OUTPUT_TOKENS, usage.output_tokens);
    span.set(attrs::CACHE_READ_TOKENS, usage.cache_read_input_tokens);
    span.set(attrs::CACHE_WRITE_TOKENS, usage.cache_creation_input_tokens);
}
