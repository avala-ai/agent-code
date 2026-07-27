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

use super::app::{App, Phase, TranscriptItem};
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

/// Longest restored user row rendered in full.
///
/// A saved user message is the *engine* payload: a skill invocation is
/// stored as the whole skill body. Nothing clamps user rows on the way
/// to the screen, so a resumed session would otherwise open with one
/// enormous block where the user remembers a one-line prompt.
const MAX_RESTORED_USER_CHARS: usize = 4000;

/// Recover something close to what the user actually typed from a saved
/// user message.
///
/// `enqueue_turn` keeps the typed line and the engine payload separate
/// and only the payload is persisted, so this reverses what it can:
/// `@mention` inlining is cut at its envelope exactly, and anything
/// still far too long to be a typed line (skill bodies, which the
/// payload gives no way to reverse) is clamped with a visible marker
/// rather than dumped into the transcript.
fn display_form(text: &str) -> String {
    let typed = match text.find(super::mentions::MENTION_ENVELOPE) {
        Some(at) => &text[..at],
        None => text,
    };
    if typed.chars().count() <= MAX_RESTORED_USER_CHARS {
        return typed.to_string();
    }
    let head: String = typed.chars().take(MAX_RESTORED_USER_CHARS).collect();
    format!("{head}\n… (expanded prompt — truncated for display)")
}

