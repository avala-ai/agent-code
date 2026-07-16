//! Terminal color themes.
//!
//! Provides a semantic color system where UI elements reference
//! named colors (error, warning, accent, etc.) rather than
//! hardcoded ANSI codes. Supports dark, light, and ANSI-only themes.

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

impl Theme {
    /// Midnight — dark theme with a restrained steel-blue accent.
    ///
    /// Kept intentionally desaturated so chrome (borders, badges, selection)
    /// stays calm; semantic colors (error/warning/success) carry the weight.
    pub fn midnight() -> Self {
        Self {
            // Soft steel blue — elegant default, not a loud brand purple.
            accent: Color::Rgb {
                r: 130,
                g: 165,
                b: 195,
            },
            error: Color::Rgb {
                r: 220,
                g: 115,
                b: 125,
            },
            warning: Color::Rgb {
                r: 210,
                g: 175,
                b: 90,
            },
            success: Color::Rgb {
                r: 95,
                g: 175,
                b: 125,
            },
            muted: Color::Rgb {
                r: 110,
                g: 112,
                b: 118,
            },
            inactive: Color::Rgb {
                r: 140,
                g: 142,
                b: 148,
            },
            // Cool gray-blue for tools — less “pastel purple”.
            tool: Color::Rgb {
                r: 145,
                g: 160,
                b: 175,
            },
            plan: Color::Rgb {
                r: 95,
                g: 155,
                b: 150,
            },
            text: Color::Rgb {
                r: 230,
                g: 232,
                b: 236,
            },
            diff_add: Color::Rgb {
                r: 56,
                g: 166,
                b: 96,
            },
            diff_remove: Color::Rgb {
                r: 179,
                g: 89,
                b: 107,
            },
            agent_colors: [
                Color::Rgb {
                    r: 220,
                    g: 38,
                    b: 38,
                },
                Color::Rgb {
                    r: 37,
                    g: 99,
                    b: 235,
                },
                Color::Rgb {
                    r: 22,
                    g: 163,
                    b: 74,
                },
                Color::Rgb {
                    r: 202,
                    g: 138,
                    b: 4,
                },
                Color::Rgb {
                    r: 147,
                    g: 51,
                    b: 234,
                },
                Color::Rgb {
                    r: 234,
                    g: 88,
                    b: 12,
                },
                Color::Rgb {
                    r: 219,
                    g: 39,
                    b: 119,
                },
                Color::Rgb {
                    r: 8,
                    g: 145,
                    b: 178,
                },
            ],
            success_shimmer: Color::Rgb {
                r: 130,
                g: 220,
                b: 150,
            },
            error_shimmer: Color::Rgb {
                r: 255,
                g: 160,
                b: 175,
            },
            warning_shimmer: Color::Rgb {
                r: 255,
                g: 220,
                b: 100,
            },
            accent_shimmer: Color::Rgb {
                r: 165,
                g: 195,
                b: 220,
            },
            muted_shimmer: Color::Rgb {
                r: 160,
                g: 162,
                b: 168,
            },
            diff_added_dimmed: Color::Rgb {
                r: 30,
                g: 70,
                b: 45,
            },
            diff_removed_dimmed: Color::Rgb {
                r: 80,
                g: 40,
                b: 50,
            },
            diff_added_word: Color::Rgb {
                r: 90,
                g: 200,
                b: 130,
            },
            diff_removed_word: Color::Rgb {
                r: 220,
                g: 130,
                b: 145,
            },
            subagent_red: Color::Rgb {
                r: 210,
                g: 100,
                b: 95,
            },
            subagent_blue: Color::Rgb {
                r: 100,
                g: 145,
                b: 210,
            },
            subagent_green: Color::Rgb {
                r: 95,
                g: 180,
                b: 130,
            },
            subagent_yellow: Color::Rgb {
                r: 200,
                g: 175,
                b: 90,
            },
            // Subagent id color only — not the product brand.
            subagent_purple: Color::Rgb {
                r: 155,
                g: 130,
                b: 185,
            },
            subagent_orange: Color::Rgb {
                r: 210,
                g: 145,
                b: 90,
            },
            subagent_pink: Color::Rgb {
                r: 200,
                g: 130,
                b: 160,
            },
            subagent_cyan: Color::Rgb {
                r: 100,
                g: 175,
                b: 190,
            },
            rate_limit_fill: Color::Rgb {
                r: 95,
                g: 175,
                b: 125,
            },
            rate_limit_empty: Color::Rgb {
                r: 48,
                g: 50,
                b: 56,
            },
            // Neutral slate selection — no purple wash.
            selection_bg: Color::Rgb {
                r: 48,
                g: 54,
                b: 64,
            },
            message_action_bg: Color::Rgb {
                r: 36,
                g: 38,
                b: 44,
            },
            user_message_bg: Color::Rgb {
                r: 32,
                g: 34,
                b: 40,
            },
            bash_message_bg: Color::Rgb {
                r: 30,
                g: 36,
                b: 32,
            },
            memory_message_bg: Color::Rgb {
                r: 40,
                g: 38,
                b: 30,
            },
            rainbow_red: Color::Rgb {
                r: 232,
                g: 75,
                b: 70,
            },
            rainbow_orange: Color::Rgb {
                r: 240,
                g: 140,
                b: 60,
            },
            rainbow_yellow: Color::Rgb {
                r: 240,
                g: 220,
                b: 80,
            },
            rainbow_green: Color::Rgb {
                r: 80,
                g: 200,
                b: 120,
            },
            rainbow_blue: Color::Rgb {
                r: 80,
                g: 140,
                b: 240,
            },
            rainbow_indigo: Color::Rgb {
                r: 120,
                g: 130,
                b: 190,
            },
            rainbow_violet: Color::Rgb {
                r: 150,
                g: 135,
                b: 180,
            },
            // Plan mode badge: same family as accent, slightly cooler.
            plan_mode: Color::Rgb {
                r: 120,
                g: 155,
                b: 185,
            },
            brief_mode: Color::Rgb {
                r: 200,
                g: 150,
                b: 95,
            },
            fast_mode: Color::Rgb {
                r: 95,
                g: 175,
                b: 125,
            },
            fast_mode_shimmer: Color::Rgb {
                r: 130,
                g: 200,
                b: 155,
            },
            is_dark: true,
        }
    }

