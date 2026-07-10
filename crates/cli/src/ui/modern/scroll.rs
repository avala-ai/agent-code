//! Follow/Free scroll state for the transcript (plan §M2).
//!
//! `Follow` pins the viewport to the bottom so new content auto-scrolls.
//! Any upward scroll switches to `Free`, which anchors the viewport to an
//! absolute top line — so new content appended below **never moves the
//! viewport** while the user is reading. A jump-to-bottom pill (rendered
//! elsewhere) counts the lines that arrived below the viewport.

/// Where the transcript viewport is anchored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollState {
    /// Pinned to the bottom; new lines auto-scroll into view.
    #[default]
    Follow,
    /// Anchored to an absolute top line; new content does not move it.
    Free { top_line: usize },
}

impl ScrollState {
    /// The top line of the viewport given the current total and height.
    /// In `Follow` this is always the bottom-most page; in `Free` the
    /// stored anchor, clamped so it can never scroll past the end.
    pub fn top(self, total: usize, height: usize) -> usize {
        let max_top = total.saturating_sub(height);
        match self {
            ScrollState::Follow => max_top,
            ScrollState::Free { top_line } => top_line.min(max_top),
        }
    }

    /// Scroll up by `n` lines, entering `Free`. Clamps at the top.
    pub fn scroll_up(&mut self, n: usize, total: usize, height: usize) {
        let cur = self.top(total, height);
        *self = ScrollState::Free {
            top_line: cur.saturating_sub(n),
        };
    }

    /// Scroll down by `n` lines. Re-enters `Follow` once the bottom is
    /// reached so subsequent content auto-scrolls again.
    pub fn scroll_down(&mut self, n: usize, total: usize, height: usize) {
        let max_top = total.saturating_sub(height);
        let cur = self.top(total, height);
        let next = (cur + n).min(max_top);
        *self = if next >= max_top {
            ScrollState::Follow
        } else {
            ScrollState::Free { top_line: next }
        };
    }

    /// Jump to the top (enters `Free` at line 0).
    pub fn go_top(&mut self) {
        *self = ScrollState::Free { top_line: 0 };
    }

    /// Jump to the bottom and re-enter `Follow`.
    pub fn go_bottom(&mut self) {
        *self = ScrollState::Follow;
    }

    pub fn is_following(self) -> bool {
        matches!(self, ScrollState::Follow)
    }

    /// Number of lines below the current viewport (for the "↓ N new" pill).
    /// Zero while following (nothing is hidden below).
    pub fn lines_below(self, total: usize, height: usize) -> usize {
        match self {
            ScrollState::Follow => 0,
            ScrollState::Free { .. } => {
                let bottom = self.top(total, height) + height;
                total.saturating_sub(bottom)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_pins_to_bottom() {
        let s = ScrollState::Follow;
        assert_eq!(s.top(100, 20), 80);
        assert_eq!(s.lines_below(100, 20), 0);
    }

    #[test]
    fn scroll_up_enters_free_and_anchors() {
        let mut s = ScrollState::Follow;
        s.scroll_up(10, 100, 20); // was top 80 → 70
        assert_eq!(s, ScrollState::Free { top_line: 70 });
        assert_eq!(s.top(100, 20), 70);
    }

    #[test]
    fn new_content_while_free_does_not_move_viewport() {
        let mut s = ScrollState::Follow;
        s.scroll_up(30, 100, 20); // top 80 → 50
        assert_eq!(s.top(100, 20), 50);
        // 500 lines stream in below; the anchored top stays put.
        assert_eq!(s.top(600, 20), 50);
        // ...and the pill now counts everything below the viewport.
        assert_eq!(s.lines_below(600, 20), 600 - (50 + 20));
    }

    #[test]
    fn scroll_down_to_bottom_reenters_follow() {
        let mut s = ScrollState::Free { top_line: 70 };
        s.scroll_down(100, 100, 20); // overshoots → Follow
        assert!(s.is_following());
    }

    #[test]
    fn to_top_and_to_bottom() {
        let mut s = ScrollState::Follow;
        s.go_top();
        assert_eq!(s, ScrollState::Free { top_line: 0 });
        s.go_bottom();
        assert!(s.is_following());
    }

    #[test]
    fn top_never_exceeds_max_even_when_anchor_is_stale() {
        // Anchor past the end (content shrank) clamps to the last page.
        let s = ScrollState::Free { top_line: 9999 };
        assert_eq!(s.top(100, 20), 80);
    }
}
