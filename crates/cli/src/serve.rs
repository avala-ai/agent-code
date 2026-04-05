//! Headless HTTP server mode.
//!
//! Runs agent-code as an HTTP API server that accepts prompts
//! via POST requests and returns results. Enables IDE integrations,
//! web UIs, and automated testing without a terminal.
//!
//! # Endpoints
//!
//! - `POST /message` — send a prompt, get the agent's response
//! - `GET /events` — SSE stream of real-time agent events
//! - `GET /status` — session status (model, turns, cost)
//! - `GET /messages` — conversation history
//! - `GET /health` — simple health check
//!
//! # Usage
//!
//! ```bash
//! agent serve                    # Start on default port 4096
//! agent serve --port 8080        # Custom port
//! ```
//!
//! Then interact via HTTP:
//! ```bash
//! curl -X POST http://localhost:4096/message \
//!   -H 'Content-Type: application/json' \
//!   -d '{"content": "what files are in this project?"}'
//!
//! # Stream events in real time:
//! curl -N http://localhost:4096/events
//! ```

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;

use agent_code_lib::query::{QueryEngine, StreamSink};

// ---------------------------------------------------------------------------
// SSE event types
// ---------------------------------------------------------------------------

/// SSE event types sent to clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum SseEvent {
    /// Partial text from the model.
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    /// A tool execution has started.
    #[serde(rename = "tool_start")]
    ToolStart { name: String },

    /// A tool has produced a result.
    #[serde(rename = "tool_result")]
    ToolResult { name: String, is_error: bool },

    /// Model is thinking (extended thinking / chain-of-thought).
    #[serde(rename = "thinking")]
    Thinking { text: String },

    /// An agent turn has completed.
    #[serde(rename = "turn_complete")]
    TurnComplete { turn: usize },

    /// Token usage update.
    #[serde(rename = "usage")]
    Usage { input_tokens: u64, output_tokens: u64 },

    /// An error occurred.
    #[serde(rename = "error")]
    Error { message: String },

    /// Context compaction happened.
    #[serde(rename = "compact")]
    Compact { freed_tokens: u64 },

    /// A warning from the engine.
    #[serde(rename = "warning")]
    Warning { message: String },

    /// The full message is complete (final event).
    #[serde(rename = "done")]
    Done {
        response: String,
        turn_count: usize,
        tools_used: Vec<String>,
        cost_usd: f64,
    },
}

// ---------------------------------------------------------------------------
// Server state & request/response types
// ---------------------------------------------------------------------------

/// Shared server state wrapped for concurrent access.
pub struct ServerState {
    pub engine: tokio::sync::Mutex<QueryEngine>,
    pub event_tx: tokio::sync::RwLock<Option<tokio::sync::broadcast::Sender<SseEvent>>>,
}

/// Request body for POST /message.
#[derive(Debug, Deserialize)]
pub struct MessageRequest {
    pub content: String,
}

/// Response from POST /message.
#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub response: String,
    pub turn_count: usize,
    pub tools_used: Vec<String>,
    pub cost_usd: f64,
}

/// Response from GET /status.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub session_id: String,
    pub model: String,
    pub cwd: String,
    pub turn_count: usize,
    pub message_count: usize,
    pub cost_usd: f64,
    pub plan_mode: bool,
    pub version: String,
    pub streaming: bool,
}

/// Response from GET /messages.
#[derive(Debug, Serialize)]
pub struct MessagesResponse {
    pub messages: Vec<MessageEntry>,
}

/// A single message in the conversation.
#[derive(Debug, Serialize)]
pub struct MessageEntry {
    pub role: String,
    pub content: String,
    pub tool_calls: usize,
}

// ---------------------------------------------------------------------------
// Broadcast-based StreamSink
// ---------------------------------------------------------------------------

