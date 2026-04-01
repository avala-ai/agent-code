//! Ratatui-based TUI rendering.
//!
//! Provides rich terminal UI widgets for tool execution, status bars,
//! and formatted output. Operates inline (no alternate screen) to
//! coexist with rustyline for input.
//!
//! Uses ratatui as a rendering library — draws widgets on demand
//! when StreamSink events fire, not in a continuous render loop.

use std::io::{self, Write};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Convert our theme Color to ratatui Color.
pub fn theme_to_ratatui(color: crossterm::style::Color) -> Color {
    match color {
        crossterm::style::Color::Rgb { r, g, b } => Color::Rgb(r, g, b),
        crossterm::style::Color::Black => Color::Black,
        crossterm::style::Color::Red => Color::Red,
        crossterm::style::Color::Green => Color::Green,
        crossterm::style::Color::Yellow => Color::Yellow,
        crossterm::style::Color::Blue => Color::Blue,
        crossterm::style::Color::Magenta => Color::Magenta,
        crossterm::style::Color::Cyan => Color::Cyan,
        crossterm::style::Color::White => Color::White,
        crossterm::style::Color::DarkGrey => Color::DarkGray,
        crossterm::style::Color::Grey => Color::Gray,
        crossterm::style::Color::DarkCyan => Color::Cyan,
        crossterm::style::Color::DarkGreen => Color::Green,
        crossterm::style::Color::DarkYellow => Color::Yellow,
        crossterm::style::Color::DarkMagenta => Color::Magenta,
        crossterm::style::Color::DarkRed => Color::Red,
        crossterm::style::Color::DarkBlue => Color::Blue,
        _ => Color::Reset,
    }
}

/// Render a tool execution block inline (no alternate screen).
/// Draws a bordered box with tool name and result summary.
pub fn render_tool_block(tool_name: &str, detail: &str, result: Option<&str>, is_error: bool) {
    let t = super::theme::current();
    let accent = theme_to_ratatui(t.tool);
    let muted = theme_to_ratatui(t.muted);
    let error_color = theme_to_ratatui(t.error);
    let success_color = theme_to_ratatui(t.success);

    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .min(100);

    // Build lines for the block.
    let mut lines = Vec::new();

    // Tool header.
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {tool_name} "),
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            truncate(detail, width.saturating_sub(tool_name.len() + 6)),
            Style::default().fg(muted),
        ),
    ]));

    // Result line.
    if let Some(result_text) = result {
        let (icon, color) = if is_error {
            ("✗", error_color)
        } else {
            ("✓", success_color)
        };
        let preview = truncate(result_text.lines().next().unwrap_or(""), width - 6);
        let line_count = result_text.lines().count();
        let mut spans = vec![
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::styled(preview, Style::default().fg(muted)),
        ];
        if line_count > 1 {
            spans.push(Span::styled(
                format!(" (+{} lines)", line_count - 1),
                Style::default().fg(muted),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Render to stdout using raw ANSI (no alternate screen needed).
    let buf = render_lines_to_ansi(&lines, width);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

/// Render a status bar showing session info.
pub fn render_status_bar(model: &str, turn: usize, tokens: u64, cost: f64) {
    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);
    let muted = theme_to_ratatui(t.muted);

    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .min(100);

    let left = format!(" {model} ");
    let right = format!(" turn {} | {} tokens | ${:.4} ", turn, tokens, cost);
    let padding = width.saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(
            left,
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding), Style::default().bg(Color::Reset)),
        Span::styled(right, Style::default().fg(muted)),
    ]);

    let buf = render_lines_to_ansi(&[line], width);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

/// Render a list of recently executed tools.
pub fn render_tool_list(tools: &[(String, bool)]) {
    let t = super::theme::current();
    let success_color = theme_to_ratatui(t.success);
    let error_color = theme_to_ratatui(t.error);
    let muted = theme_to_ratatui(t.muted);

    for (name, is_error) in tools {
        let (icon, color) = if *is_error {
            ("✗", error_color)
        } else {
            ("✓", success_color)
        };
        eprintln!(
            "  \x1b[{}m{icon}\x1b[0m \x1b[{}m{name}\x1b[0m",
            color_to_ansi_code(color),
            color_to_ansi_code(muted),
        );
    }
}

/// Render a thinking block with content preview.
pub fn render_thinking_block(text: &str) {
    let t = super::theme::current();
    let muted = theme_to_ratatui(t.muted);

    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .min(100)
        .saturating_sub(4);

    let preview = if text.len() <= width {
        text.trim().to_string()
    } else {
        let p: String = text.chars().take(width - 15).collect();
        format!("{p}... ({} chars)", text.len())
    };

    let line = Line::from(vec![
        Span::styled("  💭 ", Style::default().fg(muted)),
        Span::styled(
            preview,
            Style::default().fg(muted).add_modifier(Modifier::ITALIC),
        ),
    ]);

    let buf = render_lines_to_ansi(&[line], width + 4);
    eprint!("\r{buf}\r");
    let _ = io::stderr().flush();
}

// ---- Internal helpers ----

/// Render ratatui Lines to ANSI escape string (inline, no alternate screen).
fn render_lines_to_ansi(lines: &[Line<'_>], _width: usize) -> String {
    let mut buf = String::new();
    for line in lines {
        for span in &line.spans {
            // Apply style.
            let mut codes = Vec::new();
            if let Color::Rgb(r, g, b) = span.style.fg.unwrap_or(Color::Reset) {
                codes.push(format!("38;2;{r};{g};{b}"));
            } else if let Some(fg) = span.style.fg {
                codes.push(color_to_ansi_code(fg).to_string());
            }
            if let Color::Rgb(r, g, b) = span.style.bg.unwrap_or(Color::Reset) {
                codes.push(format!("48;2;{r};{g};{b}"));
            } else if let Some(bg) = span.style.bg {
                codes.push(format!("{}", color_to_ansi_code(bg) + 10));
            }
            if span.style.add_modifier.contains(Modifier::BOLD) {
                codes.push("1".to_string());
            }
            if span.style.add_modifier.contains(Modifier::ITALIC) {
                codes.push("3".to_string());
            }

            if !codes.is_empty() {
                buf.push_str(&format!("\x1b[{}m", codes.join(";")));
            }
            buf.push_str(&span.content);
            if !codes.is_empty() {
                buf.push_str("\x1b[0m");
            }
        }
        buf.push_str("\r\n");
    }
    buf
}

fn color_to_ansi_code(color: Color) -> u8 {
    match color {
        Color::Black => 30,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Blue => 34,
        Color::Magenta => 35,
        Color::Cyan => 36,
        Color::White => 37,
        Color::Gray => 37,
        Color::DarkGray => 90,
        _ => 39, // default
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 3 {
        format!("{}...", &s[..max - 3])
    } else {
        s[..max].to_string()
    }
}
