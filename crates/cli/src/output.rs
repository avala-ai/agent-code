//! Structured JSONL output for non-interactive / CI mode.
//!
//! `--output-format json` writes one JSON object per line to stdout.
//! Human-readable status messages go to stderr so downstream
//! consumers can pipe stdout directly into `jq` or a processing
//! script.
//!
//! ```text
//! agent -p "fix tests" --output-format json \
//!   | jq 'select(.type == "tool_call")'
//! ```

use std::io::Write;
use std::sync::Mutex;

use agent_code_lib::llm::message::Usage;
use agent_code_lib::query::StreamSink;
use serde::Serialize;

/// Output format for one-shot mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unknown output format: {other} (expected text or json)"
            )),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
            Self::Json => write!(f, "json"),
        }
    }
}

/// Exit codes for non-interactive mode (ROADMAP 7.7).
#[repr(u8)]
pub enum ExitCode {
    Success = 0,
    ConfigError = 1,
    InputError = 2,
    ToolFailure = 3,
    LlmError = 4,
    CostLimit = 5,
    TurnLimit = 6,
    PermissionDenied = 7,
}

/// Envelope event written as a single JSONL line to stdout.
#[derive(Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum Event<'a> {
    SessionStart {
        session_id: &'a str,
        model: &'a str,
        timestamp: &'a str,
    },
    /// A new agent turn has just begun. Fires BEFORE any text or tool
    /// activity for this turn, so consumers can render a "turn N"
    /// progress indicator without waiting for TurnComplete.
    TurnStart {
        turn: usize,
    },
    TextDelta {
        content: &'a str,
        turn: usize,
    },
    Thinking {
        content: &'a str,
        turn: usize,
    },
    ToolCall {
        tool: &'a str,
        input: &'a serde_json::Value,
        turn: usize,
    },
    ToolResult {
        tool: &'a str,
        output: &'a str,
        is_error: bool,
        turn: usize,
    },
    TurnComplete {
        turn: usize,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    },
    Error {
        message: &'a str,
        turn: usize,
    },
    /// Non-fatal warning surfaced by the engine (budget approaching a
    /// limit, rate-limit backoff, schema fallback, etc.). JSONL consumers
    /// need these on stdout so they can tag runs without re-parsing
    /// stderr.
    Warning {
        message: &'a str,
        turn: usize,
    },
    /// Autocompaction just freed context. The human stderr line still
    /// fires — this event lets consumers surface cost/context shifts
    /// in dashboards and audit logs.
    Compact {
        freed_tokens: u64,
        turn: usize,
    },
    SessionEnd {
        turns: usize,
        total_cost_usd: f64,
        exit_code: u8,
    },
}

/// Internal mutable state shared across sink callbacks.
struct SinkState {
    turn: usize,
    last_usage: Usage,
}

/// A [`StreamSink`] that writes JSONL events to stdout.
///
/// Status/warning messages go to stderr so the JSONL stream on
/// stdout remains machine-parseable.
pub struct JsonStreamSink {
    model: String,
    inner: Mutex<SinkState>,
}

impl JsonStreamSink {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            inner: Mutex::new(SinkState {
                turn: 0,
                last_usage: Usage::default(),
            }),
        }
    }

    /// Emit the `session_start` envelope before the first turn.
    pub fn emit_session_start(&self, session_id: &str) {
        let ts = chrono::Utc::now().to_rfc3339();
        emit(&Event::SessionStart {
            session_id,
            model: &self.model,
            timestamp: &ts,
        });
    }

    /// Emit the `session_end` envelope after the run completes.
    pub fn emit_session_end(&self, total_cost_usd: f64, exit_code: u8) {
        let state = self.inner.lock().unwrap();
        emit(&Event::SessionEnd {
            turns: state.turn,
            total_cost_usd,
            exit_code,
        });
    }
}