    /// Daybreak — light theme with soft blue accents.
    pub fn daybreak() -> Self {
        Self {
            accent: Color::Rgb {
                r: 70,
                g: 115,
                b: 160,
            },
            error: Color::Rgb {
                r: 171,
                g: 43,
                b: 63,
            },
            warning: Color::Rgb {
                r: 150,
                g: 108,
                b: 30,
            },
            success: Color::Rgb {
                r: 44,
                g: 122,
                b: 57,
            },
            muted: Color::Rgb {
                r: 140,
                g: 140,
                b: 140,
            },
            inactive: Color::Rgb {
                r: 102,
                g: 102,
                b: 102,
            },
            tool: Color::Rgb {
                r: 87,
                g: 105,
                b: 247,
            },
            plan: Color::Rgb {
                r: 50,
                g: 120,
                b: 110,
            },
            text: Color::Black,
            diff_add: Color::Rgb {
                r: 47,
                g: 157,
                b: 68,
            },
            diff_remove: Color::Rgb {
                r: 209,
                g: 69,
                b: 75,
            },
            agent_colors: [
                Color::Rgb {
                    r: 185,
                    g: 28,
                    b: 28,
                },
                Color::Rgb {
                    r: 29,
                    g: 78,
                    b: 216,
                },
                Color::Rgb {
                    r: 21,
                    g: 128,
                    b: 61,
                },
                Color::Rgb {
                    r: 161,
                    g: 98,
                    b: 7,
                },
                Color::Rgb {
                    r: 126,
                    g: 34,
                    b: 206,
                },
                Color::Rgb {
                    r: 194,
                    g: 65,
                    b: 12,
                },
                Color::Rgb {
                    r: 190,
                    g: 24,
                    b: 93,
                },
                Color::Rgb {
                    r: 14,
                    g: 116,
                    b: 144,
                },
            ],
            success_shimmer: Color::Rgb {
                r: 28,
                g: 95,
                b: 40,
            },
            error_shimmer: Color::Rgb {
                r: 135,
                g: 30,
                b: 45,
            },
            warning_shimmer: Color::Rgb {
                r: 115,
                g: 80,
                b: 20,
            },
            accent_shimmer: Color::Rgb {
                r: 90,
                g: 130,
                b: 170,
            },
            muted_shimmer: Color::Rgb {
                r: 100,
                g: 100,
                b: 100,
            },
            diff_added_dimmed: Color::Rgb {
                r: 215,
                g: 240,
                b: 220,
            },
            diff_removed_dimmed: Color::Rgb {
                r: 245,
                g: 215,
                b: 220,
            },
            diff_added_word: Color::Rgb {
                r: 30,
                g: 130,
                b: 55,
            },
            diff_removed_word: Color::Rgb {
                r: 175,
                g: 40,
                b: 55,
            },
            subagent_red: Color::Rgb {
                r: 185,
                g: 28,
                b: 28,
            },
            subagent_blue: Color::Rgb {
                r: 29,
                g: 78,
                b: 216,
            },
            subagent_green: Color::Rgb {
                r: 21,
                g: 128,
                b: 61,
            },
            subagent_yellow: Color::Rgb {
                r: 161,
                g: 98,
                b: 7,
            },
            subagent_purple: Color::Rgb {
                r: 126,
                g: 34,
                b: 206,
            },
            subagent_orange: Color::Rgb {
                r: 194,
                g: 65,
                b: 12,
            },
            subagent_pink: Color::Rgb {
                r: 190,
                g: 24,
                b: 93,
            },
            subagent_cyan: Color::Rgb {
                r: 14,
                g: 116,
                b: 144,
            },
            rate_limit_fill: Color::Rgb {
                r: 44,
                g: 122,
                b: 57,
            },
            rate_limit_empty: Color::Rgb {
                r: 215,
                g: 215,
                b: 220,
            },
            selection_bg: Color::Rgb {
                r: 200,
                g: 215,
                b: 245,
            },
            message_action_bg: Color::Rgb {
                r: 230,
                g: 230,
                b: 240,
            },
            user_message_bg: Color::Rgb {
                r: 235,
                g: 235,
                b: 245,
            },
            bash_message_bg: Color::Rgb {
                r: 230,
                g: 240,
                b: 230,
            },
            memory_message_bg: Color::Rgb {
                r: 245,
                g: 235,
                b: 225,
            },
            rainbow_red: Color::Rgb {
                r: 185,
                g: 28,
                b: 28,
            },
            rainbow_orange: Color::Rgb {
                r: 194,
                g: 65,
                b: 12,
            },
            rainbow_yellow: Color::Rgb {
                r: 161,
                g: 98,
                b: 7,
            },
            rainbow_green: Color::Rgb {
                r: 21,
                g: 128,
                b: 61,
            },
            rainbow_blue: Color::Rgb {
                r: 29,
                g: 78,
                b: 216,
            },
            rainbow_indigo: Color::Rgb {
                r: 80,
                g: 95,
                b: 160,
            },
            rainbow_violet: Color::Rgb {
                r: 110,
                g: 100,
                b: 150,
            },
            plan_mode: Color::Rgb {
                r: 70,
                g: 115,
                b: 160,
            },
            brief_mode: Color::Rgb {
                r: 175,
                g: 105,
                b: 55,
            },
            fast_mode: Color::Rgb {
                r: 40,
                g: 130,
                b: 80,
            },
            fast_mode_shimmer: Color::Rgb {
                r: 30,
                g: 100,
                b: 50,
            },
            is_dark: false,
        }
    }

