//! In-TUI session picker (`/resume`).
//!
//! `/resume <id>` already worked, but bare `/resume` printed a usage
//! line while the command's own description advertised "Interactively
//! pick a recent session to resume". This is that picker.
//!
//! Picking a session does two things, and the second is the one that
//! matters: it loads the messages into the engine *and* rebuilds the
//! visible transcript from them. Restoring only the engine would leave
//! the model with full context in front of an empty screen — the user
//! would have no idea what they had resumed.

use agent_code_lib::services::session::SessionSummary;

use super::app::{App, TranscriptItem};
use super::mode::SessionMode;

/// Overlay state for the session picker.
#[derive(Debug, Clone)]
pub struct SessionPicker {
    /// Filter over id, label, cwd and model.
    pub query: String,
    /// Highlighted row into the filtered list.
    pub selected: usize,
    /// Sessions offered, newest first.
    pub entries: Vec<SessionSummary>,
}

impl SessionPicker {
    /// Rows matching `query`, case-insensitively.
    pub fn filtered(&self) -> Vec<(usize, &SessionSummary)> {
        let q = self.query.to_ascii_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                if q.is_empty() {
                    return true;
                }
                let label = s.label.as_deref().unwrap_or("");
                s.id.to_ascii_lowercase().contains(&q)
                    || label.to_ascii_lowercase().contains(&q)
                    || s.cwd.to_ascii_lowercase().contains(&q)
                    || s.model.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    /// Id under the highlight, if the filter matched anything.
    pub fn highlighted_id(&self) -> Option<String> {
        self.filtered()
            .get(self.selected)
            .map(|(_, s)| s.id.clone())
    }
}

/// One display row: what the picker shows for a session.
///
/// Sizes the session by turn count rather than message count: the picker
/// is fed by the cached summary-only listing, which deliberately does not
/// deserialize transcripts, so `message_count` is not populated there.
pub fn summary_line(s: &SessionSummary) -> String {
    let label = s.label.clone().unwrap_or_else(|| {
        // No label: the working directory is the next most recognisable
        // thing about a session.
        std::path::Path::new(&s.cwd)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.cwd.clone())
    });
    let short_id: String = s.id.chars().take(8).collect();
    let turns = s.turn_count;
    let plural = if turns == 1 { "" } else { "s" };
    format!(
        "{short_id}  {label}  ·  {turns} turn{plural}  ·  {}",
        s.updated_at
    )
}

/// Engine-side values a restored session carries, lifted out so the App
/// mirrors can be updated in one place (and tested without an engine).
#[derive(Debug, Clone, Default)]
pub struct RestoredState {
    pub id: String,
    pub model: String,
    pub turn_count: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub plan_mode: bool,
}

