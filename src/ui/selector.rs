//! Arrow-key interactive selector for terminal menus.
//!
//! Renders a list of options with a highlighted cursor that moves
//! with up/down arrow keys. Enter confirms the selection.
//! Uses crossterm raw mode for immediate key capture.

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
}

/// Show an interactive selector and return the chosen value.
/// Returns the `value` field of the selected option.
pub fn select(options: &[SelectOption]) -> String {
    if options.is_empty() {
        return String::new();
    }

    let mut selected = 0usize;

    // Enter raw mode for immediate key capture.
    terminal::enable_raw_mode().expect("failed to enable raw mode");

    // Initial render.
    render_options(options, selected);

    loop {
        if let Ok(Event::Key(KeyEvent { code, .. })) = event::read() {
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 {
                        selected -= 1;
                    } else {
                        selected = options.len() - 1; // Wrap around.
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < options.len() - 1 {
                        selected += 1;
                    } else {
                        selected = 0; // Wrap around.
                    }
                }
                KeyCode::Enter => {
                    break;
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    break;
                }
                // Also accept letter shortcuts (a, b, c...).
                KeyCode::Char(c) => {
                    let idx = c.to_ascii_lowercase() as usize - 'a' as usize;
                    if idx < options.len() {
                        selected = idx;
                        break;
                    }
                }
                _ => {}
            }

            // Clear and re-render.
            clear_options(options.len());
            render_options(options, selected);
        }
    }

    // Restore terminal.
    terminal::disable_raw_mode().expect("failed to disable raw mode");

    // Clear the menu and show the selection.
    clear_options(options.len());
    let chosen = &options[selected];
    println!("    {} {}\r", "→".dark_cyan(), chosen.label.clone().bold());

    options[selected].value.clone()
}

fn render_options(options: &[SelectOption], selected: usize) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for (i, opt) in options.iter().enumerate() {
        let letter = (b'A' + i as u8) as char;
        if i == selected {
            // Highlighted.
            write!(
                out,
                "  {} {} {}\r\n",
                format!("❯ {letter})").dark_cyan().bold(),
                opt.label.clone().white().bold(),
                opt.description.clone().dark_grey(),
            )
            .ok();
        } else {
            write!(
                out,
                "    {}) {} {}\r\n",
                letter,
                opt.label,
                opt.description.clone().dark_grey(),
            )
            .ok();
        }
    }
    out.flush().ok();
}

fn clear_options(count: usize) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    // Move up and clear each line.
    for _ in 0..count {
        write!(out, "\x1b[A\x1b[2K").ok();
    }
    out.flush().ok();
}
