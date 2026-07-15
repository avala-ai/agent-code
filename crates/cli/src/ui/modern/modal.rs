//! HITL modal FIFO for the modern TUI (plan §M6).
//!
//! Permission asks, plan-approval, and ask-user overlays queue in
//! `App.modals` and are answered front-first. This module owns the modal
//! types and the `App` reducers that resolve them, keeping that surface out
//! of the `app.rs` view-model core.

use agent_code_lib::tools::PermissionResponse;

use super::app::{App, Phase, TranscriptItem, WaitingOn};
use super::mode::SessionMode;
use super::sink::UiQuestion;

/// A permission ask the engine is blocked on, awaiting the user's answer.
#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub name: String,
    pub description: String,
    /// Who triggered the ask (e.g. a subagent id), rendered as a distinct
    /// line rather than folded into `description`.
    pub origin: Option<String>,
    pub input_preview: Option<String>,
    pub respond: std::sync::mpsc::Sender<PermissionResponse>,
}

/// A plan proposed via ExitPlanMode, shown for review. Fire-and-forget —
/// the agent has already exited plan mode; approving just closes the modal
/// (and optionally switches out of Plan mode).
#[derive(Debug, Clone)]
pub struct PlanReview {
    pub plan_md: String,
    pub path: Option<String>,
}

/// An in-progress multi-question ask. Answers accumulate one label per
/// question; when the last is answered, `respond` receives the full vec.
#[derive(Debug, Clone)]
pub struct QuestionState {
    pub questions: Vec<UiQuestion>,
    /// Index of the question currently shown.
    pub current: usize,
    /// Highlighted option in the current question.
    pub cursor: usize,
    /// Chosen labels so far (one per answered question).
    pub answers: Vec<String>,
    pub respond: std::sync::mpsc::Sender<Vec<String>>,
}

/// A modal awaiting user input. Displayed FIFO — the front is shown; the
/// rest wait behind a "⚠ N pending" badge.
#[derive(Debug, Clone)]
pub enum Modal {
    Permission(PendingPermission),
    Plan(PlanReview),
    Question(QuestionState),
}

impl App {
    /// The modal currently displayed (front of the FIFO queue).
    pub fn front_modal(&self) -> Option<&Modal> {
        self.modals.front()
    }

    /// The permission ask currently displayed, if the front modal is one.
    pub fn front_permission(&self) -> Option<&PendingPermission> {
        match self.modals.front() {
            Some(Modal::Permission(p)) => Some(p),
            _ => None,
        }
    }

    /// Number of modals still waiting behind the front (for the badge).
    pub fn pending_modal_count(&self) -> usize {
        self.modals.len().saturating_sub(1)
    }

    /// After a modal is answered, close it and return to the turn if the
    /// queue is now empty.
    fn advance_modal_phase(&mut self) {
        if self.modals.is_empty() && self.phase == Phase::Permission {
            if self.turn_live {
                self.phase = Phase::Streaming;
                self.waiting_on = WaitingOn::Model;
            } else {
                // The modal outlived its turn (plan approval arrives right
                // before TurnComplete). Returning to Streaming here would
                // show a spinner forever with nothing running.
                self.phase = Phase::Idle;
                self.waiting_on = WaitingOn::Model;
                // A prompt queued behind the modal can start now.
                self.dispatch_queue_head();
            }
        }
        self.dirty = true;
    }

    /// Answer the front permission ask and advance the modal queue. No-op if
    /// the front modal is not a permission ask (so a mixed queue is safe).
    pub fn resolve_permission(&mut self, resp: PermissionResponse) {
        if !matches!(self.modals.front(), Some(Modal::Permission(_))) {
            return;
        }
        let Some(Modal::Permission(p)) = self.modals.pop_front() else {
            return;
        };
        let note = match resp {
            PermissionResponse::AllowOnce => format!("allowed {} once", p.name),
            PermissionResponse::AllowSession => format!("allowed {} for this session", p.name),
            PermissionResponse::Deny => format!("denied {}", p.name),
        };
        let _ = p.respond.send(resp);
        self.transcript.push(TranscriptItem::System(note));
        self.advance_modal_phase();
    }

    /// Resolve a plan-review modal: approve leaves the plan behind (and
    /// switches Plan→AcceptEdits so the follow-up can execute); keep-planning
    /// re-enters Plan; dismiss just closes. Returns true if a plan modal was
    /// at the front.
    pub fn resolve_plan(&mut self, approve: bool, keep_planning: bool) -> bool {
        if !matches!(self.modals.front(), Some(Modal::Plan(_))) {
            return false;
        }
        let Some(Modal::Plan(p)) = self.modals.pop_front() else {
            return false;
        };
        if approve {
            self.transcript
                .push(TranscriptItem::System("plan approved".into()));
            if self.mode == SessionMode::Plan {
                self.mode = SessionMode::AcceptEdits;
            }
        } else if keep_planning {
            self.transcript
                .push(TranscriptItem::System("staying in plan mode".into()));
            self.mode = SessionMode::Plan;
        } else {
            self.transcript
                .push(TranscriptItem::System("plan dismissed".into()));
        }
        let _ = &p.plan_md; // plan text already in the transcript context
        self.advance_modal_phase();
        true
    }

    /// Move the question cursor within the current question.
    pub fn question_move(&mut self, delta: i32) {
        if let Some(Modal::Question(q)) = self.modals.front_mut() {
            let n = q.questions[q.current].options.len().max(1);
            let cur = q.cursor as i32;
            q.cursor = (cur + delta).rem_euclid(n as i32) as usize;
            self.dirty = true;
        }
    }

    /// Select the highlighted (or numbered) option for the current question,
    /// advancing to the next question or sending all answers when done.
    pub fn question_select(&mut self, index: Option<usize>) {
        let done_answers = {
            let Some(Modal::Question(q)) = self.modals.front_mut() else {
                return;
            };
            let opts = &q.questions[q.current].options;
            if opts.is_empty() {
                return;
            }
            // Ignore an out-of-range digit instead of clamping: pressing
            // `9` on a 3-option question used to pick option 3 and
            // auto-advance with no undo.
            let pick = match index {
                Some(i) if i >= opts.len() => return,
                Some(i) => i,
                None => q.cursor.min(opts.len() - 1),
            };
            q.answers.push(opts[pick].clone());
            q.current += 1;
            q.cursor = 0;
            if q.current >= q.questions.len() {
                Some(q.answers.clone())
            } else {
                None
            }
        };
        if let Some(answers) = done_answers
            && let Some(Modal::Question(q)) = self.modals.pop_front()
        {
            let _ = q.respond.send(answers);
            self.transcript
                .push(TranscriptItem::System("answered".into()));
            self.advance_modal_phase();
        } else {
            self.dirty = true;
        }
    }

    /// Fail-close every queued modal (used on shutdown so turn tasks blocked
    /// in the prompter/asker never deadlock the join). Permission asks get
    /// Deny; question asks get their channel dropped (recv fails closed);
    /// plan modals are fire-and-forget.
    pub fn deny_all_modals(&mut self) {
        while let Some(m) = self.modals.pop_front() {
            match m {
                Modal::Permission(p) => {
                    let _ = p.respond.send(PermissionResponse::Deny);
                }
                Modal::Question(_) | Modal::Plan(_) => {}
            }
        }
    }
}
