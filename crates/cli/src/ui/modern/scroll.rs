//! Follow/Free scroll state for the transcript (plan §M2).
//!
//! `Follow` pins the viewport to the bottom so new content auto-scrolls.
//! Any upward scroll switches to `Free`, which anchors the viewport to an
//! absolute top line — so new content appended below **never moves the
//! viewport** while the user is reading. A jump-to-bottom pill (rendered
//! elsewhere) counts the lines that arrived below the viewport.
//!
//! A 1-column scrollbar (#558 D5-21) is overlaid on the right edge when
//! content exceeds the viewport; geometry helpers live here so hit-testing
//! and paint share one definition of thumb size / position.

use ratatui::layout::Rect;

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

    /// Jump to an absolute top line. Re-enters `Follow` at the bottom.
    pub fn set_top(&mut self, top: usize, total: usize, height: usize) {
        let max_top = total.saturating_sub(height);
        *self = if top >= max_top {
            ScrollState::Follow
        } else {
            ScrollState::Free {
                top_line: top.min(max_top),
            }
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

/// Geometry of the 1-column transcript scrollbar (track + thumb).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeom {
    pub track: Rect,
    pub thumb: Rect,
}

/// Overlay scrollbar on the right edge of `area` when `total > height`.
///
/// Returns `None` when there is nothing to scroll or the area is empty.
pub fn scrollbar_geom(
    area: Rect,
    total: usize,
    height: usize,
    top: usize,
) -> Option<ScrollbarGeom> {
    if area.width == 0 || area.height == 0 || height == 0 || total <= height {
        return None;
    }
    let track_h = area.height as usize;
    // Visible fraction of the document, at least one row, at most the track.
    let thumb_h = ((height.saturating_mul(track_h)) / total.max(1))
        .max(1)
        .min(track_h);
    let max_top = total - height;
    let travel = track_h.saturating_sub(thumb_h);
    let thumb_off = if max_top == 0 || travel == 0 {
        0
    } else {
        // Round to nearest so the thumb reaches both ends of the track.
        (top.saturating_mul(travel) + max_top / 2) / max_top
    };
    let x = area.x.saturating_add(area.width.saturating_sub(1));
    let thumb_y = area.y.saturating_add(thumb_off as u16);
    Some(ScrollbarGeom {
        track: Rect {
            x,
            y: area.y,
            width: 1,
            height: area.height,
        },
        thumb: Rect {
            x,
            y: thumb_y,
            width: 1,
            height: thumb_h as u16,
        },
    })
}

/// Map an absolute screen row on the track to a `top_line`, placing the
/// thumb so its top sits at the clamped click (drag with offset 0).
pub fn top_from_track_row(
    mouse_row: u16,
    geom: ScrollbarGeom,
    total: usize,
    height: usize,
    grab_offset: u16,
) -> usize {
    let max_top = total.saturating_sub(height);
    if max_top == 0 {
        return 0;
    }
    let travel = geom.track.height.saturating_sub(geom.thumb.height) as usize;
    if travel == 0 {
        return 0;
    }
    let raw = mouse_row
        .saturating_sub(geom.track.y)
        .saturating_sub(grab_offset);
    let thumb_top = (raw as usize).min(travel);
    (thumb_top.saturating_mul(max_top) + travel / 2) / travel
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

    #[test]
    fn set_top_enters_follow_at_bottom() {
        let mut s = ScrollState::Free { top_line: 10 };
        s.set_top(80, 100, 20);
        assert!(s.is_following());
        s.set_top(40, 100, 20);
        assert_eq!(s, ScrollState::Free { top_line: 40 });
    }

    #[test]
    fn scrollbar_hidden_when_content_fits() {
        let area = Rect {
            x: 0,
            y: 1,
            width: 80,
            height: 20,
        };
        assert!(scrollbar_geom(area, 10, 20, 0).is_none());
    }

    #[test]
    fn scrollbar_thumb_scales_and_moves() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        // 100 lines, 10 visible → thumb ≈ 1 row; at top → y=0; at bottom → y=9.
        let top = scrollbar_geom(area, 100, 10, 0).expect("overflow");
        assert_eq!(top.track.x, 39);
        assert_eq!(top.track.height, 10);
        assert_eq!(top.thumb.y, 0);
        assert_eq!(top.thumb.height, 1);

        let bot = scrollbar_geom(area, 100, 10, 90).expect("overflow");
        assert_eq!(bot.thumb.y, 9);

        // Mid: top=45 of max 90 → half travel (rounded).
        let mid = scrollbar_geom(area, 100, 10, 45).expect("overflow");
        assert_eq!(mid.thumb.y, 5);
    }

    #[test]
    fn track_row_maps_back_to_top() {
        let area = Rect {
            x: 0,
            y: 5,
            width: 20,
            height: 10,
        };
        let geom = scrollbar_geom(area, 100, 10, 0).unwrap();
        // Click bottom of track (thumb height 1) → near max top.
        let top = top_from_track_row(area.y + 9, geom, 100, 10, 0);
        assert_eq!(top, 90);
        let top0 = top_from_track_row(area.y, geom, 100, 10, 0);
        assert_eq!(top0, 0);
    }
}
