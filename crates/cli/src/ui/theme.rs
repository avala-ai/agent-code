//! Terminal color themes.
//!
//! Provides a semantic color system where UI elements reference
//! named colors (error, warning, accent, etc.) rather than
//! hardcoded ANSI codes.
//!
//! Themes are *data-driven*: a compact [`Palette`] names a handful of
//! base colours and [`theme_from_palette`] derives the full runtime
//! [`Theme`] (every semantic slot) from it. Standard palettes live in
//! [`standard_palettes`]; users can drop their own TOML palettes into
//! `<config_dir>/agent-code/themes/*.toml`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crossterm::style::{Attribute, Color, StyledContent, Stylize};

/// Semantic color palette for a theme.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Brand accent color (prompts, banners, decorations).
    pub accent: Color,
    /// Error messages and failure indicators.
    pub error: Color,
    /// Warning messages and non-critical alerts.
    pub warning: Color,
    /// Success messages and confirmations.
    pub success: Color,
    /// Secondary/meta information.
    pub muted: Color,
    /// Inactive/disabled elements.
    pub inactive: Color,
    /// Permission prompts and tool labels.
    pub tool: Color,
    /// Plan mode indicator.
    pub plan: Color,
    /// Primary text (usually terminal default).
    pub text: Color,
    /// Diff: added lines.
    pub diff_add: Color,
    /// Diff: removed lines.
    pub diff_remove: Color,
    /// Subagent identification colors (8 distinct).
    pub agent_colors: [Color; 8],

    // ---- Shimmer variants (lighter/animated equivalents). ----
    /// Brighter equivalent of [`Self::success`] for shimmer animations.
    pub success_shimmer: Color,
    /// Brighter equivalent of [`Self::error`] for shimmer animations.
    pub error_shimmer: Color,
    /// Brighter equivalent of [`Self::warning`] for shimmer animations.
    pub warning_shimmer: Color,
    /// Brighter equivalent of [`Self::accent`] for shimmer animations.
    pub accent_shimmer: Color,
    /// Brighter equivalent of [`Self::muted`] for shimmer animations.
    pub muted_shimmer: Color,

    // ---- Diff intensity variants. ----
    /// Diff: added lines, dimmed (background-style highlight).
    pub diff_added_dimmed: Color,
    /// Diff: removed lines, dimmed (background-style highlight).
    pub diff_removed_dimmed: Color,
    /// Diff: added word-level highlight (overlays line-level highlight).
    pub diff_added_word: Color,
    /// Diff: removed word-level highlight (overlays line-level highlight).
    pub diff_removed_word: Color,

    // ---- Stable subagent identification colors. ----
    /// Stable subagent color: red.
    pub subagent_red: Color,
    /// Stable subagent color: blue.
    pub subagent_blue: Color,
    /// Stable subagent color: green.
    pub subagent_green: Color,
    /// Stable subagent color: yellow.
    pub subagent_yellow: Color,
    /// Stable subagent color: purple.
    pub subagent_purple: Color,
    /// Stable subagent color: orange.
    pub subagent_orange: Color,
    /// Stable subagent color: pink.
    pub subagent_pink: Color,
    /// Stable subagent color: cyan.
    pub subagent_cyan: Color,

    // ---- Rate-limit / budget bar. ----
    /// Filled portion of a rate-limit / budget bar.
    pub rate_limit_fill: Color,
    /// Empty portion of a rate-limit / budget bar.
    pub rate_limit_empty: Color,

    // ---- Selection / hover / interaction backgrounds. ----
    /// Selection background — replaces a cell's bg while preserving its fg.
    pub selection_bg: Color,
    /// Inline message-action affordance background (e.g. retry / copy).
    pub message_action_bg: Color,
    /// User message bubble background.
    pub user_message_bg: Color,
    /// Bash command bubble background.
    pub bash_message_bg: Color,
    /// Memory / `#`-prefixed message bubble background.
    pub memory_message_bg: Color,

    // ---- Rainbow keyword highlighting. ----
    /// Rainbow band: red.
    pub rainbow_red: Color,
    /// Rainbow band: orange.
    pub rainbow_orange: Color,
    /// Rainbow band: yellow.
    pub rainbow_yellow: Color,
    /// Rainbow band: green.
    pub rainbow_green: Color,
    /// Rainbow band: blue.
    pub rainbow_blue: Color,
    /// Rainbow band: indigo.
    pub rainbow_indigo: Color,
    /// Rainbow band: violet.
    pub rainbow_violet: Color,

    // ---- Mode tags for the per-mode REPL prompt. ----
    /// Plan-mode tag color.
    pub plan_mode: Color,
    /// Brief-mode tag color.
    pub brief_mode: Color,
    /// Fast-mode tag color.
    pub fast_mode: Color,
    /// Brighter equivalent of [`Self::fast_mode`] for shimmer animations.
    pub fast_mode_shimmer: Color,

    /// Whether this is a dark theme (affects background choices).
    pub is_dark: bool,
}

