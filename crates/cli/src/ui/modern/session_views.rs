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
    /// Messages in the conversation this view was built from.
    ///
    /// The cache is only valid while the conversation still says what it
    /// said when we left. Another agent process can advance a session
    /// that is not in front here, and the resume path reloads those
    /// messages into the engine — so restoring the remembered view
    /// unconditionally shows history the model is no longer reasoning
    /// from.
    ///
    /// Message count rather than turn count: `/rewind` and `/snip`
    /// rewrite `messages` without touching `turn_count`, so a rewound
    /// session can report the same turns as the view we cached while
    /// holding a different conversation. Counting messages catches
    /// growth *and* truncation.
    ///
    /// It is a witness, not a hash — a conversation rewound and regrown
    /// to exactly the same length while we were away still matches. That
    /// needs a revision counter on the conversation itself, which the
    /// engine does not carry today.
    messages: usize,
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
    pub fn save(&mut self, session_id: &str, view: SessionView, messages: usize) {
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
        self.views.insert(
            session_id.to_string(),
            Entry {
                view,
                bytes,
                messages,
            },
        );
    }

    /// Take back a remembered view, if it still matches the session.
    ///
    /// Removed rather than cloned: the caller is about to become the
    /// live view, and a stale copy left behind would be restored over
    /// newer content the next time round.
    ///
    /// `messages` is what the conversation just loaded into the engine
    /// holds. A view remembered at a different count describes a session
    /// that has since moved on — under another agent process, say — so
    /// it is dropped and the caller falls back to the rebuild. Erring
    /// this way costs the reader their scroll position; erring the other
    /// way shows them a transcript the model has already left behind.
    pub fn take(&mut self, session_id: &str, messages: usize) -> Option<SessionView> {
        if self
            .views
            .get(session_id)
            .is_some_and(|e| e.messages != messages)
        {
            self.remove(session_id);
            return None;
        }
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

    /// Sessions with a remembered view, i.e. ones visited in this
    /// process. Used by the picker to mark rows the user can return to
    /// without a rebuild.
    pub fn visited(&self) -> impl Iterator<Item = &str> {
        self.views.keys().map(String::as_str)
    }

    /// Whether a cached view exists for `session_id`.
    ///
    /// Read live by the roster rather than snapshotted when the picker
    /// opens: a restore completing behind an open picker stashes the
    /// departing session and may consume the destination's view, so any
    /// copy taken at open time is already wrong by the time it is drawn.
    pub fn is_visited(&self, session_id: &str) -> bool {
        self.views.contains_key(session_id)
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

/// What a view costs on the heap.
///
/// **The invariant: the cache is charged the allocations it retains, so
/// every measurement here is `capacity`, never `len`.** A view is moved
/// into the cache whole, spare capacity included, and `len` can sit
/// arbitrarily far below that — `Vec::clear` and `String::clear` drop
/// contents while keeping the buffer, so a `/clear`ed session that
/// afterwards holds one short row still pins the allocation the long
/// transcript grew. Charging `len` would let exactly that case report a
/// handful of bytes while the cache held megabytes, which is the bound
/// failing silently rather than holding.
///
/// Still an estimate — it does not chase `HashMap` overhead or the
/// allocator's rounding — but it can no longer be smaller than what is
/// actually kept, which is the property the budget depends on.
fn view_bytes(view: &SessionView) -> usize {
    // The vector's own buffer covers every item slot it has room for,
    // live or spare, so the items are not counted again below — only
    // the separate heap buffers their strings own.
    let spine = view.transcript.capacity() * std::mem::size_of::<TranscriptItem>();
    let text: usize = view.transcript.iter().map(item_bytes).sum();
    let expanded = view.expanded.capacity() * std::mem::size_of::<usize>();
    spine + text + expanded
}

/// The string buffers an item owns, by capacity — see [`view_bytes`].
fn item_bytes(item: &TranscriptItem) -> usize {
    match item {
        TranscriptItem::User(t)
        | TranscriptItem::Assistant(t)
        | TranscriptItem::System(t)
        | TranscriptItem::Error(t)
        | TranscriptItem::Warning(t) => t.capacity(),
        TranscriptItem::Thinking { text, .. } => text.capacity(),
        TranscriptItem::Tool {
            call_id,
            name,
            detail,
            result,
            live,
            ..
        } => {
            call_id.capacity()
                + name.capacity()
                + detail.capacity()
                + result.as_ref().map_or(0, String::capacity)
                + live.as_ref().map_or(0, String::capacity)
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

    /// A view whose transcript carries `bytes` of text.
    fn view_of_size(bytes: usize) -> SessionView {
        SessionView {
            transcript: vec![TranscriptItem::User("x".repeat(bytes))],
            scroll: ScrollState::Follow,
            expanded: Default::default(),
            selected_item: None,
        }
    }

    /// Comfortably under half the budget, so two such views coexist and
    /// a third cannot — with slack for the per-item overhead.
    const HALF: usize = MAX_BYTES / 2 - 4096;

    #[test]
    fn a_saved_view_comes_back_intact() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"), 1);
        let got = views.take("a", 1).expect("saved view");
        assert!(matches!(&got.transcript[0], TranscriptItem::User(t) if t == "hello"));
        assert_eq!(got.scroll, ScrollState::Free { top_line: 42 });
        assert!(got.expanded.contains(&1));
        assert_eq!(got.selected_item, Some(1));
    }

    /// Another agent process can advance a session that is not in front
    /// here. The resume path reloads those messages into the engine, so
    /// handing back the remembered view would show history the model has
    /// already moved past — the user reads one conversation while the
    /// model reasons from another.
    #[test]
    fn a_view_is_dropped_when_the_session_advanced_elsewhere() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"), 3);

        assert!(
            views.take("a", 5).is_none(),
            "a view from a 3-message conversation was restored over a 5-message one"
        );
        // And it is gone, not merely refused: keeping it would hand the
        // same stale transcript back on the next switch.
        assert!(!views.contains("a"));
    }

    /// The common case is our own process advancing the session, where
    /// the remembered view is exactly right — invalidating there would
    /// cost the reader their place on every switch and defeat the cache.
    /// App's mirror is kept in step with the engine each iteration, so
    /// our own growth is already reflected in what we stashed.
    #[test]
    fn a_view_survives_when_the_session_is_unchanged() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"), 3);
        assert!(
            views.take("a", 3).is_some(),
            "an unchanged view was dropped"
        );
    }

    /// Taking removes: the caller becomes the live view, and a copy left
    /// behind would later be restored over newer content.
    #[test]
    fn taking_a_view_removes_it() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"), 1);
        assert!(views.take("a", 1).is_some());
        assert!(views.take("a", 1).is_none());
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
            1,
        );
        assert!(views.is_empty());
        assert!(views.take("a", 1).is_none());
    }

    #[test]
    fn views_are_kept_per_session() {
        let mut views = SessionViews::default();
        views.save("a", view("from a"), 1);
        views.save("b", view("from b"), 1);
        assert_eq!(views.len(), 2);
        let a = views.take("a", 1).unwrap();
        assert!(matches!(&a.transcript[0], TranscriptItem::User(t) if t == "from a"));
        assert!(views.contains("b"), "taking one session dropped another");
    }

    #[test]
    fn forgetting_drops_a_view() {
        let mut views = SessionViews::default();
        views.save("a", view("hello"), 1);
        views.forget("a");
        assert!(views.take("a", 1).is_none());
    }

    #[test]
    fn a_session_with_no_id_is_not_saved() {
        let mut views = SessionViews::default();
        views.save("", view("hello"), 1);
        assert!(views.is_empty());
    }

    /// The entry cap is what stops a long-lived TUI from pinning every
    /// session it ever visited.
    #[test]
    fn visiting_many_sessions_evicts_the_oldest() {
        let mut views = SessionViews::default();
        for i in 0..MAX_VIEWS + 3 {
            views.save(&format!("s{i}"), view("hello"), 1);
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
        views.save("a", view_of_size(HALF), 1);
        views.save("b", view_of_size(HALF), 1);
        assert_eq!(views.len(), 2, "two views inside the budget did not fit");
        views.save("c", view_of_size(HALF), 1);
        assert!(views.len() < 3, "three oversized views all stayed resident");
        assert!(views.contains("c"), "the newest view was the one dropped");
        assert!(!views.contains("a"), "the oldest view was kept");
    }

    /// Re-saving a session must not charge its bytes twice, or the
    /// budget shrinks every time the user revisits one session.
    #[test]
    fn resaving_a_session_replaces_its_entry() {
        let mut views = SessionViews::default();
        for _ in 0..6 {
            views.save("a", view_of_size(HALF), 1);
        }
        views.save("b", view_of_size(HALF), 1);
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
        views.save("small", view("hello"), 1);
        views.save("huge", view_of_size(MAX_BYTES + 1), 1);
        assert!(!views.contains("huge"), "an oversized view was cached");
        assert!(
            views.contains("small"),
            "an oversized view evicted the cache it could not join"
        );
    }

    /// Text length is not the only way a transcript gets big: a very
    /// long one costs per item too, so item count is charged as well.
    #[test]
    fn a_transcript_huge_by_item_count_is_bounded_too() {
        let mut views = SessionViews::default();
        let items = MAX_BYTES / std::mem::size_of::<TranscriptItem>() + 1;
        views.save(
            "many",
            SessionView {
                transcript: vec![TranscriptItem::User(String::new()); items],
                scroll: ScrollState::Follow,
                expanded: Default::default(),
                selected_item: None,
            },
            1,
        );
        assert!(
            !views.contains("many"),
            "a transcript over budget by item count was cached as free"
        );
    }

    /// The whole point of measuring capacity: `Vec::clear` drops the rows
    /// but keeps the buffer, so a session cleared after a long
    /// conversation and then reused for one short row still pins the
    /// large allocation — and it is that allocation, not the one visible
    /// row, that the cache would go on holding.
    #[test]
    fn a_cleared_transcript_is_charged_the_buffer_it_kept() {
        let mut views = SessionViews::default();
        let items = MAX_BYTES / std::mem::size_of::<TranscriptItem>() + 1;
        let mut transcript = vec![TranscriptItem::User(String::new()); items];
        transcript.clear();
        transcript.push(TranscriptItem::User("one short row".into()));
        assert_eq!(
            transcript.len(),
            1,
            "a len-based estimate would see a single short row here"
        );
        assert!(
            transcript.capacity() * std::mem::size_of::<TranscriptItem>() > MAX_BYTES,
            "the cleared buffer was released, so this no longer tests retention"
        );

        views.save(
            "cleared",
            SessionView {
                transcript,
                scroll: ScrollState::Follow,
                expanded: Default::default(),
                selected_item: None,
            },
            1,
        );

        assert!(
            !views.contains("cleared"),
            "an over-budget retained buffer was charged as one short row"
        );
    }

    /// Same divergence one level down: a `String` cleared and reused
    /// keeps its buffer too, so text is measured by capacity as well.
    #[test]
    fn a_cleared_string_is_charged_the_buffer_it_kept() {
        let mut views = SessionViews::default();
        let mut text = "x".repeat(MAX_BYTES + 1);
        text.clear();
        text.push_str("short");
        assert!(text.capacity() > MAX_BYTES, "the buffer was released");

        views.save(
            "cleared",
            SessionView {
                transcript: vec![TranscriptItem::User(text)],
                scroll: ScrollState::Follow,
                expanded: Default::default(),
                selected_item: None,
            },
            1,
        );

        assert!(
            !views.contains("cleared"),
            "an over-budget retained string buffer was charged as five bytes"
        );
    }
}
