//! Markdown → styled ratatui lines for the modern TUI (plan §M3).
//!
//! Assistant, thinking, and plan-preview blocks render their markdown
//! source through [`render_markdown`]. Wrapping is left to the layout
//! cache (which is unicode-aware), so this module only produces logical
//! styled lines. Fenced code is highlighted with syntect, whose syntax and
//! theme sets are loaded once via `OnceLock`. Rendering is memoized per
//! block by the layout cache's content-hash keying, so a streaming block
//! only re-parses on its own flushes.
//!
//! Within a streaming fenced code block the highlighter state is kept
//! across flushes so only newly-appended lines run through syntect
//! (D3-26): a growing block no longer re-highlights every previous line
//! on every ~10 Hz flush.

use std::cell::RefCell;
use std::ops::Range;
use std::sync::OnceLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::colors::palette;

/// A clickable link discovered while rendering (line index + column range +
/// destination). Consumed by mouse/OSC-8 handling in a later milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    pub line: usize,
    pub cols: Range<u16>,
    pub url: String,
}

/// Rendered markdown: styled lines plus the links found within them.
#[derive(Debug, Clone, Default)]
pub struct RenderedMd {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<LinkSpan>,
}

/// Guard against pathological input producing unbounded styled spans
/// (plan §7 span budget). Beyond this the tail renders unstyled.
const MAX_SPANS: usize = 20_000;

pub(super) fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Syntax-highlighting theme, matched to the active product theme's
/// polarity.
///
/// The highlighter ships its own palette, and its foregrounds are drawn
/// for a particular background: `base16-ocean.dark`'s pale greens and
/// blues are illegible on the near-white `code_bg` a light theme
/// derives. Now that the code background follows the theme, the syntax
/// theme has to follow it too.
pub(super) fn code_theme() -> &'static Theme {
    static DARK: OnceLock<Theme> = OnceLock::new();
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    if crate::ui::theme::current().is_dark {
        DARK.get_or_init(|| load_code_theme(&["base16-ocean.dark"]))
    } else {
        // InspiredGitHub ahead of base16-ocean.light: the base16 light
        // palettes are deliberately low-contrast, and their keyword
        // purple only reaches about 2.4:1 on a cream code background.
        LIGHT.get_or_init(|| load_code_theme(&["InspiredGitHub", "base16-ocean.light"]))
    }
}

/// First of `names` present in syntect's defaults, or any theme at all
/// rather than panicking on a bundled-asset change.
fn load_code_theme(names: &[&str]) -> Theme {
    let mut set = ThemeSet::load_defaults();
    for name in names {
        if let Some(t) = set.themes.remove(*name) {
            return t;
        }
    }
    set.themes
        .into_values()
        .next()
        .expect("syntect ships at least one default theme")
}

/// Render markdown source to styled lines.
pub fn render_markdown(src: &str) -> RenderedMd {
    let mut b = Builder::default();
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    for ev in Parser::new_ext(src, opts) {
        if b.spans_emitted > MAX_SPANS {
            break;
        }
        b.event(ev);
    }
    b.finish_line();
    RenderedMd {
        lines: b.lines,
        links: b.links,
    }
}

#[derive(Default)]
struct Builder {
    lines: Vec<Line<'static>>,
    links: Vec<LinkSpan>,
    cur: Vec<Span<'static>>,
    cur_cols: u16,
    spans_emitted: usize,

    // Inline style state.
    bold: bool,
    italic: bool,
    strike: bool,
    // List nesting: each entry is Some(next_number) for ordered, None for bullet.
    lists: Vec<Option<u64>>,
    quote_depth: usize,

    // Active link destination + the column where its text started.
    link: Option<(String, u16)>,

    // Heading level currently being built (styled on end).
    pending_heading: Option<HeadingLevel>,

    // Table state: cells accumulate per row; row flushes on TagEnd::TableRow.
    in_table: bool,
    table_row: Vec<String>,
    table_cell: String,

    // Fenced-code state.
    code: Option<(String, String)>, // (lang, accumulated content)
}

