//! Channel-backed [`StreamSink`] for the modern TUI.
//!
//! The engine turn runs on a detached task (`Session::spawn_turn`). The
//! sink never draws — it only pushes structured events onto an
//! unbounded channel that the TUI event loop drains each frame.

use std::sync::Arc;

use agent_code_lib::llm::message::Usage;
use agent_code_lib::query::StreamSink;
use agent_code_lib::services::output_store::TRUNCATION_NOTICE_PREFIX;
use agent_code_lib::tools::{
    PermissionPrompter, PermissionResponse, QuestionAsker, ToolResult, UserQuestion,
};
use tokio::sync::mpsc;

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
    /// A finished subagent's full output, for pane drill-in.
    ///
    /// `call_id` is the Agent tool-call id. Two calls can share an
    /// `agent_id` (it is derived from the description), so the call id is
    /// what keeps their results apart.
    SubagentOutput {
        agent_id: String,
        call_id: String,
        output: String,
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

/// Sink that forwards every stream callback onto `tx`.
pub struct ChannelSink {
    tx: mpsc::UnboundedSender<EngineEvent>,
}

impl ChannelSink {
    pub fn new(tx: mpsc::UnboundedSender<EngineEvent>) -> Arc<Self> {
        Arc::new(Self { tx })
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

    fn on_subagent_output(&self, agent_id: &str, call_id: &str, output: &str) {
        self.send(EngineEvent::SubagentOutput {
            agent_id: agent_id.to_string(),
            call_id: call_id.to_string(),
            output: bound_subagent_output(output, SUBAGENT_OUTPUT_MAX_CHARS),
        });
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

/// Ceiling on a captured subagent body pushed through the UI channel, so
/// a runaway subagent cannot flood it.
const SUBAGENT_OUTPUT_MAX_CHARS: usize = 64 * 1024;

/// Bound a captured subagent body for the UI, keeping its truncation
/// notice.
///
/// The engine may already have replaced a large result with a preview
/// plus an `(Output truncated … saved to <path>)` notice, and that notice
/// is the only pointer to the full result. A plain prefix bound cuts the
/// suffix off — for an ASCII body the preview alone already fills the
/// budget — leaving drill-in showing an incomplete body with no way to
/// reach the rest. Trim the preview instead and keep the notice.
///
/// A body that overflows without an upstream notice (an oversized
/// synthetic Agent error never reaches `persist_if_large`) gets the
/// UI's own marker instead. Either suffix is reserved out of `max`
/// before the preview is taken, so the result stays within the ceiling
/// whenever the suffix itself fits.
fn bound_subagent_output(output: &str, max: usize) -> String {
    let total = output.chars().count();
    if total <= max {
        return output.to_string();
    }
    let notice = output
        .rfind(TRUNCATION_NOTICE_PREFIX)
        .map(|idx| &output[idx..])
        .unwrap_or_default();
    let suffix = if notice.is_empty() {
        format!("\n\n(Output truncated for display: {total} characters total)")
    } else {
        notice.to_string()
    };
    let mut bounded: String = output[..output.len() - notice.len()]
        .chars()
        .take(max.saturating_sub(suffix.chars().count()))
        .collect();
    bounded.push_str(&suffix);
    bounded
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

    /// The bug: the engine persists an oversized result as a 64 KiB
    /// preview plus a notice naming the file holding the whole thing.
    /// Bounding the body to exactly that budget dropped the notice, so
    /// drill-in showed a clipped prefix and no route to the rest.
    #[test]
    fn bounding_keeps_the_engine_truncation_notice() {
        let notice =
            format!("{TRUNCATION_NOTICE_PREFIX}. Full result (999999 bytes) saved to /t/x.txt)");
        let engine_result = format!("{}{notice}", "a".repeat(SUBAGENT_OUTPUT_MAX_CHARS));

        let bounded = bound_subagent_output(&engine_result, SUBAGENT_OUTPUT_MAX_CHARS);

        assert!(
            bounded.ends_with(&notice),
            "the truncation notice was cut off, losing the path to the full result"
        );
        assert!(
            bounded.chars().count() <= SUBAGENT_OUTPUT_MAX_CHARS,
            "bound exceeded: {} chars",
            bounded.chars().count()
        );
        assert!(bounded.starts_with("aaa"), "the preview itself was dropped");
    }

    #[test]
    fn bounding_leaves_a_body_within_budget_untouched() {
        let body = "found the bug in auth.rs";
        assert_eq!(bound_subagent_output(body, SUBAGENT_OUTPUT_MAX_CHARS), body);
    }

    /// Nothing upstream truncated, but the body still overflows the UI
    /// budget: say so rather than clipping silently. The marker is paid
    /// for out of the budget, not added on top of it — an oversized
    /// synthetic Agent error never passes through `persist_if_large`, so
    /// this is the path that actually holds the ceiling.
    #[test]
    fn an_unmarked_overflow_is_marked_within_the_budget() {
        let bounded = bound_subagent_output(&"b".repeat(200), 100);
        assert!(
            bounded.chars().count() <= 100,
            "the display marker pushed the body past the ceiling: {} chars",
            bounded.chars().count()
        );
        assert!(bounded.starts_with("bbb"), "the preview itself was dropped");
        assert!(bounded.contains("truncated for display"));
        assert!(bounded.contains("200 characters total"));
    }

    /// Degenerate budget, unreachable with the real ceiling: the notice
    /// is the only pointer to the full result, so it is kept whole even
    /// when it alone exceeds the budget and nothing of the preview fits.
    #[test]
    fn a_tiny_budget_still_keeps_the_notice() {
        let notice = format!("{TRUNCATION_NOTICE_PREFIX}: 999999 bytes total)");
        let bounded = bound_subagent_output(&format!("{}{notice}", "c".repeat(50)), 4);
        assert_eq!(bounded, notice);
    }

    #[test]
    fn subagent_output_reaches_the_ui_with_its_notice() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        let notice = format!("{TRUNCATION_NOTICE_PREFIX}: 999999 bytes total)");
        sink.on_subagent_output(
            "explorer",
            "toolu_01",
            &format!("{}{notice}", "d".repeat(SUBAGENT_OUTPUT_MAX_CHARS)),
        );
        match rx.try_recv().unwrap() {
            EngineEvent::SubagentOutput {
                agent_id,
                call_id,
                output,
            } => {
                assert_eq!(agent_id, "explorer");
                assert_eq!(call_id, "toolu_01", "the per-call identity was dropped");
                assert!(output.ends_with(&notice));
            }
            other => panic!("unexpected {other:?}"),
        }
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
