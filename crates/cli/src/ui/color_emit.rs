//! Terminal color-emission downgrade.
//!
//! The theme palette stores every brand color as full RGB. Some
//! terminals — notably Apple Terminal — mishandle 24-bit color escape
//! sequences and render corrupted output. The code in this module
//! detects those terminals at startup and adapts every `Color::Rgb`
//! emission to a safe alternative (256-color cube + greyscale, or the
//! 16-color ANSI palette as a final fallback).
//!
//! The downgrade only affects emission. The `Theme` struct still holds
//! the original RGB values, so swapping themes or running on a
//! truecolor terminal continues to use the full palette.

use std::sync::OnceLock;

use crossterm::style::Color;

/// User-facing override env var. Accepts `truecolor`, `ansi256`,
/// `ansi16`, `mono`, or `auto` (the default — same as not setting it).
pub const EMIT_MODE_ENV: &str = "AGENT_CODE_COLOR_MODE";

/// Color-emission mode. Controls how `Color::Rgb` values are rendered
/// to the terminal: pass-through, quantized to ANSI 256, coerced to
/// the 16-color ANSI palette, or stripped entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitMode {
    /// 24-bit truecolor — emits `\x1b[38;2;R;G;Bm`.
    Truecolor,
    /// 256-color cube + greyscale ramp — emits `\x1b[38;5;Nm`.
    Ansi256,
    /// 16 standard ANSI colors only — emits `\x1b[3Nm` / `\x1b[9Nm`.
    Ansi16,
    /// No color at all — every foreground/background collapses to the
    /// terminal default (`\x1b[39m` / `\x1b[49m`). This is what
    /// <https://no-color.org/> asks for: the spec says *no* color, not
    /// "fewer colors". Text attributes (bold, dim, italic, underline)
    /// survive, so structure is still legible without hue.
    Mono,
}

static EMIT_MODE: OnceLock<EmitMode> = OnceLock::new();

// Test-only emit-mode override. Thread-local rather than global: the
// test harness runs each test on its own thread, so a test that pins
// Mono cannot bleed into a frame another test is rendering in parallel.
#[cfg(test)]
thread_local! {
    static TEST_MODE: std::cell::Cell<Option<EmitMode>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn test_override() -> Option<EmitMode> {
    TEST_MODE.with(std::cell::Cell::get)
}

#[cfg(not(test))]
fn test_override() -> Option<EmitMode> {
    None
}

/// Pin the emit mode on this thread until the returned guard drops.
///
/// Without this a test cannot reach any mode but truecolor (see
/// [`current`]), which makes "does the UI actually go monochrome under
/// `NO_COLOR`?" untestable — the one question the palette routing exists
/// to answer.
#[cfg(test)]
#[must_use]
pub(crate) fn pin_mode(mode: EmitMode) -> ModeGuard {
    TEST_MODE.with(|m| m.set(Some(mode)));
    ModeGuard
}

/// Restores the ambient emit mode on drop, including on panic — a failed
/// assertion must not leave the rest of the thread's tests in Mono.
#[cfg(test)]
pub(crate) struct ModeGuard;

#[cfg(test)]
impl Drop for ModeGuard {
    fn drop(&mut self) {
        TEST_MODE.with(|m| m.set(None));
    }
}

/// Returns the cached emit mode, initializing it on first call.
///
/// Under `cfg(test)` this is pinned to [`EmitMode::Truecolor`] unless a
/// test asked for something else via [`pin_mode`]: rendered frames — the
/// golden snapshots above all — must be identical on every runner, not a
/// function of the ambient `NO_COLOR` / `CLICOLOR` / `FORCE_COLOR` /
/// `TERM` environment the tests happen to inherit. Detection itself
/// stays covered through the pure [`detect_from_env`].
pub fn current() -> EmitMode {
    if let Some(mode) = test_override() {
        return mode;
    }
    if cfg!(test) {
        return EmitMode::Truecolor;
    }
    *EMIT_MODE.get_or_init(detect_uncached)
}

fn detect_uncached() -> EmitMode {
    detect_from_env(|name| std::env::var(name).ok())
}

/// Pure-function form of [`detect_uncached`] for testability. The
/// closure receives the env-var name and returns the value (or `None`
/// if unset / not unicode).
pub fn detect_from_env<F>(get: F) -> EmitMode
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(raw) = get(EMIT_MODE_ENV)
        && let Some(mode) = parse_mode_override(&raw)
    {
        return mode;
    }

    // `FORCE_COLOR` is the escape hatch users reach for when the
    // heuristics guess too low (CI logs, piped-but-colour-capable
    // viewers). It outranks the disable switches below, which is the
    // whole point of "force".
    if let Some(raw) = get("FORCE_COLOR")
        && let Some(mode) = parse_force_color(&raw)
    {
        return mode;
    }

    // https://no-color.org/ — "when present and not an empty string
    // (regardless of its value), prevent the addition of ANSI color".
    if get("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return EmitMode::Mono;
    }

    // The older BSD convention: CLICOLOR=0 means the same thing.
    if get("CLICOLOR").is_some_and(|v| v.trim() == "0") {
        return EmitMode::Mono;
    }

    if get("TERM_PROGRAM").is_some_and(|v| v == "Apple_Terminal") {
        return EmitMode::Ansi256;
    }

    if let Some(colorterm) = get("COLORTERM") {
        let lower = colorterm.to_ascii_lowercase();
        if lower == "truecolor" || lower == "24bit" {
            return EmitMode::Truecolor;
        }
    }

    if let Some(term) = get("TERM") {
        // Multiplexer wrappers commonly cap out at 256 colors even when
        // the host terminal is truecolor — assume the safe ceiling
        // unless the user has explicitly advertised truecolor via
        // COLORTERM.
        if term == "screen-256color" || term == "tmux-256color" {
            return EmitMode::Ansi256;
        }
    }

    EmitMode::Truecolor
}

