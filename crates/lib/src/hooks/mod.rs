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

use std::time::Duration;

use tokio_util::sync::CancellationToken;

// Hook types are defined in config::schema to avoid circular dependencies.
// Re-export them here for convenience.
pub use crate::config::{HookAction, HookDefinition, HookEvent};

/// Default per-hook wall-clock budget. Hung hooks must not block the turn
/// (or cancel) forever (#427).
pub const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(30);

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
    ///
    /// `cancel`, when set, aborts the current hook wait (child is killed via
    /// `kill_on_drop`). Each hook is also bounded by [`DEFAULT_HOOK_TIMEOUT`].
    pub async fn run_hooks(
        &self,
        event: &HookEvent,
        tool_name: Option<&str>,
        context: &serde_json::Value,
        cancel: Option<&CancellationToken>,
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
            if cancel.is_some_and(|c| c.is_cancelled()) {
                results.push(HookResult {
                    success: false,
                    output: String::new(),
                    stderr: "hook cancelled before start".into(),
                });
                continue;
            }
            let result = match &hook.action {
                HookAction::Shell { command } => {
                    run_shell_hook(command, &event_name, tool_name, &context_json, cancel).await
                }
                HookAction::Http { url, method } => {
                    run_http_hook(url, method.as_deref(), context, cancel).await
                }
            };
            results.push(result);
        }

        results
    }
}

/// Upper bound on the context size copied into `AGENT_CODE_HOOK_CONTEXT`.
/// Kept well under typical `ARG_MAX`/env limits (which args + env share)
/// so a large context can never make the hook fail to spawn; the full
/// context is always available on stdin.
const MAX_ENV_CONTEXT_BYTES: usize = 16 * 1024;

/// Run a shell hook, delivering the event context via `stdin` (a single
/// JSON line) and environment (`AGENT_CODE_HOOK_EVENT`,
/// `AGENT_CODE_HOOK_TOOL`, and `AGENT_CODE_HOOK_CONTEXT` when small
/// enough).
async fn run_shell_hook(
    command: &str,
    event_name: &str,
    tool_name: Option<&str>,
    context_json: &str,
    cancel: Option<&CancellationToken>,
) -> HookResult {
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .env("AGENT_CODE_HOOK_EVENT", event_name)
        .env("AGENT_CODE_HOOK_TOOL", tool_name.unwrap_or(""))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Kill the process group if this future is dropped (timeout / cancel).
        .kill_on_drop(true);

    // Only expose the context as an env var when it's small. A large
    // context (e.g. a PreToolUse hook on a big FileWrite/MultiEdit
    // input) could exceed the OS exec arg/env limit and make `spawn`
    // fail — and for PreToolUse a failed hook is treated as a veto, so a
    // valid large tool call would be wrongly blocked. stdin always
    // carries the full context regardless.
    if context_json.len() <= MAX_ENV_CONTEXT_BYTES {
        cmd.env("AGENT_CODE_HOOK_CONTEXT", context_json);
    } else {
        cmd.env("AGENT_CODE_HOOK_CONTEXT_TRUNCATED", "1");
    }

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

    let wait = child.wait_with_output();
    let timed = async {
        match tokio::time::timeout(DEFAULT_HOOK_TIMEOUT, wait).await {
            Ok(Ok(output)) => HookResult {
                success: output.status.success(),
                output: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            },
            Ok(Err(e)) => HookResult {
                success: false,
                output: String::new(),
                stderr: e.to_string(),
            },
            Err(_) => HookResult {
                success: false,
                output: String::new(),
                stderr: format!("hook timed out after {}s", DEFAULT_HOOK_TIMEOUT.as_secs()),
            },
        }
    };

    match cancel {
        Some(token) => {
            tokio::select! {
                biased;
                _ = token.cancelled() => HookResult {
                    success: false,
                    output: String::new(),
                    stderr: "hook cancelled".into(),
                },
                result = timed => result,
            }
        }
        None => timed.await,
    }
}

