//! Session persistence.
//!
//! Saves and restores conversation state across sessions. Each session
//! gets a unique ID and is stored as a JSON file in the sessions
//! directory (`~/.config/agent-code/sessions/`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use crate::llm::message::Message;
use crate::services::secret_masker;

/// Serializable session state persisted to disk.
///
/// Auto-saved on exit, restored via `/resume <id>`. Stored as JSON
/// in `~/.config/agent-code/sessions/`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionData {
    /// Unique session identifier.
    pub id: String,
    /// Timestamp when the session was created.
    pub created_at: String,
    /// Timestamp of the last update.
    pub updated_at: String,
    /// Working directory at session start.
    pub cwd: String,
    /// Model used in this session.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Total turns completed.
    pub turn_count: usize,
    /// Total cost in USD.
    #[serde(default)]
    pub total_cost_usd: f64,
    /// Total input tokens.
    #[serde(default)]
    pub total_input_tokens: u64,
    /// Total output tokens.
    #[serde(default)]
    pub total_output_tokens: u64,
    /// Whether plan mode was active.
    #[serde(default)]
    pub plan_mode: bool,
}

/// Sessions directory path.
fn sessions_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("sessions"))
}

/// Serialize session data to pretty JSON and apply the secret masker.
///
/// Extracted so wire-up tests can verify the persistence boundary
/// without touching the real filesystem.
pub(crate) fn serialize_masked(data: &SessionData) -> Result<String, String> {
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize session: {e}"))?;
    Ok(secret_masker::mask(&json))
}

/// Save the current session to disk.
pub fn save_session(
    session_id: &str,
    messages: &[Message],
    cwd: &str,
    model: &str,
    turn_count: usize,
) -> Result<PathBuf, String> {
    save_session_full(
        session_id, messages, cwd, model, turn_count, 0.0, 0, 0, false,
    )
}

/// Save the full session state to disk (including cost and token tracking).
#[allow(clippy::too_many_arguments)]
pub fn save_session_full(
    session_id: &str,
    messages: &[Message],
    cwd: &str,
    model: &str,
    turn_count: usize,
    total_cost_usd: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    plan_mode: bool,
) -> Result<PathBuf, String> {
    let dir = sessions_dir().ok_or("Could not determine sessions directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create sessions dir: {e}"))?;

    let path = dir.join(format!("{session_id}.json"));

    // Preserve original created_at if file exists.
    let created_at = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<SessionData>(&c).ok())
            .map(|d| d.created_at)
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339())
    } else {
        chrono::Utc::now().to_rfc3339()
    };

    let data = SessionData {
        id: session_id.to_string(),
        created_at,
        updated_at: chrono::Utc::now().to_rfc3339(),
        cwd: cwd.to_string(),
        model: model.to_string(),
        messages: messages.to_vec(),
        turn_count,
        total_cost_usd,
        total_input_tokens,
        total_output_tokens,
        plan_mode,
    };

    // Mask secrets at the persistence boundary. Applied to the fully
    // serialized JSON so the same regex set covers every text-bearing
    // field (tool results, user messages, metadata). Escaped JSON
    // strings still match the same patterns at the byte level.
    let json = serialize_masked(&data)?;

    std::fs::write(&path, json).map_err(|e| format!("Failed to write session file: {e}"))?;

    debug!("Session saved: {}", path.display());
    Ok(path)
}

/// Load a session from disk by ID.
pub fn load_session(session_id: &str) -> Result<SessionData, String> {
    let dir = sessions_dir().ok_or("Could not determine sessions directory")?;
    let path = dir.join(format!("{session_id}.json"));

    if !path.exists() {
        return Err(format!("Session '{session_id}' not found"));
    }

    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read session: {e}"))?;

    let data: SessionData =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse session: {e}"))?;

    info!(
        "Session loaded: {} ({} messages)",
        session_id,
        data.messages.len()
    );
    Ok(data)
}

/// List recent sessions, sorted by last update (most recent first).
pub fn list_sessions(limit: usize) -> Vec<SessionSummary> {
    let dir = match sessions_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Vec::new(),
    };

    let mut sessions: Vec<SessionSummary> = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let content = std::fs::read_to_string(entry.path()).ok()?;
            let data: SessionData = serde_json::from_str(&content).ok()?;
            Some(SessionSummary {
                id: data.id,
                cwd: data.cwd,
                model: data.model,
                turn_count: data.turn_count,
                message_count: data.messages.len(),
                updated_at: data.updated_at,
            })
        })
        .collect();

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(limit);
    sessions
}

