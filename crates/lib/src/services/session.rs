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
    /// Optional human-readable label set via `/rename`. Not used for
    /// lookup — the session ID is still the primary key — but shown
    /// in `/sessions` and the resume picker.
    #[serde(default)]
    pub label: Option<String>,
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

    // Preserve original created_at and label if file exists. Both are
    // orthogonal to the per-turn save path, so re-read rather than
    // thread them through every call site.
    let (created_at, label) = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<SessionData>(&c).ok())
            .map(|d| (d.created_at, d.label))
            .unwrap_or_else(|| (chrono::Utc::now().to_rfc3339(), None))
    } else {
        (chrono::Utc::now().to_rfc3339(), None)
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
        label,
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
                label: data.label,
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
    pub label: Option<String>,
}

/// Set (or clear) the human-readable label on a session. Pass `None`
/// to clear. Returns the path of the written file.
///
/// Implemented as load-modify-save so callers don't need to thread the
/// label through the per-turn persistence path.
pub fn set_session_label(session_id: &str, label: Option<String>) -> Result<PathBuf, String> {
    let mut data = load_session(session_id)?;
    data.label = label;
    data.updated_at = chrono::Utc::now().to_rfc3339();

    let dir = sessions_dir().ok_or("Could not determine sessions directory")?;
    let path = dir.join(format!("{session_id}.json"));
    let json = serialize_masked(&data)?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write session file: {e}"))?;
    Ok(path)
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
    use crate::llm::message::{ContentBlock, Message, UserMessage, user_message};

    /// Helper: build a session containing the given messages with
    /// fixed, deterministic metadata. Used by wire-up tests.
    fn make_session(messages: Vec<Message>) -> SessionData {
        SessionData {
            id: "fixture".into(),
            created_at: "2026-04-15T00:00:00Z".into(),
            updated_at: "2026-04-15T00:00:00Z".into(),
            cwd: "/work".into(),
            model: "test-model".into(),
            messages,
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
        }
    }

    /// Helper: a user message whose sole content block is a tool_result
    /// (simulates the agent receiving tool output that embedded a secret).
    fn tool_result_user_message(tool_use_id: &str, content: &str) -> Message {
        Message::User(UserMessage {
            uuid: uuid::Uuid::new_v4(),
            timestamp: "2026-04-15T00:00:00Z".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: false,
                extra_content: Vec::new(),
            }],
            is_meta: false,
            is_compact_summary: false,
        })
    }

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
            label: None,
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
            label: None,
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
            label: None,
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
            label: None,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(!out.contains("verylongprovidersecret1234567890"));
        assert!(out.contains("[REDACTED:credential]"));
    }

    /// Regression probe: masking must never corrupt JSON structure.
    /// Previously, the credential regex's trailing `["']?` could consume
    /// the closing quote of a JSON string value, producing unparseable
    /// output that would break /resume.
    #[test]
    fn serialize_masked_produces_parseable_json_for_unquoted_inner_secret() {
        let data = SessionData {
            id: "probe".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message("api_key=hunter2hunter2")],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
        };
        let out = serialize_masked(&data).unwrap();
        // Must still parse back as a SessionData.
        let parsed: Result<SessionData, _> = serde_json::from_str(&out);
        assert!(
            parsed.is_ok(),
            "masked session JSON failed to round-trip: {}\n---\n{out}",
            parsed.err().unwrap(),
        );
        let loaded = parsed.unwrap();
        assert_eq!(loaded.id, "probe");
        assert_eq!(loaded.messages.len(), 1);
    }

    #[test]
    fn serialize_masked_produces_parseable_json_for_multiple_secret_shapes() {
        let shapes = [
            "my api_key=hunter2hunter2",
            "password: sup3rs3cr3tv@lue (truncated)",
            r#"env DATABASE_URL=postgres://user:hunter2hunter2@host/db"#,
            "auth_token = abcdefghijklmn",
            "mixed: api_key=abcd1234efgh5678 and token=xyz12345abcd6789",
        ];
        for shape in shapes {
            let data = SessionData {
                id: "probe".to_string(),
                created_at: "2026-04-15T00:00:00Z".to_string(),
                updated_at: "2026-04-15T00:00:00Z".to_string(),
                cwd: "/work".to_string(),
                model: "test-model".to_string(),
                messages: vec![user_message(shape.to_string())],
                turn_count: 1,
                total_cost_usd: 0.0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                plan_mode: false,
                label: None,
            };
            let out = serialize_masked(&data).unwrap();
            let parsed: Result<SessionData, _> = serde_json::from_str(&out);
            assert!(
                parsed.is_ok(),
                "shape corrupted JSON: {shape:?}\nerr: {}\nout: {out}",
                parsed.err().unwrap(),
            );
        }
    }

    #[test]
    fn serialize_masked_redacts_secret_in_tool_result_block() {
        // Tool output commonly leaks env vars. When the session is
        // serialized, secrets inside ToolResult content must be scrubbed
        // just like those in plain text blocks.
        let leaked = "export AWS_SECRET_ACCESS_KEY=abcdefghijklmnopqrstuvwxyz1234";
        let data = make_session(vec![tool_result_user_message("call-1", leaked)]);
        let out = serialize_masked(&data).unwrap();
        assert!(
            !out.contains("abcdefghijklmnopqrstuvwxyz1234"),
            "tool_result secret survived serialization",
        );
        assert!(out.contains("REDACTED"));
        // Round-trip must still work.
        let _: SessionData =
            serde_json::from_str(&out).expect("tool_result session must round-trip");
    }

    #[test]
    fn serialize_masked_handles_many_messages_with_mixed_secrets() {
        // Stress: multiple messages, mixed speakers, multiple secret
        // shapes. All must be masked and the result must still parse.
        let messages = vec![
            user_message("AKIAIOSFODNN7EXAMPLE leaked in user message"),
            tool_result_user_message(
                "t1",
                r#"env dump: DATABASE_URL=postgres://user:hunter2hunter2@host/db"#,
            ),
            user_message("auth_token = abcdefghijklmnop"),
            tool_result_user_message("t2", "config.toml says api_key = \"secretprovidervalue\""),
        ];
        let data = make_session(messages);
        let out = serialize_masked(&data).unwrap();

        // No raw secrets remain.
        for needle in [
            "AKIAIOSFODNN7EXAMPLE",
            "hunter2hunter2",
            "abcdefghijklmnop",
            "secretprovidervalue",
        ] {
            assert!(!out.contains(needle), "leaked {needle} in: {out}",);
        }
        // Multiple REDACTED markers present.
        assert!(out.matches("REDACTED").count() >= 4);
        // JSON must round-trip through a real parse.
        let parsed: SessionData =
            serde_json::from_str(&out).expect("mixed-secret session must round-trip");
        assert_eq!(parsed.messages.len(), 4);
    }

    #[test]
    fn serialize_masked_is_idempotent_save_load_save() {
        // Re-saving a loaded session must produce byte-identical JSON
        // (the masker replaced all secrets on the first save; the
        // second save should find nothing to mask).
        let data = make_session(vec![
            user_message("AKIAIOSFODNN7EXAMPLE and api_key=hunter2hunter2"),
            tool_result_user_message(
                "t1",
                "ghp_abcdefghijklmnopqrstuvwxyz0123456789 then password='firstpassword1234'",
            ),
        ]);

        let first = serialize_masked(&data).unwrap();
        let loaded: SessionData = serde_json::from_str(&first).expect("first save must parse");

        // Mirror production: save_session_full re-uses timestamps from
        // in-memory state, so clone the loaded data as the next save's
        // input (keeping everything deterministic for the comparison).
        let second = serialize_masked(&loaded).unwrap();

        assert_eq!(
            first, second,
            "save→load→save is not idempotent\nfirst:\n{first}\nsecond:\n{second}",
        );
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
            label: None,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(!out.contains("REDACTED"));
        assert!(out.contains("fn main()"));
    }

    #[test]
    fn label_round_trips_through_serde() {
        let data = SessionData {
            id: "id".into(),
            created_at: "2026-04-15T00:00:00Z".into(),
            updated_at: "2026-04-15T00:00:00Z".into(),
            cwd: "/work".into(),
            model: "m".into(),
            messages: vec![],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: Some("refactor pass".into()),
        };
        let json = serde_json::to_string(&data).unwrap();
        let back: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.label.as_deref(), Some("refactor pass"));
    }

    #[test]
    fn label_missing_from_old_json_defaults_to_none() {
        // Simulate an older session file written before the label field existed.
        let json = serde_json::json!({
            "id": "old",
            "created_at": "2026-04-15T00:00:00Z",
            "updated_at": "2026-04-15T00:00:00Z",
            "cwd": "/work",
            "model": "m",
            "messages": [],
            "turn_count": 1,
        });
        let data: SessionData = serde_json::from_value(json).unwrap();
        assert!(data.label.is_none());
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
            label: None,
        };
        assert_eq!(summary.id, "xyz");
        assert_eq!(summary.turn_count, 10);
        assert_eq!(summary.message_count, 20);
    }
}