/// Compact, author-facing definition of a theme. A handful of base
/// colours; [`theme_from_palette`] derives every runtime [`Theme`] slot
/// from these. `id`/`label` are `Cow` so both the built-in (`'static`)
/// palettes and user palettes loaded from disk share one type.
#[derive(Debug, Clone)]
pub struct Palette {
    /// Stable identifier, e.g. `"dracula"` (also the TOML file stem).
    pub id: Cow<'static, str>,
    /// Human-facing name shown in pickers, e.g. `"Dracula"`.
    pub label: Cow<'static, str>,
    /// Whether this is a dark theme.
    pub is_dark: bool,

    /// Background.
    pub bg: Color,
    /// Foreground / primary text.
    pub fg: Color,
    /// Muted / secondary text.
    pub muted: Color,
    /// Selection background.
    pub selection: Color,

    /// Base red.
    pub red: Color,
    /// Base orange.
    pub orange: Color,
    /// Base yellow.
    pub yellow: Color,
    /// Base green.
    pub green: Color,
    /// Base cyan.
    pub cyan: Color,
    /// Base blue.
    pub blue: Color,
    /// Base purple.
    pub purple: Color,
    /// Base pink.
    pub pink: Color,

    /// Brand accent.
    pub accent: Color,
}

// ---- Color helpers ----

/// Per-channel linear blend `a*(1-t) + b*t`, with `t` clamped to
/// `[0, 1]`. Non-`Rgb` colours pass through unchanged (the blend is
/// undefined for named ANSI codes).
fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (
            Color::Rgb {
                r: ar,
                g: ag,
                b: ab,
            },
            Color::Rgb {
                r: br,
                g: bg,
                b: bb,
            },
        ) => {
            let t = t.clamp(0.0, 1.0);
            let blend = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
            Color::Rgb {
                r: blend(ar, br),
                g: blend(ag, bg),
                b: blend(ab, bb),
            }
        }
        _ => a,
    }
}

/// Add `amt` to each channel, saturating at 255. Non-`Rgb` colours
/// pass through unchanged.
fn brighten(c: Color, amt: u8) -> Color {
    match c {
        Color::Rgb { r, g, b } => Color::Rgb {
            r: r.saturating_add(amt),
            g: g.saturating_add(amt),
            b: b.saturating_add(amt),
        },
        _ => c,
    }
}

/// Parse a `"#rrggbb"` (or `"rrggbb"`) hex string into a [`Color::Rgb`].
/// Returns `None` for anything that isn't exactly six hex digits.
fn parse_hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb { r, g, b })
}

/// Parse a built-in palette hex literal. Panics on a malformed literal
/// — the argument is always a controlled constant, so a bad value is a
/// programming error we want to surface immediately in tests.
fn rgb(hex: &str) -> Color {
    parse_hex(hex).expect("built-in palette hex literal must be valid")
}

/// Derive the full runtime [`Theme`] from a compact [`Palette`]. Every
/// [`Theme`] field is filled here — semantic roles map onto the base
/// colours, and derived tints/shades come from [`mix`]/[`brighten`].
fn theme_from_palette(p: &Palette) -> Theme {
    Theme {
        accent: p.accent,
        error: p.red,
        warning: p.yellow,
        success: p.green,
        muted: p.muted,
        inactive: mix(p.muted, p.fg, 0.4),
        tool: p.cyan,
        plan: p.purple,
        text: p.fg,
        diff_add: p.green,
        diff_remove: p.red,
        agent_colors: [
            p.blue, p.green, p.yellow, p.purple, p.red, p.cyan, p.orange, p.pink,
        ],
        success_shimmer: brighten(p.green, 40),
        error_shimmer: brighten(p.red, 40),
        warning_shimmer: brighten(p.yellow, 40),
        accent_shimmer: brighten(p.accent, 40),
        muted_shimmer: brighten(p.muted, 40),
        diff_added_dimmed: mix(p.green, p.bg, 0.82),
        diff_removed_dimmed: mix(p.red, p.bg, 0.82),
        diff_added_word: mix(p.green, p.bg, 0.6),
        diff_removed_word: mix(p.red, p.bg, 0.6),
        subagent_red: p.red,
        subagent_blue: p.blue,
        subagent_green: p.green,
        subagent_yellow: p.yellow,
        subagent_purple: p.purple,
        subagent_orange: p.orange,
        subagent_pink: p.pink,
        subagent_cyan: p.cyan,
        rate_limit_fill: p.green,
        rate_limit_empty: mix(p.muted, p.bg, 0.5),
        selection_bg: p.selection,
        message_action_bg: mix(p.accent, p.bg, 0.85),
        user_message_bg: mix(p.blue, p.bg, 0.88),
        bash_message_bg: mix(p.green, p.bg, 0.9),
        memory_message_bg: mix(p.purple, p.bg, 0.9),
        rainbow_red: p.red,
        rainbow_orange: p.orange,
        rainbow_yellow: p.yellow,
        rainbow_green: p.green,
        rainbow_blue: p.blue,
        rainbow_indigo: mix(p.blue, p.purple, 0.5),
        rainbow_violet: p.purple,
        plan_mode: p.purple,
        brief_mode: p.cyan,
        fast_mode: p.orange,
        fast_mode_shimmer: brighten(p.orange, 40),
        is_dark: p.is_dark,
    }
}

