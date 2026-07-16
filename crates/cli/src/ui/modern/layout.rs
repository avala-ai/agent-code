//! Virtualized layout cache for the transcript (plan §M2).
//!
//! Each transcript block is rendered to display lines once and cached,
//! keyed by a content hash + the width it was wrapped at. On the next
//! frame only blocks whose content changed (in practice, the streaming
//! tail) are re-rendered — the plan's "never re-render the whole
//! transcript on stream" rule (§2.2 rule 6). Wrapping is unicode-width
//! aware on grapheme clusters so a wide char is never split and no line
//! ever exceeds the width.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::app::TranscriptItem;
use super::colors::palette;

struct Cached {
    hash: u64,
    lines: Vec<Line<'static>>,
}

/// Per-block rendered-line cache with a prefix-sum line index.
#[derive(Default)]
pub struct LayoutCache {
    width: u16,
    blocks: Vec<Cached>,
    total: usize,
}

impl std::fmt::Debug for LayoutCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutCache")
            .field("width", &self.width)
            .field("blocks", &self.blocks.len())
            .field("total", &self.total)
            .finish()
    }
}

// App derives Clone; the cache is derived state, so a clone starts empty
// and is repopulated on the next draw.
impl Clone for LayoutCache {
    fn clone(&self) -> Self {
        LayoutCache::default()
    }
}

fn hash_item(item: &TranscriptItem, expanded: bool, selected: bool) -> u64 {
    let mut h = DefaultHasher::new();
    item.hash(&mut h);
    expanded.hash(&mut h);
    selected.hash(&mut h);
    h.finish()
}

impl LayoutCache {
    /// Rebuild cache entries that are stale. A width change invalidates
    /// every block; otherwise only blocks whose content hash changed (or
    /// new blocks) are re-rendered. Returns nothing; query with
    /// [`Self::total_lines`] / [`Self::viewport`].
    pub fn sync(
        &mut self,
        items: &[TranscriptItem],
        width: u16,
        expanded: &HashSet<usize>,
        selected: Option<usize>,
    ) {
        let width_changed = width != self.width;
        self.width = width;
        if width_changed {
            self.blocks.clear();
        }

        // Fold consecutive read-only successes into groups (plan §M4); the
        // cache is keyed by display block, not raw item, so a group's hash
        // changes if any member does.
        let display = super::toolcard::plan_display(items);
        self.blocks.truncate(display.len());

        for (i, d) in display.iter().enumerate() {
            let (hash, render): (u64, Box<dyn Fn() -> Vec<Line<'static>>>) = match d {
                super::toolcard::Display::Single(idx) => {
                    let item = &items[*idx];
                    let exp = expanded.contains(idx);
                    let sel = selected == Some(*idx);
                    (
                        hash_item(item, exp, sel),
                        Box::new(move || render_item(item, exp, sel)),
                    )
                }
                super::toolcard::Display::Group(idxs) => {
                    let mut h = DefaultHasher::new();
                    "group".hash(&mut h);
                    for &idx in idxs {
                        items[idx].hash(&mut h);
                        expanded.contains(&idx).hash(&mut h);
                    }
                    let sel = selected.is_some_and(|s| idxs.contains(&s));
                    sel.hash(&mut h);
                    (h.finish(), Box::new(move || render_group(items, idxs, sel)))
                }
            };
            match self.blocks.get(i) {
                Some(c) if c.hash == hash => {} // fresh
                _ => {
                    let lines = wrap_lines(render(), width);
                    let entry = Cached { hash, lines };
                    if i < self.blocks.len() {
                        self.blocks[i] = entry;
                    } else {
                        self.blocks.push(entry);
                    }
                }
            }
        }
        self.total = self.blocks.iter().map(|b| b.lines.len()).sum();
    }

    pub fn total_lines(&self) -> usize {
        self.total
    }

    /// Absolute top line of display block `idx` (0 if out of range).
    pub fn block_start_line(&self, idx: usize) -> usize {
        self.blocks.iter().take(idx).map(|b| b.lines.len()).sum()
    }

    /// (`display block count`, `cached line count`) for the /stats command.
    pub fn stats(&self) -> (usize, usize) {
        (self.blocks.len(), self.total)
    }

