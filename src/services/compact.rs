//! History compaction.
//!
//! Manages conversation history size by summarizing older messages
//! when the context window limit approaches. Implements three
//! compaction strategies:
//!
//! - **Auto-compact**: triggered when estimated tokens exceed threshold
//! - **Reactive compact**: triggered by API `prompt_too_long` errors
//! - **Microcompact**: clears stale tool results to free tokens
//!
//! # Thresholds
//!
//! ```text
//! |<--- context window (e.g., 200K) -------------------------------->|
//! |<--- effective window (context - 20K reserved) ------------------>|
//! |<--- auto-compact threshold (effective - 13K buffer) ------------>|
//! |                                                    ↑ compact fires here
//! ```

use crate::llm::message::{
    ContentBlock, Message, SystemMessage, SystemMessageType, MessageLevel, UserMessage,
};
use crate::services::tokens;
use uuid::Uuid;

/// Buffer tokens before auto-compact fires.
const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;

/// Tokens reserved for the compact summary output.
const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;

/// Maximum consecutive auto-compact failures before circuit breaker trips.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Maximum recovery attempts for max-output-tokens errors.
pub const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: u32 = 3;

/// Tools whose results can be cleared by microcompact.
const COMPACTABLE_TOOLS: &[&str] = &[
    "FileRead", "Bash", "Grep", "Glob", "FileEdit", "FileWrite",
];

/// Token warning state for the UI.
#[derive(Debug, Clone)]
pub struct TokenWarningState {
    /// Percentage of context window remaining.
    pub percent_left: u64,
    /// Whether to show a warning in the UI.
    pub is_above_warning: bool,
    /// Whether to show an error in the UI.
    pub is_above_error: bool,
    /// Whether auto-compact should fire.
    pub should_compact: bool,
    /// Whether the context is at the blocking limit.
    pub is_blocking: bool,
}

/// Tracking state for auto-compact across turns.
#[derive(Debug, Clone, Default)]
pub struct CompactTracking {
    pub consecutive_failures: u32,
    pub was_compacted: bool,
}

/// Calculate the effective context window (total minus output reservation).
pub fn effective_context_window(model: &str) -> u64 {
    let context = tokens::context_window_for_model(model);
    let reserved = tokens::max_output_tokens_for_model(model).min(MAX_OUTPUT_TOKENS_FOR_SUMMARY);
    context.saturating_sub(reserved)
}

/// Calculate the auto-compact threshold.
pub fn auto_compact_threshold(model: &str) -> u64 {
    effective_context_window(model).saturating_sub(AUTOCOMPACT_BUFFER_TOKENS)
}

/// Calculate token warning state for the current conversation.
pub fn token_warning_state(messages: &[Message], model: &str) -> TokenWarningState {
    let token_count = tokens::estimate_context_tokens(messages);
    let threshold = auto_compact_threshold(model);
    let effective = effective_context_window(model);

    let percent_left = if effective > 0 {
        ((effective.saturating_sub(token_count)) as f64 / effective as f64 * 100.0)
            .round()
            .max(0.0) as u64
    } else {
        0
    };

    let warning_buffer = 20_000;

    TokenWarningState {
        percent_left,
        is_above_warning: token_count >= effective.saturating_sub(warning_buffer),
        is_above_error: token_count >= effective.saturating_sub(warning_buffer),
        should_compact: token_count >= threshold,
        is_blocking: token_count >= effective.saturating_sub(3_000),
    }
}

/// Check whether auto-compact should fire for this conversation.
pub fn should_auto_compact(
    messages: &[Message],
    model: &str,
    tracking: &CompactTracking,
) -> bool {
    // Circuit breaker.
    if tracking.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
        return false;
    }

    let state = token_warning_state(messages, model);
    state.should_compact
}