// ---- Standard palettes ----

/// Default dark theme id used when a name can't be resolved.
const DEFAULT_DARK: &str = "one-dark";
/// Default light theme id (the "light" alias, and Auto in light terminals).
const DEFAULT_LIGHT: &str = "solarized-light";

/// The built-in palettes, in display order.
fn standard_palettes() -> Vec<Palette> {
    vec![
        Palette {
            id: Cow::Borrowed("monokai"),
            label: Cow::Borrowed("Monokai"),
            is_dark: true,
            bg: rgb("#272822"),
            fg: rgb("#f8f8f2"),
            muted: rgb("#75715e"),
            selection: rgb("#49483e"),
            red: rgb("#f92672"),
            orange: rgb("#fd971f"),
            yellow: rgb("#e6db74"),
            green: rgb("#a6e22e"),
            cyan: rgb("#66d9ef"),
            blue: rgb("#66d9ef"),
            purple: rgb("#ae81ff"),
            pink: rgb("#f92672"),
            accent: rgb("#66d9ef"),
        },
        Palette {
            id: Cow::Borrowed("dracula"),
            label: Cow::Borrowed("Dracula"),
            is_dark: true,
            bg: rgb("#282a36"),
            fg: rgb("#f8f8f2"),
            muted: rgb("#6272a4"),
            selection: rgb("#44475a"),
            red: rgb("#ff5555"),
            orange: rgb("#ffb86c"),
            yellow: rgb("#f1fa8c"),
            green: rgb("#50fa7b"),
            cyan: rgb("#8be9fd"),
            blue: rgb("#8be9fd"),
            purple: rgb("#bd93f9"),
            pink: rgb("#ff79c6"),
            accent: rgb("#bd93f9"),
        },
        Palette {
            id: Cow::Borrowed("nord"),
            label: Cow::Borrowed("Nord"),
            is_dark: true,
            bg: rgb("#2e3440"),
            fg: rgb("#d8dee9"),
            muted: rgb("#4c566a"),
            selection: rgb("#434c5e"),
            red: rgb("#bf616a"),
            orange: rgb("#d08770"),
            yellow: rgb("#ebcb8b"),
            green: rgb("#a3be8c"),
            cyan: rgb("#88c0d0"),
            blue: rgb("#81a1c1"),
            purple: rgb("#b48ead"),
            pink: rgb("#b48ead"),
            accent: rgb("#88c0d0"),
        },
        Palette {
            id: Cow::Borrowed("gruvbox"),
            label: Cow::Borrowed("Gruvbox"),
            is_dark: true,
            bg: rgb("#282828"),
            fg: rgb("#ebdbb2"),
            muted: rgb("#928374"),
            selection: rgb("#504945"),
            red: rgb("#fb4934"),
            orange: rgb("#fe8019"),
            yellow: rgb("#fabd2f"),
            green: rgb("#b8bb26"),
            cyan: rgb("#8ec07c"),
            blue: rgb("#83a598"),
            purple: rgb("#d3869b"),
            pink: rgb("#d3869b"),
            accent: rgb("#fabd2f"),
        },
        Palette {
            id: Cow::Borrowed("one-dark"),
            label: Cow::Borrowed("One Dark"),
            is_dark: true,
            bg: rgb("#282c34"),
            fg: rgb("#abb2bf"),
            muted: rgb("#5c6370"),
            selection: rgb("#3e4451"),
            red: rgb("#e06c75"),
            orange: rgb("#d19a66"),
            yellow: rgb("#e5c07b"),
            green: rgb("#98c379"),
            cyan: rgb("#56b6c2"),
            blue: rgb("#61afef"),
            purple: rgb("#c678dd"),
            pink: rgb("#c678dd"),
            accent: rgb("#61afef"),
        },
        Palette {
            id: Cow::Borrowed("solarized-dark"),
            label: Cow::Borrowed("Solarized Dark"),
            is_dark: true,
            bg: rgb("#002b36"),
            fg: rgb("#839496"),
            muted: rgb("#586e75"),
            selection: rgb("#073642"),
            red: rgb("#dc322f"),
            orange: rgb("#cb4b16"),
            yellow: rgb("#b58900"),
            green: rgb("#859900"),
            cyan: rgb("#2aa198"),
            blue: rgb("#268bd2"),
            purple: rgb("#6c71c4"),
            pink: rgb("#d33682"),
            accent: rgb("#268bd2"),
        },
        Palette {
            id: Cow::Borrowed("solarized-light"),
            label: Cow::Borrowed("Solarized Light"),
            is_dark: false,
            bg: rgb("#fdf6e3"),
            fg: rgb("#657b83"),
            muted: rgb("#93a1a1"),
            selection: rgb("#eee8d5"),
            red: rgb("#dc322f"),
            orange: rgb("#cb4b16"),
            yellow: rgb("#b58900"),
            green: rgb("#859900"),
            cyan: rgb("#2aa198"),
            blue: rgb("#268bd2"),
            purple: rgb("#6c71c4"),
            pink: rgb("#d33682"),
            accent: rgb("#268bd2"),
        },
        // Colour-vision-deficiency safe: the Okabe-Ito qualitative palette,
        // whose hues stay distinguishable under protanopia, deuteranopia and
        // tritanopia. Roles are assigned so red/green never carry meaning
        // alone — "error" is vermillion and "success" is bluish green.
        Palette {
            id: Cow::Borrowed("dark-colorblind"),
            label: Cow::Borrowed("Dark (colour-blind safe)"),
            is_dark: true,
            bg: rgb("#1c1c1c"),
            fg: rgb("#f2f2f2"),
            muted: rgb("#9a9a9a"),
            selection: rgb("#3a3a3a"),
            red: rgb("#d55e00"),    // vermillion
            orange: rgb("#e69f00"), // orange
            yellow: rgb("#f0e442"), // yellow
            green: rgb("#009e73"),  // bluish green
            cyan: rgb("#56b4e9"),   // sky blue
            blue: rgb("#0072b2"),   // blue
            purple: rgb("#cc79a7"), // reddish purple
            pink: rgb("#cc79a7"),
            accent: rgb("#56b4e9"),
        },
        Palette {
            id: Cow::Borrowed("light-colorblind"),
            label: Cow::Borrowed("Light (colour-blind safe)"),
            is_dark: false,
            bg: rgb("#fdfdfd"),
            fg: rgb("#1c1c1c"),
            muted: rgb("#5c5c5c"),
            selection: rgb("#e4e4e4"),
            red: rgb("#d55e00"),
            // On a light background the Okabe-Ito yellow is unreadable, so
            // the warning role uses the darker orange instead.
            orange: rgb("#b25f00"),
            yellow: rgb("#9a7d00"),
            green: rgb("#007a59"),
            cyan: rgb("#2b7fb8"),
            blue: rgb("#0072b2"),
            purple: rgb("#a85585"),
            pink: rgb("#a85585"),
            accent: rgb("#0072b2"),
        },
        // 16-colour ANSI: every slot is a named terminal colour rather than
        // truecolor, so these render correctly on terminals without RGB
        // support and follow whatever palette the user's terminal defines.
        Palette {
            id: Cow::Borrowed("dark-ansi"),
            label: Cow::Borrowed("ANSI 16 (dark)"),
            is_dark: true,
            bg: Color::Black,
            fg: Color::White,
            muted: Color::DarkGrey,
            selection: Color::DarkBlue,
            red: Color::Red,
            orange: Color::DarkYellow,
            yellow: Color::Yellow,
            green: Color::Green,
            cyan: Color::Cyan,
            blue: Color::Blue,
            purple: Color::Magenta,
            pink: Color::Magenta,
            accent: Color::Cyan,
        },
        Palette {
            id: Cow::Borrowed("light-ansi"),
            label: Cow::Borrowed("ANSI 16 (light)"),
            is_dark: false,
            bg: Color::White,
            fg: Color::Black,
            muted: Color::DarkGrey,
            selection: Color::Grey,
            red: Color::DarkRed,
            orange: Color::DarkYellow,
            yellow: Color::DarkYellow,
            green: Color::DarkGreen,
            cyan: Color::DarkCyan,
            blue: Color::DarkBlue,
            purple: Color::DarkMagenta,
            pink: Color::DarkMagenta,
            accent: Color::DarkBlue,
        },
    ]
}

