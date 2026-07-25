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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::style::Color as CtColor;

    #[test]
    fn palette_reflects_active_theme_accent() {
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
