//! Streaming delta coalescer for the modern TUI.
//!
//! Assistant/thinking text arrives as many small deltas. Rendering each
//! one would repaint 100+ frames/sec during heavy streaming. `StreamBuffer`
//! accumulates deltas and releases them on a deadline (100 ms) or size cap
//! (8 KiB), whichever comes first — capping repaints at ~10/sec (plan §2.2
//! rule 2). Any non-delta engine event must flush the buffer *before* it is
//! applied so text never reorders around tool/turn boundaries (rule 3).

use std::time::Duration;

/// Flush cadence: at most one text repaint per this interval.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(100);
/// Size cap: flush early once this many bytes have accumulated.
pub const FLUSH_BYTES: usize = 8 * 1024;

/// A coalescing buffer for one kind of streaming text.
#[derive(Debug, Default, Clone)]
pub struct StreamBuffer {
    assistant: String,
    thinking: String,
    /// True once a delta has been pushed but not yet flushed.
    pending: bool,
}

/// The coalesced text released by a flush (empty strings when nothing
/// accumulated for that channel).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Flushed {
    pub assistant: String,
    pub thinking: String,
}

impl Flushed {
    pub fn is_empty(&self) -> bool {
        self.assistant.is_empty() && self.thinking.is_empty()
    }
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulate an assistant-text delta.
    pub fn push_assistant(&mut self, text: &str) {
        self.assistant.push_str(text);
        self.pending = true;
    }

    /// Accumulate a thinking-text delta.
    pub fn push_thinking(&mut self, text: &str) {
        self.thinking.push_str(text);
        self.pending = true;
    }

    /// True if any delta is waiting to be flushed.
    pub fn has_pending(&self) -> bool {
        self.pending
    }

    /// True if the buffer has reached the byte cap and should flush now,
    /// ahead of the deadline.
    pub fn should_flush_by_size(&self) -> bool {
        self.assistant.len() + self.thinking.len() >= FLUSH_BYTES
    }

    /// Drain the accumulated text. Returns [`Flushed`] with whatever was
    /// buffered; leaves the buffer empty and not pending.
    pub fn flush(&mut self) -> Flushed {
        self.pending = false;
        Flushed {
            assistant: std::mem::take(&mut self.assistant),
            thinking: std::mem::take(&mut self.thinking),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_multiple_deltas_into_one_flush() {
        let mut b = StreamBuffer::new();
        b.push_assistant("Hel");
        b.push_assistant("lo, ");
        b.push_assistant("world");
        assert!(b.has_pending());
        let out = b.flush();
        assert_eq!(out.assistant, "Hello, world");
        assert!(out.thinking.is_empty());
        assert!(!b.has_pending());
    }

    #[test]
    fn separates_assistant_and_thinking() {
        let mut b = StreamBuffer::new();
        b.push_assistant("answer");
        b.push_thinking("reasoning");
        let out = b.flush();
        assert_eq!(out.assistant, "answer");
        assert_eq!(out.thinking, "reasoning");
    }

    #[test]
    fn flush_empties_buffer() {
        let mut b = StreamBuffer::new();
        b.push_assistant("x");
        let _ = b.flush();
        let out = b.flush();
        assert!(out.is_empty());
        assert!(!b.has_pending());
    }

    #[test]
    fn size_cap_triggers_early_flush() {
        let mut b = StreamBuffer::new();
        assert!(!b.should_flush_by_size());
        b.push_assistant(&"a".repeat(FLUSH_BYTES));
        assert!(b.should_flush_by_size());
        b.flush();
        assert!(!b.should_flush_by_size());
    }

    #[test]
    fn empty_buffer_is_not_pending() {
        let b = StreamBuffer::new();
        assert!(!b.has_pending());
        assert!(!b.should_flush_by_size());
    }
}
