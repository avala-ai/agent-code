//! Full-screen modern TUI (alt-screen ratatui pager).
//!
//! Alternative to the classic rustyline REPL. Opt in with `--tui modern`
//! or `[ui] tui = "modern"`. See `docs/design/tui-modern-overhaul.md`.

mod app;
mod mode;
mod render;
mod run;
mod sink;

pub use run::run_modern_tui;

/// Which interactive surface to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiKind {
    /// Line-oriented rustyline REPL (legacy default).
    #[default]
    Classic,
    /// Full-screen ratatui app (modern overhaul).
    Modern,
}

impl TuiKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "classic" | "repl" | "legacy" => Some(Self::Classic),
            "modern" | "fullscreen" | "build" | "tui" => Some(Self::Modern),
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

/// Resolve TUI kind: CLI flag > env `AGENT_CODE_TUI` > config `ui.tui` > classic.
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
    TuiKind::parse(config_value).unwrap_or(TuiKind::Classic)
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
}
