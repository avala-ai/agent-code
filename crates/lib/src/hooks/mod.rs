//! Hook system.
//!
//! Hooks allow user-defined actions to run at specific points in the
//! agent lifecycle:
//!
//! - `PreToolUse` — before a tool executes (can block/modify)
//! - `PostToolUse` — after a tool completes
//! - `SessionStart` — when a session begins
//! - `SessionStop` — when a session ends
//! - `UserPromptSubmit` — when the user submits input
//!
//! Hooks can be shell commands, HTTP endpoints, or prompt templates,
//! configured in the settings file.

// Hook types are defined in config::schema to avoid circular dependencies.
// Re-export them here for convenience.
pub use crate::config::{HookAction, HookDefinition, HookEvent};

/// Hook registry that stores and dispatches hooks.
pub struct HookRegistry {
    hooks: Vec<HookDefinition>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: HookDefinition) {
        self.hooks.push(hook);
    }

    /// Get all hooks for a given event, optionally filtered by tool name.
    pub fn get_hooks(&self, event: &HookEvent, tool_name: Option<&str>) -> Vec<&HookDefinition> {
        self.hooks
            .iter()
            .filter(|h| {
                h.event == *event
                    && (h.tool_name.is_none()
                        || tool_name.is_none()
                        || h.tool_name.as_deref() == tool_name)
            })
            .collect()
    }

    /// Execute all hooks for a given event. Shell hooks run as subprocesses.
    pub async fn run_hooks(
        &self,
        event: &HookEvent,
        tool_name: Option<&str>,
        _context: &serde_json::Value,
    ) -> Vec<HookResult> {
        let hooks = self.get_hooks(event, tool_name);
        let mut results = Vec::new();

        for hook in hooks {
            let result = match &hook.action {
                HookAction::Shell { command } => {
                    match tokio::process::Command::new("bash")
                        .arg("-c")
                        .arg(command)
                        .output()
                        .await
                    {
                        Ok(output) => HookResult {
                            success: output.status.success(),
                            output: String::from_utf8_lossy(&output.stdout).to_string(),
                        },
                        Err(e) => HookResult {
                            success: false,
                            output: e.to_string(),
                        },
                    }
                }
                HookAction::Http { url, method } => {
                    let client = reqwest::Client::new();
                    let method = method.as_deref().unwrap_or("POST");
                    let req = match method {
                        "GET" => client.get(url),
                        _ => client.post(url),
                    };
                    match req.send().await {
                        Ok(resp) => HookResult {
                            success: resp.status().is_success(),
                            output: resp.text().await.unwrap_or_default(),
                        },
                        Err(e) => HookResult {
                            success: false,
                            output: e.to_string(),
                        },
                    }
                }
            };
            results.push(result);
        }

        results
    }
}

/// Result of executing a hook.
#[derive(Debug)]
pub struct HookResult {
    pub success: bool,
    pub output: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HookAction, HookDefinition, HookEvent};

    /// Build a shell hook that appends a single line to a temp file.
    /// Used to verify run_hooks() actually dispatches for a given event.
    fn touch_file_hook(event: HookEvent, path: &std::path::Path) -> HookDefinition {
        // Quote the path so spaces don't break the shell command. The
        // test can then read the file and assert the event fired.
        let cmd = format!("echo fired >> {:?}", path);
        HookDefinition {
            event,
            tool_name: None,
            action: HookAction::Shell { command: cmd },
        }
    }

    async fn run_and_read(event: HookEvent) -> String {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        // Truncate to start from an empty file.
        std::fs::write(&path, "").unwrap();

        let mut reg = HookRegistry::new();
        reg.register(touch_file_hook(event.clone(), &path));

        let ctx = serde_json::json!({ "probe": true });
        let results = reg.run_hooks(&event, None, &ctx).await;
        assert_eq!(results.len(), 1, "exactly one hook should have fired");
        assert!(
            results[0].success,
            "hook should succeed; output was {:?}",
            results[0].output
        );

        std::fs::read_to_string(&path).unwrap()
    }

    /// Regression guard: SessionStart is declared in the enum and
    /// historically was never wired to fire. Confirm the dispatcher
    /// actually matches hooks registered for it.
    #[tokio::test]
    async fn run_hooks_fires_session_start() {
        let body = run_and_read(HookEvent::SessionStart).await;
        assert!(body.contains("fired"), "SessionStart hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_session_stop() {
        let body = run_and_read(HookEvent::SessionStop).await;
        assert!(body.contains("fired"), "SessionStop hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_user_prompt_submit() {
        let body = run_and_read(HookEvent::UserPromptSubmit).await;
        assert!(body.contains("fired"), "UserPromptSubmit hook did not run");
    }

    /// Registering a hook for one event must NOT cause it to fire when
    /// a different event is dispatched. This protects the event-match
    /// contract callers of fire_session_start_hooks rely on.
    #[tokio::test]
    async fn run_hooks_does_not_cross_fire_between_events() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        std::fs::write(&path, "").unwrap();

        let mut reg = HookRegistry::new();
        reg.register(touch_file_hook(HookEvent::SessionStart, &path));

        let ctx = serde_json::json!({ "probe": true });
        // Dispatch a different event — the file must stay empty.
        let _ = reg.run_hooks(&HookEvent::SessionStop, None, &ctx).await;

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.is_empty(),
            "dispatching SessionStop must not fire a SessionStart hook; got {body:?}"
        );
    }
}
