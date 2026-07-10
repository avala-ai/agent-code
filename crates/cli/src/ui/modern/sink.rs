//! Channel-backed [`StreamSink`] for the modern TUI.
//!
//! The engine turn runs on a detached task (`Session::spawn_turn`). The
//! sink never draws — it only pushes structured events onto an
//! unbounded channel that the TUI event loop drains each frame.

use std::sync::Arc;

use agent_code_lib::llm::message::Usage;
use agent_code_lib::query::StreamSink;
use agent_code_lib::tools::{PermissionPrompter, PermissionResponse, ToolResult};
use tokio::sync::mpsc;

/// Events emitted by the engine toward the UI.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    Text(String),
    Thinking(String),
    ToolStart {
        name: String,
        detail: String,
    },
    ToolResult {
        name: String,
        content: String,
        is_error: bool,
    },
    TurnStart(usize),
    TurnComplete(usize),
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
        input_preview: Option<String>,
        respond: std::sync::mpsc::Sender<PermissionResponse>,
    },
}

/// Permission prompter that surfaces engine asks inside the TUI.
///
/// The classic REPL prompts on stdin; the modern TUI owns the terminal in
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
        let description = match origin {
            Some(o) if !o.is_empty() => format!("{description} (from {o})"),
            _ => description.to_string(),
        };
        let sent = self.tx.send(EngineEvent::PermissionAsk {
            name: tool_name.to_string(),
            description,
            input_preview: input_preview.map(str::to_string),
            respond: resp_tx,
        });
        if sent.is_err() {
            return PermissionResponse::Deny;
        }
        resp_rx.recv().unwrap_or(PermissionResponse::Deny)
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
        let detail = tool_detail(tool_name, input);
        self.send(EngineEvent::ToolStart {
            name: tool_name.to_string(),
            detail,
        });
    }

    fn on_tool_result(&self, tool_name: &str, result: &ToolResult) {
        let content: String = result.content.chars().take(4_000).collect();
        self.send(EngineEvent::ToolResult {
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