impl Builder {
    fn inline_style(&self) -> Style {
        let mut s = Style::default();
        if self.bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link.is_some() {
            s = s.fg(palette().accent).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn line_prefix(&self) -> Vec<Span<'static>> {
        let mut p = Vec::new();
        for _ in 0..self.quote_depth {
            p.push(Span::styled(
                "▎ ",
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
        p
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if self.cur.is_empty() && self.cur_cols == 0 {
            let prefix = self.line_prefix();
            for sp in prefix {
                self.cur_cols += sp.content.chars().count() as u16;
                self.cur.push(sp);
            }
        }
        self.cur_cols += text.chars().count() as u16;
        self.cur.push(Span::styled(text.to_string(), style));
        self.spans_emitted += 1;
    }

    fn finish_line(&mut self) {
        if !self.cur.is_empty() {
            self.lines.push(Line::from(std::mem::take(&mut self.cur)));
        }
        self.cur_cols = 0;
    }

    fn blank_line(&mut self) {
        self.finish_line();
        // Collapse consecutive blanks.
        if !matches!(self.lines.last(), Some(l) if l.spans.is_empty()) {
            self.lines.push(Line::from(""));
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some((_, buf)) = self.code.as_mut() {
                    buf.push_str(&t);
                } else if self.in_table {
                    self.table_cell.push_str(&t);
                } else {
                    let style = self.inline_style();
                    self.push_text(&t, style);
                }
            }
            Event::Code(t) => {
                let style = Style::default().fg(palette().code_fg).bg(palette().code_bg);
                self.push_text(&format!(" {t} "), style);
            }
            Event::SoftBreak => {
                let style = self.inline_style();
                self.push_text(" ", style);
            }
            Event::HardBreak => self.finish_line(),
            Event::Rule => {
                self.finish_line();
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(24),
                    Style::default().fg(palette().muted),
                )));
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                self.push_text(mark, Style::default().fg(palette().accent));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.finish_line(),
            Tag::Heading { level, .. } => {
                self.blank_line();
                self.pending_heading = Some(level);
            }
            Tag::Strong => self.bold = true,
            Tag::Emphasis => self.italic = true,
            Tag::Strikethrough => self.strike = true,
            Tag::Link { dest_url, .. } => {
                self.link = Some((dest_url.to_string(), self.cur_cols));
            }
            Tag::List(start) => self.lists.push(start),
            Tag::Item => {
                self.finish_line();
                let depth = self.lists.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_string(),
                };
                self.push_text(&indent, Style::default());
                self.push_text(&marker, Style::default().fg(palette().accent));
            }
            Tag::BlockQuote(_) => self.quote_depth += 1,
            Tag::CodeBlock(kind) => {
                self.finish_line();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::Table(_) => {
                self.blank_line();
                self.in_table = true;
                self.table_row.clear();
                self.table_cell.clear();
            }
            Tag::TableHead | Tag::TableRow => {
                self.table_row.clear();
                self.table_cell.clear();
            }
            Tag::TableCell => {
                self.table_cell.clear();
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.blank_line(),
            TagEnd::Heading(_) => {
                let level = self.pending_heading.take();
                // Apply heading style to the spans on the current line, then
                // flush. H1 also gets a dim underline rule.
                let style = match level {
                    Some(HeadingLevel::H1) => Style::default()
                        .fg(palette().accent)
                        .add_modifier(Modifier::BOLD),
                    Some(HeadingLevel::H2) => Style::default()
                        .fg(palette().accent)
                        .add_modifier(Modifier::BOLD),
                    Some(HeadingLevel::H3) => Style::default()
                        .fg(palette().inactive)
                        .add_modifier(Modifier::BOLD),
                    Some(_) => Style::default()
                        .fg(palette().inactive)
                        .add_modifier(Modifier::BOLD),
                    None => Style::default().add_modifier(Modifier::BOLD),
                };
                for sp in &mut self.cur {
                    sp.style = style;
                }
                self.finish_line();
                if let Some(HeadingLevel::H1) = level
                    && let Some(last) = self.lines.last()
                {
                    let w = last
                        .spans
                        .iter()
                        .map(|s| s.content.chars().count())
                        .sum::<usize>()
                        .max(3);
                    self.lines.push(Line::from(Span::styled(
                        "─".repeat(w),
                        Style::default().fg(palette().muted),
                    )));
                }
                self.blank_line();
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.table_cell);
                self.table_row.push(cell.trim().to_string());
            }
            TagEnd::TableRow | TagEnd::TableHead => {
                if !self.table_row.is_empty() {
                    let line = self
                        .table_row
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join(" │ ");
                    self.lines.push(Line::from(Span::styled(
                        format!(" {line} "),
                        Style::default().fg(palette().inactive),
                    )));
                    self.table_row.clear();
                }
            }
            TagEnd::Table => {
                self.in_table = false;
                self.table_row.clear();
                self.table_cell.clear();
                self.blank_line();
            }
            TagEnd::Strong => self.bold = false,
            TagEnd::Emphasis => self.italic = false,
            TagEnd::Strikethrough => self.strike = false,
            TagEnd::Link => {
                if let Some((url, start_col)) = self.link.take() {
                    self.links.push(LinkSpan {
                        line: self.lines.len(),
                        cols: start_col..self.cur_cols,
                        url,
                    });
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.blank_line();
            }
            TagEnd::Item => self.finish_line(),
            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.blank_line();
            }
            TagEnd::CodeBlock => {
                if let Some((lang, content)) = self.code.take() {
                    self.emit_code_block(&lang, &content);
                }
            }
            _ => {}
        }
    }

    fn emit_code_block(&mut self, lang: &str, content: &str) {
        let accent = palette().accent;
        let muted = palette().muted;
        let tag = if lang.is_empty() {
            "code".to_string()
        } else {
            lang.to_string()
        };
        let n_lines = content.lines().count().max(1);
        let num_w = n_lines.to_string().len().max(2);

        // Header: accent bar · language pill · line count · copy hint.
        self.lines.push(Line::from(vec![
            Span::styled("╭─ ", Style::default().fg(accent)),
            Span::styled(
                format!(" {tag} "),
                Style::default()
                    .fg(super::colors::on_fill(accent))
                    .bg(accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {n_lines} lines"),
                Style::default().fg(muted).add_modifier(Modifier::DIM),
            ),
            Span::styled(
                "  · y copy block",
                Style::default().fg(muted).add_modifier(Modifier::DIM),
            ),
        ]));

        let body = highlight_code_body(lang, content, num_w);
        self.spans_emitted = self
            .spans_emitted
            .saturating_add(body.len().saturating_mul(4));
        self.lines.extend(body);

        // Footer rule.
        self.lines
            .push(Line::from(Span::styled("╰─", Style::default().fg(accent))));
    }
}

/// Incremental syntect state for a streaming fenced block.
///
/// Layout re-renders the streaming assistant block on every flush; without
/// this, every previously highlighted line is paid for again. When the
/// fence content grows by pure append, only the new lines run through
/// `highlight_line`. A language change, non-prefix edit, gutter-width
/// change, or theme polarity change resets (cached spans bake in the
/// code-theme and `code_bg` of the paint that produced them).
struct CodeHlStream {
    lang: String,
    /// Content already fed to `hl` (and covered by `body`).
    content: String,
    body: Vec<Line<'static>>,
    num_w: usize,
    /// Theme polarity at last paint — light and dark ship different
    /// syntect themes, and `code_bg` moves with the product theme.
    dark: bool,
    /// `None` after a reset; recreated on the next append.
    hl: Option<HighlightLines<'static>>,
}

impl CodeHlStream {
    fn reset(&mut self, lang: &str, num_w: usize, dark: bool) {
        self.lang = lang.to_string();
        self.content.clear();
        self.body.clear();
        self.num_w = num_w;
        self.dark = dark;
        self.hl = None;
    }

    fn ensure_hl(&mut self, lang: &str) {
        if self.hl.is_some() {
            return;
        }
        let ss = syntax_set();
        let syntax = ss
            .find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(lang))
            .unwrap_or_else(|| ss.find_syntax_plain_text());
        // SyntaxSet / Theme are process-static; HighlightLines can be 'static.
        self.hl = Some(HighlightLines::new(syntax, code_theme()));
    }
}

thread_local! {
    static CODE_HL_STREAM: RefCell<CodeHlStream> = const {
        RefCell::new(CodeHlStream {
            lang: String::new(),
            content: String::new(),
            body: Vec::new(),
            num_w: 2,
            dark: true,
            hl: None,
        })
    };
}

/// Split into newline-terminated prefix (safe to cache) and optional
/// incomplete final line (re-highlighted every flush).
fn complete_and_tail(content: &str) -> (&str, &str) {
    match content.rfind('\n') {
        Some(i) => (&content[..=i], &content[i + 1..]),
        None => ("", content),
    }
}

fn highlight_one_line(
    hl: &mut HighlightLines<'static>,
    line: &str,
    line_no: usize,
    num_w: usize,
) -> Line<'static> {
    let ss = syntax_set();
    let accent = palette().accent;
    let muted = palette().muted;
    let gutter = Style::default().fg(muted);
    let body_bg = palette().code_bg;
    // syntect wants a trailing newline for line-oriented state.
    let fed = if line.ends_with('\n') {
        line.to_string()
    } else {
        format!("{line}\n")
    };
    let mut spans = vec![
        Span::styled("│ ", Style::default().fg(accent)),
        Span::styled(
            format!("{:>width$} ", line_no, width = num_w),
            gutter.add_modifier(Modifier::DIM),
        ),
    ];
    match hl.highlight_line(&fed, ss) {
        Ok(ranges) => {
            let mut any = false;
            for (sty, text) in ranges {
                let t = text.trim_end_matches('\n');
                if t.is_empty() {
                    continue;
                }
                any = true;
                let c = sty.foreground;
                spans.push(Span::styled(
                    t.to_string(),
                    Style::default()
                        .fg(super::colors::syntax_color(c.r, c.g, c.b))
                        .bg(body_bg),
                ));
            }
            if !any {
                spans.push(Span::styled(" ", Style::default().bg(body_bg)));
            }
        }
        Err(_) => spans.push(Span::styled(
            line.trim_end_matches('\n').to_string(),
            Style::default().fg(palette().inactive).bg(body_bg),
        )),
    }
    Line::from(spans)
}