impl StreamSink for JsonStreamSink {
    fn on_turn_start(&self, turn: usize) {
        self.inner.lock().unwrap().turn = turn;
        // Emit after updating state so downstream consumers see a
        // turn_start line carrying the new turn number. TurnComplete
        // already fires at the end; this is the matching bookend.
        emit(&Event::TurnStart { turn });
    }

    fn on_text(&self, text: &str) {
        let turn = self.inner.lock().unwrap().turn;
        emit(&Event::TextDelta {
            content: text,
            turn,
        });
    }

    fn on_thinking(&self, text: &str) {
        let turn = self.inner.lock().unwrap().turn;
        emit(&Event::Thinking {
            content: text,
            turn,
        });
    }

    fn on_tool_start(&self, tool_name: &str, input: &serde_json::Value) {
        let state = self.inner.lock().unwrap();
        emit(&Event::ToolCall {
            tool: tool_name,
            input,
            turn: state.turn,
        });
    }

    fn on_tool_result(&self, tool_name: &str, result: &agent_code_lib::tools::ToolResult) {
        let state = self.inner.lock().unwrap();
        emit(&Event::ToolResult {
            tool: tool_name,
            output: &result.content,
            is_error: result.is_error,
            turn: state.turn,
        });
    }

    fn on_usage(&self, usage: &Usage) {
        let mut state = self.inner.lock().unwrap();
        state.last_usage = usage.clone();
    }

    fn on_turn_complete(&self, turn: usize) {
        let mut state = self.inner.lock().unwrap();
        state.turn = turn;
        let cost = crate::estimate_model_cost(&state.last_usage, &self.model);
        emit(&Event::TurnComplete {
            turn,
            input_tokens: state.last_usage.input_tokens,
            output_tokens: state.last_usage.output_tokens,
            cost_usd: cost,
        });
    }

    fn on_error(&self, error: &str) {
        let turn = self.inner.lock().unwrap().turn;
        emit(&Event::Error {
            message: error,
            turn,
        });
    }

    fn on_warning(&self, msg: &str) {
        // Stderr stays for humans tailing the run; stdout JSONL is for
        // automation consumers that would otherwise lose budget /
        // rate-limit signals entirely.
        let turn = self.inner.lock().unwrap().turn;
        emit(&Event::Warning { message: msg, turn });
        let _ = writeln!(std::io::stderr(), "{msg}");
    }

    fn on_compact(&self, freed_tokens: u64) {
        let turn = self.inner.lock().unwrap().turn;
        emit(&Event::Compact { freed_tokens, turn });
        let _ = writeln!(std::io::stderr(), "compacted: freed ~{freed_tokens} tokens");
    }
}

