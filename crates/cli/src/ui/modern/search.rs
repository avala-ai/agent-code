//! In-transcript search (`Ctrl+F`).
//!
//! Searches the *rendered* transcript rather than the underlying items,
//! because that is what the user is looking at: a match in a collapsed
//! tool card or a wrapped line is found at the line they can actually
//! scroll to. [`super::layout::LayoutCache`] already holds those lines,
//! so a search is a scan over it and a jump to the matching line.
//!
//! Matching is case-insensitive unless the query contains an uppercase
//! character — the "smart case" convention from vim and ripgrep, which
//! makes the common lowercase query broad and an explicitly capitalised
//! one precise without a separate toggle.

use super::app::App;

/// Overlay state for the search bar.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// What the user has typed.
    pub query: String,
    /// Absolute layout line index of every match, in order.
    pub matches: Vec<usize>,
    /// Index into [`Self::matches`] of the highlighted one.
    pub current: usize,
    /// Scroll position when the search opened, restored on cancel so a
    /// search that finds nothing useful costs the reader their place.
    pub origin: super::scroll::ScrollState,
}

impl Search {
    /// `1/12`-style position, or `0/0` when nothing matched.
    pub fn position(&self) -> (usize, usize) {
        if self.matches.is_empty() {
            (0, 0)
        } else {
            (self.current + 1, self.matches.len())
        }
    }

    /// The matching line currently selected.
    pub fn current_line(&self) -> Option<usize> {
        self.matches.get(self.current).copied()
    }
}

/// True when `line` matches `query` under smart case.
fn line_matches(line: &str, query: &str) -> bool {
    if query.chars().any(|c| c.is_uppercase()) {
        line.contains(query)
    } else {
        line.to_lowercase().contains(query)
    }
}

impl App {
    /// Open the search bar (`Ctrl+F`).
    pub fn open_search(&mut self) {
        if self.front_modal().is_some() {
            return;
        }
        self.command_palette = None;
        self.show_shortcuts = false;
        self.search = Some(Search {
            origin: self.scroll,
            ..Search::default()
        });
        self.status_message = "search · type to find · Enter/n next · N prev · Esc close".into();
        self.dirty = true;
    }

    pub fn search_open(&self) -> bool {
        self.search.is_some()
    }

    /// Close the search bar, keeping the current scroll position.
    pub fn close_search(&mut self) {
        if self.search.take().is_some() {
            self.status_message.clear();
            self.dirty = true;
        }
    }

    /// Close and return to where the reader was before searching.
    pub fn cancel_search(&mut self) {
        if let Some(s) = self.search.take() {
            self.scroll = s.origin;
            self.status_message.clear();
            self.dirty = true;
        }
    }