/// Highlight the body of a fenced code block, reusing work when the
/// newline-terminated prefix is a pure append of the previous call.
///
/// Incomplete final lines (common while streaming) are re-highlighted
/// each flush without poisoning the cached highlighter state: only
/// complete lines are fed into the persistent `HighlightLines`.
fn highlight_code_body(lang: &str, content: &str, num_w: usize) -> Vec<Line<'static>> {
    let (complete, tail) = complete_and_tail(content);
    let dark = crate::ui::theme::current().is_dark;

    CODE_HL_STREAM.with(|cell| {
        let mut cache = cell.borrow_mut();
        let can_extend = cache.lang == lang
            && cache.num_w == num_w
            && cache.dark == dark
            && complete.starts_with(&cache.content)
            && (cache.hl.is_some() || cache.content.is_empty());

        if !can_extend {
            cache.reset(lang, num_w, dark);
        }

        if complete.len() > cache.content.len() {
            cache.ensure_hl(lang);
            let mut hl = cache.hl.take().expect("ensure_hl");
            let already = cache.content.len();
            let suffix = &complete[already..];
            let start_idx = cache.body.len();
            for (offset, line) in LinesWithEndings::from(suffix).enumerate() {
                let i = start_idx + offset;
                cache
                    .body
                    .push(highlight_one_line(&mut hl, line, i + 1, num_w));
            }
            cache.hl = Some(hl);
            cache.content = complete.to_string();
        }

        let mut out = cache.body.clone();
        if !tail.is_empty() {
            // Incomplete last line: highlight in isolation with a fresh
            // highlighter. Multi-line context is approximate until the
            // line terminates and is absorbed into the cached stream —
            // that is intentional so a growing tail stays O(1) per flush.
            let ss = syntax_set();
            let syntax = ss
                .find_syntax_by_token(lang)
                .or_else(|| ss.find_syntax_by_extension(lang))
                .unwrap_or_else(|| ss.find_syntax_plain_text());
            let mut hl = HighlightLines::new(syntax, code_theme());
            let line_no = out.len() + 1;
            out.push(highlight_one_line(&mut hl, tail, line_no, num_w));
        }
        out
    })
}

