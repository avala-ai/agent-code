//! Startup warnings for the launch surface.
//!
//! Maps [`super::diagnostics`] checks into a small, banner-ready form.
//! There is intentionally no second diagnostic path — `/doctor` and the
//! launch screen share the same checks; only the presentation differs.
//!
//! Banner rule (single slot): show the first `Warning`, else the last
//! entry. Messages are truncated to fit a typical terminal width.

use super::diagnostics::{Check, CheckStatus};

/// How loudly a startup warning should be painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningSeverity {
    /// Something is broken or blocking — yellow/warn chrome.
    Warning,
    /// Informational — dim chrome.
    Info,
}

/// A single startup problem ready for the launch banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupWarning {
    pub severity: WarningSeverity,
    pub message: String,
    /// Optional remediation hint, e.g. "Run /doctor for details and fixes."
    pub action: Option<String>,
}

/// Soft column budget for the banner message line.
const MAX_MESSAGE_COLS: usize = 60;

/// Map a single doctor check into a startup warning.
///
/// Passes are dropped. Failures become `Warning` with a `/doctor` action;
/// warnings become `Info` without an action (they're advisory).
pub fn from_check(check: &Check) -> Option<StartupWarning> {
    match check.status {
        CheckStatus::Pass => None,
        CheckStatus::Fail => Some(StartupWarning {
            severity: WarningSeverity::Warning,
            message: truncate_message(&check.detail, MAX_MESSAGE_COLS),
            action: Some("Run /doctor for details and fixes.".into()),
        }),
        CheckStatus::Warn => Some(StartupWarning {
            severity: WarningSeverity::Info,
            message: truncate_message(&check.detail, MAX_MESSAGE_COLS),
            action: None,
        }),
    }
}

/// Map a full doctor run into launch-screen warnings (passes dropped).
pub fn from_checks(checks: &[Check]) -> Vec<StartupWarning> {
    checks.iter().filter_map(from_check).collect()
}

/// Single-slot banner rule: first `Warning`, else the last entry.
pub fn pick_banner(warnings: &[StartupWarning]) -> Option<&StartupWarning> {
    warnings
        .iter()
        .find(|w| w.severity == WarningSeverity::Warning)
        .or_else(|| warnings.last())
}

fn truncate_message(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            // Prefer a clean ellipsis over a mid-glyph cut.
            if out.ends_with(' ') {
                out.pop();
            }
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(detail: &str) -> Check {
        Check {
            name: "t".into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
        }
    }

    fn warn(detail: &str) -> Check {
        Check {
            name: "t".into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
        }
    }

    fn pass(detail: &str) -> Check {
        Check {
            name: "t".into(),
            status: CheckStatus::Pass,
            detail: detail.into(),
        }
    }

    #[test]
    fn passes_are_dropped() {
        assert!(from_check(&pass("ok")).is_none());
        assert!(from_checks(&[pass("a"), pass("b")]).is_empty());
    }

    #[test]
    fn fail_becomes_warning_with_doctor_action() {
        let w = from_check(&fail("No API key set")).unwrap();
        assert_eq!(w.severity, WarningSeverity::Warning);
        assert_eq!(w.message, "No API key set");
        assert_eq!(
            w.action.as_deref(),
            Some("Run /doctor for details and fixes.")
        );
    }

    #[test]
    fn warn_becomes_info_without_action() {
        let w = from_check(&warn("optional tool missing")).unwrap();
        assert_eq!(w.severity, WarningSeverity::Info);
        assert!(w.action.is_none());
    }

    #[test]
    fn long_messages_are_truncated() {
        let long = "x".repeat(80);
        let w = from_check(&fail(&long)).unwrap();
        assert!(w.message.chars().count() <= MAX_MESSAGE_COLS + 1); // + ellipsis
        assert!(w.message.ends_with('…'));
    }

    #[test]
    fn banner_prefers_first_warning_over_later_info() {
        let ws = from_checks(&[
            warn("info first"),
            fail("broken"),
            warn("info later"),
            fail("also broken"),
        ]);
        let banner = pick_banner(&ws).unwrap();
        assert_eq!(banner.severity, WarningSeverity::Warning);
        assert_eq!(banner.message, "broken");
    }

    #[test]
    fn banner_falls_back_to_last_when_no_warning() {
        let ws = from_checks(&[warn("a"), warn("b"), warn("c")]);
        let banner = pick_banner(&ws).unwrap();
        assert_eq!(banner.message, "c");
    }

    #[test]
    fn empty_banner_is_none() {
        assert!(pick_banner(&[]).is_none());
        assert!(pick_banner(&from_checks(&[pass("ok")])).is_none());
    }
}
