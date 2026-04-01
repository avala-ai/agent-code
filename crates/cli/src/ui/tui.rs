//! Ratatui-based TUI rendering.
//!
//! Provides rich terminal UI for tool execution, status display,
//! and formatted output. Operates inline (no alternate screen) to
//! coexist with rustyline for input.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---- Theme bridge ----

/// Convert crossterm Color to ratatui Color.
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

// ---- Turn state tracking ----

/// Tracks tool executions during a single agent turn for summary display.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub name: String,
    pub detail: String,
    pub result_preview: Option<String>,
    pub is_error: bool,
    pub line_count: usize,
}

/// Accumulated state for the current turn's TUI display.
#[derive(Debug, Default, Clone)]
pub struct TurnState {
    pub tools: Vec<ToolEntry>,
    pub thinking_chars: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TurnState {
    pub fn clear(&mut self) {
        self.tools.clear();
        self.thinking_chars = 0;
        self.tokens_in = 0;
        self.tokens_out = 0;
        self.cache_read = 0;
        self.cache_write = 0;
    }

    pub fn add_tool_start(&mut self, name: &str, detail: &str) {
        self.tools.push(ToolEntry {
            name: name.to_string(),
            detail: detail.to_string(),
            result_preview: None,
            is_error: false,
            line_count: 0,
        });
    }

    pub fn complete_last_tool(&mut self, result: &str, is_error: bool) {
        if let Some(tool) = self.tools.last_mut() {
            let preview = result.lines().next().unwrap_or("(ok)");
            tool.result_preview = Some(truncate(preview, 80));
            tool.is_error = is_error;
            tool.line_count = result.lines().count();
        }
    }
}

/// Shared turn state for the TUI sink.
pub type SharedTurnState = Arc<Mutex<TurnState>>;

pub fn new_turn_state() -> SharedTurnState {
    Arc::new(Mutex::new(TurnState::default()))
}

// ---- Rendering functions ----

/// Render a tool execution header inline.
pub fn render_tool_block(tool_name: &str, detail: &str, _result: Option<&str>, _is_error: bool) {
    let t = super::theme::current();
    let accent = theme_to_ratatui(t.tool);
    let muted = theme_to_ratatui(t.muted);

    let width = term_width();

    let lines = vec![Line::from(vec![
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
    ])];

    let buf = render_lines_to_ansi(&lines);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

/// Render the thinking indicator.
pub fn render_thinking_block(text: &str) {
    let t = super::theme::current();
    let muted = theme_to_ratatui(t.muted);
    let width = term_width().saturating_sub(8);

    let preview = if text.len() <= width {
        text.trim().to_string()
    } else {
        let p: String = text.chars().take(width.saturating_sub(20)).collect();
        format!("{p}... ({} chars)", text.len())
    };

    let line = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            preview,
            Style::default().fg(muted).add_modifier(Modifier::ITALIC),
        ),
    ]);

    let buf = render_lines_to_ansi(&[line]);
    eprint!("\r{buf}\r");
    let _ = io::stderr().flush();
}