    /// Midnight Muted — softer dark theme (lower-contrast steel accent).
    pub fn midnight_muted() -> Self {
        let mut t = Self::midnight();
        t.accent = Color::Rgb {
            r: 115,
            g: 140,
            b: 160,
        };
        t.error = Color::Rgb {
            r: 195,
            g: 115,
            b: 120,
        };
        t.warning = Color::Rgb {
            r: 190,
            g: 160,
            b: 85,
        };
        t.success = Color::Rgb {
            r: 90,
            g: 155,
            b: 115,
        };
        t.tool = Color::Rgb {
            r: 130,
            g: 142,
            b: 155,
        };
        t.success_shimmer = Color::Rgb {
            r: 120,
            g: 175,
            b: 140,
        };
        t.error_shimmer = Color::Rgb {
            r: 210,
            g: 140,
            b: 145,
        };
        t.warning_shimmer = Color::Rgb {
            r: 210,
            g: 185,
            b: 110,
        };
        t.accent_shimmer = Color::Rgb {
            r: 145,
            g: 170,
            b: 190,
        };
        t.diff_added_word = Color::Rgb {
            r: 110,
            g: 200,
            b: 130,
        };
        t.diff_removed_word = Color::Rgb {
            r: 220,
            g: 130,
            b: 145,
        };
        t.subagent_red = Color::Rgb {
            r: 200,
            g: 100,
            b: 100,
        };
        t.subagent_blue = Color::Rgb {
            r: 110,
            g: 150,
            b: 220,
        };
        t.subagent_green = Color::Rgb {
            r: 100,
            g: 180,
            b: 130,
        };
        t.subagent_yellow = Color::Rgb {
            r: 210,
            g: 190,
            b: 100,
        };
        t.subagent_purple = Color::Rgb {
            r: 170,
            g: 120,
            b: 220,
        };
        t.subagent_orange = Color::Rgb {
            r: 220,
            g: 150,
            b: 90,
        };
        t.subagent_pink = Color::Rgb {
            r: 220,
            g: 130,
            b: 175,
        };
        t.subagent_cyan = Color::Rgb {
            r: 110,
            g: 190,
            b: 210,
        };
        t.rainbow_red = t.subagent_red;
        t.rainbow_orange = t.subagent_orange;
        t.rainbow_yellow = t.subagent_yellow;
        t.rainbow_green = t.subagent_green;
        t.rainbow_blue = t.subagent_blue;
        t.rainbow_indigo = Color::Rgb {
            r: 120,
            g: 110,
            b: 200,
        };
        t.rainbow_violet = t.subagent_purple;
        t.plan_mode = t.subagent_purple;
        t.brief_mode = t.subagent_orange;
        t.fast_mode = t.subagent_green;
        t.fast_mode_shimmer = Color::Rgb {
            r: 130,
            g: 210,
            b: 150,
        };
        t
    }

    /// Daybreak Muted — softer light theme.
    pub fn daybreak_muted() -> Self {
        let mut t = Self::daybreak();
        t.accent = Color::Rgb {
            r: 110,
            g: 30,
            b: 150,
        };
        t.error = Color::Rgb {
            r: 150,
            g: 50,
            b: 60,
        };
        t.warning = Color::Rgb {
            r: 130,
            g: 100,
            b: 40,
        };
        t.success = Color::Rgb {
            r: 50,
            g: 110,
            b: 55,
        };
        t.tool = Color::Rgb {
            r: 70,
            g: 85,
            b: 190,
        };
        t.success_shimmer = Color::Rgb {
            r: 35,
            g: 90,
            b: 45,
        };
        t.error_shimmer = Color::Rgb {
            r: 125,
            g: 40,
            b: 50,
        };
        t.warning_shimmer = Color::Rgb {
            r: 105,
            g: 80,
            b: 30,
        };
        t.accent_shimmer = Color::Rgb {
            r: 90,
            g: 25,
            b: 130,
        };
        t.diff_added_word = Color::Rgb {
            r: 35,
            g: 120,
            b: 55,
        };
        t.diff_removed_word = Color::Rgb {
            r: 160,
            g: 50,
            b: 60,
        };
        t.subagent_red = Color::Rgb {
            r: 165,
            g: 50,
            b: 50,
        };
        t.subagent_blue = Color::Rgb {
            r: 50,
            g: 90,
            b: 190,
        };
        t.subagent_green = Color::Rgb {
            r: 40,
            g: 120,
            b: 70,
        };
        t.subagent_yellow = Color::Rgb {
            r: 145,
            g: 105,
            b: 25,
        };
        t.subagent_purple = Color::Rgb {
            r: 115,
            g: 50,
            b: 180,
        };
        t.subagent_orange = Color::Rgb {
            r: 175,
            g: 75,
            b: 30,
        };
        t.subagent_pink = Color::Rgb {
            r: 170,
            g: 40,
            b: 100,
        };
        t.subagent_cyan = Color::Rgb {
            r: 35,
            g: 110,
            b: 135,
        };
        t.rainbow_red = t.subagent_red;
        t.rainbow_orange = t.subagent_orange;
        t.rainbow_yellow = t.subagent_yellow;
        t.rainbow_green = t.subagent_green;
        t.rainbow_blue = t.subagent_blue;
        t.rainbow_indigo = Color::Rgb {
            r: 75,
            g: 65,
            b: 175,
        };
        t.rainbow_violet = t.subagent_purple;
        t.plan_mode = t.subagent_purple;
        t.brief_mode = t.subagent_orange;
        t.fast_mode = t.subagent_green;
        t.fast_mode_shimmer = Color::Rgb {
            r: 35,
            g: 95,
            b: 50,
        };
        t
    }