fn parse_mode_override(raw: &str) -> Option<EmitMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "truecolor" | "24bit" => Some(EmitMode::Truecolor),
        "ansi256" | "256" => Some(EmitMode::Ansi256),
        "ansi16" | "16" => Some(EmitMode::Ansi16),
        "mono" | "none" | "off" | "0" => Some(EmitMode::Mono),
        "auto" | "" => None,
        _ => None,
    }
}

/// Parse `FORCE_COLOR` using the level convention shared across the
/// ecosystem: `0` disables, `1` = 16 colors, `2` = 256, `3` = 24-bit.
/// A bare (empty) or `true` value means "colour, as good as you have".
fn parse_force_color(raw: &str) -> Option<EmitMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "0" | "false" => Some(EmitMode::Mono),
        "1" => Some(EmitMode::Ansi16),
        "2" => Some(EmitMode::Ansi256),
        "" | "3" | "true" => Some(EmitMode::Truecolor),
        _ => None,
    }
}

/// Adapt a single color to the configured emit mode. Non-RGB colors
/// pass through unchanged on `Truecolor` and `Ansi256` (the named ANSI
/// variants are universal). On `Ansi16`, RGB is quantized to the
/// nearest 16-color slot; `AnsiValue` is also collapsed to its closest
/// 16-color cousin for safety. On `Mono`, every color — named,
/// indexed, or RGB — becomes the terminal default.
pub fn adapt(mode: EmitMode, color: Color) -> Color {
    match (mode, color) {
        (EmitMode::Mono, _) => Color::Reset,
        (EmitMode::Truecolor, c) => c,
        (EmitMode::Ansi256, Color::Rgb { r, g, b }) => {
            Color::AnsiValue(quantize_to_ansi256(r, g, b))
        }
        (EmitMode::Ansi256, c) => c,
        (EmitMode::Ansi16, Color::Rgb { r, g, b }) => quantize_to_ansi16(r, g, b),
        (EmitMode::Ansi16, Color::AnsiValue(idx)) => ansi256_to_ansi16(idx),
        (EmitMode::Ansi16, c) => c,
    }
}

/// Quantize an RGB triple to an ANSI 256-color index.
///
/// The 256-color palette is laid out as:
/// - 0..=15   — system colors (named ANSI 16)
/// - 16..=231 — 6×6×6 RGB cube, index = 16 + 36·R + 6·G + B
/// - 232..=255 — 24-step greyscale ramp
///
/// Greys go to the ramp when all channels match within `GREY_TOL`;
/// everything else maps to the nearest cube cell. The `+ 25` offset
/// in the cube-step rounding centers each bucket so that channel=255
/// rounds to 5 and channel=0 rounds to 0.
pub fn quantize_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    /// Asymmetry threshold for routing to the grey ramp. The ramp has
    /// 24 perceptually-uniform steps; cube greys (where R==G==B) are
    /// coarser. A few-channel jitter still routes to the ramp because
    /// it maps closer to the input luminance than the nearest cube cell.
    const GREY_TOL: u8 = 8;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min <= GREY_TOL {
        // Average channel for the ramp index. 24 ramp slots span
        // 0x08..=0xee in approximate steps of 10 — so quantize by
        // (avg - 8) / 10 clamped to 0..24.
        let avg = u32::from(r) + u32::from(g) + u32::from(b);
        let avg = (avg / 3) as u8;
        if avg < 8 {
            return 16; // closest cube black
        }
        if avg > 238 {
            return 231; // closest cube white
        }
        let step = ((u32::from(avg) - 8) * 24 / 230) as u8;
        return 232 + step.min(23);
    }

    let r6 = cube_step(r);
    let g6 = cube_step(g);
    let b6 = cube_step(b);
    16 + 36 * r6 + 6 * g6 + b6
}