/// Render a turn summary panel showing all tool executions.
pub fn render_turn_summary(state: &TurnState, turn: usize) {
    if state.tools.is_empty() {
        return;
    }

    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);
    let muted = theme_to_ratatui(t.muted);
    let success = theme_to_ratatui(t.success);
    let error = theme_to_ratatui(t.error);

    let width = term_width();
    let mut lines = Vec::new();

    // Top border.
    let border = "─".repeat(width.saturating_sub(4));
    lines.push(Line::from(vec![
        Span::styled("  ╭", Style::default().fg(muted)),
        Span::styled(border.clone(), Style::default().fg(muted)),
    ]));

    // Header.
    let tool_count = state.tools.len();
    let pass_count = state.tools.iter().filter(|t| !t.is_error).count();
    let fail_count = tool_count - pass_count;
    let header = format!(
        " turn {turn}: {tool_count} tool{} ({pass_count} ok{})",
        if tool_count != 1 { "s" } else { "" },
        if fail_count > 0 {
            format!(", {fail_count} err")
        } else {
            String::new()
        },
    );
    lines.push(Line::from(vec![
        Span::styled("  │ ", Style::default().fg(muted)),
        Span::styled(
            header,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Separator.
    lines.push(Line::from(vec![
        Span::styled("  ├", Style::default().fg(muted)),
        Span::styled(
            "─".repeat(width.saturating_sub(4)),
            Style::default().fg(muted),
        ),
    ]));

    // Tool list.
    for tool in &state.tools {
        let (icon, color) = if tool.is_error {
            ("✗", error)
        } else {
            ("✓", success)
        };

        let result_info = if let Some(ref preview) = tool.result_preview {
            let suffix = if tool.line_count > 1 {
                format!(" (+{})", tool.line_count - 1)
            } else {
                String::new()
            };
            format!(" → {}{}", truncate(preview, 50), suffix)
        } else {
            String::new()
        };

        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(muted)),
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::styled(
                &tool.name,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", truncate(&tool.detail, 40)),
                Style::default().fg(muted),
            ),
            Span::styled(result_info, Style::default().fg(muted)),
        ]));
    }

    // Token usage.
    if state.tokens_in > 0 || state.tokens_out > 0 {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(muted)),
            Span::styled(
                format!(
                    "⟡ {}in · {}out{}{}",
                    state.tokens_in,
                    state.tokens_out,
                    if state.cache_read > 0 {
                        format!(" · {}cache↓", state.cache_read)
                    } else {
                        String::new()
                    },
                    if state.cache_write > 0 {
                        format!(" · {}cache↑", state.cache_write)
                    } else {
                        String::new()
                    },
                ),
                Style::default().fg(muted),
            ),
        ]));
    }

    // Bottom border.
    lines.push(Line::from(vec![
        Span::styled("  ╰", Style::default().fg(muted)),
        Span::styled(border, Style::default().fg(muted)),
    ]));

    let buf = render_lines_to_ansi(&lines);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

/// Render a status bar at the bottom of output.
pub fn render_status_bar(model: &str, turn: usize, tokens: u64, cost: f64) {
    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);
    let muted = theme_to_ratatui(t.muted);

    let width = term_width();
    let left = format!(" {model} ");
    let right = format!(" turn {} │ {} tokens │ ${:.4} ", turn, tokens, cost);
    let padding = width.saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(
            left,
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding), Style::default()),
        Span::styled(right, Style::default().fg(muted)),
    ]);

    let buf = render_lines_to_ansi(&[line]);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

// ---- Internal helpers ----

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .min(120)
}