    /// Terminal — uses standard 16 ANSI colors only.
    pub fn terminal() -> Self {
        // Bright ANSI variants do not have dedicated `Color::Bright*`
        // names in `crossterm`; use the regular variants for "bright"
        // shimmer roles and the `Dark*` variants for "base" roles where
        // the terminal renders the brighter version on top.
        Self {
            accent: Color::Magenta,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            muted: Color::DarkGrey,
            inactive: Color::Grey,
            tool: Color::Cyan,
            plan: Color::DarkGreen,
            text: Color::Reset,
            diff_add: Color::Green,
            diff_remove: Color::Red,
            agent_colors: [
                Color::Red,
                Color::Blue,
                Color::Green,
                Color::Yellow,
                Color::Magenta,
                Color::DarkYellow,
                Color::DarkMagenta,
                Color::Cyan,
            ],
            success_shimmer: Color::Green,
            error_shimmer: Color::Red,
            warning_shimmer: Color::Yellow,
            accent_shimmer: Color::Magenta,
            muted_shimmer: Color::Grey,
            diff_added_dimmed: Color::DarkGreen,
            diff_removed_dimmed: Color::DarkRed,
            diff_added_word: Color::Green,
            diff_removed_word: Color::Red,
            subagent_red: Color::Red,
            subagent_blue: Color::Blue,
            subagent_green: Color::Green,
            subagent_yellow: Color::Yellow,
            subagent_purple: Color::Magenta,
            subagent_orange: Color::DarkYellow,
            subagent_pink: Color::DarkMagenta,
            subagent_cyan: Color::Cyan,
            rate_limit_fill: Color::Green,
            rate_limit_empty: Color::DarkGrey,
            selection_bg: Color::DarkGrey,
            message_action_bg: Color::DarkGrey,
            user_message_bg: Color::DarkGrey,
            bash_message_bg: Color::DarkGrey,
            memory_message_bg: Color::DarkGrey,
            rainbow_red: Color::Red,
            rainbow_orange: Color::DarkYellow,
            rainbow_yellow: Color::Yellow,
            rainbow_green: Color::Green,
            rainbow_blue: Color::Blue,
            rainbow_indigo: Color::DarkBlue,
            rainbow_violet: Color::Magenta,
            plan_mode: Color::Magenta,
            brief_mode: Color::DarkYellow,
            fast_mode: Color::Green,
            fast_mode_shimmer: Color::Green,
            is_dark: true,
        }
    }

    /// Dark theme using the Okabe-Ito palette — chosen for protanopia,
    /// deuteranopia, and tritanopia distinguishability. The eight
    /// canonical hex values are mapped to semantic slots so diffs and
    /// status indicators stay separable for users with common forms of
    /// colour-vision deficiency.
    pub fn dark_colorblind() -> Self {
        // Okabe-Ito palette (canonical hex):
        //   black       #000000
        //   orange      #E69F00
        //   sky-blue    #56B4E9
        //   bluish-green#009E73
        //   yellow      #F0E442
        //   blue        #0072B2
        //   vermillion  #D55E00
        //   reddish-purple #CC79A7
        let orange = Color::Rgb {
            r: 0xE6,
            g: 0x9F,
            b: 0x00,
        };
        let sky_blue = Color::Rgb {
            r: 0x56,
            g: 0xB4,
            b: 0xE9,
        };
        let bluish_green = Color::Rgb {
            r: 0x00,
            g: 0x9E,
            b: 0x73,
        };
        let yellow = Color::Rgb {
            r: 0xF0,
            g: 0xE4,
            b: 0x42,
        };
        let blue = Color::Rgb {
            r: 0x00,
            g: 0x72,
            b: 0xB2,
        };
        let vermillion = Color::Rgb {
            r: 0xD5,
            g: 0x5E,
            b: 0x00,
        };
        let reddish_purple = Color::Rgb {
            r: 0xCC,
            g: 0x79,
            b: 0xA7,
        };

        Self {
            accent: sky_blue,
            error: vermillion,
            warning: orange,
            success: bluish_green,
            muted: Color::Rgb {
                r: 130,
                g: 130,
                b: 130,
            },
            inactive: Color::Rgb {
                r: 170,
                g: 170,
                b: 170,
            },
            tool: reddish_purple,
            plan: blue,
            text: Color::White,
            diff_add: bluish_green,
            diff_remove: vermillion,
            agent_colors: [
                vermillion,
                blue,
                bluish_green,
                yellow,
                sky_blue,
                orange,
                reddish_purple,
                Color::White,
            ],
            // The Okabe-Ito palette has only seven chromatic hues plus
            // black/white; nine subagent + rainbow slots can't all be
            // unique, so some intentional repetition appears below.
            success_shimmer: bluish_green,
            error_shimmer: vermillion,
            warning_shimmer: yellow,
            accent_shimmer: sky_blue,
            muted_shimmer: Color::White,
            diff_added_dimmed: bluish_green,
            diff_removed_dimmed: vermillion,
            diff_added_word: bluish_green,
            diff_removed_word: vermillion,
            subagent_red: vermillion,
            subagent_blue: blue,
            subagent_green: bluish_green,
            subagent_yellow: yellow,
            subagent_purple: reddish_purple,
            subagent_orange: orange,
            // Repeats reddish_purple — Okabe-Ito has no separate pink.
            subagent_pink: reddish_purple,
            subagent_cyan: sky_blue,
            rate_limit_fill: bluish_green,
            rate_limit_empty: Color::Black,
            selection_bg: blue,
            message_action_bg: Color::Black,
            user_message_bg: Color::Black,
            bash_message_bg: Color::Black,
            memory_message_bg: Color::Black,
            rainbow_red: vermillion,
            rainbow_orange: orange,
            rainbow_yellow: yellow,
            rainbow_green: bluish_green,
            rainbow_blue: blue,
            // Repeats blue — Okabe-Ito has no separate indigo.
            rainbow_indigo: blue,
            rainbow_violet: reddish_purple,
            plan_mode: reddish_purple,
            brief_mode: orange,
            fast_mode: bluish_green,
            fast_mode_shimmer: bluish_green,
            is_dark: true,
        }
    }

