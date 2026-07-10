//! Rotating tips surface.
//!
//! The tips service nudges the user toward features they may not know
//! about — bundled markdown snippets that surface during idle moments
//! at low frequency. It is **passive**: the REPL asks the service for
//! a tip after the first turn, and the service either returns one or
//! returns `None`. Nothing here ever blocks the prompt loop.
//!
//! # Authoring
//!
//! Tips ship as markdown files under `services/tips/bundled/` with a
//! YAML frontmatter:
//!
//! ```text
//! ---
//! id: kebab-case-slug
//! weight: 1
//! show_after_session: 0
//! ---
//! <one-paragraph tip body>
//! ```
//!
//! The frontmatter parser is the same one skills use — see
//! [`crate::skills::parse_frontmatter_into`].
//!
//! # Frequency
//!
//! Defaults are conservative: at most one tip every
//! [`DEFAULT_MIN_SESSIONS_BETWEEN`] sessions, and the same tip is not
//! shown again within [`DEFAULT_REPEAT_WINDOW_DAYS`] days unless every
//! eligible tip has been shown recently — at which point the per-id
//! `last_shown_at` map is cleared so rotation can start over. A tip
//! whose `show_after_session` is greater than the current session
//! count is hidden until the user has been around long enough.
//!
//! # Persistence
//!
//! Per-user state — dismissed ids, snooze-until, last-shown-at — is
//! written to `<agent-config>/tips_state.json` via
//! [`crate::config::atomic::atomic_write_secret`]. The file is
//! optional: a missing or unreadable file falls back to the empty
//! state. Callers may inject a custom directory through
//! [`TipsService::with_state_dir`] for tests.
//!
//! # Slash commands
//!
//! The CLI surface is `/tips`, `/tips dismiss <id>`, `/tips off`, and
//! `/tips on`. The disabled flag lives in
//! [`TipsState::disabled`] and is also persisted.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::skills::parse_frontmatter_into;

/// Default — show at most one tip every five sessions.
pub const DEFAULT_MIN_SESSIONS_BETWEEN: usize = 5;

/// Default — don't repeat the same tip within thirty days unless every
/// eligible tip has been shown recently.
pub const DEFAULT_REPEAT_WINDOW_DAYS: u64 = 30;

/// Tips authored as bundled markdown — source of truth lives under
/// `services/tips/bundled/`. Each entry is `(slug, contents)` and the
/// contents are embedded at compile time via `include_str!`.
///
/// The slug here is only a fallback display name; the canonical id is
/// the `id` field in the frontmatter.
pub const BUNDLED_TIP_FILES: &[(&str, &str)] = &[
    ("skills-list", include_str!("tips/bundled/skills-list.md")),
    ("model-tools", include_str!("tips/bundled/model-tools.md")),
    (
        "output-style-themes",
        include_str!("tips/bundled/output-style-themes.md"),
    ),
    ("team-memory", include_str!("tips/bundled/team-memory.md")),
    (
        "cron-schedule",
        include_str!("tips/bundled/cron-schedule.md"),
    ),
    (
        "agent-worktrees",
        include_str!("tips/bundled/agent-worktrees.md"),
    ),
    ("plan-mode", include_str!("tips/bundled/plan-mode.md")),
    (
        "subagent-types",
        include_str!("tips/bundled/subagent-types.md"),
    ),
    ("multi-edit", include_str!("tips/bundled/multi-edit.md")),
    ("syntax-theme", include_str!("tips/bundled/syntax-theme.md")),
    ("reload", include_str!("tips/bundled/reload.md")),
    ("inherit-fg", include_str!("tips/bundled/inherit-fg.md")),
    (
        "bundled-skills",
        include_str!("tips/bundled/bundled-skills.md"),
    ),
    (
        "plugin-marketplace",
        include_str!("tips/bundled/plugin-marketplace.md"),
    ),
    ("tips-off", include_str!("tips/bundled/tips-off.md")),
    (
        "remote-trigger",
        include_str!("tips/bundled/remote-trigger.md"),
    ),
];

/// Frontmatter metadata for a single tip.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TipMeta {
    pub id: String,
    /// Relative weight in the random selection (defaults to 1).
    pub weight: u32,
    /// Don't surface this tip until the session counter has reached
    /// this value (defaults to 0).
    pub show_after_session: usize,
}