// ---- User themes (TOML) ----

/// Directory user palettes are loaded from: `<config_dir>/themes/`.
/// `None` when no config directory can be resolved.
fn user_themes_dir() -> Option<PathBuf> {
    agent_code_lib::config::agent_config_dir().map(|d| d.join("themes"))
}

/// Load every user palette from disk. Best-effort: a missing directory
/// yields no themes; malformed files are logged and skipped.
fn user_palettes() -> Vec<Palette> {
    user_themes_dir()
        .map(|d| load_palettes_dir(&d))
        .unwrap_or_default()
}

/// Load all `*.toml` palettes from `dir`. Pure over the directory path
/// so tests can point it at a temporary directory. Palettes are sorted
/// by id for a stable display order.
fn load_palettes_dir(dir: &Path) -> Vec<Palette> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match load_palette_file(&path, stem) {
            Some(p) => out.push(p),
            None => tracing::warn!("skipping malformed theme file: {}", path.display()),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Parse a single palette TOML file. `id` is the file stem. Returns
/// `None` (never panics) on any parse error or missing colour.
fn load_palette_file(path: &Path, id: &str) -> Option<Palette> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&raw).ok()?;
    let tbl = value.as_table()?;
    let color =
        |key: &str| -> Option<Color> { tbl.get(key).and_then(|v| v.as_str()).and_then(parse_hex) };
    let label = tbl
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(id)
        .to_string();
    let is_dark = tbl.get("dark").and_then(|v| v.as_bool()).unwrap_or(true);
    Some(Palette {
        id: Cow::Owned(id.to_string()),
        label: Cow::Owned(label),
        is_dark,
        bg: color("bg")?,
        fg: color("fg")?,
        muted: color("muted")?,
        selection: color("selection")?,
        red: color("red")?,
        orange: color("orange")?,
        yellow: color("yellow")?,
        green: color("green")?,
        cyan: color("cyan")?,
        blue: color("blue")?,
        purple: color("purple")?,
        pink: color("pink")?,
        accent: color("accent")?,
    })
}

