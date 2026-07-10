//! Full-screen modern TUI (alt-screen ratatui pager).
//!
//! This is the **default** (and only) interactive surface. See
//! `docs/design/tui-modern-overhaul.md`.

mod app;
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

/// Interactive surface kind.
///
/// Only [`Self::Modern`] is supported. Legacy `"classic"` / `"repl"` /
/// `"legacy"` names are accepted by [`Self::parse`] and resolve to
/// modern so old config/env values do not break startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiKind {
    /// Full-screen ratatui app (default and only interactive UI).
    #[default]
    Modern,
}

impl TuiKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            // Modern (and legacy aliases that used to mean classic — now
            // remapped so existing configs keep working after the flip).
            "modern" | "fullscreen" | "tui" | "classic" | "repl" | "legacy" | "" => {
                Some(Self::Modern)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        "modern"
    }

    /// True when the raw string was a legacy classic name (for a one-shot warn).
    pub fn is_legacy_classic_name(s: &str) -> bool {
        matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "classic" | "repl" | "legacy"
        )
    }
}

/// Resolve TUI kind: CLI flag > env `AGENT_CODE_TUI` > config `ui.tui` > modern.
///
/// Always yields [`TuiKind::Modern`]. Invalid values return `None` from
/// [`TuiKind::parse`]; callers should error on unknown strings.
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
    fn parse_modern_and_legacy_aliases() {
        assert_eq!(TuiKind::parse("modern"), Some(TuiKind::Modern));
        assert_eq!(TuiKind::parse("fullscreen"), Some(TuiKind::Modern));
        // Classic is gone — aliases still parse so old configs boot.
        assert_eq!(TuiKind::parse("classic"), Some(TuiKind::Modern));
        assert_eq!(TuiKind::parse("repl"), Some(TuiKind::Modern));
        assert_eq!(TuiKind::parse("nope"), None);
    }

    #[test]
    fn resolve_defaults_to_modern() {
        assert_eq!(resolve_tui_kind(None, ""), TuiKind::Modern);
        assert_eq!(resolve_tui_kind(None, "classic"), TuiKind::Modern);
        assert_eq!(resolve_tui_kind(Some("modern"), "classic"), TuiKind::Modern);
    }

    #[test]
    fn legacy_classic_name_detection() {
        assert!(TuiKind::is_legacy_classic_name("classic"));
        assert!(!TuiKind::is_legacy_classic_name("modern"));
    }
}
