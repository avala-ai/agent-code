//! Application state for the modern TUI.
//!
//! Pure data + reducers. Drawing lives in [`super::render`]; I/O in
//! [`super::run`]. This split keeps visual tests free of a live terminal.

use super::layout::LayoutCache;
use super::mode::SessionMode;
use super::scroll::ScrollState;
use super::sink::EngineEvent;
use super::stream_buffer::StreamBuffer;
use super::tasks::TaskEntry;
use super::terminal_caps::TerminalCaps;

// Modal types + resolvers live in `modal.rs`; re-export so existing
// `app::Modal` / `app::PendingPermission` paths keep working.
pub use super::modal::{Modal, PendingPermission, PlanReview, QuestionState};

/// Local `/model` action deferred to the run loop (needs the engine lock).
/// Classic uses an interactive stdin selector; under the alt-screen TUI we
/// list models in the transcript and accept `/model <id>` to switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingModelAction {
    /// Show current model + provider catalog in the transcript.
    Show,
    /// Set `config.api.model` (and the status-bar badge) to this id.
    Set(String),
}

/// Parse `/model` / `/model <id>` from a slash line. Returns `None` if the
/// input is not a model command.
pub(crate) fn parse_model_slash(input: &str) -> Option<PendingModelAction> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let rest = trimmed.trim_start_matches('/');
    let (cmd, args) = match rest.split_once(char::is_whitespace) {
        Some((c, a)) => (c, Some(a.trim())),
        None => (rest, None),
    };
    if !cmd.eq_ignore_ascii_case("model") {
        return None;
    }
    match args {
        None | Some("") => Some(PendingModelAction::Show),
        Some(name) => Some(PendingModelAction::Set(name.to_string())),
    }
}

/// Format `/model` catalog lines for the transcript (no stdin selector).
pub(crate) fn format_model_catalog(current: &str, base_url: &str) -> Vec<String> {
    let provider = agent_code_lib::llm::provider::detect_provider(current, base_url);
    let models = agent_code_lib::llm::provider::models_for_provider(provider);
    let mut lines = vec![format!("Model: {current}")];
    if models.is_empty() {
        lines.push("Use /model <name> to change.".into());
    } else {
        lines.push("Available models (use /model <id>):".into());
        for (name, desc) in models {
            let mark = if *name == current { " ✔" } else { "" };
            lines.push(format!("  {name}{mark}  — {desc}"));
        }
    }
    lines
}

/// Expand a user-invocable skill slash (`/commit`, `/review foo`) to its
/// prompt body. Returns `None` for non-slash input, modern-local commands,
/// or unknown skill names (those pass through as normal prompts).
///
/// Honors `security.disable_skill_shell_execution` via
/// [`Skill::expand_safe`](agent_code_lib::skills::Skill::expand_safe) so
/// fenced shell blocks are stripped when that policy is on.
pub(crate) fn try_expand_skill_slash(
    input: &str,
    cwd: &str,
    disable_skill_shell: bool,
) -> Option<String> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let rest = trimmed.trim_start_matches('/');
    if rest.is_empty() {
        return None;
    }
    let (cmd, args) = match rest.split_once(char::is_whitespace) {
        Some((c, a)) => (c, Some(a.trim())),
        None => (rest, None),
    };
    // Local modern commands are handled before we get here; still skip them
    // so a skill never shadows `/help` etc.
    if matches!(
        cmd,
        "exit"
            | "quit"
            | "clear"
            | "help"
            | "terminal-setup"
            | "copy"
            | "minimal"
            | "fullscreen"
            | "stats"
            | "model"
            | "cost"
            | "usage"
            | "version"
            | "status"
            | "plan"
            | "theme"
            | "permissions"
            | "queue"
            | "tasks"
    ) {
        return None;
    }
    let registry = agent_code_lib::skills::SkillRegistry::load_all(Some(std::path::Path::new(cwd)));
    let skill = registry.find(cmd)?;
    // Only user-invocable skills are slash-callable (same as classic /help).
    if !skill.metadata.user_invocable {
        return None;
    }
    Some(skill.expand_safe(args, disable_skill_shell))
}

/// One row in the scrollable transcript.
#[derive(Debug, Clone, Hash)]
pub enum TranscriptItem {
    User(String),
    Assistant(String),
    Thinking(String),
    Tool {
        /// Engine tool-call id used to correlate the result to this card.
        /// Empty when the engine used the legacy id-less callback.
        call_id: String,
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
    /// Mirror of `security.disable_skill_shell_execution` for skill slash
    /// expansion (must not bypass the policy that strips fenced shell).
    pub disable_skill_shell: bool,

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
    /// Byte index into `input` (must land on a char boundary).
    pub cursor: usize,
    /// When true: Enter inserts newline; Alt/Shift+Enter submit.
    /// When false (default): Enter submits; Alt/Shift+Enter insert newline.
    /// Toggle with Ctrl+M (prompt-focused).
    pub multiline_mode: bool,
    /// Past submitted prompts (oldest first). ↑ on empty input browses.
    pub prompt_history: Vec<String>,
    /// When `Some(i)`, the composer is showing `prompt_history[i]`.
    pub history_browse: Option<usize>,
    /// Transcript item indices whose bodies are fully expanded (tools /
    /// thinking). Default is collapsed (clamped renderer head).
    pub expanded: std::collections::HashSet<usize>,
    /// Currently selected transcript item (for fold / turn jumps).
    pub selected_item: Option<usize>,

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
    /// `/model` action waiting for the run loop (needs the engine lock).
    pub pending_model: Option<PendingModelAction>,
    /// `/clear` also clears the ENGINE conversation; applied by the run
    /// loop under try_lock (mirrors `pending_model`).
    pub pending_clear: bool,
    /// Classic slash line deferred to the run loop (needs engine + stdout
    /// capture). Full built-in command parity with the classic REPL.
    pub pending_slash: Option<String>,
    /// `!cmd` shell passthrough deferred to the run loop.
    pub pending_shell: Option<String>,
    /// Whether a turn task currently exists (set by the run loop). Lets
    /// modal resolution decide between returning to Streaming (turn still
    /// running) and Idle (modal outlived its turn).
    pub turn_live: bool,
    /// Absolute screen row of the transcript's bottom line, recorded at the
    /// last draw (0 = not drawn yet). Mouse hit-testing target for the
    /// click-to-follow jump pill.
    pub transcript_bottom_row: u16,
    /// Prompts typed mid-turn, sent FIFO when the turn ends (plan §M5).
    pub queue: std::collections::VecDeque<String>,
    /// Queue pane open (Ctrl+; / `/queue`). Shows full list + selection.
    pub show_queue_pane: bool,
    /// Selected row in the queue pane (0 = head).
    pub queue_selected: usize,
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
        Self::new_with_security(model, cwd, session_id, false)
    }

