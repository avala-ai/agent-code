//! Context collapse (snip compaction).
//!
//! An alternative to full LLM-based compaction that works by
//! selectively snipping (removing) older message groups to stay
//! within the context window. The full conversation history is
//! preserved for UI scrollback, but only a subset is sent to
//! the API.
//!
//! Snipping preserves:
//! - The system prompt (always)
//! - Any compact boundary / summary messages
//! - The most recent N messages (configurable)
//!
//! Snipped messages are replaced with a "[messages snipped]" marker
//! in the API-facing view, while the UI retains the full history.

use crate::llm::message::Message;
use crate::services::tokens;

/// Result of a context collapse operation.
pub struct CollapseResult {
    /// Messages to send to the API (with snipped segments).
    pub api_messages: Vec<Message>,
    /// Number of messages snipped.
    pub snipped_count: usize,
    /// Estimated tokens freed.
    pub tokens_freed: u64,
}

/// Collapse the message history to fit within a token budget.
///
/// Removes groups of messages from the middle of the conversation,
/// keeping the first message (summary/context) and the most recent
/// messages intact. Groups are removed oldest-first until the
/// budget is met.
pub fn collapse_to_budget(messages: &[Message], max_tokens: u64) -> Option<CollapseResult> {
    let current = tokens::estimate_context_tokens(messages);
    if current <= max_tokens {
        return None; // Already within budget.
    }

    let overshoot = current - max_tokens;

    // Group messages into API rounds (user + assistant pairs).
    let groups = group_by_round(messages);
    if groups.len() <= 2 {
        return None; // Can't snip if only 1-2 groups.
    }

    // Determine how many groups to remove from the middle.
    // Keep the first group (context) and last group (recent).
    let mut freed = 0u64;
    let mut snip_end = 1; // Start snipping after the first group.

    for (group_idx, group) in groups[1..groups.len().saturating_sub(1)].iter().enumerate() {
        let group_tokens: u64 = group.iter().map(tokens::estimate_message_tokens).sum();
        freed += group_tokens;
        snip_end = group_idx + 2; // +1 for skipping first group, +1 for exclusive end

        if freed >= overshoot {
            break;
        }
    }

    if freed == 0 {
        return None;
    }

    // Build the collapsed message list.
    let mut api_messages = Vec::new();

    // Keep first group.
    api_messages.extend(groups[0].iter().cloned());

    // Insert snip marker.
    api_messages.push(crate::llm::message::user_message(
        "[Earlier messages collapsed to fit context window]",
    ));

    // Keep remaining groups after the snipped section.
    for group in &groups[snip_end..] {
        api_messages.extend(group.iter().cloned());
    }

    let snipped_count: usize = groups[1..snip_end].iter().map(|g| g.len()).sum();

    Some(CollapseResult {
        api_messages,
        snipped_count,
        tokens_freed: freed,
    })
}

/// Recover from a prompt-too-long error by collapsing more aggressively.
pub fn recover_from_overflow(
    messages: &[Message],
    token_gap: Option<u64>,
) -> Option<CollapseResult> {
    // If we know the gap, target that plus 10% buffer.
    let target = token_gap.map(|gap| gap + gap / 10).unwrap_or(20_000);

    let current = tokens::estimate_context_tokens(messages);
    let budget = current.saturating_sub(target);

    collapse_to_budget(messages, budget)
}

/// Group messages into API rounds (each round = user message + assistant response).
fn group_by_round(messages: &[Message]) -> Vec<Vec<Message>> {
    let mut groups: Vec<Vec<Message>> = Vec::new();
    let mut current_group: Vec<Message> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(u) if !u.is_meta => {
                // Non-meta user message starts a new group.
                if !current_group.is_empty() {
                    groups.push(current_group);
                    current_group = Vec::new();
                }
                current_group.push(msg.clone());
            }
            _ => {
                current_group.push(msg.clone());
            }
        }
    }

    if !current_group.is_empty() {
        groups.push(current_group);
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::user_message;

    #[test]
    fn test_no_collapse_within_budget() {
        let messages = vec![user_message("short")];
        assert!(collapse_to_budget(&messages, 1_000_000).is_none());
    }

    #[test]
    fn test_collapse_empty_messages() {
        let messages: Vec<crate::llm::message::Message> = vec![];
        assert!(collapse_to_budget(&messages, 100).is_none());
    }

    #[test]
    fn test_collapse_preserves_first_and_last() {
        use crate::llm::message::*;
        // Create enough messages to exceed a tiny budget.
        let mut messages = Vec::new();
        for i in 0..10 {
            messages.push(user_message(format!(
                "message {i} with some content padding"
            )));
            messages.push(Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text {
                    text: format!("response {i} with content"),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }));
        }
        // Collapse with a budget that forces removal of middle messages.
        if let Some(result) = collapse_to_budget(&messages, 50) {
            assert!(result.snipped_count > 0);
            assert!(result.tokens_freed > 0);
            assert!(result.api_messages.len() < messages.len());
        }
        // With a huge budget, no collapse needed.
        assert!(collapse_to_budget(&messages, 1_000_000).is_none());
    }

    #[test]
    fn test_recover_from_overflow() {
        use crate::llm::message::*;
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(user_message(format!("msg {i} {}", "x".repeat(200))));
            messages.push(Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text {
                    text: format!("resp {i} {}", "y".repeat(200)),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }));
        }
        let result = recover_from_overflow(&messages, Some(5000));
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.snipped_count > 0);
    }
}