    /// Light counterpart of [`Theme::dark_colorblind`] using the same
    /// Okabe-Ito palette. The semantic slots are re-mapped to keep
    /// contrast against a pale background; the eight canonical hex
    /// values are still exclusively used.
    pub fn light_colorblind() -> Self {
        let orange = Color::Rgb {
            r: 0xE6,
            g: 0x9F,
            b: 0x00,
        };
        let sky_blue = Color::Rgb {
            r: 0x56,
            g: 0xB4,
            b: 0xE9,
        };
        let bluish_green = Color::Rgb {
            r: 0x00,
            g: 0x9E,
            b: 0x73,
        };
        let yellow = Color::Rgb {
            r: 0xF0,
            g: 0xE4,
            b: 0x42,
        };
        let blue = Color::Rgb {
            r: 0x00,
            g: 0x72,
            b: 0xB2,
        };
        let vermillion = Color::Rgb {
            r: 0xD5,
            g: 0x5E,
            b: 0x00,
        };
        let reddish_purple = Color::Rgb {
            r: 0xCC,
            g: 0x79,
            b: 0xA7,
        };

        Self {
            accent: blue,
            error: vermillion,
            warning: orange,
            success: bluish_green,
            muted: Color::Rgb {
                r: 110,
                g: 110,
                b: 110,
            },
            inactive: Color::Rgb {
                r: 90,
                g: 90,
                b: 90,
            },
            tool: reddish_purple,
            plan: bluish_green,
            text: Color::Black,
            diff_add: bluish_green,
            diff_remove: vermillion,
            agent_colors: [
                vermillion,
                blue,
                bluish_green,
                yellow,
                sky_blue,
                orange,
                reddish_purple,
                Color::Black,
            ],
            // The Okabe-Ito palette has only seven chromatic hues plus
            // black/white; nine subagent + rainbow slots can't all be
            // unique, so some intentional repetition appears below.
            success_shimmer: bluish_green,
            error_shimmer: vermillion,
            warning_shimmer: orange,
            accent_shimmer: blue,
            muted_shimmer: Color::Black,
            diff_added_dimmed: bluish_green,
            diff_removed_dimmed: vermillion,
            diff_added_word: bluish_green,
            diff_removed_word: vermillion,
            subagent_red: vermillion,
            subagent_blue: blue,
            subagent_green: bluish_green,
            subagent_yellow: yellow,
            subagent_purple: reddish_purple,
            subagent_orange: orange,
            // Repeats reddish_purple — Okabe-Ito has no separate pink.
            subagent_pink: reddish_purple,
            subagent_cyan: sky_blue,
            rate_limit_fill: bluish_green,
            rate_limit_empty: Color::White,
            selection_bg: sky_blue,
            message_action_bg: Color::White,
            user_message_bg: Color::White,
            bash_message_bg: Color::White,
            memory_message_bg: Color::White,
            rainbow_red: vermillion,
            rainbow_orange: orange,
            rainbow_yellow: yellow,
            rainbow_green: bluish_green,
            rainbow_blue: blue,
            // Repeats blue — Okabe-Ito has no separate indigo.
            rainbow_indigo: blue,
            rainbow_violet: reddish_purple,
            plan_mode: reddish_purple,
            brief_mode: orange,
            fast_mode: bluish_green,
            fast_mode_shimmer: bluish_green,
            is_dark: false,
        }
    }

    /// Dark theme restricted to the 16 standard ANSI colour codes —
    /// for terminals without truecolor support. Every slot is one of
    /// the named [`Color`] variants the standard ANSI palette knows
    /// (no `Color::Rgb`, no 256-colour indices).
    pub fn dark_ansi() -> Self {
        // The 16-colour ANSI palette has 16 distinct slots; many of
        // the new visual slots have to share with semantic ones. The
        // mapping below picks the closest hue from the 16 standard
        // codes, with the bright `Color::Red/Green/...` set used for
        // shimmer roles and the `Color::Dark*` set for "base" hues.
        Self {
            accent: Color::Cyan,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            muted: Color::DarkGrey,
            inactive: Color::Grey,
            tool: Color::Magenta,
            plan: Color::Blue,
            text: Color::White,
            diff_add: Color::Green,
            diff_remove: Color::Red,
            agent_colors: [
                Color::Red,
                Color::Blue,
                Color::Green,
                Color::Yellow,
                Color::Magenta,
                Color::Cyan,
                Color::DarkYellow,
                Color::DarkMagenta,
            ],
            success_shimmer: Color::Green,
            error_shimmer: Color::Red,
            warning_shimmer: Color::Yellow,
            accent_shimmer: Color::Cyan,
            muted_shimmer: Color::Grey,
            diff_added_dimmed: Color::DarkGreen,
            diff_removed_dimmed: Color::DarkRed,
            diff_added_word: Color::Green,
            diff_removed_word: Color::Red,
            subagent_red: Color::Red,
            subagent_blue: Color::Blue,
            subagent_green: Color::Green,
            subagent_yellow: Color::Yellow,
            subagent_purple: Color::Magenta,
            subagent_orange: Color::DarkYellow,
            subagent_pink: Color::DarkMagenta,
            subagent_cyan: Color::Cyan,
            rate_limit_fill: Color::Green,
            rate_limit_empty: Color::DarkGrey,
            selection_bg: Color::DarkGrey,
            message_action_bg: Color::DarkGrey,
            user_message_bg: Color::DarkGrey,
            bash_message_bg: Color::DarkGrey,
            memory_message_bg: Color::DarkGrey,
            rainbow_red: Color::Red,
            rainbow_orange: Color::DarkYellow,
            rainbow_yellow: Color::Yellow,
            rainbow_green: Color::Green,
            rainbow_blue: Color::Blue,
            rainbow_indigo: Color::DarkBlue,
            rainbow_violet: Color::Magenta,
            plan_mode: Color::Magenta,
            brief_mode: Color::DarkYellow,
            fast_mode: Color::Green,
            fast_mode_shimmer: Color::Green,
            is_dark: true,
        }
    }

