//! The resume lifecycle, as a state machine rather than an `Option`.
//!
//! Resuming a session is not one event. A load is requested, runs on a
//! blocking thread, and lands some iterations later — and for that whole
//! window the conversation on screen is about to be replaced. Work
//! submitted in the meantime (`/clear`, `/model`, a slash command, a
//! `!cmd`, a prompt) belongs to the session being *left*, and running it
//! against the one that arrives is how it takes effect on the wrong
//! conversation.
//!
//! This was previously an `Option<String>` read at 30-odd sites, with
//! each consumer of deferred work hand-writing
//!
//! ```ignore
//! if app.pending_resume.is_none() && let Some(x) = app.pending_model.take()
//! ```
//!
//! Nine near-identical guards, one per deferred action. Adding a tenth
//! deferred action meant *remembering* the condition, and forgetting it
//! was invisible — the code compiled and worked whenever no resume was in
//! flight, which is almost always. Two separate review findings came from
//! exactly that: a stateful command that was never gated, and a failure
//! path that released deferred work instead of cancelling it.
//!
//! Here the gate is a property of the work, not a condition the caller
//! remembers. [`ResumeState::allows`] takes the work's [`WorkScope`], so
//! adding a deferred action forces its author to answer "does this belong
//! to the session?" — a question with a compiler-checked answer instead
//! of a convention.

/// Whether a piece of deferred work belongs to the session or outlives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkScope {
    /// Applies to the conversation currently in front: `/clear`, `/model`,
    /// a bridged slash command, a `!cmd`, a queued prompt. Must not run
    /// while a resume is loading — it would land on the session the
    /// restore is about to replace, and a `!cmd` takes real filesystem
    /// side effects there.
    Session,
    /// Independent of which session is loaded: `/theme` is UI state, a
    /// desktop notification has already been earned. Holding these back
    /// would make the UI unresponsive for no safety gain.
    Global,
}

/// Where a resume is in its lifecycle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ResumeState {
    /// Nothing outstanding. Session-scoped work may run.
    #[default]
    Idle,
    /// A session load is in flight. The id is kept so a result arriving
    /// for a *superseded* request can be recognised and dropped — a
    /// second `/resume`, or a cancel, must not be overwritten by the
    /// first load finishing late.
    Loading { id: String },
}

impl ResumeState {
    /// May work of this scope run right now?
    ///
    /// The single place the question is answered. Every deferred consumer
    /// calls this instead of re-deriving it.
    pub fn allows(&self, scope: WorkScope) -> bool {
        match scope {
            WorkScope::Global => true,
            WorkScope::Session => matches!(self, ResumeState::Idle),
        }
    }

    /// The session id being loaded, if any.
    pub fn loading_id(&self) -> Option<&str> {
        match self {
            ResumeState::Loading { id } => Some(id.as_str()),
            ResumeState::Idle => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, ResumeState::Loading { .. })
    }

    /// True when `id` is the load currently outstanding.
    ///
    /// Used to drop a result whose request was superseded or cancelled;
    /// comparing ids is what makes late arrivals safe.
    pub fn is_awaiting(&self, id: &str) -> bool {
        self.loading_id() == Some(id)
    }