/// Rebuild transcript items from a restored conversation.
///
/// Tool calls are matched to their results by `tool_use_id` so a resumed
/// session shows the same cards it did live, rather than a wall of raw
/// blocks. Meta messages (tool results, context injection) are skipped:
/// they are conversation plumbing the user never saw the first time.
///
/// `show_thinking` mirrors `App::show_thinking_blocks` so stored
/// reasoning is reconstructed on exactly the same terms it was streamed
/// live — restoring a session must not turn thinking blocks on for a
/// user who has them off, nor drop them for a user who has them on.
pub fn transcript_from_messages(
    messages: &[agent_code_lib::llm::message::Message],
    show_thinking: bool,
) -> Vec<TranscriptItem> {
    use agent_code_lib::llm::message::{ContentBlock, Message};
    use std::collections::HashMap;

    // First pass: collect tool results so a card can be built whole.
    let mut results: HashMap<String, (String, bool)> = HashMap::new();
    for m in messages {
        if let Message::User(u) = m {
            for block in &u.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } = block
                {
                    results.insert(tool_use_id.clone(), (content.clone(), *is_error));
                }
            }
        }
    }

    let mut items = Vec::new();
    for m in messages {
        match m {
            Message::User(u) => {
                let text: String = u
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                // A compacted session keeps its removed history *only*
                // inside this summary. It rides in as a meta user message,
                // so the blanket meta skip below would drop it and the
                // resumed conversation would appear to start mid-thought
                // while the engine still reasons from the hidden text.
                if u.is_compact_summary {
                    if !text.trim().is_empty() {
                        items.push(TranscriptItem::System(format!("compacted context\n{text}")));
                    }
                    continue;
                }
                if u.is_meta {
                    continue;
                }
                if !text.trim().is_empty() {
                    items.push(TranscriptItem::User(text));
                }
            }
            Message::Assistant(a) => {
                for block in &a.content {
                    match block {
                        ContentBlock::Text { text } if !text.trim().is_empty() => {
                            items.push(TranscriptItem::Assistant(text.clone()));
                        }
                        // Reasoning was on screen live via
                        // `EngineEvent::Thinking`; the stored messages
                        // still carry it, so a resumed transcript that
                        // dropped it would be missing content the user
                        // had already seen. No duration is recorded on
                        // disk, hence `None` (renders un-timed).
                        ContentBlock::Thinking { thinking, .. }
                            if show_thinking && !thinking.trim().is_empty() =>
                        {
                            items.push(TranscriptItem::Thinking {
                                text: thinking.clone(),
                                duration_ms: None,
                            });
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let (result, is_error) = results
                                .get(id)
                                .map(|(c, e)| (Some(c.clone()), *e))
                                .unwrap_or((None, false));
                            items.push(TranscriptItem::Tool {
                                call_id: id.clone(),
                                name: name.clone(),
                                detail: tool_detail(input),
                                result,
                                is_error,
                                live: None,
                            });
                        }
                        _ => {}
                    }
                }
            }
            // System messages are engine bookkeeping, not screen content.
            Message::System(_) => {}
        }
    }
    items
}

/// A one-line description of a tool call, matching what the live cards
/// show: the most identifying argument rather than the whole input.
fn tool_detail(input: &serde_json::Value) -> String {
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ] {
        if let Some(v) = input.get(key).and_then(|v| v.as_str()) {
            return v.chars().take(120).collect();
        }
    }
    String::new()
}

impl App {
    /// Open the session picker (`/resume` with no argument).
    pub fn open_session_picker(&mut self, entries: Vec<SessionSummary>) {
        if self.front_modal().is_some() {
            return;
        }
        self.command_palette = None;
        self.show_shortcuts = false;
        self.session_picker = Some(SessionPicker {
            query: String::new(),
            selected: 0,
            entries,
        });
        self.status_message = "resume · type to filter · Enter resume · Esc cancel".into();
        self.dirty = true;
    }

    /// Result of the run loop's off-thread session scan.
    pub fn show_session_picker(&mut self, entries: Vec<SessionSummary>) {
        if entries.is_empty() {
            self.status_message.clear();
            self.transcript
                .push(TranscriptItem::System("no saved sessions found".into()));
            self.dirty = true;
            return;
        }
        // A permission / question modal owns the screen, and
        // `open_session_picker` refuses to draw over one. Dropping the rows
        // here would strand the request: no picker, no retry, and the
        // status stuck on "loading sessions…" forever. Hold them until the
        // modal clears (`retry_session_picker`).
        if self.front_modal().is_some() {
            self.status_message = "session list ready — answer the prompt first".into();
            self.deferred_sessions = Some(entries);
            self.dirty = true;
            return;
        }
        self.open_session_picker(entries);
    }

    /// The visible half of `/clear`.
    ///
    /// Split out because a `/clear` held back for a resume has to run it
    /// *twice*: once when submitted, and again when the deferred engine
    /// clear finally lands — by then the restore has repainted the
    /// screen, and skipping this would leave the restored history on
    /// display in front of an empty conversation.
    pub fn clear_transcript_view(&mut self) {
        self.transcript.clear();
        self.expanded.clear();
        self.selected_item = None;
        self.layout.invalidate();
        self.ctx_meter = None;
        self.dirty = true;
    }

