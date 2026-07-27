//! Ratatui drawing for the modern TUI.
//!
//! `draw` takes `&mut App` only so the virtualized [`super::layout::LayoutCache`]
//! can update during layout — the one mutation the view model permits. No I/O
//! happens here; used by both the live terminal and the `TestBackend` tests.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use super::anim::{blink_visible, pulse_style, spinner_glyph, toast_style};
use super::app::{App, PendingPermission, Phase};
use super::colors::palette;
use super::mode::SessionMode;
use crate::ui::text_safety::escape_deceptive;

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    app.frame_count = app.frame_count.wrapping_add(1);
    let area = frame.area();
    // Minimal skin (plan §M10) drops the header and the framed prompt for a
    // compact look — same block model, render config only.
    let minimal = app.skin == crate::ui::modern::app::Skin::Minimal;
    let header_h = if minimal { 0 } else { 3 };
    // Composer grows with content (capped); bordered fullscreen needs +2 for
    // the box, +1 for the mode/hint info line under the text.
    let prompt_h = prompt_area_height(app, minimal, area.height);
    // Queue: compact chips when non-empty, or a full pane when toggled open.
    let chips_h = if app.queue.is_empty() || app.show_queue_pane {
        0
    } else {
        1
    };
    let queue_pane_h = if app.show_queue_pane && !app.queue.is_empty() {
        (app.queue.len() as u16)
            .saturating_add(2)
            .clamp(3, 8)
            .min(area.height.saturating_sub(10).max(3))
    } else {
        0
    };
    // The search bar gets its own row so it never overdraws the composer
    // border (or transcript rows in minimal mode).
    let search_h = if app.search_open() && app.phase != Phase::Permission {
        1
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),     // header (0 in minimal)
            Constraint::Min(5),               // transcript
            Constraint::Length(1),            // status
            Constraint::Length(chips_h),      // queue chips
            Constraint::Length(queue_pane_h), // queue pane
            Constraint::Length(search_h),     // search bar
            Constraint::Length(prompt_h),     // input
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
            // Size the strip to what the grouped list actually renders
            // (headings + two lines per task) — a fixed five rows hid
            // the background group behind the agents group. Capped so
            // the transcript keeps at least half the area; the pane
            // shows a "+n more" line when it still cannot fit.
            // +1 for the block title row the pane spends.
            // Saturating, not `as u16`: a model-authored checklist has no
            // length limit, and a truncating cast would wrap a huge plan
            // round to a one-row request.
            let needed = u16::try_from(
                super::tasks::pane_rows_with_todos(&app.tasks, &app.todos, &app.collapsed_groups)
                    .saturating_add(1),
            )
            .unwrap_or(u16::MAX);
            let strip = needed
                .min(chunks[1].height / 2)
                .min(chunks[1].height.saturating_sub(3));
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
    if queue_pane_h > 0 {
        draw_queue_pane(frame, chunks[4], app);
    }
    if search_h > 0 {
        draw_search_bar(frame, chunks[5], app);
    }
    draw_input(frame, chunks[6], app);

    if app.phase == Phase::Permission
        && let Some(modal) = app.front_modal().cloned()
    {
        let behind = app.pending_modal_count();
        match modal {
            crate::ui::modern::app::Modal::Permission(p) => {
                draw_permission_modal(frame, area, &p, behind, app.perm_scroll)
            }
            crate::ui::modern::app::Modal::Plan(p) => draw_plan_modal(frame, area, &p, behind),
            crate::ui::modern::app::Modal::Question(q) => {
                draw_question_modal(frame, area, &q, behind)
            }
        }
    }

    // Palette / model picker / help never draw over HITL.
    if app.model_picker_open() && app.phase != Phase::Permission {
        draw_model_picker(frame, area, app);
    } else if app.theme_picker_open() && app.phase != Phase::Permission {
        draw_theme_picker(frame, area, app);
    } else if app.command_palette_open() && app.phase != Phase::Permission {
        draw_command_palette(frame, area, app);
    }

    if app.show_shortcuts && app.phase != Phase::Permission {
        draw_shortcuts_overlay(frame, area);
    }
}

fn draw_shortcuts_overlay(frame: &mut Frame<'_>, area: Rect) {
    let accent = palette().accent;
    let lines = vec![
        Line::from(Span::styled(
            " keyboard shortcuts ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  Enter           send · queue mid-turn · send next when idle"),
        Line::from("  Ctrl+Enter/I    interject (cancel + send now)"),
        Line::from("  Esc             never cancels · clear draft / dismiss modal"),
        Line::from("  Ctrl+C          cancel turn · double-press quit"),
        Line::from("  Shift+Tab       cycle mode Manual → Normal → AcceptEdits → Auto → Plan"),
        Line::from("  Ctrl+P / ?      command palette"),
        Line::from("  Ctrl+; / '      queue pane"),
        Line::from("  Ctrl+T          tasks pane"),
        Line::from("  Ctrl+M          model picker (scrollback) / multiline (prompt)"),
        Line::from("  /model /effort  switch model · set reasoning effort"),
        Line::from("  ↑/↓ empty       prompt history · scroll when drafting"),
        Line::from("  ←/→ empty       select transcript block"),
        Line::from("  e / Ctrl+E      fold block / expand all thinking"),
        Line::from("  y / Y           copy block body / metadata"),
        Line::from("  Ctrl+Shift+C    copy selection or last reply"),
        Line::from("  drag mouse      select transcript text · release keeps selection"),
        Line::from("  @path           mention a file · Tab completes · contents inlined"),
        Line::from("  !cmd            shell passthrough"),
        Line::from("  Ctrl+. / Ctrl+X this help"),
        Line::from(""),
        Line::from(Span::styled(
            "  Esc or Ctrl+. to close",
            Style::default().fg(palette().muted),
        )),
    ];
    draw_modal_box(
        frame,
        area,
        lines,
        " help ",
        accent,
        Some(key_hint_line("[Esc] close")),
    );
}

fn draw_command_palette(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let matches = app.palette_matches();
    let selected = app
        .command_palette
        .as_ref()
        .map(|p| p.selected)
        .unwrap_or(0);
    let query = app
        .command_palette
        .as_ref()
        .map(|p| p.query.as_str())
        .unwrap_or("");

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("/{}", query),
        Style::default()
            .fg(palette().accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    const MAX_ROWS: usize = 12;
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching commands",
            Style::default().fg(palette().muted),
        )));
    } else {
        let start = selected.saturating_sub(MAX_ROWS.saturating_sub(1).min(selected));
        let end = (start + MAX_ROWS).min(matches.len());
        for (i, (name, desc)) in matches.iter().enumerate().take(end).skip(start) {
            let is_sel = i == selected;
            let marker = if is_sel { "❯" } else { " " };
            let style = if is_sel {
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette().inactive)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} /{name}  "), style),
                Span::styled((*desc).to_string(), Style::default().fg(palette().muted)),
            ]));
        }
        if matches.len() > MAX_ROWS {
            lines.push(Line::from(Span::styled(
                format!("  … {} total", matches.len()),
                Style::default().fg(palette().muted),
            )));
        }
    }

    draw_modal_box(
        frame,
        area,
        lines,
        " commands ",
        palette().accent,
        Some(key_hint_line(
            "[↑↓] move   [Enter/Tab] select   [Esc] close   type to filter",
        )),
    );
}

/// One-line search bar in its own reserved row above the prompt, with
/// the match counter. A modal box would cover the transcript the user is
/// trying to look at, which defeats the purpose.
fn draw_search_bar(frame: &mut Frame<'_>, bar: Rect, app: &App) {
    let Some(s) = app.search.as_ref() else {
        return;
    };
    let (pos, total) = s.position();
    let counter = if s.query.is_empty() {
        String::new()
    } else if total == 0 {
        "  no matches".to_string()
    } else {
        format!("  {pos}/{total}")
    };
    let p = palette();
    let style = if total == 0 && !s.query.is_empty() {
        Style::default().fg(p.error)
    } else {
        Style::default().fg(p.accent)
    };
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let prefix = "  find: ";
    // Editing happens at the end of the query, so when it outgrows the
    // row show a horizontally scrolled tail with a leading ellipsis —
    // clipping the right edge would hide exactly the part being edited.
    let avail = (bar.width as usize).saturating_sub(prefix.len() + 1);
    let qw = s.query.as_str().width();
    let (shown, shown_w) = if qw <= avail {
        (s.query.clone(), qw)
    } else {
        let target = avail.saturating_sub(1);
        let mut w = 0usize;
        let mut kept: Vec<&str> = Vec::new();
        for g in s.query.as_str().graphemes(true).rev() {
            let gw = g.width().max(1);
            if w + gw > target {
                break;
            }
            w += gw;
            kept.push(g);
        }
        let tail: String = kept.iter().rev().copied().collect();
        (format!("…{tail}"), w + 1)
    };
    let line = Line::from(vec![
        Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
        Span::styled(shown, Style::default().fg(p.text)),
        Span::styled(counter, Style::default().fg(palette().muted)),
        Span::styled(
            "   [↓/↑] next/prev  [Enter] keep  [Esc] cancel",
            Style::default().fg(palette().muted),
        ),
    ]);
    frame.render_widget(Clear, bar);
    frame.render_widget(Paragraph::new(line), bar);
    // Typed and pasted input lands here, so the caret must too — the
    // composer suppresses its own while the bar is open.
    let x = bar
        .x
        .saturating_add(prefix.len() as u16)
        .saturating_add(shown_w as u16)
        .min(bar.x.saturating_add(bar.width.saturating_sub(1)));
    frame.set_cursor_position((x, bar.y));
}

fn draw_theme_picker(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(p) = app.theme_picker.as_ref() else {
        return;
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("filter: {}", p.query),
        Style::default()
            .fg(palette().accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("active: {}", p.original),
        Style::default().fg(palette().muted),
    )));
    lines.push(Line::from(""));

    let filtered = p.filtered();
    const MAX_ROWS: usize = 12;
    if filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching themes",
            Style::default().fg(palette().muted),
        )));
    } else {
        let start = p
            .selected
            .saturating_sub(MAX_ROWS.saturating_sub(1).min(p.selected));
        let end = (start + MAX_ROWS).min(filtered.len());
        for (i, (_, id, label)) in filtered.iter().enumerate().take(end).skip(start) {
            let is_sel = i == p.selected;
            let marker = if is_sel { "\u{276f}" } else { " " };
            let style = if is_sel {
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette().inactive)
            };
            let cur = if *id == p.original { " \u{2714}" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} {id}{cur}"), style),
                Span::styled(format!("  {label}"), Style::default().fg(palette().muted)),
            ]));
        }
        if filtered.len() > MAX_ROWS {
            lines.push(Line::from(Span::styled(
                format!("  \u{2026} {} more", filtered.len() - MAX_ROWS),
                Style::default().fg(palette().muted),
            )));
        }
    }
    draw_modal_box(
        frame,
        area,
        lines,
        " theme ",
        palette().accent,
        Some(key_hint_line(
            "[\u{2191}\u{2193}] preview   [Enter] keep   [Esc] revert",
        )),
    );
}

