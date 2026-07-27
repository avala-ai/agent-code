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
    /// The theme's own background. Not painted directly — badge
    /// foregrounds are chosen against it by [`on_fill`].
    pub bg: Color,
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
        bg: theme_to_ratatui(t.bg),
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

/// Foreground for text drawn on a solid `fill` bar — the permission
/// badge, the mouse selection, the current search match, the code-block
/// language pill.
///
/// Picks whichever of the theme's background and text colours contrasts
/// better with the fill, falling back to black or white when neither
/// clears the readability floor. A fixed choice cannot work: within one
/// theme the accent, warning and error fills sit at different
/// luminances, and across themes the same slot flips from light to
/// dark. Hardcoding black was wrong on light themes; so is always using
/// the theme background — on `solarized-light` that is cream on
/// `#b58900`, about 3:1.
///
/// Colours whose luminance cannot be determined (`Reset`, and the
/// indexed values the 256-colour downgrade produces) fall back to the
/// background. Under `NO_COLOR` that is the case that applies: every
/// candidate is already `Reset`, so the fill and its text collapse
/// together and the row stays readable.
pub fn on_fill(fill: Color) -> Color {
    /// WCAG AA for normal text. Badge text is short and usually bold,
    /// but these bars carry the words the user must act on.
    const MIN_CONTRAST: f32 = 4.5;

    let p = palette();
    let Some(fill_l) = luminance(fill) else {
        return p.bg;
    };

    // Prefer the theme's own colours, so a badge keeps the theme's
    // character wherever they are legible.
    let best = [p.bg, p.text]
        .into_iter()
        .filter_map(|c| luminance(c).map(|l| (contrast(fill_l, l), c)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((ratio, c)) = best
        && ratio >= MIN_CONTRAST
    {
        return c;
    }

    // Neither reaches the floor — `solarized-light`'s warning fill is
    // the worked example, where cream manages 3:1 and the mid-grey text
    // slot is worse still. Fall back to the achromatic pole, which
    // always maximizes contrast, and take the same adaptation hop as
    // every other colour so `NO_COLOR` still strips it.
    if contrast(fill_l, 0.0) >= contrast(fill_l, 1.0) {
        adapt_rgb(0, 0, 0)
    } else {
        adapt_rgb(255, 255, 255)
    }
}

/// WCAG contrast ratio between two relative luminances.
pub(super) fn contrast(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// WCAG relative luminance, or `None` when the colour carries no
/// inspectable RGB value.
pub(super) fn luminance(c: Color) -> Option<f32> {
    let (r, g, b) = rgb_of(c)?;
    let ch = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b))
}

/// Approximate RGB for a ratatui colour. The named ANSI slots use the
/// canonical xterm values, so the ANSI-16 accessibility themes — whose
/// palettes are named rather than RGB — still get a real contrast
/// decision instead of the fallback.
fn rgb_of(c: Color) -> Option<(u8, u8, u8)> {
    Some(match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 0, 0),
        Color::Green => (0, 205, 0),
        Color::Yellow => (205, 205, 0),
        Color::Blue => (0, 0, 238),
        Color::Magenta => (205, 0, 205),
        Color::Cyan => (0, 205, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (127, 127, 127),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (92, 92, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        _ => return None,
    })
}

/// Adapt a colour that did **not** come from the theme — the syntax
/// highlighter ships its own palette — to the terminal's colour depth.
///
/// Highlighted code is still colour, and it is the one place in the UI
/// that never passes through a theme slot, so it cannot reach
/// `adapt_for_emit`. Without this hop a syntax-highlighted code block
/// keeps emitting 24-bit RGB under `NO_COLOR`.
pub fn syntax_color(r: u8, g: u8, b: u8) -> Color {
    adapt_rgb(r, g, b)
}

/// Put a literal RGB triple through the emit-mode adaptation every
/// palette slot goes through, so it downgrades and disappears with the
/// rest of the UI instead of being pinned to 24-bit colour.
fn adapt_rgb(r: u8, g: u8, b: u8) -> Color {
    theme_to_ratatui(crate::ui::color_emit::adapt(
        crate::ui::color_emit::current(),
        crossterm::style::Color::Rgb { r, g, b },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::style::Color as CtColor;

    /// Every filled badge has to stay readable on every built-in theme.
    ///
    /// Both of the fixed choices this replaced fail here: black text is
    /// unreadable on the dark fills a light theme uses, and the theme
    /// background is unreadable on `solarized-light`'s warning fill
    /// (about 3:1, against 6.5:1 for its text colour).
    #[test]
    fn filled_badges_stay_readable_on_every_theme() {
        let _g = crate::ui::theme::test_lock();
        for name in theme::Theme::all_names() {
            crate::ui::theme::init(&name);
            let p = palette();
            for (slot, fill) in [
                ("accent", p.accent),
                ("warning", p.warning),
                ("error", p.error),
                ("tool", p.tool),
            ] {
                let fg = on_fill(fill);
                let (Some(a), Some(b)) = (luminance(fg), luminance(fill)) else {
                    continue;
                };
                let ratio = contrast(a, b);
                assert!(
                    ratio >= 4.5,
                    "{name}: {slot} badge only reaches {ratio:.2}:1 \
                     (fg {fg:?} on fill {fill:?})"
                );
            }
        }
        crate::ui::theme::init("one-dark");
    }

    /// The specific regression the contrast pick exists to prevent. On
    /// `solarized-light` *both* theme candidates fail against the warning
    /// fill — cream reaches about 3:1 and the mid-grey text slot is
    /// worse — so the badge must take the achromatic fallback rather
    /// than settle for the better of two unreadable options.
    #[test]
    fn light_theme_warning_badge_does_not_use_the_page_background() {
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("solarized-light");
        let p = palette();
        let fg = on_fill(p.warning);
        assert_ne!(
            fg, p.bg,
            "cream on #b58900 is the low-contrast pairing this avoids"
        );
        let ratio = contrast(luminance(fg).unwrap(), luminance(p.warning).unwrap());
        assert!(ratio >= 4.5, "warning badge only reaches {ratio:.2}:1");
        crate::ui::theme::init("one-dark");
    }

    /// Themes whose own colours *do* clear the floor keep them — the
    /// fallback is a floor, not a replacement for the palette.
    #[test]
    fn dark_theme_badge_keeps_the_theme_background() {
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let p = palette();
        assert_eq!(on_fill(p.warning), p.bg);
    }

    /// Under `NO_COLOR` there is nothing to contrast against; the fill
    /// and its text both collapse to the terminal default.
    #[test]
    fn on_fill_is_uncoloured_in_mono() {
        use crate::ui::color_emit::{EmitMode, pin_mode};
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let _mode = pin_mode(EmitMode::Mono);
        assert_eq!(on_fill(palette().warning), Color::Reset);
    }

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
