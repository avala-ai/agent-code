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

use std::collections::HashMap;

use super::app::TranscriptItem;
use super::scroll::ScrollState;

/// What a session looks like on screen.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub transcript: Vec<TranscriptItem>,
    pub scroll: ScrollState,
    pub expanded: std::collections::HashSet<usize>,
    pub selected_item: Option<usize>,
}

/// Views for sessions visited in this process.
#[derive(Debug, Default, Clone)]
pub struct SessionViews {
    views: HashMap<String, SessionView>,
}

impl SessionViews {
    /// Remember how `session_id` looked.
    ///
    /// An empty transcript is not stored: it means the session was never
    /// really visited, and caching it would restore a blank screen over
    /// a conversation the engine can rebuild properly.
    pub fn save(&mut self, session_id: &str, view: SessionView) {
        if session_id.is_empty() || view.transcript.is_empty() {
            return;
        }
        self.views.insert(session_id.to_string(), view);
    }

    /// Take back a remembered view, if there is one.
    ///
    /// Removed rather than cloned: the caller is about to become the
    /// live view, and a stale copy left behind would be restored over
    /// newer content the next time round.
    pub fn take(&mut self, session_id: &str) -> Option<SessionView> {
        self.views.remove(session_id)
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
        self.views.remove(session_id);
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
}