    /// A resume failed, so the session all that deferred work was meant
    /// for never arrived. None of it may run against the conversation the
    /// user was trying to leave — a prompt or `!cmd` would take real tool
    /// and filesystem side effects there — so it is cancelled and shown.
    pub fn cancel_deferred_resume_work(&mut self) {
        let mut cancelled: Vec<String> = Vec::new();
        if self.pending_clear {
            self.pending_clear = false;
            cancelled.push("/clear".into());
        }
        if self.pending_model.take().is_some() {
            cancelled.push("/model".into());
        }
        if let Some(slash) = self.pending_slash.take() {
            cancelled.push(slash);
        }
        if let Some(cmd) = self.pending_shell.take() {
            cancelled.push(format!("!{cmd}"));
        }
        self.reclaim_staged_prompts("not sent — the resume failed:", &mut cancelled);
        if !cancelled.is_empty() {
            let body = cancelled.join("\n");
            self.transcript.push(TranscriptItem::System(format!(
                "cancelled — held for the session that failed to load:\n{body}"
            )));
            self.scroll_to_bottom();
            self.dirty = true;
        }
    }

    /// Take back prompts staged against the conversation being left.
    ///
    /// The composer gets the submitted prompt when it is free; anything
    /// else is appended to `spill` for the caller to display verbatim.
    /// Nothing is reduced to a count — these are the user's own words.
    fn reclaim_staged_prompts(&mut self, header: &str, spill: &mut Vec<String>) {
        let mut carried: Vec<String> = self.queue.drain(..).collect();
        self.queue_selected = 0;
        if let Some(text) = self.pending_submit.take() {
            if self.input.trim().is_empty() {
                self.cursor = text.len();
                self.input = text;
            } else {
                // A draft already occupies the composer and must not be
                // clobbered, so this one is displayed instead.
                carried.insert(0, text);
            }
        }
        if !carried.is_empty() {
            spill.push(format!("{header}\n{}", carried.join("\n")));
        }
    }

    /// Open a picker whose rows arrived while a HITL modal was up.
    /// Called by the run loop once the modal queue drains.
    pub fn retry_session_picker(&mut self) {
        if self.front_modal().is_some() {
            return;
        }
        if let Some(entries) = self.deferred_sessions.take() {
            self.open_session_picker(entries);
        }
    }

    pub fn session_picker_open(&self) -> bool {
        self.session_picker.is_some()
    }

    pub fn close_session_picker(&mut self) {
        if self.session_picker.take().is_some() {
            self.status_message.clear();
            self.dirty = true;
        }
    }

    pub fn session_picker_move(&mut self, delta: i32) {
        let Some(p) = self.session_picker.as_mut() else {
            return;
        };
        let n = p.filtered().len() as i32;
        if n == 0 {
            p.selected = 0;
        } else {
            p.selected = (p.selected as i32 + delta).rem_euclid(n) as usize;
        }
        self.dirty = true;
    }

    pub fn session_picker_insert_char(&mut self, c: char) {
        let Some(p) = self.session_picker.as_mut() else {
            return;
        };
        if c.is_control() {
            return;
        }
        p.query.push(c);
        p.selected = 0;
        self.dirty = true;
    }

    pub fn session_picker_backspace(&mut self) {
        let Some(p) = self.session_picker.as_mut() else {
            return;
        };
        p.query.pop();
        p.selected = 0;
        self.dirty = true;
    }

    /// Accept the highlighted session; the run loop performs the load.
    pub fn session_picker_accept(&mut self) {
        let Some(id) = self
            .session_picker
            .as_ref()
            .and_then(|p| p.highlighted_id())
        else {
            // Nothing matched: treat Enter as a cancel rather than a
            // silent no-op the user cannot distinguish from a hang.
            self.close_session_picker();
            return;
        };
        self.close_session_picker();
        self.pending_resume = Some(id.clone());
        self.status_message = format!("resuming {}…", &id[..id.len().min(8)]);
        self.dirty = true;
    }

