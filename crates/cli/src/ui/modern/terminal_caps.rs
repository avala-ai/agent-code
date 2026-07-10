//! Terminal capability probe for the modern TUI (plan §M7).
//!
//! Kept cheap and synchronous: capabilities are inferred from environment
//! heuristics and crossterm's keyboard-enhancement query (which has its own
//! timeout), never from blocking escape-sequence round-trips — so startup
//! never hangs on a silent terminal. The probe feeds the synchronized-output
//! flicker fix and the `/terminal-setup` diagnostics.

/// Detected terminal capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalCaps {
    /// Terminal supports DEC 2026 synchronized output (flicker-free redraw).
    pub sync_output: bool,
    /// 24-bit color (`COLORTERM=truecolor|24bit`).
    pub truecolor: bool,
    /// Kitty keyboard protocol available (disambiguated keys, Shift+Enter).
    pub kitty_keyboard: bool,
    /// Running inside tmux (passthrough needed for OSC 52 / queries).
    pub tmux: bool,
}

impl TerminalCaps {
    /// Probe from the current environment. `enhancement` is the result of
    /// `crossterm::terminal::supports_keyboard_enhancement()` (passed in so
    /// this stays pure and testable).
    pub fn detect(get: impl Fn(&str) -> Option<String>, enhancement: bool) -> TerminalCaps {
        let term_program = get("TERM_PROGRAM").unwrap_or_default().to_lowercase();
        let term = get("TERM").unwrap_or_default().to_lowercase();
        let colorterm = get("COLORTERM").unwrap_or_default().to_lowercase();
        let tmux = get("TMUX").is_some() || term.starts_with("tmux") || term.starts_with("screen");

        let truecolor = colorterm.contains("truecolor") || colorterm.contains("24bit");

        // Terminals known to implement synchronized output well. Default off
        // for unknown terminals (config can force it on/off — see §9).
        let sync_known = [
            "kitty",
            "wezterm",
            "ghostty",
            "iterm",
            "iterm2",
            "alacritty",
        ];
        let sync_output = sync_known
            .iter()
            .any(|t| term_program.contains(t) || term.contains(t) || get("WEZTERM_PANE").is_some())
            || get("KITTY_WINDOW_ID").is_some();

        TerminalCaps {
            sync_output,
            truecolor,
            kitty_keyboard: enhancement,
            tmux,
        }
    }

    /// Remediation lines for `/terminal-setup`, keyed to detected gaps.
    pub fn remediation(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.tmux {
            out.push("set -g allow-passthrough on      # OSC 52 & queries through tmux".into());
            out.push("set -g focus-events on".into());
            out.push("set -g set-clipboard on".into());
        }
        if !self.kitty_keyboard {
            out.push(
                "Shift+Enter needs the kitty keyboard protocol — use Alt+Enter instead.".into(),
            );
        }
        if !self.truecolor {
            out.push("Set COLORTERM=truecolor for 24-bit color.".into());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn detects_truecolor_and_tmux() {
        let caps = TerminalCaps::detect(
            env(&[("COLORTERM", "truecolor"), ("TMUX", "/tmp/x")]),
            false,
        );
        assert!(caps.truecolor);
        assert!(caps.tmux);
        assert!(!caps.kitty_keyboard);
    }

    #[test]
    fn kitty_terminal_gets_sync_output() {
        let caps = TerminalCaps::detect(env(&[("KITTY_WINDOW_ID", "1")]), true);
        assert!(caps.sync_output);
        assert!(caps.kitty_keyboard);
    }

    #[test]
    fn unknown_terminal_defaults_sync_off() {
        let caps = TerminalCaps::detect(env(&[("TERM", "xterm")]), false);
        assert!(!caps.sync_output);
    }

    #[test]
    fn remediation_mentions_tmux_passthrough() {
        let caps = TerminalCaps::detect(env(&[("TMUX", "/tmp/x")]), false);
        let r = caps.remediation().join("\n");
        assert!(r.contains("allow-passthrough"));
    }
}