/// Render ratatui Lines to ANSI escape string (inline, no alternate screen).
fn render_lines_to_ansi(lines: &[Line<'_>]) -> String {
    let mut buf = String::new();
    for line in lines {
        for span in &line.spans {
            let mut codes = Vec::new();
            if let Some(fg) = span.style.fg {
                match fg {
                    Color::Rgb(r, g, b) => codes.push(format!("38;2;{r};{g};{b}")),
                    _ => codes.push(color_to_fg_code(fg).to_string()),
                }
            }
            if let Some(bg) = span.style.bg {
                match bg {
                    Color::Rgb(r, g, b) => codes.push(format!("48;2;{r};{g};{b}")),
                    _ => codes.push(color_to_bg_code(bg).to_string()),
                }
            }
            if span.style.add_modifier.contains(Modifier::BOLD) {
                codes.push("1".to_string());
            }
            if span.style.add_modifier.contains(Modifier::ITALIC) {
                codes.push("3".to_string());
            }
            if span.style.add_modifier.contains(Modifier::UNDERLINED) {
                codes.push("4".to_string());
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

fn color_to_fg_code(color: Color) -> String {
    match color {
        Color::Black => "30".into(),
        Color::Red => "31".into(),
        Color::Green => "32".into(),
        Color::Yellow => "33".into(),
        Color::Blue => "34".into(),
        Color::Magenta => "35".into(),
        Color::Cyan => "36".into(),
        Color::White | Color::Gray => "37".into(),
        Color::DarkGray => "90".into(),
        Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        _ => "39".into(),
    }
}

fn color_to_bg_code(color: Color) -> String {
    match color {
        Color::Black => "40".into(),
        Color::Red => "41".into(),
        Color::Green => "42".into(),
        Color::Yellow => "43".into(),
        Color::Blue => "44".into(),
        Color::Magenta => "45".into(),
        Color::Cyan => "46".into(),
        Color::White | Color::Gray => "47".into(),
        Color::DarkGray => "100".into(),
        Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        _ => "49".into(),
    }
}

// ---- Scrollback viewer ----

/// Interactive scrollable conversation history viewer.
/// Uses crossterm raw mode for keyboard input. Press q or Esc to exit.
pub fn scrollback_viewer(messages: &[agent_code_lib::llm::message::Message]) {
    use crossterm::{
        event::{self, Event, KeyCode, KeyEvent},
        terminal,
    };

    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);
    let muted = theme_to_ratatui(t.muted);
    let success = theme_to_ratatui(t.success);
    let error = theme_to_ratatui(t.error);

    // Build display lines from messages.
    let mut all_lines: Vec<Line<'static>> = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        match msg {
            agent_code_lib::llm::message::Message::User(u) => {
                // User header.
                all_lines.push(Line::from(vec![Span::styled(
                    format!(" [{idx}] USER "),
                    Style::default()
                        .fg(Color::Black)
                        .bg(accent)
                        .add_modifier(Modifier::BOLD),
                )]));
                // User content.
                for block in &u.content {
                    match block {
                        agent_code_lib::llm::message::ContentBlock::Text { text } => {
                            for line in text.lines() {
                                all_lines.push(Line::from(Span::raw(format!("  {line}"))));
                            }
                        }
                        agent_code_lib::llm::message::ContentBlock::ToolResult {
                            content,
                            is_error,
                            ..
                        } => {
                            let color = if *is_error { error } else { success };
                            let icon = if *is_error { "✗" } else { "✓" };
                            let preview: String = content
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(60)
                                .collect();
                            all_lines.push(Line::from(vec![
                                Span::styled(format!("  {icon} "), Style::default().fg(color)),
                                Span::styled(preview, Style::default().fg(muted)),
                            ]));
                        }
                        _ => {}
                    }
                }
                all_lines.push(Line::from(""));
            }
            agent_code_lib::llm::message::Message::Assistant(a) => {
                // Assistant header.
                let model_tag = a
                    .model
                    .as_deref()
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default();
                all_lines.push(Line::from(vec![Span::styled(
                    format!(" [{idx}] ASSISTANT{model_tag} "),
                    Style::default()
                        .fg(Color::Black)
                        .bg(success)
                        .add_modifier(Modifier::BOLD),
                )]));
                // Content.
                let mut tool_count = 0;
                for block in &a.content {
                    match block {
                        agent_code_lib::llm::message::ContentBlock::Text { text } => {
                            for line in text.lines().take(20) {
                                all_lines.push(Line::from(Span::raw(format!("  {line}"))));
                            }
                            if text.lines().count() > 20 {
                                all_lines.push(Line::from(Span::styled(
                                    format!("  ... ({} more lines)", text.lines().count() - 20),
                                    Style::default().fg(muted),
                                )));
                            }
                        }
                        agent_code_lib::llm::message::ContentBlock::ToolUse { name, .. } => {
                            tool_count += 1;
                            all_lines.push(Line::from(vec![
                                Span::styled("  → ", Style::default().fg(muted)),
                                Span::styled(
                                    name.to_string(),
                                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                                ),
                            ]));
                        }
                        agent_code_lib::llm::message::ContentBlock::Thinking {
                            thinking, ..
                        } => {
                            let preview: String = thinking.chars().take(80).collect();
                            all_lines.push(Line::from(Span::styled(
                                format!("  (thinking: {preview}...)"),
                                Style::default().fg(muted).add_modifier(Modifier::ITALIC),
                            )));
                        }
                        _ => {}
                    }
                }
                if tool_count > 0 {
                    all_lines.push(Line::from(Span::styled(
                        format!(
                            "  ({tool_count} tool call{})",
                            if tool_count != 1 { "s" } else { "" }
                        ),
                        Style::default().fg(muted),
                    )));
                }
                all_lines.push(Line::from(""));
            }
            _ => {}
        }
    }

    if all_lines.is_empty() {
        return;
    }

    // Enter scrollable view.
    let (term_w, term_h) = terminal::size().unwrap_or((80, 24));
    let view_height = (term_h as usize).saturating_sub(3); // Reserve for header + footer.
    let max_scroll = all_lines.len().saturating_sub(view_height);
    let mut scroll: usize = max_scroll; // Start at bottom (most recent).

    terminal::enable_raw_mode().expect("raw mode");

    render_scrollback(
        &all_lines,
        scroll,
        view_height,
        term_w as usize,
        messages.len(),
    );

    loop {
        if let Ok(Event::Key(KeyEvent { code, .. })) = event::read() {
            match code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up | KeyCode::Char('k') => {
                    scroll = scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    scroll = scroll.min(max_scroll).saturating_add(1).min(max_scroll);
                }
                KeyCode::PageUp => {
                    scroll = scroll.saturating_sub(view_height);
                }
                KeyCode::PageDown => {
                    scroll = (scroll + view_height).min(max_scroll);
                }
                KeyCode::Home => {
                    scroll = 0;
                }
                KeyCode::End => {
                    scroll = max_scroll;
                }
                _ => continue,
            }
            // Clear and re-render.
            clear_scrollback(view_height + 2);
            render_scrollback(
                &all_lines,
                scroll,
                view_height,
                term_w as usize,
                messages.len(),
            );
        }
    }

    terminal::disable_raw_mode().expect("disable raw mode");
    clear_scrollback(view_height + 2);
    println!("  (exited scroll view)\r");
}

fn render_scrollback(
    lines: &[Line<'_>],
    scroll: usize,
    view_height: usize,
    width: usize,
    msg_count: usize,
) {
    let t = super::theme::current();
    let muted = theme_to_ratatui(t.muted);
    let accent = theme_to_ratatui(t.accent);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Header.
    let header = format!(
        " Conversation ({msg_count} messages) │ ↑↓ scroll │ PgUp/PgDn │ Home/End │ q quit "
    );
    let hdr_line = Line::from(Span::styled(
        truncate(&header, width),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    ));
    write!(out, "{}", render_lines_to_ansi(&[hdr_line])).ok();

    // Content.
    let end = (scroll + view_height).min(lines.len());
    let visible = &lines[scroll..end];
    let buf = render_lines_to_ansi(visible);
    write!(out, "{buf}").ok();

    // Pad remaining lines.
    for _ in visible.len()..view_height {
        write!(out, "  ~\r\n").ok();
    }

    // Footer: scroll position.
    let pct = if lines.len() <= view_height {
        100
    } else {
        (scroll * 100) / lines.len().saturating_sub(view_height).max(1)
    };
    let footer = format!(" line {}-{} of {} ({pct}%) ", scroll + 1, end, lines.len());
    let ftr_line = Line::from(Span::styled(
        truncate(&footer, width),
        Style::default().fg(muted),
    ));
    write!(out, "{}", render_lines_to_ansi(&[ftr_line])).ok();

    out.flush().ok();
}

fn clear_scrollback(lines: usize) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for _ in 0..lines {
        write!(out, "\x1b[A\x1b[2K").ok();
    }
    out.flush().ok();
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

// ---- Pixel art crab banner ----

/// Render the crab mascot as pixel art using half-block characters.
/// Each cell uses ▀ with fg=top pixel, bg=bottom pixel for 2x vertical resolution.
/// Returns a Vec of pre-rendered ANSI strings (one per display line).
pub fn render_crab_banner() -> Vec<String> {
    // Color palette (matching the pixel art logo).
    const X: u8 = 0; // transparent (use terminal default)
    const P: u8 = 1; // purple body (#A422E1)
    const D: u8 = 2; // dark purple (shadow/outline)
    const L: u8 = 3; // light purple (belly highlight)
    const W: u8 = 4; // white (eyes)
    const B: u8 = 5; // black (pupils)
    const G: u8 = 6; // grey (gear)
    const T: u8 = 7; // dark grey (terminal screen)
    const C: u8 = 8; // cyan (terminal text)

    fn palette(idx: u8) -> Option<Color> {
        match idx {
            0 => None,                            // transparent
            1 => Some(Color::Rgb(164, 34, 225)),  // purple #A422E1
            2 => Some(Color::Rgb(100, 15, 140)),  // dark purple
            3 => Some(Color::Rgb(200, 140, 220)), // light purple
            4 => Some(Color::White),
            5 => Some(Color::Black),
            6 => Some(Color::Rgb(150, 150, 160)), // gear grey
            7 => Some(Color::Rgb(50, 50, 65)),    // terminal dark
            8 => Some(Color::Rgb(80, 220, 120)),  // terminal green
            _ => None,
        }
    }

    // 18 wide x 14 tall pixel grid.
    // The crab: claw holding terminal, wide body, big eyes, gear belly, legs.
    #[rustfmt::skip]
    let pixels: &[&[u8]] = &[
        //                  terminal
        &[X,X,X,X,X,X,T,T,T,T,T,X,X,X,X,X,X,X],
        &[X,X,X,X,X,X,T,C,C,C,T,X,X,X,X,X,X,X],
        &[X,X,X,X,X,X,T,T,T,T,T,X,X,X,X,X,X,X],
        //         claw up to terminal, body starts
        &[X,X,X,D,P,P,D,X,X,X,X,X,X,X,X,X,X,X],
        &[X,X,D,P,P,D,D,P,P,P,P,D,X,X,X,X,X,X],
        //         wide body with eyes
        &[X,D,P,P,W,B,P,P,P,P,W,B,P,P,D,X,X,X],
        &[X,D,P,P,W,W,P,P,P,P,W,W,P,P,D,X,X,X],
        //         body with gear
        &[X,X,D,P,P,L,L,G,G,L,L,P,P,D,X,X,X,X],
        &[X,X,D,P,L,L,G,G,G,G,L,L,P,D,X,X,X,X],
        //         lower body
        &[X,X,X,D,P,P,L,L,L,L,P,P,D,X,X,X,X,X],
        &[X,X,X,D,D,P,P,P,P,P,P,D,D,X,X,X,X,X],
        //         legs
        &[X,X,D,P,X,D,P,X,X,P,D,X,P,D,X,X,X,X],
        &[X,D,P,X,X,X,D,P,P,D,X,X,X,P,D,X,X,X],
        &[D,P,X,X,X,X,X,D,D,X,X,X,X,X,P,D,X,X],
    ];

    let height = pixels.len();
    let mut lines = Vec::new();

    // Process 2 rows at a time using ▀ (upper half block).
    let mut row = 0;
    while row < height {
        let mut line = String::new();
        line.push_str("  "); // left margin

        let top_row = pixels[row];
        let bot_row = if row + 1 < height {
            pixels[row + 1]
        } else {
            &[0u8; 18] as &[u8]
        };

        for (top, bot) in top_row.iter().zip(bot_row.iter()) {
            let fg = palette(*top);
            let bg = palette(*bot);
            render_half_block(&mut line, fg, bg);
        }

        lines.push(line);
        row += 2;
    }

    lines
}

fn render_half_block(line: &mut String, fg: Option<Color>, bg: Option<Color>) {
    match (fg, bg) {
        (None, None) => line.push(' '),
        (Some(fg_c), None) => {
            line.push_str(&format!("\x1b[{}m▀\x1b[0m", rgb_fg(fg_c)));
        }
        (None, Some(bg_c)) => {
            line.push_str(&format!("\x1b[{}m▄\x1b[0m", rgb_fg(bg_c)));
        }
        (Some(fg_c), Some(bg_c)) => {
            line.push_str(&format!("\x1b[{};{}m▀\x1b[0m", rgb_fg(fg_c), rgb_bg(bg_c)));
        }
    }
}

/// Render a shimmer frame of the crab. The purple pixels shift brightness
/// in a wave pattern based on the frame number.
pub fn render_crab_shimmer(frame: usize) -> Vec<String> {
    // Same pixel grid as render_crab_banner but with animated colors.
    const X: u8 = 0;
    const P: u8 = 1;
    const D: u8 = 2;
    const L: u8 = 3;
    const W: u8 = 4;
    const B: u8 = 5;
    const G: u8 = 6;
    const T: u8 = 7;
    const C: u8 = 8;

    fn palette_shimmer(idx: u8, col: usize, frame: usize) -> Option<Color> {
        match idx {
            0 => None,
            1 => {
                // Purple with wave shimmer.
                let wave = ((col + frame * 3) % 6) as i16;
                let boost = if wave < 3 { wave * 15 } else { (6 - wave) * 15 };
                Some(Color::Rgb(
                    (164 + boost).min(255) as u8,
                    (34 + boost / 2).min(100) as u8,
                    (225 + boost / 3).min(255) as u8,
                ))
            }
            2 => Some(Color::Rgb(100, 15, 140)),
            3 => {
                let wave = ((col + frame * 3) % 6) as i16;
                let boost = if wave < 3 { wave * 10 } else { (6 - wave) * 10 };
                Some(Color::Rgb(
                    (200 + boost).min(255) as u8,
                    (140 + boost).min(200) as u8,
                    (220 + boost / 2).min(255) as u8,
                ))
            }
            4 => Some(Color::White),
            5 => Some(Color::Black),
            6 => Some(Color::Rgb(150, 150, 160)),
            7 => Some(Color::Rgb(50, 50, 65)),
            8 => {
                // Terminal text blinks.
                if frame.is_multiple_of(2) {
                    Some(Color::Rgb(80, 220, 120))
                } else {
                    Some(Color::Rgb(50, 50, 65))
                }
            }
            _ => None,
        }
    }

    #[rustfmt::skip]
    let pixels: &[&[u8]] = &[
        &[X,X,X,X,X,X,T,T,T,T,T,X,X,X,X,X,X,X],
        &[X,X,X,X,X,X,T,C,C,C,T,X,X,X,X,X,X,X],
        &[X,X,X,X,X,X,T,T,T,T,T,X,X,X,X,X,X,X],
        &[X,X,X,D,P,P,D,X,X,X,X,X,X,X,X,X,X,X],
        &[X,X,D,P,P,D,D,P,P,P,P,D,X,X,X,X,X,X],
        &[X,D,P,P,W,B,P,P,P,P,W,B,P,P,D,X,X,X],
        &[X,D,P,P,W,W,P,P,P,P,W,W,P,P,D,X,X,X],
        &[X,X,D,P,P,L,L,G,G,L,L,P,P,D,X,X,X,X],
        &[X,X,D,P,L,L,G,G,G,G,L,L,P,D,X,X,X,X],
        &[X,X,X,D,P,P,L,L,L,L,P,P,D,X,X,X,X,X],
        &[X,X,X,D,D,P,P,P,P,P,P,D,D,X,X,X,X,X],
        &[X,X,D,P,X,D,P,X,X,P,D,X,P,D,X,X,X,X],
        &[X,D,P,X,X,X,D,P,P,D,X,X,X,P,D,X,X,X],
        &[D,P,X,X,X,X,X,D,D,X,X,X,X,X,P,D,X,X],
    ];

    let height = pixels.len();
    let mut lines = Vec::new();

    let mut row = 0;
    while row < height {
        let mut line = String::new();
        line.push_str("  ");

        let top_row = pixels[row];
        let bot_row: &[u8] = if row + 1 < height {
            pixels[row + 1]
        } else {
            &[0u8; 18]
        };

        for (col, (top, bot)) in top_row.iter().zip(bot_row.iter()).enumerate() {
            let fg = palette_shimmer(*top, col, frame);
            let bg = palette_shimmer(*bot, col, frame);
            render_half_block(&mut line, fg, bg);
        }

        lines.push(line);
        row += 2;
    }

    lines
}

fn rgb_fg(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        Color::White => "97".into(),
        Color::Black => "30".into(),
        _ => "39".into(),
    }
}

fn rgb_bg(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        Color::White => "107".into(),
        Color::Black => "40".into(),
        _ => "49".into(),
    }
}
