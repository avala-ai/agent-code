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

/// Rough cost estimation based on model and token counts.
fn estimate_cost(usage: &Usage, model: &str) -> f64 {
    // Approximate pricing per million tokens (input, output).
    let (input_price, output_price) = if model.contains("opus") {
        (15.0, 75.0)
    } else if model.contains("sonnet") {
        (3.0, 15.0)
    } else if model.contains("haiku") {
        (0.25, 1.25)
    } else {
        (3.0, 15.0) // default to mid-tier
    };

    let input_cost = (usage.input_tokens as f64 / 1_000_000.0) * input_price;
    let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * output_price;
    let cache_write_cost =
        (usage.cache_creation_input_tokens as f64 / 1_000_000.0) * input_price * 1.25;
    let cache_read_cost = (usage.cache_read_input_tokens as f64 / 1_000_000.0) * input_price * 0.1;

    input_cost + output_cost + cache_write_cost + cache_read_cost
}