/// Map a 0..=255 channel value to a 0..=5 cube step. The cube levels
/// are 0, 95, 135, 175, 215, 255 — non-linear, so we pick the closest
/// level by midpoint thresholds.
fn cube_step(c: u8) -> u8 {
    // Midpoints between adjacent cube levels.
    if c < 48 {
        0
    } else if c < 115 {
        1
    } else if c < 155 {
        2
    } else if c < 195 {
        3
    } else if c < 235 {
        4
    } else {
        5
    }
}

/// Quantize an RGB triple to the closest 16-color ANSI named color.
/// Uses simple Euclidean distance against canonical xterm RGB values.
pub fn quantize_to_ansi16(r: u8, g: u8, b: u8) -> Color {
    // Canonical xterm-256 entries for the 16 base slots.
    const PALETTE: &[(u8, u8, u8, Color)] = &[
        (0, 0, 0, Color::Black),
        (128, 0, 0, Color::DarkRed),
        (0, 128, 0, Color::DarkGreen),
        (128, 128, 0, Color::DarkYellow),
        (0, 0, 128, Color::DarkBlue),
        (128, 0, 128, Color::DarkMagenta),
        (0, 128, 128, Color::DarkCyan),
        (192, 192, 192, Color::Grey),
        (128, 128, 128, Color::DarkGrey),
        (255, 0, 0, Color::Red),
        (0, 255, 0, Color::Green),
        (255, 255, 0, Color::Yellow),
        (0, 0, 255, Color::Blue),
        (255, 0, 255, Color::Magenta),
        (0, 255, 255, Color::Cyan),
        (255, 255, 255, Color::White),
    ];
    let mut best = Color::White;
    let mut best_d: i32 = i32::MAX;
    for (pr, pg, pb, color) in PALETTE {
        let dr = i32::from(r) - i32::from(*pr);
        let dg = i32::from(g) - i32::from(*pg);
        let db = i32::from(b) - i32::from(*pb);
        let d = dr * dr + dg * dg + db * db;
        if d < best_d {
            best_d = d;
            best = *color;
        }
    }
    best
}

/// Collapse a 256-color index back to its 16-color cousin. Used when
/// `EmitMode::Ansi16` receives an already-quantized `AnsiValue`.
fn ansi256_to_ansi16(idx: u8) -> Color {
    if idx < 16 {
        // Already a 16-color slot — pass through as the matching named
        // variant (callers that emitted `AnsiValue(0..16)` get the same
        // visual).
        return match idx {
            0 => Color::Black,
            1 => Color::DarkRed,
            2 => Color::DarkGreen,
            3 => Color::DarkYellow,
            4 => Color::DarkBlue,
            5 => Color::DarkMagenta,
            6 => Color::DarkCyan,
            7 => Color::Grey,
            8 => Color::DarkGrey,
            9 => Color::Red,
            10 => Color::Green,
            11 => Color::Yellow,
            12 => Color::Blue,
            13 => Color::Magenta,
            14 => Color::Cyan,
            _ => Color::White,
        };
    }
    if idx >= 232 {
        // Greyscale ramp — pick a sensible 16-color grey.
        let step = idx - 232; // 0..=23
        return if step < 6 {
            Color::Black
        } else if step < 12 {
            Color::DarkGrey
        } else if step < 18 {
            Color::Grey
        } else {
            Color::White
        };
    }
    // 6×6×6 cube — invert the index back to channels and quantize.
    let cube = idx - 16;
    let r6 = cube / 36;
    let g6 = (cube / 6) % 6;
    let b6 = cube % 6;
    let levels = [0u8, 95, 135, 175, 215, 255];
    quantize_to_ansi16(
        levels[r6 as usize],
        levels[g6 as usize],
        levels[b6 as usize],
    )
}