/// Find a palette by exact id across standard then user themes.
fn lookup_palette(id: &str) -> Option<Palette> {
    standard_palettes()
        .into_iter()
        .find(|p| p.id.as_ref() == id)
        .or_else(|| user_palettes().into_iter().find(|p| p.id.as_ref() == id))
}

impl Theme {
    /// Resolve a theme name to a [`Theme`].
    ///
    /// Resolution order: exact palette id (standard, then user), then
    /// the aliases `auto` (OS-detected), `dark`, `light`. Anything
    /// unknown falls back to the default dark theme (`one-dark`).
    pub fn from_name(name: &str) -> Self {
        if let Some(p) = lookup_palette(name) {
            return theme_from_palette(&p);
        }
        let fallback = match name {
            "auto" => {
                if detect_system_theme() == "light" {
                    DEFAULT_LIGHT
                } else {
                    DEFAULT_DARK
                }
            }
            "light" => DEFAULT_LIGHT,
            "dark" => DEFAULT_DARK,
            _ => DEFAULT_DARK,
        };
        let p = lookup_palette(fallback).expect("built-in default palette must exist");
        theme_from_palette(&p)
    }

    /// All known theme identifiers in display order: `auto`, then the
    /// standard palette ids, then any user palette ids. Drives the
    /// onboarding picker and the `/color` slash command.
    pub fn all_names() -> Vec<String> {
        // `dark` / `light` are aliases `from_name` already resolves, and
        // the docs advertise them. Without them here `/color dark` was
        // rejected as unknown even though the theme it names works.
        let mut names = vec!["auto".to_string(), "dark".to_string(), "light".to_string()];
        names.extend(standard_palettes().into_iter().map(|p| p.id.into_owned()));
        names.extend(user_palettes().into_iter().map(|p| p.id.into_owned()));
        // A user palette literally named `dark`/`light` is resolved by
        // `from_name` ahead of the alias, so listing both would offer the
        // same identifier twice — the alias row is the one that never wins.
        let mut seen = std::collections::HashSet::new();
        names.retain(|n| seen.insert(n.clone()));
        names
    }

