//! Inline syntax-highlighted diff rendering for edit tool cards.
//!
//! `FileEdit` / `MultiEdit` return a unified diff as their tool result (see
//! `agent_code_lib::tools::file_edit::unified_diff`). Instead of printing that
//! as a flat dim block, we parse the hunks and render:
//!   * `+` / `-` / context lines with a subtle per-line background tint,
//!   * syntect syntax highlighting of the code (by file extension),
//!   * old/new line numbers in a gutter and dim `@@` hunk headers,
//!   * word-level intra-line emphasis for 1:1 changed line pairs.
//!
//! This matches peer terminal agents' diff review (syntect + colored hunks)
//! and adds background tints, line numbers, and word-level highlighting on top.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxReference;

use super::colors::{Palette, palette};
use super::markdown::{code_theme, syntax_set};

/// Half-open `[start, end)` char ranges within a single line that changed.
type WordRanges = Vec<(usize, usize)>;

/// True when a tool result looks like a unified diff worth rendering richly.
/// `FileEdit`/`MultiEdit` emit `--- path` / `+++ path` headers followed by
/// `@@` hunks.
pub fn looks_like_unified_diff(result: &str) -> bool {
    (result.starts_with("--- ") && result.contains("\n+++ ") && result.contains("@@"))
        || result.contains("\n@@ ")
}

/// Parse the `@@ -a,b +c,d @@` hunk header into (old_start, new_start).
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    // Format: @@ -old_start[,old_len] +new_start[,new_len] @@ [ctx]
    let rest = line.strip_prefix("@@ ")?;
    let mut parts = rest.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_start = old.split(',').next()?.parse().ok()?;
    let new_start = new.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// Character ranges (start, end) that differ between `old` and `new`, computed
/// word-by-word. Returns (ranges-in-old, ranges-in-new).
fn word_change_ranges(old: &str, new: &str) -> (WordRanges, WordRanges) {
    let diff = TextDiff::from_words(old, new);
    let (mut old_r, mut new_r) = (Vec::new(), Vec::new());
    let (mut oc, mut nc) = (0usize, 0usize);
    for change in diff.iter_all_changes() {
        let len = change.value().chars().count();
        match change.tag() {
            ChangeTag::Equal => {
                oc += len;
                nc += len;
            }
            ChangeTag::Delete => {
                if len > 0 {
                    old_r.push((oc, oc + len));
                }
                oc += len;
            }
            ChangeTag::Insert => {
                if len > 0 {
                    new_r.push((nc, nc + len));
                }
                nc += len;
            }
        }
    }
    (old_r, new_r)
}

/// Is char index `i` inside any (start,end) range?
fn in_ranges(i: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|&(s, e)| i >= s && i < e)
}

/// Render one code line: gutter (line number), colored marker, then syntect-
/// highlighted content where each char's background is the word-highlight color
/// when it falls in `changed` (else the line tint).
#[allow(clippy::too_many_arguments)]
fn render_code_line(
    marker: char,
    marker_color: Color,
    lineno: Option<usize>,
    content: &str,
    syntax: &SyntaxReference,
    line_bg: Option<Color>,
    word_bg: Option<Color>,
    changed: &[(usize, usize)],
    gutter_w: usize,
) -> Line<'static> {
    let p = palette();
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
    // Gutter line number (dim).
    let num = match lineno {
        Some(n) => format!("{n:>gutter_w$} "),
        None => format!("{:>w$} ", "", w = gutter_w),
    };
    spans.push(Span::styled(
        num,
        Style::default().fg(p.muted).add_modifier(Modifier::DIM),
    ));
    // Marker column (+ / - / space).
    spans.push(Span::styled(
        format!("{marker} "),
        Style::default()
            .fg(marker_color)
            .add_modifier(Modifier::BOLD),
    ));

    // Syntax-highlight the content, then re-split each highlighted run at
    // word-change boundaries so changed chars get the brighter word tint.
    let mut hl = HighlightLines::new(syntax, code_theme());
    let mut char_idx = 0usize;
    let ranges = hl.highlight_line(content, syntax_set()).unwrap_or_default();
    for (sty, text) in ranges {
        let fg = super::colors::syntax_color(sty.foreground.r, sty.foreground.g, sty.foreground.b);
        // Split this run char-by-char, coalescing adjacent chars with the
        // same background so we don't emit one span per character.
        let mut buf = String::new();
        let mut buf_bg: Option<Color> = None;
        let mut first = true;
        for ch in text.chars() {
            let bg = if word_bg.is_some() && in_ranges(char_idx, changed) {
                word_bg
            } else {
                line_bg
            };
            if first {
                buf_bg = bg;
                first = false;
            }
            if bg != buf_bg {
                spans.push(styled_span(&buf, fg, buf_bg));
                buf.clear();
                buf_bg = bg;
            }
            buf.push(ch);
            char_idx += 1;
        }
        if !buf.is_empty() {
            spans.push(styled_span(&buf, fg, buf_bg));
        }
    }
    if content.is_empty() {
        // Keep an empty tinted line visible.
        spans.push(styled_span(" ", p.text, line_bg));
    }
    Line::from(spans)
}

