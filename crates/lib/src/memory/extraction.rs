//! Background memory extraction after each turn.
//!
//! At the end of each successful query loop (model responds with no
//! more tool calls), a background task analyzes recent messages and
//! saves relevant memories. This is fire-and-forget — failures are
//! logged but never shown to the user.
//!
//! The extraction agent:
//! - Reads the last N messages for extractable knowledge
//! - Checks existing memories to avoid duplicates
//! - Writes new memory files with proper frontmatter
//! - Updates the MEMORY.md index
//!
//! Mutual exclusion: if the main agent already wrote to memory
//! files during this turn, extraction is skipped.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::llm::message::{ContentBlock, Message};
use crate::llm::provider::{Provider, ProviderRequest};

/// Tracks extraction state across turns.
pub struct ExtractionState {
    /// UUID of the last message processed by extraction.
    last_processed_index: usize,
    /// Whether an extraction is currently in progress.
    in_progress: Arc<Mutex<bool>>,
}

impl ExtractionState {
    pub fn new() -> Self {
        Self {
            last_processed_index: 0,
            in_progress: Arc::new(Mutex::new(false)),
        }
    }
}

/// Check if the main agent already wrote to memory files this turn.
/// If so, skip extraction to avoid duplication.
fn main_agent_wrote_memory(messages: &[Message], since_index: usize) -> bool {
    let memory_dir = super::ensure_memory_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default();

    if memory_dir.is_empty() {
        return false;
    }

    for msg in messages.iter().skip(since_index) {
        if let Message::Assistant(a) = msg {
            for block in &a.content {
                if let ContentBlock::ToolUse { name, input, .. } = block
                    && (name == "FileWrite" || name == "FileEdit")
                    && input
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .is_some_and(|p| p.contains("memory/"))
                {
                    return true;
                }
            }
        }
    }

    false
}

/// Build the extraction prompt for analyzing recent messages.
fn build_extraction_prompt(new_message_count: usize, memory_dir: &Path) -> String {
    // Scan existing memory files for the manifest.
    let manifest = build_memory_manifest(memory_dir);

    format!(
        "Analyze the most recent ~{new_message_count} messages in this conversation \
         and extract any knowledge worth persisting to memory.\n\n\
         Your job is to identify:\n\
         - User preferences, role, or expertise (type: user)\n\
         - Guidance about how to work: corrections or confirmed approaches (type: feedback)\n\
         - Project decisions, deadlines, or context not in the code (type: project)\n\
         - Pointers to external systems or resources (type: reference)\n\n\
         Do NOT save:\n\
         - Code patterns or architecture (derivable from reading code)\n\
         - Git history (use git log)\n\
         - Debugging solutions (fix is in the code)\n\
         - Anything ephemeral or already in AGENTS.md\n\n\
         {manifest}\n\n\
         For each memory worth saving, output a JSON object on its own line:\n\
         {{\"filename\": \"topic_name.md\", \"name\": \"Topic Name\", \
         \"description\": \"one-line description for relevance matching\", \
         \"type\": \"user|feedback|project|reference\", \
         \"content\": \"the memory content\"}}\n\n\
         Output ONLY the JSON lines, nothing else. If nothing is worth saving, \
         output nothing."
    )
}

/// Build a manifest of existing memory files with content previews.
/// This lets the LLM check for duplicates and decide whether to
/// update existing files or create new ones.
/// Public access to the memory manifest for consolidation.
pub fn build_memory_manifest_public(memory_dir: &Path) -> String {
    build_memory_manifest(memory_dir)
}

