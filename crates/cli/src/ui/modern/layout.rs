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

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::app::TranscriptItem;
use super::colors::palette;
use super::markdown::LinkSpan;
use crate::ui::text_safety::escape_deceptive;

struct Cached {
    hash: u64,
    lines: Vec<Line<'static>>,
    /// `cont[i]` — `lines[i]` is a soft-wrap continuation of `lines[i-1]`
    /// rather than the start of a logical line. Lets search operate on
    /// logical lines so a match can cross a display-wrap boundary.
    cont: Vec<bool>,
    /// Hyperlinks within this block. `line` is a **display** row index
    /// into `lines` (post-wrap).
    links: Vec<LinkSpan>,
}

/// Per-block rendered-line cache with a prefix-sum line index.
#[derive(Default)]
pub struct LayoutCache {
    width: u16,
    blocks: Vec<Cached>,
    total: usize,
    /// Bumped whenever a [`Self::sync`] actually changed cached lines, so
    /// derived state (in-transcript search) can rescan only on change
    /// instead of every frame.
    revision: u64,
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
        let mut changed = width_changed;

        // Fold consecutive read-only successes into groups (plan §M4); the
        // cache is keyed by display block, not raw item, so a group's hash
        // changes if any member does.
        let display = super::toolcard::plan_display(items);
        if display.len() != self.blocks.len() {
            changed = true;
        }
        self.blocks.truncate(display.len());