/// A [`StreamSink`] that broadcasts events via a tokio broadcast channel.
///
/// Also accumulates text and tool names (like `CollectingSink`) so the
/// final `Done` event and `POST /message` response include full data.
///
/// Uses `std::sync::Mutex` (not tokio) because locks are held only
/// for brief string appends — no async work under the lock.
struct SseBroadcastSink {
    tx: tokio::sync::broadcast::Sender<SseEvent>,
    text: std::sync::Mutex<String>,
    tools: std::sync::Mutex<Vec<String>>,
}

impl SseBroadcastSink {
    fn new() -> (Self, tokio::sync::broadcast::Receiver<SseEvent>) {
        let (tx, rx) = tokio::sync::broadcast::channel(256);
        (
            Self {
                tx,
                text: std::sync::Mutex::new(String::new()),
                tools: std::sync::Mutex::new(Vec::new()),
            },
            rx,
        )
    }

    /// Send an event, ignoring errors (no subscribers is fine).
    fn send(&self, event: SseEvent) {
        let _ = self.tx.send(event);
    }
}

impl StreamSink for SseBroadcastSink {
    fn on_text(&self, text: &str) {
        if let Ok(mut t) = self.text.lock() {
            t.push_str(text);
        }
        self.send(SseEvent::TextDelta { text: text.to_string() });
    }

    fn on_tool_start(&self, name: &str, _input: &serde_json::Value) {
        if let Ok(mut tools) = self.tools.lock()
            && !tools.contains(&name.to_string())
        {
            tools.push(name.to_string());
        }
        self.send(SseEvent::ToolStart { name: name.to_string() });
    }

    fn on_tool_result(&self, name: &str, result: &agent_code_lib::tools::ToolResult) {
        self.send(SseEvent::ToolResult {
            name: name.to_string(),
            is_error: result.is_error,
        });
    }

    fn on_thinking(&self, text: &str) {
        self.send(SseEvent::Thinking { text: text.to_string() });
    }

    fn on_turn_complete(&self, turn: usize) {
        self.send(SseEvent::TurnComplete { turn });
    }

    fn on_error(&self, error: &str) {
        if let Ok(mut t) = self.text.lock() {
            t.push_str(&format!("\n[Error: {error}]"));
        }
        self.send(SseEvent::Error { message: error.to_string() });
    }

    fn on_usage(&self, usage: &agent_code_lib::llm::message::Usage) {
        self.send(SseEvent::Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        });
    }

    fn on_compact(&self, freed_tokens: u64) {
        self.send(SseEvent::Compact { freed_tokens });
    }

    fn on_warning(&self, msg: &str) {
        self.send(SseEvent::Warning { message: msg.to_string() });
    }
}

// ---------------------------------------------------------------------------
// Server startup
// ---------------------------------------------------------------------------

