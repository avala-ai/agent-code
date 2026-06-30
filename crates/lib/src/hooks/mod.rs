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
//! - `PreCompact` — before /compact or auto-compact mutates history
//! - `PostCompact` — after compaction finishes with the actual outcome
//! - `FileChanged` — after any file-mutating tool completes
//! - `Stop` — agent finished responding; about to yield to the user
//! - `Notification` — agent needs user attention (budget / context full)
//! - `CwdChanged` — session cwd or tracked dirs changed
//! - `ConfigChange` — /reload rescanned on-disk extensions
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
        context: &serde_json::Value,
    ) -> Vec<HookResult> {
        let hooks = self.get_hooks(event, tool_name);
        let mut results = Vec::new();

        // The event's context, delivered to each hook so it can act on
        // the details (which task finished, which tool, etc.). Shell
        // hooks receive it as a JSON line on stdin (the convention
        // hook-based agents use) and in `AGENT_CODE_HOOK_CONTEXT`; HTTP
        // hooks receive it as the request body.
        let event_name = serde_json::to_value(event)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let context_json = serde_json::to_string(context).unwrap_or_else(|_| "{}".to_string());

        for hook in hooks {
            let result = match &hook.action {
                HookAction::Shell { command } => {
                    run_shell_hook(command, &event_name, tool_name, &context_json).await
                }
                HookAction::Http { url, method } => {
                    let client = reqwest::Client::new();
                    let method = method.as_deref().unwrap_or("POST");
                    let req = match method {
                        "GET" => client.get(url),
                        _ => client.post(url).json(context),
                    };
                    match req.send().await {
                        Ok(resp) => HookResult {
                            success: resp.status().is_success(),
                            output: resp.text().await.unwrap_or_default(),
                            stderr: String::new(),
                        },
                        Err(e) => HookResult {
                            success: false,
                            output: String::new(),
                            stderr: e.to_string(),
                        },
                    }
                }
            };
            results.push(result);
        }

        results
    }
}

/// Run a shell hook, delivering the event context via `stdin` (a single
/// JSON line) and environment (`AGENT_CODE_HOOK_EVENT`,
/// `AGENT_CODE_HOOK_TOOL`, `AGENT_CODE_HOOK_CONTEXT`).
async fn run_shell_hook(
    command: &str,
    event_name: &str,
    tool_name: Option<&str>,
    context_json: &str,
) -> HookResult {
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .env("AGENT_CODE_HOOK_EVENT", event_name)
        .env("AGENT_CODE_HOOK_TOOL", tool_name.unwrap_or(""))
        .env("AGENT_CODE_HOOK_CONTEXT", context_json)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HookResult {
                success: false,
                output: String::new(),
                stderr: e.to_string(),
            };
        }
    };

    // Write the context to stdin and close it so the hook can `cat` it.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(context_json.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        // Dropping `stdin` here closes the pipe (EOF for the child).
    }

    match child.wait_with_output().await {
        Ok(output) => HookResult {
            success: output.status.success(),
            output: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(e) => HookResult {
            success: false,
            output: String::new(),
            stderr: e.to_string(),
        },
    }
}

/// Result of executing a hook.
#[derive(Debug, Default, Clone)]
pub struct HookResult {
    /// True if the hook ran to completion without error (shell
    /// command exited 0, HTTP request returned 2xx).
    pub success: bool,
    /// Stdout captured from the hook subprocess, or the HTTP
    /// response body.
    pub output: String,
    /// Stderr captured from the hook subprocess. Empty for HTTP
    /// hooks. Used as the veto reason when a PreToolUse hook
    /// blocks a tool call so operators get the hook author's
    /// own error text instead of a generic message.
    pub stderr: String,
}

