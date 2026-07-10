//! Application state for the modern TUI.
//!
//! Pure data + reducers. Drawing lives in [`super::render`]; I/O in
//! [`super::run`]. This split keeps visual tests free of a live terminal.

use agent_code_lib::tools::PermissionResponse;

use super::layout::LayoutCache;
use super::mode::SessionMode;
use super::scroll::ScrollState;
use super::sink::EngineEvent;
use super::stream_buffer::StreamBuffer;
use super::tasks::TaskEntry;
use super::terminal_caps::TerminalCaps;

/// A permission ask the engine is blocked on, awaiting the user's answer.
#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub name: String,
    pub description: String,
    pub input_preview: Option<String>,
    pub respond: std::sync::mpsc::Sender<PermissionResponse>,
}

/// A plan proposed via ExitPlanMode, shown for review (plan §M6). Fire-and-
/// forget — the agent has already exited plan mode; approving just closes
/// the modal (and optionally switches out of Plan mode).
#[derive(Debug, Clone)]
pub struct PlanReview {
    pub plan_md: String,
    pub path: Option<String>,
}

/// An in-progress multi-question ask. Answers accumulate one label per
/// question; when the last is answered, `respond` receives the full vec.
#[derive(Debug, Clone)]
pub struct QuestionState {
    pub questions: Vec<super::sink::UiQuestion>,
    /// Index of the question currently shown.
    pub current: usize,
    /// Highlighted option in the current question.
    pub cursor: usize,
    /// Chosen labels so far (one per answered question).
    pub answers: Vec<String>,
    pub respond: std::sync::mpsc::Sender<Vec<String>>,
}

/// A modal awaiting user input. Displayed FIFO — the front is shown; the
/// rest wait behind a "⚠ N pending" badge (plan §M6).
#[derive(Debug, Clone)]
pub enum Modal {
    Permission(PendingPermission),
    Plan(PlanReview),
    Question(QuestionState),
}

/// One row in the scrollable transcript.
#[derive(Debug, Clone, Hash)]
pub enum TranscriptItem {
    User(String),
    Assistant(String),
    Thinking(String),
    Tool {
        name: String,
        detail: String,
        result: Option<String>,
        is_error: bool,
    },
    System(String),
    Error(String),
    Warning(String),
}

/// What a running turn is currently blocked on, for the spinner detail
/// (plan §M4 waiting-on).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WaitingOn {
    /// Waiting on the model to produce tokens.
    #[default]
    Model,
    /// A tool is executing.
    Tool(String),
    /// Blocked on the user (permission / question).
    UserInput,
}

impl WaitingOn {
    /// Spinner label (the glyph is prepended by the renderer).
    pub fn label(&self) -> String {
        match self {
            WaitingOn::Model => "thinking…".to_string(),
            WaitingOn::Tool(name) => format!("running {name}"),
            WaitingOn::UserInput => "waiting on your input".to_string(),
        }
    }
}

/// Visual skin (plan §M10). A render config, not a fork — the same block
/// model renders in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Skin {
    /// Bordered header + status + framed prompt.
    #[default]
    Fullscreen,
    /// No header, compact borderless prompt — maximizes transcript space.
    Minimal,
}

/// High-level UI phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    /// A turn is streaming; input is buffered but not submitted.
    Streaming,
    /// Modal: waiting for permission (stretch — shell only for now).
    Permission,
    /// Modal: plan review (stretch — shell only for now).
    PlanReview,
}

/// Entire TUI state.
#[derive(Debug, Clone)]
pub struct App {
    pub model: String,
    pub cwd: String,
    pub session_id: String,
    pub version: String,

    pub mode: SessionMode,
    pub phase: Phase,
    /// What the running turn is blocked on (drives the status-bar spinner).
    pub waiting_on: WaitingOn,
    /// FIFO of modals awaiting the user (permission asks, incl. from
    /// background subagents). The front is displayed (plan §M6).
    pub modals: std::collections::VecDeque<Modal>,

    pub transcript: Vec<TranscriptItem>,
    /// Follow/Free scroll anchor for the transcript (plan §M2).
    pub scroll: ScrollState,
    /// Virtualized per-block rendered-line cache. Populated during draw
    /// (the one permitted side effect in the otherwise-pure view model).
    pub layout: LayoutCache,
    /// Transcript viewport height in rows, recorded on the last draw so
    /// scroll-key handlers (which run before the next draw) have metrics.
    pub viewport_h: usize,

    pub input: String,
    pub cursor: usize,

    pub turn_count: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    /// Engine-provided context-window meter (used, max) — plan §3.4.4.
    pub ctx_meter: Option<(u64, u64)>,
    pub status_message: String,

