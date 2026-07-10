//! Application state for the modern TUI.
//!
//! Pure data + reducers. Drawing lives in [`super::render`]; I/O in
//! [`super::run`]. This split keeps visual tests free of a live terminal.

use super::mode::SessionMode;
use super::sink::EngineEvent;

/// One row in the scrollable transcript.
#[derive(Debug, Clone)]
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

    pub transcript: Vec<TranscriptItem>,
    /// Scroll offset from the bottom (0 = stick to latest).
    pub scroll_offset: u16,

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
    /// When true, runtime should cancel the active turn.
    pub cancel_requested: bool,

    /// Spinner frame index while streaming.
    pub tick: u64,
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
            transcript: vec![TranscriptItem::System(
                "Modern TUI · Shift+Tab cycle mode · Esc cancel · Ctrl+C quit · Enter send".into(),
            )],
            scroll_offset: 0,
            input: String::new(),
            cursor: 0,
            turn_count: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            status_message: String::new(),
            should_quit: false,
            pending_submit: None,
            cancel_requested: false,
            tick: 0,
        }
    }

    pub fn apply_engine(&mut self, ev: EngineEvent) {
        match ev {
            EngineEvent::Text(t) => self.push_or_append_assistant(&t),
            EngineEvent::Thinking(t) => {
                if let Some(TranscriptItem::Thinking(buf)) = self.transcript.last_mut() {
                    buf.push_str(&t);
                } else {
                    self.transcript.push(TranscriptItem::Thinking(t));
                }
            }
            EngineEvent::ToolStart { name, detail } => {
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
                if let Some(TranscriptItem::Tool {
                    result,
                    is_error: err,
                    ..
                }) = self
                    .transcript
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, TranscriptItem::Tool { result: None, .. }))
                {
                    *result = Some(content.lines().next().unwrap_or("").to_string());
                    *err = is_error;
                }
            }
            EngineEvent::TurnStart(n) => {
                self.phase = Phase::Streaming;
                self.turn_count = n;
                self.status_message = format!("turn {n}");
            }
            EngineEvent::TurnComplete(n) => {
                self.phase = Phase::Idle;
                self.turn_count = n;
                self.status_message = format!("turn {n} done");
            }
            EngineEvent::Error(e) => {
                self.phase = Phase::Idle;
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
                "Keys: Enter send · Shift+Tab mode · Esc cancel turn · Ctrl+C quit · /clear /exit"
                    .into(),
            ));
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if self.phase == Phase::Streaming {
            self.status_message = "turn in progress — cancel with Esc first".into();
            return;
        }
        self.transcript.push(TranscriptItem::User(text.clone()));
        self.input.clear();
        self.cursor = 0;
        self.pending_submit = Some(text);
        self.phase = Phase::Streaming;
        self.scroll_offset = 0;
    }

    pub fn cycle_mode_forward(&mut self) {
        self.mode = self.mode.cycle_next();
        self.status_message = format!("mode → {}", self.mode.label());
        if self.mode == SessionMode::Plan {
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

    pub fn mark_turn_idle(&mut self) {
        self.phase = Phase::Idle;
        self.cancel_requested = false;
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn engine_text_appends() {
        let mut app = App::new("m", "/tmp", "s");
        app.apply_engine(EngineEvent::Text("Hel".into()));
        app.apply_engine(EngineEvent::Text("lo".into()));
        match app.transcript.last() {
            Some(TranscriptItem::Assistant(t)) => assert_eq!(t, "Hello"),
            other => panic!("{other:?}"),
        }
    }
}
