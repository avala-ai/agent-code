//! Arrow-key interactive selector for terminal menus.
//!
//! Renders a list of options with a highlighted cursor that moves
//! with up/down arrow keys. Enter confirms the selection.
//! Supports optional live preview that updates as the cursor moves.

use std::io::Write;

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    style::Stylize,
    terminal,
};

/// A single option in the selector.
pub struct SelectOption {
    pub label: String,
    pub description: String,
    pub value: String,
    /// Optional preview content shown below the options when this item is focused.
    pub preview: Option<String>,
}

/// Show an interactive selector and return the chosen value.
pub fn select(options: &[SelectOption]) -> String {
    if options.is_empty() {
        return String::new();
    }

    let has_preview = options.iter().any(|o| o.preview.is_some());
    let mut selected = 0usize;

    terminal::enable_raw_mode().expect("failed to enable raw mode");

    render_all(options, selected, has_preview);

    loop {
        if let Ok(Event::Key(KeyEvent { code, .. })) = event::read() {
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = if selected > 0 {
                        selected - 1
                    } else {
                        options.len() - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = if selected < options.len() - 1 {
                        selected + 1
                    } else {
                        0
                    };
                }
                KeyCode::Enter => break,
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char(c) => {
                    let idx = c.to_ascii_lowercase() as usize - 'a' as usize;
                    if idx < options.len() {
                        selected = idx;
                        break;
                    }
                }
                _ => {}
            }

            clear_all(options.len(), has_preview);
            render_all(options, selected, has_preview);
        }
    }

    terminal::disable_raw_mode().expect("failed to disable raw mode");

    clear_all(options.len(), has_preview);
    let chosen = &options[selected];
    let t = super::theme::current();
    println!(
        "    {} {}\r",
        "→".with(t.accent),
        chosen.label.clone().bold()
    );

    options[selected].value.clone()
}

/// Preview lines count (fixed height so the UI doesn't jump).
const PREVIEW_LINES: usize = 6;

fn render_all(options: &[SelectOption], selected: usize, has_preview: bool) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // Render options.
    for (i, opt) in options.iter().enumerate() {
        let letter = (b'A' + i as u8) as char;
        let t = super::theme::current();
        if i == selected {
            write!(
                out,
                "  {} {} {}\r\n",
                format!("❯ {letter})").with(t.accent).bold(),
                opt.label.clone().with(t.text).bold(),
                opt.description.clone().with(t.muted),
            )
            .ok();
        } else {
            write!(
                out,
                "    {}) {} {}\r\n",
                letter,
                opt.label,
                opt.description.clone().with(t.muted),
            )
            .ok();
        }
    }

    // Render preview block if any option has preview content.
    if has_preview {
        write!(out, "\r\n").ok(); // Blank separator line.
        let preview_text = options[selected].preview.as_deref().unwrap_or("");

        let lines: Vec<&str> = preview_text.lines().collect();
        for i in 0..PREVIEW_LINES {
            if i < lines.len() {
                write!(out, "    {}\r\n", lines[i]).ok();
            } else {
                write!(out, "    \r\n").ok();
            }
        }
    }

    out.flush().ok();
}

fn clear_all(option_count: usize, has_preview: bool) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let total = option_count + if has_preview { PREVIEW_LINES + 1 } else { 0 };
    for _ in 0..total {
        write!(out, "\x1b[A\x1b[2K").ok();
    }
    out.flush().ok();
}