    /// Light counterpart of [`Theme::dark_ansi`]. Uses the dark ANSI
    /// variants where contrast on a pale background matters.
    pub fn light_ansi() -> Self {
        Self {
            accent: Color::DarkBlue,
            error: Color::DarkRed,
            warning: Color::DarkYellow,
            success: Color::DarkGreen,
            muted: Color::DarkGrey,
            inactive: Color::Grey,
            tool: Color::DarkMagenta,
            plan: Color::DarkCyan,
            text: Color::Black,
            diff_add: Color::DarkGreen,
            diff_remove: Color::DarkRed,
            agent_colors: [
                Color::DarkRed,
                Color::DarkBlue,
                Color::DarkGreen,
                Color::DarkYellow,
                Color::DarkMagenta,
                Color::DarkCyan,
                Color::Red,
                Color::Blue,
            ],
            success_shimmer: Color::DarkGreen,
            error_shimmer: Color::DarkRed,
            warning_shimmer: Color::DarkYellow,
            accent_shimmer: Color::DarkBlue,
            muted_shimmer: Color::DarkGrey,
            diff_added_dimmed: Color::Green,
            diff_removed_dimmed: Color::Red,
            diff_added_word: Color::DarkGreen,
            diff_removed_word: Color::DarkRed,
            subagent_red: Color::DarkRed,
            subagent_blue: Color::DarkBlue,
            subagent_green: Color::DarkGreen,
            subagent_yellow: Color::DarkYellow,
            subagent_purple: Color::DarkMagenta,
            // No "dark orange" in the 16-ANSI set; reuse DarkYellow.
            subagent_orange: Color::DarkYellow,
            // No "dark pink" in the 16-ANSI set; reuse DarkMagenta.
            subagent_pink: Color::DarkMagenta,
            subagent_cyan: Color::DarkCyan,
            rate_limit_fill: Color::DarkGreen,
            rate_limit_empty: Color::Grey,
            selection_bg: Color::Grey,
            message_action_bg: Color::Grey,
            user_message_bg: Color::Grey,
            bash_message_bg: Color::Grey,
            memory_message_bg: Color::Grey,
            rainbow_red: Color::DarkRed,
            rainbow_orange: Color::DarkYellow,
            rainbow_yellow: Color::DarkYellow,
            rainbow_green: Color::DarkGreen,
            rainbow_blue: Color::DarkBlue,
            rainbow_indigo: Color::Blue,
            rainbow_violet: Color::DarkMagenta,
            plan_mode: Color::DarkMagenta,
            brief_mode: Color::DarkYellow,
            fast_mode: Color::DarkGreen,
            fast_mode_shimmer: Color::DarkGreen,
            is_dark: false,
        }
    }

    /// Resolve a theme name to a Theme instance.
    pub fn from_name(name: &str) -> Self {
        match name {
            "midnight" | "dark" => Self::midnight(),
            "daybreak" | "light" => Self::daybreak(),
            "midnight-muted" => Self::midnight_muted(),
            "daybreak-muted" => Self::daybreak_muted(),
            "terminal" => Self::terminal(),
            "dark-colorblind" => Self::dark_colorblind(),
            "light-colorblind" => Self::light_colorblind(),
            "dark-ansi" => Self::dark_ansi(),
            "light-ansi" => Self::light_ansi(),
            "auto" => {
                let detected = detect_system_theme();
                if detected == "light" {
                    Self::daybreak()
                } else {
                    Self::midnight()
                }
            }
            _ => Self::midnight(),
        }
    }