fn styled_span(text: &str, fg: Color, bg: Option<Color>) -> Span<'static> {
    let mut style = Style::default().fg(fg);
    if let Some(b) = bg {
        style = style.bg(b);
    }
    Span::styled(text.to_string(), style)
}

/// Flush a buffered run of removed then added lines. When the run is a 1:1
/// replacement (equal, non-empty counts) each pair gets word-level emphasis;
/// otherwise lines render with line-level tint only.
#[allow(clippy::too_many_arguments)]
fn flush_run(
    out: &mut Vec<Line<'static>>,
    rem: &[String],
    add: &[String],
    old_ln: usize,
    new_ln: usize,
    syntax: &SyntaxReference,
    p: &Palette,
    gutter_w: usize,
) {
    let paired = !rem.is_empty() && rem.len() == add.len();
    for (i, line) in rem.iter().enumerate() {
        let changed = if paired {
            word_change_ranges(line, &add[i]).0
        } else {
            Vec::new()
        };
        out.push(render_code_line(
            '-',
            p.diff_remove,
            Some(old_ln + i),
            line,
            syntax,
            Some(p.diff_remove_dim),
            paired.then_some(p.diff_remove_word),
            &changed,
            gutter_w,
        ));
    }
    for (i, line) in add.iter().enumerate() {
        let changed = if paired {
            word_change_ranges(&rem[i], line).1
        } else {
            Vec::new()
        };
        out.push(render_code_line(
            '+',
            p.diff_add,
            Some(new_ln + i),
            line,
            syntax,
            Some(p.diff_add_dim),
            paired.then_some(p.diff_add_word),
            &changed,
            gutter_w,
        ));
    }
}

/// Skip syntect + word-diff when a unified diff is this large or larger.
/// The display cap is applied *after* highlighting; without this, a
/// multi-megabyte edit is fully highlighted on every layout cache miss.
const MAX_DIFF_BYTES: usize = 256 * 1024;
/// Line count above which we fall back to plain tinted lines.
const MAX_DIFF_LINES: usize = 2_000;
/// Single-line char count above which we avoid word-diff / highlight.
const MAX_LINE_CHARS: usize = 512;

/// True when rich highlighting would cost more than it is worth.
fn too_large_for_rich_diff(diff: &str) -> bool {
    if diff.len() > MAX_DIFF_BYTES {
        return true;
    }
    let mut lines = 0usize;
    for line in diff.lines() {
        lines += 1;
        if lines > MAX_DIFF_LINES {
            return true;
        }
        if line.chars().count() > MAX_LINE_CHARS {
            return true;
        }
    }
    false
}