/// Write a single JSONL event to stdout (locked, flushed).
fn emit(event: &Event<'_>) {
    if let Ok(line) = serde_json::to_string(event) {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = writeln!(lock, "{line}");
        let _ = lock.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_format_parse() {
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("text".parse::<OutputFormat>().unwrap(), OutputFormat::Text);
        assert_eq!("JSON".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert!("xml".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn event_serialization_text_delta() {
        let event = Event::TextDelta {
            content: "hello",
            turn: 1,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"text_delta""#));
        assert!(json.contains(r#""content":"hello""#));
        assert!(json.contains(r#""turn":1"#));
    }

    #[test]
    fn event_serialization_session_start() {
        let event = Event::SessionStart {
            session_id: "abc-123",
            model: "test-model",
            timestamp: "2026-04-15T00:00:00Z",
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"session_start""#));
        assert!(json.contains(r#""session_id":"abc-123""#));
    }

    #[test]
    fn event_serialization_tool_call() {
        let input = serde_json::json!({"file_path": "test.rs", "content": "fn main() {}"});
        let event = Event::ToolCall {
            tool: "FileWrite",
            input: &input,
            turn: 2,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_call""#));
        assert!(json.contains(r#""tool":"FileWrite""#));
        assert!(json.contains(r#""file_path":"test.rs""#));
    }

    #[test]
    fn event_serialization_session_end() {
        let event = Event::SessionEnd {
            turns: 3,
            total_cost_usd: 0.042,
            exit_code: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"session_end""#));
        assert!(json.contains(r#""exit_code":0"#));
    }

    #[test]
    fn event_serialization_turn_complete() {
        let event = Event::TurnComplete {
            turn: 1,
            input_tokens: 1234,
            output_tokens: 567,
            cost_usd: 0.003,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"turn_complete""#));
        assert!(json.contains(r#""input_tokens":1234"#));
    }

    #[test]
    fn event_serialization_error() {
        let event = Event::Error {
            message: "rate limited",
            turn: 1,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"error""#));
        assert!(json.contains(r#""message":"rate limited""#));
    }

    #[test]
    fn all_events_are_single_line_json() {
        let events: Vec<serde_json::Value> = vec![
            serde_json::to_value(Event::SessionStart {
                session_id: "x",
                model: "m",
                timestamp: "t",
            })
            .unwrap(),
            serde_json::to_value(Event::TurnStart { turn: 1 }).unwrap(),
            serde_json::to_value(Event::TextDelta {
                content: "multi\nline\ncontent",
                turn: 1,
            })
            .unwrap(),
            serde_json::to_value(Event::SessionEnd {
                turns: 1,
                total_cost_usd: 0.0,
                exit_code: 0,
            })
            .unwrap(),
            serde_json::to_value(Event::Warning {
                message: "budget exceeded\nsecond line",
                turn: 2,
            })
            .unwrap(),
            serde_json::to_value(Event::Compact {
                freed_tokens: 1500,
                turn: 3,
            })
            .unwrap(),
        ];
        for val in events {
            let line = serde_json::to_string(&val).unwrap();
            assert!(!line.contains('\n'), "event must be single-line: {line}",);
        }
    }

    #[test]
    fn event_serialization_turn_start_uses_snake_case_type() {
        let event = Event::TurnStart { turn: 5 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"turn_start""#));
        assert!(json.contains(r#""turn":5"#));
    }

    #[test]
    fn turn_start_event_does_not_include_model_or_session_fields() {
        // TurnStart is intentionally small — just the turn number. If
        // a field accidentally slips in (e.g. model), JSONL consumers
        // that snapshot the envelope shape will start silently failing.
        let event = Event::TurnStart { turn: 1 };
        let val = serde_json::to_value(event).unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj.len(), 2, "expected only `type` and `turn`, got {obj:?}");
        assert!(obj.contains_key("type"));
        assert!(obj.contains_key("turn"));
    }

    #[test]
    fn event_serialization_warning_uses_snake_case_type() {
        let event = Event::Warning {
            message: "budget approaching",
            turn: 4,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"warning""#));
        assert!(json.contains(r#""message":"budget approaching""#));
        assert!(json.contains(r#""turn":4"#));
    }

    #[test]
    fn event_serialization_compact_uses_snake_case_type() {
        let event = Event::Compact {
            freed_tokens: 2048,
            turn: 7,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"compact""#));
        assert!(json.contains(r#""freed_tokens":2048"#));
        assert!(json.contains(r#""turn":7"#));
    }

    #[test]
    fn warning_event_preserves_turn_from_sink_state() {
        // Warnings need the current turn stamped in the envelope so
        // consumers can correlate them with the surrounding TurnComplete.
        // This is a structural check — the sink reads `inner.turn` under
        // the same lock that on_turn_start writes to.
        let sink = JsonStreamSink::new("test-model");
        sink.on_turn_start(9);
        // We can't easily capture the stdout line from here without a
        // test scaffold; assert the internal state is what on_warning
        // will read instead.
        let captured_turn = sink.inner.lock().unwrap().turn;
        assert_eq!(captured_turn, 9);
    }
}
