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
    /// Whether this is a dark theme (affects background choices).
    pub is_dark: bool,
}

impl Theme {
    /// Midnight — dark theme with cyan accents.
    pub fn midnight() -> Self {
        Self {
            accent: Color::Rgb {
                r: 164,
                g: 34,
                b: 225,
            },
            error: Color::Rgb {
                r: 255,
                g: 107,
                b: 128,
            },
            warning: Color::Rgb {
                r: 255,
                g: 193,
                b: 7,
            },
            success: Color::Rgb {
                r: 78,
                g: 186,
                b: 101,
            },
            muted: Color::Rgb {
                r: 100,
                g: 100,
                b: 100,
            },
            inactive: Color::Rgb {
                r: 153,
                g: 153,
                b: 153,
            },
            tool: Color::Rgb {
                r: 177,
                g: 185,
                b: 249,
            },
            plan: Color::Rgb {
                r: 72,
                g: 150,
                b: 140,
            },
            text: Color::White,
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
            is_dark: true,
        }
    }

    /// Daybreak — light theme with blue accents.
    pub fn daybreak() -> Self {
        Self {
            accent: Color::Rgb {
                r: 130,
                g: 20,
                b: 180,
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
            is_dark: false,
        }
    }

    /// Midnight Muted — softer dark theme.
    pub fn midnight_muted() -> Self {
        let mut t = Self::midnight();
        t.accent = Color::Rgb {
            r: 140,
            g: 60,
            b: 190,
        };
        t.error = Color::Rgb {
            r: 200,
            g: 100,
            b: 110,
        };
        t.warning = Color::Rgb {
            r: 200,
            g: 170,
            b: 60,
        };
        t.success = Color::Rgb {
            r: 80,
            g: 160,
            b: 90,
        };
        t.tool = Color::Rgb {
            r: 150,
            g: 155,
            b: 200,
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
        t
    }

    /// Terminal — uses standard 16 ANSI colors only.
    pub fn terminal() -> Self {
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
            is_dark: true,
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

use std::sync::OnceLock;

static ACTIVE_THEME: OnceLock<Theme> = OnceLock::new();

/// Initialize the global theme. Call once at startup.
pub fn init(theme_name: &str) {
    let _ = ACTIVE_THEME.set(Theme::from_name(theme_name));
}

/// Get the active theme. Falls back to midnight if not initialized.
pub fn current() -> &'static Theme {
    ACTIVE_THEME.get_or_init(Theme::midnight)
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
}
