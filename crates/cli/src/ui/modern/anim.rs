//! Micro-animation helpers for the modern TUI.
//!
//! Braille spinners, calm blink cycles for action-required, and soft pulse
//! styles. Driven by [`super::app::App::tick`] (~12 fps while live / HITL).

use ratatui::style::{Color, Modifier, Style};

/// Braille spinner frames (same family as production agent screens).
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Frame at `tick` index.
pub fn spinner_glyph(tick: u64) -> char {
    SPINNER[(tick as usize) % SPINNER.len()]
}

/// ~500ms on / 500ms off at 80ms ticks (divisor 6 ≈ 480ms).
const BLINK_DIVISOR: u64 = 6;

/// Whether a blinking element should be painted this frame.
///
/// When `focused` is true, always show (no flicker while the user is
/// actively answering a modal). When unfocused, alternate visibility so
/// the tab / status bar can attract attention.
pub fn blink_visible(tick: u64, focused: bool) -> bool {
    if focused {
        return true;
    }
    (tick / BLINK_DIVISOR).is_multiple_of(2)
}

/// Soft accent pulse for live streaming chrome (opacity approximated by
/// alternating bold / normal every few frames).
pub fn pulse_style(tick: u64, base: Color) -> Style {
    let bold = (tick / 3).is_multiple_of(2);
    let mut s = Style::default().fg(base);
    if bold {
        s = s.add_modifier(Modifier::BOLD);
    }
    s
}

/// Dim style for toast / ephemeral status that is about to expire.
pub fn toast_style(remaining: u8) -> Style {
    if remaining <= 4 {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(Color::Gray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_cycles() {
        assert_eq!(spinner_glyph(0), SPINNER[0]);
        assert_eq!(spinner_glyph(10), SPINNER[0]);
        assert_ne!(spinner_glyph(0), spinner_glyph(1));
    }

    #[test]
    fn blink_always_on_when_focused() {
        for t in 0..20 {
            assert!(blink_visible(t, true));
        }
    }

    #[test]
    fn blink_toggles_when_unfocused() {
        let a = blink_visible(0, false);
        let b = blink_visible(BLINK_DIVISOR, false);
        assert_ne!(a, b);
    }
}