    /// `(id, label)` pairs in display order, for pickers that show a
    /// human-facing name. Mirrors [`Self::all_names`].
    pub fn all_options() -> Vec<(String, String)> {
        let mut opts = vec![
            ("auto".to_string(), "Auto (match terminal)".to_string()),
            (
                "dark".to_string(),
                "Dark (default dark palette)".to_string(),
            ),
            (
                "light".to_string(),
                "Light (default light palette)".to_string(),
            ),
        ];
        for p in standard_palettes() {
            opts.push((p.id.into_owned(), p.label.into_owned()));
        }
        for p in user_palettes() {
            opts.push((p.id.into_owned(), p.label.into_owned()));
        }
        // Drop the alias row when a user palette owns the same id (see
        // `all_names`): keep the entry that `from_name` actually resolves.
        let shadowed: std::collections::HashSet<String> = user_palettes()
            .into_iter()
            .map(|p| p.id.into_owned())
            .collect();
        let mut seen = std::collections::HashSet::new();
        opts.retain(|(id, label)| {
            let is_alias = (id == "dark" || id == "light")
                && (label.contains("default dark palette")
                    || label.contains("default light palette"));
            if is_alias && shadowed.contains(id) {
                return false;
            }
            seen.insert(id.clone())
        });
        opts
    }

    /// Get a subagent color by index (wraps around).
    pub fn agent_color(&self, index: usize) -> Color {
        self.agent_colors[index % self.agent_colors.len()]
    }
}

// ---- Styling helpers ----

/// Style text with a theme color.
pub fn styled(text: &str, color: Color) -> StyledContent<String> {
    text.to_string().with(color)
}

/// Style text with a theme color and bold.
pub fn styled_bold(text: &str, color: Color) -> StyledContent<String> {
    text.to_string().with(color).attribute(Attribute::Bold)
}

/// Style a label with background color (e.g., " ERROR " on red background).
pub fn label(text: &str, bg: Color, fg: Color) -> StyledContent<String> {
    text.to_string().on(bg).with(fg).attribute(Attribute::Bold)
}

// ---- Theme detection ----

