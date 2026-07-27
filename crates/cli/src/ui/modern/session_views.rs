//! Per-session view state, so switching sessions keeps your place.
//!
//! Resuming rebuilds the transcript from the stored conversation, which
//! is correct but lossy in one way that matters: it loses *where you
//! were*. Switch to another session and back, and you land at the bottom
//! of a freshly rebuilt transcript with every expansion collapsed —
//! having to find your place again each time makes moving between
//! sessions cost more than it saves.
//!
//! So the view is snapshotted on the way out and restored on the way
//! back. The engine still reloads the conversation either way; this only
//! governs what the user sees.
//!
//! The cache is a convenience, not a store of record: everything in it
//! can be rebuilt from the conversation on disk. That is what makes it
//! safe to bound — see [`MAX_VIEWS`] and [`MAX_BYTES`].

use std::collections::HashMap;

use super::app::TranscriptItem;
use super::scroll::ScrollState;

/// How many sessions keep a remembered view.
///
/// The picker lists up to 50 sessions, but switching is in practice a
/// back-and-forth among a few of them, so the most recent handful covers
/// the behaviour this cache exists for. The point of the cap is that the
/// cost is a function of the bound and not of how long the TUI has been
/// running: without it, every session ever visited pins its whole
/// transcript until the process exits.
const MAX_VIEWS: usize = 8;

/// Total transcript bytes the cache may retain, across all views.
///
/// An entry count alone is not a memory bound — a single transcript can
/// carry many large tool results — so the byte budget is the real limit
/// and [`MAX_VIEWS`] just keeps the bookkeeping small. 8 MiB is far more
/// than ordinary sessions occupy and small enough to be uninteresting
/// next to the rest of the process.
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// What a session looks like on screen.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub transcript: Vec<TranscriptItem>,
    pub scroll: ScrollState,
    pub expanded: std::collections::HashSet<usize>,
    pub selected_item: Option<usize>,
}

#[derive(Debug, Clone)]
struct Entry {
    view: SessionView,
    /// Charged against [`MAX_BYTES`]; recorded at insert so eviction
    /// never has to re-walk a transcript to know what it frees.
    bytes: usize,
}

/// Views for sessions visited in this process.
///
/// Bounded: saving evicts least-recently-saved entries until the new one
/// fits within [`MAX_VIEWS`] and [`MAX_BYTES`]. Eviction only costs the
/// evicted session its scroll position and expansions — the transcript
/// is rebuilt from the conversation on the next resume.
#[derive(Debug, Default, Clone)]
pub struct SessionViews {
    views: HashMap<String, Entry>,
    /// Session ids, least recently saved first. Bounded by [`MAX_VIEWS`],
    /// so the linear scans over it are cheaper than a second index.
    order: Vec<String>,
    /// Sum of every live entry's `bytes`.
    bytes: usize,
}

impl SessionViews {
    /// Remember how `session_id` looked.
    ///
    /// An empty transcript is not stored: it means the session was never
    /// really visited, and caching it would restore a blank screen over
    /// a conversation the engine can rebuild properly.
    ///
    /// Nor is a view too large to fit the budget on its own. Admitting
    /// one would mean flushing every other session's place *and* still
    /// blowing the bound, which is the failure this cap exists to
    /// prevent; that session falls back to the engine's rebuild.
    pub fn save(&mut self, session_id: &str, view: SessionView) {
        if session_id.is_empty() || view.transcript.is_empty() {
            return;
        }
        let bytes = view_bytes(&view);
        // Re-saving a session replaces its entry, so retire the old one
        // first or its bytes stay charged against the budget forever.
        self.remove(session_id);
        if bytes > MAX_BYTES {
            return;
        }
        while self.views.len() >= MAX_VIEWS || self.bytes + bytes > MAX_BYTES {
            let Some(oldest) = self.order.first().cloned() else {
                break;
            };
            self.remove(&oldest);
        }
        self.order.push(session_id.to_string());
        self.bytes += bytes;
        self.views
            .insert(session_id.to_string(), Entry { view, bytes });
    }

    /// Take back a remembered view, if there is one.
    ///
    /// Removed rather than cloned: the caller is about to become the
    /// live view, and a stale copy left behind would be restored over
    /// newer content the next time round.
    pub fn take(&mut self, session_id: &str) -> Option<SessionView> {
        self.remove(session_id)
    }

    pub fn contains(&self, session_id: &str) -> bool {
        self.views.contains_key(session_id)
    }