    /// Replace the visible transcript with a restored conversation.
    pub fn restore_transcript(&mut self, items: Vec<TranscriptItem>, id: &str, turns: usize) {
        self.transcript = items;
        self.expanded.clear();
        self.selected_item = None;
        self.layout.invalidate();
        self.transcript.push(TranscriptItem::System(format!(
            "resumed session {} · {} turns",
            &id[..id.len().min(8)],
            turns
        )));
        self.scroll_to_bottom();
        self.status_message.clear();
        self.dirty = true;
    }

    /// Point every App mirror at the session just restored.
    ///
    /// The engine is only half the story: the header, the mode badge and
    /// `/cost` all read App's own copies. Leaving them behind makes the
    /// UI describe the session the user just left — a NORMAL badge over a
    /// read-only plan-mode context, the old model name, the old spend.
    ///
    /// Returns the [`SessionMode`] the caller must push into the *live*
    /// engine handles (plan atomic + permission-checker default); App
    /// deliberately holds no engine Arc, so it cannot do that itself.
    pub fn adopt_restored_session(&mut self, s: &RestoredState) -> SessionMode {
        self.session_id = s.id.clone();
        if !s.model.is_empty() {
            self.model = s.model.clone();
        }
        self.turn_count = s.turn_count;
        self.tokens_in = s.tokens_in;
        self.tokens_out = s.tokens_out;
        self.cost_usd = s.cost_usd;
        // Belongs to the conversation that was just replaced; the next
        // turn re-reports it for the restored one.
        self.ctx_meter = None;
        self.mode = if s.plan_mode {
            SessionMode::Plan
        } else {
            SessionMode::Normal
        };

        // Prompts staged against the previous conversation must not be
        // auto-dispatched into the session that replaced it — but they
        // are the user's own words, so none of them is dropped silently.
        // A prompt submitted while the session was still loading never
        // ran (turn spawning is blocked for exactly that window), so it
        // goes back to the composer; anything that cannot is reproduced
        // verbatim, where it can still be read and copied.
        let mut spill = Vec::new();
        self.reclaim_staged_prompts("not sent — written for the previous session:", &mut spill);
        for line in spill {
            self.transcript.push(TranscriptItem::System(line));
            self.scroll_to_bottom();
        }
        self.dirty = true;
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message, UserMessage};
    use agent_code_lib::tools::PermissionResponse;
    use uuid::Uuid;