/// Plain +/- tinted lines — no syntect, no word-diff. Used when
/// [`too_large_for_rich_diff`] trips so the layout path stays O(display).
fn render_plain_unified_diff(diff: &str, expanded: bool, cap: usize) -> Vec<Line<'static>> {
    let p = palette();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut seen_hunk = false;
    for raw in diff.lines() {
        if raw.starts_with("--- ") || raw.starts_with("+++ ") {
            continue;
        }
        if !seen_hunk && !raw.starts_with("@@") {
            continue;
        }
        if raw.starts_with("@@") {
            seen_hunk = true;
            out.push(Line::from(Span::styled(
                format!("   {raw}"),
                Style::default()
                    .fg(p.accent)
                    .add_modifier(Modifier::DIM | Modifier::ITALIC),
            )));
            continue;
        }
        let (marker, fg, content) = match raw.chars().next() {
            Some('-') => ('-', p.diff_remove, &raw[1..]),
            Some('+') => ('+', p.diff_add, &raw[1..]),
            _ => (' ', p.muted, raw.strip_prefix(' ').unwrap_or(raw)),
        };
        out.push(Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(fg)),
            Span::styled(content.to_string(), Style::default().fg(fg)),
        ]));
    }
    if !expanded && out.len() > cap {
        let hidden = out.len() - cap;
        out.truncate(cap);
        out.push(Line::from(Span::styled(
            format!("   … +{hidden} more diff lines · e expand  (plain: large diff)"),
            Style::default()
                .fg(p.muted)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        )));
    }
    out
}