    pub fn search_insert_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        let Some(s) = self.search.as_mut() else {
            return;
        };
        s.query.push(c);
        self.recompute_search(true);
    }

    pub fn search_backspace(&mut self) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        s.query.pop();
        self.recompute_search(true);
    }

    /// Move to the next match, wrapping at the end.
    pub fn search_next(&mut self) {
        self.step_search(1);
    }

    /// Move to the previous match, wrapping at the start.
    pub fn search_prev(&mut self) {
        self.step_search(-1);
    }

    fn step_search(&mut self, delta: i32) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        if s.matches.is_empty() {
            return;
        }
        let n = s.matches.len() as i32;
        s.current = ((s.current as i32 + delta).rem_euclid(n)) as usize;
        self.scroll_to_match();
        self.dirty = true;
    }

    /// Rescan the transcript for the current query.
    ///
    /// `reset` puts the selection back on the first match, which is what
    /// editing the query should do; stepping through results does not.
    pub fn recompute_search(&mut self, reset: bool) {
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else {
            return;
        };
        let mut matches = Vec::new();
        if !query.is_empty() {
            let needle = if query.chars().any(|c| c.is_uppercase()) {
                query.clone()
            } else {
                query.to_lowercase()
            };
            let total = self.layout.total_lines();
            if let Some(text) = self.layout.plain_range(0, total.saturating_sub(1)) {
                for (i, line) in text.lines().enumerate() {
                    if line_matches(line, &needle) {
                        matches.push(i);
                    }
                }
            }
        }
        if let Some(s) = self.search.as_mut() {
            s.matches = matches;
            if reset || s.current >= s.matches.len() {
                s.current = 0;
            }
        }
        self.scroll_to_match();
        self.dirty = true;
    }

    /// Centre the viewport on the selected match.
    fn scroll_to_match(&mut self) {
        let Some(line) = self.search.as_ref().and_then(|s| s.current_line()) else {
            return;
        };
        // Centring rather than top-aligning keeps the surrounding context
        // visible, which is usually why the line was being looked for.
        let half = self.viewport_h / 2;
        self.scroll = super::scroll::ScrollState::Free {
            top_line: line.saturating_sub(half),
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::modern::app::{App, TranscriptItem};

    fn app_with_lines() -> App {
        let mut app = App::new("m", "/tmp", "s");
        app.viewport_h = 10;
        for text in [
            "first line about auth",
            "second line",
            "third line about AUTH tokens",
            "fourth line",
            "fifth mentions auth again",
        ] {
            app.transcript.push(TranscriptItem::System(text.into()));
        }
        // Search reads the rendered layout, so it has to be synced
        // first — same call the renderer makes each frame.
        let expanded = app.expanded.clone();
        let selected = app.selected_item;
        app.layout.sync(&app.transcript, 80, &expanded, selected);
        app
    }

    #[test]
    fn a_lowercase_query_matches_case_insensitively() {
        let mut app = app_with_lines();
        app.open_search();
        for c in "auth".chars() {
            app.search_insert_char(c);
        }
        let s = app.search.as_ref().unwrap();
        assert_eq!(
            s.matches.len(),
            3,
            "expected all three auth lines, got {:?}",
            s.matches
        );
    }

    /// Smart case: an uppercase character in the query makes it precise,
    /// so `AUTH` finds only the line that is actually shouting.
    #[test]
    fn an_uppercase_query_is_case_sensitive() {
        let mut app = app_with_lines();
        app.open_search();
        for c in "AUTH".chars() {
            app.search_insert_char(c);
        }
        assert_eq!(app.search.as_ref().unwrap().matches.len(), 1);
    }

    #[test]
    fn next_and_prev_wrap_around() {
        let mut app = app_with_lines();
        app.open_search();
        for c in "auth".chars() {
            app.search_insert_char(c);
        }
        assert_eq!(app.search.as_ref().unwrap().position(), (1, 3));
        app.search_next();
        assert_eq!(app.search.as_ref().unwrap().position(), (2, 3));
        app.search_next();
        app.search_next();
        assert_eq!(
            app.search.as_ref().unwrap().position(),
            (1, 3),
            "next past the end must wrap to the first match"
        );
        app.search_prev();
        assert_eq!(
            app.search.as_ref().unwrap().position(),
            (3, 3),
            "prev before the start must wrap to the last match"
        );
    }

    #[test]
    fn a_query_with_no_matches_reports_zero_and_does_not_move() {
        let mut app = app_with_lines();
        let before = app.scroll;
        app.open_search();
        for c in "zzzznotpresent".chars() {
            app.search_insert_char(c);
        }
        assert_eq!(app.search.as_ref().unwrap().position(), (0, 0));
        assert_eq!(app.scroll, before, "an empty result scrolled the viewport");
    }

    /// Esc restores the reader's place. A search that turns up nothing
    /// useful should not cost them where they were.
    #[test]
    fn cancelling_restores_the_original_scroll_position() {
        let mut app = app_with_lines();
        app.scroll_to_top();
        let before = app.scroll;
        app.open_search();
        // A match far enough down that centring actually moves the
        // viewport — a match on line 0 would restore trivially and the
        // test would prove nothing.
        for c in "fifth".chars() {
            app.search_insert_char(c);
        }
        assert_ne!(app.scroll, before, "jumping to a match should move");
        app.cancel_search();
        assert!(!app.search_open());
        assert_eq!(app.scroll, before, "Esc did not restore the position");
    }

    /// Enter keeps the position it jumped to — the point of accepting is
    /// to stay where the match is.
    #[test]
    fn closing_keeps_the_match_position() {
        let mut app = app_with_lines();
        app.scroll_to_top();
        app.open_search();
        for c in "fifth".chars() {
            app.search_insert_char(c);
        }
        let at_match = app.scroll;
        app.close_search();
        assert!(!app.search_open());
        assert_eq!(app.scroll, at_match);
    }

    #[test]
    fn editing_the_query_reselects_the_first_match() {
        let mut app = app_with_lines();
        app.open_search();
        for c in "auth".chars() {
            app.search_insert_char(c);
        }
        app.search_next();
        assert_eq!(app.search.as_ref().unwrap().current, 1);
        app.search_backspace();
        assert_eq!(
            app.search.as_ref().unwrap().current,
            0,
            "editing the query should return to the first match"
        );
    }
}
