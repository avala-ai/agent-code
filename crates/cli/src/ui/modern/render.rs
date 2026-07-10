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
use super::colors::palette;
use super::mode::SessionMode;

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    app.frame_count = app.frame_count.wrapping_add(1);
    let area = frame.area();
    // Minimal skin (plan §M10) drops the header and the framed prompt for a
    // compact look — same block model, render config only.
    let minimal = app.skin == crate::ui::modern::app::Skin::Minimal;
    let header_h = if minimal { 0 } else { 3 };
    let prompt_h = if minimal { 1 } else { 3 };
    // A chips row appears above the prompt only when prompts are queued.
    let chips_h = if app.queue.is_empty() { 0 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h), // header (0 in minimal)
            Constraint::Min(5),           // transcript
            Constraint::Length(1),        // status
            Constraint::Length(chips_h),  // queue chips (0 when empty)
            Constraint::Length(prompt_h), // input
        ])
        .split(area);

    if header_h > 0 {
        draw_header(frame, chunks[0], app);
    }
    // Tasks pane (plan §M8): a right split ≥110 wide, else a below-transcript
    // strip; hidden when there are no tasks.
    if app.tasks_visible() {
        let (transcript_area, pane_area) = if chunks[1].width >= 110 {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(20), Constraint::Length(32)])
                .split(chunks[1]);
            (cols[0], cols[1])
        } else {
            let strip = 5.min(chunks[1].height.saturating_sub(3));
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(strip)])
                .split(chunks[1]);
            (rows[0], rows[1])
        };
        draw_transcript(frame, transcript_area, app);
        draw_tasks_pane(frame, pane_area, app);
    } else {
        draw_transcript(frame, chunks[1], app);
    }
    draw_status(frame, chunks[2], app);
    if chips_h > 0 {
        draw_queue_chips(frame, chunks[3], app);
    }
    draw_input(frame, chunks[4], app);

    if app.phase == Phase::Permission
        && let Some(modal) = app.front_modal().cloned()
    {
        let behind = app.pending_modal_count();
        match modal {
            crate::ui::modern::app::Modal::Permission(p) => {
                draw_permission_modal(frame, area, &p, behind)
            }
            crate::ui::modern::app::Modal::Plan(p) => draw_plan_modal(frame, area, &p, behind),
            crate::ui::modern::app::Modal::Question(q) => {
                draw_question_modal(frame, area, &q, behind)
            }
        }
    }
}

