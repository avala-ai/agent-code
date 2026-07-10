//! Ratatui drawing for the modern TUI.
//!
//! Pure function of [`App`] + area — used by both the live terminal and
//! the `TestBackend` visual tests.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::app::{App, PendingPermission, Phase, TranscriptItem};
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

    if app.phase == Phase::Permission
        && let Some(ref pending) = app.pending_permission
    {
        draw_permission_modal(frame, area, pending);
    }
}

fn draw_permission_modal(frame: &mut Frame<'_>, area: Rect, pending: &PendingPermission) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        pending.description.clone(),
        Style::default().fg(Color::White),
    )));
    if let Some(ref preview) = pending.input_preview {
        lines.push(Line::from(""));
        const MAX_PREVIEW: usize = 10;
        let total = preview.lines().count();
        for row in preview.lines().take(MAX_PREVIEW) {
            lines.push(Line::from(Span::styled(
                row.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if total > MAX_PREVIEW {
            lines.push(Line::from(Span::styled(
                format!("… {} more lines", total - MAX_PREVIEW),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[y] allow once   [a] allow session   [n]/[Esc] deny",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));

    let width = area.width.saturating_sub(6).clamp(24, 70);
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2).max(3));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(format!(" permission · {} ", pending.name));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
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
        Span::styled(
            if app.mode_pending {
                // `*` = not yet applied to the engine (lock held by the turn).
                format!(" {}* ", app.mode.short_badge())
            } else {
                format!(" {} ", app.mode.short_badge())
            },
            if app.mode_pending {
                mode_style.add_modifier(Modifier::DIM)
            } else {
                mode_style
            },
        ),
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
    // Char-based, not byte-based: byte slicing panics on multibyte cwds.
    let count = path.chars().count();
    if max < 4 || count <= max {
        return path.to_string();
    }
    let tail: String = path.chars().skip(count - (max - 1)).collect();
    format!("…{tail}")
}

fn truncate_mid(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
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
    fn permission_modal_renders_over_ui() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        app.pending_permission = Some(PendingPermission {
            name: "Bash".into(),
            description: "Bash: run `cargo publish`".into(),
            input_preview: Some("{\n  \"command\": \"cargo publish\"\n}".into()),
            respond,
        });
        term.draw(|f| draw(f, &app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("permission · Bash"), "buffer:\n{s}");
        assert!(s.contains("allow once"), "buffer:\n{s}");
        assert!(s.contains("cargo publish"), "buffer:\n{s}");
    }

    #[test]
    fn truncate_helpers_are_char_safe() {
        let p = "/home/пользователь/проект-с-длинным-именем";
        let t = truncate_path(p, 10);
        assert!(t.chars().count() <= 10, "{t}");
        let m = truncate_mid("日本語のセッション識別子", 6);
        assert!(m.chars().count() <= 6, "{m}");
    }

    #[test]
    fn pending_mode_badge_shows_star() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.mode = SessionMode::Plan;
        app.mode_pending = true;
        term.draw(|f| draw(f, &app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("PLAN*"), "buffer:\n{s}");
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
