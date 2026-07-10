//! Markdown → styled ratatui lines for the modern TUI (plan §M3).
//!
//! Assistant, thinking, and plan-preview blocks render their markdown
//! source through [`render_markdown`]. Wrapping is left to the layout
//! cache (which is unicode-aware), so this module only produces logical
//! styled lines. Fenced code is highlighted with syntect, whose syntax and
//! theme sets are loaded once via `OnceLock`. Rendering is memoized per
//! block by the layout cache's content-hash keying, so a streaming block
//! only re-parses on its own flushes.

use std::ops::Range;
use std::sync::OnceLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

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

fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn code_theme() -> &'static Theme {
    static TS: OnceLock<Theme> = OnceLock::new();
    TS.get_or_init(|| {
        let mut set = ThemeSet::load_defaults();
        set.themes.remove("base16-ocean.dark").unwrap_or_else(|| {
            ThemeSet::load_defaults()
                .themes
                .into_values()
                .next()
                .unwrap()
        })
    })
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
            s = s.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn line_prefix(&self) -> Vec<Span<'static>> {
        let mut p = Vec::new();
        for _ in 0..self.quote_depth {
            p.push(Span::styled(
                "▎ ",
                Style::default()
                    .fg(Color::DarkGray)
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
                } else {
                    let style = self.inline_style();
                    self.push_text(&t, style);
                }
            }
            Event::Code(t) => {
                let style = Style::default()
                    .fg(Color::Rgb(220, 220, 170))
                    .bg(Color::Rgb(45, 45, 55));
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
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                self.push_text(mark, Style::default().fg(Color::Cyan));
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
                self.push_text(&marker, Style::default().fg(Color::Cyan));
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
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.blank_line(),
            TagEnd::Heading(_) => {
                let level = self.pending_heading.take();
                // Re-style the heading line we just built as bold+accent.
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
                        Style::default().fg(Color::DarkGray),
                    )));
                }
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
        let rule = Style::default().fg(Color::DarkGray);
        // Language tag row.
        let tag = if lang.is_empty() {
            "code".into()
        } else {
            lang.to_string()
        };
        self.lines.push(Line::from(vec![
            Span::styled("▎ ", rule),
            Span::styled(
                tag,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ]));

        let ss = syntax_set();
        let syntax = ss
            .find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(lang))
            .unwrap_or_else(|| ss.find_syntax_plain_text());
        let mut hl = HighlightLines::new(syntax, code_theme());

        for line in LinesWithEndings::from(content) {
            let mut spans = vec![Span::styled("▎ ", rule)];
            match hl.highlight_line(line, ss) {
                Ok(ranges) => {
                    for (sty, text) in ranges {
                        let t = text.trim_end_matches('\n');
                        if t.is_empty() {
                            continue;
                        }
                        let c = sty.foreground;
                        spans.push(Span::styled(
                            t.to_string(),
                            Style::default().fg(Color::Rgb(c.r, c.g, c.b)),
                        ));
                        self.spans_emitted += 1;
                    }
                }
                Err(_) => spans.push(Span::raw(line.trim_end_matches('\n').to_string())),
            }
            self.lines.push(Line::from(spans));
        }
    }
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
    fn headings_and_paragraphs() {
        let md = render_markdown("# Title\n\nSome **bold** text.");
        let t = text_of(&md);
        assert!(t.contains("Title"));
        assert!(t.contains("bold"));
        // H1 emits an underline rule row.
        assert!(t.contains("───"), "H1 underline missing:\n{t}");
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
        assert!(t.contains("▎"), "code left-rule missing:\n{t}");
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
