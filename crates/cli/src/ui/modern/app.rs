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

/// A permission ask the engine is blocked on, awaiting the user's answer.
#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub name: String,
    pub description: String,
    pub input_preview: Option<String>,
    pub respond: std::sync::mpsc::Sender<PermissionResponse>,
}

/// A modal awaiting user input. Displayed FIFO — the front is shown; the
/// rest wait behind a "⚠ N pending" badge (plan §M6). Currently only
/// permission asks; plan-approval and ask-user overlays extend this enum
/// once the engine emits their events.
#[derive(Debug, Clone)]
pub enum Modal {
    Permission(PendingPermission),
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
    /// True while the UI mode has not yet been applied to the engine
    /// (the turn task holds the engine lock). The badge shows a `*`.
    pub mode_pending: bool,
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
            mode_pending: false,
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
            status_message: String::new(),
            should_quit: false,
            pending_submit: None,
            queue: std::collections::VecDeque::new(),
            cancel_requested: false,
            quit_armed: false,
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
        }
    }

    /// The permission ask currently displayed (front of the modal queue).
    pub fn front_permission(&self) -> Option<&PendingPermission> {
        match self.modals.front() {
            Some(Modal::Permission(p)) => Some(p),
            None => None,
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

    /// Answer the front permission ask and advance the modal queue. When the
    /// queue empties, focus returns to the running turn.
    pub fn resolve_permission(&mut self, resp: PermissionResponse) {
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
        if self.modals.is_empty() && self.phase == Phase::Permission {
            self.phase = Phase::Streaming;
            self.waiting_on = WaitingOn::Model;
        }
        self.dirty = true;
    }

    /// Deny every queued modal (used on shutdown so blocked turn tasks in
    /// the prompter never deadlock the join).
    pub fn deny_all_modals(&mut self) {
        while let Some(Modal::Permission(p)) = self.modals.pop_front() {
            let _ = p.respond.send(PermissionResponse::Deny);
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
                 Esc clear prompt · permission prompt: y once / a session / n deny · /clear /exit"
                    .into(),
            ));
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
            let msg = if self.phase == Phase::Streaming {
                "Plan mode: applies when the engine is free (badge shows * until then)."
            } else {
                "Plan mode: writes blocked until you leave plan mode (Shift+Tab)."
            };
            self.transcript.push(TranscriptItem::System(msg.into()));
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
