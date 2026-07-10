//! Channel-backed [`StreamSink`] for the modern TUI.
//!
//! The engine turn runs on a detached task (`Session::spawn_turn`). The
//! sink never draws — it only pushes structured events onto an
//! unbounded channel that the TUI event loop drains each frame.

use std::sync::Arc;

use agent_code_lib::llm::message::Usage;
use agent_code_lib::query::StreamSink;
use agent_code_lib::tools::ToolResult;
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
}