    pub should_quit: bool,
    /// Prompt waiting to be started as a turn by the runtime.
    pub pending_submit: Option<String>,
    /// Prompts typed mid-turn, sent FIFO when the turn ends (plan §M5).
    pub queue: std::collections::VecDeque<String>,
    /// When true, runtime should cancel the active turn.
    pub cancel_requested: bool,
    /// Ctrl+C on an empty idle prompt arms quit; a second press within
    /// [`super::run::QUIT_ARM_WINDOW`] quits. The run loop disarms on expiry.
    pub quit_armed: bool,

    /// Tracked subagents for the tasks pane (plan §M8).
    pub tasks: Vec<TaskEntry>,
    /// Whether the tasks pane is shown (Ctrl+T); auto-hidden when no tasks.
    pub show_tasks: bool,
    /// Detected terminal capabilities, set once at loop start (plan §M7).
    pub caps: TerminalCaps,
    /// Visual skin (plan §M10); toggled with /minimal · /fullscreen.
    pub skin: Skin,
    /// Frames actually drawn — instrumentation for /stats and the idle
    /// zero-frame invariant.
    pub frame_count: u64,

    /// Spinner frame index while streaming.
    pub tick: u64,

    /// Coalesces streaming text deltas so heavy streaming repaints at
    /// ≤10 fps instead of once per delta (plan §2.2).
    pub stream_buf: StreamBuffer,
    /// Set whenever visible state changed and the frame must be redrawn.
    /// Idle (no events, no pending deltas) leaves this false → zero frames.
    pub dirty: bool,
}

