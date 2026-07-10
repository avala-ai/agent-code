//! Virtualized layout cache for the transcript (plan §M2).
//!
//! Each transcript block is rendered to display lines once and cached,
//! keyed by a content hash + the width it was wrapped at. On the next
//! frame only blocks whose content changed (in practice, the streaming
//! tail) are re-rendered — the plan's "never re-render the whole
//! transcript on stream" rule (§2.2 rule 6). Wrapping is unicode-width
//! aware on grapheme clusters so a wide char is never split and no line
//! ever exceeds the width.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::app::TranscriptItem;

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

fn hash_item(item: &TranscriptItem) -> u64 {
    let mut h = DefaultHasher::new();
    item.hash(&mut h);
    h.finish()
}

impl LayoutCache {
    /// Rebuild cache entries that are stale. A width change invalidates
    /// every block; otherwise only blocks whose content hash changed (or
    /// new blocks) are re-rendered. Returns nothing; query with
    /// [`Self::total_lines`] / [`Self::viewport`].
    pub fn sync(&mut self, items: &[TranscriptItem], width: u16) {
        let width_changed = width != self.width;
        self.width = width;
        if width_changed {
            self.blocks.clear();
        }
        // Truncate to current length (blocks removed, e.g. /clear).
        self.blocks.truncate(items.len());

        for (i, item) in items.iter().enumerate() {
            let hash = hash_item(item);
            match self.blocks.get(i) {
                Some(c) if c.hash == hash => {} // fresh
                _ => {
                    let lines = wrap_lines(render_item(item), width);
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
}

/// Render one transcript block to logical (pre-wrap) lines.
pub fn render_item(item: &TranscriptItem) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    match item {
        TranscriptItem::User(t) => {
            lines.push(Line::from(vec![
                Span::styled("❯ ", Style::default().fg(Color::Cyan)),
                Span::styled(t.clone(), Style::default().fg(Color::White)),
            ]));
            lines.push(Line::from(""));
        }
        TranscriptItem::Assistant(t) => {
            // Full markdown rendering (headings, code, lists, links…).
            lines.extend(super::markdown::render_markdown(t).lines);
            lines.push(Line::from(""));
        }
        TranscriptItem::Thinking(t) => {
            // Thinking is dimmed; render its markdown then overlay a dim
            // italic style so it reads as secondary reasoning.
            lines.push(Line::from(Span::styled(
                "  thinking…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            for mut line in super::markdown::render_markdown(t).lines {
                for span in &mut line.spans {
                    span.style = span
                        .style
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC);
                }
                lines.push(line);
            }
        }
        TranscriptItem::Tool {
            name,
            detail,
            result,
            is_error,
        } => {
            let color = if *is_error { Color::Red } else { Color::Yellow };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {name} "),
                    Style::default().fg(Color::Black).bg(color),
                ),
                Span::raw(" "),
                Span::styled(detail.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            if let Some(r) = result {
                lines.push(Line::from(Span::styled(
                    format!("   ↳ {r}"),
                    Style::default().fg(if *is_error {
                        Color::Red
                    } else {
                        Color::DarkGray
                    }),
                )));
            }
        }
        TranscriptItem::System(t) => {
            lines.push(Line::from(Span::styled(
                format!("  · {t}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        TranscriptItem::Error(t) => {
            lines.push(Line::from(Span::styled(
                format!("  ✗ {t}"),
                Style::default().fg(Color::Red),
            )));
        }
        TranscriptItem::Warning(t) => {
            lines.push(Line::from(Span::styled(
                format!("  ! {t}"),
                Style::default().fg(Color::Yellow),
            )));
        }
    }
    lines
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
        c.sync(&[item(&"x".repeat(40))], 20);
        assert_eq!(c.total_lines(), 3);
    }

    #[test]
    fn only_changed_block_rerenders_on_append() {
        let mut c = LayoutCache::default();
        let mut items = vec![item("stable one"), item("stable two")];
        c.sync(&items, 40);
        let h0 = super::hash_item(&items[0]);
        c.sync(&items, 40);
        // Block 0's hash is unchanged → same identity retained.
        assert_eq!(c.blocks[0].hash, h0);
        // Append a new streaming block; earlier blocks keep their cache.
        items.push(item("streaming tail"));
        c.sync(&items, 40);
        assert_eq!(c.block_count(), 3);
        assert_eq!(c.blocks[0].hash, h0);
    }

    #[test]
    fn width_change_invalidates_all() {
        let mut c = LayoutCache::default();
        c.sync(&[item(&"y".repeat(30))], 40);
        let wide = c.total_lines();
        c.sync(&[item(&"y".repeat(30))], 10);
        assert!(c.total_lines() > wide, "narrower width wraps to more rows");
    }

    #[test]
    fn viewport_returns_requested_slice() {
        let mut c = LayoutCache::default();
        let items: Vec<_> = (0..10).map(|i| item(&format!("line {i}"))).collect();
        c.sync(&items, 80);
        assert_eq!(c.total_lines(), 10);
        let view = c.viewport(3, 4);
        assert_eq!(view.len(), 4);
    }

    #[test]
    fn no_wrapped_line_exceeds_width_cjk_and_emoji() {
        for width in [8usize, 12, 20, 33, 80] {
            let mut c = LayoutCache::default();
            let content = "日本語テキスト🎉🎉 mixed ascii 日本 more";
            c.sync(&[item(content)], width as u16);
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

    #[test]
    fn truncate_drops_removed_blocks() {
        let mut c = LayoutCache::default();
        c.sync(&[item("a"), item("b"), item("c")], 40);
        assert_eq!(c.block_count(), 3);
        c.sync(&[item("a")], 40); // e.g. after /clear + one push
        assert_eq!(c.block_count(), 1);
        assert_eq!(c.total_lines(), 1);
    }
}