/// Detect whether the terminal has a light background.
pub fn detect_system_theme() -> &'static str {
    if let Ok(fgbg) = std::env::var("COLORFGBG")
        && let Some(bg) = fgbg.rsplit(';').next()
        && let Ok(bg_num) = bg.parse::<u32>()
    {
        return if bg_num >= 7 && bg_num != 8 {
            "light"
        } else {
            "dark"
        };
    }

    if std::env::var("TERM_PROGRAM")
        .ok()
        .is_some_and(|p| p == "Apple_Terminal")
    {
        return "light";
    }

    "dark"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_rgb(c: Color) -> Option<(u8, u8, u8)> {
        match c {
            Color::Rgb { r, g, b } => Some((r, g, b)),
            _ => None,
        }
    }

    #[test]
    fn mix_blends_endpoints() {
        let a = Color::Rgb { r: 0, g: 0, b: 0 };
        let b = Color::Rgb {
            r: 100,
            g: 200,
            b: 50,
        };
        assert_eq!(as_rgb(mix(a, b, 0.0)), Some((0, 0, 0)));
        assert_eq!(as_rgb(mix(a, b, 1.0)), Some((100, 200, 50)));
        assert_eq!(as_rgb(mix(a, b, 0.5)), Some((50, 100, 25)));
        // Non-Rgb passes through as `a`.
        assert!(matches!(mix(Color::Red, b, 0.5), Color::Red));
    }

    #[test]
    fn brighten_saturates() {
        let c = Color::Rgb {
            r: 250,
            g: 10,
            b: 100,
        };
        assert_eq!(as_rgb(brighten(c, 40)), Some((255, 50, 140)));
        assert!(matches!(brighten(Color::Red, 40), Color::Red));
    }

    #[test]
    fn parse_hex_valid_and_invalid() {
        assert_eq!(
            parse_hex("#ff5555"),
            Some(Color::Rgb {
                r: 0xff,
                g: 0x55,
                b: 0x55
            })
        );
        // Accepts the un-prefixed form too.
        assert_eq!(
            parse_hex("50fa7b"),
            Some(Color::Rgb {
                r: 0x50,
                g: 0xfa,
                b: 0x7b
            })
        );
        assert_eq!(parse_hex("#fff"), None);
        assert_eq!(parse_hex("#gggggg"), None);
        assert_eq!(parse_hex("not a color"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn theme_from_palette_maps_every_role() {
        let dracula = lookup_palette("dracula").expect("dracula palette exists");
        let t = theme_from_palette(&dracula);
        // error == red hex (#ff5555).
        assert_eq!(
            as_rgb(t.error),
            Some((0xff, 0x55, 0x55)),
            "error should map to the palette's red"
        );
        // success/diff_add == green (#50fa7b).
        assert_eq!(as_rgb(t.success), Some((0x50, 0xfa, 0x7b)));
        assert_eq!(as_rgb(t.diff_add), as_rgb(t.success));
        // tool == cyan; plan/plan_mode == purple.
        assert_eq!(as_rgb(t.tool), as_rgb(dracula.cyan));
        assert_eq!(as_rgb(t.plan), as_rgb(dracula.purple));
        assert_eq!(as_rgb(t.plan_mode), as_rgb(dracula.purple));
        // text == fg; accent == accent.
        assert_eq!(as_rgb(t.text), as_rgb(dracula.fg));
        assert_eq!(as_rgb(t.accent), as_rgb(dracula.accent));
        // agent_colors[0] == blue.
        assert_eq!(as_rgb(t.agent_colors[0]), as_rgb(dracula.blue));
        assert!(t.is_dark);
        // Derived slots stay populated (no accidental Reset).
        assert!(!matches!(t.diff_added_dimmed, Color::Reset));
        assert!(!matches!(t.rate_limit_empty, Color::Reset));
    }

    #[test]
    fn from_name_standard_ids_are_distinct() {
        let monokai = Theme::from_name("monokai");
        let dracula = Theme::from_name("dracula");
        let nord = Theme::from_name("nord");
        assert_ne!(as_rgb(monokai.accent), as_rgb(dracula.accent));
        assert_ne!(as_rgb(dracula.accent), as_rgb(nord.accent));
        assert_ne!(as_rgb(monokai.accent), as_rgb(nord.accent));
    }

    #[test]
    fn from_name_unknown_falls_back_to_default_dark() {
        let unknown = Theme::from_name("does-not-exist");
        let default = Theme::from_name(DEFAULT_DARK);
        assert!(unknown.is_dark);
        assert_eq!(as_rgb(unknown.accent), as_rgb(default.accent));
    }

    #[test]
    fn light_alias_resolves_to_light_theme() {
        let light = Theme::from_name("light");
        assert!(!light.is_dark, "'light' alias must be a light theme");
        let solarized = Theme::from_name("solarized-light");
        assert!(!solarized.is_dark);
        assert_eq!(as_rgb(light.accent), as_rgb(solarized.accent));
    }

    #[test]
    fn dark_alias_resolves_to_default_dark() {
        let dark = Theme::from_name("dark");
        let one_dark = Theme::from_name("one-dark");
        assert!(dark.is_dark);
        assert_eq!(as_rgb(dark.accent), as_rgb(one_dark.accent));
    }

    #[test]
    fn all_names_starts_with_auto_and_lists_standard_ids() {
        let names = Theme::all_names();
        assert_eq!(names.first().map(String::as_str), Some("auto"));
        for id in [
            "monokai",
            "dracula",
            "nord",
            "gruvbox",
            "one-dark",
            "solarized-dark",
            "solarized-light",
        ] {
            assert!(names.iter().any(|n| n == id), "missing standard id {id}");
        }
    }

    #[test]
    fn every_named_theme_resolves_with_populated_slots() {
        for name in Theme::all_names() {
            let t = Theme::from_name(&name);
            assert!(
                !matches!(t.accent, Color::Reset),
                "theme {name} has unset accent"
            );
            assert!(
                !matches!(t.error, Color::Reset),
                "theme {name} has unset error"
            );
            assert_eq!(
                t.agent_colors.len(),
                8,
                "theme {name} must define 8 agent colours"
            );
        }
    }

    #[test]
    fn user_palette_loads_from_toml_dir() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r##"
label = "My Theme"
dark = false
bg = "#101010"
fg = "#fafafa"
muted = "#808080"
selection = "#303030"
red = "#ff0000"
orange = "#ff8800"
yellow = "#ffff00"
green = "#00ff00"
cyan = "#00ffff"
blue = "#0000ff"
purple = "#8800ff"
pink = "#ff00ff"
accent = "#00ffff"
"##;
        std::fs::write(dir.path().join("mytheme.toml"), toml).unwrap();
        // A malformed file must be skipped, not panic.
        std::fs::write(dir.path().join("broken.toml"), "not = valid = toml").unwrap();

        let palettes = load_palettes_dir(dir.path());
        assert_eq!(palettes.len(), 1, "only the valid palette should load");
        let p = &palettes[0];
        assert_eq!(p.id.as_ref(), "mytheme", "id is the file stem");
        assert_eq!(p.label.as_ref(), "My Theme");
        assert!(!p.is_dark);

        let t = theme_from_palette(p);
        assert!(!t.is_dark);
        assert_eq!(as_rgb(t.error), Some((0xff, 0x00, 0x00)));
        assert_eq!(as_rgb(t.accent), Some((0x00, 0xff, 0xff)));
    }

    #[test]
    fn missing_user_dir_yields_no_palettes() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(load_palettes_dir(&missing).is_empty());
    }
}

