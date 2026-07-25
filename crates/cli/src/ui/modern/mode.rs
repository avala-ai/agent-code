//! Session interaction modes for the modern TUI.
//!
//! Session interaction modes: a single key cycles planning, normal
//! work, and elevated permission shortcuts.

use agent_code_lib::config::PermissionMode;

/// User-visible session mode.
///
/// The canonical cycle is `Manual → Normal → AcceptEdits → Auto → Plan →
/// Manual` (plan of record §3.3 / #404). There is deliberately **no**
/// always-approve / YOLO mode in the interactive cycle: auto-allowing every
/// tool is a config-level decision (`[permissions] default_mode = "allow"`),
/// and the sandbox-bypass axis is enforced by the engine via
/// `security.disable_bypass_permissions` — not by a UI mode. `Auto` is not
/// that mode: it approves only provably read-only shell commands and still
/// prompts for everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionMode {
    /// Prompt for every tool call, overriding config auto-allow rules
    /// (engine `PermissionMode::Ask`). The strictest interactive mode.
    Manual,
    /// Default interactive behaviour — permissions from config.
    #[default]
    Normal,
    /// Auto-allow file edits; other mutations still follow config.
    AcceptEdits,
    /// Auto-allow file edits *and* provably read-only shell commands.
    /// Strictly more permissive than [`SessionMode::AcceptEdits`], which is
    /// why it sits after it in the cycle. Mutating, networked, privileged
    /// and unrecognised commands still prompt.
    Auto,
    /// Read-only exploration / planning (engine `plan_mode`).
    Plan,
}

impl SessionMode {
    pub const ALL: [SessionMode; 5] = [
        SessionMode::Manual,
        SessionMode::Normal,
        SessionMode::AcceptEdits,
        SessionMode::Auto,
        SessionMode::Plan,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Normal => "normal",
            Self::AcceptEdits => "accept-edits",
            Self::Auto => "auto",
            Self::Plan => "plan",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            Self::Manual => "MANUAL",
            Self::Normal => "NORMAL",
            Self::AcceptEdits => "ACCEPT",
            Self::Auto => "AUTO",
            Self::Plan => "PLAN",
        }
    }

    pub fn cycle_next(self) -> Self {
        match self {
            Self::Manual => Self::Normal,
            Self::Normal => Self::AcceptEdits,
            Self::AcceptEdits => Self::Auto,
            Self::Auto => Self::Plan,
            Self::Plan => Self::Manual,
        }
    }

    pub fn cycle_prev(self) -> Self {
        match self {
            Self::Manual => Self::Plan,
            Self::Normal => Self::Manual,
            Self::AcceptEdits => Self::Normal,
            Self::Auto => Self::AcceptEdits,
            Self::Plan => Self::Auto,
        }
    }

    /// Permission mode this session mode imposes on the engine, or `None` to
    /// fall back to the config default. Encodes the whole mode→permission
    /// policy here (a table), so the event loop never special-cases modes.
    pub fn permission_hint(self) -> Option<PermissionMode> {
        match self {
            Self::Manual => Some(PermissionMode::Ask),
            Self::Normal => None,
            Self::AcceptEdits => Some(PermissionMode::AcceptEdits),
            Self::Auto => Some(PermissionMode::Auto),
            Self::Plan => Some(PermissionMode::Plan),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_is_circular() {
        let mut m = SessionMode::Normal;
        for _ in 0..SessionMode::ALL.len() {
            m = m.cycle_next();
        }
        assert_eq!(m, SessionMode::Normal);
        for _ in 0..SessionMode::ALL.len() {
            m = m.cycle_prev();
        }
        assert_eq!(m, SessionMode::Normal);
    }

    #[test]
    fn canonical_cycle_order() {
        // Manual → Normal → AcceptEdits → Auto → Plan. Auto sits after
        // AcceptEdits because it is strictly more permissive: it adds
        // provably read-only shell commands on top of auto-accepted edits.
        assert_eq!(SessionMode::Manual.cycle_next(), SessionMode::Normal);
        assert_eq!(SessionMode::Normal.cycle_next(), SessionMode::AcceptEdits);
        assert_eq!(SessionMode::AcceptEdits.cycle_next(), SessionMode::Auto);
        assert_eq!(SessionMode::Auto.cycle_next(), SessionMode::Plan);
        assert_eq!(SessionMode::Plan.cycle_next(), SessionMode::Manual);
    }

    #[test]
    fn cycle_prev_is_the_inverse_of_cycle_next() {
        for m in SessionMode::ALL {
            assert_eq!(m.cycle_next().cycle_prev(), m);
            assert_eq!(m.cycle_prev().cycle_next(), m);
        }
    }

    #[test]
    fn every_mode_has_a_distinct_label_and_badge() {
        let labels: Vec<&str> = SessionMode::ALL.iter().map(|m| m.label()).collect();
        let badges: Vec<&str> = SessionMode::ALL.iter().map(|m| m.short_badge()).collect();
        for i in 0..SessionMode::ALL.len() {
            for j in (i + 1)..SessionMode::ALL.len() {
                assert_ne!(labels[i], labels[j]);
                assert_ne!(badges[i], badges[j]);
            }
        }
    }

    #[test]
    fn auto_maps_to_auto_permission() {
        assert_eq!(
            SessionMode::Auto.permission_hint(),
            Some(PermissionMode::Auto)
        );
        assert_eq!(SessionMode::Auto.label(), "auto");
        assert_eq!(SessionMode::Auto.short_badge(), "AUTO");
    }

    #[test]
    fn accept_edits_maps_to_accept_edits_permission() {
        assert_eq!(
            SessionMode::AcceptEdits.permission_hint(),
            Some(PermissionMode::AcceptEdits)
        );
    }

    #[test]
    fn normal_defers_to_config() {
        assert_eq!(SessionMode::Normal.permission_hint(), None);
    }

    #[test]
    fn plan_maps_to_plan_permission() {
        assert_eq!(
            SessionMode::Plan.permission_hint(),
            Some(PermissionMode::Plan)
        );
    }

    #[test]
    fn manual_forces_ask() {
        // Manual overrides config auto-allow — it must prompt for everything.
        assert_eq!(
            SessionMode::Manual.permission_hint(),
            Some(PermissionMode::Ask)
        );
    }
}
