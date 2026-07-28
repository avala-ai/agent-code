//! Work staged against the conversation currently in front.
//!
//! A `/model`, a `/clear`, a bridged slash line, a `!cmd` and a submitted
//! prompt are all *deferred*: they need the engine lock, or an idle turn
//! slot, so the composer stages them and the run loop picks them up an
//! iteration or more later. In between, a `/resume` can land and replace
//! the conversation they were written for.
//!
//! Everything here therefore belongs to the session, and there are three
//! distinct things a caller can want:
//!
//! - **stage** it — always allowed, it is just bookkeeping;
//! - **take** it to run — allowed only when no resume is in flight;
//! - **discard** it because the session is being left — always allowed,
//!   and the reason this is not simply "take without the check".
//!
//! The fields are private so those three cannot be confused. A take
//! requires a [`ResumeState`] to consult, which is what makes the gate
//! impossible to omit: there is no path to the value that does not pass
//! one in. The previous shape was six `pub` fields and a `.is_none()`
//! check written by hand at each consumer — nine near-identical guards,
//! where adding a tenth consumer meant *remembering* the condition.
//! Forgetting it was invisible, because the code behaves correctly
//! whenever no resume is in flight, which is almost always. Two separate
//! review findings were that same omission.
//!
//! A discard is spelled `discard_*` rather than `take_*` for the same
//! reason: cancelling and releasing both end at "the field is now empty",
//! and the bug that started this was a failure path that released
//! deferred work where it meant to cancel it. Different verbs make that
//! a choice at the call site instead of an accident.

use super::app::PendingModelAction;
use super::resume_state::{ResumeState, WorkScope};

/// A prompt the user submitted, with the words they actually typed.
///
/// The two travel together because they are only ever correct together:
/// `payload` has `@path` mentions and skill bodies already inlined for
/// the engine, so handing *it* back to the composer would paste a whole
/// file into the input and expand the mention a second time on
/// resubmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    /// The engine payload: mentions and skill bodies expanded.
    pub payload: String,
    /// The line the user typed, when it differs from the payload.
    pub display: Option<String>,
}

impl Submission {
    /// A prompt the composer sent unchanged, where the payload *is*
    /// what the user typed.
    pub fn verbatim(text: impl Into<String>) -> Self {
        Submission {
            payload: text.into(),
            display: None,
        }
    }

    /// The user's own words, falling back to the payload when the
    /// composer sent it verbatim.
    ///
    /// Every caller that shows a submission to a human wants this, and
    /// each of them used to write the fallback itself.
    pub fn user_text(self) -> String {
        self.display.unwrap_or(self.payload)
    }
}

/// Deferred, session-scoped work awaiting the run loop.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionWork {
    submit: Option<Submission>,
    model: Option<PendingModelAction>,
    clear: bool,
    slash: Option<String>,
    shell: Option<String>,
}

impl SessionWork {
    // ---- stage: always allowed, it is only bookkeeping ----

    /// Stage a submitted prompt. `display` is the line the user typed,
    /// when the payload differs from it.
    pub fn stage_submit(&mut self, payload: String, display: Option<String>) {
        self.submit = Some(Submission { payload, display });
    }

    pub fn stage_model(&mut self, action: PendingModelAction) {
        self.model = Some(action);
    }

    pub fn stage_clear(&mut self) {
        self.clear = true;
    }

    pub fn stage_slash(&mut self, line: String) {
        self.slash = Some(line);
    }

    pub fn stage_shell(&mut self, cmd: String) {
        self.shell = Some(cmd);
    }

    // ---- take: to run now, only when no resume holds the session ----

    /// Take the staged prompt to start a turn, unless a resume holds it.
    pub fn take_submit(&mut self, resume: &ResumeState) -> Option<Submission> {
        self.gated(resume, |w| w.submit.take())
    }

    /// Take a deferred `/model` action, unless a resume holds it.
    pub fn take_model(&mut self, resume: &ResumeState) -> Option<PendingModelAction> {
        self.gated(resume, |w| w.model.take())
    }

    /// Take a deferred bridged slash command, unless a resume holds it.
    pub fn take_slash(&mut self, resume: &ResumeState) -> Option<String> {
        self.gated(resume, |w| w.slash.take())
    }

    /// Take a deferred `!cmd`, unless a resume holds it.
    pub fn take_shell(&mut self, resume: &ResumeState) -> Option<String> {
        self.gated(resume, |w| w.shell.take())
    }

    /// Claim a deferred `/clear`, unless a resume holds it.
    ///
    /// The flag is cleared only when the claim succeeds, so a `/clear`
    /// held back for a resume stays pending rather than being dropped.
    pub fn claim_clear(&mut self, resume: &ResumeState) -> bool {
        if !self.clear || !resume.allows(WorkScope::Session) {
            return false;
        }
        self.clear = false;
        true
    }

    /// The one place the session gate is applied.
    fn gated<T>(
        &mut self,
        resume: &ResumeState,
        take: impl FnOnce(&mut Self) -> Option<T>,
    ) -> Option<T> {
        if !resume.allows(WorkScope::Session) {
            return None;
        }
        take(self)
    }

    // ---- discard: the session is being left, so nothing may run ----
    //
    // Deliberately ungated. These are called *because* a resume is in
    // flight, which is exactly when a take must refuse.

    /// Take back the staged prompt to return it to the composer.
    pub fn discard_submit(&mut self) -> Option<Submission> {
        self.submit.take()
    }

