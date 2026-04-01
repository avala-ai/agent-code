//! Terminal rendering for markdown and code.
//!
//! Converts markdown text to styled terminal output with syntax
//! highlighting for fenced code blocks. Uses pulldown-cmark for markdown
//! parsing and syntect for code coloring.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

/// Render markdown text to the terminal with syntax highlighting.
///
/// Uses pulldown-cmark to parse markdown structure, routes fenced code
/// blocks to syntect highlighting, and tables to box-drawing rendering.
pub fn render_markdown(text: &str) -> String {
    let t = super::theme::current();
    let accent = color_to_ansi(t.accent);
    let tool = color_to_ansi(t.tool);
    let muted = color_to_ansi(t.muted);
    let reset = "\x1b[0m";

    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, opts);

    let mut output = String::new();

    // State tracking.
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
    let mut in_heading = false;
    let mut heading_text = String::new();
    let mut bold_depth: u32 = 0;
    let mut italic_depth: u32 = 0;
    let mut in_link = false;
    let mut link_url = String::new();
    let mut link_text = String::new();
    let mut list_stack: Vec<Option<u64>> = Vec::new(); // None = unordered, Some(n) = ordered
    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut _in_table_head = false;
    let mut paragraph_text = String::new();
    let mut in_paragraph = false;

    for event in parser {
        match event {
            // --- Paragraphs ---
            Event::Start(Tag::Paragraph) => {
                in_paragraph = true;
                paragraph_text.clear();
            }
            Event::End(TagEnd::Paragraph) => {
                in_paragraph = false;
                output.push_str(&paragraph_text);
                output.push('\n');
                paragraph_text.clear();
            }

            // --- Headings ---
            Event::Start(Tag::Heading { .. }) => {
                in_heading = true;
                heading_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                output.push_str(&format!("\x1b[1;4m{heading_text}{reset}\n"));
                heading_text.clear();
            }

            // --- Code blocks ---
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_buffer.clear();
                code_lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                output.push_str(&highlight_code(&code_buffer, &code_lang));
                output.push('\n');
                code_buffer.clear();
                code_lang.clear();
            }

            // --- Emphasis / strong ---
            Event::Start(Tag::Emphasis) => {
                italic_depth += 1;
                push_to_active(
                    &mut output,
                    &mut heading_text,
                    &mut paragraph_text,
                    &mut link_text,
                    &mut current_cell,
                    in_heading,
                    in_paragraph,
                    in_link,
                    in_table,
                    "\x1b[3m",
                );
            }
            Event::End(TagEnd::Emphasis) => {
                italic_depth = italic_depth.saturating_sub(1);
                push_to_active(
                    &mut output,
                    &mut heading_text,
                    &mut paragraph_text,
                    &mut link_text,
                    &mut current_cell,
                    in_heading,
                    in_paragraph,
                    in_link,
                    in_table,
                    reset,
                );
                // Restore bold if still active.
                if bold_depth > 0 {
                    push_to_active(
                        &mut output,
                        &mut heading_text,
                        &mut paragraph_text,
                        &mut link_text,
                        &mut current_cell,
                        in_heading,
                        in_paragraph,
                        in_link,
                        in_table,
                        "\x1b[1m",
                    );
                }
            }
            Event::Start(Tag::Strong) => {
                bold_depth += 1;
                push_to_active(
                    &mut output,
                    &mut heading_text,
                    &mut paragraph_text,
                    &mut link_text,
                    &mut current_cell,
                    in_heading,
                    in_paragraph,
                    in_link,
                    in_table,
                    "\x1b[1m",
                );
            }
            Event::End(TagEnd::Strong) => {
                bold_depth = bold_depth.saturating_sub(1);
                push_to_active(
                    &mut output,
                    &mut heading_text,
                    &mut paragraph_text,
                    &mut link_text,
                    &mut current_cell,
                    in_heading,
                    in_paragraph,
                    in_link,
                    in_table,
                    reset,
                );
                // Restore italic if still active.
                if italic_depth > 0 {
                    push_to_active(
                        &mut output,
                        &mut heading_text,
                        &mut paragraph_text,
                        &mut link_text,
                        &mut current_cell,
                        in_heading,
                        in_paragraph,
                        in_link,
                        in_table,
                        "\x1b[3m",
                    );
                }
            }

            // --- Inline code ---
            Event::Code(code) => {
                let formatted = format!("{accent}{code}{reset}");
                if in_heading {
                    heading_text.push_str(&formatted);
                } else if in_link {
                    link_text.push_str(&formatted);
                } else if in_table {
                    current_cell.push_str(&code);
                } else if in_paragraph {
                    paragraph_text.push_str(&formatted);
                } else {
                    output.push_str(&formatted);
                }
            }

            // --- Links ---
            Event::Start(Tag::Link { dest_url, .. }) => {
                in_link = true;
                link_url = dest_url.to_string();
                link_text.clear();
            }
            Event::End(TagEnd::Link) => {
                in_link = false;
                let formatted = if link_url.is_empty() {
                    link_text.clone()
                } else {
                    format!("{link_text} ({muted}{link_url}{reset})")
                };
                if in_paragraph {
                    paragraph_text.push_str(&formatted);
                } else {
                    output.push_str(&formatted);
                }
                link_text.clear();
                link_url.clear();
            }

            // --- Lists ---
            Event::Start(Tag::List(first_item)) => {
                list_stack.push(first_item);
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                if let Some(ordered_start) = list_stack.last().copied().flatten() {
                    // Ordered list — figure out the current number.
                    // pulldown-cmark gives us the start, we track from there.
                    output.push_str(&format!("{indent}{tool}{ordered_start}.{reset} "));
                    // Increment for next sibling.
                    if let Some(entry) = list_stack.last_mut() {
                        *entry = Some(ordered_start + 1);
                    }
                } else {
                    output.push_str(&format!("{indent}  {tool}•{reset} "));
                }
            }
            Event::End(TagEnd::Item) => {
                // Ensure line break after list item.
                if !output.ends_with('\n') {
                    output.push('\n');
                }
            }

            // --- Tables ---
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                // Convert table_rows to the format render_table expects.
                let lines: Vec<String> = table_rows
                    .iter()
                    .enumerate()
                    .flat_map(|(i, row)| {
                        let data_line = format!("| {} |", row.join(" | "));
                        if i == 0 && table_rows.len() > 1 {
                            // Insert separator after header.
                            let sep = format!(
                                "| {} |",
                                row.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
                            );
                            vec![data_line, sep]
                        } else {
                            vec![data_line]
                        }
                    })
                    .collect();
                if !lines.is_empty() {
                    output.push_str(&render_table(&lines));
                }
                table_rows.clear();
            }
            Event::Start(Tag::TableHead) => {
                _in_table_head = true;
                current_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                _in_table_head = false;
                table_rows.push(current_row.clone());
                current_row.clear();
            }
            Event::Start(Tag::TableRow) => {
                current_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                table_rows.push(current_row.clone());
                current_row.clear();
            }
            Event::Start(Tag::TableCell) => {
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                current_row.push(current_cell.clone());
                current_cell.clear();
            }

            // --- Text ---
            Event::Text(txt) => {
                if in_code_block {
                    code_buffer.push_str(&txt);
                } else if in_heading {
                    heading_text.push_str(&txt);
                } else if in_link {
                    link_text.push_str(&txt);
                } else if in_table {
                    current_cell.push_str(&txt);
                } else if in_paragraph {
                    paragraph_text.push_str(&txt);
                } else {
                    output.push_str(&txt);
                }
            }

            // --- Soft/hard breaks ---
            Event::SoftBreak => {
                if in_heading {
                    heading_text.push(' ');
                } else if in_paragraph {
                    paragraph_text.push(' ');
                } else {
                    output.push(' ');
                }
            }
            Event::HardBreak => {
                if in_paragraph {
                    paragraph_text.push('\n');
                } else {
                    output.push('\n');
                }
            }

            // --- Horizontal rule ---
            Event::Rule => {
                output.push_str(&format!("{muted}────────────────────{reset}\n"));
            }

            _ => {}
        }
    }

    output
}