fn build_memory_manifest(memory_dir: &Path) -> String {
    let headers = super::scanner::scan_memory_files(memory_dir);
    if headers.is_empty() {
        return "No existing memory files.".to_string();
    }

    let mut manifest = String::from(
        "Existing memory files (update existing rather than creating duplicates):\n\n",
    );
    for h in &headers {
        let desc = h
            .meta
            .as_ref()
            .map(|m| {
                format!(
                    "{} ({})",
                    m.description,
                    m.memory_type
                        .as_ref()
                        .map(|t| format!("{t:?}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();

        // Read first 5 lines of content (after frontmatter) for context.
        let preview = std::fs::read_to_string(&h.path)
            .ok()
            .map(|content| {
                let after_frontmatter = if content.starts_with("---") {
                    content
                        .find("\n---\n")
                        .map(|pos| &content[pos + 5..])
                        .unwrap_or(&content)
                } else {
                    &content
                };
                after_frontmatter
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default();

        manifest.push_str(&format!(
            "- **{}**: {}\n  Preview: {}\n",
            h.filename, desc, preview
        ));
    }
    manifest
}

/// Run memory extraction as a background task.
///
/// Called at the end of each successful turn. Fire-and-forget:
/// errors are logged but never surface to the user.
pub async fn extract_memories_background(
    messages: Vec<Message>,
    state: Arc<Mutex<ExtractionState>>,
    llm: Arc<dyn Provider>,
    model: String,
) {
    let mut extraction_state = state.lock().await;

    // Check if already in progress (coalescing).
    {
        let mut in_progress = extraction_state.in_progress.lock().await;
        if *in_progress {
            debug!("Memory extraction already in progress, skipping");
            return;
        }
        *in_progress = true;
    }

    let since_index = extraction_state.last_processed_index;
    let new_count = messages.len().saturating_sub(since_index);

    if new_count < 4 {
        debug!("Too few new messages for extraction ({new_count})");
        let mut in_progress = extraction_state.in_progress.lock().await;
        *in_progress = false;
        return;
    }

    // Check if main agent already wrote memories.
    if main_agent_wrote_memory(&messages, since_index) {
        info!("Main agent wrote to memory this turn, skipping extraction");
        extraction_state.last_processed_index = messages.len();
        let mut in_progress = extraction_state.in_progress.lock().await;
        *in_progress = false;
        return;
    }

    let memory_dir = match super::ensure_memory_dir() {
        Some(d) => d,
        None => {
            let mut in_progress = extraction_state.in_progress.lock().await;
            *in_progress = false;
            return;
        }
    };

    let prompt = build_extraction_prompt(new_count, &memory_dir);

    // Drop the lock before the API call.
    let last_index = messages.len();
    let in_progress_flag = extraction_state.in_progress.clone();
    drop(extraction_state);

    // Call the LLM for extraction.
    let request = ProviderRequest {
        messages: vec![crate::llm::message::user_message(&prompt)],
        system_prompt: "You are a memory extraction agent. Output only JSON lines.".to_string(),
        tools: vec![],
        model,
        max_tokens: 2048,
        temperature: Some(0.0),
        enable_caching: false,
        tool_choice: Default::default(),
        metadata: None,
        // Background extraction: not user-cancellable, passes a fresh token.
        cancel: tokio_util::sync::CancellationToken::new(),
    };

    let result = match llm.stream(&request).await {
        Ok(mut rx) => {
            let mut output = String::new();
            while let Some(event) = rx.recv().await {
                if let crate::llm::stream::StreamEvent::TextDelta(text) = event {
                    output.push_str(&text);
                }
            }
            output
        }
        Err(e) => {
            warn!("Memory extraction API call failed: {e}");
            let mut in_progress = in_progress_flag.lock().await;
            *in_progress = false;
            return;
        }
    };

    // Parse JSON lines and save memories.
    let mut saved = 0;
    for line in result.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            let filename = entry
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown.md");
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");
            let description = entry
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mem_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("user");
            let content = entry.get("content").and_then(|v| v.as_str()).unwrap_or("");

            if content.is_empty() {
                continue;
            }

            let memory_type = match mem_type {
                "feedback" => Some(super::types::MemoryType::Feedback),
                "project" => Some(super::types::MemoryType::Project),
                "reference" => Some(super::types::MemoryType::Reference),
                _ => Some(super::types::MemoryType::User),
            };

            let meta = super::types::MemoryMeta {
                name: name.to_string(),
                description: description.to_string(),
                memory_type,
            };

            match super::writer::write_memory(&memory_dir, filename, &meta, content) {
                Ok(path) => {
                    info!("Extracted memory: {} → {}", name, path.display());
                    saved += 1;
                }
                Err(e) => {
                    warn!("Failed to save extracted memory '{}': {e}", name);
                }
            }
        }
    }

    if saved > 0 {
        info!("Memory extraction complete: {saved} memories saved");
    } else {
        debug!("Memory extraction: nothing worth saving");
    }

    // Advance cursor and release lock.
    let mut state = state.lock().await;
    state.last_processed_index = last_index;
    let mut in_progress = in_progress_flag.lock().await;
    *in_progress = false;
}