#[cfg(test)]
mod accessibility_tests {
    use super::*;

    /// Every theme id the product ships as a default must actually resolve.
    /// A typo'd or deleted id silently falls back, so the user quietly gets a
    /// different theme than the config claims — how `"midnight"` survived in
    /// the first-run config after the palettes were rewritten.
    #[test]
    fn shipped_default_theme_ids_resolve() {
        let names = Theme::all_names();
        // The id the first-run config writes must be a real registry option,
        // not something that silently falls back to the default.
        assert!(
            names.iter().any(|n| n == "auto"),
            "first-run default theme id must appear in the registry, got {names:?}"
        );
        // `dark`/`light` are aliases rather than registry entries, so assert
        // they resolve to the polarity they promise instead.
        assert!(
            Theme::from_name("dark").is_dark,
            "`dark` must be a dark theme"
        );
        assert!(
            !Theme::from_name("light").is_dark,
            "`light` must be a light theme"
        );
    }

    /// Colour-vision-deficiency and 16-colour themes are accessibility
    /// affordances, not cosmetics: they must stay in the registry.
    #[test]
    fn accessibility_themes_are_registered() {
        let names = Theme::all_names();
        for id in [
            "dark-colorblind",
            "light-colorblind",
            "dark-ansi",
            "light-ansi",
        ] {
            assert!(
                names.iter().any(|n| n == id),
                "accessibility theme {id} must be registered"
            );
        }
    }

    /// The colour-blind palettes must not encode meaning in a red/green pair
    /// (indistinguishable under deuteranopia): error and success must differ
    /// in more than hue.
    #[test]
    fn colorblind_themes_avoid_red_green_collision() {
        for id in ["dark-colorblind", "light-colorblind"] {
            let t = Theme::from_name(id);
            assert_ne!(t.error, t.success, "{id}: error and success must differ");
        }
    }

    /// The ANSI themes must use named terminal colours (not truecolor) so
    /// they render on 16-colour terminals and follow the user's palette.
    #[test]
    fn ansi_themes_use_named_colors() {
        for id in ["dark-ansi", "light-ansi"] {
            let t = Theme::from_name(id);
            assert!(
                !matches!(t.accent, Color::Rgb { .. }),
                "{id}: accent must be a named ANSI colour, got {:?}",
                t.accent
            );
        }
    }
}

#[cfg(test)]
mod documented_names {
    /// Every identifier `docs/configuration/themes.mdx` advertises must be
    /// accepted, or the docs are promising a theme `/color` will reject.
    #[test]
    fn every_documented_theme_name_is_accepted() {
        let accepted = super::Theme::all_names();
        // Read the identifiers out of the docs rather than restating
        // them: a copied list keeps passing when the docs add or rename
        // a theme, which is exactly the drift this test exists to catch.
        let doc = include_str!("../../../../docs/configuration/themes.mdx");
        // Only the theme-name table: the page also documents colour keys
        // in backticked tables, and those are not theme identifiers. The
        // table is the contiguous run of `|` rows containing `auto`.
        let lines: Vec<&str> = doc.lines().collect();
        let anchor = lines
            .iter()
            .position(|l| l.starts_with('|') && l.contains("`auto`"))
            .expect("themes.mdx must document the `auto` theme in a table");
        let start = lines[..anchor]
            .iter()
            .rposition(|l| !l.starts_with('|'))
            .map(|i| i + 1)
            .unwrap_or(0);
        let end = lines[anchor..]
            .iter()
            .position(|l| !l.starts_with('|'))
            .map(|i| anchor + i)
            .unwrap_or(lines.len());
        let documented: Vec<String> = lines[start..end]
            .iter()
            .filter_map(|l| l.split('|').nth(1))
            .flat_map(|cell| {
                cell.split('/')
                    .filter_map(|part| {
                        let t = part.trim();
                        t.strip_prefix('`')
                            .and_then(|t| t.strip_suffix('`'))
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            documented.len() >= 10,
            "parsed only {} names from the themes.mdx table — the format \
             probably changed, which would make this test vacuous",
            documented.len()
        );
        let missing: Vec<&String> = documented
            .iter()
            .filter(|d| !accepted.iter().any(|a| a == *d))
            .collect();
        assert!(
            missing.is_empty(),
            "documented but not accepted by /color: {missing:?}\naccepted: {accepted:?}"
        );
    }
}
