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
    ContentBlock, Message, MessageLevel, SystemMessage, SystemMessageType, UserMessage,
};
use crate::services::{secret_masker, tokens};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Number of recent turns during which file reads are locked at `Full`
/// fidelity and cannot be compressed by the summarizer.
pub const PROTECTED_TURN_WINDOW: usize = 2;

/// Fidelity of a file's representation in conversation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionLevel {
    /// Complete file contents in context (recently read).
    Full,
    /// Key sections only — functions referenced, changed lines.
    Partial,
    /// LLM-generated 2-3 sentence summary of the file's role.
    Summary,
    /// File removed from context entirely.
    Excluded,
}

/// Per-file tracking record used by the history compressor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCompressionRecord {
    pub path: PathBuf,
    pub level: CompressionLevel,
    /// 12-byte SHA256 slice — short enough to be cheap, long enough
    /// to detect any real content change.
    #[serde(with = "hex_hash")]
    pub content_hash: [u8; 12],
    /// Line range retained at `Partial` level, if any.
    pub line_range: Option<(usize, usize)>,
    /// Turn index where this file was last referenced by any tool.
    pub last_referenced_turn: usize,
}

impl FileCompressionRecord {
    /// True while the file is within the protected turn window.
    pub fn is_protected(&self, current_turn: usize) -> bool {
        current_turn.saturating_sub(self.last_referenced_turn) < PROTECTED_TURN_WINDOW
    }
}

/// Set of file compression records tracked for a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileCompressionState {
    pub files: HashMap<PathBuf, FileCompressionRecord>,
}

impl FileCompressionState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a file read at the given turn, setting level to `Full`.
    /// If the content has changed since the last record, the level is
    /// reset to `Full` (stale summaries must be discarded). If unchanged,
    /// the existing level is preserved but the turn marker is updated.
    pub fn record_read(&mut self, path: &Path, content: &str, turn: usize) {
        let hash = hash_content(content);
        match self.files.get_mut(path) {
            Some(existing) => {
                if existing.content_hash != hash {
                    existing.content_hash = hash;
                    existing.level = CompressionLevel::Full;
                    existing.line_range = None;
                }
                existing.last_referenced_turn = turn;
            }
            None => {
                self.files.insert(
                    path.to_path_buf(),
                    FileCompressionRecord {
                        path: path.to_path_buf(),
                        level: CompressionLevel::Full,
                        content_hash: hash,
                        line_range: None,
                        last_referenced_turn: turn,
                    },
                );
            }
        }
    }

    /// Demote a file's compression level, unless it is currently protected.
    pub fn demote(&mut self, path: &Path, level: CompressionLevel, current_turn: usize) -> bool {
        if let Some(rec) = self.files.get_mut(path) {
            if rec.is_protected(current_turn) {
                return false;
            }
            rec.level = level;
            return true;
        }
        false
    }

    /// Persist the state to the standard compression_state.json path
    /// next to a session file.
    pub fn save(&self, session_id: &str) -> Result<PathBuf, String> {
        let path = compression_state_path(session_id)
            .ok_or_else(|| "Could not determine cache dir".to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create compression state dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize compression state: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("write compression state: {e}"))?;
        Ok(path)
    }

    /// Load state from disk for a session id. Returns `None` if no
    /// state file exists yet (fresh session).
    pub fn load(session_id: &str) -> Option<Self> {
        let path = compression_state_path(session_id)?;
        if !path.exists() {
            return None;
        }
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }
}

/// Compute a 12-byte SHA256 slice of `content` for change detection.
pub fn hash_content(content: &str) -> [u8; 12] {
    let digest = Sha256::digest(content.as_bytes());
    let mut out = [0u8; 12];
    out.copy_from_slice(&digest[..12]);
    out
}

/// Path to the compression state sidecar file for a session.
fn compression_state_path(session_id: &str) -> Option<PathBuf> {
    dirs::cache_dir().map(|d| {
        d.join("agent-code")
            .join("sessions")
            .join(format!("{session_id}.compression.json"))
    })
}