    /// Like [`Self::new`] but records the skill-shell security policy from
    /// the engine config (used by the live run loop).
    pub fn new_with_security(
        model: impl Into<String>,
        cwd: impl Into<String>,
        session_id: impl Into<String>,
        disable_skill_shell: bool,
    ) -> Self {
        Self {
            model: model.into(),
            cwd: cwd.into(),
            session_id: session_id.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            disable_skill_shell,
            mode: SessionMode::Normal,
            phase: Phase::Idle,
            waiting_on: WaitingOn::Model,
            modals: std::collections::VecDeque::new(),
            transcript: vec![TranscriptItem::System(
                "Modern TUI · Enter send · Alt+Enter newline · Shift+Tab mode · Ctrl+C cancel · Esc never cancels".into(),
            )],
            scroll: ScrollState::Follow,
            layout: LayoutCache::default(),
            viewport_h: 0,
            input: String::new(),
            cursor: 0,
            multiline_mode: false,
            prompt_history: Vec::new(),
            history_browse: None,
            expanded: std::collections::HashSet::new(),
            selected_item: None,
            turn_count: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            ctx_meter: None,
            status_message: String::new(),
            should_quit: false,
            pending_submit: None,
            pending_model: None,
            pending_clear: false,
            pending_slash: None,
            pending_shell: None,
            turn_live: false,
            transcript_bottom_row: 0,
            queue: std::collections::VecDeque::new(),
            show_queue_pane: false,
            queue_selected: 0,
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
        // Text/thinking deltas are coalesced; everything else is a "barrier"
        // event that must flush buffered text first so ordering is preserved
        // (plan §2.2 rule 3: deltas never reorder around tool/turn events).
        // Deltas do NOT set dirty: buffered text isn't rendered until it is
        // flushed into the transcript, so repainting per delta drew identical
        // frames at token rate — paying a full layout resync each time — and
        // defeated the ≤10 fps coalescer budget. The flush tick sets dirty.
        match ev {
            EngineEvent::Text(t) => {
                self.stream_buf.push_assistant(&t);
                return;
            }
            EngineEvent::Thinking(t) => {
                self.stream_buf.push_thinking(&t);
                return;
            }
            _ => {
                self.flush_stream();
                // Any barrier event changes visible state — repaint.
                self.dirty = true;
            }
        }

        match ev {
            // Deltas handled above.
            EngineEvent::Text(_) | EngineEvent::Thinking(_) => unreachable!(),
            EngineEvent::ToolStart {
                call_id,
                name,
                detail,
            } => {
                self.waiting_on = WaitingOn::Tool(name.clone());
                self.transcript.push(TranscriptItem::Tool {
                    call_id,
                    name,
                    detail,
                    result: None,
                    is_error: false,
                });
            }
            EngineEvent::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } => {
                // Correlate by the engine's stable call_id so parallel tool
                // calls attach to the right card regardless of completion
                // order. Fall back to the oldest still-pending card when the
                // id is absent (legacy id-less callback) or unmatched.
                let by_id = (!call_id.is_empty())
                    .then(|| {
                        self.transcript.iter().position(|i| {
                            matches!(
                                i,
                                TranscriptItem::Tool { call_id: c, result: None, .. }
                                    if *c == call_id
                            )
                        })
                    })
                    .flatten();
                let idx = by_id.or_else(|| {
                    self.transcript
                        .iter()
                        .position(|i| matches!(i, TranscriptItem::Tool { result: None, .. }))
                });
                if let Some(TranscriptItem::Tool {
                    result,
                    is_error: err,
                    ..
                }) = idx.and_then(|i| self.transcript.get_mut(i))
                {
                    // Keep the FULL result on the card (the renderer clamps
                    // to a few lines). Keeping only the first line destroyed
                    // failure output — a failing `cargo test` showed a single
                    // useless line with no way to see more.
                    *result = Some(content);
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
                // A pending modal keeps the Permission phase: PlanProposed is
                // fire-and-forget and typically arrives right before this
                // event, so flipping to Idle here orphaned the plan-approval
                // modal (invisible, unanswerable, blocking later modals).
                self.phase = if self.modals.is_empty() {
                    Phase::Idle
                } else {
                    Phase::Permission
                };
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
                origin,
                input_preview,
                respond,
            } => {
                // FIFO: concurrent asks (e.g. lead + background subagent)
                // queue behind the current one instead of being dropped.
                self.modals.push_back(Modal::Permission(PendingPermission {
                    name,
                    description,
                    origin,
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
        self.history_browse = None; // in-place edit leaves history mode
        self.dirty = true;
    }

    /// Insert a newline at the cursor (Alt+Enter / Shift+Enter in normal mode).
    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Toggle multiline compose mode (Ctrl+M). Flips Enter vs Alt/Shift+Enter.
    pub fn toggle_multiline_mode(&mut self) {
        self.multiline_mode = !self.multiline_mode;
        self.status_message = if self.multiline_mode {
            "multiline on — Enter newline · Alt/Shift+Enter send".into()
        } else {
            "multiline off — Enter send · Alt/Shift+Enter newline".into()
        };
        self.dirty = true;
    }

    /// Insert a pasted string at the cursor (bracketed paste / Event::Paste).
    /// Same phase gate as [`insert_char`]. Preserves newlines so multi-line
    /// pastes (code, logs) survive into the next submit.
    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.phase != Phase::Idle && self.phase != Phase::Streaming {
            return;
        }
        // Normalize CRLF → LF so Windows pastes don't leave bare `\r`s.
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let idx = self.cursor.min(self.input.len());
        self.input.insert_str(idx, &normalized);
        self.cursor = idx + normalized.len();
        self.history_browse = None;
        self.dirty = true;
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
            self.dirty = true;
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
        self.dirty = true;
    }

    pub fn move_right(&mut self) {
        if let Some((i, c)) = self.input[self.cursor..].char_indices().next() {
            self.cursor += i + c.len_utf8();
            self.dirty = true;
        }
    }

    /// Move cursor to the previous visual line (same column when possible).
    pub fn move_up_line(&mut self) {
        let (line, col) = self.cursor_line_col();
        if line == 0 {
            return;
        }
        self.cursor = self.byte_at_line_col(line - 1, col);
        self.dirty = true;
    }

    /// Move cursor to the next visual line (same column when possible).
    pub fn move_down_line(&mut self) {
        let (line, col) = self.cursor_line_col();
        let lines = self.input_line_count();
        if line + 1 >= lines {
            return;
        }
        self.cursor = self.byte_at_line_col(line + 1, col);
        self.dirty = true;
    }

    /// Home of current line (not whole buffer).
    pub fn move_line_start(&mut self) {
        let (line, _) = self.cursor_line_col();
        self.cursor = self.byte_at_line_col(line, 0);
        self.dirty = true;
    }

    /// End of current line.
    pub fn move_line_end(&mut self) {
        let (line, _) = self.cursor_line_col();
        let len = self.line_char_len(line);
        self.cursor = self.byte_at_line_col(line, len);
        self.dirty = true;
    }

    /// Number of lines in the composer (at least 1).
    pub fn input_line_count(&self) -> usize {
        // "a\n" is two lines (second empty); `lines()` would count only one.
        self.input.chars().filter(|c| *c == '\n').count() + 1
    }

    /// (line, col) of the cursor; col is a char index on that line.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let cursor = self.cursor.min(self.input.len());
        let before = &self.input[..cursor];
        let line = before.chars().filter(|c| *c == '\n').count();
        let col = before
            .rsplit('\n')
            .next()
            .map(|s| s.chars().count())
            .unwrap_or(0);
        (line, col)
    }

    fn line_char_len(&self, line: usize) -> usize {
        self.input
            .split('\n')
            .nth(line)
            .map(|s| s.chars().count())
            .unwrap_or(0)
    }

    fn byte_at_line_col(&self, line: usize, col: usize) -> usize {
        let mut cur_line = 0usize;
        let mut cur_col = 0usize;
        for (bi, ch) in self.input.char_indices() {
            if cur_line == line && cur_col == col {
                return bi;
            }
            if ch == '\n' {
                if cur_line == line {
                    // Past end of requested line — clamp to EOL.
                    return bi;
                }
                cur_line += 1;
                cur_col = 0;
            } else {
                cur_col += 1;
            }
        }
        if cur_line == line {
            return self.input.len();
        }
        self.input.len()
    }

    /// True when the composer holds more than one line (or trailing newline).
    pub fn input_is_multiline(&self) -> bool {
        self.input.contains('\n')
    }

    pub fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            // Empty-Enter while idle sends the next queued prompt — after an
            // aborted turn the UI says "queued prompts kept — press Enter to
            // send", and this is what makes that true.
            if self.phase == Phase::Idle {
                self.dispatch_queue_head();
            }
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
            // Also clear the ENGINE conversation (classic parity): clearing
            // only the view silently kept paying for the entire prior
            // context. The run loop applies this under try_lock.
            self.pending_clear = true;
            self.ctx_meter = None;
            self.input.clear();
            self.cursor = 0;
            self.dirty = true;
            return;
        }
        if text == "/help" {
            self.transcript.push(TranscriptItem::System(
                "Keys: Enter send · Alt+Enter newline · Shift+Tab mode · Ctrl+C cancel · Esc never cancels · \
                 Ctrl+T tasks · Ctrl+; queue · y/Y copy block · e expand · \
                 /model /copy /cost /status /plan /theme /queue /terminal-setup /stats /exit · skills: /commit …"
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
        if text == "/copy" {
            self.copy_last_assistant();
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
        if text == "/cost" || text == "/usage" {
            let tok = self.tokens_in + self.tokens_out;
            self.transcript.push(TranscriptItem::System(format!(
                "usage: turn {} · {} in / {} out ({} total) · ${:.4} · model {}",
                self.turn_count, self.tokens_in, self.tokens_out, tok, self.cost_usd, self.model
            )));
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/version" {
            self.transcript.push(TranscriptItem::System(format!(
                "agent-code {} · modern TUI",
                self.version
            )));
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/status" {
            self.transcript.push(TranscriptItem::System(format!(
                "status: model={} · mode={} · phase={:?} · cwd={} · sid={} · queue={}",
                self.model,
                self.mode.label(),
                self.phase,
                self.cwd,
                self.session_id,
                self.queue.len()
            )));
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/plan" {
            self.mode = SessionMode::Plan;
            self.status_message = "mode → PLAN".into();
            self.transcript.push(TranscriptItem::System(
                "mode → PLAN (Shift+Tab to cycle)".into(),
            ));
            self.input.clear();
            self.cursor = 0;
            self.dirty = true;
            return;
        }
        if text == "/theme" {
            // Cycle skins for now (full theme pack is later).
            self.skin = match self.skin {
                Skin::Fullscreen => Skin::Minimal,
                Skin::Minimal => Skin::Fullscreen,
            };
            self.status_message = format!("skin → {:?}", self.skin);
            self.transcript.push(TranscriptItem::System(format!(
                "skin → {:?}  (/minimal · /fullscreen)",
                self.skin
            )));
            self.input.clear();
            self.cursor = 0;
            self.dirty = true;
            return;
        }
        if text == "/permissions" {
            self.transcript.push(TranscriptItem::System(format!(
                "permissions: mode={} · Shift+Tab cycles Manual/Normal/AcceptEdits/Plan · \
                 modal: y once / a session / n deny",
                self.mode.label()
            )));
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/queue" {
            self.toggle_queue_pane();
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if text == "/tasks" {
            self.toggle_tasks();
            self.input.clear();
            self.cursor = 0;
            return;
        }
        // /model needs the engine lock (run loop applies). Handled even
        // mid-turn so a switch is not sent to the LLM as a prompt.
        if let Some(action) = parse_model_slash(&text) {
            self.pending_model = Some(action);
            self.input.clear();
            self.cursor = 0;
            self.dirty = true;
            return;
        }
        // `!shell` passthrough — run loop captures output into history.
        if let Some(cmd) = text.strip_prefix('!').map(str::trim)
            && !cmd.is_empty()
        {
            self.pending_shell = Some(cmd.to_string());
            self.transcript
                .push(TranscriptItem::User(format!("!{cmd}")));
            self.input.clear();
            self.cursor = 0;
            self.dirty = true;
            return;
        }

        // Classic slash commands not handled above → full command bridge.
        if text.starts_with('/') {
            let name = text
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("");
            if crate::commands::is_builtin_command(name) {
                self.pending_slash = Some(text.clone());
                self.input.clear();
                self.cursor = 0;
                self.dirty = true;
                return;
            }
        }

        // Mid-turn: queue the prompt instead of dropping it (plan §M5).
        // Skill expansion happens when the queue head is dispatched so the
        // expanded body is what the engine sees.
        if self.phase == Phase::Streaming {
            self.queue.push_back(text);
            self.input.clear();
            self.cursor = 0;
            self.status_message = format!("{} queued", self.queue.len());
            return;
        }
        self.enqueue_turn(text);
    }

    /// Tab-complete a partial slash command in the composer.
    pub fn complete_slash_tab(&mut self) {
        let trimmed = self.input.trim();
        if !trimmed.starts_with('/') {
            return;
        }
        // Only complete the command token (before first space).
        if trimmed.contains(char::is_whitespace) {
            return;
        }
        let partial = trimmed.trim_start_matches('/');
        let cands = crate::commands::complete_slash(partial);
        match cands.as_slice() {
            [] => {
                self.status_message = "no matching commands".into();
            }
            [only] => {
                self.input = format!("/{only} ");
                self.cursor = self.input.len();
                self.status_message = format!("/{only}");
            }
            many => {
                // Longest common prefix among candidates.
                let mut prefix = many[0].to_string();
                for c in &many[1..] {
                    while !c.starts_with(&prefix) && !prefix.is_empty() {
                        prefix.pop();
                    }
                }
                if prefix.len() > partial.len() {
                    self.input = format!("/{prefix}");
                    self.cursor = self.input.len();
                }
                let list = many.iter().take(12).copied().collect::<Vec<_>>().join(" ");
                let more = if many.len() > 12 {
                    format!(" …+{}", many.len() - 12)
                } else {
                    String::new()
                };
                self.transcript
                    .push(TranscriptItem::System(format!("commands: {list}{more}")));
                self.status_message = format!("{} matches", many.len());
            }
        }
        self.dirty = true;
    }

    /// Apply a deferred `/model` action against live engine state.
    /// Returns `true` if applied (caller should clear `pending_model`).
    pub fn apply_model_action(
        &mut self,
        action: PendingModelAction,
        current_model: &str,
        base_url: &str,
        set_model: impl FnOnce(String),
    ) {
        match action {
            PendingModelAction::Show => {
                for line in format_model_catalog(current_model, base_url) {
                    self.transcript.push(TranscriptItem::System(line));
                }
            }
            PendingModelAction::Set(name) => {
                set_model(name.clone());
                self.model = name.clone();
                self.status_message = format!("model → {name}");
                self.transcript
                    .push(TranscriptItem::System(format!("Model changed to: {name}")));
            }
        }
        self.dirty = true;
    }

    /// Enqueue a prompt produced by a slash command (`CommandResult::Prompt`).
    pub fn enqueue_turn_from_command(&mut self, text: String) {
        self.enqueue_turn(text);
    }

    /// Resolve user text into a turn: expand `/skill` invocations the same
    /// way slash dispatch does via `commands::execute` skill lookup.
    fn enqueue_turn(&mut self, text: String) {
        let (display, prompt) =
            match try_expand_skill_slash(&text, &self.cwd, self.disable_skill_shell) {
                Some(expanded) => {
                    // Keep the slash visible in the transcript; send the expanded
                    // skill body to the engine as the real user message.
                    (text, expanded)
                }
                None => {
                    // A slash input that is neither a built-in nor a skill is a
                    // typo — sending it to the model as a prompt costs a turn
                    // and does nothing useful.
                    if text.starts_with('/') {
                        self.transcript.push(TranscriptItem::System(format!(
                            "unknown command {text} — /help lists available commands"
                        )));
                        self.input.clear();
                        self.cursor = 0;
                        self.dirty = true;
                        return;
                    }
                    (text.clone(), text)
                }
            };
        self.push_prompt_history(&display);
        self.transcript.push(TranscriptItem::User(display));
        self.input.clear();
        self.cursor = 0;
        self.history_browse = None;
        self.pending_submit = Some(prompt);
        self.phase = Phase::Streaming;
        // Jump back to the live tail when the user sends.
        self.scroll = ScrollState::Follow;
        self.dirty = true;
    }

    const HISTORY_CAP: usize = 100;

    fn push_prompt_history(&mut self, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        // De-dupe consecutive identical entries.
        if self.prompt_history.last().map(|s| s.as_str()) == Some(t) {
            return;
        }
        self.prompt_history.push(t.to_string());
        if self.prompt_history.len() > Self::HISTORY_CAP {
            let drop_n = self.prompt_history.len() - Self::HISTORY_CAP;
            self.prompt_history.drain(0..drop_n);
        }
    }

    /// ↑ on empty composer: step backward through prompt history.
    pub fn history_older(&mut self) {
        if self.prompt_history.is_empty() {
            return;
        }
        let next = match self.history_browse {
            None => self.prompt_history.len().saturating_sub(1),
            Some(0) => 0,
            Some(i) => i.saturating_sub(1),
        };
        self.history_browse = Some(next);
        self.input = self.prompt_history[next].clone();
        self.cursor = self.input.len();
        self.dirty = true;
    }

    /// ↓ while browsing history: step forward; past newest clears the draft.
    pub fn history_newer(&mut self) {
        let Some(i) = self.history_browse else {
            return;
        };
        if i + 1 >= self.prompt_history.len() {
            self.history_browse = None;
            self.input.clear();
            self.cursor = 0;
        } else {
            self.history_browse = Some(i + 1);
            self.input = self.prompt_history[i + 1].clone();
            self.cursor = self.input.len();
        }
        self.dirty = true;
    }

    /// Leave history browse when the user edits the draft in place.
    pub fn history_note_edit(&mut self) {
        self.history_browse = None;
    }

    /// Toggle full body for the selected transcript item (tools / thinking).
    pub fn toggle_expand_selected(&mut self) {
        if self.selected_item.is_none() {
            if let Some(i) = self.last_foldable_index() {
                self.selected_item = Some(i);
            } else {
                self.status_message = "nothing to expand".into();
                self.dirty = true;
                return;
            }
        }
        let idx = self.selected_item.expect("set above");
        if !self.item_is_foldable(idx) {
            self.status_message = "selected block has no fold body".into();
            self.dirty = true;
            return;
        }
        if self.expanded.contains(&idx) {
            self.expanded.remove(&idx);
            self.status_message = "collapsed".into();
        } else {
            self.expanded.insert(idx);
            self.status_message = "expanded".into();
        }
        self.dirty = true;
    }

    /// Expand or collapse every thinking block.
    pub fn toggle_expand_all_thinking(&mut self) {
        let thinking: Vec<usize> = self
            .transcript
            .iter()
            .enumerate()
            .filter_map(|(i, t)| matches!(t, TranscriptItem::Thinking(_)).then_some(i))
            .collect();
        if thinking.is_empty() {
            self.status_message = "no thinking blocks".into();
            self.dirty = true;
            return;
        }
        let all_open = thinking.iter().all(|i| self.expanded.contains(i));
        if all_open {
            for i in thinking {
                self.expanded.remove(&i);
            }
            self.status_message = "thinking collapsed".into();
        } else {
            for i in thinking {
                self.expanded.insert(i);
            }
            self.status_message = "thinking expanded".into();
        }
        self.dirty = true;
    }

    fn item_is_foldable(&self, idx: usize) -> bool {
        matches!(
            self.transcript.get(idx),
            Some(
                TranscriptItem::Tool {
                    result: Some(r),
                    ..
                }
            ) if !r.is_empty()
        ) || matches!(self.transcript.get(idx), Some(TranscriptItem::Thinking(t)) if !t.is_empty())
            || matches!(self.transcript.get(idx), Some(TranscriptItem::Assistant(t)) if t.lines().count() > 12)
    }

    fn last_foldable_index(&self) -> Option<usize> {
        (0..self.transcript.len())
            .rev()
            .find(|&i| self.item_is_foldable(i))
    }

    /// Shift+Left: previous user turn — select + scroll into view.
    pub fn jump_prev_user_turn(&mut self) {
        let cur = self.selected_item.unwrap_or(self.transcript.len());
        let prev = (0..cur.min(self.transcript.len()))
            .rev()
            .find(|&i| matches!(self.transcript.get(i), Some(TranscriptItem::User(_))));
        if let Some(i) = prev {
            self.selected_item = Some(i);
            self.scroll_to_item(i);
            self.dirty = true;
        }
    }

    /// Shift+Right: next user turn.
    pub fn jump_next_user_turn(&mut self) {
        let start = self.selected_item.map(|i| i + 1).unwrap_or(0);
        let next = (start..self.transcript.len())
            .find(|&i| matches!(self.transcript.get(i), Some(TranscriptItem::User(_))));
        if let Some(i) = next {
            self.selected_item = Some(i);
            self.scroll_to_item(i);
            self.dirty = true;
        }
    }

    /// Select previous/next transcript item (empty composer only).
    pub fn select_prev_item(&mut self) {
        let cur = self.selected_item.unwrap_or(self.transcript.len());
        if cur == 0 {
            return;
        }
        let i = cur.saturating_sub(1);
        self.selected_item = Some(i);
        self.scroll_to_item(i);
        self.dirty = true;
    }

    pub fn select_next_item(&mut self) {
        let start = self.selected_item.map(|i| i + 1).unwrap_or(0);
        if start >= self.transcript.len() {
            return;
        }
        self.selected_item = Some(start);
        self.scroll_to_item(start);
        self.dirty = true;
    }

    /// Scroll so `item` is near the top of the Free viewport.
    fn scroll_to_item(&mut self, item: usize) {
        // Layout may be stale; best-effort using last sync's block map.
        let display = super::toolcard::plan_display(&self.transcript);
        let Some(d_idx) = display.iter().position(|d| match d {
            super::toolcard::Display::Single(i) => *i == item,
            super::toolcard::Display::Group(idxs) => idxs.contains(&item),
        }) else {
            return;
        };
        let line = self.layout.block_start_line(d_idx);
        let total = self.layout.total_lines().max(1);
        let h = self.viewport_h.max(1);
        // Free scroll anchored so the block is visible.
        self.scroll = super::scroll::ScrollState::Free {
            top_line: line.min(total.saturating_sub(h)),
        };
    }

    /// Dispatch the head of the queue as the next turn, if idle and non-empty.
    /// Called by the run loop when a turn finishes successfully (plan §M5:
    /// `queue.auto_send`, default on).
    pub fn dispatch_queue_head(&mut self) {
        if self.phase != Phase::Idle || self.pending_submit.is_some() {
            return;
        }
        if let Some(text) = self.queue.pop_front() {
            self.enqueue_turn(text);
        }
    }

    /// Interject / send-now: cancel the live turn (if any) and send the
    /// composer text — or the head of the queue when the composer is empty.
    /// Idle with text behaves like a normal submit.
    pub fn interject(&mut self) {
        let text = if !self.input.trim().is_empty() {
            let t = std::mem::take(&mut self.input);
            self.cursor = 0;
            Some(t)
        } else {
            self.queue.pop_front()
        };
        let Some(text) = text else {
            self.status_message = "nothing to send now".into();
            self.dirty = true;
            return;
        };
        if self.turn_live || self.phase == Phase::Streaming {
            // Stage the next turn, then ask the run loop to cancel the current.
            // Skill expansion / transcript row happen in enqueue_turn; keep
            // phase Streaming so cancel still sees a live turn.
            self.transcript.push(TranscriptItem::System(
                "interject — cancelling turn to send now…".into(),
            ));
            // enqueue_turn clears input (already empty) and sets pending_submit.
            self.enqueue_turn(text);
            self.request_cancel();
        } else {
            self.enqueue_turn(text);
        }
        self.dirty = true;
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

    /// Ask the run loop to cancel the in-flight turn (if any). Always sets
    /// the flag — the loop no-ops when there is no turn handle — so a phase
    /// desync (e.g. TurnComplete flipped Idle early) cannot swallow Ctrl+C.
    pub fn request_cancel(&mut self) {
        self.cancel_requested = true;
        self.status_message = "cancelling…".into();
        self.dirty = true;
    }

    /// Clear the prompt editor (idle interrupt with non-empty input).
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
        report.push_str("  clipboard routes:\n");
        for line in crate::clipboard::describe_routes() {
            report.push_str(&format!("    {line}\n"));
        }
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

    /// `/copy` — last assistant transcript text → clipboard cascade.
    pub fn copy_last_assistant(&mut self) {
        let text = self.transcript.iter().rev().find_map(|item| match item {
            TranscriptItem::Assistant(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        });
        let Some(text) = text else {
            self.report_copy_err("no assistant message in the transcript yet");
            return;
        };
        self.copy_text_report(&text, "assistant");
    }

    /// `y` — copy selected block body (or last assistant if none selected).
    pub fn copy_selected_content(&mut self) {
        let text = if let Some(idx) = self.selected_item {
            self.block_copy_text(idx, false)
        } else {
            self.transcript.iter().rev().find_map(|item| match item {
                TranscriptItem::Assistant(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
        };
        let Some(text) = text else {
            self.report_copy_err("nothing to copy — select a block (←/→) or wait for a reply");
            return;
        };
        self.copy_text_report(&text, "block");
    }

    /// `Y` — copy selected block metadata (tool name/detail, user prompt, …).
    pub fn copy_selected_meta(&mut self) {
        let Some(idx) = self.selected_item else {
            self.report_copy_err("select a block first (←/→ when composer empty)");
            return;
        };
        let Some(text) = self.block_copy_text(idx, true) else {
            self.report_copy_err("selected block has no metadata");
            return;
        };
        self.copy_text_report(&text, "meta");
    }

    fn block_copy_text(&self, idx: usize, meta_only: bool) -> Option<String> {
        match self.transcript.get(idx)? {
            TranscriptItem::User(s) => Some(s.clone()),
            TranscriptItem::Assistant(s) => Some(s.clone()),
            TranscriptItem::Thinking(s) => Some(s.clone()),
            TranscriptItem::System(s) | TranscriptItem::Error(s) | TranscriptItem::Warning(s) => {
                Some(s.clone())
            }
            TranscriptItem::Tool {
                name,
                detail,
                result,
                ..
            } => {
                if meta_only {
                    Some(format!("{name} · {detail}"))
                } else {
                    let body = result.as_deref().unwrap_or("");
                    if body.is_empty() {
                        Some(format!("{name} · {detail}"))
                    } else {
                        Some(format!("{name} · {detail}\n{body}"))
                    }
                }
            }
        }
    }

    fn copy_text_report(&mut self, text: &str, label: &str) {
        match crate::clipboard::copy_text(text) {
            Ok(result) => {
                let msg = format!(
                    "copied {label} ({} bytes) via {}",
                    text.len(),
                    result.summary()
                );
                self.status_message = msg.clone();
                self.transcript.push(TranscriptItem::System(msg));
            }
            Err(e) => self.report_copy_err(&e),
        }
        self.dirty = true;
    }

    fn report_copy_err(&mut self, e: &str) {
        let msg = format!("copy: {e}");
        self.status_message = msg.clone();
        self.transcript.push(TranscriptItem::System(msg));
        self.dirty = true;
    }

    /// Toggle the full queue pane (Ctrl+; / `/queue`).
    pub fn toggle_queue_pane(&mut self) {
        self.show_queue_pane = !self.show_queue_pane;
        if self.show_queue_pane && self.queue_selected >= self.queue.len() {
            self.queue_selected = self.queue.len().saturating_sub(1);
        }
        self.status_message = if self.show_queue_pane {
            format!("queue pane on · {} item(s)", self.queue.len())
        } else {
            "queue pane off".into()
        };
        self.dirty = true;
    }

    pub fn queue_select_prev(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        self.queue_selected = self.queue_selected.saturating_sub(1);
        self.dirty = true;
    }

    pub fn queue_select_next(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let max = self.queue.len().saturating_sub(1);
        self.queue_selected = (self.queue_selected + 1).min(max);
        self.dirty = true;
    }

    /// Send the selected queue row now (cancel live turn if needed).
    pub fn queue_send_selected(&mut self) {
        if self.queue.is_empty() {
            self.status_message = "queue empty".into();
            self.dirty = true;
            return;
        }
        let idx = self.queue_selected.min(self.queue.len() - 1);
        let text = self.queue.remove(idx).unwrap_or_default();
        self.queue_selected = idx.min(self.queue.len().saturating_sub(1));
        if self.queue.is_empty() {
            self.show_queue_pane = false;
        }
        if self.turn_live || self.phase == Phase::Streaming {
            self.transcript.push(TranscriptItem::System(
                "queue send-now — cancelling turn…".into(),
            ));
            self.enqueue_turn(text);
            self.request_cancel();
        } else {
            self.enqueue_turn(text);
        }
        self.dirty = true;
    }

    /// Delete the selected queue row.
    pub fn queue_delete_selected(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let idx = self.queue_selected.min(self.queue.len() - 1);
        self.queue.remove(idx);
        self.queue_selected = idx.min(self.queue.len().saturating_sub(1));
        self.status_message = format!("{} queued", self.queue.len());
        if self.queue.is_empty() {
            self.show_queue_pane = false;
        }
        self.dirty = true;
    }

    pub fn mark_turn_idle(&mut self) {
        // Any text still buffered when the turn ends must land now.
        self.flush_stream();
        self.turn_live = false;
        // Same rule as TurnComplete: a pending modal (plan approval after a
        // finished plan turn) keeps the Permission phase so it stays visible
        // and answerable; answering it advances to Idle.
        self.phase = if self.modals.is_empty() {
            Phase::Idle
        } else {
            Phase::Permission
        };
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
    use agent_code_lib::tools::PermissionResponse;

    // Build a transcript tall enough to scroll, then prime layout metrics as
    // the draw would (one System block = one wrapped line each here).
    fn app_with_lines(n: usize, viewport_h: usize) -> App {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.clear();
        for i in 0..n {
            app.transcript
                .push(TranscriptItem::System(format!("line {i}")));
        }
        app.layout
            .sync(&app.transcript, 80, &std::collections::HashSet::new(), None);
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
        app.layout
            .sync(&app.transcript, 80, &std::collections::HashSet::new(), None);
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
    fn try_expand_skill_slash_expands_bundled_user_invocable() {
        // Bundled skills load without a project dir.
        let expanded = try_expand_skill_slash("/verify", "/tmp", false);
        assert!(
            expanded.is_some(),
            "bundled /verify skill should expand in modern TUI"
        );
        let body = expanded.unwrap();
        assert!(
            !body.starts_with('/'),
            "expanded body should be the skill prompt, not the slash"
        );
        assert!(body.len() > 20, "skill body should be non-trivial");
    }

    #[test]
    fn try_expand_skill_slash_ignores_plain_text_and_local_commands() {
        assert!(try_expand_skill_slash("hello", "/tmp", false).is_none());
        assert!(try_expand_skill_slash("/help", "/tmp", false).is_none());
        assert!(try_expand_skill_slash("/clear", "/tmp", false).is_none());
        assert!(try_expand_skill_slash("/model", "/tmp", false).is_none());
        assert!(try_expand_skill_slash("/model grok-4", "/tmp", false).is_none());
        assert!(try_expand_skill_slash("/not-a-real-skill-xyz", "/tmp", false).is_none());
    }

    #[test]
    fn try_expand_skill_slash_strips_shell_when_policy_on() {
        // Project skill with a fenced bash block must lose the shell body
        // when disable_skill_shell is true (Codex review #419).
        let dir = tempfile::tempdir().expect("tempdir");
        let skills = dir.path().join(".agent").join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("shell-skill.md"),
            "---\ndescription: test skill with shell\nwhenToUse: tests only\nuserInvocable: true\n---\nRun:\n```bash\necho SECRET_SHELL_CMD\n```\nDone.\n",
        )
        .unwrap();
        let cwd = dir.path().to_str().unwrap();
        let allowed = try_expand_skill_slash("/shell-skill", cwd, false).expect("expand");
        assert!(
            allowed.contains("SECRET_SHELL_CMD"),
            "shell kept when policy off: {allowed}"
        );
        let stripped = try_expand_skill_slash("/shell-skill", cwd, true).expect("expand");
        assert!(
            !stripped.contains("SECRET_SHELL_CMD"),
            "shell must be stripped when policy on: {stripped}"
        );
        assert!(
            stripped.contains("Shell execution disabled"),
            "policy notice missing: {stripped}"
        );
    }

    #[test]
    fn parse_model_slash_show_and_set() {
        assert_eq!(parse_model_slash("/model"), Some(PendingModelAction::Show));
        assert_eq!(
            parse_model_slash("/model  "),
            Some(PendingModelAction::Show)
        );
        assert_eq!(
            parse_model_slash("/model grok-4.5"),
            Some(PendingModelAction::Set("grok-4.5".into()))
        );
        assert_eq!(
            parse_model_slash("/MODEL gpt-5.4"),
            Some(PendingModelAction::Set("gpt-5.4".into()))
        );
        assert!(parse_model_slash("/help").is_none());
        assert!(parse_model_slash("model").is_none());
        assert!(parse_model_slash("hello").is_none());
    }

    #[test]
    fn insert_str_at_cursor_and_normalizes_crlf() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "ab".into();
        app.cursor = 1;
        app.insert_str("X\r\nY\rZ");
        assert_eq!(app.input, "aX\nY\nZb");
        assert_eq!(app.cursor, 1 + "X\nY\nZ".len());
    }

    #[test]
    fn insert_str_works_while_streaming() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.insert_str("queued paste");
        assert_eq!(app.input, "queued paste");
    }

    #[test]
    fn insert_str_ignored_during_permission_modal() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        app.insert_str("nope");
        assert!(app.input.is_empty());
    }

    #[test]
    fn submit_model_show_sets_pending_not_turn() {
        let mut app = App::new("grok-4", "/tmp", "s");
        app.input = "/model".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(app.pending_model, Some(PendingModelAction::Show));
        assert!(app.pending_submit.is_none());
        assert!(app.input.is_empty());
        assert_eq!(app.phase, Phase::Idle);
    }

    #[test]
    fn submit_model_set_sets_pending_even_while_streaming() {
        let mut app = App::new("grok-4", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.input = "/model grok-4.5".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(
            app.pending_model,
            Some(PendingModelAction::Set("grok-4.5".into()))
        );
        assert!(
            app.queue.is_empty(),
            "/model must not be queued as a prompt"
        );
        assert!(app.pending_submit.is_none());
    }

    #[test]
    fn apply_model_action_set_updates_badge_and_transcript() {
        let mut app = App::new("old-model", "/tmp", "s");
        let mut engine_model = "old-model".to_string();
        app.apply_model_action(
            PendingModelAction::Set("new-model".into()),
            "old-model",
            "",
            |name| engine_model = name,
        );
        assert_eq!(engine_model, "new-model");
        assert_eq!(app.model, "new-model");
        assert!(app.pending_model.is_none());
        match app.transcript.last() {
            Some(TranscriptItem::System(s)) => assert!(s.contains("new-model")),
            other => panic!("expected System line, got {other:?}"),
        }
    }

    #[test]
    fn apply_model_action_show_lists_catalog() {
        let mut app = App::new("grok-4", "/tmp", "s");
        let before = app.transcript.len();
        app.apply_model_action(
            PendingModelAction::Show,
            "grok-4",
            "https://api.x.ai/v1",
            |_| {},
        );
        assert!(app.transcript.len() > before);
        let joined: String = app
            .transcript
            .iter()
            .filter_map(|i| match i {
                TranscriptItem::System(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Model: grok-4"));
        assert!(
            joined.contains("/model"),
            "catalog should mention how to switch"
        );
    }

    #[test]
    fn format_model_catalog_marks_current() {
        let lines = format_model_catalog("grok-4", "https://api.x.ai/v1");
        let joined = lines.join("\n");
        assert!(joined.contains("grok-4 ✔") || joined.contains("Model: grok-4"));
        assert!(joined.contains("Use /model") || joined.contains("Available models"));
    }

    #[test]
    fn submit_skill_slash_queues_expanded_prompt() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "/verify".into();
        app.cursor = app.input.len();
        app.submit();
        match app.transcript.last() {
            Some(TranscriptItem::User(s)) => assert_eq!(s, "/verify"),
            other => panic!("expected User(/verify), got {other:?}"),
        }
        let pending = app.pending_submit.expect("skill should produce a turn");
        assert!(
            !pending.starts_with('/'),
            "engine must receive the expanded skill body, got: {pending}"
        );
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
    fn deltas_buffer_without_redraw_until_flush() {
        // Buffered text isn't rendered until it lands in the transcript, so
        // a delta must NOT request a repaint — dirty-per-delta drew identical
        // frames at token rate and defeated the ≤10 fps coalescer budget.
        // The flush (tick or barrier event) is what repaints.
        let mut app = App::new("m", "/tmp", "s");
        app.dirty = false;
        let before = app.transcript.len();
        app.apply_engine(EngineEvent::Text("x".into()));
        assert!(!app.dirty, "a buffered delta must not request a redraw");
        assert_eq!(
            app.transcript.len(),
            before,
            "delta is buffered, not applied"
        );
        app.flush_stream();
        assert!(app.dirty, "the flush requests the redraw");
        assert_eq!(app.transcript.len(), before + 1);
    }

    #[test]
    fn plan_modal_survives_turn_complete() {
        // PlanProposed is fire-and-forget and typically arrives right before
        // TurnComplete; flipping to Idle used to orphan the modal (invisible,
        // unanswerable, blocking later modals).
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.apply_engine(EngineEvent::PlanProposed {
            plan_md: "# plan".into(),
            path: None,
        });
        app.apply_engine(EngineEvent::TurnComplete(1));
        assert_eq!(app.phase, Phase::Permission, "modal keeps Permission");
        assert!(app.front_modal().is_some(), "plan modal still answerable");
        // The turn is gone; approving must land on Idle, not a phantom
        // Streaming spinner.
        app.turn_live = false;
        app.resolve_plan(true, false);
        assert_eq!(app.phase, Phase::Idle);
    }

    #[test]
    fn empty_enter_dispatches_queue_when_idle() {
        let mut app = App::new("m", "/tmp", "s");
        app.queue.push_back("queued one".into());
        app.input.clear();
        app.cursor = 0;
        app.submit();
        assert_eq!(app.pending_submit.as_deref(), Some("queued one"));
        assert!(app.queue.is_empty());
    }

    #[test]
    fn interject_mid_turn_stages_prompt_and_cancels() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.turn_live = true;
        app.input = "stop and do this".into();
        app.cursor = app.input.len();
        app.interject();
        assert!(app.cancel_requested, "interject must cancel the live turn");
        assert_eq!(app.pending_submit.as_deref(), Some("stop and do this"));
        assert!(app.input.is_empty());
        assert!(
            app.transcript
                .iter()
                .any(|t| matches!(t, TranscriptItem::System(s) if s.contains("interject"))),
            "status line for interject"
        );
    }

    #[test]
    fn interject_empty_composer_sends_queue_head() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.turn_live = true;
        app.queue.push_back("from queue".into());
        app.interject();
        assert!(app.cancel_requested);
        assert_eq!(app.pending_submit.as_deref(), Some("from queue"));
        assert!(app.queue.is_empty());
    }

    #[test]
    fn interject_idle_with_text_is_normal_submit() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "hello".into();
        app.cursor = 5;
        app.interject();
        assert!(!app.cancel_requested);
        assert_eq!(app.pending_submit.as_deref(), Some("hello"));
    }

    #[test]
    fn insert_newline_and_line_col_tracking() {
        let mut app = App::new("m", "/tmp", "s");
        app.insert_str("ab");
        app.insert_newline();
        app.insert_str("cd");
        assert_eq!(app.input, "ab\ncd");
        assert_eq!(app.input_line_count(), 2);
        assert_eq!(app.cursor_line_col(), (1, 2));
        app.move_up_line();
        assert_eq!(app.cursor_line_col(), (0, 2));
        app.move_line_start();
        assert_eq!(app.cursor_line_col(), (0, 0));
        app.move_line_end();
        assert_eq!(app.cursor_line_col(), (0, 2));
    }

    #[test]
    fn multiline_mode_toggle_flips_flag() {
        let mut app = App::new("m", "/tmp", "s");
        assert!(!app.multiline_mode);
        app.toggle_multiline_mode();
        assert!(app.multiline_mode);
        app.toggle_multiline_mode();
        assert!(!app.multiline_mode);
    }

    #[test]
    fn prompt_history_up_down() {
        let mut app = App::new("m", "/tmp", "s");
        app.enqueue_turn("first".into());
        app.pending_submit = None;
        app.phase = Phase::Idle;
        app.enqueue_turn("second".into());
        app.pending_submit = None;
        app.phase = Phase::Idle;
        app.input.clear();
        app.history_older();
        assert_eq!(app.input, "second");
        app.history_older();
        assert_eq!(app.input, "first");
        app.history_newer();
        assert_eq!(app.input, "second");
        app.history_newer();
        assert!(app.input.is_empty());
        assert!(app.history_browse.is_none());
    }

    #[test]
    fn expand_toggles_tool_body() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::Tool {
            call_id: "1".into(),
            name: "Bash".into(),
            detail: "ls".into(),
            result: Some("a\nb\nc\nd\ne".into()),
            is_error: false,
        });
        let idx = app.transcript.len() - 1;
        app.selected_item = Some(idx);
        assert!(!app.expanded.contains(&idx));
        app.toggle_expand_selected();
        assert!(app.expanded.contains(&idx));
        app.toggle_expand_selected();
        assert!(!app.expanded.contains(&idx));
    }

    #[test]
    fn block_copy_text_tool_meta_and_body() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::Tool {
            call_id: "1".into(),
            name: "Bash".into(),
            detail: "ls -la".into(),
            result: Some("file.rs\n".into()),
            is_error: false,
        });
        let idx = app.transcript.len() - 1;
        app.selected_item = Some(idx);
        assert_eq!(
            app.block_copy_text(idx, true).as_deref(),
            Some("Bash · ls -la")
        );
        let body = app.block_copy_text(idx, false).unwrap();
        assert!(body.contains("ls -la"));
        assert!(body.contains("file.rs"));
    }

    #[test]
    fn queue_pane_send_selected_idle() {
        let mut app = App::new("m", "/tmp", "s");
        app.queue.push_back("a".into());
        app.queue.push_back("b".into());
        app.show_queue_pane = true;
        app.queue_selected = 1;
        app.queue_send_selected();
        assert_eq!(app.pending_submit.as_deref(), Some("b"));
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.queue.front().map(|s| s.as_str()), Some("a"));
    }

    #[test]
    fn cost_slash_reports_usage() {
        let mut app = App::new("m", "/tmp", "s");
        app.tokens_in = 10;
        app.tokens_out = 5;
        app.cost_usd = 0.01;
        app.input = "/cost".into();
        app.cursor = 5;
        app.submit();
        assert!(app.transcript.iter().any(
            |t| matches!(t, TranscriptItem::System(s) if s.contains("usage:") && s.contains("15"))
        ),);
    }

    #[test]
    fn jump_user_turns() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::User("one".into()));
        app.transcript.push(TranscriptItem::Assistant("a1".into()));
        app.transcript.push(TranscriptItem::User("two".into()));
        app.jump_prev_user_turn();
        assert_eq!(app.selected_item, Some(app.transcript.len() - 1));
        app.jump_prev_user_turn();
        let first_user = app
            .transcript
            .iter()
            .position(|t| matches!(t, TranscriptItem::User(s) if s == "one"));
        assert_eq!(app.selected_item, first_user);
        app.jump_next_user_turn();
        let second = app
            .transcript
            .iter()
            .position(|t| matches!(t, TranscriptItem::User(s) if s == "two"));
        assert_eq!(app.selected_item, second);
    }

    #[test]
    fn unknown_slash_rejected_with_hint_not_sent() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "/definitely-not-a-command".into();
        app.cursor = app.input.len();
        app.submit();
        assert!(app.pending_submit.is_none(), "must not become a model turn");
        assert!(
            app.transcript
                .iter()
                .any(|t| matches!(t, TranscriptItem::System(s) if s.contains("unknown command"))),
            "hint shown: {:?}",
            app.transcript
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn clear_requests_engine_conversation_clear() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "/clear".into();
        app.cursor = 6;
        app.submit();
        assert!(app.pending_clear, "engine clear deferred to run loop");
        assert!(app.transcript.is_empty());
    }

    #[test]
    fn tool_result_keeps_full_multiline_content() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::ToolStart {
            call_id: "c1".into(),
            name: "Bash".into(),
            detail: "cargo test".into(),
        });
        app.apply_engine(EngineEvent::ToolResult {
            call_id: "c1".into(),
            name: "Bash".into(),
            content: "error[E0308]: mismatched types\n --> src/main.rs:1\nnote: expected u32"
                .into(),
            is_error: true,
        });
        let full = app.transcript.iter().find_map(|t| match t {
            TranscriptItem::Tool {
                result: Some(r), ..
            } => Some(r.clone()),
            _ => None,
        });
        let full = full.expect("tool card has a result");
        assert!(
            full.contains("note: expected u32"),
            "full output retained (was first-line-only): {full}"
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
            call_id: String::new(),
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
        // Legacy id-less fallback: with empty call_ids the result attaches to
        // the oldest still-pending card.
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::ToolStart {
            call_id: String::new(),
            name: "Bash".into(),
            detail: "first".into(),
        });
        app.apply_engine(EngineEvent::ToolStart {
            call_id: String::new(),
            name: "Grep".into(),
            detail: "second".into(),
        });
        app.apply_engine(EngineEvent::ToolResult {
            call_id: String::new(),
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
    fn tool_results_correlate_by_call_id_out_of_order() {
        // Parallel tool calls can complete in any order; the result must
        // attach to the card with the matching call_id, not the oldest.
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::ToolStart {
            call_id: "call_a".into(),
            name: "Bash".into(),
            detail: "first".into(),
        });
        app.apply_engine(EngineEvent::ToolStart {
            call_id: "call_b".into(),
            name: "Grep".into(),
            detail: "second".into(),
        });
        // The SECOND call returns first.
        app.apply_engine(EngineEvent::ToolResult {
            call_id: "call_b".into(),
            name: "Grep".into(),
            content: "second-out".into(),
            is_error: false,
        });
        let n = app.transcript.len();
        match &app.transcript[n - 2] {
            TranscriptItem::Tool { detail, result, .. } => {
                assert_eq!(detail, "first");
                assert!(result.is_none(), "call_a must stay pending");
            }
            other => panic!("{other:?}"),
        }
        match &app.transcript[n - 1] {
            TranscriptItem::Tool { detail, result, .. } => {
                assert_eq!(detail, "second");
                assert_eq!(result.as_deref(), Some("second-out"));
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
        app.layout
            .sync(&app.transcript, 80, &std::collections::HashSet::new(), None);
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
        assert!(
            last.contains("clipboard routes:"),
            "terminal-setup must report clipboard cascade"
        );
        assert!(last.contains("native candidates"));
        // tmux + no-truecolor + no-kitty → remediation lines present.
        assert!(last.contains("allow-passthrough"));
        assert!(last.contains("COLORTERM=truecolor"));
    }

    #[test]
    fn copy_reports_no_assistant_when_empty() {
        let mut app = App::new("m", "/tmp", "s");
        app.copy_last_assistant();
        assert!(
            app.transcript
                .iter()
                .any(|t| matches!(t, TranscriptItem::System(s) if s.contains("no assistant"))),
            "empty transcript should say nothing to copy"
        );
    }

    #[test]
    fn copy_slash_clears_input_and_targets_last_assistant() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript
            .push(TranscriptItem::Assistant("hello from agent".into()));
        app.input = "/copy".into();
        app.cursor = app.input.len();
        app.submit();
        assert!(app.input.is_empty());
        assert!(
            app.transcript.iter().any(|t| matches!(
                t,
                TranscriptItem::System(s) if s.contains("copied") || s.contains("copy failed")
            )),
            "copy should report success or failure"
        );
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
            call_id: "c1".into(),
            name: "Bash".into(),
            detail: "cargo test".into(),
        });
        assert_eq!(app.waiting_on, WaitingOn::Tool("Bash".into()));
        app.apply_engine(EngineEvent::ToolResult {
            call_id: "c1".into(),
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
            origin: None,
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
        // These model a MID-TURN modal; the run loop sets this at spawn.
        app.turn_live = true;
        app.apply_engine(EngineEvent::TurnStart(1));
        let (respond, rx) = std::sync::mpsc::channel();
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "Bash".into(),
            description: "Bash: run `cargo test`".into(),
            origin: Some("subagent-3".into()),
            input_preview: Some("{\"command\": \"cargo test\"}".into()),
            respond,
        });
        assert_eq!(app.phase, Phase::Permission);
        assert_eq!(
            app.front_permission().and_then(|p| p.origin.as_deref()),
            Some("subagent-3"),
            "origin is kept typed on the pending permission"
        );

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
        // These model a MID-TURN modal; the run loop sets this at spawn.
        app.turn_live = true;
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
        // These model a MID-TURN modal; the run loop sets this at spawn.
        app.turn_live = true;
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
        // These model a MID-TURN modal; the run loop sets this at spawn.
        app.turn_live = true;
        app.apply_engine(EngineEvent::TurnStart(1));
        let (r1, rx1) = std::sync::mpsc::channel();
        let (r2, rx2) = std::sync::mpsc::channel();
        // Lead + background subagent ask at the same time.
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "Bash".into(),
            description: "lead".into(),
            origin: None,
            input_preview: None,
            respond: r1,
        });
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "FileWrite".into(),
            description: "subagent".into(),
            origin: Some("subagent-1".into()),
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
                origin: None,
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
