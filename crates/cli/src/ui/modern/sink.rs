//! Channel-backed [`StreamSink`] for the modern TUI.
//!
//! The engine turn runs on a detached task (`Session::spawn_turn`). The
//! sink never draws — it only pushes structured events onto an
//! unbounded channel that the TUI event loop drains each frame.

use std::sync::Arc;

use agent_code_lib::llm::message::Usage;
use agent_code_lib::query::StreamSink;
use agent_code_lib::tools::{
    PermissionPrompter, PermissionResponse, QuestionAsker, ToolResult, UserQuestion,
};
use tokio::sync::mpsc;

/// One checklist entry as it crosses the sink: `(id, content, status)`,
/// still the raw strings `TodoWrite` supplied. Interpreting `status` is
/// the UI's job (`TodoStatus::parse`).
pub type TodoFields = (String, String, String);

/// Validated checklists waiting for their tool result, keyed by call id.
type PendingTodos = Vec<(String, Vec<TodoFields>)>;

/// A question relayed to the UI (flattened from the engine's `UserQuestion`).
#[derive(Debug, Clone)]
pub struct UiQuestion {
    pub question: String,
    pub options: Vec<String>,
}

/// Events emitted by the engine toward the UI.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    Text(String),
    Thinking(String),
    /// The model's current checklist, parsed from a `TodoWrite` call.
    ///
    /// Carried as its own event because `ToolStart` forwards only a
    /// display string; the checklist needs the structured items.
    TodoUpdate {
        items: Vec<TodoFields>,
    },
    ToolStart {
        /// Stable engine tool-call id, used to correlate the result card.
        /// Empty only if the engine emitted the legacy id-less callback.
        call_id: String,
        name: String,
        detail: String,
    },
    ToolResult {
        /// Matches the `call_id` of the originating [`EngineEvent::ToolStart`].
        call_id: String,
        name: String,
        content: String,
        is_error: bool,
    },
    /// Progressive tool stdout/stderr (bash live-tail). Correlated by `call_id`.
    ToolOutput {
        call_id: String,
        chunk: String,
    },
    TurnStart(usize),
    TurnComplete(usize),
    /// Terminal outcome for a finished turn (`error` / `cancelled` / `max_turns` / …).
    TurnOutcome {
        turn: usize,
        outcome: String,
    },
    Error(String),
    Warning(String),
    Usage {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    },
    Compact {
        freed: u64,
    },
    /// Running context-window meter from the engine (plan §3.4.4). The UI
    /// never re-scans the transcript to compute this.
    ContextUsage {
        used: u64,
        max: u64,
    },
    /// Background / typed subagent lifecycle update for the tasks pane (M8).
    SubagentUpdate {
        agent_id: String,
        state: String,
        headline: String,
    },
    /// A tool call needs interactive permission. The turn task is blocked
    /// until a [`PermissionResponse`] is sent back on `respond` (dropping
    /// it counts as deny).
    PermissionAsk {
        name: String,
        description: String,
        /// Who triggered the ask (e.g. a subagent id), kept typed so the UI
        /// renders it distinctly instead of splicing it into `description`.
        origin: Option<String>,
        input_preview: Option<String>,
        respond: std::sync::mpsc::Sender<PermissionResponse>,
    },
    /// A plan was proposed via ExitPlanMode (plan §M6). Fire-and-forget:
    /// the UI shows it for review; the agent has already exited plan mode.
    PlanProposed {
        plan_md: String,
        path: Option<String>,
    },
    /// The agent asked the user a multiple-choice question (AskUserQuestion).
    /// The turn task blocks until one label per question is sent on `respond`
    /// (dropping it fails the tool, per the QuestionAsker contract).
    QuestionAsk {
        questions: Vec<UiQuestion>,
        respond: std::sync::mpsc::Sender<Vec<String>>,
    },
}