        for (i, d) in display.iter().enumerate() {
            let hash = match d {
                super::toolcard::Display::Single(idx) => {
                    let item = &items[*idx];
                    hash_item(item, expanded.contains(idx), selected == Some(*idx))
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
                    h.finish()
                }
            };
            match self.blocks.get(i) {
                Some(c) if c.hash == hash => {} // fresh
                _ => {
                    let (raw, logical_links) = match d {
                        super::toolcard::Display::Single(idx) => {
                            let item = &items[*idx];
                            let exp = expanded.contains(idx);
                            let sel = selected == Some(*idx);
                            render_item_with_links(item, exp, sel)
                        }
                        super::toolcard::Display::Group(idxs) => {
                            let sel = selected.is_some_and(|s| idxs.contains(&s));
                            (render_group(items, idxs, sel), Vec::new())
                        }
                    };
                    let (lines, cont, row_cols) = wrap_lines_tagged(raw, width);
                    let links = remap_links_through_wrap(&logical_links, &cont, &row_cols);
                    let entry = Cached {
                        hash,
                        lines,
                        cont,
                        links,
                    };
                    if i < self.blocks.len() {
                        self.blocks[i] = entry;
                    } else {
                        self.blocks.push(entry);
                    }
                    changed = true;
                }
            }
        }
        self.total = self.blocks.iter().map(|b| b.lines.len()).sum();
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Monotonic change counter: unchanged between two [`Self::sync`]
    /// calls iff the cached lines are byte-identical.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Drop every cached block so the next [`Self::sync`] re-renders the
    /// whole transcript. Used by the force-full-redraw chord (Ctrl+L),
    /// which exists precisely for the case where the cache is correct but
    /// the screen is not (another process wrote over the alt-screen).
    pub fn invalidate(&mut self) {
        self.blocks.clear();
        self.total = 0;
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

    /// Hyperlinks whose display row falls in `[top, top + height)`.
    ///
    /// Each link's `line` is rewritten to an **absolute** transcript line
    /// index so the draw path can map it onto a screen row.
    pub fn links_in_range(&self, top: usize, height: usize) -> Vec<LinkSpan> {
        if height == 0 {
            return Vec::new();
        }
        let end = top.saturating_add(height);
        let mut out = Vec::new();
        let mut base = 0usize;
        for block in &self.blocks {
            let block_end = base + block.lines.len();
            if block_end <= top {
                base = block_end;
                continue;
            }
            if base >= end {
                break;
            }
            for link in &block.links {
                let abs = base.saturating_add(link.line);
                if abs >= top && abs < end {
                    out.push(LinkSpan {
                        line: abs,
                        cols: link.cols.clone(),
                        url: link.url.clone(),
                    });
                }
            }
            base = block_end;
        }
        out
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
    ///
    /// Soft-wrap continuation rows are concatenated without a hard newline so
    /// a triple-click of a wrapped logical line copies as one line.
    pub fn plain_range(&self, start: usize, end: usize) -> Option<String> {
        let lo = start.min(end);
        let hi = start.max(end);
        if self.total == 0 {
            return None;
        }
        let hi = hi.min(self.total.saturating_sub(1));
        let mut out = String::new();
        let mut idx = 0usize;
        let mut any = false;
        for b in &self.blocks {
            for (li, line) in b.lines.iter().enumerate() {
                if idx >= lo && idx <= hi {
                    let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                    let cont = b.cont.get(li).copied().unwrap_or(false);
                    if any {
                        // Only insert a hard break when this row starts a
                        // new logical line (not a soft-wrap continuation).
                        if !(cont && idx > lo) {
                            out.push('\n');
                        }
                    }
                    out.push_str(&plain);
                    any = true;
                }
                idx += 1;
                if idx > hi {
                    return if any { Some(out) } else { None };
                }
            }
        }
        if any { Some(out) } else { None }
    }

    /// Absolute layout line index under a viewport-relative row.
    pub fn abs_line_at(&self, top: usize, row_in_view: usize) -> Option<usize> {
        let abs = top.saturating_add(row_in_view);
        if abs < self.total { Some(abs) } else { None }
    }

    /// Inclusive display-line span of the soft-wrap group containing `abs`.
    ///
    /// A triple-click selects this span so a wrapped logical line is
    /// captured as one unit (#558 D5-34).
    pub fn soft_wrap_span(&self, abs: usize) -> Option<(usize, usize)> {
        if abs >= self.total {
            return None;
        }
        let mut base = 0usize;
        for b in &self.blocks {
            let block_end = base + b.lines.len();
            if abs < block_end {
                let li = abs - base;
                // Walk back over continuation rows.
                let mut start_li = li;
                while start_li > 0 && b.cont.get(start_li).copied().unwrap_or(false) {
                    start_li -= 1;
                }
                // Walk forward over following continuations.
                let mut end_li = li;
                while end_li + 1 < b.lines.len() && b.cont.get(end_li + 1).copied().unwrap_or(false)
                {
                    end_li += 1;
                }
                return Some((base + start_li, base + end_li));
            }
            base = block_end;
        }
        None
    }

    /// Plain text grouped by logical line: each inner vec is the display
    /// rows `(absolute index, text)` one pre-wrap line occupies. Search
    /// scans these so a query can match across a display-wrap boundary.
    pub fn logical_rows(&self) -> Vec<Vec<(usize, String)>> {
        let mut out: Vec<Vec<(usize, String)>> = Vec::new();
        let mut abs = 0usize;
        for b in &self.blocks {
            for (li, line) in b.lines.iter().enumerate() {
                let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                let cont = b.cont.get(li).copied().unwrap_or(false);
                match out.last_mut() {
                    Some(rows) if cont => rows.push((abs, plain)),
                    _ => out.push(vec![(abs, plain)]),
                }
                abs += 1;
            }
        }
        out
    }
}

/// A copy of `item` with deceptive characters escaped, or `None` when it
/// has none.
///
/// See [`crate::ui::text_safety`]: the transcript renders file contents,
/// diffs and tool output, all of which can carry text designed to display
/// differently from the bytes it holds.
fn scrub_item(item: &TranscriptItem) -> Option<TranscriptItem> {
    use std::borrow::Cow;
    let dirty = |s: &str| matches!(escape_deceptive(s), Cow::Owned(_));
    let esc = |s: &str| escape_deceptive(s).into_owned();
    let esc_opt = |s: &Option<String>| s.as_deref().map(&esc);

    match item {
        TranscriptItem::User(t) => dirty(t).then(|| TranscriptItem::User(esc(t))),
        TranscriptItem::Assistant(t) => dirty(t).then(|| TranscriptItem::Assistant(esc(t))),
        TranscriptItem::Thinking { text, duration_ms } => {
            dirty(text).then(|| TranscriptItem::Thinking {
                text: esc(text),
                duration_ms: *duration_ms,
            })
        }
        TranscriptItem::System(t) => dirty(t).then(|| TranscriptItem::System(esc(t))),
        TranscriptItem::Error(t) => dirty(t).then(|| TranscriptItem::Error(esc(t))),
        TranscriptItem::Warning(t) => dirty(t).then(|| TranscriptItem::Warning(esc(t))),
        TranscriptItem::Tool {
            call_id,
            name,
            detail,
            result,
            is_error,
            live,
        } => {
            let any = dirty(name)
                || dirty(detail)
                || result.as_deref().is_some_and(dirty)
                || live.as_deref().is_some_and(dirty);
            any.then(|| TranscriptItem::Tool {
                call_id: call_id.clone(),
                name: esc(name),
                detail: esc(detail),
                result: esc_opt(result),
                is_error: *is_error,
                live: esc_opt(live),
            })
        }
    }
}

/// Map pre-wrap [`LinkSpan`]s onto post-wrap display rows.
///
/// Each logical line becomes one or more display rows (`cont` marks
/// soft-wrap continuations). `row_cols[i]` is the `[start, end)` range of
/// logical display columns covered by display row `i` — the same units as
/// [`LinkSpan::cols`], and the actual breakpoints from [`wrap_one`] (which
/// may underfill a row when a wide grapheme does not fit).
///
/// A link that spans multiple wrap rows emits one [`LinkSpan`] per display
/// row with local column ranges.
fn remap_links_through_wrap(
    links: &[LinkSpan],
    cont: &[bool],
    row_cols: &[(u16, u16)],
) -> Vec<LinkSpan> {
    if links.is_empty() || cont.is_empty() {
        return Vec::new();
    }
    // display_row of the start of each logical line.
    let mut logical_starts: Vec<usize> = Vec::new();
    for (i, &is_cont) in cont.iter().enumerate() {
        if !is_cont {
            logical_starts.push(i);
        }
    }
    let mut out = Vec::new();
    for l in links {
        let Some(&base) = logical_starts.get(l.line) else {
            continue;
        };
        let mut end_row = base + 1;
        while end_row < cont.len() && cont[end_row] {
            end_row += 1;
        }
        let link_start = l.cols.start;
        let link_end = l.cols.end.max(link_start.saturating_add(1));
        for row in base..end_row {
            let (rs, re) = row_cols.get(row).copied().unwrap_or((0, 0));
            if re <= rs {
                continue;
            }
            let o_start = link_start.max(rs);
            let o_end = link_end.min(re);
            if o_start < o_end {
                out.push(LinkSpan {
                    line: row,
                    cols: (o_start - rs)..(o_end - rs),
                    url: l.url.clone(),
                });
            }
        }
    }
    out
}

/// Render one transcript block to logical (pre-wrap) lines.
pub fn render_item(item: &TranscriptItem, expanded: bool, selected: bool) -> Vec<Line<'static>> {
    render_item_with_links(item, expanded, selected).0
}

/// Like [`render_item`], plus pre-wrap link spans for assistant markdown.
fn render_item_with_links(
    item: &TranscriptItem,
    expanded: bool,
    selected: bool,
) -> (Vec<Line<'static>>, Vec<LinkSpan>) {
    // Make bidi overrides and zero-width characters visible before any
    // arm renders. Done once here rather than per-variant so a new
    // `TranscriptItem` cannot quietly arrive unscrubbed, and it costs
    // nothing for the ordinary case: `scrub_item` returns `None` — no
    // clone — unless the text actually contains one.
    let scrubbed = scrub_item(item);
    let item = scrubbed.as_ref().unwrap_or(item);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut links: Vec<LinkSpan> = Vec::new();
    let sel = if selected {
        Span::styled("▌", Style::default().fg(palette().accent))
    } else {
        Span::raw(" ")
    };
    match item {
        TranscriptItem::User(t) => {
            // Tint the user's own turns so they're findable when scanning
            // back through a long transcript. The prefix selects the tint:
            // `!` is a shell passthrough and `#` a memory note, which read
            // as different kinds of input than a prompt.
            let p = palette();
            let (marker, bg) = match t.chars().next() {
                Some('!') => ("!", p.bash_msg_bg),
                Some('#') => ("#", p.memory_msg_bg),
                _ => ("❯", p.user_msg_bg),
            };
            let body = t.strip_prefix(['!', '#']).unwrap_or(t).trim_start();
            let text_style = Style::default().fg(p.text).bg(bg);
            lines.push(Line::from(vec![
                sel.clone(),
                Span::styled(
                    format!("{marker} "),
                    Style::default()
                        .fg(p.accent)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(body.to_string(), text_style),
                // One cell of trailing tint so the highlight reads as a
                // block rather than stopping flush against the last glyph.
                Span::styled(" ", text_style),
            ]));
            lines.push(Line::from(""));
        }
        TranscriptItem::Assistant(t) => {
            let md = super::markdown::render_markdown(t);
            let mut body = md.lines;
            let mut body_links = md.links;
            if !expanded {
                let max = 12;
                let total = body.len();
                if total > max {
                    body.truncate(max);
                    body_links.retain(|l| l.line < max);
                    body.push(Line::from(Span::styled(
                        "  … folded · press e to expand".to_string(),
                        Style::default()
                            .fg(palette().muted)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            // Selection gutter on the first body line shifts columns by one.
            for l in &mut body_links {
                if l.line == 0 {
                    l.cols.start = l.cols.start.saturating_add(1);
                    l.cols.end = l.cols.end.saturating_add(1);
                }
            }
            if let Some(first) = body.first_mut() {
                first.spans.insert(0, sel.clone());
            } else {
                lines.push(Line::from(sel.clone()));
            }
            let base = lines.len();
            for l in &mut body_links {
                l.line = l.line.saturating_add(base);
            }
            lines.extend(body);
            lines.push(Line::from(""));
            links = body_links;
        }
        TranscriptItem::Thinking { text, duration_ms } => {
            // Grok-style: collapsed header "Thought" / "Thought for Xs";
            // expanded body is dim italic markdown.
            let header = match duration_ms {
                Some(ms) if *ms > 0 => {
                    let secs = *ms as f64 / 1000.0;
                    if secs >= 10.0 {
                        format!("  Thought for {secs:.0}s")
                    } else {
                        format!("  Thought for {secs:.1}s")
                    }
                }
                Some(_) => "  Thought".to_string(),
                None => {
                    // Still streaming — show live-ish header + short preview.
                    if text.is_empty() {
                        "  Thinking…".to_string()
                    } else {
                        let preview: String = text.chars().take(48).collect();
                        format!(
                            "  Thinking… {}{}",
                            preview,
                            if text.chars().count() > 48 { "…" } else { "" }
                        )
                    }
                }
            };
            lines.push(Line::from(vec![
                sel.clone(),
                Span::styled(
                    header,
                    Style::default()
                        .fg(palette().muted)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
            if expanded {
                for mut line in super::markdown::render_markdown(text).lines {
                    for span in &mut line.spans {
                        span.style = span
                            .style
                            .fg(palette().muted)
                            .add_modifier(Modifier::ITALIC);
                    }
                    lines.push(line);
                }
            } else if !text.is_empty() {
                lines.push(Line::from(Span::styled(
                    "     (e expand · Ctrl+E all thinking)".to_string(),
                    Style::default().fg(palette().muted),
                )));
            }
        }
        TranscriptItem::Tool {
            name,
            detail,
            result,
            is_error,
            live,
            ..
        } => {
            // Prefer final result; while running, show live tail if any.
            let body = result.as_deref().or(live.as_deref());
            let running = result.is_none();
            lines.extend(render_tool_card(
                name, detail, body, *is_error, expanded, selected, running,
            ))
        }
        TranscriptItem::System(t) => {
            lines.push(Line::from(vec![
                sel,
                Span::styled(format!(" · {t}"), Style::default().fg(palette().muted)),
            ]));
        }
        TranscriptItem::Error(t) => {
            lines.push(Line::from(vec![
                sel,
                Span::styled(format!(" ✗ {t}"), Style::default().fg(palette().error)),
            ]));
        }
        TranscriptItem::Warning(t) => {
            lines.push(Line::from(vec![
                sel,
                Span::styled(format!(" ! {t}"), Style::default().fg(palette().warning)),
            ]));
        }
    }
    (lines, links)
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
    running: bool,
) -> Vec<Line<'static>> {
    use super::toolcard::ToolKind;
    let kind = ToolKind::classify(name);
    let (glyph, status_color) = match (running, is_error) {
        (true, _) => ("⚡", palette().warning), // running (may have live tail)
        (false, false) => ("✓", palette().success), // ok
        (false, true) => ("✗", palette().error), // failed
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
        Span::styled("· ", Style::default().fg(palette().muted)),
        Span::styled(detail.to_string(), Style::default().fg(palette().inactive)),
    ])];
    // Errors keep more of their output visible; successes stay compact
    // unless the user expanded the card (`e`).
    if let Some(r) = result
        && !r.is_empty()
    {
        // Rich inline diff for successful edit tools: FileEdit/MultiEdit return
        // a unified diff as their result, so render it as a syntax-highlighted
        // +/- diff (with word-level emphasis) rather than a flat dim block.
        // `detail` is the edited file path, used for syntax detection.
        if !is_error && kind == ToolKind::Edit && super::diffview::looks_like_unified_diff(r) {
            lines.extend(super::diffview::render_unified_diff(
                r, detail, expanded, 24,
            ));
        } else {
            let color = if is_error {
                palette().error
            } else {
                palette().muted
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
                        .fg(palette().muted)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }
    lines
}

/// Render a folded read-only group as a single summary line (plan §M4):
/// `▸ read N (first, second, …)`.
fn render_group(items: &[TranscriptItem], idxs: &[usize], selected: bool) -> Vec<Line<'static>> {
    // Folded groups bypass render_item, so the deceptive-character
    // scrub must run here too: a FileRead/Grep/WebFetch detail carrying
    // bidi or zero-width controls would otherwise reach the terminal
    // unescaped whenever three reads folded.
    let details: Vec<String> = idxs
        .iter()
        .filter_map(|&i| match &items[i] {
            TranscriptItem::Tool { detail, .. } => Some(escape_deceptive(detail).into_owned()),
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
                .fg(palette().success)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({shown}{more})"),
            Style::default().fg(palette().muted),
        ),
    ])]
}

/// Wrap logical lines to `width` display columns, unicode-width aware.
pub fn wrap_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    wrap_lines_tagged(lines, width).0
}

/// [`wrap_lines`] plus:
/// - a per-row continuation flag: `true` for rows that are soft-wrap
///   overflow of the previous row (never the first row of a logical line)
/// - per-row `[start, end)` ranges of logical display columns covered by
///   each display row (for remapping link hit rects through underfilled
///   wrap boundaries)
pub fn wrap_lines_tagged(
    lines: Vec<Line<'static>>,
    width: u16,
) -> (Vec<Line<'static>>, Vec<bool>, Vec<(u16, u16)>) {
    if width == 0 {
        let cont = vec![false; lines.len()];
        let row_cols = lines
            .iter()
            .map(|l| {
                let w = l
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()) as u16)
                    .fold(0u16, u16::saturating_add);
                (0, w)
            })
            .collect();
        return (lines, cont, row_cols);
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut cont = Vec::with_capacity(lines.len());
    let mut row_cols = Vec::with_capacity(lines.len());
    for line in lines {
        let first = out.len();
        wrap_one(line, width as usize, &mut out, &mut row_cols);
        cont.resize(out.len(), true);
        cont[first] = false;
    }
    (out, cont, row_cols)
}

/// Wrap a single styled line, preserving span styles across the split.
/// Splits on grapheme-cluster boundaries and never exceeds `width` columns.
///
/// Each emitted row appends `(logical_start, logical_end)` to `row_cols` —
/// the display-column range within the original logical line covered by
/// that row. Underfilled rows (wide grapheme would not fit the remainder)
/// record a range shorter than `width`.
fn wrap_one(
    line: Line<'static>,
    width: usize,
    out: &mut Vec<Line<'static>>,
    row_cols: &mut Vec<(u16, u16)>,
) {
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let mut row_start_col = 0u16;
    let mut logical_col = 0u16;
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
                row_cols.push((row_start_col, logical_col));
                row_start_col = logical_col;
                cur_w = 0;
            }
            buf.push_str(g);
            cur_w += gw;
            logical_col = logical_col.saturating_add(gw as u16);
        }
        flush_buf(&mut cur, &mut buf, style);
    }
    // Always push the final (possibly empty) line so blank lines survive.
    out.push(Line::from(cur));
    row_cols.push((row_start_col, logical_col));
}

#[cfg(test)]
mod tests {

    /// Tool results carry file contents and command output — the most
    /// likely place for text authored by someone other than the user.
    #[test]
    fn a_tool_result_cannot_smuggle_a_bidi_override_into_the_transcript() {
        let item = TranscriptItem::Tool {
            call_id: "c1".into(),
            name: "FileRead".into(),
            detail: "src/auth.rs".into(),
            result: Some("if user.is_admin \u{202e}{ grant() }\u{202c}".into()),
            is_error: false,
            live: None,
        };
        let text: String = render_item(&item, true, false)
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.to_string()))
            .collect();
        assert!(
            !text.contains('\u{202e}'),
            "override reached the transcript: {text:?}"
        );
    }