/// Start the HTTP server.
pub async fn run_server(engine: QueryEngine, port: u16) -> anyhow::Result<()> {
    let state = Arc::new(ServerState {
        engine: tokio::sync::Mutex::new(engine),
        event_tx: tokio::sync::RwLock::new(None),
    });

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    // Write lock file for IDE discovery.
    let lock_file = agent_code_lib::services::bridge::write_lock_file(port, &cwd).ok();

    let app = Router::new()
        .route("/message", post(handle_message))
        .route("/events", get(handle_events))
        .route("/status", get(handle_status))
        .route("/messages", get(handle_messages))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    eprintln!("agent-code server listening on http://{addr}");
    eprintln!("POST /message  — send a prompt");
    eprintln!("GET  /events   — SSE event stream");
    eprintln!("GET  /status   — session status");
    eprintln!("GET  /messages — conversation history");
    eprintln!("GET  /health   — health check");
    eprintln!();
    eprintln!("Press Ctrl+C to stop.");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Clean up lock file.
    if let Some(ref lf) = lock_file {
        agent_code_lib::services::bridge::remove_lock_file(lf);
    }

    eprintln!("\nServer stopped.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /message — send a prompt and get the response.
async fn handle_message(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<MessageRequest>,
) -> Result<Json<MessageResponse>, (StatusCode, String)> {
    let (sink, _rx) = SseBroadcastSink::new();
    let sink = Arc::new(sink);

    // Publish the broadcast sender so SSE clients can subscribe.
    {
        let mut event_tx = state.event_tx.write().await;
        *event_tx = Some(sink.tx.clone());
    }

    let sink_ref: &dyn StreamSink = &*sink;

    let mut engine = state.engine.lock().await;

    let turn_result = engine.run_turn_with_sink(&req.content, sink_ref).await;

    let response_text = sink.text.lock().map(|t| t.clone()).unwrap_or_default();
    let tools_used = sink.tools.lock().map(|t| t.clone()).unwrap_or_default();

    let state_ref = engine.state();
    let turn_count = state_ref.turn_count;
    let cost_usd = state_ref.total_cost_usd;

    // Send the Done event before clearing the channel.
    sink.send(SseEvent::Done {
        response: response_text.clone(),
        turn_count,
        tools_used: tools_used.clone(),
        cost_usd,
    });

    // Clear the broadcast sender.
    {
        let mut event_tx = state.event_tx.write().await;
        *event_tx = None;
    }

    turn_result.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(MessageResponse {
        response: response_text,
        turn_count,
        tools_used,
        cost_usd,
    }))
}

/// GET /events — SSE stream of real-time agent events.
async fn handle_events(
    State(state): State<Arc<ServerState>>,
) -> Sse<impl futures::stream::Stream<Item = Result<Event, Infallible>>> {
    // Subscribe to the current broadcast channel, or create a
    // placeholder so this connection waits for the next message.
    let rx = {
        let event_tx = state.event_tx.read().await;
        match &*event_tx {
            Some(tx) => tx.subscribe(),
            None => {
                // No active message — create a channel so this SSE
                // connection is ready when the next POST /message arrives.
                let (tx, rx) = tokio::sync::broadcast::channel(256);
                drop(event_tx);
                let mut event_tx = state.event_tx.write().await;
                *event_tx = Some(tx);
                rx
            }
        }
    };

    let stream = BroadcastStream::new(rx).filter_map(|result| {
        futures::future::ready(match result {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None, // Lagged — skip
        })
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// GET /status — session information.
async fn handle_status(State(state): State<Arc<ServerState>>) -> Json<StatusResponse> {
    let engine = state.engine.lock().await;
    let s = engine.state();

    let streaming = state.event_tx.read().await.is_some();

    Json(StatusResponse {
        session_id: s.session_id.clone(),
        model: s.config.api.model.clone(),
        cwd: s.cwd.clone(),
        turn_count: s.turn_count,
        message_count: s.messages.len(),
        cost_usd: s.total_cost_usd,
        plan_mode: s.plan_mode,
        version: env!("CARGO_PKG_VERSION").to_string(),
        streaming,
    })
}

/// GET /messages — conversation history.
async fn handle_messages(State(state): State<Arc<ServerState>>) -> Json<MessagesResponse> {
    let engine = state.engine.lock().await;
    let messages: Vec<MessageEntry> = engine
        .state()
        .messages
        .iter()
        .map(|msg| match msg {
            agent_code_lib::llm::message::Message::User(u) => {
                let text: String = u
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let agent_code_lib::llm::message::ContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                MessageEntry {
                    role: "user".into(),
                    content: text,
                    tool_calls: 0,
                }
            }
            agent_code_lib::llm::message::Message::Assistant(a) => {
                let text: String = a
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let agent_code_lib::llm::message::ContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let tc = a
                    .content
                    .iter()
                    .filter(|b| {
                        matches!(
                            b,
                            agent_code_lib::llm::message::ContentBlock::ToolUse { .. }
                        )
                    })
                    .count();
                MessageEntry {
                    role: "assistant".into(),
                    content: text,
                    tool_calls: tc,
                }
            }
            _ => MessageEntry {
                role: "system".into(),
                content: String::new(),
                tool_calls: 0,
            },
        })
        .collect();

    Json(MessagesResponse { messages })
}

/// GET /health — simple health check.
async fn handle_health() -> &'static str {
    "ok"
}

/// Wait for Ctrl+C for graceful shutdown.
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
}