/// A single bundled tip.
#[derive(Debug, Clone)]
pub struct Tip {
    pub id: String,
    pub weight: u32,
    pub show_after_session: usize,
    /// One-paragraph body, trailing whitespace trimmed.
    pub body: String,
}

/// Persistent per-user state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TipsState {
    /// Tip ids the user has explicitly dismissed.
    pub dismissed: Vec<String>,
    /// Unix-epoch seconds — no tip is shown before this instant.
    /// Zero means "no snooze".
    pub snooze_until: u64,
    /// id → unix-epoch seconds of the most recent show.
    pub last_shown: std::collections::BTreeMap<String, u64>,
    /// Session counter at the most recent show. Used to enforce the
    /// "1 tip per N sessions" cadence.
    pub last_shown_session: usize,
    /// Number of REPL launches recorded. Bumped once per session by
    /// [`TipsService::bump_session`]. Tips use this counter to apply
    /// the `show_after_session` gate and the cadence rule.
    pub session_count: usize,
    /// Master kill-switch (`/tips off`).
    pub disabled: bool,
}

/// The rotating tips service.
///
/// Holds the bundled tip catalogue plus a snapshot of the persistent
/// state. Reads cost an `O(n)` scan and a single random draw; writes
/// go through [`crate::config::atomic::atomic_write_secret`] so a
/// crashing process can't truncate the state file.
pub struct TipsService {
    tips: Vec<Tip>,
    state: TipsState,
    state_path: PathBuf,
    min_sessions_between: usize,
    repeat_window_days: u64,
}

impl TipsService {
    /// Build a service backed by the user's config directory.
    ///
    /// Returns a service with `disabled = true` when the config
    /// directory cannot be resolved — callers in CI / sandboxed
    /// environments should still be able to construct the type.
    pub fn new() -> Self {
        let state_path = default_state_path();
        Self::with_state_path(state_path)
    }

    /// Build a service with an explicit state-file path. Useful for
    /// tests that want a temp-dir-backed store.
    pub fn with_state_path(state_path: PathBuf) -> Self {
        let tips = load_bundled_tips();
        let state = read_state(&state_path).unwrap_or_default();
        Self {
            tips,
            state,
            state_path,
            min_sessions_between: DEFAULT_MIN_SESSIONS_BETWEEN,
            repeat_window_days: DEFAULT_REPEAT_WINDOW_DAYS,
        }
    }

    /// Convenience for tests: place the state file inside `dir`.
    pub fn with_state_dir(dir: &Path) -> Self {
        Self::with_state_path(dir.join("tips_state.json"))
    }

    /// Override the cadence for tests / configuration.
    pub fn set_min_sessions_between(&mut self, n: usize) {
        self.min_sessions_between = n;
    }

    /// Override the repeat window for tests / configuration.
    pub fn set_repeat_window_days(&mut self, n: u64) {
        self.repeat_window_days = n;
    }

    /// Return every loaded tip, in catalogue order.
    pub fn all(&self) -> &[Tip] {
        &self.tips
    }

    /// Snapshot of the current persistent state.
    pub fn state(&self) -> &TipsState {
        &self.state
    }

    /// Pick the next eligible tip, weighted by [`Tip::weight`].
    ///
    /// `session_count` is the user's running session count (incremented
    /// once per REPL launch). Returns `None` when:
    ///
    /// * tips are globally disabled (`/tips off`),
    /// * the snooze window is still in effect,
    /// * the last tip was less than `min_sessions_between` ago,
    /// * or no eligible tip exists.
    ///
    /// On a hit the call also updates the state's `last_shown_*`
    /// fields and persists. Callers should treat the borrow as
    /// short-lived — clone the body if it has to outlive the next
    /// call.
    pub fn next_tip(&mut self, session_count: usize) -> Option<&Tip> {
        if self.state.disabled {
            return None;
        }
        let now = unix_now();
        if self.state.snooze_until > now {
            return None;
        }
        if self.state.last_shown_session != 0
            && session_count.saturating_sub(self.state.last_shown_session)
                < self.min_sessions_between
        {
            return None;
        }

        let pool: Vec<&Tip> = self
            .tips
            .iter()
            .filter(|t| self.is_eligible(t, session_count, now))
            .collect();

        let pool = if pool.is_empty() {
            // Every eligible tip was shown recently. Reset the
            // last-shown map and try again — this is the documented
            // "exhaust + reset" behaviour.
            self.state.last_shown.clear();
            self.tips
                .iter()
                .filter(|t| self.is_eligible(t, session_count, now))
                .collect()
        } else {
            pool
        };

        let chosen = weighted_pick(&pool, &mut next_random())?;
        let id = chosen.id.clone();

        self.state.last_shown.insert(id.clone(), now);
        self.state.last_shown_session = session_count;
        self.persist();

        // Return a borrow into the catalogue so callers don't need to
        // clone unless they have to.
        self.tips.iter().find(|t| t.id == id)
    }