/// Perform microcompact: clear stale tool results to free tokens.
///
/// Replaces the content of old tool_result blocks with a placeholder,
/// keeping the most recent `keep_recent` results intact.
pub fn microcompact(messages: &mut [Message], keep_recent: usize) -> u64 {
    let keep_recent = keep_recent.max(1);

    // Collect indices of compactable tool results (in order).
    let mut compactable_indices: Vec<(usize, usize)> = Vec::new(); // (msg_idx, block_idx)

    for (msg_idx, msg) in messages.iter().enumerate() {
        if let Message::User(u) = msg {
            for (block_idx, block) in u.content.iter().enumerate() {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    // Check if this tool_use_id corresponds to a compactable tool.
                    if is_compactable_tool_result(messages, tool_use_id) {
                        compactable_indices.push((msg_idx, block_idx));
                    }
                }
            }
        }
    }

    if compactable_indices.len() <= keep_recent {
        return 0;
    }

    // Clear all but the most recent `keep_recent`.
    let clear_count = compactable_indices.len() - keep_recent;
    let to_clear = &compactable_indices[..clear_count];

    let mut freed_tokens = 0u64;

    for &(msg_idx, block_idx) in to_clear {
        if let Message::User(ref mut u) = messages[msg_idx] {
            if let ContentBlock::ToolResult {
                ref mut content,
                tool_use_id: _,
                is_error: _,
            } = u.content[block_idx]
            {
                let old_tokens = tokens::estimate_tokens(content);
                let placeholder = "[Old tool result cleared]".to_string();
                let new_tokens = tokens::estimate_tokens(&placeholder);
                *content = placeholder;
                freed_tokens += old_tokens.saturating_sub(new_tokens);
            }
        }
    }

    freed_tokens
}

/// Check if a tool_use_id corresponds to a compactable tool.
fn is_compactable_tool_result(messages: &[Message], tool_use_id: &str) -> bool {
    for msg in messages {
        if let Message::Assistant(a) = msg {
            for block in &a.content {
                if let ContentBlock::ToolUse { id, name, .. } = block {
                    if id == tool_use_id {
                        return COMPACTABLE_TOOLS
                            .iter()
                            .any(|t| t.eq_ignore_ascii_case(name));
                    }
                }
            }
        }
    }
    false
}

/// Create a compact boundary marker message.
pub fn compact_boundary_message(summary: &str) -> Message {
    Message::System(SystemMessage {
        uuid: Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        subtype: SystemMessageType::CompactBoundary,
        content: format!("[Conversation compacted. Summary: {summary}]"),
        level: MessageLevel::Info,
    })
}

/// Build a compact summary request: asks the LLM to summarize
/// the conversation up to a certain point.
pub fn build_compact_summary_prompt(messages: &[Message]) -> String {
    let mut context = String::new();
    for msg in messages {
        match msg {
            Message::User(u) => {
                context.push_str("User: ");
                for block in &u.content {
                    if let ContentBlock::Text { text } = block {
                        context.push_str(text);
                    }
                }
                context.push('\n');
            }
            Message::Assistant(a) => {
                context.push_str("Assistant: ");
                for block in &a.content {
                    if let ContentBlock::Text { text } = block {
                        context.push_str(text);
                    }
                }
                context.push('\n');
            }
            _ => {}
        }
    }

    format!(
        "Summarize this conversation concisely, preserving key decisions, \
         file changes made, and important context. Focus on what the user \
         was trying to accomplish and what was done.\n\n{context}"
    )
}

/// Build the recovery message injected when max-output-tokens is hit.
pub fn max_output_recovery_message() -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        content: vec![ContentBlock::Text {
            text: "Output token limit hit. Resume directly — no apology, no recap \
                   of what you were doing. Pick up mid-thought if that is where the \
                   cut happened. Break remaining work into smaller pieces."
                .to_string(),
        }],
        is_meta: true,
        is_compact_summary: false,
    })
}

/// Parse a "prompt too long" error to extract the token gap.
///
/// Looks for patterns like "prompt is too long: 137500 tokens > 135000 maximum"
/// and returns the difference (2500 in this example).
pub fn parse_prompt_too_long_gap(error_text: &str) -> Option<u64> {
    let re = regex::Regex::new(r"(\d+)\s*tokens?\s*>\s*(\d+)").ok()?;
    let captures = re.captures(error_text)?;
    let actual: u64 = captures.get(1)?.as_str().parse().ok()?;
    let limit: u64 = captures.get(2)?.as_str().parse().ok()?;
    let gap = actual.saturating_sub(limit);
    if gap > 0 { Some(gap) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_compact_threshold() {
        // Sonnet: 200K context, 16K max output (capped at 20K), effective = 180K
        // Threshold = 180K - 13K = 167K
        let threshold = auto_compact_threshold("claude-sonnet");
        assert_eq!(threshold, 200_000 - 16_384 - 13_000);
    }

    #[test]
    fn test_parse_prompt_too_long_gap() {
        let msg = "prompt is too long: 137500 tokens > 135000 maximum";
        assert_eq!(parse_prompt_too_long_gap(msg), Some(2500));
    }

    #[test]
    fn test_parse_prompt_too_long_no_match() {
        assert_eq!(parse_prompt_too_long_gap("some other error"), None);
    }
}
