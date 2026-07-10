//! Classic theme → ratatui colors for the modern TUI.
//!
//! Modern used to hardcode `Color::Cyan` / `Color::Magenta` for brand
//! highlights. Classic REPL paints prompts, selectors, and badges with
//! [`crate::ui::theme::Theme::accent`] — purple in the default midnight
//! theme (`#A422E1`). Route every modern highlight through this palette
//! so both surfaces stay in sync when the user picks a theme.

use ratatui::style::Color;

use crate::ui::theme;
use crate::ui::tui::theme_to_ratatui;

/// Snapshot of the active classic theme as ratatui colors.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Brand highlight (classic prompt / selector) — purple on midnight.
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
    fn midnight_accent_is_classic_purple() {
        // Classic midnight brand color — the highlight purple.
        let classic = theme::Theme::midnight();
        assert!(
            matches!(
                classic.accent,
                CtColor::Rgb {
                    r: 164,
                    g: 34,
                    b: 225
                }
            ),
            "classic midnight accent drifted: {:?}",
            classic.accent
        );
        // theme_to_ratatui preserves truecolor RGB.
        assert_eq!(
            theme_to_ratatui(CtColor::Rgb {
                r: 164,
                g: 34,
                b: 225
            }),
            Color::Rgb(164, 34, 225)
        );
        crate::ui::theme::init("midnight");
        let p = palette();
        // Under truecolor emit mode this is exact purple; under 256-color
        // it may be Indexed — either way it must not fall back to Cyan.
        assert_ne!(p.accent, Color::Cyan, "modern must not hardcode cyan");
        if let Color::Rgb(r, g, b) = p.accent {
            assert_eq!((r, g, b), (164, 34, 225));
        }
    }
}
