//! Session-level warning registry.
//!
//! A simple append-only registry for non-fatal warnings surfaced to
//! the user in the REPL — things like "dangerous permission flag set",
//! "`gh` not on PATH, PR commands will fail", or "stale config key
//! ignored". The TUI renders the pending warnings as a banner at
//! startup and before each prompt so they're noticed without breaking
//! the stream.
//!
//! Design notes:
//!
//! - Messages are de-duplicated on push: a warning fired twice only
//!   shows up once. Callers can freely re-push on every check without
//!   worrying about noise.
//! - The registry is a process-wide singleton. There is one session at
//!   a time, so sharing is safe and removes plumbing.
//! - No logging integration on purpose — warnings here are *user-facing*
//!   and distinct from `tracing::warn!`, which routes to logs.

use std::sync::{Mutex, OnceLock};

/// Severity of a user-facing warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLevel {
    /// Informational — user should know but nothing is at risk.
    Info,
    /// Caution — something is disabled, missing, or dangerous.
    Warn,
}

impl WarningLevel {
    /// Short label used by the TUI banner renderer.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
        }
    }
}

/// A single registered warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub level: WarningLevel,
    pub message: String,
}

static REGISTRY: OnceLock<Mutex<Vec<Warning>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<Warning>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Push a warning. If a warning with identical `level` and `message`
/// is already registered, this is a no-op (the registry de-duplicates
/// by content, so callers can be noisy).
pub fn push(level: WarningLevel, message: impl Into<String>) {
    let message = message.into();
    let mut guard = registry().lock().unwrap();
    let candidate = Warning {
        level,
        message: message.clone(),
    };
    if guard.iter().any(|w| w == &candidate) {
        return;
    }
    guard.push(candidate);
}

/// Shorthand for `push(WarningLevel::Warn, ...)`.
pub fn warn(message: impl Into<String>) {
    push(WarningLevel::Warn, message);
}

/// Shorthand for `push(WarningLevel::Info, ...)`.
pub fn info(message: impl Into<String>) {
    push(WarningLevel::Info, message);
}

/// Snapshot the current warnings without clearing. The TUI calls this
/// each time it wants to render the banner. Returns an empty Vec when
/// nothing is registered.
pub fn snapshot() -> Vec<Warning> {
    registry().lock().unwrap().clone()
}

/// Clear all warnings. Used by tests and by any explicit "dismiss all"
/// UX. Production callers should prefer `snapshot()` and only clear
/// when the user explicitly dismisses.
pub fn clear() {
    registry().lock().unwrap().clear();
}

/// Number of registered warnings — used by tests and by the status
/// command to decide whether to render the banner.
pub fn len() -> usize {
    registry().lock().unwrap().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize tests against the global registry. Tests in this
    // module share a singleton so they can't run in parallel without
    // clobbering each other's state.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn setup() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        guard
    }

    #[test]
    fn push_and_snapshot_roundtrip() {
        let _lock = setup();
        warn("first");
        info("second");
        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].level, WarningLevel::Warn);
        assert_eq!(snap[0].message, "first");
        assert_eq!(snap[1].level, WarningLevel::Info);
        assert_eq!(snap[1].message, "second");
    }

    #[test]
    fn duplicate_push_is_ignored() {
        let _lock = setup();
        warn("same");
        warn("same");
        warn("same");
        assert_eq!(len(), 1);
    }

    #[test]
    fn different_levels_same_message_are_distinct() {
        let _lock = setup();
        warn("x");
        info("x");
        assert_eq!(len(), 2);
    }

    #[test]
    fn clear_empties_registry() {
        let _lock = setup();
        warn("something");
        assert_eq!(len(), 1);
        clear();
        assert_eq!(len(), 0);
    }

    #[test]
    fn snapshot_does_not_drain() {
        let _lock = setup();
        warn("sticky");
        let _ = snapshot();
        let _ = snapshot();
        assert_eq!(len(), 1);
    }
}