/// Serde helper: store `[u8; 12]` as a 24-char lowercase hex string.
mod hex_hash {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 12], ser: S) -> Result<S::Ok, S::Error> {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        ser.serialize_str(&hex)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<[u8; 12], D::Error> {
        let s = String::deserialize(de)?;
        if s.len() != 24 {
            return Err(serde::de::Error::custom("expected 24 hex chars"));
        }
        let mut out = [0u8; 12];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let byte = u8::from_str_radix(std::str::from_utf8(chunk).unwrap_or(""), 16)
                .map_err(serde::de::Error::custom)?;
            out[i] = byte;
        }
        Ok(out)
    }
}

/// Buffer tokens before auto-compact fires.
const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;

/// Tokens reserved for the compact summary output.
const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;

/// Maximum consecutive auto-compact failures before circuit breaker trips.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Maximum recovery attempts for max-output-tokens errors.
pub const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: u32 = 3;

/// Tools whose results can be cleared by microcompact.
const COMPACTABLE_TOOLS: &[&str] = &["FileRead", "Bash", "Grep", "Glob", "FileEdit", "FileWrite"];

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
pub fn should_auto_compact(messages: &[Message], model: &str, tracking: &CompactTracking) -> bool {
    // Circuit breaker.
    if tracking.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
        return false;
    }

    let state = token_warning_state(messages, model);
    state.should_compact
}

/// Estimate how many tokens a microcompact would free, without
/// touching the history. Mirrors the traversal of `microcompact` so
/// the number the user is told to expect matches what will happen.
pub fn estimate_compactable_tokens(messages: &[Message], keep_recent: usize) -> u64 {
    let keep_recent = keep_recent.max(1);
    let mut compactable: Vec<(usize, usize)> = Vec::new();
    for (msg_idx, msg) in messages.iter().enumerate() {
        if let Message::User(u) = msg {
            for (block_idx, block) in u.content.iter().enumerate() {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block
                    && is_compactable_tool_result(messages, tool_use_id)
                {
                    compactable.push((msg_idx, block_idx));
                }
            }
        }
    }
    if compactable.len() <= keep_recent {
        return 0;
    }
    let clear_count = compactable.len() - keep_recent;
    let placeholder = "[Old tool result cleared]";
    let new_tokens = tokens::estimate_tokens(placeholder);
    let mut freed = 0u64;
    for &(msg_idx, block_idx) in &compactable[..clear_count] {
        if let Message::User(u) = &messages[msg_idx]
            && let ContentBlock::ToolResult { content, .. } = &u.content[block_idx]
        {
            let old_tokens = tokens::estimate_tokens(content);
            freed += old_tokens.saturating_sub(new_tokens);
        }
    }
    freed
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
        if let Message::User(ref mut u) = messages[msg_idx]
            && let ContentBlock::ToolResult {
                ref mut content,
                tool_use_id: _,
                is_error: _,
                ..
            } = u.content[block_idx]
        {
            let old_tokens = tokens::estimate_tokens(content);
            let placeholder = "[Old tool result cleared]".to_string();
            let new_tokens = tokens::estimate_tokens(&placeholder);
            *content = placeholder;
            freed_tokens += old_tokens.saturating_sub(new_tokens);
        }
    }

    freed_tokens
}

