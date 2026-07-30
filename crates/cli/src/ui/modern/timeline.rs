//! Conversation timeline rail (#558 D5-19).
//!
//! A slim left-edge strip of the transcript body with markers for turns
//! (user / assistant / errors / tools). Hover shows a preview popup;
//! click jumps the viewport so that block is near the top.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::app::{App, TranscriptItem, WELCOME_SYSTEM_LINE};
use super::colors::palette;
use super::layout::LayoutCache;
use super::toolcard::{Display, plan_display};

/// One navigable marker on the rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineMarker {
    /// Index into `app.transcript`.
    pub item: usize,
    /// Absolute display-line index of the block start.
    pub abs_line: usize,
    pub kind: MarkerKind,
    /// Short preview for the hover popup.
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    User,
    Assistant,
    Tool,
    Error,
    Warning,
}

/// Build markers from the last layout sync + transcript.
///
/// Always includes user turns and errors/warnings. Assistant and tool
/// markers are included when the conversation is short enough that the
/// rail stays readable (≤ `rail_h` markers preferred).
pub fn build_markers(
    items: &[TranscriptItem],
    layout: &LayoutCache,
    rail_h: usize,
) -> Vec<TimelineMarker> {
    let display = plan_display(items);
    let mut candidates: Vec<TimelineMarker> = Vec::new();
    for (bi, d) in display.iter().enumerate() {
        let item_idx = match d {
            Display::Single(i) => *i,
            Display::Group(idxs) => idxs[0],
        };
        if item_idx >= items.len() {
            continue;
        }
        let Some((kind, label)) = classify(&items[item_idx]) else {
            continue;
        };
        candidates.push(TimelineMarker {
            item: item_idx,
            abs_line: layout.block_start_line(bi),
            kind,
            label,
        });
    }
    if candidates.is_empty() {
        return candidates;
    }
    // Prefer sparse rail: always keep User/Error/Warning; drop Assistant/Tool
    // when over-dense.
    let hard: Vec<_> = candidates
        .iter()
        .filter(|m| {
            matches!(
                m.kind,
                MarkerKind::User | MarkerKind::Error | MarkerKind::Warning
            )
        })
        .cloned()
        .collect();
    let soft_budget = rail_h.saturating_sub(hard.len());
    if candidates.len() <= rail_h || soft_budget == 0 {
        if candidates.len() <= rail_h {
            return candidates;
        }
        return hard;
    }
    // Interleave soft markers until budget fills.
    let mut out = hard;
    let mut soft: Vec<_> = candidates
        .into_iter()
        .filter(|m| matches!(m.kind, MarkerKind::Assistant | MarkerKind::Tool))
        .collect();
    // Evenly sample soft markers.
    if soft.len() > soft_budget {
        let step = soft.len() as f64 / soft_budget as f64;
        soft = (0..soft_budget)
            .map(|i| soft[(i as f64 * step) as usize].clone())
            .collect();
    }
    out.extend(soft);
    out.sort_by_key(|m| m.abs_line);
    out
}

fn classify(item: &TranscriptItem) -> Option<(MarkerKind, String)> {
    match item {
        TranscriptItem::User(t) => Some((MarkerKind::User, preview(t, 48))),
        TranscriptItem::Assistant(t) => Some((MarkerKind::Assistant, preview(t, 48))),
        TranscriptItem::Tool {
            name,
            detail,
            is_error,
            ..
        } => {
            let kind = if *is_error {
                MarkerKind::Error
            } else {
                MarkerKind::Tool
            };
            Some((kind, preview(&format!("{name} {detail}"), 48)))
        }
        TranscriptItem::Error(t) => Some((MarkerKind::Error, preview(t, 48))),
        TranscriptItem::Warning(t) => Some((MarkerKind::Warning, preview(t, 48))),
        TranscriptItem::System(t) if t == WELCOME_SYSTEM_LINE => None,
        TranscriptItem::System(_) | TranscriptItem::Thinking { .. } => None,
    }
}

