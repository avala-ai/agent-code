//! Application state management.
//!
//! Centralized state store for the session. Tracks conversation
//! messages, active queries, costs, token usage, and UI state.

use std::collections::HashMap;

use crate::config::Config;
use crate::llm::message::{Message, Usage};

/// Preset response styles selectable via `/output-style`.
///
/// Each style injects a short instruction block into the system
/// prompt shaping the model's voice. `Default` emits nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseStyle {
    /// No style override (the codebase's default voice).
    #[default]
    Default,
    /// Shorter, fewer qualifiers — but not as strict as `/brief`.
    Concise,
    /// Explain reasoning and consider alternatives when relevant.
    Explanatory,
    /// Teach as you go: narrate what you're doing and why before
    /// each significant action. Aimed at users new to the codebase.
    Learning,
}

impl ResponseStyle {
    /// Parse a style name (case-insensitive). Returns `None` for
    /// unknown names so callers can print a usage string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "default" | "normal" | "off" => Some(Self::Default),
            "concise" | "terse" => Some(Self::Concise),
            "explanatory" | "explain" => Some(Self::Explanatory),
            "learning" | "teach" | "teacher" => Some(Self::Learning),
            _ => None,
        }
    }

    /// Display name for use in status messages / menus.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Concise => "concise",
            Self::Explanatory => "explanatory",
            Self::Learning => "learning",
        }
    }

    /// The prompt fragment this style contributes. Empty for
    /// `Default`, which emits no override block.
    pub fn prompt_fragment(&self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Concise => {
                "Prefer shorter responses with fewer qualifiers. Skip prefaces and \
                 recaps. Report results directly. When a short answer suffices, use \
                 one."
            }
            Self::Explanatory => {
                "Explain your reasoning as you go. When a decision has alternatives, \
                 briefly note the trade-off you considered and why the chosen path \
                 wins. Prioritise clarity over brevity."
            }
            Self::Learning => {
                "You are pair-programming with someone new to this codebase. Before \
                 each significant edit or tool call, narrate what you're about to do \
                 and why in plain language. Favour explanation over terseness, but \
                 keep it focused on the task at hand."
            }
        }
    }
}

/// Global application state for the session.
///
/// Tracks the conversation history, token usage, cost, model config,
/// and per-model breakdowns. Mutated by the query engine during turns
/// and read by commands like `/cost`, `/status`, `/stats`.
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
    /// When true, the next outgoing request skips prompt caching so the
    /// cache prefix is rebuilt from scratch. Consumed (reset to false)
    /// after the next request. Set by `/break-cache`.
    pub break_cache_next: bool,
    /// Extra directories the user has explicitly added to the working
    /// set with `/add-dir`. Surfaced in the system prompt so the agent
    /// knows it's allowed to read/edit files outside `cwd` without
    /// re-asking. Cleared on session exit — not persisted.
    pub additional_dirs: Vec<String>,
    /// When true, the system prompt instructs the model to keep
    /// responses terse (≤3 sentences unless asked for detail). Toggled
    /// by `/brief`. Session-local — not persisted.
    pub brief_mode: bool,
    /// Selected response style. `/output-style <name>` flips it.
    /// `Default` emits no prompt override. Session-local.
    pub response_style: ResponseStyle,
    /// Saved `config.api.model` before `/fast` swapped in the fast
    /// alternative. `Some` ⇔ currently in fast mode. Restored on the
    /// next `/fast` toggle. Session-local — not persisted.
    pub pre_fast_model: Option<String>,
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
            break_cache_next: false,
            additional_dirs: Vec::new(),
            brief_mode: false,
            response_style: ResponseStyle::default(),
            pre_fast_model: None,
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
        assert!(!state.break_cache_next);
        assert!(state.additional_dirs.is_empty());
        assert!(!state.brief_mode);
        assert_eq!(state.response_style, ResponseStyle::Default);
        assert!(state.pre_fast_model.is_none());
    }

    #[test]
    fn response_style_parses_canonical_names() {
        assert_eq!(
            ResponseStyle::from_name("default"),
            Some(ResponseStyle::Default)
        );
        assert_eq!(
            ResponseStyle::from_name("concise"),
            Some(ResponseStyle::Concise)
        );
        assert_eq!(
            ResponseStyle::from_name("explanatory"),
            Some(ResponseStyle::Explanatory),
        );
        assert_eq!(
            ResponseStyle::from_name("learning"),
            Some(ResponseStyle::Learning),
        );
    }

    #[test]
    fn response_style_parses_aliases() {
        assert_eq!(
            ResponseStyle::from_name("OFF"),
            Some(ResponseStyle::Default)
        );
        assert_eq!(
            ResponseStyle::from_name("Terse"),
            Some(ResponseStyle::Concise)
        );
        assert_eq!(
            ResponseStyle::from_name("explain"),
            Some(ResponseStyle::Explanatory),
        );
        assert_eq!(
            ResponseStyle::from_name("teach"),
            Some(ResponseStyle::Learning),
        );
    }

    #[test]
    fn response_style_rejects_unknown() {
        assert_eq!(ResponseStyle::from_name("bogus"), None);
        assert_eq!(ResponseStyle::from_name(""), None);
    }

    #[test]
    fn response_style_default_has_no_prompt_fragment() {
        assert!(ResponseStyle::Default.prompt_fragment().is_empty());
    }

    #[test]
    fn response_style_non_default_have_prompt_fragments() {
        for s in [
            ResponseStyle::Concise,
            ResponseStyle::Explanatory,
            ResponseStyle::Learning,
        ] {
            assert!(
                !s.prompt_fragment().is_empty(),
                "style {s:?} needs a fragment"
            );
        }
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
