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
    /// The protocol is available **and** safe to actually turn on here.
    /// See [`ENHANCEMENT_DENYLIST`].
    pub kitty_keyboard_safe: bool,
    /// Running inside tmux (passthrough needed for OSC 52 / queries).
    pub tmux: bool,
    /// Terminal is known to honour OSC 8 hyperlinks (clickable URLs).
    /// When false, links still render underlined and open on click via
    /// the hit-rect registry, but we do not emit OSC 8 sequences.
    pub osc8_hyperlinks: bool,
    /// Terminal is believed to honour OSC 22 pointer-shape changes
    /// (pointer over hyperlinks — #558 D10-18).
    pub osc22_pointer: bool,
}

/// `TERM_PROGRAM` markers for hosts that answer the keyboard-enhancement
/// query affirmatively but mis-encode shifted printable characters once
/// the protocol is pushed — typing `A` or `?` starts arriving as escape
/// noise. These are the browser-engine terminal widgets embedded in code
/// editors; they share one upstream emulator, so the marker list is short.
/// Matched case-insensitively as a substring so editor forks that keep the
/// upstream marker are covered too.
const ENHANCEMENT_DENYLIST: &[&str] = &["vscode", "cursor", "windsurf", "zed"];

/// Whether the keyboard-enhancement flags may be pushed on this host.
///
/// Pure so the decision is unit-testable without a terminal: `enhancement`
/// is what the terminal answered, `term_program` is `$TERM_PROGRAM`.
pub fn keyboard_enhancement_allowed(enhancement: bool, term_program: &str) -> bool {
    if !enhancement {
        return false;
    }
    let program = term_program.to_lowercase();
    !ENHANCEMENT_DENYLIST.iter().any(|p| program.contains(p))
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

        // OSC 8 is well supported on the same modern hosts that do
        // synchronized output; leave off for unknown / bare `screen`.
        let osc8_hyperlinks = sync_output
            || term_program.contains("kitty")
            || term_program.contains("wezterm")
            || term_program.contains("ghostty")
            || term_program.contains("iterm")
            || term.contains("xterm-kitty");

        // OSC 22 pointer shapes: kitty / wezterm / ghostty today.
        let osc22_pointer = term_program.contains("kitty")
            || term_program.contains("wezterm")
            || term_program.contains("ghostty")
            || get("KITTY_WINDOW_ID").is_some()
            || get("WEZTERM_PANE").is_some();

        TerminalCaps {
            sync_output,
            truecolor,
            kitty_keyboard: enhancement,
            kitty_keyboard_safe: keyboard_enhancement_allowed(enhancement, &term_program),
            tmux,
            osc8_hyperlinks,
            osc22_pointer,
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
        } else if !self.kitty_keyboard_safe {
            out.push(
                "This host reports the kitty keyboard protocol but mis-encodes shifted keys \
                 under it, so it is left off — use Alt+Enter / Ctrl+I."
                    .into(),
            );
        }
        if !self.truecolor {
            out.push("Set COLORTERM=truecolor for 24-bit color.".into());
        }
        if !self.osc8_hyperlinks {
            out.push(
                "OSC 8 hyperlinks are off on this host — click a link in the transcript to open it, \
                 or use a terminal that supports OSC 8 (kitty, WezTerm, Ghostty, iTerm2)."
                    .into(),
            );
        }
        if !self.osc22_pointer {
            out.push(
                "OSC 22 pointer shapes are off — link hover will not change the mouse cursor."
                    .into(),
            );
        }
        out
    }
}

/// Emit OSC 22 pointer shape (`pointer` for links, empty/`default` to reset).
///
/// Best-effort write to stdout; failures are ignored (tests / redirected
/// stdout). Shape names follow the common kitty/wezterm set.
pub fn set_pointer_shape(shape: &str) {
    use std::io::Write;
    // OSC 22 ; <shape> ST  — empty shape restores the default.
    let seq = format!("\x1b]22;{shape}\x1b\\");
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
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
        assert!(caps.osc8_hyperlinks, "kitty should advertise OSC 8");
        assert!(caps.osc22_pointer, "kitty should advertise OSC 22");
    }

    #[test]
    fn unknown_terminal_defaults_sync_off() {
        let caps = TerminalCaps::detect(env(&[("TERM", "xterm")]), false);
        assert!(!caps.sync_output);
        assert!(!caps.osc8_hyperlinks);
        assert!(!caps.osc22_pointer);
    }

    #[test]
    fn enhancement_allowed_only_when_supported() {
        assert!(keyboard_enhancement_allowed(true, "kitty"));
        assert!(keyboard_enhancement_allowed(true, ""));
        assert!(!keyboard_enhancement_allowed(false, "kitty"));
        // Unsupported wins even on a host that is not denylisted.
        assert!(!keyboard_enhancement_allowed(false, ""));
    }

    #[test]
    fn editor_embedded_hosts_never_get_the_protocol() {
        // These hosts answer the query with "yes" but garble shifted
        // printables once the flags are pushed.
        for program in ["vscode", "Cursor", "Windsurf", "zed", "vscode-insiders"] {
            assert!(
                !keyboard_enhancement_allowed(true, program),
                "{program} must be denied"
            );
        }
    }

    #[test]
    fn detect_marks_denylisted_host_supported_but_unsafe() {
        let caps = TerminalCaps::detect(env(&[("TERM_PROGRAM", "vscode")]), true);
        assert!(caps.kitty_keyboard, "the terminal did answer yes");
        assert!(!caps.kitty_keyboard_safe, "but we must not enable it");
        let r = caps.remediation().join("\n");
        assert!(r.contains("mis-encodes"), "{r}");
    }

    #[test]
    fn detect_marks_normal_host_safe() {
        let caps = TerminalCaps::detect(env(&[("TERM_PROGRAM", "ghostty")]), true);
        assert!(caps.kitty_keyboard && caps.kitty_keyboard_safe);
        assert!(!caps.remediation().iter().any(|l| l.contains("mis-encodes")));
    }

    #[test]
    fn remediation_mentions_tmux_passthrough() {
        let caps = TerminalCaps::detect(env(&[("TMUX", "/tmp/x")]), false);
        let r = caps.remediation().join("\n");
        assert!(r.contains("allow-passthrough"));
    }
}
