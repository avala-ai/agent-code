//! Headless HTTP server mode.
//!
//! Runs agent-code as an HTTP API server that accepts prompts
//! via POST requests and returns results. Enables IDE integrations,
//! web UIs, and automated testing without a terminal.
//!
//! # Endpoints
//!
//! - `POST /message` — send a prompt, get the agent's response
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
//! ```

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use agent_code_lib::query::{QueryEngine, StreamSink};

/// Shared server state wrapped for concurrent access.
pub struct ServerState {
    pub engine: tokio::sync::Mutex<QueryEngine>,
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

/// A StreamSink that collects output text.
///
/// Uses std::sync::Mutex (not tokio) because locks are held only
/// for brief string appends — no async work under the lock.
struct CollectingSink {
    text: std::sync::Mutex<String>,
    tools: std::sync::Mutex<Vec<String>>,
}

impl CollectingSink {
    fn new() -> Self {
        Self {
            text: std::sync::Mutex::new(String::new()),
            tools: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl StreamSink for CollectingSink {
    fn on_text(&self, text: &str) {
        if let Ok(mut t) = self.text.lock() {
            t.push_str(text);
        }
    }

    fn on_tool_start(&self, name: &str, _input: &serde_json::Value) {
        if let Ok(mut tools) = self.tools.lock()
            && !tools.contains(&name.to_string())
        {
            tools.push(name.to_string());
        }
    }

    fn on_tool_result(&self, _name: &str, _result: &agent_code_lib::tools::ToolResult) {}

    fn on_error(&self, error: &str) {
        if let Ok(mut t) = self.text.lock() {
            t.push_str(&format!("\n[Error: {error}]"));
        }
    }
}

/// Start the HTTP server.
pub async fn run_server(engine: QueryEngine, port: u16) -> anyhow::Result<()> {
    let state = Arc::new(ServerState {
        engine: tokio::sync::Mutex::new(engine),
    });

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    // Write lock file for IDE discovery.
    let lock_file = agent_code_lib::services::bridge::write_lock_file(port, &cwd).ok();

    let app = Router::new()
        .route("/message", post(handle_message))
        .route("/status", get(handle_status))
        .route("/messages", get(handle_messages))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    eprintln!("agent-code server listening on http://{addr}");
    eprintln!("POST /message  — send a prompt");
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

/// POST /message — send a prompt and get the response.
async fn handle_message(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<MessageRequest>,
) -> Result<Json<MessageResponse>, (StatusCode, String)> {
    let sink = Arc::new(CollectingSink::new());
    let sink_ref: &dyn StreamSink = &*sink;

    let mut engine = state.engine.lock().await;

    engine
        .run_turn_with_sink(&req.content, sink_ref)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response_text = sink.text.lock().map(|t| t.clone()).unwrap_or_default();
    let tools_used = sink.tools.lock().map(|t| t.clone()).unwrap_or_default();

    let state_ref = engine.state();
    Ok(Json(MessageResponse {
        response: response_text,
        turn_count: state_ref.turn_count,
        tools_used,
        cost_usd: state_ref.total_cost_usd,
    }))
}

/// GET /status — session information.
async fn handle_status(State(state): State<Arc<ServerState>>) -> Json<StatusResponse> {
    let engine = state.engine.lock().await;
    let s = engine.state();

    Json(StatusResponse {
        session_id: s.session_id.clone(),
        model: s.config.api.model.clone(),
        cwd: s.cwd.clone(),
        turn_count: s.turn_count,
        message_count: s.messages.len(),
        cost_usd: s.total_cost_usd,
        plan_mode: s.plan_mode,
        version: env!("CARGO_PKG_VERSION").to_string(),
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
