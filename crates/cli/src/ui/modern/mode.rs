//! Session interaction modes for the modern TUI.
//!
//! Session interaction modes: a single key cycles planning, normal
//! work, and elevated permission shortcuts.

use agent_code_lib::config::PermissionMode;

/// User-visible session mode.
///
/// The canonical cycle is `Manual → Normal → AcceptEdits → Plan → Manual`
/// (plan of record §3.3 / #404). There is deliberately **no** always-approve
/// / YOLO mode in the interactive cycle: auto-allowing every tool is a
/// config-level decision (`[permissions] default_mode = "allow"`), and the
/// sandbox-bypass axis is enforced by the engine via
/// `security.disable_bypass_permissions` — not by a UI mode.
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
    /// Read-only exploration / planning (engine `plan_mode`).
    Plan,
}

impl SessionMode {
    pub const ALL: [SessionMode; 4] = [
        SessionMode::Manual,
        SessionMode::Normal,
        SessionMode::AcceptEdits,
        SessionMode::Plan,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Normal => "normal",
            Self::AcceptEdits => "accept-edits",
            Self::Plan => "plan",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            Self::Manual => "MANUAL",
            Self::Normal => "NORMAL",
            Self::AcceptEdits => "ACCEPT",
            Self::Plan => "PLAN",
        }
    }

    pub fn cycle_next(self) -> Self {
        match self {
            Self::Manual => Self::Normal,
            Self::Normal => Self::AcceptEdits,
            Self::AcceptEdits => Self::Plan,
            Self::Plan => Self::Manual,
        }
    }

    pub fn cycle_prev(self) -> Self {
        match self {
            Self::Manual => Self::Plan,
            Self::Normal => Self::Manual,
            Self::AcceptEdits => Self::Normal,
            Self::Plan => Self::AcceptEdits,
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
        for _ in 0..4 {
            m = m.cycle_next();
        }
        assert_eq!(m, SessionMode::Normal);
        for _ in 0..4 {
            m = m.cycle_prev();
        }
        assert_eq!(m, SessionMode::Normal);
    }

    #[test]
    fn canonical_cycle_order() {
        // Plan of record §3.3 / #404: Manual → Normal → AcceptEdits → Plan.
        assert_eq!(SessionMode::Manual.cycle_next(), SessionMode::Normal);
        assert_eq!(SessionMode::Normal.cycle_next(), SessionMode::AcceptEdits);
        assert_eq!(SessionMode::AcceptEdits.cycle_next(), SessionMode::Plan);
        assert_eq!(SessionMode::Plan.cycle_next(), SessionMode::Manual);
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
