//! Product theme → ratatui colors for the modern TUI.
//!
//! Shared theme paints prompts, selectors, and badges with
//! [`crate::ui::theme::Theme::accent`] (steel-blue on midnight). Route every
//! modern highlight through this palette so chrome stays in sync when the
//! user picks a theme.

use ratatui::style::Color;

use crate::ui::theme;
use crate::ui::tui::theme_to_ratatui;

/// Snapshot of the active product theme as ratatui colors.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Brand highlight (prompt / selector / borders) — calm steel-blue.
    pub accent: Color,
    pub tool: Color,
    pub warning: Color,
    pub error: Color,
    pub success: Color,
    pub muted: Color,
    pub inactive: Color,
    pub text: Color,
    pub plan: Color,
    /// Foreground for text sitting on an accent/warning fill.
    pub on_accent: Color,
    /// Inline-code / code-block foreground and background.
    pub code_fg: Color,
    pub code_bg: Color,
    // Diff rendering (inline edit cards).
    pub diff_add: Color,
    pub diff_remove: Color,
    pub diff_add_dim: Color,
    pub diff_remove_dim: Color,
    pub diff_add_word: Color,
    pub diff_remove_word: Color,
    // Message backgrounds — tint the user's own turns so they stand out
    // from the model's output when scanning back through a transcript.
    pub user_msg_bg: Color,
    pub bash_msg_bg: Color,
    pub memory_msg_bg: Color,
}

/// Read the active theme (falls back to midnight if not initialized).
pub fn palette() -> Palette {
    let t = theme::current();
    Palette {
        accent: theme_to_ratatui(t.accent),
        tool: theme_to_ratatui(t.tool),
        warning: theme_to_ratatui(t.warning),
        error: theme_to_ratatui(t.error),
        success: theme_to_ratatui(t.success),
        muted: theme_to_ratatui(t.muted),
        inactive: theme_to_ratatui(t.inactive),
        text: theme_to_ratatui(t.text),
        plan: theme_to_ratatui(t.plan_mode),
        on_accent: theme_to_ratatui(t.on_accent),
        code_fg: theme_to_ratatui(t.code_fg),
        code_bg: theme_to_ratatui(t.code_bg),
        diff_add: theme_to_ratatui(t.diff_add),
        diff_remove: theme_to_ratatui(t.diff_remove),
        diff_add_dim: theme_to_ratatui(t.diff_added_dimmed),
        diff_remove_dim: theme_to_ratatui(t.diff_removed_dimmed),
        diff_add_word: theme_to_ratatui(t.diff_added_word),
        diff_remove_word: theme_to_ratatui(t.diff_removed_word),
        user_msg_bg: theme_to_ratatui(t.user_message_bg),
        bash_msg_bg: theme_to_ratatui(t.bash_message_bg),
        memory_msg_bg: theme_to_ratatui(t.memory_message_bg),
    }
}

/// Adapt a colour that did **not** come from the theme — the syntax
/// highlighter ships its own palette — to the terminal's colour depth.
///
/// Highlighted code is still colour, and it is the one place in the UI
/// that never passes through a theme slot, so it cannot reach
/// `adapt_for_emit`. Without this hop a syntax-highlighted code block
/// keeps emitting 24-bit RGB under `NO_COLOR`.
pub fn syntax_color(r: u8, g: u8, b: u8) -> Color {
    theme_to_ratatui(crate::ui::color_emit::adapt(
        crate::ui::color_emit::current(),
        crossterm::style::Color::Rgb { r, g, b },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::style::Color as CtColor;

    #[test]
    fn palette_reflects_active_theme_accent() {
        let _g = crate::ui::theme::test_lock();
        // one-dark's accent is #61afef (97, 175, 239).
        let classic = theme::Theme::from_name("one-dark");
        assert!(
            matches!(
                classic.accent,
                CtColor::Rgb {
                    r: 97,
                    g: 175,
                    b: 239
                }
            ),
            "one-dark accent drifted: {:?}",
            classic.accent
        );
        crate::ui::theme::init("one-dark");
        let p = palette();
        // Must not fall back to loud Magenta/Cyan brand hardcodes.
        assert_ne!(p.accent, Color::Cyan);
        assert_ne!(p.accent, Color::Magenta);
        if let Color::Rgb(r, g, b) = p.accent {
            assert_eq!((r, g, b), (97, 175, 239));
        }
    }
}