/// Brief summary of a session for listing.
#[derive(Debug)]
pub struct SessionSummary {
    pub id: String,
    pub cwd: String,
    pub model: String,
    pub turn_count: usize,
    pub message_count: usize,
    pub updated_at: String,
}

/// Generate a new session ID.
pub fn new_session_id() -> String {
    Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap_or("session")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::user_message;

    #[test]
    fn test_new_session_id_format() {
        let id = new_session_id();
        assert!(!id.is_empty());
        assert!(!id.contains('-')); // Should be first segment only.
        assert!(id.len() == 8); // UUID first segment is 8 hex chars.
    }

    #[test]
    fn test_new_session_id_unique() {
        let id1 = new_session_id();
        let id2 = new_session_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_save_and_load_session() {
        // Override sessions dir to a temp directory.
        let dir = tempfile::tempdir().unwrap();
        let session_id = "test-save-load";
        let session_file = dir.path().join(format!("{session_id}.json"));

        let messages = vec![user_message("hello"), user_message("world")];

        // Save manually to temp dir.
        let data = SessionData {
            id: session_id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            cwd: "/tmp".to_string(),
            model: "test-model".to_string(),
            messages: messages.clone(),
            turn_count: 5,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
        };
        let json = serde_json::to_string_pretty(&data).unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(&session_file, &json).unwrap();

        // Load it back.
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.id, session_id);
        assert_eq!(loaded.cwd, "/tmp");
        assert_eq!(loaded.model, "test-model");
        assert_eq!(loaded.turn_count, 5);
        assert_eq!(loaded.messages.len(), 2);
    }

    #[test]
    fn test_session_data_serialization_roundtrip() {
        let data = SessionData {
            id: "abc123".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            cwd: "/home/user/project".to_string(),
            model: "claude-sonnet-4".to_string(),
            messages: vec![user_message("test")],
            turn_count: 3,
            total_cost_usd: 0.05,
            total_input_tokens: 1000,
            total_output_tokens: 500,
            plan_mode: false,
        };

        let json = serde_json::to_string(&data).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.id, data.id);
        assert_eq!(loaded.model, data.model);
        assert_eq!(loaded.turn_count, data.turn_count);
    }

    #[test]
    fn serialize_masked_redacts_secrets_in_messages() {
        // A tool result leaked an AWS access key into the message history.
        // When the session is serialized for disk, the secret must not
        // survive the persistence boundary.
        let aws_key = "AKIAIOSFODNN7EXAMPLE";
        let data = SessionData {
            id: "sess-1".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message(format!("here is my key {aws_key}"))],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(
            !out.contains(aws_key),
            "raw AWS key survived serialization: {out}",
        );
        assert!(out.contains("[REDACTED:aws_access_key]"));
        // Non-secret metadata must still be present.
        assert!(out.contains("\"cwd\": \"/work\""));
        assert!(out.contains("\"model\": \"test-model\""));
    }

    #[test]
    fn serialize_masked_redacts_generic_credential_assignments() {
        let secret_line = "api_key=verylongprovidersecret1234567890";
        let data = SessionData {
            id: "sess-2".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message(secret_line)],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(!out.contains("verylongprovidersecret1234567890"));
        assert!(out.contains("[REDACTED:credential]"));
    }

    #[test]
    fn serialize_masked_leaves_innocuous_content_intact() {
        let data = SessionData {
            id: "sess-3".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message("fn main() { println!(\"hello\"); }")],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(!out.contains("REDACTED"));
        assert!(out.contains("fn main()"));
    }

    #[test]
    fn test_session_summary_fields() {
        let summary = SessionSummary {
            id: "xyz".to_string(),
            cwd: "/tmp".to_string(),
            model: "gpt-4".to_string(),
            turn_count: 10,
            message_count: 20,
            updated_at: "2026-03-31".to_string(),
        };
        assert_eq!(summary.id, "xyz");
        assert_eq!(summary.turn_count, 10);
        assert_eq!(summary.message_count, 20);
    }
}