    fn summary(id: &str, label: Option<&str>, cwd: &str) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            cwd: cwd.to_string(),
            model: "test-model".into(),
            turn_count: 3,
            message_count: 6,
            updated_at: "2026-07-25T10:00:00Z".into(),
            label: label.map(|s| s.to_string()),
            tags: Vec::new(),
        }
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Text { text: text.into() }],
            is_meta: false,
            is_compact_summary: false,
        })
    }

    #[test]
    fn filtering_matches_id_label_cwd_and_model() {
        let picker = SessionPicker {
            query: String::new(),
            selected: 0,
            entries: vec![
                summary("aaa11111", Some("auth work"), "/home/u/api"),
                summary("bbb22222", None, "/home/u/webapp"),
            ],
        };
        for (q, expected) in [
            ("auth", 1),
            ("webapp", 1),
            ("bbb", 1),
            ("test-model", 2),
            ("nothing", 0),
        ] {
            let p = SessionPicker {
                query: q.to_string(),
                ..picker.clone()
            };
            assert_eq!(p.filtered().len(), expected, "query {q}");
        }
    }

    #[test]
    fn accepting_requests_the_highlighted_session() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_session_picker(vec![
            summary("aaa11111", None, "/a"),
            summary("bbb22222", None, "/b"),
        ]);
        app.session_picker_move(1);
        app.session_picker_accept();
        assert!(!app.session_picker_open());
        assert_eq!(app.pending_resume.as_deref(), Some("bbb22222"));
    }

    /// Enter with an empty result set must not resume something the user
    /// never selected.
    #[test]
    fn accepting_with_no_match_cancels() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_session_picker(vec![summary("aaa11111", None, "/a")]);
        for c in "zzzz".chars() {
            app.session_picker_insert_char(c);
        }
        app.session_picker_accept();
        assert!(!app.session_picker_open());
        assert!(
            app.pending_resume.is_none(),
            "resumed an unselected session"
        );
    }

    /// The point of the feature: a resumed session has to be *visible*,
    /// not just present in the engine.
    #[test]
    fn a_conversation_rebuilds_into_transcript_items() {
        let messages = vec![
            user("add a null check"),
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![
                    ContentBlock::Text {
                        text: "On it.".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "FileEdit".into(),
                        input: serde_json::json!({"file_path": "src/auth.rs"}),
                    },
                ],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
            Message::User(UserMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "1 edit applied".into(),
                    is_error: false,
                    extra_content: Vec::new(),
                }],
                is_meta: true,
                is_compact_summary: false,
            }),
        ];

        let items = transcript_from_messages(&messages, true);
        assert_eq!(items.len(), 3, "got {items:?}");
        assert!(matches!(&items[0], TranscriptItem::User(t) if t == "add a null check"));
        assert!(matches!(&items[1], TranscriptItem::Assistant(t) if t == "On it."));
        match &items[2] {
            TranscriptItem::Tool {
                name,
                detail,
                result,
                ..
            } => {
                assert_eq!(name, "FileEdit");
                assert_eq!(detail, "src/auth.rs");
                assert_eq!(
                    result.as_deref(),
                    Some("1 edit applied"),
                    "the tool result was not paired back to its call"
                );
            }
            other => panic!("expected a tool card, got {other:?}"),
        }
    }

    /// Tool results arrive as `is_meta` user messages. Rendering them as
    /// user turns would show the transcript talking to itself.
    #[test]
    fn meta_messages_do_not_become_user_turns() {
        let messages = vec![Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Text {
                text: "injected context".into(),
            }],
            is_meta: true,
            is_compact_summary: false,
        })];
        assert!(transcript_from_messages(&messages, true).is_empty());
    }

    /// `/resume` must not scan the sessions directory on the thread that
    /// draws frames and reads keys — it only raises a request.
    #[test]
    fn resume_only_requests_the_session_list() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "/resume".into();
        app.cursor = app.input.len();
        app.submit();
        assert!(
            app.pending_session_list,
            "the run loop was never asked for the session list"
        );
        assert!(
            !app.session_picker_open(),
            "the picker opened before the list was fetched — the scan ran inline"
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn an_empty_session_list_reports_instead_of_opening_the_picker() {
        let mut app = App::new("m", "/tmp", "s");
        app.show_session_picker(Vec::new());
        assert!(!app.session_picker_open());
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::System(t) if t.contains("no saved sessions"))),
        );
    }

    #[test]
    fn a_returned_session_list_opens_the_picker() {
        let mut app = App::new("m", "/tmp", "s");
        app.show_session_picker(vec![summary("aaa11111", None, "/a")]);
        assert!(app.session_picker_open());
    }

    /// The picker is fed by the index-cached summary listing, which does
    /// not deserialize transcripts — so rows must not be sized by a
    /// `message_count` that path never populates.
    #[test]
    fn rows_are_sized_by_turns_not_unpopulated_message_counts() {
        let mut s = summary("aaa11111", Some("auth work"), "/home/u/api");
        s.message_count = 0;
        s.turn_count = 3;
        let line = summary_line(&s);
        assert!(line.contains("3 turns"), "got {line}");
        assert!(
            !line.contains("0 msg"),
            "row reports a count it never read: {line}"
        );

        s.turn_count = 1;
        assert!(summary_line(&s).contains("1 turn "), "{}", summary_line(&s));
    }

    /// A compacted session's earlier turns survive only inside the
    /// compact summary. Dropping it with the rest of the meta plumbing
    /// leaves the resumed conversation starting mid-thought while the
    /// engine still reasons from the hidden text.
    #[test]
    fn a_compact_summary_survives_into_the_restored_transcript() {
        let messages = vec![
            Message::User(UserMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text {
                    text: "earlier we refactored the auth module".into(),
                }],
                is_meta: true,
                is_compact_summary: true,
            }),
            user("now add tests"),
        ];
        let items = transcript_from_messages(&messages, true);
        assert_eq!(items.len(), 2, "got {items:?}");
        assert!(
            matches!(&items[0], TranscriptItem::System(t) if t.contains("refactored the auth module")),
            "the compact summary was dropped: {items:?}"
        );
        assert!(matches!(&items[1], TranscriptItem::User(t) if t == "now add tests"));
    }

    fn thinking_turn() -> Vec<Message> {
        vec![Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![
                ContentBlock::Thinking {
                    thinking: "the null check belongs in the caller".into(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "Adding it now.".into(),
                },
            ],
            model: None,
            usage: None,
            stop_reason: None,
            request_id: None,
        })]
    }

    /// Reasoning was on screen live; the stored messages still carry it,
    /// so a resumed transcript that drops it is missing content the user
    /// had already read.
    #[test]
    fn stored_thinking_is_restored_when_thinking_blocks_are_shown() {
        let items = transcript_from_messages(&thinking_turn(), true);
        assert_eq!(items.len(), 2, "got {items:?}");
        assert!(
            matches!(&items[0], TranscriptItem::Thinking { text, duration_ms }
                if text.contains("belongs in the caller") && duration_ms.is_none()),
            "stored reasoning was dropped: {items:?}"
        );
        assert!(matches!(&items[1], TranscriptItem::Assistant(t) if t == "Adding it now."));
    }

    /// ...and restoring must not switch thinking blocks *on* for a user
    /// who keeps them off.
    #[test]
    fn stored_thinking_is_omitted_when_thinking_blocks_are_hidden() {
        let items = transcript_from_messages(&thinking_turn(), false);
        assert_eq!(items.len(), 1, "got {items:?}");
        assert!(matches!(&items[0], TranscriptItem::Assistant(_)));
    }

    /// Rendering hides the picker behind a HITL modal, so leaving it
    /// *open* handed y/a/n, Esc and Enter to an invisible overlay: the
    /// visible modal looked unresponsive, and Enter could schedule a
    /// resume the user never saw themselves choose.
    #[test]
    fn a_permission_ask_closes_an_open_session_picker() {
        use super::super::sink::EngineEvent;
        let mut app = App::new("m", "/tmp", "s");
        app.open_session_picker(vec![summary("aaa11111", None, "/a")]);
        assert!(app.session_picker_open());

        let (tx, _rx) = std::sync::mpsc::channel::<PermissionResponse>();
        app.apply_engine(EngineEvent::PermissionAsk {
            name: "Bash".into(),
            description: "run".into(),
            origin: None,
            input_preview: None,
            respond: tx,
        });

        assert!(
            !app.session_picker_open(),
            "picker kept the keyboard under a HITL modal"
        );
        assert!(app.pending_resume.is_none(), "a resume was scheduled");
    }

    /// A `/clear` held back for a resume lands after the restore has
    /// repainted the screen, so the visual clear has to run again — or
    /// the restored history sits in front of an empty conversation.
    #[test]
    fn the_visual_clear_can_be_reapplied_after_a_restore() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "/clear".into();
        app.cursor = app.input.len();
        app.submit();
        assert!(app.pending_clear, "engine clear was not deferred");
        assert!(app.transcript.is_empty());

        // The restore repopulates the screen while the engine clear is
        // still pending.
        app.restore_transcript(
            vec![TranscriptItem::User("restored".into())],
            "abcdef123456",
            2,
        );
        assert!(!app.transcript.is_empty());

        // Applying the deferred clear must take the view with it.
        app.clear_transcript_view();
        assert!(
            app.transcript.is_empty(),
            "restored history survived the deferred /clear: {:?}",
            app.transcript
        );
        assert!(app.ctx_meter.is_none());
    }

    /// A failed resume must not release work that was deferred *for* the
    /// session that never arrived: a prompt or `!cmd` would take real
    /// side effects in the conversation the user was trying to leave.
    #[test]
    fn a_failed_resume_cancels_the_work_it_deferred() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_resume = Some("abcdef123456".into());
        app.pending_clear = true;
        app.pending_slash = Some("/cost".into());
        app.pending_shell = Some("rm -rf build".into());
        app.pending_submit = Some("keep going".into());

        app.cancel_deferred_resume_work();

        assert!(!app.pending_clear, "/clear would run on the old session");
        assert!(
            app.pending_slash.is_none(),
            "slash command would run on the old session"
        );
        assert!(
            app.pending_shell.is_none(),
            "shell command would run on the old session"
        );
        assert!(
            app.pending_submit.is_none(),
            "prompt would start a turn on the old session"
        );
        // Nothing vanishes: the prompt returns to the composer and the
        // rest is named in full.
        assert_eq!(app.input, "keep going");
        let said = app
            .transcript
            .iter()
            .filter_map(|i| match i {
                TranscriptItem::System(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in ["/clear", "/cost", "!rm -rf build"] {
            assert!(
                said.contains(expected),
                "{expected} was cancelled silently: {said}"
            );
        }
    }

    /// Put a permission modal in front. The receiver is returned so the
    /// caller keeps the channel alive for the length of the test.
    fn a_modal(app: &mut App) -> std::sync::mpsc::Receiver<PermissionResponse> {
        use super::super::app::{Modal, PendingPermission, Phase};
        let (tx, rx) = std::sync::mpsc::channel::<PermissionResponse>();
        app.modals.push_back(Modal::Permission(PendingPermission {
            name: "Bash".into(),
            description: "run".into(),
            origin: None,
            input_preview: None,
            respond: tx,
        }));
        app.phase = Phase::Permission;
        rx
    }

    /// Rows that arrive while a permission modal owns the screen must not
    /// be dropped: the picker would never open, nothing would retry, and
    /// the status bar would read "loading sessions…" forever.
    #[test]
    fn picker_rows_arriving_under_a_modal_are_held_not_dropped() {
        let mut app = App::new("m", "/tmp", "s");
        let _rx = a_modal(&mut app);
        app.show_session_picker(vec![summary("aaa11111", None, "/a")]);
        assert!(!app.session_picker_open(), "picker drew over a HITL modal");
        assert!(
            app.deferred_sessions.is_some(),
            "the session list was dropped on the floor"
        );

        // Still blocked while the modal is up.
        app.retry_session_picker();
        assert!(!app.session_picker_open());

        // Modal answered: the held rows open the picker.
        app.modals.pop_front();
        app.retry_session_picker();
        assert!(
            app.session_picker_open(),
            "held session list never opened the picker"
        );
        assert!(app.deferred_sessions.is_none());
    }

    #[test]
    fn retrying_without_held_rows_is_a_noop() {
        let mut app = App::new("m", "/tmp", "s");
        app.retry_session_picker();
        assert!(!app.session_picker_open());
    }

    fn restored() -> RestoredState {
        RestoredState {
            id: "abcdef123456".into(),
            model: "restored-model".into(),
            turn_count: 7,
            tokens_in: 1234,
            tokens_out: 567,
            cost_usd: 4.25,
            plan_mode: true,
        }
    }

    /// The engine is only half a resume: the header, the badge and
    /// `/cost` all read App's own copies of this state.
    #[test]
    fn restored_state_is_mirrored_into_the_app() {
        let mut app = App::new("old-model", "/tmp", "old-session");
        app.mode = SessionMode::Normal;
        app.turn_count = 99;
        app.tokens_in = 1;
        app.tokens_out = 2;
        app.cost_usd = 99.0;
        app.ctx_meter = Some((10, 20));

        let mode = app.adopt_restored_session(&restored());

        assert_eq!(mode, SessionMode::Plan, "caller cannot apply the live mode");
        assert_eq!(
            app.mode,
            SessionMode::Plan,
            "badge still shows the old mode"
        );
        assert_eq!(app.model, "restored-model");
        assert_eq!(app.session_id, "abcdef123456");
        assert_eq!(app.turn_count, 7);
        assert_eq!(app.tokens_in, 1234);
        assert_eq!(app.tokens_out, 567);
        assert_eq!(app.cost_usd, 4.25);
        assert!(
            app.ctx_meter.is_none(),
            "context meter still describes the discarded conversation"
        );
    }

    /// A non-plan session must clear a plan badge inherited from the
    /// session being replaced, not just fail to set one.
    #[test]
    fn restoring_a_non_plan_session_leaves_plan_mode() {
        let mut app = App::new("m", "/tmp", "s");
        app.mode = SessionMode::Plan;
        let mode = app.adopt_restored_session(&RestoredState {
            plan_mode: false,
            ..restored()
        });
        assert_eq!(mode, SessionMode::Normal);
        assert_eq!(app.mode, SessionMode::Normal);
    }

    /// An empty stored model must not blank the header.
    #[test]
    fn an_empty_restored_model_keeps_the_current_one() {
        let mut app = App::new("current-model", "/tmp", "s");
        app.adopt_restored_session(&RestoredState {
            model: String::new(),
            ..restored()
        });
        assert_eq!(app.model, "current-model");
    }

    /// Prompts typed against the previous conversation must not be fired
    /// at the session that replaced it.
    #[test]
    fn resuming_drops_prompts_staged_for_the_previous_session() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_submit = Some("for the old session".into());
        app.queue.push_back("also for the old session".into());
        app.adopt_restored_session(&restored());
        assert!(
            app.pending_submit.is_none(),
            "old prompt survived the resume"
        );
        assert!(app.queue.is_empty(), "old queue survived the resume");
        // The staged prompt goes back to the (empty) composer; the queued
        // one is reproduced verbatim rather than reduced to a count.
        assert_eq!(app.input, "for the old session");
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::System(t)
                    if t.contains("also for the old session"))),
            "a queued prompt was eaten: {:?}",
            app.transcript
        );
    }

    /// A prompt submitted while the session was still loading never ran
    /// (turn spawning is blocked for that window), so it must come back
    /// to the composer rather than be eaten or fired at the old session.
    #[test]
    fn a_prompt_submitted_during_the_load_returns_to_the_composer() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_submit = Some("check the parser".into());
        app.adopt_restored_session(&restored());
        assert!(
            app.pending_submit.is_none(),
            "prompt would still start a turn"
        );
        assert_eq!(app.input, "check the parser", "the typed prompt was lost");
        assert_eq!(app.cursor, "check the parser".len());
    }

    /// ...unless the composer already holds a draft, which must not be
    /// clobbered — report the staged prompt instead.
    #[test]
    fn a_staged_prompt_is_reported_when_the_composer_is_busy() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "half-written draft".into();
        app.pending_submit = Some("check the parser".into());
        app.adopt_restored_session(&restored());
        assert_eq!(app.input, "half-written draft", "draft was clobbered");
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::System(t) if t.contains("check the parser"))),
            "the staged prompt was reduced to a count and its text lost: {:?}",
            app.transcript
        );
    }

    /// The resume waits for the live turn to be reaped; the reaper's
    /// clean-finish path must not send the old queue in the meantime.
    #[test]
    fn a_pending_resume_suppresses_queue_auto_dispatch() {
        use super::super::app::Phase;
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.queue.push_back("old conversation prompt".into());
        app.pending_resume = Some("abcdef123456".into());
        app.mark_turn_idle();
        app.dispatch_queue_head();
        assert!(
            app.pending_submit.is_none(),
            "queued prompt was dispatched into the session being resumed"
        );
        assert_eq!(app.queue.len(), 1, "the prompt should still be queued");
    }

    #[test]
    fn restoring_replaces_the_transcript_and_notes_the_session() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::User("stale".into()));
        app.restore_transcript(
            vec![TranscriptItem::User("fresh".into())],
            "abcdef123456",
            4,
        );
        assert!(
            !app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "stale")),
            "the previous transcript survived a resume"
        );
        assert!(matches!(&app.transcript[0], TranscriptItem::User(t) if t == "fresh"));
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptItem::System(t)) if t.contains("abcdef12")
        ));
    }
}
