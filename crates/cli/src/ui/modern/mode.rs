//! Session interaction modes for the modern TUI.
//!
//! Session interaction modes: a single key cycles planning, normal
//! work, and elevated permission shortcuts.

use agent_code_lib::config::PermissionMode;

/// User-visible session mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionMode {
    /// Default interactive behaviour — permissions from config.
    #[default]
    Normal,
    /// Read-only exploration / planning (engine `plan_mode`).
    Plan,
    /// Auto-allow file edits; other mutations still follow config.
    AcceptEdits,
    /// Auto-allow tool calls (subject to `disable_bypass_permissions`).
    AlwaysApprove,
}

impl SessionMode {
    pub const ALL: [SessionMode; 4] = [
        SessionMode::Normal,
        SessionMode::Plan,
        SessionMode::AcceptEdits,
        SessionMode::AlwaysApprove,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Plan => "plan",
            Self::AcceptEdits => "accept-edits",
            Self::AlwaysApprove => "always-approve",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Plan => "PLAN",
            Self::AcceptEdits => "ACCEPT",
            Self::AlwaysApprove => "YOLO",
        }
    }

    pub fn cycle_next(self) -> Self {
        match self {
            Self::Normal => Self::Plan,
            Self::Plan => Self::AcceptEdits,
            Self::AcceptEdits => Self::AlwaysApprove,
            Self::AlwaysApprove => Self::Normal,
        }
    }

    pub fn cycle_prev(self) -> Self {
        match self {
            Self::Normal => Self::AlwaysApprove,
            Self::Plan => Self::Normal,
            Self::AcceptEdits => Self::Plan,
            Self::AlwaysApprove => Self::AcceptEdits,
        }
    }

    /// Permission overlay hint for status display / future engine wiring.
    pub fn permission_hint(self) -> Option<PermissionMode> {
        match self {
            Self::Normal => None,
            Self::Plan => Some(PermissionMode::Plan),
            Self::AcceptEdits => Some(PermissionMode::AcceptEdits),
            Self::AlwaysApprove => Some(PermissionMode::Allow),
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
    fn plan_maps_to_plan_permission() {
        assert_eq!(
            SessionMode::Plan.permission_hint(),
            Some(PermissionMode::Plan)
        );
    }
}
