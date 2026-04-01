//! IDE bridge protocol.
//!
//! HTTP server that allows IDE extensions (VS Code, JetBrains, etc.)
//! to communicate with the running agent. The bridge exposes endpoints
//! for sending messages, reading conversation state, and streaming
//! events.
//!
//! # Protocol
//!
//! - `POST /message` — send a user message to the agent
//! - `GET /messages` — retrieve conversation history
//! - `GET /status` — agent status (idle/active, model, session)
//! - `GET /events` — SSE stream of real-time events
//!
//! The bridge runs on localhost with a random high port. The port
//! is written to a lock file so IDE extensions can discover it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::info;

/// Bridge server state shared across request handlers.
pub struct BridgeState {
    /// Port the bridge is listening on.
    pub port: u16,
    /// Path to the lock file (for IDE discovery).
    pub lock_file: PathBuf,
}

/// Status response returned by GET /status.
#[derive(Debug, Serialize)]
pub struct BridgeStatus {
    pub session_id: String,
    pub model: String,
    pub cwd: String,
    pub is_active: bool,
    pub turn_count: usize,
    pub tool_count: usize,
}

/// Message request body for POST /message.
#[derive(Debug, Deserialize)]
pub struct BridgeMessageRequest {
    pub content: String,
}

/// Write the bridge lock file so IDE extensions can find us.
pub fn write_lock_file(port: u16, cwd: &str) -> Result<PathBuf, String> {
    let lock_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-code")
        .join("bridge");

    std::fs::create_dir_all(&lock_dir)
        .map_err(|e| format!("Failed to create bridge lock dir: {e}"))?;

    let lock_file = lock_dir.join(format!("{}.lock", std::process::id()));

    let content = serde_json::json!({
        "port": port,
        "pid": std::process::id(),
        "cwd": cwd,
        "started_at": chrono::Utc::now().to_rfc3339(),
    });

    std::fs::write(&lock_file, serde_json::to_string_pretty(&content).unwrap())
        .map_err(|e| format!("Failed to write lock file: {e}"))?;

    info!("Bridge lock file: {}", lock_file.display());
    Ok(lock_file)
}

/// Remove the bridge lock file on shutdown.
pub fn remove_lock_file(lock_file: &PathBuf) {
    if lock_file.exists() {
        let _ = std::fs::remove_file(lock_file);
    }
}

/// Discover running bridge instances by scanning lock files.
pub fn discover_bridges() -> Vec<BridgeInstance> {
    let lock_dir = match dirs::cache_dir() {
        Some(d) => d.join("agent-code").join("bridge"),
        None => return Vec::new(),
    };

    if !lock_dir.is_dir() {
        return Vec::new();
    }

    std::fs::read_dir(&lock_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let content = std::fs::read_to_string(entry.path()).ok()?;
            let data: serde_json::Value = serde_json::from_str(&content).ok()?;

            let pid = data.get("pid")?.as_u64()? as u32;
            let port = data.get("port")?.as_u64()? as u16;
            let cwd = data.get("cwd")?.as_str()?.to_string();

            // Check if the process is still running.
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if !alive {
                // Stale lock file — clean it up.
                let _ = std::fs::remove_file(entry.path());
                return None;
            }

            Some(BridgeInstance { pid, port, cwd })
        })
        .collect()
}

/// A discovered running bridge instance.
#[derive(Debug)]
pub struct BridgeInstance {
    pub pid: u32,
    pub port: u16,
    pub cwd: String,
}