/// Plan-approval modal: renders the plan markdown with approve/keep/dismiss.
fn draw_plan_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    plan: &crate::ui::modern::app::PlanReview,
    pending_behind: usize,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if pending_behind > 0 {
        lines.push(Line::from(Span::styled(
            format!("⚠ {pending_behind} more pending"),
            Style::default().fg(palette().warning),
        )));
        lines.push(Line::from(""));
    }
    // Show up to a bounded slice of the rendered plan markdown.
    let rendered = super::markdown::render_markdown(&plan.plan_md).lines;
    let max_body = area.height.saturating_sub(8) as usize;
    let total = rendered.len();
    for l in rendered.into_iter().take(max_body) {
        lines.push(l);
    }
    if total > max_body {
        lines.push(Line::from(Span::styled(
            format!("… {} more lines", total - max_body),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let title = match &plan.path {
        Some(p) => format!(" plan · {p} "),
        None => " plan · proposed ".to_string(),
    };
    let accent = palette().accent;
    draw_modal_box(
        frame,
        area,
        lines,
        &title,
        accent,
        Some(key_hint_line(
            "[a] approve & start   [k] keep planning   [Esc] dismiss",
        )),
    );
}

/// Ask-user question overlay: the current question + numbered options.
fn draw_question_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    q: &crate::ui::modern::app::QuestionState,
    pending_behind: usize,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if q.questions.len() > 1 {
        lines.push(Line::from(Span::styled(
            format!("question {} of {}", q.current + 1, q.questions.len()),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let cur = &q.questions[q.current];
    lines.push(Line::from(Span::styled(
        cur.question.clone(),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for (i, opt) in cur.options.iter().enumerate() {
        let selected = i == q.cursor;
        let marker = if selected { "❯" } else { " " };
        let accent = palette().accent;
        let style = if selected {
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        lines.push(Line::from(Span::styled(
            format!("{marker} {}. {opt}", i + 1),
            style,
        )));
    }
    let accent = palette().accent;
    draw_modal_box(
        frame,
        area,
        lines,
        " question ",
        accent,
        Some(key_hint_line(
            "↑/↓ move · [1]–[9] pick · Enter select · Esc cancel",
        )),
    );
}

/// Sticky footer style for modal keybindings — always visible, never clipped
/// by a long body/preview.
fn key_hint_line(text: impl Into<String>) -> Line<'static> {
    let warning = palette().warning;
    Line::from(Span::styled(
        text.into(),
        Style::default()
            .fg(warning)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Shared centered modal box with a border + title and an optional sticky
/// footer (key hints). The footer is laid out in its own row so wrapped body
/// text cannot push it off-screen.
fn draw_modal_box(
    frame: &mut Frame<'_>,
    area: Rect,
    lines: Vec<Line<'static>>,
    title: &str,
    border: Color,
    footer: Option<Line<'static>>,
) {
    let width = area.width.saturating_sub(6).clamp(40, 76);
    let footer_h: u16 = u16::from(footer.is_some());
    // +2 border, +footer, +1 breathing room for wrap
    let wanted = (lines.len() as u16)
        .saturating_add(2)
        .saturating_add(footer_h)
        .saturating_add(1);
    let height = wanted.min(area.height.saturating_sub(2).max(4 + footer_h));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(title.to_string());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if let Some(footer_line) = footer {
        let body_h = inner.height.saturating_sub(1);
        let body = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_h,
        };
        let foot = Rect {
            x: inner.x,
            y: inner.y.saturating_add(body_h),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            body,
        );
        // Fixed footer: key hints always land on the last inner row.
        frame.render_widget(Paragraph::new(footer_line), foot);
    } else {
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            inner,
        );
    }
}

/// Queue chips row: `⧉ queued: ❶ "…" ❷ "…"` above the prompt (plan §M5).
fn draw_queue_chips(frame: &mut Frame<'_>, area: Rect, app: &App) {
    const CIRCLED: [&str; 9] = ["❶", "❷", "❸", "❹", "❺", "❻", "❼", "❽", "❾"];
    let accent = palette().accent;
    let mut spans = vec![Span::styled(
        "⧉ queued: ",
        Style::default()
            .fg(accent)
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

/// Tasks/agents pane: state-ordered subagent rows (plan §M8).
fn draw_tasks_pane(frame: &mut Frame<'_>, area: Rect, app: &App) {
    use super::tasks::TaskState;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let inner_w = area.width.saturating_sub(2) as usize;
    for t in &app.tasks {
        let p = palette();
        let color = match t.state {
            TaskState::Working => Color::Blue,
            TaskState::NeedsInput => p.warning,
            TaskState::Done => p.success,
            TaskState::Failed => p.error,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", t.state.glyph()), Style::default().fg(color)),
            Span::styled(
                format!("{} ", t.state.word()),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        // Headline on its own row, truncated to the pane width.
        let head: String = t.headline.chars().take(inner_w.max(4)).collect();
        lines.push(Line::from(Span::styled(
            format!("  {head}"),
            Style::default().fg(Color::Gray),
        )));
    }
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" agents ({}) ", app.tasks.len()));
    frame.render_widget(Paragraph::new(lines).block(block), area);
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
                .fg(palette().warning)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        pending.description.clone(),
        Style::default().fg(Color::White),
    )));
    if let Some(ref origin) = pending.origin {
        lines.push(Line::from(Span::styled(
            format!("from {origin}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    if let Some(ref preview) = pending.input_preview {
        lines.push(Line::from(""));
        // Keep preview short so description stays readable; key footer is
        // sticky either way, but a huge body is still noisy.
        const MAX_PREVIEW: usize = 8;
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

    // Keys live in a sticky footer (not the scrollable body) so long
    // descriptions / previews cannot clip them — that was leaving some
    // popups with no [y]/[n] guidance at all.
    let accent = palette().accent;
    draw_modal_box(
        frame,
        area,
        lines,
        &format!(" permission · {} ", pending.name),
        accent,
        // Keep ≤ ~40 cols so min-width modals still show every binding
        // (digits 1/2/3 work the same as y/a/n; listed in /help).
        Some(key_hint_line(
            "[y] once   [a] session   [n]/[Esc] deny",
        )),
    );
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let accent = palette().accent;
    let mode_style = mode_style(app.mode);
    let title = Line::from(vec![
        Span::styled(
            " agent-code ",
            Style::default()
                .fg(Color::Black)
                .bg(accent)
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
    let accent = palette().accent;
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label,
            Style::default()
                .fg(Color::Black)
                .bg(accent)
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

    // Context meter: yellow ≥70%, red ≥90% (plan §M1/§6).
    if let Some((used, max)) = app.ctx_meter
        && max > 0
    {
        let pct = ((used as f64 / max as f64) * 100.0).round() as u32;
        let p = palette();
        let color = if pct >= 90 {
            p.error
        } else if pct >= 70 {
            p.warning
        } else {
            Color::DarkGray
        };
        spans.push(Span::styled(
            format!(" ctx {pct}% "),
            Style::default().fg(color),
        ));
        spans.push(Span::raw("│"));
    }

    // Waiting-on spinner while a turn runs (plan §M4); otherwise the last
    // status message.
    let accent = palette().accent;
    let warning = palette().warning;
    if app.phase == Phase::Streaming {
        let glyph = SPINNER[(app.tick as usize) % SPINNER.len()];
        let (glyph_color, text_color) = match app.waiting_on {
            super::app::WaitingOn::UserInput => (warning, warning),
            _ => (accent, Color::Gray),
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
            Style::default().fg(accent),
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
    let p = palette();
    let border = if app.phase == Phase::Streaming {
        p.warning
    } else {
        p.accent
    };
    let prompt = format!("❯ {}", app.input);
    // Minimal skin: borderless single-line prompt.
    if app.skin == crate::ui::modern::app::Skin::Minimal {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                prompt,
                Style::default().fg(border),
            ))),
            area,
        );
        return;
    }
    let title = if app.phase == Phase::Streaming {
        " input (buffered until turn ends) "
    } else {
        " input "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(title);
    frame.render_widget(Paragraph::new(prompt).block(block), area);
}

fn mode_style(mode: SessionMode) -> Style {
    let p = palette();
    let (fg, bg) = match mode {
        SessionMode::Manual => (Color::Black, p.warning),
        SessionMode::Normal => (Color::Black, p.success),
        SessionMode::AcceptEdits => (Color::Black, Color::Blue),
        // Classic plan-mode tag color (purple on midnight).
        SessionMode::Plan => (Color::Black, p.plan),
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
                    origin: Some("subagent-2".into()),
                    input_preview: Some("{\n  \"command\": \"cargo publish\"\n}".into()),
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("permission · Bash"), "buffer:\n{s}");
        assert!(s.contains("[y]"), "key hint [y] missing:\n{s}");
        assert!(s.contains("once"), "buffer:\n{s}");
        assert!(s.contains("[n]"), "key hint [n] missing:\n{s}");
        assert!(s.contains("[a]"), "key hint [a] missing:\n{s}");
        assert!(s.contains("session"), "session hint missing:\n{s}");
        assert!(s.contains("deny"), "deny hint missing:\n{s}");
        assert!(s.contains("cargo publish"), "buffer:\n{s}");
        assert!(s.contains("from subagent-2"), "origin line missing:\n{s}");
    }

    #[test]
    fn permission_modal_keys_visible_with_long_preview() {
        // Regression: tall body + wrap used to clip the key footer.
        let backend = TestBackend::new(60, 16);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        let preview = (0..20)
            .map(|i| format!("line {i} of a very long command preview"))
            .collect::<Vec<_>>()
            .join("\n");
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                PendingPermission {
                    name: "Bash".into(),
                    description: "Bash: run a long pipeline that wraps across many columns and rows"
                        .into(),
                    origin: None,
                    input_preview: Some(preview),
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("[y]"), "sticky footer [y] missing under tall body:\n{s}");
        assert!(s.contains("[n]"), "sticky footer [n] missing under tall body:\n{s}");
        assert!(
            s.contains("[Esc]") || s.contains("deny"),
            "deny/Esc hint missing under tall body:\n{s}"
        );
    }

    #[test]
    fn minimal_skin_drops_header_and_border() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("gpt-5.4", "/home/user/project", "abc12345");
        app.skin = crate::ui::modern::app::Skin::Minimal;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        // No branding header in minimal.
        assert!(!s.contains("agent-code"), "header should be hidden:\n{s}");
        // Prompt still present.
        assert!(s.contains('❯'), "prompt missing:\n{s}");
    }

    #[test]
    fn plan_and_question_modals_render() {
        // Plan modal.
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        app.modals.push_back(crate::ui::modern::app::Modal::Plan(
            crate::ui::modern::app::PlanReview {
                plan_md: "# Ship it\n\n- step one".into(),
                path: Some("/tmp/plans/ship.md".into()),
            },
        ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("plan · /tmp/plans/ship.md"), "plan title:\n{s}");
        assert!(s.contains("approve & start"), "plan buttons:\n{s}");

        // Question modal.
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        app.modals
            .push_back(crate::ui::modern::app::Modal::Question(
                crate::ui::modern::app::QuestionState {
                    questions: vec![crate::ui::modern::sink::UiQuestion {
                        question: "Which approach?".into(),
                        options: vec!["MVP first".into(), "Risk first".into()],
                    }],
                    current: 0,
                    cursor: 0,
                    answers: vec![],
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("Which approach?"), "question text:\n{s}");
        assert!(s.contains("MVP first"), "option text:\n{s}");
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
                        origin: None,
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
    fn tool_card_renders() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::Tool {
            call_id: String::new(),
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
    fn tasks_pane_renders_when_agents_present() {
        // Wide terminal → right-split pane.
        let backend = TestBackend::new(120, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(crate::ui::modern::sink::EngineEvent::SubagentUpdate {
            agent_id: "research-1".into(),
            state: "working".into(),
            headline: "scanning crates for StreamSink".into(),
        });
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("agents (1)"), "pane title missing:\n{s}");
        assert!(s.contains("working"), "state word missing:\n{s}");
        assert!(s.contains("scanning crates"), "headline missing:\n{s}");
    }

    #[test]
    fn context_meter_renders_with_percentage() {
        let backend = TestBackend::new(100, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.ctx_meter = Some((41, 100));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("ctx 41%"), "meter missing:\n{s}");
    }

    #[test]
    fn context_meter_red_at_high_usage() {
        // 95% → the "ctx 95%" cells should use the theme error color.
        let backend = TestBackend::new(100, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.ctx_meter = Some((95, 100));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let s = buffer_to_string(buf);
        assert!(s.contains("ctx 95%"), "buffer:\n{s}");
        let error = palette().error;
        let mut found = false;
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                let cell = &buf[(x, y)];
                if cell.symbol() == "c"
                    && x + 6 < buf.area().width
                    && buf[(x + 1, y)].symbol() == "t"
                    && buf[(x + 2, y)].symbol() == "x"
                    && cell.style().fg == Some(error)
                {
                    found = true;
                }
            }
        }
        assert!(found, "ctx meter should use theme error color at 95%");
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