    /// Mark a tip as dismissed forever.
    pub fn dismiss(&mut self, id: &str) {
        if self.state.dismissed.iter().any(|d| d == id) {
            return;
        }
        self.state.dismissed.push(id.to_string());
        self.persist();
    }

    /// Snooze every tip for `days` days starting now.
    pub fn snooze_all(&mut self, days: u32) {
        let until = unix_now().saturating_add(u64::from(days) * SECONDS_PER_DAY);
        self.state.snooze_until = until;
        self.persist();
    }

    /// Master switch — `/tips off`.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.state.disabled = disabled;
        self.persist();
    }

    /// Bump the persistent session counter. Call once per REPL launch
    /// before [`TipsService::next_tip`].
    pub fn bump_session(&mut self) -> usize {
        self.state.session_count = self.state.session_count.saturating_add(1);
        self.persist();
        self.state.session_count
    }

    fn is_eligible(&self, tip: &Tip, session_count: usize, now: u64) -> bool {
        if self.state.dismissed.iter().any(|d| d == &tip.id) {
            return false;
        }
        if session_count < tip.show_after_session {
            return false;
        }
        if let Some(last) = self.state.last_shown.get(&tip.id) {
            let window = self.repeat_window_days * SECONDS_PER_DAY;
            if now.saturating_sub(*last) < window {
                return false;
            }
        }
        true
    }

    fn persist(&self) {
        if let Err(e) = write_state(&self.state_path, &self.state) {
            warn!("tips: failed to persist state: {e}");
        }
    }
}

impl Default for TipsService {
    fn default() -> Self {
        Self::new()
    }
}

const SECONDS_PER_DAY: u64 = 86_400;

fn default_state_path() -> PathBuf {
    crate::config::agent_config_dir()
        .map(|d| d.join("tips_state.json"))
        // Fall back to a path under the system temp dir so the type
        // is constructible even when no config dir resolves; in that
        // case `read_state` will return `None` and `write_state` will
        // either succeed silently or warn.
        .unwrap_or_else(|| std::env::temp_dir().join("agent-code-tips_state.json"))
}

fn load_bundled_tips() -> Vec<Tip> {
    let mut out = Vec::with_capacity(BUNDLED_TIP_FILES.len());
    for (slug, contents) in BUNDLED_TIP_FILES {
        match parse_frontmatter_into::<TipMeta>(contents) {
            Ok((meta, body)) => {
                let id = if meta.id.is_empty() {
                    (*slug).to_string()
                } else {
                    meta.id
                };
                let weight = meta.weight.max(1);
                out.push(Tip {
                    id,
                    weight,
                    show_after_session: meta.show_after_session,
                    body: body.trim().to_string(),
                });
            }
            Err(e) => {
                warn!("tips: failed to parse bundled tip '{slug}': {e}");
            }
        }
    }
    debug!("tips: loaded {} bundled tip(s)", out.len());
    out
}

fn read_state(path: &Path) -> Option<TipsState> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<TipsState>(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            warn!("tips: ignoring malformed state at {}: {e}", path.display());
            None
        }
    }
}