/// Permission prompter that surfaces engine asks inside the TUI.
///
/// Headless and tests may prompt on stdin; the modern TUI owns the terminal in
/// raw mode, so `ask()` instead pushes a [`EngineEvent::PermissionAsk`]
/// onto the event channel and blocks the turn task until the event loop
/// answers. Fails **closed**: if the UI is gone (channel closed or the
/// responder dropped), the tool call is denied — without this prompter the
/// executor's `None` default silently auto-allows every ask.
pub struct ModernPrompter {
    tx: mpsc::UnboundedSender<EngineEvent>,
}

impl ModernPrompter {
    pub fn new(tx: mpsc::UnboundedSender<EngineEvent>) -> Arc<Self> {
        Arc::new(Self { tx })
    }
}

impl PermissionPrompter for ModernPrompter {
    fn ask(
        &self,
        tool_name: &str,
        description: &str,
        input_preview: Option<&str>,
        origin: Option<&str>,
    ) -> PermissionResponse {
        let (resp_tx, resp_rx) = std::sync::mpsc::channel();
        let sent = self.tx.send(EngineEvent::PermissionAsk {
            name: tool_name.to_string(),
            description: description.to_string(),
            origin: origin.filter(|o| !o.is_empty()).map(str::to_string),
            input_preview: input_preview.map(str::to_string),
            respond: resp_tx,
        });
        if sent.is_err() {
            return PermissionResponse::Deny;
        }
        resp_rx.recv().unwrap_or(PermissionResponse::Deny)
    }
}

/// Question asker that surfaces `AskUserQuestion` as a UI modal instead of
/// blocking on stdin (which would hang under the alt-screen raw mode). Like
/// [`ModernPrompter`] it blocks the turn task on a response channel and
/// fails closed if the UI is gone.
pub struct ModernQuestionAsker {
    tx: mpsc::UnboundedSender<EngineEvent>,
}

impl ModernQuestionAsker {
    pub fn new(tx: mpsc::UnboundedSender<EngineEvent>) -> Arc<Self> {
        Arc::new(Self { tx })
    }
}

impl QuestionAsker for ModernQuestionAsker {
    fn ask(&self, questions: &[UserQuestion]) -> Result<Vec<String>, String> {
        let ui: Vec<UiQuestion> = questions
            .iter()
            .map(|q| UiQuestion {
                question: q.question.clone(),
                options: q.options.iter().map(|o| o.label.clone()).collect(),
            })
            .collect();
        let (resp_tx, resp_rx) = std::sync::mpsc::channel();
        self.tx
            .send(EngineEvent::QuestionAsk {
                questions: ui,
                respond: resp_tx,
            })
            .map_err(|_| "UI closed".to_string())?;
        resp_rx.recv().map_err(|_| "no answer".to_string())
    }
}

/// How many validated-but-unresolved checklists to hold. A cancelled
/// turn can leave a tool start with no matching result, so the buffer is
/// capped rather than trusted to drain.
const MAX_PENDING_TODOS: usize = 8;

/// Sink that forwards every stream callback onto `tx`.
pub struct ChannelSink {
    tx: mpsc::UnboundedSender<EngineEvent>,
    /// Validated `TodoWrite` checklists waiting for their tool result,
    /// keyed by call id. See [`ChannelSink::on_tool_call_start`].
    pending_todos: std::sync::Mutex<PendingTodos>,
}