    /// Folded read groups render through `render_group`, not
    /// `render_item` — the scrub must hold on that path too (a Grep
    /// query or WebFetch URL with a bidi override, folded with two
    /// clean reads, previously reached the terminal unescaped).
    #[test]
    fn a_folded_read_group_cannot_smuggle_a_bidi_override() {
        let read = |detail: &str| TranscriptItem::Tool {
            call_id: "c".into(),
            name: "FileRead".into(),
            detail: detail.into(),
            result: Some("ok".into()),
            is_error: false,
            live: None,
        };
        let items = vec![
            read("src/a.rs"),
            read("evil\u{202e}sr.nigol\u{202c}.rs"),
            read("src/b.rs"),
        ];
        let text: String = render_group(&items, &[0, 1, 2], false)
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.to_string()))
            .collect();
        assert!(
            !text.contains('\u{202e}'),
            "override reached the folded group line: {text:?}"
        );
        assert!(
            text.contains("src/a.rs"),
            "clean details still render: {text:?}"
        );
    }

    #[test]
    fn logical_rows_join_soft_wraps_but_not_separate_items() {
        let mut c = LayoutCache::default();
        let items = vec![
            TranscriptItem::System("abcdefghijklmnopqrstuvwxyz".into()),
            TranscriptItem::System("short".into()),
        ];
        c.sync(&items, 10, &HashSet::new(), None);
        let logical = c.logical_rows();
        let alphabet = logical
            .iter()
            .find(|rows| {
                let joined: String = rows.iter().map(|(_, s)| s.as_str()).collect();
                joined.contains("abcdefghijklmnopqrstuvwxyz")
            })
            .expect("alphabet joins back together across its wrap rows");
        assert!(
            alphabet.len() > 1,
            "26 chars at width 10 must wrap: {alphabet:?}"
        );
        assert!(
            logical
                .iter()
                .any(|rows| rows.len() == 1 && rows[0].1.contains("short")),
            "separate items must stay separate logical lines"
        );
    }

    /// The search rescan is gated on this counter, so a sync that changed
    /// nothing must not bump it — otherwise every spinner frame rescans.
    #[test]
    fn revision_bumps_only_when_cached_lines_change() {
        let mut c = LayoutCache::default();
        let mut items = vec![
            TranscriptItem::System("one".into()),
            TranscriptItem::System("two".into()),
        ];
        let exp = HashSet::new();
        c.sync(&items, 80, &exp, None);
        let r1 = c.revision();
        c.sync(&items, 80, &exp, None);
        assert_eq!(c.revision(), r1, "no-op sync must not bump the revision");
        items.push(TranscriptItem::System("three".into()));
        c.sync(&items, 80, &exp, None);
        let r2 = c.revision();
        assert_ne!(r2, r1, "new content must bump the revision");
        c.sync(&items, 40, &exp, None);
        assert_ne!(c.revision(), r2, "a width change rewraps everything");
    }

    #[test]
    fn every_transcript_variant_is_scrubbed() {
        // Guards the entry-point approach: if a variant were rendered
        // without going through `scrub_item`, it would show up here.
        let rlo = "\u{202e}";
        let items = vec![
            TranscriptItem::User(format!("u{rlo}")),
            TranscriptItem::Assistant(format!("a{rlo}")),
            TranscriptItem::Thinking {
                text: format!("t{rlo}"),
                duration_ms: Some(1200),
            },
            TranscriptItem::System(format!("s{rlo}")),
            TranscriptItem::Error(format!("e{rlo}")),
            TranscriptItem::Warning(format!("w{rlo}")),
            TranscriptItem::Tool {
                call_id: "c".into(),
                name: "Bash".into(),
                detail: format!("d{rlo}"),
                result: Some(format!("r{rlo}")),
                is_error: false,
                live: None,
            },
        ];
        for item in items {
            let text: String = render_item(&item, true, false)
                .iter()
                .flat_map(|l| l.spans.iter().map(|sp| sp.content.to_string()))
                .collect();
            assert!(
                !text.contains(rlo),
                "unscrubbed variant {item:?} rendered: {text:?}"
            );
        }
    }

    /// Clean text must not be copied or altered on the way to the screen.
    #[test]
    fn ordinary_transcript_text_is_untouched() {
        let item = TranscriptItem::System("plain · text — with dashes".into());
        assert!(scrub_item(&item).is_none(), "cloned a clean item");
    }

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
    fn assistant_markdown_links_are_cached_and_queryable() {
        let mut c = LayoutCache::default();
        let items = vec![TranscriptItem::Assistant(
            "see [docs](https://example.com/a) for more".into(),
        )];
        c.sync(&items, 80, &std::collections::HashSet::new(), None);
        let links = c.links_in_range(0, c.total_lines());
        assert!(
            links.iter().any(|l| l.url == "https://example.com/a"),
            "expected assistant link in layout cache, got {links:?}"
        );
    }

    #[test]
    fn soft_wrap_span_covers_continuation_rows() {
        let mut c = LayoutCache::default();
        // Width 10 forces "abcdefghijklmnopqrstuvwxyz" into several rows.
        c.sync(
            &[TranscriptItem::System("abcdefghijklmnopqrstuvwxyz".into())],
            10,
            &HashSet::new(),
            None,
        );
        let total = c.total_lines();
        assert!(total > 1, "expected soft wraps, got {total}");
        let (lo, hi) = c.soft_wrap_span(1).expect("span");
        assert_eq!(lo, 0, "group starts at first display row");
        assert_eq!(hi, total - 1, "group covers all wrapped rows of the item");
        // Copy must not inject hard newlines for soft wraps.
        let plain = c.plain_range(lo, hi).expect("plain");
        assert!(
            !plain.contains('\n'),
            "soft-wrap copy should be one logical line: {plain:?}"
        );
        assert!(plain.contains("abcdefghijklmnopqrstuvwxyz") || plain.contains("abcdefghij"));
    }

    #[test]
    fn remap_links_splits_across_wrap_rows() {
        // Logical line index 0, link covering cols 5..25 with full-width
        // rows of 10 → three display rows: 5..10, 0..10, 0..5.
        let cont = vec![false, true, true]; // one logical line, three rows
        let row_cols = vec![(0, 10), (10, 20), (20, 25)];
        let links = vec![LinkSpan {
            line: 0,
            cols: 5..25,
            url: "https://example.com".into(),
        }];
        let mapped = remap_links_through_wrap(&links, &cont, &row_cols);
        assert_eq!(mapped.len(), 3, "{mapped:?}");
        assert_eq!(mapped[0].line, 0);
        assert_eq!(mapped[0].cols, 5..10);
        assert_eq!(mapped[1].line, 1);
        assert_eq!(mapped[1].cols, 0..10);
        assert_eq!(mapped[2].line, 2);
        assert_eq!(mapped[2].cols, 0..5);
        assert!(mapped.iter().all(|l| l.url == "https://example.com"));
    }

    #[test]
    fn remap_links_uses_underfilled_wrap_boundaries() {
        // Width 5: "aaaa界link" wraps after the four a's (界 is 2 cells and
        // does not fit the remainder). Link "link" sits at cols 6..10.
        // Naive col/width would put start at row 1 col 1; the real row
        // starts at logical col 4, so the link begins at local col 2.
        let line = Line::from("aaaa界link");
        let (wrapped, cont, row_cols) = wrap_lines_tagged(vec![line], 5);
        assert!(wrapped.len() >= 2, "expected wrap: {wrapped:?}");
        assert_eq!(row_cols[0], (0, 4), "first row underfills: {row_cols:?}");
        let links = vec![LinkSpan {
            line: 0,
            cols: 6..10,
            url: "https://example.com".into(),
        }];
        let mapped = remap_links_through_wrap(&links, &cont, &row_cols);
        assert!(!mapped.is_empty(), "{mapped:?}");
        assert_eq!(mapped[0].line, 1, "{mapped:?}");
        assert_eq!(mapped[0].cols.start, 2, "{mapped:?}");
        assert_eq!(mapped[0].url, "https://example.com");
        // Continuation of the link on later rows, if any, must stay local.
        for m in &mapped {
            assert!(m.cols.end > m.cols.start, "{m:?}");
            assert!(m.cols.end <= 5, "local col past width: {m:?}");
        }
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
            live: None,
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
            live: None,
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

#[cfg(test)]
mod user_message_tint_tests {
    use super::*;

    fn render_user(text: &str) -> Vec<Line<'static>> {
        render_item(&TranscriptItem::User(text.into()), false, false)
    }

    /// Every span of the user's line carries a background, so the turn
    /// reads as a tinted block. These theme slots existed but nothing
    /// consumed them, leaving user input visually identical to output.
    #[test]
    fn user_turn_is_tinted_across_the_whole_line() {
        let lines = render_user("hello world");
        let spans = &lines[0].spans;
        // Skip the leading selection gutter cell.
        let painted: Vec<_> = spans.iter().skip(1).collect();
        assert!(!painted.is_empty());
        for s in painted {
            assert!(
                s.style.bg.is_some(),
                "every span after the gutter must be tinted: {s:?}"
            );
        }
    }

    #[test]
    fn prefix_selects_a_distinct_tint_per_input_kind() {
        let bg = |t: &str| render_user(t)[0].spans[2].style.bg.unwrap();
        let prompt = bg("hello");
        let shell = bg("!ls -la");
        let memory = bg("#remember this");
        assert_ne!(prompt, shell, "shell passthrough needs its own tint");
        assert_ne!(prompt, memory, "memory note needs its own tint");
        assert_ne!(shell, memory);
    }

    #[test]
    fn marker_is_stripped_from_the_rendered_body() {
        let lines = render_user("!ls -la");
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("ls -la"));
        assert!(
            !text.contains("!ls"),
            "marker should not be duplicated: {text}"
        );
    }
}
