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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeState {
    /// Nothing outstanding. Session-scoped work may run.
    ///
    /// The counter survives here so a generation is never reused: a read
    /// abandoned before a cancel must not collide with one issued after
    /// it, or the stale result would be accepted as the new request's.
    Idle { issued: u64 },
    /// A session load is in flight, tagged with the attempt that asked
    /// for it. A result arriving for a *superseded* request is then
    /// recognisable and dropped — a second `/resume`, or a cancel, must
    /// not be overwritten by the first load finishing late.
    ///
    /// The generation and not merely the id: selecting A, then B, then A
    /// again is three attempts, and the first A's result must not
    /// satisfy the third. It may carry an error, or a snapshot taken
    /// before another process wrote the session.
    Loading { id: String, generation: u64 },
}

impl Default for ResumeState {
    fn default() -> Self {
        ResumeState::Idle { issued: 0 }
    }
}

impl ResumeState {
    /// May work of this scope run right now?
    ///
    /// The single place the question is answered. Every deferred consumer
    /// calls this instead of re-deriving it.
    pub fn allows(&self, scope: WorkScope) -> bool {
        match scope {
            WorkScope::Global => true,
            WorkScope::Session => matches!(self, ResumeState::Idle { .. }),
        }
    }

    /// The session id being loaded, if any.
    pub fn loading_id(&self) -> Option<&str> {
        match self {
            ResumeState::Loading { id, .. } => Some(id.as_str()),
            ResumeState::Idle { .. } => None,
        }
    }

    /// The attempt currently outstanding, if any.
    pub fn generation(&self) -> Option<u64> {
        match self {
            ResumeState::Loading { generation, .. } => Some(*generation),
            ResumeState::Idle { .. } => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, ResumeState::Loading { .. })
    }

    /// True when `generation` is the attempt currently outstanding.
    ///
    /// Used to drop a result whose request was superseded or cancelled.
    /// Comparing attempts rather than ids is what makes returning to a
    /// session safe: the same id can be asked for twice, and only the
    /// later ask should be answered.
    pub fn is_awaiting(&self, generation: u64) -> bool {
        self.generation() == Some(generation)
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
    pub fn load_to_start<'a>(&'a self, in_flight: &[u64]) -> Option<(&'a str, u64)> {
        match self {
            ResumeState::Loading { id, generation } if !in_flight.contains(generation) => {
                Some((id.as_str(), *generation))
            }
            _ => None,
        }
    }

    /// Begin a load, replacing any outstanding one.
    ///
    /// Replacing is deliberate: a second `/resume` supersedes the first,
    /// and the earlier load's result is dropped when it arrives because
    /// [`Self::is_awaiting`] no longer matches it.
    pub fn begin(&mut self, id: impl Into<String>) -> u64 {
        let generation = self.issued().wrapping_add(1);
        *self = ResumeState::Loading {
            id: id.into(),
            generation,
        };
        generation
    }

    /// The highest attempt number handed out so far.
    fn issued(&self) -> u64 {
        match self {
            ResumeState::Idle { issued } => *issued,
            ResumeState::Loading { generation, .. } => *generation,
        }
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
        *self = ResumeState::Idle {
            issued: self.issued(),
        };
        was
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_allows_everything() {
        let s = ResumeState::default();
        assert!(s.allows(WorkScope::Session));
        assert!(s.allows(WorkScope::Global));
    }

    /// The property the nine hand-written guards were each trying to
    /// express, now expressed once.
    #[test]
    fn loading_holds_session_work_but_not_global_work() {
        let s = ResumeState::Loading {
            id: "abc".into(),
            generation: 1,
        };
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
        let mut s = ResumeState::default();
        let first = s.begin("first");
        assert!(s.is_awaiting(first));

        let second = s.begin("second");
        assert!(
            !s.is_awaiting(first),
            "the superseded load would still have been applied"
        );
        assert!(s.is_awaiting(second));
    }

    /// Cancelling means nothing is awaited — a result arriving later is
    /// dropped rather than restoring a session the user decided against.
    #[test]
    fn settling_stops_awaiting_anything() {
        let mut s = ResumeState::default();
        let attempt = s.begin("abc");
        assert_eq!(s.settle().as_deref(), Some("abc"));
        assert!(!s.is_loading());
        assert!(!s.is_awaiting(attempt));
        // Settling again is harmless and yields nothing.
        assert_eq!(s.settle(), None);
    }

    /// A `/resume` that supersedes another must start immediately. The
    /// read it replaced may be blocked on an unavailable mount, and
    /// waiting for it means the user's newer choice never loads.
    #[test]
    fn a_superseding_load_starts_while_the_old_one_is_still_running() {
        let mut s = ResumeState::default();
        let first = s.begin("first");
        assert_eq!(s.load_to_start(&[]), Some(("first", first)));

        // "first" is now off-thread; nothing further to start for it.
        let running = vec![first];
        assert_eq!(s.load_to_start(&running), None);

        // The user picks another session while "first" is still stuck.
        let second = s.begin("second");
        assert_eq!(
            s.load_to_start(&running),
            Some(("second", second)),
            "the superseding load waited for the read it replaced"
        );
    }

    /// Selecting A, then B, then A again is three attempts. The first
    /// A read is still in flight, so an id-only check would treat it as
    /// already satisfying the third and start nothing — then accept its
    /// result. That result may carry an error from the first attempt, or
    /// a snapshot taken before another process wrote the session.
    #[test]
    fn returning_to_a_session_is_a_new_attempt_not_the_one_still_running() {
        let mut s = ResumeState::default();
        let first_a = s.begin("A");
        let b = s.begin("B");
        let running = vec![first_a, b];

        let second_a = s.begin("A");
        assert_ne!(second_a, first_a, "the same id reused its old attempt");
        assert_eq!(
            s.load_to_start(&running),
            Some(("A", second_a)),
            "re-selecting A was answered by the read already in flight"
        );
        assert!(
            !s.is_awaiting(first_a),
            "the first A read would have satisfied the third selection"
        );
        assert!(s.is_awaiting(second_a));
    }

    /// An attempt number is never reused, even across a cancel: a read
    /// abandoned before one must not be mistaken for a request made
    /// after it.
    #[test]
    fn attempt_numbers_survive_a_settle() {
        let mut s = ResumeState::default();
        let abandoned = s.begin("A");
        s.settle();
        let fresh = s.begin("A");
        assert_ne!(
            fresh, abandoned,
            "a cancelled attempt's number came back around"
        );
        assert!(!s.is_awaiting(abandoned));
    }

    #[test]
    fn nothing_starts_when_no_resume_is_awaited() {
        let s = ResumeState::default();
        assert_eq!(s.load_to_start(&[]), None);
        assert_eq!(s.load_to_start(&[7]), None);
    }

    #[test]
    fn idle_awaits_nothing() {
        let s = ResumeState::default();
        assert!(!s.is_awaiting(1));
        assert!(!s.is_loading());
        assert_eq!(s.loading_id(), None);
    }
}