fn draw_model_picker(frame: &mut Frame<'_>, area: Rect, app: &App) {
    use crate::ui::modern::app::EFFORT_LEVELS;

    let Some(p) = app.model_picker.as_ref() else {
        return;
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    if p.effort_phase {
        let filtered = p.filtered();
        let model = filtered
            .get(p.selected)
            .map(|(_, id, _)| *id)
            .unwrap_or(p.current.as_str());
        lines.push(Line::from(Span::styled(
            format!("effort for {model}"),
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        for (i, level) in EFFORT_LEVELS.iter().enumerate() {
            let is_sel = i == p.effort_selected;
            let marker = if is_sel { "❯" } else { " " };
            let style = if is_sel {
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette().inactive)
            };
            let cur = if app.effort.as_deref() == Some(*level) {
                " ✔"
            } else {
                ""
            };
            lines.push(Line::from(Span::styled(
                format!("{marker} {level}{cur}"),
                style,
            )));
        }
        draw_modal_box(
            frame,
            area,
            lines,
            " reasoning effort ",
            palette().accent,
            Some(key_hint_line("[↑↓] move   [Enter] apply   [Esc/⌫] back")),
        );
        return;
    }

    lines.push(Line::from(Span::styled(
        format!("filter: {}", p.query),
        Style::default()
            .fg(palette().accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("current: {}", p.current),
        Style::default().fg(palette().muted),
    )));
    lines.push(Line::from(""));

    let filtered = p.filtered();
    const MAX_ROWS: usize = 12;
    if filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching models",
            Style::default().fg(palette().muted),
        )));
    } else {
        let start = p
            .selected
            .saturating_sub(MAX_ROWS.saturating_sub(1).min(p.selected));
        let end = (start + MAX_ROWS).min(filtered.len());
        for (i, (_, id, desc)) in filtered.iter().enumerate().take(end).skip(start) {
            let is_sel = i == p.selected;
            let marker = if is_sel { "❯" } else { " " };
            let style = if is_sel {
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette().inactive)
            };
            let cur = if *id == p.current { " ✔" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} {id}{cur}  "), style),
                Span::styled((*desc).to_string(), Style::default().fg(palette().muted)),
            ]));
        }
        if filtered.len() > MAX_ROWS {
            lines.push(Line::from(Span::styled(
                format!("  … {} total", filtered.len()),
                Style::default().fg(palette().muted),
            )));
        }
    }

    draw_modal_box(
        frame,
        area,
        lines,
        " models ",
        palette().accent,
        Some(key_hint_line(
            "[↑↓] move   [Enter] select   [Tab] effort   [Esc] close",
        )),
    );
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
            Style::default().fg(palette().muted),
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
    _pending_behind: usize,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if q.questions.len() > 1 {
        lines.push(Line::from(Span::styled(
            format!("question {} of {}", q.current + 1, q.questions.len()),
            Style::default().fg(palette().muted),
        )));
    }
    let cur = &q.questions[q.current];
    lines.push(Line::from(Span::styled(
        cur.question.clone(),
        Style::default()
            .fg(palette().text)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for (i, opt) in cur.options.iter().enumerate() {
        let selected = i == q.cursor;
        let marker = if selected { "❯" } else { " " };
        let accent = palette().accent;
        let style = if selected {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette().inactive)
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
        Style::default().fg(warning).add_modifier(Modifier::BOLD),
    ))
}

/// The plain text of a line, for measuring how tall it will render.
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Rows `text` occupies when word-wrapped into `width` columns, matching
/// `Paragraph`'s greedy wrap: words move to the next row whole, and a
/// word wider than the line is split across rows.
///
/// Measured in display columns, not characters. A CJK glyph or emoji
/// occupies two cells, so counting characters underestimates the rows a
/// line needs and the box would be sized too short — clipping the very
/// text the user is answering about.
fn wrapped_rows(text: &str, width: u16) -> u16 {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if width == 0 {
        return 1;
    }
    let width = width as usize;
    let mut rows = 1usize;
    let mut col = 0usize;
    for word in text.split_whitespace() {
        let w = UnicodeWidthStr::width(word);
        if col > 0 && col + 1 + w > width {
            rows += 1;
            col = 0;
        }
        if w > width {
            // Split by columns, not by characters: a double-width glyph
            // straddling the edge moves to the next row whole.
            // The check above already emptied a partly-filled row.
            for ch in word.chars() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if col + cw > width {
                    rows += 1;
                    col = 0;
                }
                col += cw;
            }
        } else {
            col = if col == 0 { w } else { col + 1 + w };
        }
    }
    rows.try_into().unwrap_or(u16::MAX)
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
    // The footer gets the rows its hints actually need. It used to be a
    // single row whatever it held, so on a narrow terminal ratatui
    // clipped the tail — and the tail is `[n] deny`, the one binding
    // that must never be the one to disappear while its key stays live.
    let footer_h: u16 = footer
        .as_ref()
        .map(|l| wrapped_rows(&line_text(l), width.saturating_sub(2)))
        .unwrap_or(0);
    // Size the body from the rows the text will actually occupy once
    // wrapped, not from the number of `Line`s. A single long line —
    // the permission modal's durable-grant row is the one that can run
    // long — otherwise claimed one row and was clipped after the modal
    // had already been sized, hiding content the user is answering
    // about.
    let body_rows: u16 = lines
        .iter()
        .map(|l| wrapped_rows(&line_text(l), width.saturating_sub(2)))
        .fold(0u16, |a, b| a.saturating_add(b));
    // +2 border, +footer, +1 breathing room for wrap
    let wanted = body_rows
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
        let foot_h = footer_h.min(inner.height);
        let body_h = inner.height.saturating_sub(foot_h);
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
            height: foot_h,
        };
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
        // Sticky footer: key hints always land on the last inner rows,
        // wrapping within them rather than being cut off.
        frame.render_widget(Paragraph::new(footer_line).wrap(Wrap { trim: false }), foot);
    } else {
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

/// Queue chips row: `⧉ queued: ❶ "…" ❷ "…"` above the prompt (plan §M5).
fn draw_queue_chips(frame: &mut Frame<'_>, area: Rect, app: &App) {
    const CIRCLED: [&str; 9] = ["❶", "❷", "❸", "❹", "❺", "❻", "❼", "❽", "❾"];
    let accent = palette().accent;
    let mut spans = vec![Span::styled(
        "⧉ queued: ",
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    )];
    for (i, p) in app.queue.iter().enumerate().take(CIRCLED.len()) {
        let mark = CIRCLED[i];
        let text: String = p.chars().take(40).collect();
        let ellipsis = if p.chars().count() > 40 { "…" } else { "" };
        spans.push(Span::styled(
            format!("{mark} \"{text}{ellipsis}\"  "),
            Style::default().fg(palette().inactive),
        ));
    }
    spans.push(Span::styled(
        " Ctrl+; pane",
        Style::default().fg(palette().muted),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Full queue pane: selectable rows, Enter send-now, Backspace delete.
fn draw_queue_pane(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let accent = palette().accent;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(
            format!(" queue · {} ", app.queue.len()),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, p) in app.queue.iter().enumerate() {
        let selected = i == app.queue_selected;
        let mark = if selected { "▸ " } else { "  " };
        let preview: String = p
            .chars()
            .take(inner.width.saturating_sub(4) as usize)
            .collect();
        let style = if selected {
            // Underline + accent fg — calmer than inverted brand fill.
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(palette().inactive)
        };
        lines.push(Line::from(Span::styled(
            format!("{mark}{}. {preview}", i + 1),
            style,
        )));
    }
    if lines.len() < inner.height as usize {
        lines.push(Line::from(Span::styled(
            " ↑/↓ select · Enter send-now · Backspace drop · Ctrl+; close",
            Style::default().fg(palette().muted),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// One selectable pane row, tied back to the task list it came from.
///
/// `task` is an index into `app.tasks` (what the selection stores), not
/// a position among rendered rows — folding a group must not shift it.
struct RowAnchor {
    task: usize,
    /// First line of this row: the one carrying the `❯` marker. Rows are
    /// not a uniform height — a task is a status line plus a headline, a
    /// folded heading is a single line — so this is recorded rather than
    /// derived from `end`.
    start: usize,
    /// Line index this row's rendering ends on.
    end: usize,
    /// Tasks this row accounts for: 1 normally, the group size for a
    /// folded heading.
    covers: usize,
}

/// Tasks/agents pane: state-ordered subagent rows (plan §M8).
fn draw_tasks_pane(frame: &mut Frame<'_>, area: Rect, app: &App) {
    use super::tasks::TaskState;
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(palette().muted))
        .title(format!(" agents ({}) ", app.tasks.len()));
    // The title consumes the top row even without a top border, so
    // measure the real content box instead of the outer area.
    let inner = block.inner(area);
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Where each pane row ends, keyed by the task index it belongs to —
    // the selection is an index into `app.tasks`, so a folded group
    // ahead of it must not shift the lookup. Used by the overflow
    // windowing below.
    let mut anchors: Vec<RowAnchor> = Vec::new();
    let inner_w = (inner.width as usize).saturating_sub(1);
    let max_rows = inner.height as usize;
    // Checklist first: it is what the model says it is doing, which
    // frames everything below it. The list is model-authored and can be
    // any length, so cap the rows it may claim — all of the strip when
    // it is the only thing here, at most half once tasks compete for the
    // space — and window the items inside that cap. Bounding it here is
    // what keeps the task rows and the overflow footer on screen.
    if !app.todos.is_empty() {
        let (done, total) = super::tasks::todo_progress(&app.todos);
        // Rows the whole plan block may occupy, chrome included.
        let allowance = if app.tasks.is_empty() {
            max_rows
        } else {
            max_rows / 2
        };
        // Chrome: the heading, plus the blank separator when tasks
        // follow. Counting the separator here is what keeps the block
        // inside its allowance on a short pane.
        let separator = usize::from(!app.tasks.is_empty());
        let mut budget = allowance.saturating_sub(1 + separator);
        // One more row goes to the "+n more" footer when items are elided.
        if app.todos.len() > budget {
            budget = budget.saturating_sub(1);
        }
        let (start, shown) = super::tasks::todo_window(&app.todos, budget);
        let block_start = lines.len();
        lines.push(Line::from(Span::styled(
            format!("plan  {done}/{total}"),
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        )));
        for todo in &app.todos[start..start + shown] {
            let p = palette();
            let (color, modifier) = match todo.status {
                super::tasks::TodoStatus::Done => (p.success, Modifier::DIM),
                super::tasks::TodoStatus::InProgress => (p.accent, Modifier::BOLD),
                super::tasks::TodoStatus::Pending => (p.inactive, Modifier::empty()),
            };
            // Model-authored text on its way to the terminal: scrub bidi
            // overrides and zero-width characters like every other
            // surface that renders untrusted content, before truncating
            // so the escape markers are what gets measured. Line breaks
            // collapse to spaces because one item must stay one row —
            // the height budget above is counted in items.
            let text: String = escape_deceptive(&todo.content)
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(inner_w.saturating_sub(4).max(4))
                .collect();
            lines.push(Line::from(Span::styled(
                format!("  {} {text}", todo.status.glyph()),
                Style::default().fg(color).add_modifier(modifier),
            )));
        }
        let hidden = app.todos.len() - shown;
        if hidden > 0 {
            lines.push(Line::from(Span::styled(
                format!("  … +{hidden} more"),
                Style::default().fg(palette().muted),
            )));
        }
        if separator == 1 {
            lines.push(Line::from(""));
        }
        // Backstop: on a pane too short for even the chrome, the block is
        // trimmed to its allowance so it cannot crowd out the task rows.
        // Ordering puts the separator last, so it is the first thing lost.
        lines.truncate(block_start + allowance);
    }

    let mut last_source: Option<super::tasks::TaskSource> = None;
    for (idx, t) in app.tasks.iter().enumerate() {
        let group_folded = app.collapsed_groups.contains(&t.source);
        let p = palette();
        let selected = idx == app.tasks_selected;
        // Group header when the source changes. Subagents and background
        // jobs arrive from different places but read as one list, so they
        // share the pane with a heading to tell them apart.
        if last_source != Some(t.source) {
            if last_source.is_some() {
                lines.push(Line::from(""));
            }
            let count = app.tasks.iter().filter(|x| x.source == t.source).count();
            // A folded group shows its size, so collapsing does not hide
            // how much is behind it.
            let heading = if group_folded {
                format!("{} {} ({count})", "▸", t.source.heading())
            } else {
                format!("{} {}", "▾", t.source.heading())
            };
            // A folded group is selected through its heading, so the
            // marker has to appear there or the pane looks unselected.
            lines.push(Line::from(vec![
                Span::styled(
                    if group_folded && selected { "❯" } else { " " }.to_string(),
                    Style::default().fg(p.accent),
                ),
                Span::styled(
                    heading,
                    Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
                ),
            ]));
            last_source = Some(t.source);
            if group_folded {
                // The heading is the whole group's one row on screen, so
                // it accounts for every task behind it.
                anchors.push(RowAnchor {
                    task: idx,
                    start: lines.len() - 1,
                    end: lines.len() - 1,
                    covers: count,
                });
            }
        }
        // Folded: the heading above stands in for the group's rows.
        if group_folded {
            continue;
        }
        let color = match t.state {
            TaskState::Working => palette().accent,
            TaskState::NeedsInput => p.warning,
            TaskState::Done => p.success,
            TaskState::Failed => p.error,
        };
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "❯" } else { " " }.to_string(),
                Style::default().fg(p.accent),
            ),
            Span::styled(format!("{} ", t.state.glyph()), Style::default().fg(color)),
            Span::styled(
                format!("{} ", t.state.word()),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        // Headline on its own row, truncated to the pane width. The
        // text is model- or tool-supplied: scrub bidi overrides and
        // zero-width characters like every other surface that shows it.
        let head: String = crate::ui::text_safety::escape_deceptive(&t.headline)
            .chars()
            .take(inner_w.max(4))
            .collect();
        lines.push(Line::from(Span::styled(
            format!("  {head}"),
            Style::default().fg(palette().inactive),
        )));
        anchors.push(RowAnchor {
            task: idx,
            // Status line: pushed just above the headline.
            start: lines.len() - 2,
            end: lines.len() - 1,
            covers: 1,
        });
    }
    // When the area is still too short (many tasks, tiny terminal),
    // window the rows around the selection — Up/Down cycles the whole
    // list, so the marked task must stay on screen — and say how many
    // tasks are hidden rather than silently truncating mid-task. The
    // checklist above is already bounded and carries its own count, so
    // this footer speaks only for the rows the arrows navigate.
    if lines.len() > max_rows && max_rows > 0 {
        let sel = anchors
            .iter()
            .find(|a| a.task == app.tasks_selected)
            .or_else(|| {
                // Selection inside a folded group but not on its first
                // row: the nearest anchor at or before it is that
                // group's heading.
                anchors.iter().rev().find(|a| a.task <= app.tasks_selected)
            });
        let sel_end = sel.map(|a| a.end).unwrap_or(0);
        // The row carrying the ❯ marker: the status line for a task, the
        // heading itself for a folded group. When space is too tight for
        // the whole row, this is the line that must survive — deriving it
        // as `sel_end - 1` would step off a one-line folded heading onto
        // the blank separator above it and hide the selection.
        let sel_start = sel.map(|a| a.start).unwrap_or(0);
        if max_rows == 1 {
            let row = lines
                .into_iter()
                .nth(sel_start)
                .unwrap_or_else(|| Line::from("…"));
            lines = vec![row];
        } else {
            let total = lines.len();
            let window = |visible_h: usize| {
                let anchor = if visible_h >= 2 { sel_end } else { sel_start };
                let offset = anchor.saturating_sub(visible_h - 1).min(total - visible_h);
                // A folded heading that survives the window accounts for
                // its whole group: the user can read its "(n)", so those
                // rows are not part of the "+n more" that scrolled away.
                let shown: usize = anchors
                    .iter()
                    .filter(|a| a.end >= offset && a.end < offset + visible_h)
                    .map(|a| a.covers)
                    .sum();
                (offset, app.tasks.len().saturating_sub(shown))
            };
            // The footer costs a row, so it has to earn it. When every
            // task is already on screen there is nothing to announce —
            // the rows lost are checklist chrome, which carries its own
            // count — and a "+0 more (↑/↓)" would be an invitation to
            // press arrows that reveal nothing.
            let (offset, hidden) = window(max_rows - 1);
            if hidden == 0 {
                let (offset, _) = window(max_rows);
                lines = lines.into_iter().skip(offset).take(max_rows).collect();
            } else {
                lines = lines.into_iter().skip(offset).take(max_rows - 1).collect();
                lines.push(Line::from(Span::styled(
                    format!("… +{hidden} more (↑/↓)"),
                    Style::default().fg(palette().muted),
                )));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_permission_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    pending: &PendingPermission,
    pending_behind: usize,
    scroll: usize,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    // The durable grant leads the body, not the footer. The footer is a
    // single fixed row that cannot wrap, so a narrow terminal truncated
    // the tail of the prefix while `[P]` stayed live — a durable grant
    // whose full scope the user could not read. Here it wraps, and being
    // the first row it survives however short the modal gets. Escaped
    // like every other untrusted string in this modal: the prefix is
    // derived from a model-supplied command, and a bidi or zero-width
    // control in it would make the grant read as something other than
    // the bytes to be persisted.
    if let Some(ref prefix) = pending.suggested_prefix {
        lines.push(Line::from(Span::styled(
            format!("[P] always allow: {}", escape_deceptive(prefix)),
            Style::default()
                .fg(palette().warning)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    if pending_behind > 0 {
        lines.push(Line::from(Span::styled(
            format!("⚠ {pending_behind} more pending"),
            Style::default()
                .fg(palette().warning)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    // Everything below is model- or tool-supplied text on the surface
    // where the user authorizes execution. Bidi overrides and zero-width
    // characters would let the rendering disagree with the bytes that
    // actually run, so they are made visible before display.
    lines.push(Line::from(Span::styled(
        escape_deceptive(&pending.description).into_owned(),
        Style::default().fg(palette().text),
    )));
    if let Some(ref origin) = pending.origin {
        lines.push(Line::from(Span::styled(
            format!("from {}", escape_deceptive(origin)),
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    if let Some(ref preview) = pending.input_preview {
        lines.push(Line::from(""));
        // Adaptive viewport over the FULL input: tall terminals show
        // more rows; ↑/↓ (and PgUp/PgDn) pan the rest so a 200-line
        // tool input can be inspected before answering. The floor
        // keeps the description readable on small terminals.
        let viewport = (area.height as usize).saturating_sub(12).clamp(8, 30);
        let rows: Vec<&str> = preview.lines().collect();
        let total = rows.len();
        let scroll = scroll.min(total.saturating_sub(viewport));
        if scroll > 0 {
            lines.push(Line::from(Span::styled(
                format!("… {scroll} earlier lines (↑ to scroll)"),
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
        for row in rows.iter().skip(scroll).take(viewport) {
            lines.push(Line::from(Span::styled(
                escape_deceptive(row).into_owned(),
                Style::default().fg(palette().muted),
            )));
        }
        let below = total.saturating_sub(scroll + viewport);
        if below > 0 {
            lines.push(Line::from(Span::styled(
                format!("… {below} more lines (↓ to scroll)"),
                Style::default()
                    .fg(palette().muted)
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
        // The name can come from a plugin manifest or executable filename,
        // so it is untrusted like the rest of the modal text.
        &format!(" permission · {} ", escape_deceptive(&pending.name)),
        accent,
        // Keep ≤ 40 cols so min-width modals still show every binding —
        // the deny action must never be the one that gets clipped.
        // Esc denies too, and digits 1/2/4 mirror y/a/A; both in /help.
        // The prefix itself is NOT named here: it is variable-length, and
        // this row cannot wrap. The body row above carries the scope.
        Some(key_hint_line(if pending.suggested_prefix.is_some() {
            "[y] once [a] session [A] always [P] prefix [n] deny"
        } else {
            "[y] once [a] session [A] always [n] deny"
        })),
    );
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let accent = palette().accent;
    let mode_style = mode_style(app.mode);
    let title = Line::from(vec![
        Span::styled(
            "agent-code",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(&app.version, Style::default().fg(palette().muted)),
        Span::raw("  "),
        Span::styled(&app.model, Style::default().fg(palette().inactive)),
        if let Some(ref e) = app.effort {
            Span::styled(format!(" ·{e}"), Style::default().fg(palette().muted))
        } else {
            Span::raw("")
        },
        Span::raw("  "),
        Span::styled(format!(" {} ", app.mode.short_badge()), mode_style),
        Span::raw("  "),
        Span::styled(
            truncate_path(&app.cwd, area.width.saturating_sub(40) as usize),
            Style::default().fg(palette().muted),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(palette().muted));
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
    app.layout.sync(
        &app.transcript,
        inner.width,
        &app.expanded,
        app.selected_item,
    );
    app.viewport_h = height;
    // The transcript may have grown since the query was typed, so the
    // line indices behind the match list can go stale. Recompute without
    // resetting the selection, so streaming output does not yank the
    // reader off the match they are on. Gated on the layout revision:
    // spinner frames repaint every ~80ms and an O(transcript) rescan per
    // frame would make long sessions crawl.
    if app.search_open() && app.search.as_ref().map(|s| s.layout_rev) != Some(app.layout.revision())
    {
        app.recompute_search(false);
    }
    // Record the bottom screen row for mouse hit-testing (jump pill).
    app.transcript_bottom_row = inner.y + inner.height.saturating_sub(1);
    let total = app.layout.total_lines();
    let top = app.scroll.top(total, height);
    let view = app.layout.viewport(top, height);

    // Apply selection highlight on the visible slice.
    let view = apply_selection_highlight(view, top, app.selection);
    let view = apply_search_highlight(view, top, app);

    let title = match app.phase {
        Phase::Streaming => {
            let f = spinner_glyph(app.tick);
            format!(" {f} streaming ")
        }
        Phase::Permission => {
            if blink_visible(app.tick, app.terminal_focused) {
                format!(" {} action required ", spinner_glyph(app.tick))
            } else {
                "  action required ".into()
            }
        }
        _ => " transcript ".into(),
    };
    let title_style = match app.phase {
        Phase::Streaming => pulse_style(app.tick, palette().accent),
        Phase::Permission => Style::default()
            .fg(palette().warning)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(palette().muted),
    };
    let title_block = Block::default()
        .borders(Borders::NONE)
        .title(Span::styled(title, title_style));
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
                .fg(accent)
                .add_modifier(Modifier::BOLD | Modifier::DIM),
        ))),
        rect,
    );
}

fn apply_selection_highlight(
    mut view: Vec<Line<'static>>,
    top: usize,
    selection: Option<super::app::TextSelection>,
) -> Vec<Line<'static>> {
    let Some(sel) = selection else {
        return view;
    };
    let lo = sel.start_line.min(sel.end_line);
    let hi = sel.start_line.max(sel.end_line);
    let accent = palette().accent;
    for (i, line) in view.iter_mut().enumerate() {
        let abs = top + i;
        if abs >= lo && abs <= hi {
            let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            *line = Line::from(Span::styled(
                plain,
                Style::default()
                    .fg(super::colors::on_fill(accent))
                    .bg(accent)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    view
}

/// Paint the row the `2/3` counter points at, so stepping through
/// matches is visibly anchored. Filled with the warning colour to stay
/// distinct from the accent-filled mouse selection.
fn apply_search_highlight(
    mut view: Vec<Line<'static>>,
    top: usize,
    app: &App,
) -> Vec<Line<'static>> {
    if app.phase == Phase::Permission {
        return view;
    }
    let Some(cur) = app.search.as_ref().and_then(|s| s.current_line()) else {
        return view;
    };
    let Some(rel) = cur.checked_sub(top) else {
        return view;
    };
    if let Some(line) = view.get_mut(rel) {
        let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        *line = Line::from(Span::styled(
            plain,
            Style::default()
                .fg(super::colors::on_fill(palette().warning))
                .bg(palette().warning)
                .add_modifier(Modifier::BOLD),
        ));
    }
    view
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let tokens = app.tokens_in + app.tokens_out;
    let mut spans = Vec::new();
    // The mode badge must be visible in EVERY state (product bar). The
    // minimal skin hides the header that normally carries it, so show it
    // in the status bar there — permission behavior differs radically
    // between Manual/AcceptEdits/Plan and must never be invisible.
    if matches!(app.skin, super::app::Skin::Minimal) {
        spans.push(Span::styled(
            format!(" {} ", app.mode.short_badge()),
            mode_style(app.mode),
        ));
        spans.push(Span::raw("│"));
    }
    spans.extend([
        Span::styled(
            format!(" turn {} ", app.turn_count),
            Style::default().fg(palette().muted),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" {tokens} tok "),
            Style::default().fg(palette().muted),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" ${:.4} ", app.cost_usd),
            Style::default().fg(palette().muted),
        ),
        Span::raw("│"),
    ]);

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
            palette().muted
        };
        spans.push(Span::styled(
            format!(" ctx {pct}% "),
            Style::default().fg(color),
        ));
        spans.push(Span::raw("│"));
    }

    // Live spinner / blinking action-required / toast / idle message.
    let accent = palette().accent;
    let warning = palette().warning;
    match app.phase {
        Phase::Streaming => {
            let glyph = spinner_glyph(app.tick);
            let (glyph_color, text_color) = match app.waiting_on {
                super::app::WaitingOn::UserInput => (warning, warning),
                _ => (accent, palette().inactive),
            };
            spans.push(Span::styled(
                format!(" {glyph} "),
                pulse_style(app.tick, glyph_color),
            ));
            spans.push(Span::styled(
                format!(
                    "{} ",
                    app.waiting_on.label_with_elapsed(app.thinking_started_at)
                ),
                Style::default().fg(text_color),
            ));
        }
        Phase::Permission => {
            let show = blink_visible(app.tick, app.terminal_focused);
            if show {
                spans.push(Span::styled(
                    format!(" {} ", spinner_glyph(app.tick)),
                    Style::default().fg(warning).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    " action required ",
                    Style::default()
                        .fg(super::colors::on_fill(warning))
                        .bg(warning)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    "  · waiting for you ·  ",
                    Style::default().fg(warning).add_modifier(Modifier::DIM),
                ));
            }
        }
        _ => {
            if let Some((ref msg, left)) = app.toast {
                spans.push(Span::styled(format!(" {msg} "), toast_style(left)));
            } else {
                spans.push(Span::styled(
                    format!(" {} ", app.status_message),
                    Style::default().fg(palette().inactive),
                ));
            }
        }
    }
    if !app.queue.is_empty() {
        spans.push(Span::raw("│"));
        let q_style = if app.phase == Phase::Streaming {
            pulse_style(app.tick, accent)
        } else {
            Style::default().fg(accent)
        };
        spans.push(Span::styled(
            format!(" ⧉ {} queued ", app.queue.len()),
            q_style,
        ));
    }
    if app.selection.is_some() {
        spans.push(Span::raw("│"));
        spans.push(Span::styled(
            " sel · Ctrl+Shift+C copy ",
            Style::default().fg(accent).add_modifier(Modifier::DIM),
        ));
    }
    spans.push(Span::raw("│"));
    spans.push(Span::styled(
        format!(" sid {} ", truncate_mid(&app.session_id, 12)),
        Style::default().fg(palette().muted),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Height of the prompt region given content and skin.
fn prompt_area_height(app: &App, minimal: bool, total_h: u16) -> u16 {
    let lines = app.input_line_count() as u16;
    // Cap growth so the transcript keeps room; leave at least 8 rows above.
    let max_body = total_h
        .saturating_sub(header_and_chrome_reserve(minimal))
        .min(10);
    let body = lines.clamp(1, max_body.max(1));
    if minimal {
        body
    } else {
        // borders (2) + body + info line (1)
        body.saturating_add(3).min(total_h.saturating_sub(6).max(3))
    }
}

fn header_and_chrome_reserve(minimal: bool) -> u16 {
    // header + status + min transcript
    if minimal { 1 + 1 + 8 } else { 3 + 1 + 8 }
}

fn draw_input(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let p = palette();
    let border = if app.phase == Phase::Streaming {
        p.warning
    } else {
        p.accent
    };
    let body_style = Style::default().fg(p.text);
    let prefix_style = Style::default().fg(border).add_modifier(Modifier::BOLD);
    // Vi is modal, so the mode has to be visible: an invisible mode is
    // how you delete a line you meant to type.
    let prompt_marker = if app.in_normal_mode() { "▪ " } else { "❯ " };

    // Build per-line display with ❯ only on the first row.
    let input_lines: Vec<&str> = if app.input.is_empty() {
        vec![""]
    } else {
        // Keep trailing empty line when the draft ends with \n.
        let mut v: Vec<&str> = app.input.split('\n').collect();
        if app.input.ends_with('\n') {
            // split already yields trailing "" for trailing newline
        }
        if v.is_empty() {
            v.push("");
        }
        v
    };

    let mut display_lines: Vec<Line<'static>> = Vec::with_capacity(input_lines.len());
    for (i, segment) in input_lines.iter().enumerate() {
        let mut spans = Vec::new();
        if i == 0 {
            spans.push(Span::styled(prompt_marker.to_string(), prefix_style));
        } else {
            spans.push(Span::raw("  ".to_string())); // indent continuation
        }
        spans.push(Span::styled((*segment).to_string(), body_style));
        display_lines.push(Line::from(spans));
    }

    if app.skin == crate::ui::modern::app::Skin::Minimal {
        frame.render_widget(Paragraph::new(display_lines), area);
        set_prompt_cursor(frame, area, app, /*bordered*/ false);
        return;
    }

    let title = if app.phase == Phase::Streaming {
        " composer · queued until turn ends "
    } else if app.multiline_mode {
        " composer · multiline "
    } else {
        " composer "
    };
    let hint = if app.multiline_mode {
        "Enter newline · Alt/Shift+Enter send · Ctrl+Enter interject · Shift+Tab mode · Ctrl+M"
    } else {
        "Enter send · Alt/Shift+Enter newline · Ctrl+Enter interject · Shift+Tab mode · Ctrl+M"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            title,
            Style::default().fg(border).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Body + one-row hint footer inside the box.
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let body_h = inner.height.saturating_sub(1).max(1);
    let body_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: body_h,
    };
    let hint_area = Rect {
        x: inner.x,
        y: inner.y + body_h,
        width: inner.width,
        height: if inner.height > 1 { 1 } else { 0 },
    };
    frame.render_widget(
        Paragraph::new(display_lines).wrap(Wrap { trim: false }),
        body_area,
    );
    if hint_area.height > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_mid(hint, hint_area.width as usize),
                Style::default().fg(palette().muted),
            ))),
            hint_area,
        );
    }
    set_prompt_cursor(frame, body_area, app, /*bordered*/ true);
}

/// Place the terminal cursor on the composer caret.
fn set_prompt_cursor(frame: &mut Frame<'_>, body_area: Rect, app: &App, _bordered: bool) {
    if body_area.width == 0 || body_area.height == 0 {
        return;
    }
    // While the search bar is open it owns typed input, so it owns the
    // caret too (`draw_search_bar` places it after the query).
    if app.search_open() && app.phase != Phase::Permission {
        return;
    }
    let (line, col) = app.cursor_line_col();
    // Prefix "❯ " is 2 columns on line 0; continuation lines are indented 2.
    let prefix_cols: u16 = 2;
    let x = body_area
        .x
        .saturating_add(prefix_cols)
        .saturating_add(col as u16)
        .min(
            body_area
                .x
                .saturating_add(body_area.width.saturating_sub(1)),
        );
    let y = body_area.y.saturating_add(line as u16).min(
        body_area
            .y
            .saturating_add(body_area.height.saturating_sub(1)),
    );
    frame.set_cursor_position((x, y));
}

fn mode_style(mode: SessionMode) -> Style {
    let p = palette();
    // Text-only badges — no filled color blocks (keeps chrome minimal).
    let fg = match mode {
        SessionMode::Manual => p.warning,
        SessionMode::Normal => p.success,
        SessionMode::AcceptEdits => p.tool,
        SessionMode::Auto => p.accent,
        SessionMode::Plan => p.plan,
    };
    Style::default().fg(fg).add_modifier(Modifier::BOLD)
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
    // The crate root allows dead code for its public API surface, which
    // also silences a test that loses its `#[test]` — exactly what a
    // merge did to `normal_mode_is_visible_in_the_prompt` here. Opt this
    // module back in: an unannotated test is unreachable, so the
    // compiler is the thing that should notice.
    #![deny(dead_code)]

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

    /// The approval modal is where the user authorizes execution, so what
    /// it paints has to match the bytes that will run. A bidi override in
    /// the command would otherwise render as a different command entirely.
    #[test]
    fn permission_modal_reveals_a_bidi_override_in_the_command() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        // Displays as `rm -rf /tmp/safe # apply patch` in a terminal that
        // honours the override.
        let attack = "rm -rf /tmp/safe \u{202e}# hctap ylppa\u{202c}";
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                PendingPermission {
                    name: "Bash".into(),
                    description: format!("Bash: run `{attack}`"),
                    origin: None,
                    input_preview: Some(format!("{{\n  \"command\": \"{attack}\"\n}}")),
                    suggested_prefix: None,
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(
            !s.contains('\u{202e}'),
            "a bidi override reached the screen:\n{s}"
        );
        assert!(s.contains("<U+202E>"), "override not surfaced:\n{s}");
    }

    /// `[P]` approves a *durable* grant, so the prefix beside it has to
    /// read as the bytes that will be persisted and later authorized. The
    /// prefix is derived from the model-supplied command, so a bidi
    /// override in it would make the footer advertise a different grant
    /// from the one being made.
    #[test]
    fn permission_modal_reveals_a_bidi_override_in_the_offered_prefix() {
        let backend = TestBackend::new(100, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                PendingPermission {
                    name: "Bash".into(),
                    description: "Bash: run a command".into(),
                    origin: None,
                    input_preview: None,
                    suggested_prefix: Some("git\u{202e}sutats".into()),
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(
            !s.contains('\u{202e}'),
            "a bidi override in the offered prefix reached the screen:\n{s}"
        );
        assert!(
            s.contains("git<U+202E>sutats"),
            "override in the offered prefix not surfaced:\n{s}"
        );
    }

    /// `[P]` creates a *durable* grant, so its full scope has to be
    /// readable before the key is live. The hint used to live in the
    /// one-row footer, which cannot wrap: at 60 columns the prefix tail
    /// was truncated while `[P]` still worked.
    #[test]
    fn permission_modal_shows_the_whole_prefix_on_a_narrow_terminal() {
        // Long enough that no single row of a narrow modal could hold it.
        let prefix = "/usr/local/bin/kubectl rollout-status-with-a-long-name";
        for cols in [46u16, 60, 80, 120] {
            let backend = TestBackend::new(cols, 24);
            let mut term = Terminal::new(backend).unwrap();
            let mut app = App::new("m", "/tmp", "s");
            app.phase = Phase::Permission;
            let (respond, _rx) = std::sync::mpsc::channel();
            app.modals
                .push_back(crate::ui::modern::app::Modal::Permission(
                    PendingPermission {
                        name: "Bash".into(),
                        description: "Bash: run a command".into(),
                        origin: None,
                        input_preview: None,
                        suggested_prefix: Some(prefix.into()),
                        respond,
                    },
                ));
            term.draw(|f| draw(f, &mut app)).unwrap();
            let s = buffer_to_string(term.backend().buffer());
            // The body wraps, so the prefix may be split across rows —
            // possibly mid-word. Dropping borders and every space
            // reassembles it either way.
            let flat = squeeze(&s);
            assert!(
                flat.contains(&squeeze(prefix)),
                "the prefix was not fully visible at {cols} columns:\n{s}"
            );
            assert!(
                s.contains("[P]"),
                "the prefix binding vanished at {cols} columns:\n{s}"
            );
            // Every binding stays readable. `[n] deny` is the one that
            // used to fall off the end of a one-row footer while its key
            // stayed live.
            for hint in ["[y]", "[a]", "[A]", "[P]", "[n]deny"] {
                assert!(
                    flat.contains(hint),
                    "binding {hint} was clipped at {cols} columns:\n{s}"
                );
            }
        }
    }

    /// The deny binding must survive the narrowest supported modal even
    /// when no prefix is offered — the plain footer is 40 columns and the
    /// inner width at a 46-column terminal is 38.
    #[test]
    fn permission_modal_keeps_every_binding_at_the_narrowest_width() {
        for prefix in [None, Some("git status".to_string())] {
            let backend = TestBackend::new(46, 24);
            let mut term = Terminal::new(backend).unwrap();
            let mut app = App::new("m", "/tmp", "s");
            app.phase = Phase::Permission;
            let (respond, _rx) = std::sync::mpsc::channel();
            app.modals
                .push_back(crate::ui::modern::app::Modal::Permission(
                    PendingPermission {
                        name: "Bash".into(),
                        description: "Bash: run a command".into(),
                        origin: None,
                        input_preview: None,
                        suggested_prefix: prefix.clone(),
                        respond,
                    },
                ));
            term.draw(|f| draw(f, &mut app)).unwrap();
            let s = buffer_to_string(term.backend().buffer());
            let flat = squeeze(&s);
            for hint in ["[y]once", "[a]session", "[A]always", "[n]deny"] {
                assert!(
                    flat.contains(hint),
                    "binding {hint} was clipped with prefix={prefix:?}:\n{s}"
                );
            }
        }
    }

    /// The modal is sized from the rows its text actually occupies. A
    /// prefix long enough to wrap several times counted as one line, so
    /// the box was sized too short and clipped the rest of it — while
    /// `[P]` stayed live over a scope the user could not finish reading.
    #[test]
    fn permission_modal_grows_for_a_prefix_that_wraps_many_rows() {
        let prefix = format!(
            "/opt/{}/bin/kubectl rollout",
            "very-long-directory".repeat(6)
        );
        for cols in [60u16, 80, 120] {
            let backend = TestBackend::new(cols, 40);
            let mut term = Terminal::new(backend).unwrap();
            let mut app = App::new("m", "/tmp", "s");
            app.phase = Phase::Permission;
            let (respond, _rx) = std::sync::mpsc::channel();
            app.modals
                .push_back(crate::ui::modern::app::Modal::Permission(
                    PendingPermission {
                        name: "Bash".into(),
                        description: "Bash: run a command".into(),
                        origin: None,
                        input_preview: None,
                        suggested_prefix: Some(prefix.clone()),
                        respond,
                    },
                ));
            term.draw(|f| draw(f, &mut app)).unwrap();
            let s = buffer_to_string(term.backend().buffer());
            let flat = squeeze(&s);
            assert!(
                flat.contains(&squeeze(&prefix)),
                "a wrapping prefix was clipped at {cols} columns:\n{s}"
            );
            // The description it pushed down must survive too.
            assert!(
                flat.contains(&squeeze("Bash: run a command")),
                "the modal body was clipped at {cols} columns:\n{s}"
            );
        }
    }

    /// Screen text with the box borders and all whitespace removed, so a
    /// string that wrapped across rows — even mid-word — can still be
    /// searched for as one piece.
    fn squeeze(s: &str) -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !"│┌┐└┘─".contains(*c))
            .collect()
    }

    #[test]
    fn wrapped_rows_counts_greedy_word_wrap() {
        assert_eq!(wrapped_rows("", 10), 1);
        assert_eq!(wrapped_rows("short", 10), 1);
        // Wraps whole words to the next row.
        assert_eq!(wrapped_rows("aaaa bbbb cccc", 10), 2);
        // A word wider than the line is split across rows.
        assert_eq!(wrapped_rows(&"x".repeat(25), 10), 3);
        assert_eq!(wrapped_rows("ab", 0), 1);
    }

    /// Rows are display columns, not characters. A CJK glyph takes two
    /// cells, so ten of them fill a 20-column line — counting characters
    /// would call that one row and size the modal half as tall as it
    /// needs to be.
    #[test]
    fn wrapped_rows_measures_display_width() {
        let cjk = "日".repeat(10); // 10 chars, 20 columns
        assert_eq!(wrapped_rows(&cjk, 20), 1);
        assert_eq!(wrapped_rows(&cjk, 10), 2);
        assert_eq!(wrapped_rows(&cjk, 6), 4);
        // An odd width cannot split a double-width glyph, so the row
        // ends one column early.
        assert_eq!(wrapped_rows(&cjk, 5), 5);
    }

    /// The permission modal must grow for double-width text too, or the
    /// durable-grant row is clipped exactly as it was for long prefixes.
    #[test]
    fn permission_modal_grows_for_double_width_text() {
        let backend = TestBackend::new(60, 40);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        let description = format!("Bash: {}", "日本語".repeat(20));
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                PendingPermission {
                    name: "Bash".into(),
                    description: description.clone(),
                    origin: None,
                    input_preview: None,
                    suggested_prefix: Some("git status".into()),
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        let flat = squeeze(&s);
        assert!(
            flat.contains(&squeeze(&description)),
            "double-width text was clipped:\n{s}"
        );
        assert!(
            flat.contains(&squeeze("git status")),
            "the durable-grant row was pushed out:\n{s}"
        );
    }

    /// The modal title names the tool being approved. A plugin manifest or
    /// executable filename can carry a bidi override into that name, which
    /// would misrender the identity of the tool on the authorization screen.
    #[test]
    fn permission_modal_reveals_a_bidi_override_in_the_tool_name() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                PendingPermission {
                    name: "deploy\u{202e}hsilbup\u{202c}".into(),
                    description: "run plugin".into(),
                    origin: None,
                    input_preview: None,
                    suggested_prefix: None,
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(
            !s.contains('\u{202e}'),
            "a bidi override in the tool name reached the screen:\n{s}"
        );
        assert!(
            s.contains("deploy<U+202E>hsilbup<U+202C>"),
            "override in the title not surfaced:\n{s}"
        );
    }

    #[test]
    fn search_bar_shows_the_query_and_match_count() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        for t in ["alpha auth beta", "gamma", "delta auth"] {
            app.transcript.push(TranscriptItem::System(t.into()));
        }
        // One frame to populate the layout the search reads.
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.open_search();
        for c in "auth".chars() {
            app.search_insert_char(c);
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("find: auth"), "buffer:\n{s}");
        assert!(s.contains("1/2"), "match counter missing:\n{s}");
    }

    #[test]
    fn search_bar_says_so_when_nothing_matches() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::System("alpha".into()));
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.open_search();
        for c in "zzz".chars() {
            app.search_insert_char(c);
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("no matches"), "buffer:\n{s}");
    }

    /// Focus follows input: while the bar is open the terminal caret must
    /// sit after the query, not blink in the composer the keys no longer
    /// reach.
    #[test]
    fn search_bar_owns_the_terminal_cursor() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::System("alpha".into()));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let composer_cursor = term.get_cursor_position().unwrap();
        app.open_search();
        for c in "al".chars() {
            app.search_insert_char(c);
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        let cur = term.get_cursor_position().unwrap();
        let s = buffer_to_string(term.backend().buffer());
        let bar_row = s
            .lines()
            .position(|l| l.contains("find: al"))
            .expect("search bar row") as u16;
        assert_eq!(cur.y, bar_row, "caret must be on the search row:\n{s}");
        assert_eq!(cur.x, 8 + 2, "caret must sit right after the query");
        assert_ne!(cur.y, composer_cursor.y, "caret must leave the composer");
    }

    /// A query wider than the row scrolls horizontally so the tail being
    /// edited stays visible next to the caret.
    #[test]
    fn long_query_keeps_its_editable_tail_visible() {
        let backend = TestBackend::new(40, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::System("alpha".into()));
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.open_search();
        for c in "prefix_that_is_much_longer_than_the_row_tail".chars() {
            app.search_insert_char(c);
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        let row = s.lines().find(|l| l.contains("find:")).expect("bar row");
        assert!(
            row.contains("_tail"),
            "the end of the query must stay visible: {row:?}"
        );
        assert!(row.contains('…'), "scrolled query must be marked: {row:?}");
        let cur = term.get_cursor_position().unwrap();
        assert_eq!(cur.x, 39, "caret must sit at the visible end");
    }

    /// Stepping matches must visibly anchor the `n/m` counter.
    #[test]
    fn current_search_match_row_is_highlighted() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        for t in ["alpha auth beta", "gamma", "delta auth"] {
            app.transcript.push(TranscriptItem::System(t.into()));
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.open_search();
        for c in "auth".chars() {
            app.search_insert_char(c);
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        let row = s
            .lines()
            .position(|l| l.contains("alpha auth beta"))
            .expect("first match visible") as u16;
        let buf = term.backend().buffer();
        let bg = |y: u16| buf.cell((0u16, y)).unwrap().style().bg;
        assert_ne!(
            bg(row),
            Some(ratatui::style::Color::Reset),
            "current match row must carry a highlight background:\n{s}"
        );
        let other = s
            .lines()
            .position(|l| l.contains("gamma"))
            .expect("non-match visible") as u16;
        assert_eq!(
            bg(other),
            Some(ratatui::style::Color::Reset),
            "non-match rows must stay unhighlighted:\n{s}"
        );
    }

    /// The bar gets a reserved layout row; drawing it at a fixed offset
    /// used to overwrite the composer's top border.
    #[test]
    fn search_bar_does_not_overdraw_the_composer_border() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.transcript
            .push(TranscriptItem::System("alpha auth".into()));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let corners_before = buffer_to_string(term.backend().buffer())
            .matches('╭')
            .count();
        app.open_search();
        for c in "auth".chars() {
            app.search_insert_char(c);
        }
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("find: auth"), "buffer:\n{s}");
        assert_eq!(
            s.matches('╭').count(),
            corners_before,
            "search bar must not eat a border row:\n{s}"
        );
    }

    /// The checklist was only ever visible as the text of a tool card,
    /// which scrolls away. The pane keeps it in front of the user.
    #[test]
    fn the_plan_checklist_renders_in_the_tasks_pane() {
        use crate::ui::modern::sink::EngineEvent;
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TodoUpdate {
            epoch: 0,
            items: vec![
                ("1".into(), "read the parser".into(), "done".into()),
                ("2".into(), "add the guard".into(), "in_progress".into()),
                ("3".into(), "write tests".into(), "pending".into()),
            ],
        });
        assert!(
            app.tasks_visible(),
            "a plan with no tasks should still show the pane"
        );
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("plan  1/3"), "progress missing:\n{s}");
        assert!(s.contains("add the guard"), "item missing:\n{s}");
        assert!(s.contains("✔"), "done glyph missing:\n{s}");
        assert!(s.contains("◐"), "in-progress glyph missing:\n{s}");
        assert!(s.contains("□"), "pending glyph missing:\n{s}");
    }

    /// Checklist text is model-authored and can carry text the model
    /// copied out of an untrusted repository. Like the transcript, the
    /// tool cards and the permission modal, this pane must not hand bidi
    /// overrides or zero-width characters straight to the terminal.
    #[test]
    fn checklist_content_is_scrubbed_of_deceptive_unicode() {
        use crate::ui::modern::sink::EngineEvent;
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TodoUpdate {
            epoch: 0,
            items: vec![(
                "1".into(),
                "delete \u{202e}sredro\u{202c} safely".into(),
                "in_progress".into(),
            )],
        });
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(
            s.contains("<U+202E>"),
            "bidi override was not escaped:\n{s}"
        );
        assert!(
            !s.contains('\u{202e}') && !s.contains('\u{202c}'),
            "raw bidi control reached the terminal:\n{s}"
        );
    }

    /// A model can emit a checklist of any length. The pane caps the rows
    /// it draws and says how many it held back — the previous code
    /// appended every entry and then reported "+0 more", which both
    /// overflowed the strip and lied about it.
    #[test]
    fn a_long_checklist_is_windowed_and_reports_what_it_hid() {
        use crate::ui::modern::sink::EngineEvent;
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        let items: Vec<(String, String, String)> = (0..60)
            .map(|i| {
                let status = match i {
                    n if n < 40 => "done",
                    40 => "in_progress",
                    _ => "pending",
                };
                // Zero-padded so no label is a prefix of another and the
                // "drawn" count below cannot double-count.
                (
                    format!("{i}"),
                    format!("checklist entry {i:02}"),
                    status.into(),
                )
            })
            .collect();
        app.apply_engine(EngineEvent::TodoUpdate { epoch: 0, items });
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());

        assert!(s.contains("plan  40/60"), "progress missing:\n{s}");
        assert!(
            s.contains("checklist entry 40"),
            "the in-progress item was windowed out:\n{s}"
        );
        assert!(
            !s.contains("+0 more"),
            "reported zero hidden while hiding entries:\n{s}"
        );
        // Everything it did not draw is accounted for in one honest count.
        let hidden: usize = s
            .split("+")
            .find_map(|seg| seg.split(" more").next()?.trim().parse().ok())
            .unwrap_or_else(|| panic!("no hidden count rendered:\n{s}"));
        let drawn = (0..60)
            .filter(|i| s.contains(&format!("checklist entry {i:02}")))
            .count();
        assert_eq!(hidden + drawn, 60, "hidden + drawn did not cover the list");

        // The pane must not have eaten the transcript's half of the screen.
        assert!(drawn < 60 && drawn > 0, "drew {drawn} of 60 rows:\n{s}");
    }

    /// A checklist and tasks competing for a short strip must never
    /// produce "+0 more (↑/↓)": the arrows navigate tasks, so promising
    /// hidden rows when every task is already drawn sends the user
    /// pressing keys that do nothing. Swept across heights because the
    /// failure only appeared at particular pane sizes.
    #[test]
    fn a_cramped_pane_never_promises_rows_the_arrows_cannot_reach() {
        use crate::ui::modern::sink::EngineEvent;
        for height in 8..=30u16 {
            for todo_count in [1usize, 2, 3, 8, 40] {
                let backend = TestBackend::new(60, height);
                let mut term = Terminal::new(backend).unwrap();
                let mut app = App::new("m", "/tmp", "s");
                crate::ui::modern::tasks::upsert(&mut app.tasks, "a1", "working", "explore");
                app.apply_engine(EngineEvent::TodoUpdate {
                    epoch: 0,
                    items: (0..todo_count)
                        .map(|i| (format!("{i}"), format!("entry {i:02}"), "pending".into()))
                        .collect(),
                });
                term.draw(|f| draw(f, &mut app)).unwrap();
                let s = buffer_to_string(term.backend().buffer());
                assert!(
                    !s.contains("+0 more"),
                    "empty overflow promise at {height} rows with {todo_count} todos:\n{s}"
                );
            }
        }
    }

    /// A frame exercising the chrome most tempting to hand-colour: an
    /// error, a warning, thinking, the tasks pane, a failed tool, and
    /// assistant markdown carrying both inline code and a fenced,
    /// syntax-highlighted block.
    fn colour_probe_app() -> App {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::User("hello".into()));
        app.transcript.push(TranscriptItem::Tool {
            call_id: "c1".into(),
            name: "Bash".into(),
            detail: "ls".into(),
            result: Some("a\nb".into()),
            is_error: false,
            live: None,
        });
        app.transcript
            .push(TranscriptItem::Error("it broke".into()));
        app.transcript
            .push(TranscriptItem::Warning("careful".into()));
        app.transcript.push(TranscriptItem::Thinking {
            text: "considering".into(),
            duration_ms: Some(1500),
        });
        app.transcript.push(TranscriptItem::Tool {
            call_id: "c2".into(),
            name: "FileEdit".into(),
            detail: "src/x.rs".into(),
            result: Some("boom".into()),
            is_error: true,
            live: None,
        });
        // Last, so the bottom-anchored transcript keeps it on screen:
        // a colour test that silently scrolls its own subject out of the
        // viewport passes for the wrong reason.
        app.transcript.push(TranscriptItem::Assistant(
            "call `run()` first\n\n```rust\nfn main() { let x = 1; }\n```\n".into(),
        ));
        crate::ui::modern::tasks::upsert(&mut app.tasks, "a1", "working", "explore");
        // The checklist is the other half of the tasks pane, so this
        // guard should cover its rows too, not only the agent rows.
        // Exactly one item: the strip this frame affords is short enough
        // that a longer checklist windows down to its heading and a
        // "+n more", leaving no item row on screen to inspect. `done`
        // puts the success colour in the frame, which the working agent
        // row above does not.
        app.apply_engine(crate::ui::modern::sink::EngineEvent::TodoUpdate {
            epoch: app.conversation_epoch,
            items: vec![("1".into(), "read the parser".into(), "done".into())],
        });
        app
    }

    /// Guards [`colour_probe_app`] against going vacuous: both the
    /// inline-code chip and the syntax-highlighted block must be on
    /// screen for the colour assertions below to mean anything.
    fn assert_probe_subject_visible(buf: &ratatui::buffer::Buffer) {
        let s = buffer_to_string(buf);
        assert!(s.contains("run()"), "inline code scrolled away:\n{s}");
        assert!(s.contains("fn main"), "code block scrolled away:\n{s}");
    }

    /// Every colour on screen must come from the theme.
    ///
    /// The theme adapts itself for the terminal's colour depth
    /// (`adapt_for_emit`), so a hardcoded `Color::DarkGray` in the chrome
    /// silently opts that cell out of the adaptation — under `NO_COLOR`
    /// it stays coloured. One-dark's slots are all RGB, so any *named*
    /// colour in the frame is a bypass.
    ///
    /// This catches named literals only. A hardcoded `Color::Rgb` is
    /// just as much a bypass and looks identical here — that is what
    /// [`no_colour_mode_leaves_no_colour_on_screen`] is for.
    #[test]
    fn the_frame_uses_only_theme_colours() {
        use ratatui::style::Color;
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = colour_probe_app();
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        assert_probe_subject_visible(buf);
        let mut offenders: Vec<String> = Vec::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                for (what, c) in [("fg", cell.fg), ("bg", cell.bg)] {
                    let named = !matches!(c, Color::Rgb(..) | Color::Reset);
                    if named {
                        offenders.push(format!("{what}={c:?} at {x},{y}"));
                    }
                }
            }
        }
        offenders.sort();
        offenders.dedup_by(|a, b| a.split(" at").next() == b.split(" at").next());
        assert!(
            offenders.is_empty(),
            "{} chrome colours bypass the palette: {:?}",
            offenders.len(),
            offenders
        );
    }

    /// `NO_COLOR` asks for *no* colour, not less of it — so in
    /// [`EmitMode::Mono`] every cell in a rendered frame must carry the
    /// terminal default for both foreground and background. Modifiers
    /// (bold, dim, italic) survive; hue does not.
    ///
    /// This is the assertion that a hardcoded colour cannot survive,
    /// whatever form it takes: only values that reached the screen
    /// through a palette slot (or through
    /// [`crate::ui::modern::colors::syntax_color`]) get adapted, so any
    /// literal — named *or* RGB — shows up here as a coloured cell.
    #[test]
    fn no_colour_mode_leaves_no_colour_on_screen() {
        use crate::ui::color_emit::{EmitMode, pin_mode};
        use ratatui::style::Color;
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let _mode = pin_mode(EmitMode::Mono);

        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = colour_probe_app();
        // Both filled highlights (mouse selection, current search match)
        // draw text on a coloured bar; a hardcoded black foreground there
        // turns invisible once the fill collapses to the default.
        app.open_search();
        for c in "run".chars() {
            app.search_insert_char(c);
        }
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        assert_probe_subject_visible(buf);
        let mut offenders: Vec<String> = Vec::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                for (what, c) in [("fg", cell.fg), ("bg", cell.bg)] {
                    if c != Color::Reset {
                        offenders.push(format!("{what}={c:?} at {x},{y}"));
                    }
                }
            }
        }
        offenders.sort();
        offenders.dedup_by(|a, b| a.split(" at").next() == b.split(" at").next());
        assert!(
            offenders.is_empty(),
            "{} cells still carry colour under NO_COLOR: {:?}",
            offenders.len(),
            offenders
        );
    }

    /// The mirror image of [`the_frame_uses_only_theme_colours`]: in
    /// [`EmitMode::Ansi16`] every palette slot is quantized to a *named*
    /// colour, so a surviving `Color::Rgb` is a value that never went
    /// through the adaptation — the exact bypass a truecolor-only test
    /// cannot see.
    ///
    /// Run against the `dark-ansi` accessibility palette, whose slots are
    /// named to begin with, so the frame has to come out named end to end.
    #[test]
    fn ansi16_mode_quantizes_every_colour_on_screen() {
        use crate::ui::color_emit::{EmitMode, pin_mode};
        use ratatui::style::Color;
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("dark-ansi");
        let _mode = pin_mode(EmitMode::Ansi16);

        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = colour_probe_app();
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        assert_probe_subject_visible(buf);
        let mut offenders: Vec<String> = Vec::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                for (what, c) in [("fg", cell.fg), ("bg", cell.bg)] {
                    if matches!(c, Color::Rgb(..)) {
                        offenders.push(format!("{what}={c:?} at {x},{y}"));
                    }
                }
            }
        }
        offenders.sort();
        offenders.dedup_by(|a, b| a.split(" at").next() == b.split(" at").next());
        assert!(
            offenders.is_empty(),
            "{} cells kept 24-bit colour on a 16-colour terminal: {:?}",
            offenders.len(),
            offenders
        );
    }

    /// The filled highlight bars used to paint `Color::Black` text on a
    /// coloured fill. That is wrong twice over: under `NO_COLOR` the fill
    /// collapses to the terminal default while the black text stays
    /// black — invisible on any dark terminal — and on a light theme the
    /// fill is dark enough that black was already a poor choice.
    ///
    /// The replacement is not a fixed foreground either: `on_fill` picks
    /// against the actual fill, so the bar clears a readable contrast
    /// ratio on light and dark themes alike.
    #[test]
    fn filled_highlights_take_their_foreground_from_the_palette() {
        use crate::ui::color_emit::{EmitMode, pin_mode};
        use ratatui::style::Color;
        let _g = crate::ui::theme::test_lock();

        let view = vec![Line::from("match here")];
        let sel = super::super::app::TextSelection {
            start_line: 0,
            end_line: 0,
        };

        for theme in ["one-dark", "solarized-light", "dark-ansi", "light-ansi"] {
            crate::ui::theme::init(theme);
            let painted = apply_selection_highlight(view.clone(), 0, Some(sel));
            let style = painted[0].spans[0].style;
            assert_eq!(
                style.fg,
                Some(crate::ui::modern::colors::on_fill(palette().accent)),
                "{theme}: selection bar bypassed on_fill"
            );
            assert_ne!(style.fg, style.bg, "{theme}: selection bar is invisible");
        }

        // Mono: the fill and its foreground both collapse to the default,
        // so the row stays legible instead of going black-on-black.
        crate::ui::theme::init("one-dark");
        let _mode = pin_mode(EmitMode::Mono);
        let painted = apply_selection_highlight(view, 0, Some(sel));
        assert_eq!(painted[0].spans[0].style.fg, Some(Color::Reset));
        assert_eq!(painted[0].spans[0].style.bg, Some(Color::Reset));
    }

    #[test]
    fn a_folded_group_hides_its_rows_and_shows_its_size() {
        use crate::ui::modern::tasks::TaskSource;
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        crate::ui::modern::tasks::upsert(&mut app.tasks, "a1", "working", "explore the parser");
        crate::ui::modern::tasks::upsert_with_source(
            &mut app.tasks,
            "b1",
            "working",
            "cargo build",
            TaskSource::Background,
        );

        term.draw(|f| draw(f, &mut app)).unwrap();
        let open = buffer_to_string(term.backend().buffer());
        assert!(open.contains("explore the parser"), "buffer:\n{open}");
        assert!(open.contains("▾ agents"), "no expanded marker:\n{open}");

        app.collapsed_groups.push(TaskSource::Subagent);
        term.draw(|f| draw(f, &mut app)).unwrap();
        let folded = buffer_to_string(term.backend().buffer());
        assert!(
            !folded.contains("explore the parser"),
            "folding did not hide the row:\n{folded}"
        );
        // The count keeps the group honest about what it is hiding.
        assert!(folded.contains("▸ agents (1)"), "buffer:\n{folded}");
        // The other group is untouched.
        assert!(folded.contains("cargo build"), "buffer:\n{folded}");
    }

    #[test]
    fn normal_mode_is_visible_in_the_prompt() {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.vi_mode = true;
        app.input = "hello".into();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let insert = buffer_to_string(term.backend().buffer());
        assert!(insert.contains("❯ hello"), "buffer:\n{insert}");

        app.composer_mode = crate::ui::modern::app::ComposerMode::Normal;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let normal = buffer_to_string(term.backend().buffer());
        assert!(
            normal.contains("▪ hello"),
            "normal mode is indistinguishable from insert:\n{normal}"
        );
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
                    suggested_prefix: None,
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
    fn permission_modal_scrolls_through_full_input() {
        // 200-line input: the top shows an adaptive window; scrolling
        // pans to rows that were previously hidden (#413).
        let preview: String = (1..=200)
            .map(|i| format!("\"arg{i}\": {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let backend = TestBackend::new(90, 30);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, _rx) = std::sync::mpsc::channel();
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                PendingPermission {
                    name: "Bash".into(),
                    description: "big input".into(),
                    origin: None,
                    input_preview: Some(preview),
                    suggested_prefix: None,
                    respond,
                },
            ));

        term.draw(|f| draw(f, &mut app)).unwrap();
        let top = buffer_to_string(term.backend().buffer());
        assert!(top.contains("\"arg1\":"), "unscrolled shows head:\n{top}");
        assert!(!top.contains("\"arg150\":"), "tail hidden at top:\n{top}");
        assert!(top.contains("more lines (↓"), "below indicator:\n{top}");

        app.perm_scroll = 148;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let scrolled = buffer_to_string(term.backend().buffer());
        assert!(
            scrolled.contains("\"arg150\":"),
            "scrolled view reaches deep rows:\n{scrolled}"
        );
        assert!(
            scrolled.contains("earlier lines (↑"),
            "above indicator:\n{scrolled}"
        );
        assert!(
            scrolled.contains("[y]") && scrolled.contains("[n]"),
            "key footer stays visible while scrolled:\n{scrolled}"
        );

        // Absurd offset clamps to the end instead of blanking the view.
        app.perm_scroll = 10_000;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let clamped = buffer_to_string(term.backend().buffer());
        assert!(
            clamped.contains("\"arg200\": 200"),
            "clamped to last rows:\n{clamped}"
        );
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
                    description:
                        "Bash: run a long pipeline that wraps across many columns and rows".into(),
                    origin: None,
                    input_preview: Some(preview),
                    suggested_prefix: None,
                    respond,
                },
            ));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(
            s.contains("[y]"),
            "sticky footer [y] missing under tall body:\n{s}"
        );
        assert!(
            s.contains("[n]"),
            "sticky footer [n] missing under tall body:\n{s}"
        );
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
                        suggested_prefix: None,
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
            live: None,
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

    /// One agent + one background task need seven strip rows (two
    /// headings, a gap, two lines per task); the old fixed five-row
    /// strip clipped the background group entirely on narrow terminals.
    #[test]
    fn narrow_terminal_strip_shows_the_background_group() {
        let backend = TestBackend::new(100, 30);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(crate::ui::modern::sink::EngineEvent::SubagentUpdate {
            agent_id: "a1".into(),
            state: "working".into(),
            headline: "explore parser".into(),
        });
        app.sync_background_tasks(vec![crate::ui::modern::tasks::ManagerRow {
            id: "b1".into(),
            state: "working".into(),
            headline: "cargo build --release".into(),
            subagent_id: None,
        }]);
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("background"), "background heading missing:\n{s}");
        assert!(
            s.contains("cargo build"),
            "background headline missing:\n{s}"
        );
    }

    /// Task headlines are model-/tool-supplied text; the pane must run
    /// them through the same deceptive-character scrub as every other
    /// surface (zero-width chars, bidi overrides).
    #[test]
    fn tasks_pane_escapes_deceptive_headline_text() {
        let backend = TestBackend::new(120, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(crate::ui::modern::sink::EngineEvent::SubagentUpdate {
            agent_id: "a1".into(),
            state: "working".into(),
            headline: "rm -\u{200B}rf tmp".into(),
        });
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("<U+200B>"), "zero-width char not escaped:\n{s}");
        assert!(
            !s.contains("rm -\u{200B}rf"),
            "raw deceptive text rendered:\n{s}"
        );
    }

    /// When even the grown strip cannot fit every task, the pane says
    /// how many are hidden instead of silently clipping.
    #[test]
    fn overflowing_tasks_pane_reports_hidden_rows() {
        let backend = TestBackend::new(100, 16);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        let rows = (0..6)
            .map(|i| crate::ui::modern::tasks::ManagerRow {
                id: format!("b{i}"),
                state: "working".into(),
                headline: format!("job {i}"),
                subagent_id: None,
            })
            .collect();
        app.sync_background_tasks(rows);
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("… +"), "hidden-task indicator missing:\n{s}");
    }

    /// Up/Down cycles through every task, so the window must scroll to
    /// keep the marked task on screen instead of clipping a fixed prefix.
    #[test]
    fn overflow_window_follows_the_selection() {
        let backend = TestBackend::new(100, 16);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/tmp", "s");
        let rows = (0..6)
            .map(|i| crate::ui::modern::tasks::ManagerRow {
                id: format!("b{i}"),
                state: "working".into(),
                headline: format!("job number {i}"),
                subagent_id: None,
            })
            .collect();
        app.sync_background_tasks(rows);
        app.tasks_selected = 5;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(
            s.contains("job number 5"),
            "selected task scrolled out of view:\n{s}"
        );
    }

    /// A pane holding a big folded group ahead of a selected, expanded
    /// one — the shape that broke the overflow window.
    fn app_with_folded_group_then_background(agents: usize, jobs: usize) -> App {
        let mut app = App::new("m", "/tmp", "s");
        for i in 0..agents {
            crate::ui::modern::tasks::upsert(
                &mut app.tasks,
                &format!("a{i}"),
                "working",
                &format!("agent number {i}"),
            );
        }
        let rows = (0..jobs)
            .map(|i| crate::ui::modern::tasks::ManagerRow {
                id: format!("b{i}"),
                state: "working".into(),
                headline: format!("job number {i}"),
                subagent_id: None,
            })
            .collect();
        app.sync_background_tasks(rows);
        app.collapsed_groups
            .push(crate::ui::modern::tasks::TaskSource::Subagent);
        app
    }

    /// The "… +n more" count.
    fn hidden_count(s: &str) -> usize {
        let tail = s.split("… +").nth(1).expect("no overflow indicator");
        tail.split(' ')
            .next()
            .expect("no count")
            .parse()
            .expect("count not a number")
    }

    /// `task_ends` was a dense list of *rendered* rows indexed by the
    /// absolute task index, so a folded group ahead of the selection
    /// shifted the lookup: the window anchored on the wrong row (or fell
    /// back to zero) and scrolled the selected task off screen.
    #[test]
    fn overflow_window_follows_selection_past_a_folded_group() {
        let backend = TestBackend::new(100, 16);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app_with_folded_group_then_background(20, 6);
        // Last background row: far below the folded group.
        app.tasks_selected = app.tasks.len() - 1;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(
            s.contains("job number 5"),
            "selected task scrolled out of view past a folded group:\n{s}"
        );
    }

    /// Walking down the visible group must keep every step on screen,
    /// not just the first.
    #[test]
    fn stepping_through_a_group_after_a_folded_one_stays_visible() {
        let backend = TestBackend::new(100, 16);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app_with_folded_group_then_background(20, 6);
        app.tasks_selected =
            crate::ui::modern::tasks::selectable_indices(&app.tasks, &app.collapsed_groups)[1];
        for step in 0..6 {
            term.draw(|f| draw(f, &mut app)).unwrap();
            let s = buffer_to_string(term.backend().buffer());
            let headline = format!("job number {step}");
            assert!(
                s.contains(&headline),
                "step {step}: selected task not on screen:\n{s}"
            );
            app.tasks_select(1);
        }
    }

    /// A folded heading that is on screen already tells the user how
    /// many rows are behind it, so those must not be counted again in
    /// the "+n more" tally.
    #[test]
    fn a_visible_folded_heading_accounts_for_its_own_rows() {
        let backend = TestBackend::new(100, 16);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app_with_folded_group_then_background(20, 6);
        app.tasks_selected =
            crate::ui::modern::tasks::selectable_indices(&app.tasks, &app.collapsed_groups)[0];
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("▸ agents (20)"), "folded heading missing:\n{s}");
        let hidden = hidden_count(&s);
        assert!(
            hidden <= 6,
            "the 20 rows behind the visible folded heading were counted as hidden: +{hidden}\n{s}"
        );
    }

    /// A folded heading is one line, but the window used to derive the
    /// marker row as `end - 1` — the shape of a two-line task row. On a
    /// short pane that stepped onto the blank separator above the
    /// heading and dropped the selected group off screen entirely, so
    /// the user had to unfold it blind. The second group is the folded
    /// one here: the first group's heading sits at line 0, where the
    /// off-by-one is invisible.
    #[test]
    fn a_selected_folded_heading_survives_every_pane_height() {
        for h in 8..=22u16 {
            let mut term = Terminal::new(TestBackend::new(100, h)).unwrap();
            let mut app = App::new("m", "/tmp", "s");
            for i in 0..12 {
                crate::ui::modern::tasks::upsert(
                    &mut app.tasks,
                    &format!("a{i}"),
                    "working",
                    &format!("agent number {i}"),
                );
            }
            let rows = (0..3)
                .map(|i| crate::ui::modern::tasks::ManagerRow {
                    id: format!("b{i}"),
                    state: "working".into(),
                    headline: format!("job number {i}"),
                    subagent_id: None,
                })
                .collect();
            app.sync_background_tasks(rows);
            // "background" sorts after "agents", so this is the second
            // group.
            app.collapsed_groups
                .push(crate::ui::modern::tasks::TaskSource::Background);
            app.tasks_selected = app
                .tasks
                .iter()
                .position(|t| t.source == crate::ui::modern::tasks::TaskSource::Background)
                .unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
            let s = buffer_to_string(term.backend().buffer());
            assert!(
                s.contains("❯▸ background (3)"),
                "height {h}: selected folded heading not on screen:\n{s}"
            );
        }
    }

    /// The same window must still keep a selected *task* row's marker
    /// on screen — the marker sits on the status line, above the
    /// headline.
    #[test]
    fn a_selected_task_row_keeps_its_marker_at_every_pane_height() {
        for h in 8..=22u16 {
            let mut term = Terminal::new(TestBackend::new(100, h)).unwrap();
            let mut app = App::new("m", "/tmp", "s");
            let rows = (0..8)
                .map(|i| crate::ui::modern::tasks::ManagerRow {
                    id: format!("b{i}"),
                    state: "working".into(),
                    headline: format!("job number {i}"),
                    subagent_id: None,
                })
                .collect();
            app.sync_background_tasks(rows);
            app.tasks_selected = app.tasks.len() - 1;
            term.draw(|f| draw(f, &mut app)).unwrap();
            let s = buffer_to_string(term.backend().buffer());
            assert!(s.contains('❯'), "height {h}: selection marker lost:\n{s}");
        }
    }

    /// The selection marker has to render on the heading when the
    /// selected group is folded, or the pane looks like it lost focus.
    #[test]
    fn a_folded_heading_shows_the_selection_marker() {
        let backend = TestBackend::new(100, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = app_with_folded_group_then_background(2, 2);
        app.tasks_selected =
            crate::ui::modern::tasks::selectable_indices(&app.tasks, &app.collapsed_groups)[0];
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = buffer_to_string(term.backend().buffer());
        assert!(s.contains("❯▸ agents (2)"), "marker not on heading:\n{s}");
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