impl ChannelSink {
    pub fn new(tx: mpsc::UnboundedSender<EngineEvent>) -> Arc<Self> {
        Arc::new(Self {
            tx,
            pending_todos: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn send(&self, ev: EngineEvent) {
        let _ = self.tx.send(ev);
    }
}

impl StreamSink for ChannelSink {
    fn on_text(&self, text: &str) {
        if !text.is_empty() {
            self.send(EngineEvent::Text(text.to_string()));
        }
    }

    fn on_thinking(&self, text: &str) {
        if !text.is_empty() {
            self.send(EngineEvent::Thinking(text.to_string()));
        }
    }

    fn on_tool_start(&self, tool_name: &str, input: &serde_json::Value) {
        // Legacy id-less path: the engine calls `on_tool_call_start` instead,
        // so this only fires for sinks/tests using the base callback. Empty
        // `call_id` makes the UI fall back to oldest-pending correlation.
        self.on_tool_call_start("", tool_name, input);
    }

    fn on_tool_result(&self, tool_name: &str, result: &ToolResult) {
        self.on_tool_call_result("", tool_name, result);
    }

    fn on_tool_call_start(&self, call_id: &str, tool_name: &str, input: &serde_json::Value) {
        // The checklist is the point of a TodoWrite call, so surface it
        // as state rather than leaving it buried in a tool card. It is
        // only parsed here — publishing it waits for the tool result,
        // because this callback runs ahead of both input validation and
        // the permission check, and a call the user denies must not get
        // to redescribe the active plan.
        //
        // Entries must carry the three fields TodoWrite declares
        // required, each a string. Filling missing ones with empty
        // strings would let a malformed call blank the pane into pending
        // rows describing work the model never wrote; one bad entry
        // rejects the batch, since a partial checklist misdescribes the
        // plan as badly as an empty one.
        //
        // The `status` *value* is deliberately not checked against the
        // schema's enum: `TodoStatus::parse` already treats anything it
        // does not recognise as pending, and dropping a whole update
        // over one unexpected word would strand the pane on stale work.
        if tool_name == "TodoWrite"
            && let Some(todos) = input.get("todos").and_then(|v| v.as_array())
        {
            let items: Option<Vec<TodoFields>> = todos
                .iter()
                .map(|t| {
                    let field = |k: &str| t.get(k)?.as_str().map(str::to_string);
                    Some((field("id")?, field("content")?, field("status")?))
                })
                .collect();
            if let Some(items) = items {
                let mut pending = self
                    .pending_todos
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                pending.retain(|(id, _)| id != call_id);
                pending.push((call_id.to_string(), items));
                let overflow = pending.len().saturating_sub(MAX_PENDING_TODOS);
                pending.drain(..overflow);
            }
        }
        let detail = tool_detail(tool_name, input);
        self.send(EngineEvent::ToolStart {
            call_id: call_id.to_string(),
            name: tool_name.to_string(),
            detail,
        });
    }

    fn on_tool_call_result(&self, call_id: &str, tool_name: &str, result: &ToolResult) {
        let content: String = result.content.chars().take(4_000).collect();
        self.send(EngineEvent::ToolResult {
            call_id: call_id.to_string(),
            name: tool_name.to_string(),
            content,
            is_error: result.is_error,
        });
        // Now the checklist parsed at tool-start is safe to publish: only
        // a call that actually ran describes the plan the model is
        // working to. A permission denial or an input rejection arrives
        // here as an error result, and leaves the previous list standing.
        // Either way the entry is dropped, so it cannot be published by a
        // later call.
        if tool_name == "TodoWrite" {
            let parsed = {
                let mut pending = self
                    .pending_todos
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                pending
                    .iter()
                    .position(|(id, _)| id == call_id)
                    .map(|i| pending.remove(i).1)
            };
            if let Some(items) = parsed
                && !result.is_error
            {
                self.send(EngineEvent::TodoUpdate { items });
            }
        }
    }

    fn on_turn_start(&self, turn: usize) {
        self.send(EngineEvent::TurnStart(turn));
    }

    fn on_turn_complete(&self, turn: usize) {
        self.send(EngineEvent::TurnComplete(turn));
    }

    fn on_tool_output(&self, call_id: &str, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        self.send(EngineEvent::ToolOutput {
            call_id: call_id.to_string(),
            chunk: chunk.to_string(),
        });
    }

    fn on_turn_outcome(&self, turn: usize, outcome: &str) {
        self.send(EngineEvent::TurnOutcome {
            turn,
            outcome: outcome.to_string(),
        });
    }

    fn on_error(&self, error: &str) {
        self.send(EngineEvent::Error(error.to_string()));
    }

    fn on_warning(&self, msg: &str) {
        self.send(EngineEvent::Warning(msg.to_string()));
    }

    fn on_usage(&self, usage: &Usage) {
        self.send(EngineEvent::Usage {
            input: usage.input_tokens,
            output: usage.output_tokens,
            cache_read: usage.cache_read_input_tokens,
            cache_write: usage.cache_creation_input_tokens,
        });
    }

    fn on_compact(&self, freed_tokens: u64) {
        self.send(EngineEvent::Compact {
            freed: freed_tokens,
        });
    }

    fn on_context_usage(&self, used: u64, max: u64) {
        self.send(EngineEvent::ContextUsage { used, max });
    }

    fn on_subagent_update(&self, agent_id: &str, state: &str, headline: &str) {
        self.send(EngineEvent::SubagentUpdate {
            agent_id: agent_id.to_string(),
            state: state.to_string(),
            headline: headline.to_string(),
        });
    }

    fn on_plan_proposed(&self, plan_md: &str, path: Option<&str>) {
        self.send(EngineEvent::PlanProposed {
            plan_md: plan_md.to_string(),
            path: path.map(str::to_string),
        });
    }
}

fn tool_detail(name: &str, input: &serde_json::Value) -> String {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input.get(*k).and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string()
    };
    let raw = match name {
        "Bash" | "PowerShell" => pick(&["command"]),
        "FileRead" | "FileWrite" | "FileEdit" | "MultiEdit" => pick(&["file_path", "path"]),
        "Grep" => pick(&["pattern"]),
        "Glob" => pick(&["pattern"]),
        "Agent" => pick(&["description"]),
        "WebFetch" => pick(&["url"]),
        "WebSearch" => pick(&["query"]),
        _ => input
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str()))
            .unwrap_or("")
            .to_string(),
    };
    if raw.chars().count() > 72 {
        format!("{}…", raw.chars().take(71).collect::<String>())
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_forwards_text() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        sink.on_text("hello");
        match rx.try_recv().unwrap() {
            EngineEvent::Text(t) => assert_eq!(t, "hello"),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// `on_tool_call_start` runs before the engine validates the call and
    /// before the user approves it, so a malformed `TodoWrite` must not
    /// be allowed to overwrite a good checklist. Filling the missing
    /// fields with empty strings would blank the pane into pending rows
    /// describing work the model never wrote.
    #[test]
    fn a_malformed_todo_write_leaves_the_checklist_alone() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);

        let todos = |v: serde_json::Value| serde_json::json!({ "todos": v });
        let rejected = [
            // Missing each required field in turn.
            serde_json::json!([{ "content": "a", "status": "pending" }]),
            serde_json::json!([{ "id": "1", "status": "pending" }]),
            serde_json::json!([{ "id": "1", "content": "a" }]),
            // Right keys, wrong types.
            serde_json::json!([{ "id": 1, "content": "a", "status": "pending" }]),
            serde_json::json!([{ "id": "1", "content": null, "status": "pending" }]),
            // One bad entry poisons the batch: a partial checklist would
            // misdescribe the plan just as badly as a blank one.
            serde_json::json!([
                { "id": "1", "content": "a", "status": "done" },
                { "id": "2", "status": "pending" },
            ]),
            // Not objects at all.
            serde_json::json!(["just a string"]),
        ];
        // Drives a whole call and reports whether the checklist moved.
        let run = |rx: &mut mpsc::UnboundedReceiver<EngineEvent>,
                   input: &serde_json::Value,
                   is_error: bool| {
            sink.on_tool_call_start("c1", "TodoWrite", input);
            let mut result = ToolResult::success("Todo list (1 items):\n[ ] 1: a");
            result.is_error = is_error;
            sink.on_tool_call_result("c1", "TodoWrite", &result);
            std::iter::from_fn(|| rx.try_recv().ok())
                .any(|ev| matches!(ev, EngineEvent::TodoUpdate { .. }))
        };

        for input in rejected {
            assert!(
                !run(&mut rx, &todos(input.clone()), false),
                "malformed input replaced the checklist: {input}"
            );
        }

        // A well-formed call lands, and an empty list is a valid "no
        // plan" signal rather than a malformed one.
        let valid = todos(serde_json::json!([
            { "id": "1", "content": "a", "status": "pending" }
        ]));
        assert!(run(&mut rx, &valid, false), "valid input was dropped");
        assert!(
            run(&mut rx, &todos(serde_json::json!([])), false),
            "an empty checklist should be publishable"
        );

        // A denied or rejected call never ran, so it does not get to
        // redescribe the plan — the previous checklist stands.
        assert!(
            !run(&mut rx, &valid, true),
            "an errored TodoWrite still replaced the checklist"
        );
    }