    /// The load that should be started, given what is already running.
    ///
    /// `None` when nothing is awaited, or when the awaited load is the
    /// one already in flight. The comparison is by id and not by a bare
    /// "a read is running" flag, because those two are not the same
    /// question: a second `/resume` supersedes the first, and with a
    /// flag the superseding load could not start until the one it
    /// replaced returned. A session sitting on an unavailable mount then
    /// held the picker behind a result nobody wanted.
    ///
    /// The superseded read is left to finish and be discarded on
    /// arrival — [`Self::is_awaiting`] already refuses it.
    pub fn load_to_start<'a>(&'a self, in_flight: &[String]) -> Option<&'a str> {
        match self.loading_id() {
            Some(id) if !in_flight.iter().any(|f| f == id) => Some(id),
            _ => None,
        }
    }

    /// Begin a load, replacing any outstanding one.
    ///
    /// Replacing is deliberate: a second `/resume` supersedes the first,
    /// and the earlier load's result is dropped when it arrives because
    /// [`Self::is_awaiting`] no longer matches it.
    pub fn begin(&mut self, id: impl Into<String>) {
        *self = ResumeState::Loading { id: id.into() };
    }

    /// Return to idle, yielding the id that was outstanding.
    ///
    /// Deliberately named for the *transition*, not the field. Both
    /// completion and cancellation end here, but they differ in what they
    /// do with the deferred work — completion lets it run against the
    /// restored session, cancellation must discard it. Making that a
    /// separate, explicit decision at each call site is the point; the
    /// previous code assigned `None` and left the difference to whether
    /// someone remembered to call the cancel helper first.
    pub fn settle(&mut self) -> Option<String> {
        let was = self.loading_id().map(str::to_string);
        *self = ResumeState::Idle;
        was
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_allows_everything() {
        let s = ResumeState::Idle;
        assert!(s.allows(WorkScope::Session));
        assert!(s.allows(WorkScope::Global));
    }

    /// The property the nine hand-written guards were each trying to
    /// express, now expressed once.
    #[test]
    fn loading_holds_session_work_but_not_global_work() {
        let s = ResumeState::Loading { id: "abc".into() };
        assert!(
            !s.allows(WorkScope::Session),
            "session work ran while a resume was loading"
        );
        assert!(
            s.allows(WorkScope::Global),
            "global work was held back for no safety gain"
        );
    }

    /// A load that finishes after its request was superseded must be
    /// recognisable as stale, or the user lands in a session they moved
    /// on from.
    #[test]
    fn a_superseded_load_is_no_longer_awaited() {
        let mut s = ResumeState::Idle;
        s.begin("first");
        assert!(s.is_awaiting("first"));

        s.begin("second");
        assert!(
            !s.is_awaiting("first"),
            "the superseded load would still have been applied"
        );
        assert!(s.is_awaiting("second"));
    }

    /// Cancelling means nothing is awaited — a result arriving later is
    /// dropped rather than restoring a session the user decided against.
    #[test]
    fn settling_stops_awaiting_anything() {
        let mut s = ResumeState::Idle;
        s.begin("abc");
        assert_eq!(s.settle().as_deref(), Some("abc"));
        assert_eq!(s, ResumeState::Idle);
        assert!(!s.is_awaiting("abc"));
        // Settling again is harmless and yields nothing.
        assert_eq!(s.settle(), None);
    }

    /// A `/resume` that supersedes another must start immediately. The
    /// read it replaced may be blocked on an unavailable mount, and
    /// waiting for it means the user's newer choice never loads.
    #[test]
    fn a_superseding_load_starts_while_the_old_one_is_still_running() {
        let mut s = ResumeState::Idle;
        s.begin("first");
        assert_eq!(s.load_to_start(&[]), Some("first"));

        // "first" is now off-thread; nothing further to start for it.
        let running = vec!["first".to_string()];
        assert_eq!(s.load_to_start(&running), None);

        // The user picks another session while "first" is still stuck.
        s.begin("second");
        assert_eq!(
            s.load_to_start(&running),
            Some("second"),
            "the superseding load waited for the read it replaced"
        );
    }

    #[test]
    fn nothing_starts_when_no_resume_is_awaited() {
        let s = ResumeState::Idle;
        assert_eq!(s.load_to_start(&[]), None);
        assert_eq!(s.load_to_start(&["stale".to_string()]), None);
    }

    #[test]
    fn idle_awaits_nothing() {
        let s = ResumeState::Idle;
        assert!(!s.is_awaiting("abc"));
        assert!(!s.is_loading());
        assert_eq!(s.loading_id(), None);
    }
}