    /// All known theme identifiers in display order. Drives the
    /// onboarding picker and the `/theme` slash command.
    pub fn all_names() -> &'static [&'static str] {
        &[
            "auto",
            "midnight",
            "daybreak",
            "dark-colorblind",
            "light-colorblind",
            "dark-ansi",
            "light-ansi",
        ]
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

/// Resolve a config theme name, handling "auto".
pub fn resolve_theme(configured: &str) -> String {
    if configured == "auto" {
        let detected = detect_system_theme();
        if detected == "light" {
            "daybreak".to_string()
        } else {
            "midnight".to_string()
        }
    } else {
        configured.to_string()
    }
}

// ---- Global theme access ----
//
// The active theme is held behind a `RwLock` rather than a `OnceLock`
// so the `/theme` slash command can re-paint the running session
// without requiring a restart. Writes happen at most once per user
// command; reads are cheap because `Theme` is `Clone` and the lock
// hold time is microseconds.

use std::sync::RwLock;

static ACTIVE_THEME: RwLock<Option<Theme>> = RwLock::new(None);

/// Initialize (or re-set) the global theme. Safe to call from the
/// startup path *and* from the `/theme` slash command — the latter
/// overrides any previously installed theme.
pub fn init(theme_name: &str) {
    let theme = Theme::from_name(theme_name);
    if let Ok(mut guard) = ACTIVE_THEME.write() {
        *guard = Some(theme);
    }
}

/// Get a snapshot of the active theme. Falls back to midnight if not
/// initialized. Returns by value (cheap clone) so callers don't need
/// to hold the lock across rendering.
pub fn current() -> Theme {
    ACTIVE_THEME
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(Theme::midnight)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_non_auto() {
        assert_eq!(resolve_theme("midnight"), "midnight");
        assert_eq!(resolve_theme("daybreak"), "daybreak");
    }

    #[test]
    fn test_resolve_auto_returns_valid_theme() {
        let result = resolve_theme("auto");
        assert!(result == "midnight" || result == "daybreak");
    }

    #[test]
    fn test_theme_from_name() {
        let t = Theme::from_name("midnight");
        assert!(t.is_dark);
        let t = Theme::from_name("daybreak");
        assert!(!t.is_dark);
    }

    #[test]
    fn test_agent_color_wraps() {
        let t = Theme::midnight();
        let c0 = t.agent_color(0);
        let c8 = t.agent_color(8);
        assert!(matches!(c0, Color::Rgb { .. }));
        assert!(matches!(c8, Color::Rgb { .. }));
    }

    #[test]
    fn every_named_theme_resolves() {
        // Tripwire: every name advertised by `all_names` must produce a
        // theme whose every slot is populated. `Color` has no Default
        // impl so a missing field is a compile error already; this
        // covers semantic completeness too — matching `Color::Reset`
        // on a non-text slot is the "I forgot to set this" footgun.
        for name in Theme::all_names() {
            let t = Theme::from_name(name);
            assert!(
                !matches!(t.accent, Color::Reset),
                "theme {name} has unset accent"
            );
            assert!(
                !matches!(t.error, Color::Reset),
                "theme {name} has unset error"
            );
            assert!(
                !matches!(t.diff_add, Color::Reset),
                "theme {name} has unset diff_add"
            );
            assert!(
                !matches!(t.diff_remove, Color::Reset),
                "theme {name} has unset diff_remove"
            );
            assert_eq!(
                t.agent_colors.len(),
                8,
                "theme {name} must define 8 agent colours"
            );
        }
    }

    #[test]
    fn colorblind_palette_uses_okabe_ito_hex_values() {
        // Spec: every Okabe-Ito canonical hex value must show up in
        // either dark-colorblind or light-colorblind. The two themes
        // share the same palette and only re-map semantic slots, so a
        // single union suffices.
        let okabe_ito: &[(u8, u8, u8)] = &[
            (0xE6, 0x9F, 0x00), // orange
            (0x56, 0xB4, 0xE9), // sky-blue
            (0x00, 0x9E, 0x73), // bluish-green
            (0xF0, 0xE4, 0x42), // yellow
            (0x00, 0x72, 0xB2), // blue
            (0xD5, 0x5E, 0x00), // vermillion
            (0xCC, 0x79, 0xA7), // reddish-purple
        ];

        let collect = |t: &Theme| -> Vec<(u8, u8, u8)> {
            let mut all = vec![
                t.accent,
                t.error,
                t.warning,
                t.success,
                t.tool,
                t.plan,
                t.diff_add,
                t.diff_remove,
            ];
            all.extend_from_slice(&t.agent_colors);
            all.into_iter()
                .filter_map(|c| match c {
                    Color::Rgb { r, g, b } => Some((r, g, b)),
                    _ => None,
                })
                .collect()
        };

        let dark = Theme::dark_colorblind();
        let light = Theme::light_colorblind();
        let mut union: Vec<(u8, u8, u8)> = collect(&dark);
        union.extend(collect(&light));
        union.sort_unstable();
        union.dedup();

        for triple in okabe_ito {
            assert!(
                union.contains(triple),
                "Okabe-Ito colour {:02X}{:02X}{:02X} missing from colourblind themes",
                triple.0,
                triple.1,
                triple.2,
            );
        }
        // Black (the eighth Okabe-Ito colour) should appear as text in
        // at least the light variant — assert separately so the union
        // check stays focused on the chromatic seven.
        assert!(matches!(light.text, Color::Black));
    }

    #[test]
    fn ansi_only_themes_use_no_truecolor() {
        // For every slot in the ANSI-only themes, the colour must be
        // one of the 16 standard ANSI variants — no `Rgb`, no
        // `AnsiValue`. The latter would still resolve in 256-colour
        // terminals but doesn't degrade in 16-colour ones.
        fn is_ansi_16(c: Color) -> bool {
            matches!(
                c,
                Color::Reset
                    | Color::Black
                    | Color::Red
                    | Color::Green
                    | Color::Yellow
                    | Color::Blue
                    | Color::Magenta
                    | Color::Cyan
                    | Color::Grey
                    | Color::White
                    | Color::DarkGrey
                    | Color::DarkRed
                    | Color::DarkGreen
                    | Color::DarkYellow
                    | Color::DarkBlue
                    | Color::DarkMagenta
                    | Color::DarkCyan
            )
        }
        for theme in [Theme::dark_ansi(), Theme::light_ansi()] {
            for c in [
                theme.accent,
                theme.error,
                theme.warning,
                theme.success,
                theme.muted,
                theme.inactive,
                theme.tool,
                theme.plan,
                theme.text,
                theme.diff_add,
                theme.diff_remove,
            ] {
                assert!(is_ansi_16(c), "ANSI theme leaked non-ANSI colour {c:?}");
            }
            for c in theme.agent_colors {
                assert!(is_ansi_16(c), "ANSI theme agent colour {c:?} is non-ANSI");
            }
            for (label, c) in new_slots(&theme) {
                assert!(
                    is_ansi_16(c),
                    "ANSI theme slot {label} leaked non-ANSI colour {c:?}"
                );
            }
        }
    }

    /// Every non-array slot added in the palette expansion, paired with
    /// its name for diagnostic messages. Tests iterate over this to
    /// avoid drifting when slots are added.
    fn new_slots(t: &Theme) -> [(&'static str, Color); 35] {
        [
            ("success_shimmer", t.success_shimmer),
            ("error_shimmer", t.error_shimmer),
            ("warning_shimmer", t.warning_shimmer),
            ("accent_shimmer", t.accent_shimmer),
            ("muted_shimmer", t.muted_shimmer),
            ("diff_added_dimmed", t.diff_added_dimmed),
            ("diff_removed_dimmed", t.diff_removed_dimmed),
            ("diff_added_word", t.diff_added_word),
            ("diff_removed_word", t.diff_removed_word),
            ("subagent_red", t.subagent_red),
            ("subagent_blue", t.subagent_blue),
            ("subagent_green", t.subagent_green),
            ("subagent_yellow", t.subagent_yellow),
            ("subagent_purple", t.subagent_purple),
            ("subagent_orange", t.subagent_orange),
            ("subagent_pink", t.subagent_pink),
            ("subagent_cyan", t.subagent_cyan),
            ("rate_limit_fill", t.rate_limit_fill),
            ("rate_limit_empty", t.rate_limit_empty),
            ("selection_bg", t.selection_bg),
            ("message_action_bg", t.message_action_bg),
            ("user_message_bg", t.user_message_bg),
            ("bash_message_bg", t.bash_message_bg),
            ("memory_message_bg", t.memory_message_bg),
            ("rainbow_red", t.rainbow_red),
            ("rainbow_orange", t.rainbow_orange),
            ("rainbow_yellow", t.rainbow_yellow),
            ("rainbow_green", t.rainbow_green),
            ("rainbow_blue", t.rainbow_blue),
            ("rainbow_indigo", t.rainbow_indigo),
            ("rainbow_violet", t.rainbow_violet),
            ("plan_mode", t.plan_mode),
            ("brief_mode", t.brief_mode),
            ("fast_mode", t.fast_mode),
            ("fast_mode_shimmer", t.fast_mode_shimmer),
        ]
    }

    /// Every constructor (named themes plus the ones omitted from the
    /// onboarding list) must produce a struct without panicking. This
    /// guards against a stray bad `Color::Rgb` triple slipping in.
    #[test]
    fn every_constructor_returns_successfully() {
        let _ = Theme::midnight();
        let _ = Theme::daybreak();
        let _ = Theme::midnight_muted();
        let _ = Theme::daybreak_muted();
        let _ = Theme::terminal();
        let _ = Theme::dark_colorblind();
        let _ = Theme::light_colorblind();
        let _ = Theme::dark_ansi();
        let _ = Theme::light_ansi();
    }

    type ThemeCtor = fn() -> Theme;

    fn all_constructors() -> &'static [(&'static str, ThemeCtor)] {
        &[
            ("midnight", Theme::midnight),
            ("daybreak", Theme::daybreak),
            ("midnight_muted", Theme::midnight_muted),
            ("daybreak_muted", Theme::daybreak_muted),
            ("terminal", Theme::terminal),
            ("dark_colorblind", Theme::dark_colorblind),
            ("light_colorblind", Theme::light_colorblind),
            ("dark_ansi", Theme::dark_ansi),
            ("light_ansi", Theme::light_ansi),
        ]
    }

    #[test]
    fn every_new_slot_is_populated_in_every_theme() {
        // `Color::Reset` is the "I forgot to set this" footgun for any
        // emphasis slot — none of the new slots want to inherit. (The
        // existing `text` slot is allowed to be `Reset`; the new slots
        // are visual emphasis, where "inherit" produces no emphasis.)
        for (name, ctor) in all_constructors() {
            let t = ctor();
            for (label, c) in new_slots(&t) {
                assert!(
                    !matches!(c, Color::Reset),
                    "theme {name} has unset {label} slot"
                );
            }
        }
    }

    #[test]
    fn colorblind_themes_only_use_okabe_ito_palette_for_new_slots() {
        // Allowlist: the seven canonical Okabe-Ito hex values plus
        // black and white. Every new slot in either colourblind theme
        // must resolve to one of these.
        fn is_allowed(c: Color) -> bool {
            const ALLOWED: &[(u8, u8, u8)] = &[
                (0xE6, 0x9F, 0x00), // orange
                (0x56, 0xB4, 0xE9), // sky-blue
                (0x00, 0x9E, 0x73), // bluish-green
                (0xF0, 0xE4, 0x42), // yellow
                (0x00, 0x72, 0xB2), // blue
                (0xD5, 0x5E, 0x00), // vermillion
                (0xCC, 0x79, 0xA7), // reddish-purple
            ];
            match c {
                Color::Rgb { r, g, b } => ALLOWED.contains(&(r, g, b)),
                Color::Black | Color::White => true,
                _ => false,
            }
        }

        for (name, theme) in [
            ("dark_colorblind", Theme::dark_colorblind()),
            ("light_colorblind", Theme::light_colorblind()),
        ] {
            for (label, c) in new_slots(&theme) {
                assert!(
                    is_allowed(c),
                    "{name} slot {label} uses non-Okabe-Ito colour {c:?}"
                );
            }
        }
    }
}