fn preview(s: &str, max: usize) -> String {
    let one: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let trimmed = one.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Whether the rail is worth drawing (enough content + height).
pub fn should_draw(total_lines: usize, viewport_h: usize, markers: usize) -> bool {
    viewport_h >= 4 && markers >= 2 && total_lines > viewport_h.saturating_div(2)
}

/// Map a marker's absolute line onto a rail row within `rail`.
pub fn marker_row(abs_line: usize, total: usize, rail_h: usize) -> u16 {
    if rail_h == 0 || total <= 1 {
        return 0;
    }
    let max = total.saturating_sub(1);
    ((abs_line.min(max) * (rail_h - 1)) / max) as u16
}

/// Viewport band on the rail: (start_row, height) in rail-local rows.
pub fn viewport_band(top: usize, view_h: usize, total: usize, rail_h: usize) -> (u16, u16) {
    if rail_h == 0 || total == 0 {
        return (0, 0);
    }
    let start = marker_row(top, total, rail_h);
    let end_line = top.saturating_add(view_h).min(total.saturating_sub(1));
    let end = marker_row(end_line, total, rail_h);
    let h = end.saturating_sub(start).saturating_add(1);
    (start, h.max(1))
}

/// Paint the rail and register hit targets. `rail` is the full strip rect.
pub fn draw(
    frame: &mut Frame<'_>,
    rail: Rect,
    app: &mut App,
    markers: &[TimelineMarker],
    total: usize,
    top: usize,
    view_h: usize,
) {
    if rail.width == 0 || rail.height == 0 || markers.is_empty() {
        return;
    }
    let p = palette();
    let track = Style::default().fg(p.muted).add_modifier(Modifier::DIM);
    // Base track.
    for row in 0..rail.height {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("│", track))),
            Rect {
                x: rail.x,
                y: rail.y.saturating_add(row),
                width: 1,
                height: 1,
            },
        );
    }
    // Viewport band (second column if wide enough).
    let (band_start, band_h) = viewport_band(top, view_h, total, rail.height as usize);
    let band_style = Style::default().fg(p.inactive).bg(p.bg);
    for row in 0..band_h {
        let y = rail.y.saturating_add(band_start).saturating_add(row);
        if y >= rail.y.saturating_add(rail.height) {
            break;
        }
        let glyph = if rail.width >= 2 { "▌" } else { "│" };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(glyph, band_style))),
            Rect {
                x: rail.x,
                y,
                width: 1.min(rail.width),
                height: 1,
            },
        );
    }
    // Markers (topmost wins for hit test — register after paint).
    let hover_item = match &app.hit_registry.hover {
        Some(super::hit_rect::HitTarget::Timeline { item }) => Some(*item),
        _ => None,
    };
    for m in markers {
        let row = marker_row(m.abs_line, total, rail.height as usize);
        let y = rail.y.saturating_add(row);
        let hot = hover_item == Some(m.item);
        let (glyph, style) = marker_style(m.kind, hot);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(glyph, style))),
            Rect {
                x: rail.x,
                y,
                width: 1,
                height: 1,
            },
        );
        app.hit_registry.register(
            Rect {
                x: rail.x,
                y,
                width: rail.width.max(1),
                height: 1,
            },
            super::hit_rect::HitTarget::Timeline { item: m.item },
        );
    }
    // Hover preview popup to the right of the rail.
    if let Some(item) = hover_item
        && let Some(m) = markers.iter().find(|m| m.item == item)
    {
        draw_preview(frame, rail, m, total);
    }
}

fn marker_style(kind: MarkerKind, hot: bool) -> (&'static str, Style) {
    let p = palette();
    let (glyph, fg) = match kind {
        MarkerKind::User => ("●", p.accent),
        MarkerKind::Assistant => ("○", p.inactive),
        MarkerKind::Tool => ("◆", p.tool),
        MarkerKind::Error => ("✕", p.error),
        MarkerKind::Warning => ("!", p.warning),
    };
    let style = if hot {
        Style::default()
            .fg(super::colors::on_fill(fg))
            .bg(fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(fg).add_modifier(Modifier::BOLD)
    };
    (glyph, style)
}

fn draw_preview(frame: &mut Frame<'_>, rail: Rect, m: &TimelineMarker, total: usize) {
    let p = palette();
    let kind = match m.kind {
        MarkerKind::User => "you",
        MarkerKind::Assistant => "assistant",
        MarkerKind::Tool => "tool",
        MarkerKind::Error => "error",
        MarkerKind::Warning => "warning",
    };
    let title = format!(" {kind} ");
    let body = format!(" {} ", m.label);
    let w = (title.chars().count().max(body.chars().count()) as u16)
        .saturating_add(2)
        .clamp(12, 48);
    let h = 3u16;
    let x = rail.x.saturating_add(rail.width).saturating_add(1);
    let y = rail
        .y
        .saturating_add(marker_row(m.abs_line, total.max(1), rail.height as usize))
        .min(rail.y.saturating_add(rail.height.saturating_sub(h)));
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .title(Span::styled(
            title,
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(body, Style::default().fg(p.text)))).block(block),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::super::app::TranscriptItem;
    use super::*;

    #[test]
    fn markers_prefer_user_and_errors() {
        let items = vec![
            TranscriptItem::System(WELCOME_SYSTEM_LINE.into()),
            TranscriptItem::User("hello".into()),
            TranscriptItem::Assistant("hi there".into()),
            TranscriptItem::Error("boom".into()),
            TranscriptItem::User("again".into()),
        ];
        let mut layout = LayoutCache::default();
        layout.sync(&items, 80, &Default::default(), None);
        let m = build_markers(&items, &layout, 20);
        assert!(m.iter().any(|x| x.kind == MarkerKind::User));
        assert!(m.iter().any(|x| x.kind == MarkerKind::Error));
        assert!(!m.iter().any(|x| matches!(
            items.get(x.item),
            Some(TranscriptItem::System(s)) if s == WELCOME_SYSTEM_LINE
        )));
    }

    #[test]
    fn marker_row_maps_ends() {
        assert_eq!(marker_row(0, 100, 10), 0);
        assert_eq!(marker_row(99, 100, 10), 9);
    }

    #[test]
    fn should_draw_requires_enough_markers() {
        assert!(!should_draw(100, 20, 1));
        assert!(should_draw(100, 20, 3));
        assert!(!should_draw(5, 20, 5)); // short transcript
    }
}
