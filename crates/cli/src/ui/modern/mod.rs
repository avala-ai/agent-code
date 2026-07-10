//! Full-screen modern TUI (alt-screen ratatui pager).
//!
//! Default interactive surface. Opt into the classic rustyline REPL with
//! `--tui classic` or `[ui] tui = "classic"`. See
//! `docs/design/tui-modern-overhaul.md`.

mod app;
mod colors;
mod layout;
mod markdown;
mod modal;
mod mode;
mod render;
mod run;
mod scroll;
mod sink;
mod stream_buffer;
mod tasks;
mod terminal_caps;
mod toolcard;

pub use run::run_modern_tui;

/// Which interactive surface to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiKind {
    /// Line-oriented rustyline REPL.
    Classic,
    /// Full-screen ratatui app (default).
    #[default]
    Modern,
}

impl TuiKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "classic" | "repl" | "legacy" => Some(Self::Classic),
            "modern" | "fullscreen" | "tui" => Some(Self::Modern),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Modern => "modern",
        }
    }
}

/// Resolve TUI kind: CLI flag > env `AGENT_CODE_TUI` > config `ui.tui` > modern.
pub fn resolve_tui_kind(cli_flag: Option<&str>, config_value: &str) -> TuiKind {
    if let Some(s) = cli_flag
        && let Some(k) = TuiKind::parse(s)
    {
        return k;
    }
    if let Ok(env) = std::env::var("AGENT_CODE_TUI")
        && let Some(k) = TuiKind::parse(&env)
    {
        return k;
    }
    TuiKind::parse(config_value).unwrap_or(TuiKind::Modern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aliases() {
        assert_eq!(TuiKind::parse("modern"), Some(TuiKind::Modern));
        assert_eq!(TuiKind::parse("fullscreen"), Some(TuiKind::Modern));
        assert_eq!(TuiKind::parse("classic"), Some(TuiKind::Classic));
        assert_eq!(TuiKind::parse("nope"), None);
    }

    #[test]
    fn resolve_prefers_cli() {
        assert_eq!(resolve_tui_kind(Some("modern"), "classic"), TuiKind::Modern);
        assert_eq!(
            resolve_tui_kind(Some("classic"), "modern"),
            TuiKind::Classic
        );
    }

    #[test]
    fn resolve_defaults_to_modern() {
        assert_eq!(resolve_tui_kind(None, ""), TuiKind::Modern);
        assert_eq!(resolve_tui_kind(None, "garbage"), TuiKind::Modern);
    }
}
