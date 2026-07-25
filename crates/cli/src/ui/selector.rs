//! Arrow-key interactive selector for terminal menus.
//!
//! Renders a list of options with a highlighted cursor that moves
//! with up/down arrow keys. Enter confirms the selection.
//! Supports optional live preview that updates as the cursor moves.

use std::io::Write;

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
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

/// How the selector loop was exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectorExit {
    /// Enter or a letter hotkey confirmed the selection.
    Chosen,
    /// Esc / `q` dismissed the menu (legacy: falls through to the
    /// highlighted value in [`select`]).
    Dismissed,
    /// Ctrl+C / Ctrl+D. The user is bailing out — never treat this as
    /// picking anything. Raw mode swallows the SIGINT a cooked terminal
    /// would deliver, so the selector must recognize the chord itself;
    /// before this arm existed, Ctrl+C fell into the letter-hotkey arm
    /// ('c' − 'a' = 2) and silently chose the third option, or did
    /// nothing at all in shorter menus — the selector felt hung.
    Aborted,
}

/// What a key event does to the selector. Pure — extracted from the
/// event loop so the key contract is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    MoveTo(usize),
    Choose(usize),
    Confirm,
    Dismiss,
    Abort,
    Ignore,
}

/// Decide what `key` does given the current highlight and option count.
fn key_action(key: &KeyEvent, selected: usize, len: usize) -> KeyAction {
    // Only act on presses (and repeats, so held arrows keep moving).
    // Kitty-protocol terminals also emit Release events.
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return KeyAction::Ignore;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL)
        || key.modifiers.contains(KeyModifiers::SUPER);
    match key.code {
        // Ctrl+C / Ctrl+D (or their raw ETX / EOT bytes — some paths
        // deliver the byte without the CONTROL modifier) abort.
        KeyCode::Char('\u{3}') | KeyCode::Char('\u{4}') => KeyAction::Abort,
        KeyCode::Char(c)
            if ctrl && (c.eq_ignore_ascii_case(&'c') || c.eq_ignore_ascii_case(&'d')) =>
        {
            KeyAction::Abort
        }
        // Any other Ctrl/Alt chord is not a hotkey or nav key.
        _ if ctrl || key.modifiers.contains(KeyModifiers::ALT) => KeyAction::Ignore,
        KeyCode::Up | KeyCode::Char('k') => {
            KeyAction::MoveTo(if selected > 0 { selected - 1 } else { len - 1 })
        }
        KeyCode::Down | KeyCode::Char('j') => {
            KeyAction::MoveTo(if selected < len - 1 { selected + 1 } else { 0 })
        }
        KeyCode::Enter => KeyAction::Confirm,
        KeyCode::Char('q') | KeyCode::Esc => KeyAction::Dismiss,
        // Letter hotkeys A..Z select directly. Guard on alphabetic so
        // digits / punctuation below 'a' can't underflow the index.
        KeyCode::Char(c) if c.is_ascii_alphabetic() => {
            let idx = c.to_ascii_lowercase() as usize - 'a' as usize;
            if idx < len {
                KeyAction::Choose(idx)
            } else {
                KeyAction::Ignore
            }
        }
        _ => KeyAction::Ignore,
    }
}

/// Show an interactive selector and return the chosen value.
///
/// Esc/`q` cancel by returning the currently-highlighted value (legacy
/// behavior kept for non-security callers). Ctrl+C / Ctrl+D return an
/// empty string — every caller already treats empty as "nothing
/// picked", and bailing out must never commit the highlighted row. For
/// prompts where Esc-cancel must NOT fall through to a default action
/// (e.g. permission modals), use [`select_cancellable`] instead.
pub fn select(options: &[SelectOption]) -> String {
    if options.is_empty() {
        return String::new();
    }
    let (index, exit) = select_index(options);
    if exit == SelectorExit::Aborted {
        print_choice("✕", "cancelled");
        return String::new();
    }
    print_choice("→", &options[index].label);
    options[index].value.clone()
}

/// Like [`select`], but returns `None` when the user cancels with Esc/`q`
/// (or bails with Ctrl+C / Ctrl+D) instead of falling through to the
/// highlighted option. Callers that gate a side effect on the result
/// (permission prompts) must use this so a dismissed modal cannot
/// silently pick the default.
pub fn select_cancellable(options: &[SelectOption]) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    let (index, exit) = select_index(options);
    if exit != SelectorExit::Chosen {
        print_choice("✕", "cancelled");
        None
    } else {
        print_choice("→", &options[index].label);
        Some(options[index].value.clone())
    }
}