    /// Drop a deferred `/clear`, reporting whether one was staged.
    pub fn discard_clear(&mut self) -> bool {
        std::mem::take(&mut self.clear)
    }

    /// Drop a deferred `/model`, reporting whether one was staged.
    pub fn discard_model(&mut self) -> bool {
        self.model.take().is_some()
    }

    pub fn discard_slash(&mut self) -> Option<String> {
        self.slash.take()
    }

    pub fn discard_shell(&mut self) -> Option<String> {
        self.shell.take()
    }

    // ---- peek ----

    /// Whether a prompt is staged, regardless of whether it may run.
    ///
    /// Callers use this to decide whether the composer is busy, which is
    /// true while a resume holds the prompt as well.
    pub fn submit_staged(&self) -> bool {
        self.submit.is_some()
    }

    /// The staged prompt's engine payload, for assertions and status.
    pub fn submit_payload(&self) -> Option<&str> {
        self.submit.as_ref().map(|s| s.payload.as_str())
    }

    /// The typed line staged alongside the prompt, when it differs.
    pub fn submit_display(&self) -> Option<&str> {
        self.submit.as_ref().and_then(|s| s.display.as_deref())
    }

    pub fn clear_staged(&self) -> bool {
        self.clear
    }

    pub fn slash_staged(&self) -> Option<&str> {
        self.slash.as_deref()
    }

    pub fn shell_staged(&self) -> Option<&str> {
        self.shell.as_deref()
    }

    pub fn model_staged(&self) -> Option<&PendingModelAction> {
        self.model.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loading() -> ResumeState {
        let mut r = ResumeState::default();
        r.begin("abc");
        r
    }

    /// The property the hand-written guards were each expressing, now
    /// expressed once — and unreachable around.
    #[test]
    fn a_loading_resume_withholds_every_kind_of_session_work() {
        let idle = ResumeState::default();
        let busy = loading();

        let mut w = SessionWork::default();
        w.stage_submit("payload".into(), None);
        w.stage_model(PendingModelAction::Show);
        w.stage_clear();
        w.stage_slash("/cost".into());
        w.stage_shell("rm -rf build".into());

        assert!(w.take_submit(&busy).is_none(), "prompt escaped");
        assert!(w.take_model(&busy).is_none(), "model action escaped");
        assert!(w.take_slash(&busy).is_none(), "slash command escaped");
        assert!(w.take_shell(&busy).is_none(), "shell command escaped");
        assert!(!w.claim_clear(&busy), "/clear escaped");

        // Withheld, not dropped: the same work runs once the resume
        // settles, which is the whole point of holding it.
        assert!(w.take_submit(&idle).is_some());
        assert!(w.take_model(&idle).is_some());
        assert!(w.take_slash(&idle).is_some());
        assert!(w.take_shell(&idle).is_some());
        assert!(w.claim_clear(&idle));
    }

    /// A discard happens *because* a resume is in flight, so it must not
    /// consult the gate that a take does.
    #[test]
    fn discarding_works_while_a_resume_is_loading() {
        let busy = loading();
        let mut w = SessionWork::default();
        w.stage_submit("payload".into(), Some("typed".into()));
        w.stage_clear();
        w.stage_slash("/cost".into());
        w.stage_shell("make clean".into());
        w.stage_model(PendingModelAction::Show);

        assert!(!w.claim_clear(&busy), "precondition: takes are held");

        assert_eq!(
            w.discard_submit().map(Submission::user_text).as_deref(),
            Some("typed")
        );
        assert!(w.discard_clear());
        assert!(w.discard_model());
        assert_eq!(w.discard_slash().as_deref(), Some("/cost"));
        assert_eq!(w.discard_shell().as_deref(), Some("make clean"));
        assert_eq!(w, SessionWork::default(), "discard left work behind");
    }

    /// Returning the engine payload to the composer would paste an
    /// inlined file back into the input and expand the mention twice.
    #[test]
    fn the_user_gets_their_own_words_back_not_the_expanded_payload() {
        let mut w = SessionWork::default();
        w.stage_submit(
            "look at <file>...500 lines...</file>".into(),
            Some("look at @src/main.rs".into()),
        );
        let got = w.discard_submit().unwrap().user_text();
        assert_eq!(got, "look at @src/main.rs");
    }

    /// With nothing inlined there is no separate display line, and the
    /// payload *is* what the user typed.
    #[test]
    fn a_verbatim_prompt_falls_back_to_the_payload() {
        let mut w = SessionWork::default();
        w.stage_submit("keep going".into(), None);
        assert_eq!(w.discard_submit().unwrap().user_text(), "keep going");
    }

    /// A held `/clear` must stay pending: dropping it would silently
    /// lose the user's request rather than deferring it.
    #[test]
    fn a_refused_claim_leaves_the_clear_staged() {
        let mut w = SessionWork::default();
        w.stage_clear();
        assert!(!w.claim_clear(&loading()));
        assert!(w.clear_staged(), "the held /clear was dropped");
        assert!(w.claim_clear(&ResumeState::default()));
        assert!(!w.clear_staged());
    }

    /// The composer is busy while a resume holds the prompt, so a peek
    /// reports what is staged rather than what may run.
    #[test]
    fn a_peek_sees_work_that_a_take_would_withhold() {
        let mut w = SessionWork::default();
        w.stage_submit("hello".into(), None);
        assert!(w.take_submit(&loading()).is_none());
        assert!(w.submit_staged(), "the staged prompt went missing");
        assert_eq!(w.submit_payload(), Some("hello"));
    }
}
