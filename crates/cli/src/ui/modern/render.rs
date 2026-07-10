//! Ratatui drawing for the modern TUI.
//!
//! `draw` takes `&mut App` only so the virtualized [`super::layout::LayoutCache`]
//! can update during layout — the one mutation the view model permits. No I/O
//! happens here; used by both the live terminal and the `TestBackend` tests.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::app::{App, PendingPermission, Phase};
use super::mode::SessionMode;

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    // A chips row appears above the prompt only when prompts are queued.
    let chips_h = if app.queue.is_empty() { 0 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),       // header
            Constraint::Min(5),          // transcript
            Constraint::Length(1),       // status
            Constraint::Length(chips_h), // queue chips (0 when empty)
            Constraint::Length(3),       // input
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_transcript(frame, chunks[1], app);
    draw_status(frame, chunks[2], app);
    if chips_h > 0 {
        draw_queue_chips(frame, chunks[3], app);
    }
    draw_input(frame, chunks[4], app);

    if app.phase == Phase::Permission
        && let Some(pending) = app.front_permission().cloned()
    {
        draw_permission_modal(frame, area, &pending, app.pending_modal_count());
    }
}

/// Queue chips row: `⧉ queued: ❶ "…" ❷ "…"` above the prompt (plan §M5).
fn draw_queue_chips(frame: &mut Frame<'_>, area: Rect, app: &App) {
    const CIRCLED: [&str; 9] = ["❶", "❷", "❸", "❹", "❺", "❻", "❼", "❽", "❾"];
    let mut spans = vec![Span::styled(
        "⧉ queued: ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    for (i, p) in app.queue.iter().enumerate().take(CIRCLED.len()) {
        let mark = CIRCLED[i];
        let text: String = p.chars().take(40).collect();
        let ellipsis = if p.chars().count() > 40 { "…" } else { "" };
        spans.push(Span::styled(
            format!("{mark} \"{text}{ellipsis}\"  "),
            Style::default().fg(Color::Gray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_permission_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    pending: &PendingPermission,
    pending_behind: usize,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if pending_behind > 0 {
        lines.push(Line::from(Span::styled(
            format!("⚠ {pending_behind} more pending"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
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

/// Draw the transcript. Populates `app.layout` (the one permitted view-model
/// side effect), then renders only the virtualized viewport slice — off-screen
/// blocks are never copied. The `app` is `&mut` so the cache can update.
fn draw_transcript(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    // Reserve the top row for the title/spinner.
    let inner = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    let height = inner.height as usize;

    // Rebuild only the changed blocks at this width; record metrics for the
    // scroll-key handlers that run before the next draw.
    app.layout.sync(&app.transcript, inner.width);
    app.viewport_h = height;
    let total = app.layout.total_lines();
    let top = app.scroll.top(total, height);
    let view = app.layout.viewport(top, height);

    let title_block =
        Block::default()
            .borders(Borders::NONE)
            .title(if app.phase == Phase::Streaming {
                let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
                let f = frames[(app.tick as usize) % frames.len()];
                format!(" {f} streaming ")
            } else {
                " transcript ".into()
            });
    // Lines are pre-wrapped by the layout cache; no widget wrapping.
    frame.render_widget(Paragraph::new(view).block(title_block), area);

    // Jump-to-bottom pill when reading above the live tail (plan §M2).
    let below = app.scroll.lines_below(total, height);
    if below > 0 {
        draw_jump_pill(frame, inner, below);
    }
}

/// Floating "↓ N new" pill anchored bottom-right of the transcript area.
fn draw_jump_pill(frame: &mut Frame<'_>, area: Rect, n: usize) {
    let label = if n > 99 {
        " ↓ 99+ new ".to_string()
    } else {
        format!(" ↓ {n} new ")
    };
    let w = label.chars().count() as u16;
    if area.width < w + 1 || area.height < 1 {
        return;
    }
    let rect = Rect {
        x: area.x + area.width - w - 1,
        y: area.y + area.height.saturating_sub(1),
        width: w,
        height: 1,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))),
        rect,
    );
}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let tokens = app.tokens_in + app.tokens_out;
    let mut spans = vec![
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
    ];

    // Waiting-on spinner while a turn runs (plan §M4); otherwise the last
    // status message.
    if app.phase == Phase::Streaming {
        let glyph = SPINNER[(app.tick as usize) % SPINNER.len()];
        let (glyph_color, text_color) = match app.waiting_on {
            super::app::WaitingOn::UserInput => (Color::Yellow, Color::Yellow),
            _ => (Color::Cyan, Color::Gray),
        };
        spans.push(Span::styled(
            format!(" {glyph} "),
            Style::default().fg(glyph_color),
        ));
        spans.push(Span::styled(
            format!("{} ", app.waiting_on.label()),
            Style::default().fg(text_color),
        ));
    } else {
        spans.push(Span::styled(
            format!(" {} ", app.status_message),
            Style::default().fg(Color::Gray),
        ));
    }
    if !app.queue.is_empty() {
        spans.push(Span::raw("│"));
        spans.push(Span::styled(
            format!(" ⧉ {} queued ", app.queue.len()),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans.push(Span::raw("│"));
    spans.push(Span::styled(
        format!(" sid {} ", truncate_mid(&app.session_id, 12)),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    use crate::ui::modern::app::TranscriptItem;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn idle_frame_contains_branding_and_mode() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("gpt-5.4", "/home/user/project", "abc12345");
        term.draw(|f| draw(f, &mut app)).unwrap();
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
        term.draw(|f| draw(f, &mut app)).unwrap();
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
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                PendingPermission {
                    name: "Bash".into(),
                    description: "Bash: run `cargo publish`".into(),
                    input_preview: Some("{\n  \"command\": \"cargo publish\"\n}".into()),
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("permission · Bash"), "buffer:\n{s}");
        assert!(s.contains("allow once"), "buffer:\n{s}");
        assert!(s.contains("cargo publish"), "buffer:\n{s}");
    }

    #[test]
    fn permission_modal_shows_pending_badge_when_queued() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        for name in ["first", "second", "third"] {
            let (respond, _rx) = std::sync::mpsc::channel();
            app.modals
                .push_back(crate::ui::modern::app::Modal::Permission(
                    PendingPermission {
                        name: name.into(),
                        description: format!("{name} ask"),
                        input_preview: None,
                        respond,
                    },
                ));
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("permission · first"), "front modal:\n{s}");
        assert!(s.contains("2 more pending"), "badge missing:\n{s}");
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
        term.draw(|f| draw(f, &mut app)).unwrap();
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
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        // Typed card: kind label + status glyph + detail.
        assert!(s.contains("bash"), "kind label missing; buffer:\n{s}");
        assert!(s.contains('✓'), "ok glyph missing; buffer:\n{s}");
        assert!(s.contains("cargo test"), "buffer:\n{s}");
    }

    #[test]
    fn queue_chips_and_count_render() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.queue.push_back("fix the flaky test".into());
        app.queue.push_back("then update changelog".into());
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("queued:"), "chips row missing:\n{s}");
        assert!(s.contains("fix the flaky test"), "chip text missing:\n{s}");
        assert!(s.contains("2 queued"), "status count missing:\n{s}");
    }

    #[test]
    fn waiting_on_spinner_shows_running_tool() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.waiting_on = crate::ui::modern::app::WaitingOn::Tool("Bash".into());
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("running Bash"), "buffer:\n{s}");
    }

    #[test]
    fn assistant_markdown_renders_in_transcript() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::Assistant(
            "# Heading\n\nSome **bold** and `code` and a list:\n\n- item one\n- item two".into(),
        ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("Heading"), "buffer:\n{s}");
        assert!(s.contains("• item one"), "buffer:\n{s}");
        assert!(s.contains("bold"), "buffer:\n{s}");
    }

    #[test]
    fn jump_pill_shows_when_scrolled_up() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.clear();
        for i in 0..200 {
            app.transcript
                .push(TranscriptItem::System(format!("row {i}")));
        }
        // First draw records the viewport height, then scroll up into Free.
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.scroll_up(50);
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("new"), "expected jump pill; buffer:\n{s}");
        // Following (bottom) shows no pill.
        app.scroll_to_bottom();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s2 = buffer_to_string(term.backend().buffer());
        assert!(!s2.contains("↓"), "no pill while following; buffer:\n{s2}");
    }
}