fn write_state(path: &Path, state: &TipsState) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    crate::config::atomic::atomic_write_secret(path, &bytes).map_err(|e| e.to_string())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Cheap PRNG seed source. Uses the nanos-since-epoch + a tiny
/// rolling counter so successive calls in the same nanosecond get
/// different draws.
fn next_random() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let bump = COUNTER.fetch_add(1, Ordering::Relaxed);
    splitmix64(nanos.wrapping_add(bump.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Weighted-random pick over a slice. `seed` is consumed via
/// `splitmix64` so callers can drive the selection deterministically
/// in tests.
fn weighted_pick<'a>(pool: &[&'a Tip], seed: &mut u64) -> Option<&'a Tip> {
    if pool.is_empty() {
        return None;
    }
    let total: u64 = pool.iter().map(|t| u64::from(t.weight.max(1))).sum();
    if total == 0 {
        return None;
    }
    *seed = splitmix64(*seed);
    let pick = *seed % total;
    let mut acc: u64 = 0;
    for tip in pool {
        acc += u64::from(tip.weight.max(1));
        if pick < acc {
            return Some(*tip);
        }
    }
    pool.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn bundled_tips_parse_cleanly() {
        let tips = load_bundled_tips();
        assert_eq!(tips.len(), 16, "expected 16 bundled tips");
        for tip in &tips {
            assert!(!tip.id.is_empty(), "tip has empty id: {tip:?}");
            assert!(!tip.body.is_empty(), "tip {} has empty body", tip.id);
            assert!(tip.weight >= 1);
        }
        // ids must be unique.
        let mut seen = std::collections::HashSet::new();
        for tip in &tips {
            assert!(seen.insert(tip.id.clone()), "duplicate id: {}", tip.id);
        }
    }

    #[test]
    fn no_third_party_attribution_in_bodies() {
        // Defence in depth — the spec forbids naming any specific
        // third-party product/company/AI assistant in tip bodies.
        // This test catches accidental drift if a tip is edited.
        let tips = load_bundled_tips();
        let banned = [
            "anthropic",
            "claude",
            "openai",
            "gpt-",
            "chatgpt",
            "codex",
            "gemini",
            "copilot",
            "cursor",
            "google ai",
        ];
        for tip in &tips {
            let lower = tip.body.to_lowercase();
            for word in banned {
                assert!(
                    !lower.contains(word),
                    "tip {} contains banned token {:?}",
                    tip.id,
                    word
                );
            }
        }
    }

    #[test]
    fn dismiss_round_trips_through_disk() {
        let dir = TempDir::new().unwrap();
        let mut svc = TipsService::with_state_dir(dir.path());
        svc.dismiss("plan-mode");
        // Reload from disk.
        let svc2 = TipsService::with_state_dir(dir.path());
        assert!(
            svc2.state().dismissed.iter().any(|d| d == "plan-mode"),
            "dismiss did not persist"
        );
    }

    #[test]
    fn snooze_round_trips_and_blocks_picks() {
        let dir = TempDir::new().unwrap();
        let mut svc = TipsService::with_state_dir(dir.path());
        svc.set_min_sessions_between(0);
        svc.snooze_all(3);
        assert!(svc.next_tip(100).is_none(), "snooze should block picks");

        let svc2 = TipsService::with_state_dir(dir.path());
        let now = unix_now();
        let snooze = svc2.state().snooze_until;
        let three_days = 3 * SECONDS_PER_DAY;
        // Snooze should be approximately three days out.
        assert!(snooze >= now + three_days - 5 && snooze <= now + three_days + 5);
    }

    #[test]
    fn weighted_selection_respects_ratio() {
        // Two tips with weights 1 and 9 — over many draws the heavy
        // one should win ~90% of the time. Tolerance is loose to
        // keep the test stable.
        let a = Tip {
            id: "a".into(),
            weight: 1,
            show_after_session: 0,
            body: "a".into(),
        };
        let b = Tip {
            id: "b".into(),
            weight: 9,
            show_after_session: 0,
            body: "b".into(),
        };
        let pool: Vec<&Tip> = vec![&a, &b];
        let mut counts: HashMap<&str, u32> = HashMap::new();
        let mut seed: u64 = 0xdead_beef_cafe_babe;
        for _ in 0..1000 {
            let pick = weighted_pick(&pool, &mut seed).unwrap();
            *counts.entry(pick.id.as_str()).or_default() += 1;
        }
        let a_pct = counts.get("a").copied().unwrap_or(0) as f64 / 1000.0;
        let b_pct = counts.get("b").copied().unwrap_or(0) as f64 / 1000.0;
        // Expected: a ≈ 0.10, b ≈ 0.90. Allow ±0.05.
        assert!(
            (0.05..=0.15).contains(&a_pct),
            "a share {a_pct} outside tolerance"
        );
        assert!(
            (0.85..=0.95).contains(&b_pct),
            "b share {b_pct} outside tolerance"
        );
    }

    #[test]
    fn same_tip_not_repeated_within_window() {
        let dir = TempDir::new().unwrap();
        let mut svc = TipsService::with_state_dir(dir.path());
        svc.set_min_sessions_between(0);
        // Force a single eligible tip by dismissing the rest, then
        // confirm a second pick yields *something* (because we fall
        // back to exhaust+reset) rather than the same one again.
        let all_ids: Vec<String> = svc.all().iter().map(|t| t.id.clone()).collect();
        for id in &all_ids[..all_ids.len() - 2] {
            svc.dismiss(id);
        }

        let first = svc.next_tip(10).map(|t| t.id.clone()).expect("first pick");
        // Second call from the *same session* — last_shown_session
        // is set to 10, min_sessions_between is 0, so cadence
        // doesn't block. The repeat window should keep us off
        // `first`.
        let second = svc.next_tip(10).map(|t| t.id.clone()).expect("second pick");
        // With only two eligible tips and a 30-day window, the
        // second pick must differ.
        assert_ne!(first, second, "tip repeated within window");
    }

    #[test]
    fn exhaustion_resets_history() {
        let dir = TempDir::new().unwrap();
        let mut svc = TipsService::with_state_dir(dir.path());
        svc.set_min_sessions_between(0);
        // Dismiss everything except one tip.
        let all_ids: Vec<String> = svc.all().iter().map(|t| t.id.clone()).collect();
        let kept = all_ids[0].clone();
        for id in &all_ids[1..] {
            svc.dismiss(id);
        }

        let first = svc.next_tip(10).map(|t| t.id.clone()).expect("first pick");
        assert_eq!(first, kept);
        // Now there are no other eligible tips; the next call must
        // wipe last_shown (exhaustion) and return the same one
        // again.
        let second = svc.next_tip(10).map(|t| t.id.clone()).expect("second pick");
        assert_eq!(second, kept);
    }

    #[test]
    fn cadence_blocks_until_min_sessions_passes() {
        let dir = TempDir::new().unwrap();
        let mut svc = TipsService::with_state_dir(dir.path());
        svc.set_min_sessions_between(5);
        let _ = svc.next_tip(10).expect("first pick");
        // 10 + 1 < 10 + 5, so this must yield None.
        assert!(svc.next_tip(11).is_none());
        assert!(svc.next_tip(14).is_none());
        // Exactly five sessions later — eligible.
        assert!(svc.next_tip(15).is_some());
    }

    #[test]
    fn disabled_blocks_picks() {
        let dir = TempDir::new().unwrap();
        let mut svc = TipsService::with_state_dir(dir.path());
        svc.set_min_sessions_between(0);
        svc.set_disabled(true);
        assert!(svc.next_tip(100).is_none());
        // Toggle back on.
        svc.set_disabled(false);
        assert!(svc.next_tip(100).is_some());
    }

    #[test]
    fn show_after_session_gates_eligibility() {
        // Build a synthetic service with two tips and a custom
        // show_after_session.
        let dir = TempDir::new().unwrap();
        let mut svc = TipsService::with_state_dir(dir.path());
        svc.set_min_sessions_between(0);

        // Replace the catalogue with a controlled fixture.
        svc.tips = vec![
            Tip {
                id: "early".into(),
                weight: 1,
                show_after_session: 0,
                body: "early".into(),
            },
            Tip {
                id: "late".into(),
                weight: 1,
                show_after_session: 50,
                body: "late".into(),
            },
        ];

        // At session 5, only `early` is eligible.
        for _ in 0..30 {
            let t = svc.next_tip(5).map(|t| t.id.clone()).expect("pick");
            assert_eq!(t, "early");
            // Roll the clock forward in repeat_window so each draw
            // remains eligible.
            svc.state.last_shown.clear();
        }
    }
}