impl App {
    pub fn new(
        model: impl Into<String>,
        cwd: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            cwd: cwd.into(),
            session_id: session_id.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            mode: SessionMode::Normal,
            phase: Phase::Idle,
            waiting_on: WaitingOn::Model,
            modals: std::collections::VecDeque::new(),
            transcript: vec![TranscriptItem::System(
                "Modern TUI · Shift+Tab mode · Ctrl+C cancel turn / quit · Esc clear prompt · Enter send".into(),
            )],
            scroll: ScrollState::Follow,
            layout: LayoutCache::default(),
            viewport_h: 0,
            input: String::new(),
            cursor: 0,
            turn_count: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            ctx_meter: None,
            status_message: String::new(),
            should_quit: false,
            pending_submit: None,
            queue: std::collections::VecDeque::new(),
            cancel_requested: false,
            quit_armed: false,
            tasks: Vec::new(),
            show_tasks: true,
            caps: TerminalCaps::default(),
            skin: Skin::Fullscreen,
            frame_count: 0,
            tick: 0,
            stream_buf: StreamBuffer::new(),
            // Draw the first frame.
            dirty: true,
        }
    }

    pub fn apply_engine(&mut self, ev: EngineEvent) {
        // Any state change must repaint.
        self.dirty = true;

        // Text/thinking deltas are coalesced; everything else is a "barrier"
        // event that must flush buffered text first so ordering is preserved
        // (plan §2.2 rule 3: deltas never reorder around tool/turn events).
        match ev {
            EngineEvent::Text(t) => {
                self.stream_buf.push_assistant(&t);
                return;
            }
            EngineEvent::Thinking(t) => {
                self.stream_buf.push_thinking(&t);
                return;
            }
            _ => self.flush_stream(),
        }

        match ev {
            // Deltas handled above.
            EngineEvent::Text(_) | EngineEvent::Thinking(_) => unreachable!(),
            EngineEvent::ToolStart { name, detail } => {
                self.waiting_on = WaitingOn::Tool(name.clone());
                self.transcript.push(TranscriptItem::Tool {
                    name,
                    detail,
                    result: None,
                    is_error: false,
                });
            }
            EngineEvent::ToolResult {
                content, is_error, ..
            } => {
                // Attach to the OLDEST pending tool: the executor runs calls
                // sequentially in start order, so results arrive in the same
                // order (rev() paired results with the newest start and
                // swapped outputs whenever a turn had 2+ tool calls).
                if let Some(TranscriptItem::Tool {
                    result,
                    is_error: err,
                    ..
                }) = self
                    .transcript
                    .iter_mut()
                    .find(|i| matches!(i, TranscriptItem::Tool { result: None, .. }))
                {
                    *result = Some(content.lines().next().unwrap_or("").to_string());
                    *err = is_error;
                }
                // Back to waiting on the model once a tool returns.
                self.waiting_on = WaitingOn::Model;
            }
            EngineEvent::TurnStart(n) => {
                self.phase = Phase::Streaming;
                self.waiting_on = WaitingOn::Model;
                self.turn_count = n;
                self.status_message = format!("turn {n}");
            }
            EngineEvent::TurnComplete(n) => {
                self.phase = Phase::Idle;
                self.turn_count = n;
                self.status_message = format!("turn {n} done");
            }
            EngineEvent::Error(e) => {
                // Do NOT flip to Idle here: the engine reports stream errors
                // mid-turn and keeps going (recovery, tool execution). The
                // run loop marks Idle when the turn handle actually finishes.
                self.transcript.push(TranscriptItem::Error(e));
            }
            EngineEvent::Warning(w) => {
                self.transcript.push(TranscriptItem::Warning(w));
            }
            EngineEvent::Usage { input, output, .. } => {
                self.tokens_in = self.tokens_in.saturating_add(input);
                self.tokens_out = self.tokens_out.saturating_add(output);
            }
            EngineEvent::Compact { freed } => {
                self.transcript
                    .push(TranscriptItem::System(format!("compacted ~{freed} tokens")));
            }
            EngineEvent::ContextUsage { used, max } => {
                self.ctx_meter = Some((used, max));
            }
            EngineEvent::SubagentUpdate {
                agent_id,
                state,
                headline,
            } => {
                super::tasks::upsert(&mut self.tasks, &agent_id, &state, &headline);
            }
            EngineEvent::PermissionAsk {
                name,
                description,
                input_preview,
                respond,
            } => {
                // FIFO: concurrent asks (e.g. lead + background subagent)
                // queue behind the current one instead of being dropped.
                self.modals.push_back(Modal::Permission(PendingPermission {
                    name,
                    description,
                    input_preview,
                    respond,
                }));
                self.phase = Phase::Permission;
                self.waiting_on = WaitingOn::UserInput;
            }
            EngineEvent::PlanProposed { plan_md, path } => {
                self.modals
                    .push_back(Modal::Plan(PlanReview { plan_md, path }));
                self.phase = Phase::Permission;
                self.waiting_on = WaitingOn::UserInput;
            }
            EngineEvent::QuestionAsk { questions, respond } => {
                if questions.is_empty() {
                    let _ = respond.send(Vec::new());
                } else {
                    self.modals.push_back(Modal::Question(QuestionState {
                        questions,
                        current: 0,
                        cursor: 0,
                        answers: Vec::new(),
                        respond,
                    }));
                    self.phase = Phase::Permission;
                    self.waiting_on = WaitingOn::UserInput;
                }
            }
        }
    }

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

    /// After a modal is answered, close it and return to the turn if the
    /// queue is now empty.
    fn advance_modal_phase(&mut self) {
        if self.modals.is_empty() && self.phase == Phase::Permission {
            self.phase = Phase::Streaming;
            self.waiting_on = WaitingOn::Model;
        }
        self.dirty = true;
    }

    /// Resolve a plan-review modal (plan §M6): approve leaves plan behind (and
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
        // Keep the plan in the transcript for reference.
        let _ = &p.plan_md;
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
            let pick = index.unwrap_or(q.cursor).min(opts.len() - 1);
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

    /// Number of modals still waiting behind the front (for the badge).
    pub fn pending_modal_count(&self) -> usize {
        self.modals.len().saturating_sub(1)
    }

    /// Drain any buffered streaming text into the transcript. Called before
    /// applying a non-delta event and on the coalescer's flush deadline.
    pub fn flush_stream(&mut self) {
        if !self.stream_buf.has_pending() {
            return;
        }
        let out = self.stream_buf.flush();
        if !out.thinking.is_empty() {
            if let Some(TranscriptItem::Thinking(buf)) = self.transcript.last_mut() {
                buf.push_str(&out.thinking);
            } else {
                self.transcript.push(TranscriptItem::Thinking(out.thinking));
            }
        }
        if !out.assistant.is_empty() {
            self.push_or_append_assistant(&out.assistant);
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

    fn push_or_append_assistant(&mut self, t: &str) {
        if let Some(TranscriptItem::Assistant(buf)) = self.transcript.last_mut() {
            buf.push_str(t);
        } else {
            self.transcript
                .push(TranscriptItem::Assistant(t.to_string()));
        }
    }

    pub fn insert_char(&mut self, c: char) {
        if self.phase != Phase::Idle && self.phase != Phase::Streaming {
            return;
        }
        // Allow typing while streaming (buffer for next turn).
        let idx = self.cursor.min(self.input.len());
        self.input.insert(idx, c);
        self.cursor = idx + c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 || self.input.is_empty() {
            return;
        }
        let prev = self
            .input
            .char_indices()
            .take_while(|(i, _)| *i < self.cursor)
            .last();
        if let Some((i, _)) = prev {
            self.input.remove(i);
            self.cursor = i;
        }
    }

    pub fn move_left(&mut self) {
        if let Some((i, _)) = self
            .input
            .char_indices()
            .take_while(|(i, _)| *i < self.cursor)
            .last()
        {
            self.cursor = i;
        } else {
            self.cursor = 0;
        }
    }

    pub fn move_right(&mut self) {
        if let Some((i, c)) = self.input[self.cursor..].char_indices().next() {
            self.cursor += i + c.len_utf8();
        }
    }

    pub fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if text == "/exit" || text == "/quit" {
            self.should_quit = true;
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/clear" {
            self.transcript.clear();
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/help" {
            self.transcript.push(TranscriptItem::System(
                "Keys: Enter send · Shift+Tab mode · Ctrl+C cancel turn (then quit) · \
                 Esc clear prompt · Ctrl+T tasks · permission prompt: y once / a session / n deny · \
                 /clear /terminal-setup /exit"
                    .into(),
            ));
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/terminal-setup" {
            self.emit_terminal_setup();
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/minimal" || text == "/fullscreen" {
            self.skin = if text == "/minimal" {
                Skin::Minimal
            } else {
                Skin::Fullscreen
            };
            self.status_message = format!("skin → {text}");
            self.input.clear();
            self.cursor = 0;
            self.dirty = true;
            return;
        }
        if text == "/stats" {
            let (blocks, cached) = self.layout.stats();
            self.transcript.push(TranscriptItem::System(format!(
                "stats: {} transcript items · {blocks} layout blocks · {cached} cached lines · \
                 {} frames drawn · queue {} · tasks {}",
                self.transcript.len(),
                self.frame_count,
                self.queue.len(),
                self.tasks.len(),
            )));
            self.input.clear();
            self.cursor = 0;
            return;
        }
        // Mid-turn: queue the prompt instead of dropping it (plan §M5).
        if self.phase == Phase::Streaming {
            self.queue.push_back(text);
            self.input.clear();
            self.cursor = 0;
            self.status_message = format!("{} queued", self.queue.len());
            return;
        }
        self.transcript.push(TranscriptItem::User(text.clone()));
        self.input.clear();
        self.cursor = 0;
        self.pending_submit = Some(text);
        self.phase = Phase::Streaming;
        // Jump back to the live tail when the user sends.
        self.scroll = ScrollState::Follow;
    }

    /// Dispatch the head of the queue as the next turn, if idle and non-empty.
    /// Called by the run loop when a turn finishes successfully (plan §M5:
    /// `queue.auto_send`, default on).
    pub fn dispatch_queue_head(&mut self) {
        if self.phase != Phase::Idle || self.pending_submit.is_some() {
            return;
        }
        if let Some(text) = self.queue.pop_front() {
            self.transcript.push(TranscriptItem::User(text.clone()));
            self.pending_submit = Some(text);
            self.phase = Phase::Streaming;
            self.scroll = ScrollState::Follow;
            self.dirty = true;
        }
    }

    /// Pop the newest queued prompt back into the editor for editing
    /// (Alt+↑). No-op if the queue is empty.
    pub fn pop_newest_queued_to_editor(&mut self) {
        if let Some(text) = self.queue.pop_back() {
            self.input = text;
            self.cursor = self.input.len();
            self.status_message = format!("{} queued", self.queue.len());
            self.dirty = true;
        }
    }

    /// Delete the newest queued prompt (Alt+-). No-op if empty.
    pub fn delete_newest_queued(&mut self) {
        if self.queue.pop_back().is_some() {
            self.status_message = format!("{} queued", self.queue.len());
            self.dirty = true;
        }
    }

    /// Scroll the transcript up by `n` display lines (enters Free).
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll
            .scroll_up(n, self.layout.total_lines(), self.viewport_h);
        self.dirty = true;
    }

    /// Scroll down by `n` lines (re-enters Follow at the bottom).
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll
            .scroll_down(n, self.layout.total_lines(), self.viewport_h);
        self.dirty = true;
    }

    /// Jump to the top of the transcript.
    pub fn scroll_to_top(&mut self) {
        self.scroll.go_top();
        self.dirty = true;
    }

    /// Jump to the bottom and resume following the live tail.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll.go_bottom();
        self.dirty = true;
    }

    pub fn cycle_mode_forward(&mut self) {
        self.mode = self.mode.cycle_next();
        self.status_message = format!("mode → {}", self.mode.label());
        if self.mode == SessionMode::Plan {
            // Live mode applies immediately (even mid-turn) via
            // Session::apply_live_mode.
            self.transcript.push(TranscriptItem::System(
                "Plan mode: writes blocked until you leave plan mode (Shift+Tab).".into(),
            ));
        }
    }

    pub fn request_cancel(&mut self) {
        if self.phase == Phase::Streaming {
            self.cancel_requested = true;
            self.status_message = "cancelling…".into();
        }
    }

    /// Clear the prompt editor (Esc / Ctrl+C navigation — never cancels a turn).
    pub fn clear_prompt(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    /// Toggle the tasks pane (Ctrl+T). No-op display-wise when no tasks exist,
    /// since the pane is hidden without any.
    pub fn toggle_tasks(&mut self) {
        self.show_tasks = !self.show_tasks;
        self.dirty = true;
    }

    /// Whether the tasks pane should render (has tasks and is toggled on).
    pub fn tasks_visible(&self) -> bool {
        self.show_tasks && !self.tasks.is_empty()
    }

    /// Emit the `/terminal-setup` diagnostics into the transcript: a
    /// capability table plus copyable remediation lines (plan §M7).
    pub fn emit_terminal_setup(&mut self) {
        let c = self.caps;
        let yn = |b: bool| if b { "✓" } else { "✗" };
        let mut report = String::from("terminal-setup:\n");
        report.push_str(&format!("  synchronized output : {}\n", yn(c.sync_output)));
        report.push_str(&format!("  truecolor           : {}\n", yn(c.truecolor)));
        report.push_str(&format!(
            "  kitty keyboard      : {}\n",
            yn(c.kitty_keyboard)
        ));
        report.push_str(&format!("  tmux                : {}\n", yn(c.tmux)));
        let rem = c.remediation();
        if !rem.is_empty() {
            report.push_str("  remediation:\n");
            for line in rem {
                report.push_str(&format!("    {line}\n"));
            }
        }
        self.transcript
            .push(TranscriptItem::System(report.trim_end().to_string()));
        self.dirty = true;
    }

    pub fn mark_turn_idle(&mut self) {
        // Any text still buffered when the turn ends must land now.
        self.flush_stream();
        self.phase = Phase::Idle;
        self.cancel_requested = false;
        self.dirty = true;
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a transcript tall enough to scroll, then prime layout metrics as
    // the draw would (one System block = one wrapped line each here).
    fn app_with_lines(n: usize, viewport_h: usize) -> App {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.clear();
        for i in 0..n {
            app.transcript
                .push(TranscriptItem::System(format!("line {i}")));
        }
        app.layout.sync(&app.transcript, 80);
        app.viewport_h = viewport_h;
        app
    }

    #[test]
    fn scroll_up_enters_free_and_shows_pill() {
        let mut app = app_with_lines(100, 20);
        assert!(app.scroll.is_following());
        app.scroll_up(30);
        assert!(!app.scroll.is_following(), "upward scroll enters Free");
        // Lines are now hidden below the viewport → the pill would show.
        assert!(
            app.scroll
                .lines_below(app.layout.total_lines(), app.viewport_h)
                > 0
        );
    }

    #[test]
    fn new_content_while_free_does_not_move_viewport() {
        let mut app = app_with_lines(100, 20);
        app.scroll_up(40);
        let top_before = app.scroll.top(app.layout.total_lines(), app.viewport_h);
        // Stream 50 more lines in while the user reads.
        for i in 0..50 {
            app.apply_engine(EngineEvent::Text(format!("stream {i}\n")));
        }
        app.flush_stream();
        app.layout.sync(&app.transcript, 80);
        let top_after = app.scroll.top(app.layout.total_lines(), app.viewport_h);
        assert_eq!(top_before, top_after, "viewport must not move while Free");
    }

    #[test]
    fn end_key_returns_to_follow() {
        let mut app = app_with_lines(100, 20);
        app.scroll_up(30);
        assert!(!app.scroll.is_following());
        app.scroll_to_bottom();
        assert!(app.scroll.is_following());
    }

    #[test]
    fn submit_returns_to_follow() {
        let mut app = app_with_lines(100, 20);
        app.scroll_up(30);
        app.input = "hi".into();
        app.cursor = 2;
        app.submit();
        assert!(app.scroll.is_following(), "sending jumps back to the tail");
    }

    #[test]
    fn idle_flush_does_not_dirty() {
        // Zero-frame invariant: with nothing buffered, a flush must not mark
        // the frame dirty, so an idle loop never repaints.
        let mut app = App::new("m", "/tmp", "s");
        app.dirty = false;
        app.flush_stream();
        assert!(!app.dirty, "empty flush must not request a redraw");
        assert!(!app.stream_buf.has_pending());
    }

    #[test]
    fn deltas_dirty_but_do_not_touch_transcript_until_flush() {
        let mut app = App::new("m", "/tmp", "s");
        app.dirty = false;
        let before = app.transcript.len();
        app.apply_engine(EngineEvent::Text("x".into()));
        assert!(app.dirty, "a delta requests a redraw");
        assert_eq!(
            app.transcript.len(),
            before,
            "delta is buffered, not applied"
        );
    }

    #[test]
    fn submit_queues_prompt_and_clears_input() {
        let mut app = App::new("gpt-test", "/tmp", "sid");
        app.input = "hello world".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(app.pending_submit.as_deref(), Some("hello world"));
        assert!(app.input.is_empty());
        assert_eq!(app.phase, Phase::Streaming);
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptItem::User(t)) if t == "hello world"
        ));
    }

    #[test]
    fn exit_slash_quits() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "/exit".into();
        app.cursor = 5;
        app.submit();
        assert!(app.should_quit);
        assert!(app.pending_submit.is_none());
    }

    #[test]
    fn engine_text_coalesces_and_flushes() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::Text("Hel".into()));
        app.apply_engine(EngineEvent::Text("lo".into()));
        // Deltas are buffered, not yet in the transcript.
        assert!(app.stream_buf.has_pending());
        assert!(!matches!(
            app.transcript.last(),
            Some(TranscriptItem::Assistant(_))
        ));
        app.flush_stream();
        match app.transcript.last() {
            Some(TranscriptItem::Assistant(t)) => assert_eq!(t, "Hello"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn non_delta_event_flushes_buffered_text_first() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::Text("partial answer".into()));
        // A tool start is a barrier: it must flush the text before it applies,
        // so the assistant text lands *before* the tool card in the transcript.
        app.apply_engine(EngineEvent::ToolStart {
            name: "Bash".into(),
            detail: "ls".into(),
        });
        let n = app.transcript.len();
        assert!(
            matches!(&app.transcript[n - 2], TranscriptItem::Assistant(t) if t == "partial answer")
        );
        assert!(matches!(
            &app.transcript[n - 1],
            TranscriptItem::Tool { .. }
        ));
        assert!(!app.stream_buf.has_pending());
    }

    #[test]
    fn tool_results_attach_in_start_order() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::ToolStart {
            name: "Bash".into(),
            detail: "first".into(),
        });
        app.apply_engine(EngineEvent::ToolStart {
            name: "Grep".into(),
            detail: "second".into(),
        });
        app.apply_engine(EngineEvent::ToolResult {
            name: "Bash".into(),
            content: "r1".into(),
            is_error: false,
        });
        let n = app.transcript.len();
        match &app.transcript[n - 2] {
            TranscriptItem::Tool { detail, result, .. } => {
                assert_eq!(detail, "first");
                assert_eq!(result.as_deref(), Some("r1"));
            }
            other => panic!("{other:?}"),
        }
        match &app.transcript[n - 1] {
            TranscriptItem::Tool { detail, result, .. } => {
                assert_eq!(detail, "second");
                assert!(result.is_none());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn enter_while_streaming_queues_not_drops() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.input = "fix the flaky test".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.queue.front().unwrap(), "fix the flaky test");
        assert!(app.input.is_empty());
        assert!(
            app.pending_submit.is_none(),
            "must not start a turn mid-turn"
        );
    }

    #[test]
    fn queue_dispatches_head_on_idle() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.queue.push_back("first".into());
        app.queue.push_back("second".into());
        // Turn ends.
        app.mark_turn_idle();
        app.dispatch_queue_head();
        assert_eq!(app.pending_submit.as_deref(), Some("first"));
        assert_eq!(app.phase, Phase::Streaming);
        assert_eq!(app.queue.len(), 1, "second stays queued");
    }

    #[test]
    fn dispatch_is_noop_while_busy() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.queue.push_back("later".into());
        app.dispatch_queue_head();
        assert!(app.pending_submit.is_none());
        assert_eq!(app.queue.len(), 1);
    }

    #[test]
    fn alt_up_pops_newest_to_editor() {
        let mut app = App::new("m", "/tmp", "s");
        app.queue.push_back("one".into());
        app.queue.push_back("two".into());
        app.pop_newest_queued_to_editor();
        assert_eq!(app.input, "two");
        assert_eq!(app.queue.len(), 1);
        app.delete_newest_queued();
        assert!(app.queue.is_empty());
    }

    #[test]
    fn skin_slash_commands_toggle() {
        let mut app = App::new("m", "/tmp", "s");
        assert_eq!(app.skin, Skin::Fullscreen);
        app.input = "/minimal".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(app.skin, Skin::Minimal);
        assert!(app.pending_submit.is_none(), "skin toggle is not a turn");
        app.input = "/fullscreen".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(app.skin, Skin::Fullscreen);
    }

    #[test]
    fn stats_command_reports_counts() {
        let mut app = App::new("m", "/tmp", "s");
        app.frame_count = 7;
        app.layout.sync(&app.transcript, 80);
        app.input = "/stats".into();
        app.cursor = app.input.len();
        app.submit();
        match app.transcript.last() {
            Some(TranscriptItem::System(t)) => {
                assert!(t.contains("frames drawn"), "{t}");
                assert!(t.contains("7 frames"), "{t}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn terminal_setup_reports_caps_and_remediation() {
        let mut app = App::new("m", "/tmp", "s");
        app.caps = super::super::terminal_caps::TerminalCaps {
            sync_output: true,
            truecolor: false,
            kitty_keyboard: false,
            tmux: true,
        };
        app.emit_terminal_setup();
        let last = match app.transcript.last() {
            Some(TranscriptItem::System(t)) => t.clone(),
            other => panic!("{other:?}"),
        };
        assert!(last.contains("terminal-setup"));
        assert!(last.contains("synchronized output : ✓"));
        assert!(last.contains("tmux                : ✓"));
        // tmux + no-truecolor + no-kitty → remediation lines present.
        assert!(last.contains("allow-passthrough"));
        assert!(last.contains("COLORTERM=truecolor"));
    }

    #[test]
    fn subagent_update_populates_and_orders_tasks() {
        let mut app = App::new("m", "/tmp", "s");
        assert!(!app.tasks_visible(), "no pane without tasks");
        app.apply_engine(EngineEvent::SubagentUpdate {
            agent_id: "research-1".into(),
            state: "working".into(),
            headline: "scanning crates/".into(),
        });
        app.apply_engine(EngineEvent::SubagentUpdate {
            agent_id: "edit-1".into(),
            state: "needs input".into(),
            headline: "confirm write".into(),
        });
        assert_eq!(app.tasks.len(), 2);
        assert!(app.tasks_visible());
        // needs-input floats to the top.
        assert_eq!(app.tasks[0].agent_id, "edit-1");
    }

    #[test]
    fn ctrl_t_toggles_pane_visibility() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::SubagentUpdate {
            agent_id: "a".into(),
            state: "working".into(),
            headline: "h".into(),
        });
        assert!(app.tasks_visible());
        app.toggle_tasks();
        assert!(!app.tasks_visible(), "toggled off");
        app.toggle_tasks();
        assert!(app.tasks_visible(), "toggled back on");
    }

    #[test]
    fn context_usage_updates_meter() {
        let mut app = App::new("m", "/tmp", "s");
        assert!(app.ctx_meter.is_none());
        app.apply_engine(EngineEvent::ContextUsage {
            used: 41_000,
            max: 100_000,
        });
        assert_eq!(app.ctx_meter, Some((41_000, 100_000)));
    }

    #[test]
    fn waiting_on_tracks_tool_lifecycle() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TurnStart(1));
        assert_eq!(app.waiting_on, WaitingOn::Model);
        app.apply_engine(EngineEvent::ToolStart {
            name: "Bash".into(),
            detail: "cargo test".into(),
        });
        assert_eq!(app.waiting_on, WaitingOn::Tool("Bash".into()));
        app.apply_engine(EngineEvent::ToolResult {
            name: "Bash".into(),
            content: "ok".into(),
            is_error: false,
        });
        assert_eq!(app.waiting_on, WaitingOn::Model);
    }

    #[test]
    fn waiting_on_user_input_during_permission() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TurnStart(1));
        let (respond, _rx) = std::sync::mpsc::channel();
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "Bash".into(),
            description: "d".into(),
            input_preview: None,
            respond,
        });
        assert_eq!(app.waiting_on, WaitingOn::UserInput);
        assert_eq!(app.waiting_on.label(), "waiting on your input");
        app.resolve_permission(PermissionResponse::AllowOnce);
        assert_eq!(app.waiting_on, WaitingOn::Model);
    }

    #[test]
    fn stream_error_does_not_end_turn_phase() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TurnStart(1));
        assert_eq!(app.phase, Phase::Streaming);
        app.apply_engine(EngineEvent::Error("boom".into()));
        // The engine keeps running after stream errors; only the run
        // loop's reap (or TurnComplete) may mark the UI idle.
        assert_eq!(app.phase, Phase::Streaming);
    }

    #[test]
    fn permission_ask_opens_modal_and_resolve_unblocks() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TurnStart(1));
        let (respond, rx) = std::sync::mpsc::channel();
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "Bash".into(),
            description: "Bash: run `cargo test`".into(),
            input_preview: Some("{\"command\": \"cargo test\"}".into()),
            respond,
        });
        assert_eq!(app.phase, Phase::Permission);
        assert!(app.front_permission().is_some());

        app.resolve_permission(PermissionResponse::AllowOnce);
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::AllowOnce)));
        assert_eq!(app.phase, Phase::Streaming);
        assert!(app.front_permission().is_none());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptItem::System(t)) if t.contains("allowed Bash once")
        ));
    }

    #[test]
    fn plan_proposed_opens_modal_and_approve_switches_mode() {
        let mut app = App::new("m", "/tmp", "s");
        app.mode = SessionMode::Plan;
        app.apply_engine(EngineEvent::TurnStart(1));
        app.apply_engine(EngineEvent::PlanProposed {
            plan_md: "# Plan\n\ndo the thing".into(),
            path: Some("/tmp/plans/x.md".into()),
        });
        assert_eq!(app.phase, Phase::Permission);
        assert!(matches!(app.front_modal(), Some(Modal::Plan(_))));
        // Approve → leaves plan mode into AcceptEdits so the follow-up can run.
        assert!(app.resolve_plan(true, false));
        assert_eq!(app.mode, SessionMode::AcceptEdits);
        assert_eq!(app.phase, Phase::Streaming);
    }

    #[test]
    fn question_flow_collects_one_label_per_question() {
        use super::super::sink::UiQuestion;
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TurnStart(1));
        let (respond, rx) = std::sync::mpsc::channel();
        app.apply_engine(EngineEvent::QuestionAsk {
            questions: vec![
                UiQuestion {
                    question: "pick a".into(),
                    options: vec!["a1".into(), "a2".into()],
                },
                UiQuestion {
                    question: "pick b".into(),
                    options: vec!["b1".into(), "b2".into()],
                },
            ],
            respond,
        });
        assert_eq!(app.phase, Phase::Permission);
        // Answer Q1 with option 2, Q2 by moving cursor then Enter.
        app.question_select(Some(1)); // "a2"
        assert!(matches!(app.front_modal(), Some(Modal::Question(_))));
        app.question_move(1); // cursor → "b2"
        app.question_select(None); // "b2"
        let answers = rx.try_recv().unwrap();
        assert_eq!(answers, vec!["a2".to_string(), "b2".to_string()]);
        assert_eq!(app.phase, Phase::Streaming);
    }

    #[test]
    fn empty_question_set_answers_immediately() {
        let mut app = App::new("m", "/tmp", "s");
        let (respond, rx) = std::sync::mpsc::channel();
        app.apply_engine(EngineEvent::QuestionAsk {
            questions: vec![],
            respond,
        });
        assert!(rx.try_recv().unwrap().is_empty());
        assert!(app.modals.is_empty());
    }

    #[test]
    fn concurrent_permission_asks_queue_fifo() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::TurnStart(1));
        let (r1, rx1) = std::sync::mpsc::channel();
        let (r2, rx2) = std::sync::mpsc::channel();
        // Lead + background subagent ask at the same time.
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "Bash".into(),
            description: "lead".into(),
            input_preview: None,
            respond: r1,
        });
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "FileWrite".into(),
            description: "subagent".into(),
            input_preview: None,
            respond: r2,
        });
        // Both are queued; front is the first, badge shows 1 behind.
        assert_eq!(app.front_permission().unwrap().name, "Bash");
        assert_eq!(app.pending_modal_count(), 1);

        app.resolve_permission(PermissionResponse::AllowOnce);
        assert!(matches!(rx1.try_recv(), Ok(PermissionResponse::AllowOnce)));
        // Still in Permission phase — the second modal advances to the front.
        assert_eq!(app.phase, Phase::Permission);
        assert_eq!(app.front_permission().unwrap().name, "FileWrite");

        app.resolve_permission(PermissionResponse::Deny);
        assert!(matches!(rx2.try_recv(), Ok(PermissionResponse::Deny)));
        assert_eq!(app.phase, Phase::Streaming);
    }

    #[test]
    fn deny_all_modals_unblocks_everything() {
        let mut app = App::new("m", "/tmp", "s");
        let (r1, rx1) = std::sync::mpsc::channel();
        let (r2, rx2) = std::sync::mpsc::channel();
        for (name, respond) in [("a", r1), ("b", r2)] {
            app.apply_engine(EngineEvent::PermissionAsk {
                name: name.into(),
                description: name.into(),
                input_preview: None,
                respond,
            });
        }
        app.deny_all_modals();
        assert!(matches!(rx1.try_recv(), Ok(PermissionResponse::Deny)));
        assert!(matches!(rx2.try_recv(), Ok(PermissionResponse::Deny)));
        assert!(app.modals.is_empty());
    }

    #[test]
    fn resolve_permission_without_pending_is_noop() {
        let mut app = App::new("m", "/tmp", "s");
        let before = app.transcript.len();
        app.resolve_permission(PermissionResponse::Deny);
        assert_eq!(app.transcript.len(), before);
    }
}