    pub fn len(&self) -> usize {
        self.views.len()
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// Drop a session's view — used when its conversation is cleared, so
    /// a later switch does not restore a transcript the user deleted.
    pub fn forget(&mut self, session_id: &str) {
        self.remove(session_id);
    }

    /// The one place an entry leaves the map, so the byte total and the
    /// recency order cannot drift out of step with it.
    fn remove(&mut self, session_id: &str) -> Option<SessionView> {
        let entry = self.views.remove(session_id)?;
        self.bytes = self.bytes.saturating_sub(entry.bytes);
        self.order.retain(|id| id != session_id);
        Some(entry.view)
    }
}

/// Roughly what a view costs on the heap.
///
/// Only the string payloads are counted: they are what makes a
/// transcript large, and the budget wants an order of magnitude, not an
/// allocator-exact figure.
fn view_bytes(view: &SessionView) -> usize {
    view.transcript.iter().map(item_bytes).sum()
}

fn item_bytes(item: &TranscriptItem) -> usize {
    match item {
        TranscriptItem::User(t)
        | TranscriptItem::Assistant(t)
        | TranscriptItem::System(t)
        | TranscriptItem::Error(t)
        | TranscriptItem::Warning(t) => t.len(),
        TranscriptItem::Thinking { text, .. } => text.len(),
        TranscriptItem::Tool {
            call_id,
            name,
            detail,
            result,
            live,
            ..
        } => {
            call_id.len()
                + name.len()
                + detail.len()
                + result.as_ref().map_or(0, String::len)
                + live.as_ref().map_or(0, String::len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text: &str) -> SessionView {
        SessionView {
            transcript: vec![TranscriptItem::User(text.into())],
            scroll: ScrollState::Free { top_line: 42 },
            expanded: [1usize].into_iter().collect(),
            selected_item: Some(1),
        }
    }

    /// A view whose transcript is `bytes` long.
    fn view_of_size(bytes: usize) -> SessionView {
        SessionView {
            transcript: vec![TranscriptItem::User("x".repeat(bytes))],
            scroll: ScrollState::Follow,
            expanded: Default::default(),
            selected_item: None,
        }
    }

    #[test]
    fn a_saved_view_comes_back_intact() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"));
        let got = views.take("a").expect("saved view");
        assert!(matches!(&got.transcript[0], TranscriptItem::User(t) if t == "hello"));
        assert_eq!(got.scroll, ScrollState::Free { top_line: 42 });
        assert!(got.expanded.contains(&1));
        assert_eq!(got.selected_item, Some(1));
    }

    /// Taking removes: the caller becomes the live view, and a copy left
    /// behind would later be restored over newer content.
    #[test]
    fn taking_a_view_removes_it() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"));
        assert!(views.take("a").is_some());
        assert!(views.take("a").is_none());
    }

    /// An empty transcript means the session was never really visited.
    /// Caching it would restore a blank screen over a conversation the
    /// engine could have rebuilt.
    #[test]
    fn an_empty_view_is_not_saved() {
        let mut views = SessionViews::default();
        views.save(
            "a",
            SessionView {
                transcript: Vec::new(),
                scroll: ScrollState::Follow,
                expanded: Default::default(),
                selected_item: None,
            },
        );
        assert!(views.is_empty());
        assert!(views.take("a").is_none());
    }

    #[test]
    fn views_are_kept_per_session() {
        let mut views = SessionViews::default();
        views.save("a", view("from a"));
        views.save("b", view("from b"));
        assert_eq!(views.len(), 2);
        let a = views.take("a").unwrap();
        assert!(matches!(&a.transcript[0], TranscriptItem::User(t) if t == "from a"));
        assert!(views.contains("b"), "taking one session dropped another");
    }

    #[test]
    fn forgetting_drops_a_view() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"));
        views.forget("a");
        assert!(views.take("a").is_none());
    }

    #[test]
    fn a_session_with_no_id_is_not_saved() {
        let mut views = SessionViews::default();
        views.save("", view("hello"));
        assert!(views.is_empty());
    }

    /// The entry cap is what stops a long-lived TUI from pinning every
    /// session it ever visited.
    #[test]
    fn visiting_many_sessions_evicts_the_oldest() {
        let mut views = SessionViews::default();
        for i in 0..MAX_VIEWS + 3 {
            views.save(&format!("s{i}"), view("hello"));
        }
        assert_eq!(views.len(), MAX_VIEWS);
        assert!(!views.contains("s0"), "oldest view survived the cap");
        assert!(!views.contains("s2"), "oldest views survived the cap");
        assert!(
            views.contains(&format!("s{}", MAX_VIEWS + 2)),
            "newest view was evicted"
        );
    }

    /// Big transcripts, not entry count, are what actually consume
    /// memory, so the byte budget has to bind before the entry cap does.
    #[test]
    fn large_transcripts_are_evicted_before_the_entry_cap() {
        let mut views = SessionViews::default();
        let half = MAX_BYTES / 2;
        views.save("a", view_of_size(half));
        views.save("b", view_of_size(half));
        views.save("c", view_of_size(half));
        assert!(views.len() < 3, "three oversized views all stayed resident");
        assert!(views.contains("c"), "the newest view was the one dropped");
        assert!(!views.contains("a"), "the oldest view was kept");
    }

    /// Re-saving a session must not charge its bytes twice, or the
    /// budget shrinks every time the user revisits one session.
    #[test]
    fn resaving_a_session_replaces_its_entry() {
        let mut views = SessionViews::default();
        let half = MAX_BYTES / 2;
        for _ in 0..6 {
            views.save("a", view_of_size(half));
        }
        views.save("b", view_of_size(half));
        assert!(
            views.contains("a") && views.contains("b"),
            "repeated saves of one session leaked budget"
        );
    }

    /// A view that cannot fit even in an empty cache is skipped rather
    /// than admitted at the cost of every other session's place.
    #[test]
    fn a_view_larger_than_the_budget_is_not_cached() {
        let mut views = SessionViews::default();
        views.save("small", view("hello"));
        views.save("huge", view_of_size(MAX_BYTES + 1));
        assert!(!views.contains("huge"), "an oversized view was cached");
        assert!(
            views.contains("small"),
            "an oversized view evicted the cache it could not join"
        );
    }
}