/// Format a foreground color as an ANSI escape sequence under the
/// given emit mode. Always returns a complete `\x1b[...m` SGR string.
pub fn format_fg(mode: EmitMode, color: Color) -> String {
    sgr(mode, color, /* bg = */ false)
}

/// Format a background color as an ANSI escape sequence under the
/// given emit mode.
pub fn format_bg(mode: EmitMode, color: Color) -> String {
    sgr(mode, color, /* bg = */ true)
}

fn sgr(mode: EmitMode, color: Color, bg: bool) -> String {
    // Monochrome resets the colour *slot* only. A full `\x1b[0m` here
    // would also drop bold/dim/italic/underline, which is exactly the
    // structure no-color.org expects us to keep.
    if mode == EmitMode::Mono {
        return if bg { "\x1b[49m" } else { "\x1b[39m" }.to_string();
    }
    let adapted = adapt(mode, color);
    let (fg_prefix, bg_prefix) = ("38", "48");
    let prefix = if bg { bg_prefix } else { fg_prefix };
    match adapted {
        Color::Rgb { r, g, b } => format!("\x1b[{prefix};2;{r};{g};{b}m"),
        Color::AnsiValue(n) => format!("\x1b[{prefix};5;{n}m"),
        Color::Reset => "\x1b[0m".to_string(),
        named => named_sgr(named, bg),
    }
}