/// Push text to whichever buffer is currently active.
#[allow(clippy::too_many_arguments)]
fn push_to_active(
    output: &mut String,
    heading: &mut String,
    paragraph: &mut String,
    link: &mut String,
    cell: &mut String,
    in_heading: bool,
    in_paragraph: bool,
    in_link: bool,
    in_table: bool,
    text: &str,
) {
    if in_heading {
        heading.push_str(text);
    } else if in_link {
        link.push_str(text);
    } else if in_table {
        cell.push_str(text);
    } else if in_paragraph {
        paragraph.push_str(text);
    } else {
        output.push_str(text);
    }
}

/// Syntax-highlight a code block using syntect.
fn highlight_code(code: &str, lang: &str) -> String {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    // Find syntax by language name or extension.
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = String::new();

    for line in LinesWithEndings::from(code) {
        match highlighter.highlight_line(line, &ss) {
            Ok(ranges) => {
                let escaped = as_24_bit_terminal_escaped(&ranges, false);
                result.push_str(&escaped);
            }
            Err(_) => {
                result.push_str(line);
            }
        }
    }

    // Reset terminal colors.
    result.push_str("\x1b[0m");
    result
}

/// Render inline markdown elements (bold, italic, code spans, links).
/// Uses the active theme for colors instead of hardcoded ANSI codes.
/// Kept for any callers that need single-line inline rendering.
fn render_inline(line: &str) -> String {
    // Delegate to the pulldown-cmark parser for consistency.
    let result = render_markdown(line);
    // Strip trailing newline that render_markdown adds.
    result.trim_end_matches('\n').to_string()
}