/// Render a unified-diff string as rich diff `Line`s. `file_path` drives syntax
/// detection. When `!expanded` and the diff exceeds `cap` lines, it is
/// truncated with a "… +N more" hint.
///
/// Large diffs skip syntect and word-level highlighting and use plain
/// tinted lines so layout never pays quadratic highlight cost on a
/// multi-megabyte edit result.
pub fn render_unified_diff(
    diff: &str,
    file_path: &str,
    expanded: bool,
    cap: usize,
) -> Vec<Line<'static>> {
    if too_large_for_rich_diff(diff) {
        return render_plain_unified_diff(diff, expanded, cap);
    }

    let p = palette();
    let ss = syntax_set();
    let syntax = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| ss.find_syntax_by_extension(ext))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    // Gutter width from the highest line number the diff mentions.
    let gutter_w = diff
        .lines()
        .filter_map(parse_hunk_header)
        .flat_map(|(o, n)| [o, n])
        .max()
        .map(|m| (m + diff.lines().count()).to_string().len().max(2))
        .unwrap_or(3);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut old_ln = 0usize;
    let mut new_ln = 0usize;
    // Buffered change run (removed then added) awaiting flush.
    let mut rem: Vec<String> = Vec::new();
    let mut add: Vec<String> = Vec::new();
    let mut rem_start = 0usize;
    let mut add_start = 0usize;

    macro_rules! flush {
        () => {
            if !rem.is_empty() || !add.is_empty() {
                flush_run(
                    &mut out, &rem, &add, rem_start, add_start, syntax, &p, gutter_w,
                );
                rem.clear();
                add.clear();
            }
        };
    }

    // Skip any preamble the tool prepends before the actual diff (e.g.
    // "Replaced N occurrence(s) in <path>") — the card header already conveys
    // it. We start emitting only once the first hunk header is seen.
    let mut seen_hunk = false;
    for raw in diff.lines() {
        if raw.starts_with("--- ") || raw.starts_with("+++ ") {
            continue; // path is already shown in the card header
        }
        if !seen_hunk && !raw.starts_with("@@") {
            continue; // preamble before the first hunk
        }
        if raw.starts_with("@@") {
            seen_hunk = true;
            flush!();
            if let Some((o, n)) = parse_hunk_header(raw) {
                old_ln = o;
                new_ln = n;
            }
            out.push(Line::from(Span::styled(
                format!("   {raw}"),
                Style::default()
                    .fg(p.accent)
                    .add_modifier(Modifier::DIM | Modifier::ITALIC),
            )));
            continue;
        }
        match raw.chars().next() {
            Some('-') => {
                if rem.is_empty() {
                    rem_start = old_ln;
                }
                rem.push(raw[1..].to_string());
                old_ln += 1;
            }
            Some('+') => {
                if add.is_empty() {
                    add_start = new_ln;
                }
                add.push(raw[1..].to_string());
                new_ln += 1;
            }
            _ => {
                flush!();
                let content = raw.strip_prefix(' ').unwrap_or(raw);
                out.push(render_code_line(
                    ' ',
                    p.muted,
                    Some(new_ln),
                    content,
                    syntax,
                    None,
                    None,
                    &[],
                    gutter_w,
                ));
                old_ln += 1;
                new_ln += 1;
            }
        }
    }
    flush!();

    // Collapse when not expanded.
    if !expanded && out.len() > cap {
        let hidden = out.len() - cap;
        out.truncate(cap);
        out.push(Line::from(Span::styled(
            format!("   … +{hidden} more diff lines · e expand"),
            Style::default()
                .fg(p.muted)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        )));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    let x = 1;\n+    let x = 2;\n }\n";

    #[test]
    fn detects_unified_diff() {
        assert!(looks_like_unified_diff(SAMPLE));
        assert!(!looks_like_unified_diff("just some text\nno diff here"));
        assert!(!looks_like_unified_diff("Wrote 3 lines to foo.txt"));
    }

    #[test]
    fn parses_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -1,3 +1,3 @@"), Some((1, 1)));
        assert_eq!(
            parse_hunk_header("@@ -10,5 +12,7 @@ fn foo"),
            Some((10, 12))
        );
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(parse_hunk_header("not a hunk"), None);
    }

    #[test]
    fn word_ranges_isolate_the_change() {
        let (old_r, new_r) = word_change_ranges("    let x = 1;", "    let x = 2;");
        // Only the "1" vs "2" token differs.
        assert!(!old_r.is_empty(), "expected a removed-word range");
        assert!(!new_r.is_empty(), "expected an added-word range");
        // The changed char is near the end, not at index 0.
        assert!(old_r.iter().all(|&(s, _)| s > 0));
    }

    #[test]
    fn renders_hunk_context_and_changes() {
        let lines = render_unified_diff(SAMPLE, "src/lib.rs", true, 100);
        // 1 hunk header + 1 context + 1 removed + 1 added + 1 trailing context.
        assert_eq!(lines.len(), 5, "expected 5 rendered diff lines");
    }

    #[test]
    fn collapse_truncates_and_hints() {
        let big: String = std::iter::once("@@ -1,20 +1,20 @@".to_string())
            .chain((0..20).map(|i| format!("+line {i}")))
            .collect::<Vec<_>>()
            .join("\n");
        let collapsed = render_unified_diff(&big, "x.txt", false, 6);
        assert_eq!(collapsed.len(), 7); // 6 + hint
        let expanded = render_unified_diff(&big, "x.txt", true, 6);
        assert_eq!(expanded.len(), 21); // header + 20 lines, no hint
    }

    #[test]
    fn oversized_diff_uses_plain_path() {
        // A single huge line trips the per-line guard without allocating
        // multi-megabyte fixtures.
        let huge_line = format!("+{}", "x".repeat(MAX_LINE_CHARS + 1));
        let diff = format!("@@ -1,1 +1,1 @@\n{huge_line}\n");
        assert!(too_large_for_rich_diff(&diff));
        let lines = render_unified_diff(&diff, "big.rs", true, 100);
        assert!(!lines.is_empty(), "plain path must still produce output");
        // Marker is present without requiring syntect.
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with('+') || text.contains("@@"), "{text}");
    }

    #[test]
    fn modest_diff_is_not_considered_oversized() {
        assert!(!too_large_for_rich_diff(SAMPLE));
    }

    #[test]
    fn header_lines_are_skipped() {
        let lines = render_unified_diff(SAMPLE, "src/lib.rs", true, 100);
        // No rendered line should contain the +++/--- path headers.
        for l in &lines {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(!text.contains("+++ "), "path header leaked into output");
        }
    }

    #[test]
    fn skips_preamble_before_hunk() {
        // FileEdit prefixes the diff with "Replaced N occurrence(s) in <path>".
        let with_preamble = format!("Replaced 1 occurrence(s) in src/lib.rs\n\n{SAMPLE}");
        let lines = render_unified_diff(&with_preamble, "src/lib.rs", true, 100);
        assert_eq!(lines.len(), 5, "preamble should not add rendered lines");
        for l in &lines {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                !text.contains("Replaced"),
                "preamble leaked into diff output"
            );
        }
    }
}