/// Check if a tool_use_id corresponds to a compactable tool.
fn is_compactable_tool_result(messages: &[Message], tool_use_id: &str) -> bool {
    for msg in messages {
        if let Message::Assistant(a) = msg {
            for block in &a.content {
                if let ContentBlock::ToolUse { id, name, .. } = block
                    && id == tool_use_id
                {
                    return COMPACTABLE_TOOLS
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case(name));
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
///
/// All message text is run through [`secret_masker`] before being
/// passed to the summarizer, so secrets that appeared in tool output
/// never end up baked into the summary.
pub fn build_compact_summary_prompt(messages: &[Message]) -> String {
    let mut context = String::new();
    for msg in messages {
        match msg {
            Message::User(u) => {
                context.push_str("User: ");
                for block in &u.content {
                    if let ContentBlock::Text { text } = block {
                        context.push_str(&secret_masker::mask(text));
                    }
                }
                context.push('\n');
            }
            Message::Assistant(a) => {
                context.push_str("Assistant: ");
                for block in &a.content {
                    if let ContentBlock::Text { text } = block {
                        context.push_str(&secret_masker::mask(text));
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

/// Perform full LLM-based compaction of the conversation history.
///
/// Splits the message history into two parts: messages to summarize
/// (older) and messages to keep (recent). Calls the LLM to generate
/// a summary, then replaces the old messages with:
/// 1. A compact boundary marker
/// 2. A summary message (as a user message with is_compact_summary=true)
/// 3. The kept recent messages
///
/// Returns the number of messages removed, or None if compaction failed.
pub async fn compact_with_llm(
    messages: &mut Vec<Message>,
    llm: &dyn crate::llm::provider::Provider,
    model: &str,
    cancel: tokio_util::sync::CancellationToken,
) -> Option<usize> {
    if messages.len() < 4 {
        return None; // Not enough messages to compact.
    }

    // Keep the most recent messages (at least 40K tokens worth, or
    // minimum 5 messages with text content).
    let keep_count = calculate_keep_count(messages);
    let split_point = messages.len().saturating_sub(keep_count);

    if split_point < 2 {
        return None; // Not enough to summarize.
    }

    let to_summarize = &messages[..split_point];
    let summary_prompt = build_compact_summary_prompt(to_summarize);

    // Call the LLM to generate the summary.
    let summary_messages = vec![crate::llm::message::user_message(&summary_prompt)];
    let request = crate::llm::provider::ProviderRequest {
        messages: summary_messages,
        system_prompt: "You are a conversation summarizer. Produce a concise summary \
                        preserving key decisions, file changes, and important context. \
                        Do not use tools."
            .to_string(),
        tools: vec![],
        model: model.to_string(),
        max_tokens: 4096,
        temperature: None,
        enable_caching: false,
        tool_choice: Default::default(),
        metadata: None,
        cancel,
    };

    let mut rx = match llm.stream(&request).await {
        Ok(rx) => rx,
        Err(e) => {
            tracing::warn!("Compact LLM call failed: {e}");
            return None;
        }
    };

    // Collect the summary text.
    let mut summary = String::new();
    while let Some(event) = rx.recv().await {
        if let crate::llm::stream::StreamEvent::TextDelta(text) = event {
            summary.push_str(&text);
        }
    }

    if summary.is_empty() {
        return None;
    }

    // Replace old messages with boundary + summary + kept messages.
    let kept = messages[split_point..].to_vec();
    let removed = split_point;

    messages.clear();
    messages.push(compact_boundary_message(&summary));
    messages.push(Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        content: vec![ContentBlock::Text {
            text: format!("[Conversation compacted. Prior context summary:]\n\n{summary}"),
        }],
        is_meta: true,
        is_compact_summary: true,
    }));
    messages.extend(kept);

    tracing::info!("Compacted {removed} messages into summary");
    Some(removed)
}

/// Calculate how many recent messages to keep during compaction.
///
/// Keeps at least 5 messages with text content, or messages totaling
/// at least 10K estimated tokens, whichever is more.
fn calculate_keep_count(messages: &[Message]) -> usize {
    let min_text_messages = 5;
    let min_tokens = 10_000u64;
    let max_tokens = 40_000u64;

    let mut count = 0usize;
    let mut text_count = 0usize;
    let mut token_total = 0u64;

    // Walk backwards from the end.
    for msg in messages.iter().rev() {
        let tokens = crate::services::tokens::estimate_message_tokens(msg);
        token_total += tokens;
        count += 1;

        // Count messages with text content.
        let has_text = match msg {
            Message::User(u) => u
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { .. })),
            Message::Assistant(a) => a
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { .. })),
            _ => false,
        };
        if has_text {
            text_count += 1;
        }

        // Stop if we've met both minimums.
        if text_count >= min_text_messages && token_total >= min_tokens {
            break;
        }
        // Hard cap.
        if token_total >= max_tokens {
            break;
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_content_detects_change() {
        let a = hash_content("hello world");
        let b = hash_content("hello world");
        let c = hash_content("hello world!");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn file_record_protected_inside_window() {
        let rec = FileCompressionRecord {
            path: PathBuf::from("src/main.rs"),
            level: CompressionLevel::Full,
            content_hash: hash_content("fn main() {}"),
            line_range: None,
            last_referenced_turn: 5,
        };
        assert!(rec.is_protected(5));
        assert!(rec.is_protected(6));
        assert!(!rec.is_protected(7));
    }

    #[test]
    fn record_read_resets_level_on_content_change() {
        let mut state = FileCompressionState::new();
        let path = PathBuf::from("src/lib.rs");
        state.record_read(&path, "original", 1);
        // Demote outside protection window.
        state.files.get_mut(&path).unwrap().last_referenced_turn = 0;
        state.demote(&path, CompressionLevel::Summary, 10);
        assert_eq!(
            state.files.get(&path).unwrap().level,
            CompressionLevel::Summary
        );
        // Re-read with new content — level must reset to Full.
        state.record_read(&path, "changed content", 11);
        assert_eq!(
            state.files.get(&path).unwrap().level,
            CompressionLevel::Full
        );
    }

    #[test]
    fn record_read_preserves_level_on_unchanged_content() {
        let mut state = FileCompressionState::new();
        let path = PathBuf::from("src/lib.rs");
        state.record_read(&path, "same", 1);
        state.files.get_mut(&path).unwrap().last_referenced_turn = 0;
        state.demote(&path, CompressionLevel::Partial, 10);
        state.record_read(&path, "same", 11);
        assert_eq!(
            state.files.get(&path).unwrap().level,
            CompressionLevel::Partial
        );
    }

    #[test]
    fn demote_refuses_protected_files() {
        let mut state = FileCompressionState::new();
        let path = PathBuf::from("src/hot.rs");
        state.record_read(&path, "contents", 5);
        let ok = state.demote(&path, CompressionLevel::Summary, 5);
        assert!(!ok);
        assert_eq!(
            state.files.get(&path).unwrap().level,
            CompressionLevel::Full
        );
    }

    #[test]
    fn compression_state_empty_roundtrip() {
        let state = FileCompressionState::new();
        let json = serde_json::to_string(&state).unwrap();
        let back: FileCompressionState = serde_json::from_str(&json).unwrap();
        assert!(back.files.is_empty());
    }

    #[test]
    fn compression_state_handles_unicode_paths() {
        let mut state = FileCompressionState::new();
        let path = PathBuf::from("src/crates/café/niño.rs");
        state.record_read(&path, "contents", 1);
        let json = serde_json::to_string(&state).unwrap();
        let back: FileCompressionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.files.len(), 1);
        assert!(back.files.contains_key(&path));
    }

    #[test]
    fn compression_state_demote_after_protection_window_expires() {
        // Read at turn 0 → protected until turn 2 (PROTECTED_TURN_WINDOW = 2).
        // Demote attempts inside the window fail; one past the window succeeds.
        let mut state = FileCompressionState::new();
        let path = PathBuf::from("src/hot.rs");
        state.record_read(&path, "contents", 0);
        assert!(!state.demote(&path, CompressionLevel::Summary, 0));
        assert!(!state.demote(&path, CompressionLevel::Summary, 1));
        assert!(state.demote(&path, CompressionLevel::Summary, 2));
        assert_eq!(
            state.files.get(&path).unwrap().level,
            CompressionLevel::Summary
        );
    }

    #[test]
    fn compression_state_roundtrip() {
        let mut state = FileCompressionState::new();
        state.record_read(Path::new("a.rs"), "alpha", 1);
        state.record_read(Path::new("b.rs"), "beta", 2);
        let json = serde_json::to_string(&state).unwrap();
        let back: FileCompressionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.files.len(), 2);
        assert_eq!(
            back.files.get(Path::new("a.rs")).unwrap().content_hash,
            hash_content("alpha"),
        );
    }

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

    #[test]
    fn test_effective_context_window() {
        // Sonnet: 200K context - 16K output = 184K (capped at 20K → 180K)
        let eff = effective_context_window("claude-sonnet");
        assert!(eff > 100_000);
        assert!(eff < 200_000);
    }

    #[test]
    fn test_token_warning_state_empty() {
        let state = token_warning_state(&[], "claude-sonnet");
        assert_eq!(state.percent_left, 100);
        assert!(!state.is_above_warning);
        assert!(!state.is_blocking);
    }

    #[test]
    fn test_should_auto_compact_empty() {
        let tracking = CompactTracking::default();
        assert!(!should_auto_compact(&[], "claude-sonnet", &tracking));
    }

    #[test]
    fn test_should_auto_compact_circuit_breaker() {
        let tracking = CompactTracking {
            consecutive_failures: 5,
            was_compacted: false,
        };
        // Even with huge message list, circuit breaker should prevent compaction.
        assert!(!should_auto_compact(&[], "claude-sonnet", &tracking));
    }

    #[test]
    fn test_microcompact_empty() {
        let mut messages = vec![];
        let freed = microcompact(&mut messages, 2);
        assert_eq!(freed, 0);
    }

    #[test]
    fn test_microcompact_keeps_recent() {
        use crate::llm::message::*;
        // Create a tool result message.
        let mut messages = vec![
            Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "FileRead".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
            Message::User(UserMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "file content here".repeat(100),
                    is_error: false,
                    extra_content: vec![],
                }],
                is_meta: true,
                is_compact_summary: false,
            }),
        ];
        // keep_recent=5 means this single result should be kept.
        let freed = microcompact(&mut messages, 5);
        assert_eq!(freed, 0);
    }

    #[test]
    fn estimate_compactable_tokens_matches_microcompact() {
        use crate::llm::message::*;
        // 5 compactable tool results; keep_recent=2 should free 3 of them.
        let mut msgs: Vec<Message> = Vec::new();
        for i in 0..5 {
            msgs.push(Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolUse {
                    id: format!("id_{i}"),
                    name: "FileRead".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }));
            msgs.push(Message::User(UserMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: format!("id_{i}"),
                    content: "some file contents ".repeat(50),
                    is_error: false,
                    extra_content: vec![],
                }],
                is_meta: true,
                is_compact_summary: false,
            }));
        }
        let estimated = estimate_compactable_tokens(&msgs, 2);
        let mut clone = msgs.clone();
        let actual = microcompact(&mut clone, 2);
        assert_eq!(estimated, actual);
        assert!(estimated > 0);
    }

    #[test]
    fn estimate_compactable_tokens_returns_zero_when_keep_covers_all() {
        use crate::llm::message::*;
        let msgs = vec![
            Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "FileRead".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
            Message::User(UserMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "big tool result".repeat(100),
                    is_error: false,
                    extra_content: vec![],
                }],
                is_meta: true,
                is_compact_summary: false,
            }),
        ];
        assert_eq!(estimate_compactable_tokens(&msgs, 5), 0);
    }

    #[test]
    fn test_compact_boundary_message() {
        let msg = compact_boundary_message("test summary");
        if let Message::System(s) = msg {
            assert_eq!(
                s.subtype,
                crate::llm::message::SystemMessageType::CompactBoundary
            );
        } else {
            panic!("Expected system message");
        }
    }

    #[test]
    fn test_max_output_recovery_message() {
        let msg = max_output_recovery_message();
        match msg {
            Message::User(u) => {
                assert!(!u.content.is_empty());
            }
            _ => panic!("Expected user message"),
        }
    }

    #[test]
    fn test_build_compact_summary_prompt() {
        use crate::llm::message::*;
        let messages = vec![user_message("hello"), user_message("world")];
        let prompt = build_compact_summary_prompt(&messages);
        assert!(prompt.contains("Summarize"));
    }

    #[test]
    fn test_effective_context_window_gpt_model() {
        let eff = effective_context_window("gpt-4o");
        // gpt-4: 128K context, 16K max output (capped at 20K → 16K), effective = 128K - 16K = 112K
        assert_eq!(eff, 128_000 - 16_384);
    }

    #[test]
    fn test_auto_compact_threshold_gpt_model() {
        let threshold = auto_compact_threshold("gpt-4o");
        assert_eq!(threshold, 128_000 - 16_384 - 13_000);
    }

    #[test]
    fn test_parse_prompt_too_long_gap_with_comma_format() {
        // Numbers without commas embedded, but different magnitudes.
        let msg = "prompt is too long: 137500 tokens > 135000 maximum";
        assert_eq!(parse_prompt_too_long_gap(msg), Some(2500));
    }

    #[test]
    fn test_parse_prompt_too_long_gap_equal_tokens_returns_none() {
        let msg = "prompt is too long: 135000 tokens > 135000 maximum";
        // gap = 0, so returns None.
        assert_eq!(parse_prompt_too_long_gap(msg), None);
    }

    #[test]
    fn test_token_warning_state_large_count_should_compact() {
        use crate::llm::message::*;
        // Create a huge message that will exceed the threshold.
        let big_text = "a".repeat(800_000); // ~200K tokens
        let messages = vec![user_message(&big_text)];
        let state = token_warning_state(&messages, "claude-sonnet");
        assert!(state.should_compact);
    }

    #[test]
    fn test_should_auto_compact_empty_tracking_small_conversation() {
        let tracking = CompactTracking::default();
        let messages = vec![crate::llm::message::user_message("tiny")];
        assert!(!should_auto_compact(&messages, "claude-sonnet", &tracking));
    }

    #[test]
    fn test_compact_boundary_message_content_format() {
        let msg = compact_boundary_message("my summary");
        if let Message::System(s) = &msg {
            assert!(s.content.contains("my summary"));
            assert!(s.content.starts_with("[Conversation compacted."));
        } else {
            panic!("Expected System message");
        }
    }

    #[test]
    fn test_build_compact_summary_prompt_includes_user_and_assistant() {
        use crate::llm::message::*;
        let messages = vec![
            user_message("user said this"),
            Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text {
                    text: "assistant said that".into(),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
        ];
        let prompt = build_compact_summary_prompt(&messages);
        assert!(prompt.contains("user said this"));
        assert!(prompt.contains("assistant said that"));
        assert!(prompt.contains("User:"));
        assert!(prompt.contains("Assistant:"));
    }

    #[test]
    fn build_compact_summary_prompt_masks_secrets_in_user_messages() {
        use crate::llm::message::*;
        let aws_key = "AKIAIOSFODNN7EXAMPLE";
        let messages = vec![user_message(format!(
            "I pasted my AWS key {aws_key} into the file"
        ))];
        let prompt = build_compact_summary_prompt(&messages);
        assert!(
            !prompt.contains(aws_key),
            "raw AWS key survived compaction prompt: {prompt}",
        );
        assert!(prompt.contains("[REDACTED:aws_access_key]"));
    }

    #[test]
    fn build_compact_summary_prompt_masks_secrets_in_assistant_messages() {
        use crate::llm::message::*;
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let messages = vec![Message::Assistant(AssistantMessage {
            uuid: uuid::Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Text {
                text: format!("I used this token: {secret}"),
            }],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })];
        let prompt = build_compact_summary_prompt(&messages);
        assert!(!prompt.contains(secret));
        assert!(prompt.contains("REDACTED"));
    }

    #[test]
    fn test_max_output_recovery_message_is_meta() {
        let msg = max_output_recovery_message();
        if let Message::User(u) = &msg {
            assert!(u.is_meta);
        } else {
            panic!("Expected User message");
        }
    }

    #[test]
    fn test_calculate_keep_count_returns_at_least_5_for_large_list() {
        use crate::llm::message::*;
        // Create 20 messages with text content.
        let messages: Vec<Message> = (0..20)
            .map(|i| user_message(format!("message {i}")))
            .collect();
        let keep = calculate_keep_count(&messages);
        assert!(keep >= 5, "keep_count was {keep}, expected at least 5");
    }
}