/// Convert a crossterm Color to an ANSI escape sequence prefix.
fn color_to_ansi(color: crossterm::style::Color) -> String {
    match color {
        crossterm::style::Color::Rgb { r, g, b } => format!("\x1b[38;2;{r};{g};{b}m"),
        crossterm::style::Color::DarkCyan => "\x1b[36m".to_string(),
        crossterm::style::Color::Cyan => "\x1b[96m".to_string(),
        crossterm::style::Color::Red => "\x1b[31m".to_string(),
        crossterm::style::Color::Green => "\x1b[32m".to_string(),
        crossterm::style::Color::Yellow => "\x1b[33m".to_string(),
        crossterm::style::Color::Blue => "\x1b[34m".to_string(),
        crossterm::style::Color::Magenta => "\x1b[35m".to_string(),
        crossterm::style::Color::Grey => "\x1b[37m".to_string(),
        crossterm::style::Color::DarkGrey => "\x1b[90m".to_string(),
        crossterm::style::Color::White => "\x1b[97m".to_string(),
        crossterm::style::Color::Black => "\x1b[30m".to_string(),
        _ => "\x1b[36m".to_string(), // fallback to cyan
    }
}

/// Detect whether a line looks like a markdown table row (`| col | col |`).
fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() > 1
}

/// Returns true if this table row is a separator line like `|---|---|`.
fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed
        .trim_start_matches('|')
        .trim_end_matches('|')
        .chars()
        .all(|c| c == '-' || c == ':' || c == '|' || c == ' ')
}

/// Parse a table row into cells (splitting on `|` and trimming).
fn parse_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Strip leading and trailing pipes, then split on `|`.
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

/// Render accumulated markdown table lines using box-drawing characters.
fn render_table(lines: &[String]) -> String {
    // Parse all rows, skipping separator rows.
    let rows: Vec<Vec<String>> = lines
        .iter()
        .filter(|l| !is_separator_row(l))
        .map(|l| parse_table_row(l))
        .collect();

    if rows.is_empty() {
        return String::new();
    }

    // Determine the number of columns and max width per column.
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths = vec![0usize; num_cols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }
    }
    // Ensure at least width 1 per column.
    for w in &mut col_widths {
        if *w == 0 {
            *w = 1;
        }
    }

    let mut out = String::new();

    // Top border: ┌───┬───┐
    out.push('┌');
    for (i, &w) in col_widths.iter().enumerate() {
        for _ in 0..w + 2 {
            out.push('─');
        }
        out.push(if i + 1 < num_cols { '┬' } else { '┐' });
    }
    out.push('\n');

    for (row_idx, row) in rows.iter().enumerate() {
        // Data row: │ val │ val │
        out.push('│');
        for (i, w) in col_widths.iter().enumerate() {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            out.push(' ');
            out.push_str(cell);
            for _ in 0..*w - cell.len() {
                out.push(' ');
            }
            out.push_str(" │");
        }
        out.push('\n');

        // After the header row (row 0), draw a separator: ├───┼───┤
        if row_idx == 0 && rows.len() > 1 {
            out.push('├');
            for (i, &w) in col_widths.iter().enumerate() {
                for _ in 0..w + 2 {
                    out.push('─');
                }
                out.push(if i + 1 < num_cols { '┼' } else { '┤' });
            }
            out.push('\n');
        }
    }

    // Bottom border: └───┴───┘
    out.push('└');
    for (i, &w) in col_widths.iter().enumerate() {
        for _ in 0..w + 2 {
            out.push('─');
        }
        out.push(if i + 1 < num_cols { '┴' } else { '┘' });
    }
    out.push('\n');

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_heading() {
        let result = render_inline("# Hello World");
        assert!(result.contains("Hello World"));
        assert!(result.contains("\x1b[1;4m")); // bold + underline
    }

    #[test]
    fn test_render_list_item() {
        let result = render_markdown("- item one");
        assert!(result.contains("•"));
        assert!(result.contains("item one"));
    }

    #[test]
    fn test_highlight_code_doesnt_panic() {
        let result = highlight_code("fn main() {}\n", "rust");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_is_table_line() {
        assert!(is_table_line("| a | b |"));
        assert!(is_table_line("| --- | --- |"));
        assert!(!is_table_line("not a table"));
        assert!(!is_table_line("|"));
        assert!(!is_table_line("| no closing pipe"));
    }

    #[test]
    fn test_render_table_basic() {
        let lines = vec![
            "| Name | Age |".to_string(),
            "| --- | --- |".to_string(),
            "| Alice | 30 |".to_string(),
            "| Bob | 25 |".to_string(),
        ];
        let result = render_table(&lines);
        assert!(result.contains('┌'));
        assert!(result.contains('┘'));
        assert!(result.contains("Alice"));
        assert!(result.contains("Bob"));
        // Header separator present.
        assert!(result.contains('┼'));
    }

    #[test]
    fn test_render_markdown_with_table() {
        let md = "# Title\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nDone.";
        let result = render_markdown(md);
        assert!(result.contains("Title"));
        assert!(result.contains('┌'));
        assert!(result.contains("Done."));
    }
}