/// Drop the stream highlight cache so fenced-block memory is released
/// (e.g. on `/clear`) and tests do not leak state into each other.
pub(super) fn reset_code_hl_stream() {
    CODE_HL_STREAM.with(|cell| {
        cell.borrow_mut().reset("", 2, true);
    });
}

#[cfg(test)]
fn clear_code_hl_stream() {
    reset_code_hl_stream();
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn text_of(md: &RenderedMd) -> String {
        md.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn streaming_code_block_reuses_highlight_on_append() {
        clear_code_hl_stream();
        // Incomplete last line while streaming.
        let a = highlight_code_body("rust", "fn mai", 2);
        assert_eq!(a.len(), 1, "incomplete tail is one line");
        // Line completes and more lines arrive.
        let b = highlight_code_body("rust", "fn main() {\n    let x = 1;\n", 2);
        assert_eq!(b.len(), 2);
        // Pure complete-line append.
        let c = highlight_code_body("rust", "fn main() {\n    let x = 1;\n}\n", 2);
        assert_eq!(c.len(), 3, "append extends body without full reset");
        // Identical content is a cache hit.
        let d = highlight_code_body("rust", "fn main() {\n    let x = 1;\n}\n", 2);
        assert_eq!(d.len(), 3);
        // Language change must reset.
        let e = highlight_code_body("python", "print(1)\n", 2);
        assert_eq!(e.len(), 1);
        clear_code_hl_stream();
    }

    /// Markdown was the last part of the transcript still painting
    /// hardcoded RGB: the inline-code chip, the code-block background,
    /// and syntect's own highlight palette. None of those reach
    /// `adapt_for_emit` on their own, so under `NO_COLOR` they kept
    /// emitting 24-bit colour long after the chrome had stopped.
    #[test]
    fn no_colour_mode_strips_inline_code_and_highlighting() {
        use crate::ui::color_emit::{EmitMode, pin_mode};
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let _mode = pin_mode(EmitMode::Mono);
        clear_code_hl_stream();

        let md = render_markdown("uses `run()` here\n\n```rust\nfn main() { let x = 1; }\n```\n");
        let mut offenders = Vec::new();
        for line in &md.lines {
            for span in &line.spans {
                for (what, c) in [("fg", span.style.fg), ("bg", span.style.bg)] {
                    if let Some(c) = c
                        && c != ratatui::style::Color::Reset
                    {
                        offenders.push(format!("{what}={c:?} on {:?}", span.content));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "markdown still carries colour under NO_COLOR: {offenders:?}"
        );
    }

    /// The syntax theme has to follow the product theme's polarity.
    ///
    /// `code_bg` now comes from the palette, so on a light theme the
    /// code block is near-white. The highlighter kept its own fixed
    /// `base16-ocean.dark` palette, whose pale foregrounds were drawn
    /// for a dark background — light-on-light, unreadable. Every
    /// highlighted run must clear a legibility floor against the
    /// background it is actually painted on.
    #[test]
    fn highlighting_stays_legible_against_the_code_background() {
        use super::super::colors::{contrast, luminance};
        let _g = crate::ui::theme::test_lock();
        for name in ["one-dark", "solarized-light"] {
            clear_code_hl_stream();
            crate::ui::theme::init(name);
            let bg = palette().code_bg;
            let bg_l = luminance(bg).expect("code_bg is rgb on these themes");
            let md = render_markdown("```rust\nfn main() { let x = 1; }\n```\n");
            let mut checked = 0;
            for line in &md.lines {
                for span in &line.spans {
                    if span.style.bg != Some(bg) || span.content.trim().is_empty() {
                        continue;
                    }
                    let Some(fg_l) = span.style.fg.and_then(luminance) else {
                        continue;
                    };
                    let ratio = contrast(fg_l, bg_l);
                    assert!(
                        ratio >= 3.0,
                        "{name}: {:?} reaches only {ratio:.2}:1 on {bg:?}",
                        span.content
                    );
                    checked += 1;
                }
            }
            assert!(checked > 0, "{name}: no highlighted runs were checked");
        }
        crate::ui::theme::init("one-dark");
    }

    /// one-dark → solarized-light must invalidate the stream cache: cached
    /// spans bake in the dark `code_bg` / syntect theme and would paint
    /// unreadable light-on-light if reused after a polarity flip.
    #[test]
    fn theme_polarity_change_resets_stream_cache() {
        let _g = crate::ui::theme::test_lock();
        clear_code_hl_stream();
        crate::ui::theme::init("one-dark");
        let dark_bg = palette().code_bg;
        let a = highlight_code_body("rust", "fn main() {}\n", 2);
        assert!(
            a.iter()
                .any(|l| l.spans.iter().any(|s| s.style.bg == Some(dark_bg))),
            "dark theme should paint against dark code_bg"
        );

        crate::ui::theme::init("solarized-light");
        let light_bg = palette().code_bg;
        assert_ne!(dark_bg, light_bg, "fixture themes must differ in code_bg");
        let b = highlight_code_body("rust", "fn main() {}\n", 2);
        assert!(
            b.iter()
                .any(|l| l.spans.iter().any(|s| s.style.bg == Some(light_bg))),
            "polarity flip must re-highlight against light code_bg"
        );
        assert!(
            !b.iter()
                .any(|l| l.spans.iter().any(|s| s.style.bg == Some(dark_bg))),
            "must not reuse dark-theme cached spans after light theme"
        );
        clear_code_hl_stream();
        crate::ui::theme::init("one-dark");
    }

    /// The inline-code chip and the fenced block share one pair of
    /// palette slots, so a theme swap moves both together.
    #[test]
    fn inline_code_uses_the_theme_code_slots() {
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let md = render_markdown("uses `run()` here");
        let chip = md
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("run()"))
            .expect("inline code span");
        assert_eq!(chip.style.fg, Some(palette().code_fg));
        assert_eq!(chip.style.bg, Some(palette().code_bg));
    }

    #[test]
    fn headings_and_paragraphs() {
        let md = render_markdown("# Title\n\nSome **bold** text.");
        let t = text_of(&md);
        assert!(t.contains("Title"));
        assert!(t.contains("bold"));
        // H1 emits an underline rule row.
        assert!(t.contains("───"), "H1 underline missing:\n{t}");
        // Heading text itself is bold (not only the underline rule).
        let title_bold = md.lines.iter().any(|l| {
            l.spans.iter().any(|s| {
                s.content.as_ref().contains("Title")
                    && s.style.add_modifier.contains(Modifier::BOLD)
            })
        });
        assert!(title_bold, "heading span should be bold:\n{t}");
    }

    #[test]
    fn tables_emit_row_breaks_and_cell_separators() {
        let md = render_markdown("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |");
        let t = text_of(&md);
        assert!(t.contains('│'), "cell separator missing:\n{t}");
        // Rows must not collapse to a single run-on line.
        let data_rows = t
            .lines()
            .filter(|l| l.contains('│') && (l.contains('1') || l.contains('3')))
            .count();
        assert!(data_rows >= 2, "expected separate table rows, got:\n{t}");
        assert!(t.contains('1') && t.contains('2') && t.contains('3') && t.contains('4'));
    }

    #[test]
    fn bullet_and_ordered_lists() {
        let md = render_markdown("- one\n- two\n\n1. first\n2. second");
        let t = text_of(&md);
        assert!(t.contains("• one"), "{t}");
        assert!(t.contains("• two"), "{t}");
        assert!(t.contains("1. first"), "{t}");
        assert!(t.contains("2. second"), "{t}");
    }

    #[test]
    fn inline_code_and_fenced_code() {
        let md = render_markdown("Use `cargo test`.\n\n```rust\nfn main() {}\n```");
        let t = text_of(&md);
        assert!(t.contains("cargo test"));
        assert!(t.contains("fn main"));
        assert!(t.contains("rust"), "lang tag missing:\n{t}");
        assert!(
            t.contains("╭─") || t.contains("│"),
            "code snippet chrome missing:\n{t}"
        );
        assert!(t.contains("y copy"), "copy hint missing:\n{t}");
    }

    #[test]
    fn blockquote_prefix() {
        let md = render_markdown("> quoted line");
        let t = text_of(&md);
        assert!(t.contains("▎"), "quote prefix missing:\n{t}");
        assert!(t.contains("quoted line"));
    }

    #[test]
    fn links_are_collected() {
        let md = render_markdown("see [docs](https://example.com/x) here");
        assert_eq!(md.links.len(), 1);
        assert_eq!(md.links[0].url, "https://example.com/x");
    }

    #[test]
    fn no_panic_on_adversarial_widths_and_unicode() {
        let corpus = "# 日本語\n\n- 项目一\n- 🎉 emoji\n\n> 引用\n\n```py\nprint('日本語')\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let md = render_markdown(corpus);
        // Every line is a valid styled line; widths are finite.
        for l in &md.lines {
            let w: usize = l
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(w < 10_000);
        }
    }

    #[test]
    fn strong_span_is_bold() {
        let md = render_markdown("**hi**");
        let bold =
            md.lines.iter().flat_map(|l| &l.spans).any(|s| {
                s.content.as_ref() == "hi" && s.style.add_modifier.contains(Modifier::BOLD)
            });
        assert!(bold);
    }
}
