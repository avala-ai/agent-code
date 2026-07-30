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
    /// Timeline rail marker — click jumps to a transcript item (#558 D5-19).
    Timeline { item: usize },
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

    /// M9 acceptance: ≥20 hit-test coordinates covering stacked targets,
    /// edges, misses, wrap-like link runs, and group/status chips.
    #[test]
    fn hit_matrix_covers_twenty_coords() {
        let mut reg = HitRegistry::default();
        // Painter order: earlier = under, later = topmost.
        reg.register(r(0, 0, 80, 20), HitTarget::Transcript { item: 0 });
        reg.register(
            r(2, 3, 12, 1),
            HitTarget::Hyperlink {
                url: "https://example.com/a".into(),
            },
        );
        // Soft-wrap continuation of the same link (second display row).
        reg.register(
            r(0, 4, 8, 1),
            HitTarget::Hyperlink {
                url: "https://example.com/a".into(),
            },
        );
        reg.register(r(0, 0, 1, 20), HitTarget::Timeline { item: 2 });
        reg.register(
            r(79, 0, 1, 20),
            HitTarget::Scrollbar {
                kind: ScrollbarKind::Track,
            },
        );
        reg.register(
            r(79, 5, 1, 4),
            HitTarget::Scrollbar {
                kind: ScrollbarKind::Thumb,
            },
        );
        reg.register(r(10, 21, 8, 1), HitTarget::StatusChip { id: "mode" });
        reg.register(r(20, 21, 10, 1), HitTarget::StatusChip { id: "ctx" });
        reg.register(r(32, 21, 8, 1), HitTarget::StatusChip { id: "todos" });
        reg.register(r(0, 22, 80, 2), HitTarget::Composer);
        reg.register(r(50, 2, 20, 1), HitTarget::TaskRow { index: 1 });
        reg.register(r(50, 3, 20, 1), HitTarget::QueueRow { index: 0 });
        reg.register(
            r(40, 10, 15, 1),
            HitTarget::Control {
                id: "jump_pill".into(),
            },
        );
        reg.register(r(5, 15, 30, 1), HitTarget::LaunchRecent { index: 3 });

        // (x, y, expected target summary)
        let cases: &[(u16, u16, &str)] = &[
            (0, 0, "timeline"),   // 1 left rail over transcript
            (1, 0, "transcript"), // 2
            // Link at x=2 width=12 → columns [2, 14).
            (5, 3, "hyperlink"),   // 3 link body
            (14, 3, "transcript"), // 4 just past link end
            (2, 3, "hyperlink"),   // 5 link start edge
            (13, 3, "hyperlink"),  // 6 link last column
            (3, 4, "hyperlink"),   // 7 wrap continuation
            (79, 6, "thumb"),      // 8 scrollbar thumb over track
            (79, 1, "track"),      // 9 track only
            (79, 19, "track"),     // 10 track bottom
            (14, 21, "mode"),      // 11 status chip
            (24, 21, "ctx"),       // 12
            (35, 21, "todos"),     // 13
            (40, 22, "composer"),  // 14
            (55, 2, "task"),       // 15
            (55, 3, "queue"),      // 16
            (45, 10, "jump"),      // 17
            (10, 15, "launch"),    // 18
            (90, 0, "miss"),       // 19 outside
            (40, 30, "miss"),      // 20 outside
            (70, 8, "transcript"), // 21 empty middle of transcript
            (0, 19, "timeline"),   // 22 rail bottom
            (50, 21, "miss"),      // 23 between chips / no chip
        ];
        assert!(cases.len() >= 20, "need ≥20 coords, got {}", cases.len());

        for &(x, y, want) in cases {
            let got = match reg.hit_test(x, y) {
                Some(HitTarget::Timeline { .. }) => "timeline",
                Some(HitTarget::Transcript { .. }) => "transcript",
                Some(HitTarget::Hyperlink { .. }) => "hyperlink",
                Some(HitTarget::Scrollbar {
                    kind: ScrollbarKind::Thumb,
                }) => "thumb",
                Some(HitTarget::Scrollbar {
                    kind: ScrollbarKind::Track,
                }) => "track",
                Some(HitTarget::StatusChip { id: "mode" }) => "mode",
                Some(HitTarget::StatusChip { id: "ctx" }) => "ctx",
                Some(HitTarget::StatusChip { id: "todos" }) => "todos",
                Some(HitTarget::Composer) => "composer",
                Some(HitTarget::TaskRow { .. }) => "task",
                Some(HitTarget::QueueRow { .. }) => "queue",
                Some(HitTarget::Control { id }) if id == "jump_pill" => "jump",
                Some(HitTarget::LaunchRecent { .. }) => "launch",
                None => "miss",
                Some(other) => panic!("unexpected target at ({x},{y}): {other:?}"),
            };
            assert_eq!(got, want, "coord ({x},{y})");
        }
    }

    #[test]
    fn zero_size_rects_are_ignored() {
        let mut reg = HitRegistry::default();
        reg.register(r(0, 0, 0, 5), HitTarget::Composer);
        reg.register(r(0, 0, 5, 0), HitTarget::Composer);
        assert!(reg.is_empty());
        assert_eq!(reg.hit_test(0, 0), None);
    }
}