// Shell hooks dispatch via `bash -c`, which isn't available on Windows
// without WSL. Gate the tests on unix so the Windows CI job doesn't try
// to spawn a subprocess that fails with a WSL install-distribution error.
#[cfg(all(test, unix))]
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

    /// PostCompact is the newest variant. Confirm the dispatcher matches
    /// it correctly so a hook registered for `post_compact` actually
    /// receives the event.
    #[tokio::test]
    async fn run_hooks_fires_post_compact() {
        let body = run_and_read(HookEvent::PostCompact).await;
        assert!(body.contains("fired"), "PostCompact hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_file_changed() {
        let body = run_and_read(HookEvent::FileChanged).await;
        assert!(body.contains("fired"), "FileChanged hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_stop() {
        let body = run_and_read(HookEvent::Stop).await;
        assert!(body.contains("fired"), "Stop hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_notification() {
        let body = run_and_read(HookEvent::Notification).await;
        assert!(body.contains("fired"), "Notification hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_task_completed() {
        let body = run_and_read(HookEvent::TaskCompleted).await;
        assert!(body.contains("fired"), "TaskCompleted hook did not run");
    }

    /// Run a shell hook that writes `script` (with the context delivered
    /// via env/stdin) and return what it captured.
    async fn run_capturing_hook(script_to_file: impl Fn(&std::path::Path) -> String) -> String {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        std::fs::write(&path, "").unwrap();

        let mut reg = HookRegistry::new();
        reg.register(HookDefinition {
            event: HookEvent::TaskCompleted,
            tool_name: None,
            action: HookAction::Shell {
                command: script_to_file(&path),
            },
        });

        let ctx = serde_json::json!({ "id": "a7", "status": "completed", "kind": "LocalAgent" });
        let results = reg.run_hooks(&HookEvent::TaskCompleted, None, &ctx).await;
        assert!(results[0].success, "hook failed: {:?}", results[0].stderr);
        std::fs::read_to_string(&path).unwrap()
    }

    #[tokio::test]
    async fn run_hooks_delivers_context_via_env() {
        let body =
            run_capturing_hook(|p| format!("printf '%s' \"$AGENT_CODE_HOOK_CONTEXT\" > {p:?}"))
                .await;
        assert!(
            body.contains("\"id\":\"a7\""),
            "env context missing: {body}"
        );
        assert!(body.contains("\"status\":\"completed\""));
    }

    #[tokio::test]
    async fn run_hooks_delivers_context_via_stdin() {
        let body = run_capturing_hook(|p| format!("cat > {p:?}")).await;
        assert!(
            body.contains("\"id\":\"a7\""),
            "stdin context missing: {body}"
        );
    }

    #[tokio::test]
    async fn run_hooks_sets_event_name_env() {
        let body =
            run_capturing_hook(|p| format!("printf '%s' \"$AGENT_CODE_HOOK_EVENT\" > {p:?}")).await;
        assert_eq!(body, "task_completed");
    }

    #[tokio::test]
    async fn run_hooks_fires_cwd_changed() {
        let body = run_and_read(HookEvent::CwdChanged).await;
        assert!(body.contains("fired"), "CwdChanged hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_config_change() {
        let body = run_and_read(HookEvent::ConfigChange).await;
        assert!(body.contains("fired"), "ConfigChange hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_error() {
        let body = run_and_read(HookEvent::Error).await;
        assert!(body.contains("fired"), "Error hook did not run");
    }

    #[tokio::test]
    async fn run_hooks_fires_permission_denied() {
        let body = run_and_read(HookEvent::PermissionDenied).await;
        assert!(body.contains("fired"), "PermissionDenied hook did not run");
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

    // ---- veto path prerequisites: HookResult must carry stderr and
    //      report `success = false` on non-zero exit so the query
    //      engine's pre-tool-use gate has enough info to block a tool
    //      call and surface the reason.

    #[tokio::test]
    async fn run_hooks_nonzero_exit_sets_success_false() {
        let mut reg = HookRegistry::new();
        reg.register(HookDefinition {
            event: HookEvent::PreToolUse,
            tool_name: None,
            action: HookAction::Shell {
                command: "exit 1".into(),
            },
        });
        let ctx = serde_json::json!({});
        let results = reg.run_hooks(&HookEvent::PreToolUse, None, &ctx).await;
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].success,
            "hook exiting 1 must set success=false; got {:?}",
            results[0]
        );
    }

    #[tokio::test]
    async fn run_hooks_captures_stderr_separately_from_stdout() {
        // Hook authors signal block reasons on stderr (shell
        // convention). The dispatcher must capture stderr into a
        // dedicated field so downstream veto handling can surface
        // the exact message without scraping mixed output.
        let mut reg = HookRegistry::new();
        reg.register(HookDefinition {
            event: HookEvent::PreToolUse,
            tool_name: None,
            action: HookAction::Shell {
                command: "echo on_stdout; echo on_stderr >&2; exit 2".into(),
            },
        });
        let ctx = serde_json::json!({});
        let results = reg.run_hooks(&HookEvent::PreToolUse, None, &ctx).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(
            results[0].output.contains("on_stdout"),
            "stdout should be populated: {:?}",
            results[0].output
        );
        assert!(
            results[0].stderr.contains("on_stderr"),
            "stderr should be populated: {:?}",
            results[0].stderr
        );
        // Stdout and stderr must NOT be mixed.
        assert!(
            !results[0].output.contains("on_stderr"),
            "stderr leaked into stdout: {:?}",
            results[0].output
        );
    }

    #[tokio::test]
    async fn run_hooks_zero_exit_leaves_success_true_regardless_of_stderr() {
        // A hook that exits 0 is not a veto even if it wrote to
        // stderr — some hooks log progress on stderr as a matter of
        // style. Success is tied to exit status only.
        let mut reg = HookRegistry::new();
        reg.register(HookDefinition {
            event: HookEvent::PreToolUse,
            tool_name: None,
            action: HookAction::Shell {
                command: "echo noisy >&2; exit 0".into(),
            },
        });
        let ctx = serde_json::json!({});
        let results = reg.run_hooks(&HookEvent::PreToolUse, None, &ctx).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert!(results[0].stderr.contains("noisy"));
    }
}
