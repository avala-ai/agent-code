//! Token estimation.
//!
//! Estimates token counts for messages and content blocks using a
//! character-based heuristic. Uses actual API usage data when
//! available, falling back to rough estimation for new messages.
//!
//! Default ratio: 4 bytes per token (conservative for most content).

use crate::llm::message::{ContentBlock, Message};

/// Default bytes per token for estimation.
const BYTES_PER_TOKEN: f64 = 4.0;

/// Fixed token estimate for image content blocks.
const IMAGE_TOKEN_ESTIMATE: u64 = 2000;

/// Estimate token count from a string.
pub fn estimate_tokens(content: &str) -> u64 {
    (content.len() as f64 / BYTES_PER_TOKEN).round() as u64
}

/// Estimate tokens for a single content block.
pub fn estimate_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text { text } => estimate_tokens(text),
        ContentBlock::ToolUse { name, input, .. } => {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            estimate_tokens(name) + estimate_tokens(&input_str)
        }
        ContentBlock::ToolResult { content, .. } => estimate_tokens(content),
        ContentBlock::Thinking { thinking, .. } => estimate_tokens(thinking),
        ContentBlock::Image { .. } => IMAGE_TOKEN_ESTIMATE,
    }
}

/// Estimate tokens for a single message.
pub fn estimate_message_tokens(msg: &Message) -> u64 {
    match msg {
        Message::User(u) => {
            // Per-message overhead (role, formatting).
            let overhead = 4;
            let content: u64 = u.content.iter().map(estimate_block_tokens).sum();
            overhead + content
        }
        Message::Assistant(a) => {
            let overhead = 4;
            let content: u64 = a.content.iter().map(estimate_block_tokens).sum();
            overhead + content
        }
        Message::System(s) => {
            let overhead = 4;
            overhead + estimate_tokens(&s.content)
        }
    }
}

/// Estimate total context tokens for a message history.
///
/// Uses a hybrid approach: actual API usage counts for the most
/// recent assistant response, plus rough estimation for any
/// messages added after that point.
pub fn estimate_context_tokens(messages: &[Message]) -> u64 {
    if messages.is_empty() {
        return 0;
    }

    // Find the most recent assistant message with usage data.
    let mut last_usage_idx = None;
    for (i, msg) in messages.iter().enumerate().rev() {
        if let Message::Assistant(a) = msg {
            if a.usage.is_some() {
                last_usage_idx = Some(i);
                break;
            }
        }
    }

    match last_usage_idx {
        Some(idx) => {
            // Use actual API token count up to this point.
            let usage = messages[idx]
                .as_assistant()
                .and_then(|a| a.usage.as_ref())
                .unwrap();
            let api_tokens = usage.total();

            // Estimate tokens for messages added after the API call.
            let new_tokens: u64 = messages[idx + 1..]
                .iter()
                .map(estimate_message_tokens)
                .sum();

            api_tokens + new_tokens
        }
        None => {
            // No API usage data — estimate everything.
            messages.iter().map(estimate_message_tokens).sum()
        }
    }
}

/// Get the context window size for a model.
pub fn context_window_for_model(model: &str) -> u64 {
    let lower = model.to_lowercase();

    // Check for extended context variants first.
    if lower.contains("1m") || lower.contains("1000k") {
        return 1_000_000;
    }

    if lower.contains("opus") {
        200_000
    } else if lower.contains("sonnet") {
        200_000
    } else if lower.contains("haiku") {
        200_000
    } else if lower.contains("gpt-4") {
        128_000
    } else if lower.contains("gpt-3.5") {
        16_384
    } else {
        // Conservative default.
        128_000
    }
}

/// Get the max output tokens for a model.
pub fn max_output_tokens_for_model(model: &str) -> u64 {
    let lower = model.to_lowercase();
    if lower.contains("opus") || lower.contains("sonnet") {
        16_384
    } else if lower.contains("haiku") {
        8_192
    } else {
        16_384
    }
}

/// Get the maximum thinking token budget for a model.
pub fn max_thinking_tokens_for_model(model: &str) -> u64 {
    let lower = model.to_lowercase();
    if lower.contains("opus") {
        32_000
    } else if lower.contains("sonnet") {
        16_000
    } else if lower.contains("haiku") {
        8_000
    } else {
        16_000
    }
}

// Helper to extract assistant message ref.
trait AsAssistant {
    fn as_assistant(&self) -> Option<&crate::llm::message::AssistantMessage>;
}

impl AsAssistant for Message {
    fn as_assistant(&self) -> Option<&crate::llm::message::AssistantMessage> {
        match self {
            Message::Assistant(a) => Some(a),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        // 100 chars / 4 = 25 tokens.
        let text = "a".repeat(100);
        assert_eq!(estimate_tokens(&text), 25);
    }

    #[test]
    fn test_empty_messages() {
        assert_eq!(estimate_context_tokens(&[]), 0);
    }
}
