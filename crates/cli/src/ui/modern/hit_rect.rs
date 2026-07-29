//! Per-frame mouse hit-rect registry.
//!
//! Renderers register `(Rect, Target)` regions each draw. Click and hover
//! resolve centrally against the registry instead of inventing per-widget
//! hit tests (which drift and duplicate state — see #558).
//!
//! Cleared at the start of every frame so stale rects never survive a
//! layout change.

use ratatui::layout::Rect;

/// What a registered region represents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    /// Transcript row / block (line selection already covers some of this).
    Transcript { item: usize },
    /// Tasks pane row.
    TaskRow { index: usize },
    /// Queue pane row.
    QueueRow { index: usize },
    /// Launch-surface recent session row (#557 Phase 3).
    LaunchRecent { index: usize },
    /// Markdown hyperlink in the transcript (OSC-8 / click-to-open).
    Hyperlink { url: String },
    /// Status-bar chip (future: model, mode, cwd).
    StatusChip { id: &'static str },
    /// Composer input area.
    Composer,
    /// Scrollbar thumb / track.
    Scrollbar { kind: ScrollbarKind },
    /// Generic named control for one-off widgets.
    Control { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarKind {
    Track,
    Thumb,
}

/// One registered region for the current frame.
#[derive(Debug, Clone)]
pub struct HitRect {
    pub rect: Rect,
    pub target: HitTarget,
}

/// Per-frame registry. Drawn into during render; queried from mouse handlers.
#[derive(Debug, Default, Clone)]
pub struct HitRegistry {
    rects: Vec<HitRect>,
    /// Last hover target (for leave/enter detection).
    pub hover: Option<HitTarget>,
}

impl HitRegistry {
    /// Drop every rect — call at the start of each draw.
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    /// Register a region. Later registrations are hit-tested first
    /// (painter's algorithm: topmost wins).
    pub fn register(&mut self, rect: Rect, target: HitTarget) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.rects.push(HitRect { rect, target });
    }

    /// Topmost target containing `(x, y)`, if any.
    pub fn hit_test(&self, x: u16, y: u16) -> Option<&HitTarget> {
        self.rects.iter().rev().find_map(|h| {
            if contains(h.rect, x, y) {
                Some(&h.target)
            } else {
                None
            }
        })
    }

    /// Update hover; returns `(entered, left)` targets when they change.
    pub fn set_hover(&mut self, next: Option<HitTarget>) -> (Option<HitTarget>, Option<HitTarget>) {
        if self.hover == next {
            return (None, None);
        }
        let left = self.hover.take();
        self.hover = next.clone();
        (next, left)
    }

    pub fn len(&self) -> usize {
        self.rects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x.saturating_add(r.width) && y >= r.y && y < r.y.saturating_add(r.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn topmost_wins() {
        let mut reg = HitRegistry::default();
        reg.register(r(0, 0, 10, 10), HitTarget::Composer);
        reg.register(r(2, 2, 4, 4), HitTarget::Control { id: "btn".into() });
        assert_eq!(
            reg.hit_test(3, 3),
            Some(&HitTarget::Control { id: "btn".into() })
        );
        assert_eq!(reg.hit_test(0, 0), Some(&HitTarget::Composer));
        assert_eq!(reg.hit_test(20, 20), None);
    }

    #[test]
    fn clear_drops_rects_keeps_hover() {
        let mut reg = HitRegistry::default();
        reg.register(r(0, 0, 1, 1), HitTarget::Composer);
        reg.hover = Some(HitTarget::Composer);
        reg.clear();
        assert!(reg.is_empty());
        assert_eq!(reg.hover, Some(HitTarget::Composer));
    }

    #[test]
    fn set_hover_reports_enter_leave() {
        let mut reg = HitRegistry::default();
        let (enter, leave) = reg.set_hover(Some(HitTarget::Composer));
        assert_eq!(enter, Some(HitTarget::Composer));
        assert_eq!(leave, None);
        let (enter, leave) = reg.set_hover(Some(HitTarget::TaskRow { index: 0 }));
        assert_eq!(enter, Some(HitTarget::TaskRow { index: 0 }));
        assert_eq!(leave, Some(HitTarget::Composer));
        let (enter, leave) = reg.set_hover(None);
        assert_eq!(enter, None);
        assert_eq!(leave, Some(HitTarget::TaskRow { index: 0 }));
    }
}