/// First eight *characters* of a session id, for compact display.
///
/// Character-safe, not byte-safe: ids come from session filenames, and
/// an imported or hand-renamed file can carry non-ASCII, where slicing
/// at byte 8 can land mid-character and panic the whole TUI.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
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
    let short_id = short_id(&s.id);
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
    /// Reasoning effort *after* the engine reset — the configured
    /// startup value, not the discarded conversation's choice.
    pub effort: Option<String>,
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
                    items.push(TranscriptItem::User(display_form(&text)));
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
        // Every other overlay that owns the keyboard has to go first.
        // `handle_key` routes to search *before* the picker, so rows that
        // arrive after the user opened Ctrl+F would draw a picker that
        // silently ignored every keystroke; the model and theme pickers
        // are routed after, so they would instead be left open and
        // unreachable. These are mutually exclusive overlays.
        self.cancel_search();
        self.model_picker = None;
        // Cancel, not drop: the theme picker previews live, so discarding
        // it would strand the global palette on a theme the user never
        // accepted while the config still names the original.
        self.theme_picker_cancel();
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

    /// Remove the transcript row a staged submission already drew.
    ///
    /// `submit` echoes the line the moment it is accepted, so a
    /// submission cancelled or handed back during a resume would
    /// otherwise stay on screen looking like it had been sent.
    fn remove_staged_row(&mut self, display: &str) {
        let Some(idx) = self
            .transcript
            .iter()
            .rposition(|i| matches!(i, TranscriptItem::User(t) if t == display))
        else {
            return;
        };
        // `@mentions:` notes are drawn immediately after the row.
        if matches!(
            self.transcript.get(idx + 1),
            Some(TranscriptItem::System(t)) if t.starts_with("@mentions:")
        ) {
            self.transcript.remove(idx + 1);
        }
        self.transcript.remove(idx);
        self.expanded.clear();
        self.selected_item = None;
        self.layout.invalidate();
        self.dirty = true;
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
        // Notices carried from the accept are already on the transcript,
        // and the swap they were waiting for is never going to happen.
        // Left here they would surface again — stale and attributed to
        // the wrong resume — the next time one succeeds.
        self.resume_notices.clear();
        // Terminal path: no restore follows, so nothing needs to survive
        // a transcript swap that will not happen.
        self.cancel_pending_session_work(
            "cancelled — held for the session that failed to load:",
            false,
        );
    }

    /// Session-scoped work staged against a conversation that is being
    /// left behind.
    ///
    /// It cannot run where it was written (that conversation is going
    /// away) and must not run anywhere else, so it is cancelled — but
    /// reproduced verbatim, never reduced to a count. The submitted
    /// prompt goes back to the composer when the composer is free.
    /// `carry` keeps a copy for [`App::restore_transcript`] to re-emit:
    /// a successful resume replaces the whole transcript, which would
    /// otherwise wipe the report before the user ever read it.
    fn cancel_pending_session_work(&mut self, why: &str, carry: bool) {
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
            let row = format!("!{cmd}");
            self.remove_staged_row(&row);
            cancelled.push(row);
        }
        self.reclaim_staged_prompts("not sent:", &mut cancelled);
        if !cancelled.is_empty() {
            let body = cancelled.join("\n");
            let note = format!("{why}\n{body}");
            if carry {
                self.resume_notices.push(note.clone());
            }
            self.transcript.push(TranscriptItem::System(note));
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
        if let Some(payload) = self.pending_submit.take() {
            // The line the user typed, not the engine payload: that has
            // `@path` mentions and skill bodies already inlined, and
            // handing it back would drop a whole file into the composer
            // and expand the mention a second time on resubmission.
            let text = self.pending_submit_display.take().unwrap_or(payload);
            // The row `submit` drew is a claim this was sent. It never
            // ran, so it goes with the prompt.
            self.remove_staged_row(&text);
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
        // Submitting during the resume window moved the phase to
        // Streaming, but the gate stopped any turn from spawning, so
        // nothing will ever reap it back to Idle — and a stuck Streaming
        // phase makes every later Enter queue instead of send.
        //
        // Only when no turn is actually live. Accepting the picker
        // mid-stream reaches here with a real `TurnHandle` still owned by
        // the event loop: forcing Idle there would send Ctrl+C down the
        // quit path instead of cancelling the turn. A HITL modal likewise
        // keeps the phase it owns.
        if !self.turn_live && self.modals.is_empty() && self.phase != Phase::Idle {
            self.phase = Phase::Idle;
            self.dirty = true;
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

    /// Pasted text goes to the picker's filter while it is open — it
    /// owns typed input, so it owns pasted input too. Otherwise the
    /// paste silently edits the composer hidden behind it and the draft
    /// resurfaces after the picker closes.
    pub fn session_picker_insert_str(&mut self, text: &str) {
        let Some(p) = self.session_picker.as_mut() else {
            return;
        };
        // A filter is one line; newlines and control characters in a
        // pasted id or label are noise.
        p.query.extend(text.chars().filter(|c| !c.is_control()));
        p.selected = 0;
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
        // Work already staged against the conversation we are leaving is
        // resolved here, at the moment of the decision. Left alone the
        // resume gates would merely *hold* it, and it would then land on
        // the restored session — a `/clear` issued against the old
        // conversation would clear the new one.
        self.cancel_pending_session_work(
            "cancelled — issued before you resumed another session:",
            true,
        );
        self.pending_resume = Some(id.clone());
        self.status_message = format!("resuming {}…", short_id(&id));
        self.dirty = true;
    }

    /// Adopt the restored conversation's checklist.
    ///
    /// A resume rewrites history exactly as `/rewind` and `/snip` do, so
    /// it uses the same seam: bump the epoch, which disowns anything the
    /// previous conversation still has in flight, then rebuild the pane
    /// from the messages actually restored. Skipping it leaves the pane
    /// showing the discarded session's work.
    pub fn adopt_restored_todos(&mut self, messages: &[agent_code_lib::llm::message::Message]) {
        self.new_conversation();
        self.todos = super::tasks::todos_from_messages(messages);
    }

    /// Replace the visible transcript with a restored conversation.
    pub fn restore_transcript(&mut self, items: Vec<TranscriptItem>, id: &str, turns: usize) {
        // Remember the session being left, so switching back lands where
        // it was rather than at the bottom of a rebuilt transcript.
        self.stash_current_view();

        // Returning to a session visited earlier: restore what was on
        // screen instead of the rebuild. The engine reloaded the
        // conversation either way; this only decides what is shown.
        if let Some(view) = self.session_views.take(id) {
            self.transcript = view.transcript;
            self.expanded = view.expanded;
            self.selected_item = view.selected_item;
            self.layout.invalidate();
            self.scroll = view.scroll;
            for note in std::mem::take(&mut self.resume_notices) {
                self.transcript.push(TranscriptItem::System(note));
            }
            self.status_message.clear();
            self.dirty = true;
            return;
        }

        self.transcript = items;
        self.expanded.clear();
        self.selected_item = None;
        self.layout.invalidate();
        self.transcript.push(TranscriptItem::System(format!(
            "resumed session {} · {} turns",
            short_id(id),
            turns
        )));
        // Re-emit anything reported before the swap: it was pushed onto
        // the transcript we just replaced, so without this the promised
        // verbatim report of cancelled work never reaches the user.
        for note in std::mem::take(&mut self.resume_notices) {
            self.transcript.push(TranscriptItem::System(note));
        }
        self.scroll_to_bottom();
        self.status_message.clear();
        self.dirty = true;
    }

    /// Snapshot the current session's view before switching away.
    ///
    /// A session with nothing on screen is not stored — see
    /// [`super::session_views::SessionViews::save`].
    pub fn stash_current_view(&mut self) {
        let id = self.session_id.clone();
        let view = super::session_views::SessionView {
            transcript: self.transcript.clone(),
            scroll: self.scroll,
            expanded: self.expanded.clone(),
            selected_item: self.selected_item,
        };
        self.session_views.save(&id, view);
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
        self.effort = s.effort.clone();
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
    /// Switching away and back must land where you left. Rebuilding is
    /// correct but loses position and expansions, which makes moving
    /// between sessions cost more than it saves.
    #[test]
    fn returning_to_a_session_restores_where_you_were() {
        let mut app = App::new("m", "/tmp", "session-a");
        app.transcript
            .push(TranscriptItem::User("work in a".into()));
        app.scroll = crate::ui::modern::scroll::ScrollState::Free { top_line: 7 };
        app.expanded.insert(0);

        // Switch to B: A's view is stashed, B is rebuilt from messages.
        app.session_id = "session-a".into();
        app.restore_transcript(
            vec![TranscriptItem::User("work in b".into())],
            "session-b",
            1,
        );
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "work in b")),
            "did not switch to B"
        );

        // Back to A: the stashed view wins over the rebuild, so the
        // rebuilt items passed here must NOT be what is shown.
        app.session_id = "session-b".into();
        app.restore_transcript(
            vec![TranscriptItem::User("rebuilt from disk".into())],
            "session-a",
            1,
        );
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "work in a")),
            "A's view was not restored: {:?}",
            app.transcript
        );
        assert!(
            !app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "rebuilt from disk")),
            "the rebuild replaced a cached view"
        );
        assert_eq!(
            app.scroll,
            crate::ui::modern::scroll::ScrollState::Free { top_line: 7 },
            "scroll position was not restored"
        );
        assert!(app.expanded.contains(&0), "expansions were not restored");
    }

    /// A session never visited has no cached view, so it must be built
    /// from the conversation rather than showing someone else's.
    #[test]
    fn a_first_visit_uses_the_rebuilt_transcript() {
        let mut app = App::new("m", "/tmp", "session-a");
        app.transcript.push(TranscriptItem::User("in a".into()));
        app.restore_transcript(
            vec![TranscriptItem::User("fresh from disk".into())],
            "never-seen",
            2,
        );
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "fresh from disk"))
        );
        assert!(
            !app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "in a"))
        );
    }

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

    /// `handle_key` routes to search before the picker, so a picker
    /// opened while search is up would ignore every keystroke.
    #[test]
    fn opening_the_picker_closes_the_overlays_that_would_eat_its_keys() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_search();
        assert!(app.search_open());
        app.open_session_picker(vec![summary("aaa11111", None, "/a")]);
        assert!(app.session_picker_open());
        assert!(
            !app.search_open(),
            "search still owns the keyboard under a visible picker"
        );
        assert!(app.model_picker.is_none());
        assert!(app.theme_picker.is_none());
        assert!(app.command_palette.is_none());
    }

    /// The theme picker previews live, so dropping it would strand the
    /// global palette on a theme the user never accepted. Mutates the
    /// process-global theme, hence the shared test lock.
    #[test]
    fn opening_the_picker_reverts_a_theme_preview_rather_than_dropping_it() {
        let _g = crate::ui::theme::test_lock();
        let mut app = App::new("m", "/tmp", "s");
        app.set_theme("one-dark");
        app.pending_theme = None;

        app.open_theme_picker();
        // Browsing previews the candidate globally without committing.
        app.theme_picker_move(1);
        app.theme_picker_move(1);

        app.open_session_picker(vec![summary("aaa11111", None, "/a")]);

        assert!(app.session_picker_open());
        assert!(app.theme_picker.is_none());
        assert_eq!(app.theme_name, "one-dark");
        assert!(
            app.pending_theme.is_none(),
            "an unaccepted preview was queued for persistence"
        );
        assert_eq!(
            crate::ui::theme::current().accent,
            crate::ui::theme::Theme::from_name("one-dark").accent,
            "an unaccepted preview survived opening the resume picker"
        );
    }

    /// Rows that arrive late must clear search too — the user has had a
    /// whole scan's worth of time to press Ctrl+F.
    #[test]
    fn late_rows_also_close_search() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_session_list = true;
        app.open_search();
        app.show_session_picker(vec![summary("aaa11111", None, "/a")]);
        assert!(app.session_picker_open());
        assert!(!app.search_open());
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
            effort: None,
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

    /// Work staged *before* the picker was accepted belongs to the
    /// conversation being left. Merely gating it would hold it until the
    /// restore, at which point a `/clear` issued against the old session
    /// would clear the newly restored one.
    #[test]
    fn accepting_cancels_work_staged_against_the_old_session() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_clear = true;
        app.pending_slash = Some("/cost".into());
        app.pending_shell = Some("make clean".into());
        app.open_session_picker(vec![summary("aaa11111", None, "/a")]);

        app.session_picker_accept();

        assert_eq!(app.pending_resume.as_deref(), Some("aaa11111"));
        assert!(
            !app.pending_clear,
            "/clear would have cleared the restored session"
        );
        assert!(app.pending_slash.is_none());
        assert!(app.pending_shell.is_none());
        let said = app
            .transcript
            .iter()
            .filter_map(|i| match i {
                TranscriptItem::System(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in ["/clear", "/cost", "!make clean"] {
            assert!(
                said.contains(expected),
                "{expected} was cancelled silently: {said}"
            );
        }
    }

    /// The cancellation report is pushed onto the transcript that the
    /// restore then replaces wholesale, so it has to be re-emitted or
    /// the user never sees what was cancelled on their behalf.
    #[test]
    fn the_cancellation_report_survives_the_transcript_swap() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_shell = Some("make clean".into());
        app.open_session_picker(vec![summary("aaa11111", None, "/a")]);
        app.session_picker_accept();

        // The restore replaces everything that was on screen.
        app.restore_transcript(vec![TranscriptItem::User("restored".into())], "aaa11111", 2);

        let said = app
            .transcript
            .iter()
            .filter_map(|i| match i {
                TranscriptItem::System(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            said.contains("!make clean"),
            "the cancellation report was wiped by the resume: {said}"
        );
        assert!(
            app.resume_notices.is_empty(),
            "notices must not be re-emitted a second time"
        );
    }

    /// A resume that fails after cancelling work must not leave its
    /// report queued: the next successful resume would replay it,
    /// stale and against the wrong session.
    #[test]
    fn a_failed_resume_does_not_leave_its_report_for_the_next_one() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_shell = Some("make clean".into());
        app.open_session_picker(vec![summary("aaa11111", None, "/a")]);
        app.session_picker_accept();
        assert!(!app.resume_notices.is_empty(), "nothing was carried");

        // The chosen session turns out to be missing or corrupt.
        app.cancel_deferred_resume_work();
        assert!(app.resume_notices.is_empty());

        // A later, unrelated resume must not replay it.
        app.restore_transcript(
            vec![TranscriptItem::User("a different session".into())],
            "bbb22222",
            1,
        );
        let said = app
            .transcript
            .iter()
            .filter_map(|i| match i {
                TranscriptItem::System(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !said.contains("make clean"),
            "a stale report resurfaced on an unrelated resume: {said}"
        );
    }

    /// Reasoning effort is chosen per conversation and not saved, so the
    /// resumed session must show what the engine actually reset to.
    #[test]
    fn the_effort_mirror_follows_the_restored_session() {
        let mut app = App::new("m", "/tmp", "s");
        app.effort = Some("high".into());
        app.adopt_restored_session(&RestoredState {
            effort: None,
            ..restored()
        });
        assert!(
            app.effort.is_none(),
            "the badge still claims an effort the restored session never chose"
        );
    }

    /// A resume rewrites history, so the checklist pane must follow the
    /// restored conversation rather than keep describing the discarded
    /// one. Same seam `/rewind` and `/snip` use.
    #[test]
    fn resuming_rebuilds_the_checklist_from_the_restored_conversation() {
        let mut app = App::new("m", "/tmp", "s");
        app.todos = super::super::tasks::todos_from_messages(&[
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolUse {
                    id: "old".into(),
                    name: "TodoWrite".into(),
                    input: serde_json::json!({
                        "todos": [{ "id": "1", "content": "the discarded plan", "status": "in_progress" }]
                    }),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "old".into(),
                    content: "ok".into(),
                    is_error: false,
                    extra_content: Vec::new(),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }),
        ]);
        assert_eq!(app.todos.len(), 1, "fixture did not build a checklist");
        let epoch_before = app.conversation_epoch;

        // The restored session never called TodoWrite.
        app.adopt_restored_todos(&[user("just a question")]);

        assert!(
            app.todos.is_empty(),
            "the pane still shows the discarded session's checklist: {:?}",
            app.todos
        );
        assert_ne!(
            app.conversation_epoch, epoch_before,
            "in-flight work from the old conversation was not disowned"
        );
    }

    /// A saved user message is the engine payload, so a turn sent with
    /// an `@path` mention stores the inlined file. Rebuilding it
    /// verbatim shows the whole file as something the user typed.
    #[test]
    fn a_restored_mention_turn_shows_the_typed_line_not_the_inlined_file() {
        let payload = format!(
            "look at @src/main.rs{}\n<file path=\"src/main.rs\">\nfn main() {{}}\n</file>\n",
            super::super::mentions::MENTION_ENVELOPE
        );
        let items = transcript_from_messages(&[user(&payload)], true);
        assert_eq!(items.len(), 1);
        match &items[0] {
            TranscriptItem::User(t) => {
                assert_eq!(t, "look at @src/main.rs");
                assert!(
                    !t.contains("fn main"),
                    "the inlined file leaked into the row"
                );
            }
            other => panic!("expected a user row, got {other:?}"),
        }
    }

    /// A skill invocation is stored as the whole skill body and cannot be
    /// reversed from the payload, so it is clamped rather than dumped —
    /// nothing clamps user rows on the way to the screen.
    #[test]
    fn an_enormous_restored_user_turn_is_clamped() {
        let huge = "x".repeat(MAX_RESTORED_USER_CHARS * 2);
        let items = transcript_from_messages(&[user(&huge)], true);
        match &items[0] {
            TranscriptItem::User(t) => {
                assert!(
                    t.chars().count() < huge.chars().count(),
                    "the whole payload was rendered as one user row"
                );
                assert!(
                    t.contains("truncated for display"),
                    "clamped without saying so"
                );
            }
            other => panic!("expected a user row, got {other:?}"),
        }
    }

    /// One slot each: a second submission before the loop drains the
    /// first silently discarded it while its row claimed it had run.
    #[test]
    fn a_second_shell_submission_does_not_discard_the_first() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_resume = Some("abcdef123456".into());

        app.input = "!one".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(app.pending_shell.as_deref(), Some("one"));

        app.input = "!two".into();
        app.cursor = app.input.len();
        app.submit();

        assert_eq!(
            app.pending_shell.as_deref(),
            Some("one"),
            "the queued command was overwritten and never ran"
        );
        assert_eq!(app.input, "!two", "the refused text was lost");
        assert!(app.status_message.contains("still pending"));
    }

    /// `/clear` submitted during a load must not blank the screen: the
    /// engine clear is deferred and is cancelled if the load fails,
    /// which would leave an empty transcript in front of a conversation
    /// that is still there.
    #[test]
    fn clear_during_a_resume_holds_its_visible_half() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.push(TranscriptItem::User("earlier".into()));
        app.pending_resume = Some("abcdef123456".into());

        app.input = "/clear".into();
        app.cursor = app.input.len();
        app.submit();

        assert!(app.pending_clear, "the engine clear was not staged");
        assert!(
            !app.transcript.is_empty(),
            "the screen was blanked before the resume was known to succeed"
        );

        // The load fails: the clear is cancelled and the history stands.
        app.cancel_deferred_resume_work();
        assert!(!app.pending_clear);
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "earlier")),
            "history was lost to a clear that never ran"
        );
    }

    /// `submit` echoes the line immediately; a prompt handed back during
    /// a resume never ran, so the row must not stay on screen claiming
    /// it was sent.
    #[test]
    fn reclaiming_removes_the_row_the_submission_drew() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_resume = Some("abcdef123456".into());
        app.input = "check the parser".into();
        app.cursor = app.input.len();
        app.submit();
        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "check the parser")),
            "fixture did not draw the row"
        );

        app.adopt_restored_session(&restored());

        assert_eq!(
            app.input, "check the parser",
            "the prompt was not handed back"
        );
        assert!(
            !app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::User(t) if t == "check the parser")),
            "the row still claims the prompt was sent: {:?}",
            app.transcript
        );
    }

    /// `pending_submit` holds the engine payload, with `@path` mentions
    /// and skill bodies inlined. Handing *that* back would drop a whole
    /// file into the composer and expand the mention again on resend.
    #[test]
    fn reclaiming_returns_the_typed_line_not_the_expanded_payload() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_submit = Some(
            "look at @src/main.rs
<file>…thousands of lines…</file>"
                .into(),
        );
        app.pending_submit_display = Some("look at @src/main.rs".into());

        app.adopt_restored_session(&restored());

        assert_eq!(
            app.input, "look at @src/main.rs",
            "the expanded payload was pasted into the composer"
        );
        assert!(app.pending_submit_display.is_none());
    }

    /// Ids come from session filenames, so an imported or hand-renamed
    /// file can carry non-ASCII. Slicing at byte 8 lands mid-character
    /// and takes the whole TUI down.
    #[test]
    fn a_non_ascii_session_id_does_not_panic() {
        let id = "日本語のセッション";
        let mut app = App::new("m", "/tmp", "s");
        app.open_session_picker(vec![summary(id, None, "/a")]);

        // Rendering the row, accepting it, and reporting the restore all
        // truncate the id.
        let _ = summary_line(&summary(id, None, "/a"));
        app.session_picker_accept();
        assert_eq!(app.pending_resume.as_deref(), Some(id));
        app.restore_transcript(vec![], id, 1);

        assert!(
            app.transcript
                .iter()
                .any(|i| matches!(i, TranscriptItem::System(t) if t.contains("日本語のセ"))),
            "the restore note did not name the session: {:?}",
            app.transcript
        );
    }

    /// The picker captures typed keys, so it must capture pasted text
    /// too — otherwise the paste edits an invisible composer draft.
    #[test]
    fn pasting_filters_the_picker_instead_of_the_hidden_composer() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_session_picker(vec![
            summary("aaa11111", Some("auth work"), "/a"),
            summary("bbb22222", None, "/b"),
        ]);

        app.session_picker_insert_str(
            "auth
work",
        );

        let p = app.session_picker.as_ref().unwrap();
        assert_eq!(
            p.query, "authwork",
            "newlines leaked into a one-line filter"
        );
        assert!(
            app.input.is_empty(),
            "the paste edited the composer behind the picker: {:?}",
            app.input
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

    /// Submitting during the load moves the phase to Streaming, but the
    /// gate stops any turn spawning, so nothing reaps it — leaving every
    /// later Enter queueing instead of sending.
    #[test]
    fn reclaiming_a_load_time_prompt_returns_the_app_to_idle() {
        let mut app = App::new("m", "/tmp", "s");
        app.pending_resume = Some("abcdef123456".into());
        app.input = "check the parser".into();
        app.cursor = app.input.len();
        app.submit();
        assert_eq!(app.phase, Phase::Streaming, "submit did not stage a turn");

        app.adopt_restored_session(&restored());

        assert_eq!(
            app.phase,
            Phase::Idle,
            "the app is stuck mid-turn with no turn to end it"
        );
    }

    /// Accepting the picker mid-stream reaches the reclaim path with a
    /// real turn still owned by the event loop. Forcing Idle there would
    /// send Ctrl+C down the quit path instead of cancelling the turn.
    #[test]
    fn reclaiming_does_not_unstick_a_genuinely_live_turn() {
        let mut app = App::new("m", "/tmp", "s");
        app.mark_turn_started();
        assert!(app.turn_live, "fixture did not start a turn");
        app.pending_submit = Some("staged".into());

        app.adopt_restored_session(&restored());

        assert!(app.turn_live, "a live turn was marked dead");
        assert_ne!(
            app.phase,
            Phase::Idle,
            "Ctrl+C would now quit instead of cancelling the running turn"
        );
    }

    /// ...but a HITL modal still owns the phase.
    #[test]
    fn reclaiming_does_not_steal_the_phase_from_a_modal() {
        let mut app = App::new("m", "/tmp", "s");
        let _rx = a_modal(&mut app);
        app.pending_submit = Some("staged".into());
        app.adopt_restored_session(&restored());
        assert_eq!(app.phase, Phase::Permission, "the modal lost its phase");
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