/// Print the confirmed/cancelled line under a dismissed selector.
fn print_choice(marker: &str, label: &str) {
    let t = super::theme::current();
    println!(
        "    {} {}\r",
        marker.with(t.accent),
        label.to_string().bold()
    );
}

/// Core selector loop. Returns the highlighted index and how the menu
/// was exited.
fn select_index(options: &[SelectOption]) -> (usize, SelectorExit) {
    let has_preview = options.iter().any(|o| o.preview.is_some());
    let mut selected = 0usize;
    let mut exit = SelectorExit::Chosen;

    terminal::enable_raw_mode().expect("failed to enable raw mode");

    render_all(options, selected, has_preview);

    loop {
        if let Ok(Event::Key(key)) = event::read() {
            match key_action(&key, selected, options.len()) {
                KeyAction::MoveTo(idx) => selected = idx,
                KeyAction::Choose(idx) => {
                    selected = idx;
                    break;
                }
                KeyAction::Confirm => break,
                KeyAction::Dismiss => {
                    exit = SelectorExit::Dismissed;
                    break;
                }
                KeyAction::Abort => {
                    exit = SelectorExit::Aborted;
                    break;
                }
                KeyAction::Ignore => continue,
            }

            clear_all(options.len(), has_preview);
            render_all(options, selected, has_preview);
        }
    }

    terminal::disable_raw_mode().expect("failed to disable raw mode");

    clear_all(options.len(), has_preview);

    (selected, exit)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn ctrl_c_aborts_instead_of_choosing_third_option() {
        let action = key_action(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), 0, 5);
        assert_eq!(action, KeyAction::Abort);
    }

    #[test]
    fn ctrl_c_aborts_even_in_short_menus() {
        // Before the Abort arm, Ctrl+C in a 2-option menu hit the hotkey
        // arm, indexed out of range, and was silently ignored — the
        // selector looked hung.
        let action = key_action(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), 0, 2);
        assert_eq!(action, KeyAction::Abort);
    }

    #[test]
    fn ctrl_d_and_raw_etx_eot_abort() {
        for k in [
            key(KeyCode::Char('d'), KeyModifiers::CONTROL),
            key(KeyCode::Char('D'), KeyModifiers::CONTROL),
            key(KeyCode::Char('\u{3}'), KeyModifiers::NONE),
            key(KeyCode::Char('\u{4}'), KeyModifiers::NONE),
        ] {
            assert_eq!(key_action(&k, 0, 4), KeyAction::Abort, "{k:?}");
        }
    }

    #[test]
    fn super_c_aborts_like_ctrl_c() {
        let action = key_action(&key(KeyCode::Char('c'), KeyModifiers::SUPER), 0, 5);
        assert_eq!(action, KeyAction::Abort);
    }

    #[test]
    fn plain_c_is_still_the_third_hotkey() {
        let action = key_action(&key(KeyCode::Char('c'), KeyModifiers::NONE), 0, 5);
        assert_eq!(action, KeyAction::Choose(2));
    }

    #[test]
    fn other_ctrl_chords_are_ignored_not_hotkeys() {
        let action = key_action(&key(KeyCode::Char('b'), KeyModifiers::CONTROL), 0, 5);
        assert_eq!(action, KeyAction::Ignore);
        let action = key_action(&key(KeyCode::Char('j'), KeyModifiers::CONTROL), 1, 5);
        assert_eq!(action, KeyAction::Ignore, "Ctrl+J must not navigate");
    }

    #[test]
    fn digits_below_a_do_not_underflow() {
        // '1' < 'a': the old unguarded subtraction underflowed (panic in
        // debug builds). Must be ignored.
        let action = key_action(&key(KeyCode::Char('1'), KeyModifiers::NONE), 0, 5);
        assert_eq!(action, KeyAction::Ignore);
    }

    #[test]
    fn esc_and_q_dismiss() {
        assert_eq!(
            key_action(&key(KeyCode::Esc, KeyModifiers::NONE), 0, 3),
            KeyAction::Dismiss
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('q'), KeyModifiers::NONE), 0, 3),
            KeyAction::Dismiss
        );
    }

    #[test]
    fn release_events_are_ignored() {
        let mut k = key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        k.kind = KeyEventKind::Release;
        assert_eq!(key_action(&k, 0, 5), KeyAction::Ignore);
    }

    #[test]
    fn nav_wraps_both_ways() {
        assert_eq!(
            key_action(&key(KeyCode::Up, KeyModifiers::NONE), 0, 3),
            KeyAction::MoveTo(2)
        );
        assert_eq!(
            key_action(&key(KeyCode::Down, KeyModifiers::NONE), 2, 3),
            KeyAction::MoveTo(0)
        );
    }
}
