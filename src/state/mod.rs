//! Application state management.
//!
//! Centralized state store for the session. Tracks conversation
//! messages, active queries, costs, token usage, and UI state.

use std::collections::HashMap;

use crate::config::Config;
use crate::llm::message::{Message, Usage};

/// Global application state for the session.
pub struct AppState {
    /// Configuration snapshot.
    pub config: Config,
    /// Full conversation history.
    pub messages: Vec<Message>,
    /// Whether a query is currently in progress.
    pub is_query_active: bool,
    /// Accumulated token usage across all turns.
    pub total_usage: Usage,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Number of agent turns completed.
    pub turn_count: usize,
    /// Current working directory.
    pub cwd: String,
    /// Per-model token usage.
    pub model_usage: HashMap<String, Usage>,
    /// Whether plan mode is active (read-only tools only).
    pub plan_mode: bool,
    /// Shared background task manager.
    pub task_manager: std::sync::Arc<crate::services::background::TaskManager>,
    /// Session ID for persistence.
    pub session_id: String,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());

        Self {
            config,
            messages: Vec::new(),
            is_query_active: false,
            total_usage: Usage::default(),
            total_cost_usd: 0.0,
            turn_count: 0,
            cwd,
            model_usage: HashMap::new(),
            plan_mode: false,
            task_manager: std::sync::Arc::new(crate::services::background::TaskManager::new()),
            session_id: crate::services::session::new_session_id(),
        }
    }

    /// Record usage from a completed API call.
    pub fn record_usage(&mut self, usage: &Usage, model: &str) {
        self.total_usage.merge(usage);
        self.model_usage
            .entry(model.to_string())
            .or_default()
            .merge(usage);
        self.total_cost_usd += estimate_cost(usage, model);
    }

    /// Push a message into the conversation history.
    pub fn push_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Get the conversation history.
    pub fn history(&self) -> &[Message] {
        &self.messages
    }
}

/// Cost estimation using the per-model pricing database.
fn estimate_cost(usage: &Usage, model: &str) -> f64 {
    crate::services::pricing::calculate_cost(
        model,
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state() {
        let state = AppState::new(crate::config::Config::default());
        assert!(!state.cwd.is_empty());
        assert_eq!(state.turn_count, 0);
        assert_eq!(state.total_cost_usd, 0.0);
        assert!(state.messages.is_empty());
    }

    #[test]
    fn test_push_message() {
        let mut state = AppState::new(crate::config::Config::default());
        state.push_message(crate::llm::message::user_message("hello"));
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.history().len(), 1);
    }

    #[test]
    fn test_record_usage() {
        let mut state = AppState::new(crate::config::Config::default());
        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        state.record_usage(&usage, "claude-sonnet-4");
        assert_eq!(state.total_usage.input_tokens, 1000);
        assert_eq!(state.total_usage.output_tokens, 500);
        assert!(state.total_cost_usd > 0.0);
    }

    #[test]
    fn test_record_usage_accumulates() {
        let mut state = AppState::new(crate::config::Config::default());
        let u1 = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        let u2 = Usage {
            input_tokens: 200,
            output_tokens: 30,
            ..Default::default()
        };
        state.record_usage(&u1, "claude-sonnet-4");
        state.record_usage(&u2, "claude-sonnet-4");
        assert_eq!(state.total_usage.output_tokens, 80); // 50 + 30.
    }

    #[test]
    fn test_model_usage_tracking() {
        let mut state = AppState::new(crate::config::Config::default());
        let u1 = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        state.record_usage(&u1, "model-a");
        state.record_usage(&u1, "model-b");
        assert!(state.model_usage.contains_key("model-a"));
        assert!(state.model_usage.contains_key("model-b"));
    }
}
