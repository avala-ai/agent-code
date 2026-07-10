//! Ratatui drawing for the modern TUI.
//!
//! Pure function of [`App`] + area — used by both the live terminal and
//! the `TestBackend` visual tests.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::app::{App, Phase, TranscriptItem};
use super::mode::SessionMode;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(5),    // transcript
            Constraint::Length(1), // status
            Constraint::Length(3), // input
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_transcript(frame, chunks[1], app);
    draw_status(frame, chunks[2], app);
    draw_input(frame, chunks[3], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mode_style = mode_style(app.mode);
    let title = Line::from(vec![
        Span::styled(
            " agent-code ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(&app.version, Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(&app.model, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(format!(" {} ", app.mode.short_badge()), mode_style),
        Span::raw("  "),
        Span::styled(
            truncate_path(&app.cwd, area.width.saturating_sub(40) as usize),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(Paragraph::new(title).block(block), area);
}

fn draw_transcript(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for item in &app.transcript {
        match item {
            TranscriptItem::User(t) => {
                lines.push(Line::from(vec![
                    Span::styled("❯ ", Style::default().fg(Color::Cyan)),
                    Span::styled(t.clone(), Style::default().fg(Color::White)),
                ]));
                lines.push(Line::from(""));
            }
            TranscriptItem::Assistant(t) => {
                for row in t.lines() {
                    lines.push(Line::from(Span::raw(row.to_string())));
                }
                lines.push(Line::from(""));
            }
            TranscriptItem::Thinking(t) => {
                let preview: String = t.chars().take(120).collect();
                lines.push(Line::from(Span::styled(
                    format!("  thinking… {preview}"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
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
    }

    // Stick to bottom unless user scrolled up.
    let height = area.height.saturating_sub(2) as usize;
    let total = lines.len();
    let max_off = total.saturating_sub(height);
    let off = (app.scroll_offset as usize).min(max_off);
    let start = total.saturating_sub(height + off);
    let end = total.saturating_sub(off);
    let view: Vec<Line<'static>> = lines
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();

    let block = Block::default()
        .borders(Borders::NONE)
        .title(if app.phase == Phase::Streaming {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let f = frames[(app.tick as usize) % frames.len()];
            format!(" {f} streaming ")
        } else {
            " transcript ".into()
        });
    frame.render_widget(
        Paragraph::new(view).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let tokens = app.tokens_in + app.tokens_out;
    let line = Line::from(vec![
        Span::styled(
            format!(" turn {} ", app.turn_count),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" {tokens} tok "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" ${:.4} ", app.cost_usd),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" {} ", app.status_message),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" sid {} ", truncate_mid(&app.session_id, 12)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_input(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let border = if app.phase == Phase::Streaming {
        Color::Yellow
    } else {
        Color::Cyan
    };
    let title = if app.phase == Phase::Streaming {
        " input (buffered until turn ends) "
    } else {
        " input "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(title);
    let prompt = format!("❯ {}", app.input);
    frame.render_widget(Paragraph::new(prompt).block(block), area);
}

fn mode_style(mode: SessionMode) -> Style {
    let (fg, bg) = match mode {
        SessionMode::Normal => (Color::Black, Color::Green),
        SessionMode::Plan => (Color::Black, Color::Magenta),
        SessionMode::AcceptEdits => (Color::Black, Color::Blue),
        SessionMode::AlwaysApprove => (Color::Black, Color::Red),
    };
    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

fn truncate_path(path: &str, max: usize) -> String {
    if max < 4 || path.len() <= max {
        return path.to_string();
    }
    format!("…{}", &path[path.len() - (max - 1)..])
}

fn truncate_mid(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    format!("{}…", &s[..keep.min(s.len())])
}

/// Dump the terminal buffer to a plain multi-line string for snapshots.
#[cfg(test)]
pub fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area();
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            out.push_str(cell.symbol());
        }
        // trim trailing spaces per row for stable snapshots
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn idle_frame_contains_branding_and_mode() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let app = App::new("gpt-5.4", "/home/user/project", "abc12345");
        term.draw(|f| draw(f, &app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("agent-code"), "buffer:\n{s}");
        assert!(s.contains("NORMAL"), "buffer:\n{s}");
        assert!(s.contains("gpt-5.4"), "buffer:\n{s}");
        assert!(s.contains("Shift+Tab"), "buffer:\n{s}");
    }

    #[test]
    fn plan_mode_badge_visible() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.mode = SessionMode::Plan;
        app.transcript
            .push(TranscriptItem::User("design auth".into()));
        term.draw(|f| draw(f, &app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("PLAN"), "buffer:\n{s}");
        assert!(s.contains("design auth"), "buffer:\n{s}");
    }

    #[test]
    fn tool_card_renders() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::Tool {
            name: "Bash".into(),
            detail: "cargo test".into(),
            result: Some("ok".into()),
            is_error: false,
        });
        term.draw(|f| draw(f, &app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("Bash"), "buffer:\n{s}");
        assert!(s.contains("cargo test"), "buffer:\n{s}");
    }
}