fn named_sgr(color: Color, bg: bool) -> String {
    let (fg_base, bg_base, bright_fg_base, bright_bg_base) = (30, 40, 90, 100);
    let (offset, bright) = match color {
        Color::Black => (0, false),
        Color::DarkRed => (1, false),
        Color::DarkGreen => (2, false),
        Color::DarkYellow => (3, false),
        Color::DarkBlue => (4, false),
        Color::DarkMagenta => (5, false),
        Color::DarkCyan => (6, false),
        Color::Grey => (7, false),
        Color::DarkGrey => (0, true),
        Color::Red => (1, true),
        Color::Green => (2, true),
        Color::Yellow => (3, true),
        Color::Blue => (4, true),
        Color::Magenta => (5, true),
        Color::Cyan => (6, true),
        Color::White => (7, true),
        _ => return "\x1b[39m".to_string(),
    };
    let base = match (bg, bright) {
        (false, false) => fg_base,
        (true, false) => bg_base,
        (false, true) => bright_fg_base,
        (true, true) => bright_bg_base,
    };
    format!("\x1b[{}m", base + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Quantizer ----

    #[test]
    fn black_quantizes_to_cube_index_16() {
        assert_eq!(quantize_to_ansi256(0, 0, 0), 16);
    }

    #[test]
    fn white_quantizes_to_cube_index_231() {
        assert_eq!(quantize_to_ansi256(255, 255, 255), 231);
    }

    #[test]
    fn mid_grey_routes_to_grey_ramp() {
        // RGB(128,128,128) is dead-center grey — must route to the ramp,
        // not the cube. Step ≈ (128-8) * 24 / 230 ≈ 12, so index 244.
        let idx = quantize_to_ansi256(128, 128, 128);
        assert!(
            (232..=255).contains(&idx),
            "mid-grey {idx} must land in greyscale ramp 232..=255"
        );
        assert!(
            (242..=246).contains(&idx),
            "mid-grey {idx} expected near 244"
        );
    }

    #[test]
    fn pure_red_quantizes_to_196() {
        // Pure red — channels (5, 0, 0) → 16 + 36·5 = 196.
        assert_eq!(quantize_to_ansi256(255, 0, 0), 196);
    }

    #[test]
    fn pure_blue_quantizes_to_21() {
        // Pure blue — channels (0, 0, 5) → 16 + 5 = 21.
        assert_eq!(quantize_to_ansi256(0, 0, 255), 21);
    }

    #[test]
    fn near_grey_with_asymmetry_routes_to_cube() {
        // Channels (120, 128, 135) span 15 — outside grey tolerance,
        // so this must hit the 6×6×6 cube, not the ramp.
        let idx = quantize_to_ansi256(120, 128, 135);
        assert!(
            (16..=231).contains(&idx),
            "asymmetric near-grey {idx} must hit cube, not ramp"
        );
    }

    // ---- Detector ----

    #[test]
    fn apple_terminal_detected_as_ansi256() {
        let env = |name: &str| match name {
            "TERM_PROGRAM" => Some("Apple_Terminal".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Ansi256);
    }

    #[test]
    fn colorterm_truecolor_detected() {
        let env = |name: &str| match name {
            "COLORTERM" => Some("truecolor".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Truecolor);
    }

    #[test]
    fn colorterm_24bit_detected() {
        let env = |name: &str| match name {
            "COLORTERM" => Some("24bit".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Truecolor);
    }

    #[test]
    fn no_color_overrides_apple_terminal() {
        // NO_COLOR (https://no-color.org/) is universal and must win
        // over terminal-program-specific behavior.
        let env = |name: &str| match name {
            "NO_COLOR" => Some("1".to_string()),
            "TERM_PROGRAM" => Some("Apple_Terminal".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Mono);
    }

    // ---- NO_COLOR / CLICOLOR / FORCE_COLOR ----

    #[test]
    fn no_color_selects_mono_not_a_smaller_palette() {
        // The spec says *no* colour. Downgrading to ANSI 16 still emits
        // colour and violates it.
        let env = |name: &str| match name {
            "NO_COLOR" => Some("1".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Mono);
    }

    #[test]
    fn no_color_honours_any_non_empty_value() {
        // "regardless of its value" — only an empty string is ignored.
        for v in ["1", "0", "false", "yes", " "] {
            let env = move |name: &str| match name {
                "NO_COLOR" => Some(v.to_string()),
                _ => None,
            };
            assert_eq!(detect_from_env(env), EmitMode::Mono, "NO_COLOR={v:?}");
        }
        let empty = |name: &str| match name {
            "NO_COLOR" => Some(String::new()),
            _ => None,
        };
        assert_eq!(detect_from_env(empty), EmitMode::Truecolor);
    }

    #[test]
    fn clicolor_zero_selects_mono() {
        let env = |name: &str| match name {
            "CLICOLOR" => Some("0".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Mono);
        // Any other CLICOLOR value is not a disable signal.
        let on = |name: &str| match name {
            "CLICOLOR" => Some("1".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(on), EmitMode::Truecolor);
    }

    #[test]
    fn force_color_overrides_no_color() {
        let env = |name: &str| match name {
            "NO_COLOR" => Some("1".to_string()),
            "FORCE_COLOR" => Some("3".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Truecolor);
    }

    #[test]
    fn force_color_overrides_detection_upward() {
        // Apple Terminal would otherwise be capped at 256 colours.
        let env = |name: &str| match name {
            "TERM_PROGRAM" => Some("Apple_Terminal".to_string()),
            "FORCE_COLOR" => Some("3".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Truecolor);
    }

    #[test]
    fn force_color_levels_map_to_palettes() {
        let with = |v: &'static str| {
            move |name: &str| match name {
                "FORCE_COLOR" => Some(v.to_string()),
                _ => None,
            }
        };
        assert_eq!(detect_from_env(with("0")), EmitMode::Mono);
        assert_eq!(detect_from_env(with("1")), EmitMode::Ansi16);
        assert_eq!(detect_from_env(with("2")), EmitMode::Ansi256);
        assert_eq!(detect_from_env(with("3")), EmitMode::Truecolor);
        assert_eq!(detect_from_env(with("")), EmitMode::Truecolor);
        assert_eq!(detect_from_env(with("true")), EmitMode::Truecolor);
        // Junk falls through to the normal heuristics.
        assert_eq!(detect_from_env(with("banana")), EmitMode::Truecolor);
    }

    #[test]
    fn explicit_override_beats_force_color() {
        let env = |name: &str| match name {
            EMIT_MODE_ENV => Some("mono".to_string()),
            "FORCE_COLOR" => Some("3".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Mono);
    }

    #[test]
    fn mono_strips_every_color_to_the_terminal_default() {
        assert_eq!(
            adapt(EmitMode::Mono, Color::Rgb { r: 1, g: 2, b: 3 }),
            Color::Reset
        );
        assert_eq!(adapt(EmitMode::Mono, Color::Red), Color::Reset);
        assert_eq!(adapt(EmitMode::Mono, Color::AnsiValue(196)), Color::Reset);
    }

    #[test]
    fn mono_emits_default_slots_and_preserves_attributes() {
        // SGR 39 / 49 reset only the foreground / background slot. A
        // blanket `\x1b[0m` would also clear bold/dim/italic/underline,
        // which is the structure a no-colour user still relies on.
        let fg = format_fg(EmitMode::Mono, Color::Rgb { r: 9, g: 9, b: 9 });
        let bg = format_bg(EmitMode::Mono, Color::Rgb { r: 9, g: 9, b: 9 });
        assert_eq!(fg, "\x1b[39m");
        assert_eq!(bg, "\x1b[49m");
        assert!(!fg.contains("[0m"), "must not reset attributes: {fg:?}");
        assert!(!bg.contains("[0m"), "must not reset attributes: {bg:?}");
    }

    #[test]
    fn explicit_override_beats_apple_terminal() {
        let env = |name: &str| match name {
            EMIT_MODE_ENV => Some("truecolor".to_string()),
            "TERM_PROGRAM" => Some("Apple_Terminal".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Truecolor);
    }

    #[test]
    fn explicit_override_beats_no_color() {
        // The user override wins over every heuristic — including
        // NO_COLOR. A user who has set both clearly knows what they
        // want.
        let env = |name: &str| match name {
            EMIT_MODE_ENV => Some("ansi256".to_string()),
            "NO_COLOR" => Some("1".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Ansi256);
    }

    #[test]
    fn screen_256color_detected_as_ansi256() {
        let env = |name: &str| match name {
            "TERM" => Some("screen-256color".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Ansi256);
    }

    #[test]
    fn tmux_256color_detected_as_ansi256() {
        let env = |name: &str| match name {
            "TERM" => Some("tmux-256color".to_string()),
            _ => None,
        };
        assert_eq!(detect_from_env(env), EmitMode::Ansi256);
    }

    #[test]
    fn empty_env_defaults_to_truecolor() {
        let env = |_name: &str| None;
        assert_eq!(detect_from_env(env), EmitMode::Truecolor);
    }

    /// Golden-frame snapshots (and every other colour-asserting test)
    /// depend on this: the emit mode inside the test binary must not
    /// vary with the terminal environment of whoever runs the suite.
    #[test]
    fn current_is_pinned_to_truecolor_under_test() {
        assert_eq!(current(), EmitMode::Truecolor);
    }

    // ---- Format functions ----

    #[test]
    fn format_fg_truecolor_emits_38_2_sequence() {
        let s = format_fg(
            EmitMode::Truecolor,
            Color::Rgb {
                r: 12,
                g: 34,
                b: 56,
            },
        );
        assert_eq!(s, "\x1b[38;2;12;34;56m");
    }

    #[test]
    fn format_fg_ansi256_emits_38_5_sequence() {
        let s = format_fg(EmitMode::Ansi256, Color::Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(s, "\x1b[38;5;196m");
    }

    #[test]
    fn format_bg_ansi256_emits_48_5_sequence() {
        let s = format_bg(EmitMode::Ansi256, Color::Rgb { r: 0, g: 0, b: 255 });
        assert_eq!(s, "\x1b[48;5;21m");
    }

    #[test]
    fn format_fg_ansi16_uses_named_variants() {
        // Pure red under ANSI 16 → bright red (\x1b[91m).
        let s = format_fg(EmitMode::Ansi16, Color::Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(s, "\x1b[91m");
    }

    #[test]
    fn format_fg_ansi16_dark_red_for_mid_red() {
        // RGB(128,0,0) is canonical DarkRed → \x1b[31m.
        let s = format_fg(EmitMode::Ansi16, Color::Rgb { r: 128, g: 0, b: 0 });
        assert_eq!(s, "\x1b[31m");
    }

    #[test]
    fn adapt_passes_named_colors_through_under_ansi256() {
        // Named colors are universal — must not get clobbered.
        assert_eq!(adapt(EmitMode::Ansi256, Color::Red), Color::Red);
        assert_eq!(adapt(EmitMode::Ansi256, Color::DarkBlue), Color::DarkBlue);
    }

    #[test]
    fn adapt_truecolor_is_identity() {
        let rgb = Color::Rgb { r: 1, g: 2, b: 3 };
        assert_eq!(adapt(EmitMode::Truecolor, rgb), rgb);
    }

    #[test]
    fn adapt_ansi256_quantizes_rgb() {
        let rgb = Color::Rgb { r: 255, g: 0, b: 0 };
        assert_eq!(adapt(EmitMode::Ansi256, rgb), Color::AnsiValue(196));
    }

    #[test]
    fn quantize_to_ansi16_known_anchors() {
        assert_eq!(quantize_to_ansi16(0, 0, 0), Color::Black);
        assert_eq!(quantize_to_ansi16(255, 255, 255), Color::White);
        assert_eq!(quantize_to_ansi16(255, 0, 0), Color::Red);
        assert_eq!(quantize_to_ansi16(0, 255, 0), Color::Green);
        assert_eq!(quantize_to_ansi16(0, 0, 255), Color::Blue);
    }
}