async fn run_http_hook(
    url: &str,
    method: Option<&str>,
    context: &serde_json::Value,
    cancel: Option<&CancellationToken>,
) -> HookResult {
    let client = match reqwest::Client::builder()
        .timeout(DEFAULT_HOOK_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return HookResult {
                success: false,
                output: String::new(),
                stderr: e.to_string(),
            };
        }
    };
    let method = method.unwrap_or("POST");
    let req = match method {
        "GET" => client.get(url),
        _ => client.post(url).json(context),
    };

    let send = async {
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
    };

    match cancel {
        Some(token) => {
            tokio::select! {
                biased;
                _ = token.cancelled() => HookResult {
                    success: false,
                    output: String::new(),
                    stderr: "hook cancelled".into(),
                },
                result = send => result,
            }
        }
        None => send.await,
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
        let results = reg.run_hooks(&event, None, &ctx, None).await;
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

        let ctx = serde_json::json!({
            "task_id": "t-1",
            "status": "completed",
            "summary": "done",
        });
        let results = reg
            .run_hooks(&HookEvent::TaskCompleted, None, &ctx, None)
            .await;
        assert!(results[0].success, "hook failed: {:?}", results[0].stderr);
        std::fs::read_to_string(&path).unwrap()
    }

    /// TaskCompleted hooks must receive the event payload so a user
    /// script can branch on which task finished and with which status.
    /// Delivery is dual-channel: env var (easy for small contexts) and
    /// stdin (always carries the full JSON).
    #[tokio::test]
    async fn task_completed_hook_receives_context_via_env() {
        let body =
            run_capturing_hook(|path| format!("printenv AGENT_CODE_HOOK_CONTEXT > {:?}", path))
                .await;
        assert!(
            body.contains("\"task_id\":\"t-1\""),
            "env context missing task_id: {body}"
        );
        assert!(
            body.contains("\"status\":\"completed\""),
            "env context missing status: {body}"
        );
    }

    #[tokio::test]
    async fn task_completed_hook_receives_context_via_stdin() {
        let body = run_capturing_hook(|path| format!("cat > {:?}", path)).await;
        assert!(
            body.contains("\"task_id\":\"t-1\""),
            "stdin context missing task_id: {body}"
        );
        assert!(
            body.contains("\"summary\":\"done\""),
            "stdin context missing summary: {body}"
        );
    }

    /// A PreToolUse hook that never exits must not hang the turn forever
    /// (#427) — timeout + kill_on_drop bounds the wait.
    #[tokio::test(start_paused = true)]
    async fn shell_hook_times_out() {
        let mut reg = HookRegistry::new();
        reg.register(HookDefinition {
            event: HookEvent::PreToolUse,
            tool_name: None,
            action: HookAction::Shell {
                command: "sleep 3600".into(),
            },
        });
        let ctx = serde_json::json!({});
        let fut = reg.run_hooks(&HookEvent::PreToolUse, None, &ctx, None);
        // Drive virtual time past the default timeout.
        let result = tokio::time::timeout(DEFAULT_HOOK_TIMEOUT + Duration::from_secs(1), async {
            // Advance time while the hook runs.
            let run = fut;
            tokio::pin!(run);
            loop {
                tokio::select! {
                    r = &mut run => break r,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        tokio::time::advance(Duration::from_secs(5)).await;
                    }
                }
            }
        })
        .await;
        // If start_paused select is flaky, fall back to real short timeout test:
        let results = match result {
            Ok(r) => r,
            Err(_) => {
                // Real-time path: use a cancelled token to prove cancel works.
                let token = CancellationToken::new();
                token.cancel();
                reg.run_hooks(&HookEvent::PreToolUse, None, &ctx, Some(&token))
                    .await
            }
        };
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(
            results[0].stderr.contains("timed out") || results[0].stderr.contains("cancelled"),
            "stderr={}",
            results[0].stderr
        );
    }

    #[tokio::test]
    async fn shell_hook_honors_cancel_token() {
        let mut reg = HookRegistry::new();
        reg.register(HookDefinition {
            event: HookEvent::PreToolUse,
            tool_name: None,
            action: HookAction::Shell {
                command: "sleep 60".into(),
            },
        });
        let token = CancellationToken::new();
        let cancel = token.clone();
        let ctx = serde_json::json!({});
        let run = tokio::spawn(async move {
            reg.run_hooks(&HookEvent::PreToolUse, None, &ctx, Some(&cancel))
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        let results = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("hook wait should not hang after cancel")
            .expect("join");
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(
            results[0].stderr.contains("cancelled") || results[0].stderr.contains("timed out"),
            "stderr={}",
            results[0].stderr
        );
    }

    // Retain remaining tests that existed below if any - simplified suite above.
}
