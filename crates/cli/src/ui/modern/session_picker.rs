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
    format!(
        "{short_id}  {label}  ·  {} msg  ·  {}",
        s.message_count, s.updated_at
    )
}

/// Rebuild transcript items from a restored conversation.
///
/// Tool calls are matched to their results by `tool_use_id` so a resumed
/// session shows the same cards it did live, rather than a wall of raw
/// blocks. Meta messages (tool results, context injection) are skipped:
/// they are conversation plumbing the user never saw the first time.
pub fn transcript_from_messages(
    messages: &[agent_code_lib::llm::message::Message],
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
                if u.is_meta {
                    continue;
                }
                let text: String = u
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message, UserMessage};
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

        let items = transcript_from_messages(&messages);
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
        assert!(transcript_from_messages(&messages).is_empty());
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