    /// How many blocks were (re)rendered this width — test hook.
    #[cfg(test)]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Collect the display lines in `[top, top + height)`, cloning only the
    /// visible slice (virtualization — off-screen blocks are never copied).
    pub fn viewport(&self, top: usize, height: usize) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(height);
        let mut idx = 0usize;
        let end = top + height;
        for b in &self.blocks {
            let block_len = b.lines.len();
            let block_end = idx + block_len;
            if block_end <= top {
                idx = block_end;
                continue;
            }
            if idx >= end {
                break;
            }
            for (li, line) in b.lines.iter().enumerate() {
                let abs = idx + li;
                if abs >= top && abs < end {
                    out.push(line.clone());
                }
            }
            idx = block_end;
        }
        out
    }

    /// Plain (unstyled) text for absolute lines in `[start, end]` inclusive.
    pub fn plain_range(&self, start: usize, end: usize) -> Option<String> {
        let lo = start.min(end);
        let hi = start.max(end);
        if self.total == 0 {
            return None;
        }
        let hi = hi.min(self.total.saturating_sub(1));
        let mut parts = Vec::new();
        let mut idx = 0usize;
        for b in &self.blocks {
            for line in &b.lines {
                if idx >= lo && idx <= hi {
                    let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                    parts.push(plain);
                }
                idx += 1;
                if idx > hi {
                    return Some(parts.join("\n"));
                }
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    /// Absolute layout line index under a viewport-relative row.
    pub fn abs_line_at(&self, top: usize, row_in_view: usize) -> Option<usize> {
        let abs = top.saturating_add(row_in_view);
        if abs < self.total { Some(abs) } else { None }
    }
}

/// Render one transcript block to logical (pre-wrap) lines.
pub fn render_item(item: &TranscriptItem, expanded: bool, selected: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let sel = if selected {
        Span::styled("▌", Style::default().fg(palette().accent))
    } else {
        Span::raw(" ")
    };
    match item {
        TranscriptItem::User(t) => {
            let accent = palette().accent;
            lines.push(Line::from(vec![
                sel.clone(),
                Span::styled("❯ ", Style::default().fg(accent)),
                Span::styled(t.clone(), Style::default().fg(Color::White)),
            ]));
            lines.push(Line::from(""));
        }
        TranscriptItem::Assistant(t) => {
            let mut body = super::markdown::render_markdown(t).lines;
            if !expanded {
                let max = 12;
                let total = body.len();
                if total > max {
                    body.truncate(max);
                    body.push(Line::from(Span::styled(
                        "  … folded · press e to expand".to_string(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            if let Some(first) = body.first_mut() {
                first.spans.insert(0, sel.clone());
            } else {
                lines.push(Line::from(sel.clone()));
            }
            lines.extend(body);
            lines.push(Line::from(""));
        }
        TranscriptItem::Thinking(t) => {
            // Thinking is dimmed; collapsed by default to one line.
            lines.push(Line::from(vec![
                sel.clone(),
                Span::styled(
                    if expanded {
                        "  thinking…".to_string()
                    } else {
                        let preview: String = t.chars().take(60).collect();
                        format!(
                            "  thinking… {}{}",
                            preview,
                            if t.chars().count() > 60 { "…" } else { "" }
                        )
                    },
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
            if expanded {
                for mut line in super::markdown::render_markdown(t).lines {
                    for span in &mut line.spans {
                        span.style = span
                            .style
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC);
                    }
                    lines.push(line);
                }
            } else if !t.is_empty() {
                lines.push(Line::from(Span::styled(
                    "     (e expand · Ctrl+E all thinking)".to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        TranscriptItem::Tool {
            name,
            detail,
            result,
            is_error,
            ..
        } => lines.extend(render_tool_card(
            name,
            detail,
            result.as_deref(),
            *is_error,
            expanded,
            selected,
        )),
        TranscriptItem::System(t) => {
            lines.push(Line::from(vec![
                sel,
                Span::styled(format!(" · {t}"), Style::default().fg(Color::DarkGray)),
            ]));
        }
        TranscriptItem::Error(t) => {
            lines.push(Line::from(vec![
                sel,
                Span::styled(format!(" ✗ {t}"), Style::default().fg(Color::Red)),
            ]));
        }
        TranscriptItem::Warning(t) => {
            lines.push(Line::from(vec![
                sel,
                Span::styled(format!(" ! {t}"), Style::default().fg(Color::Yellow)),
            ]));
        }
    }
    lines
}

/// Render a typed tool card (plan §M4): kind icon + label + status glyph,
/// with the result line dim on success and red (kept visible) on error.
fn render_tool_card(
    name: &str,
    detail: &str,
    result: Option<&str>,
    is_error: bool,
    expanded: bool,
    selected: bool,
) -> Vec<Line<'static>> {
    use super::toolcard::ToolKind;
    let kind = ToolKind::classify(name);
    let (glyph, status_color) = match (result, is_error) {
        (None, _) => ("⚡", Color::Yellow),      // running
        (Some(_), false) => ("✓", Color::Green), // ok
        (Some(_), true) => ("✗", Color::Red),    // failed
    };
    let sel = if selected {
        Span::styled("▌", Style::default().fg(palette().accent))
    } else {
        Span::raw(" ")
    };
    let mut lines = vec![Line::from(vec![
        sel,
        Span::styled(format!("{glyph} "), Style::default().fg(status_color)),
        Span::styled(
            format!("{} {} ", kind.icon(), kind.label()),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("· ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail.to_string(), Style::default().fg(Color::Gray)),
    ])];
    // Errors keep more of their output visible; successes stay compact
    // unless the user expanded the card (`e`).
    if let Some(r) = result
        && !r.is_empty()
    {
        let color = if is_error {
            Color::Red
        } else {
            Color::DarkGray
        };
        let total = r.lines().count();
        let head = if expanded {
            total
        } else if is_error {
            5
        } else {
            1
        };
        for (i, line) in r.lines().take(head).enumerate() {
            let prefix = if i == 0 { "   ↳ " } else { "     " };
            lines.push(Line::from(Span::styled(
                format!("{prefix}{line}"),
                Style::default().fg(color),
            )));
        }
        if !expanded && total > head {
            lines.push(Line::from(Span::styled(
                format!("     … +{} more lines · e expand", total - head),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
    }
    lines
}

/// Render a folded read-only group as a single summary line (plan §M4):
/// `▸ read N (first, second, …)`.
fn render_group(items: &[TranscriptItem], idxs: &[usize], selected: bool) -> Vec<Line<'static>> {
    let details: Vec<String> = idxs
        .iter()
        .filter_map(|&i| match &items[i] {
            TranscriptItem::Tool { detail, .. } => Some(detail.clone()),
            _ => None,
        })
        .collect();
    let shown = details
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = if details.len() > 2 { ", …" } else { "" };
    let n = idxs.len();
    let accent = palette().accent;
    let sel = if selected {
        Span::styled("▌", Style::default().fg(accent))
    } else {
        Span::raw(" ")
    };
    vec![Line::from(vec![
        sel,
        Span::styled("▸ ", Style::default().fg(accent)),
        Span::styled(
            format!("read {n} "),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({shown}{more})"),
            Style::default().fg(Color::DarkGray),
        ),
    ])]
}

/// Wrap logical lines to `width` display columns, unicode-width aware.
pub fn wrap_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        wrap_one(line, width as usize, &mut out);
    }
    out
}

/// Wrap a single styled line, preserving span styles across the split.
/// Splits on grapheme-cluster boundaries and never exceeds `width` columns.
fn wrap_one(line: Line<'static>, width: usize, out: &mut Vec<Line<'static>>) {
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    // Accumulate graphemes of the current span so runs of same style stay
    // in one Span instead of one Span per grapheme.
    let mut buf = String::new();

    let flush_buf = |cur: &mut Vec<Span<'static>>, buf: &mut String, style: Style| {
        if !buf.is_empty() {
            cur.push(Span::styled(std::mem::take(buf), style));
        }
    };

    for span in line.spans {
        let style = span.style;
        for g in span.content.as_ref().graphemes(true) {
            let gw = UnicodeWidthStr::width(g).max(1);
            if cur_w + gw > width && cur_w > 0 {
                flush_buf(&mut cur, &mut buf, style);
                out.push(Line::from(std::mem::take(&mut cur)));
                cur_w = 0;
            }
            buf.push_str(g);
            cur_w += gw;
        }
        flush_buf(&mut cur, &mut buf, style);
    }
    // Always push the final (possibly empty) line so blank lines survive.
    out.push(Line::from(cur));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(s: &str) -> TranscriptItem {
        TranscriptItem::System(s.to_string())
    }

    #[test]
    fn total_lines_counts_wrapped_rows() {
        let mut c = LayoutCache::default();
        // "  · " prefix (4) + 40 chars = 44 cols; at width 20 wraps to 3 rows.
        c.sync(
            &[item(&"x".repeat(40))],
            20,
            &std::collections::HashSet::new(),
            None,
        );
        assert_eq!(c.total_lines(), 3);
    }

    #[test]
    fn only_changed_block_rerenders_on_append() {
        let mut c = LayoutCache::default();
        let mut items = vec![item("stable one"), item("stable two")];
        c.sync(&items, 40, &std::collections::HashSet::new(), None);
        let h0 = super::hash_item(&items[0], false, false);
        c.sync(&items, 40, &std::collections::HashSet::new(), None);
        // Block 0's hash is unchanged → same identity retained.
        assert_eq!(c.blocks[0].hash, h0);
        // Append a new streaming block; earlier blocks keep their cache.
        items.push(item("streaming tail"));
        c.sync(&items, 40, &std::collections::HashSet::new(), None);
        assert_eq!(c.block_count(), 3);
        assert_eq!(c.blocks[0].hash, h0);
    }

    #[test]
    fn width_change_invalidates_all() {
        let mut c = LayoutCache::default();
        c.sync(
            &[item(&"y".repeat(30))],
            40,
            &std::collections::HashSet::new(),
            None,
        );
        let wide = c.total_lines();
        c.sync(
            &[item(&"y".repeat(30))],
            10,
            &std::collections::HashSet::new(),
            None,
        );
        assert!(c.total_lines() > wide, "narrower width wraps to more rows");
    }

    #[test]
    fn viewport_returns_requested_slice() {
        let mut c = LayoutCache::default();
        let items: Vec<_> = (0..10).map(|i| item(&format!("line {i}"))).collect();
        c.sync(&items, 80, &std::collections::HashSet::new(), None);
        assert_eq!(c.total_lines(), 10);
        let view = c.viewport(3, 4);
        assert_eq!(view.len(), 4);
    }

    #[test]
    fn no_wrapped_line_exceeds_width_cjk_and_emoji() {
        for width in [8usize, 12, 20, 33, 80] {
            let mut c = LayoutCache::default();
            let content = "日本語テキスト🎉🎉 mixed ascii 日本 more";
            c.sync(
                &[item(content)],
                width as u16,
                &std::collections::HashSet::new(),
                None,
            );
            for line in c.viewport(0, c.total_lines()) {
                let w: usize = line
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                assert!(w <= width, "line width {w} exceeds {width}");
            }
        }
    }

    fn read_ok(detail: &str) -> TranscriptItem {
        TranscriptItem::Tool {
            call_id: String::new(),
            name: "FileRead".into(),
            detail: detail.into(),
            result: Some("42 lines".into()),
            is_error: false,
        }
    }

    fn line_text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn three_reads_render_as_one_group_line() {
        let mut c = LayoutCache::default();
        let items = vec![read_ok("a.rs"), read_ok("b.rs"), read_ok("c.rs")];
        c.sync(&items, 80, &std::collections::HashSet::new(), None);
        // One folded block → one display line "▸ read 3 (a.rs, b.rs, …)".
        assert_eq!(c.total_lines(), 1);
        let text = line_text(&c.viewport(0, 1)[0]);
        assert!(text.contains("read 3"), "{text}");
        assert!(text.contains("a.rs"), "{text}");
    }

    #[test]
    fn typed_tool_card_shows_kind_and_status() {
        let mut c = LayoutCache::default();
        // A single failed bash card: red ✗, expanded result kept.
        let items = vec![TranscriptItem::Tool {
            call_id: String::new(),
            name: "Bash".into(),
            detail: "cargo test".into(),
            result: Some("exit 1".into()),
            is_error: true,
        }];
        c.sync(&items, 80, &std::collections::HashSet::new(), None);
        let all: String = c
            .viewport(0, c.total_lines())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("bash"), "kind label missing:\n{all}");
        assert!(all.contains('✗'), "error glyph missing:\n{all}");
        assert!(all.contains("exit 1"), "error output hidden:\n{all}");
    }

    #[test]
    fn truncate_drops_removed_blocks() {
        let mut c = LayoutCache::default();
        c.sync(
            &[item("a"), item("b"), item("c")],
            40,
            &std::collections::HashSet::new(),
            None,
        );
        assert_eq!(c.block_count(), 3);
        c.sync(&[item("a")], 40, &std::collections::HashSet::new(), None); // e.g. after /clear + one push
        assert_eq!(c.block_count(), 1);
        assert_eq!(c.total_lines(), 1);
    }
}