    /// Publication waits for the result, so a start with no result must
    /// not leave the parsed list sitting where a later call could
    /// publish it — nor accumulate without bound across a long session
    /// of cancelled turns.
    #[test]
    fn an_unresolved_todo_write_is_never_published_and_is_bounded() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        let input = serde_json::json!({
            "todos": [{ "id": "1", "content": "a", "status": "pending" }]
        });

        // Starts that never resolve (a cancelled turn) publish nothing.
        for i in 0..MAX_PENDING_TODOS * 3 {
            sink.on_tool_call_start(&format!("c{i}"), "TodoWrite", &input);
        }
        let published = std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|ev| matches!(ev, EngineEvent::TodoUpdate { .. }))
            .count();
        assert_eq!(published, 0, "an unresolved start published a checklist");
        assert!(
            sink.pending_todos.lock().unwrap().len() <= MAX_PENDING_TODOS,
            "the pending buffer grew without bound"
        );

        // The evicted ids are gone for good: a late result cannot revive
        // one and publish a checklist from an abandoned turn.
        sink.on_tool_call_result("c0", "TodoWrite", &ToolResult::success("ok"));
        let revived = std::iter::from_fn(|| rx.try_recv().ok())
            .any(|ev| matches!(ev, EngineEvent::TodoUpdate { .. }));
        assert!(!revived, "an evicted checklist was published late");
    }

    #[test]
    fn prompter_denies_when_ui_gone() {
        let (tx, rx) = mpsc::unbounded_channel();
        let prompter = ModernPrompter::new(tx);
        drop(rx);
        let resp = prompter.ask("Bash", "run", None, None);
        assert!(matches!(resp, PermissionResponse::Deny));
    }

    #[test]
    fn prompter_denies_when_responder_dropped() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let prompter = ModernPrompter::new(tx);
        let worker = std::thread::spawn(move || prompter.ask("Bash", "run", None, None));
        // Receive the ask, then drop it (and its responder) unanswered.
        let ev = loop {
            match rx.try_recv() {
                Ok(ev) => break ev,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        };
        drop(ev);
        assert!(matches!(worker.join().unwrap(), PermissionResponse::Deny));
    }

    #[test]
    fn prompter_returns_users_answer() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let prompter = ModernPrompter::new(tx);
        let worker =
            std::thread::spawn(move || prompter.ask("Bash", "cargo test", Some("{}"), None));
        let ev = loop {
            match rx.try_recv() {
                Ok(ev) => break ev,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        };
        match ev {
            EngineEvent::PermissionAsk { name, respond, .. } => {
                assert_eq!(name, "Bash");
                respond.send(PermissionResponse::AllowSession).unwrap();
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            worker.join().unwrap(),
            PermissionResponse::AllowSession
        ));
    }
}
