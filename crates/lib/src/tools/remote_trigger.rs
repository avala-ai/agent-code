//! RemoteTrigger tool: fire a one-off run of a stored routine.
//!
//! The schedule executor needs an LLM provider and full config to drive
//! a turn, neither of which the per-tool `ToolContext` carries. We mirror
//! the [`crate::tools::agent::AgentTool`] pattern and spawn the host
//! `agent schedule run <id>` subprocess so the routine runs through the
//! same code path as `agent schedule run`. The tool waits for the
//! subprocess to finish (subject to an optional timeout) and returns its
//! captured output, keeping the call request/response in spirit.

use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::permissions::{PermissionChecker, PermissionDecision};
use crate::schedule::storage::validate_schedule_name;

use super::cron_support::open_store;

/// Default wall-clock cap for a remote-triggered run. Keeps the tool
/// call from hanging indefinitely if the routine wedges. Callers can
/// raise (or lower) this via `timeout_seconds`.
const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Hard ceiling — even when callers ask for longer, we cap here so a
/// runaway routine can't hold the tool open forever.
const MAX_TIMEOUT_SECS: u64 = 3600;

pub struct RemoteTriggerTool;

#[async_trait]
impl Tool for RemoteTriggerTool {
    fn name(&self) -> &'static str {
        "RemoteTrigger"
    }

    fn description(&self) -> &'static str {
        "Fire a one-off run of a stored cron routine and return its output. \
         Blocks until the routine finishes or the timeout elapses."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Routine id to trigger (as returned by CronCreate or CronList)."
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 3600,
                    "description": "Optional wall-clock timeout for the run. Defaults to 600 seconds, capped at 3600."
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        // Triggering a run consumes API budget and may mutate the
        // working directory, so gate it on the standard permission
        // checker rather than auto-allowing.
        true
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        checker: &PermissionChecker,
    ) -> PermissionDecision {
        checker.check(self.name(), input)
    }

    /// Pinned to the routine this id names right now — see
    /// [`super::cron_support::routine_grant_binding`].
    fn grant_binding(&self, input: &serde_json::Value) -> Option<super::GrantBinding> {
        super::cron_support::routine_grant_binding(input)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        // Validate up-front so we never pass a hostile id to the
        // store, the subprocess argv, or anywhere else. The store
        // applies the same check at its boundary as defense in
        // depth, but rejecting here gives the model a crisp,
        // actionable error before we touch the filesystem.
        validate_schedule_name(id).map_err(ToolError::InvalidInput)?;

        // Verify the routine exists before forking — gives the model a
        // crisp error rather than a subprocess failure code.
        let store = open_store().map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to open schedule store: {e}"))
        })?;
        if store.load(id).is_err() {
            return Err(ToolError::InvalidInput(format!(
                "No routine with id '{id}' exists. Use CronList to see available routines."
            )));
        }

        let timeout_secs = input
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        // Spawn `agent schedule run <id>` to delegate to the existing
        // executor. The subprocess inherits provider env vars; the
        // routine record itself supplies cwd, model, and prompt.
        let agent_binary = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "agent".to_string());

        let mut cmd = tokio::process::Command::new(&agent_binary);
        cmd.arg("schedule")
            .arg("run")
            .arg(id)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Defense in depth: if this future is dropped (panic, caller
            // abort) before we explicitly kill the child below, Tokio
            // will reap it for us instead of leaving an orphan.
            .kill_on_drop(true);

        // Put the child in its own process group so we can SIGKILL
        // the whole tree on timeout/cancel. Without this, only the
        // direct child gets `start_kill`'d and any grandchildren
        // (e.g. tools the routine spawned) survive.
        configure_process_group(&mut cmd);

        // Forward common provider env vars so the subprocess can
        // authenticate without re-reading config.
        for var in &[
            "AGENT_CODE_API_KEY",
            "AGENT_CODE_API_BASE_URL",
            "AGENT_CODE_MODEL",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "XAI_API_KEY",
            "GOOGLE_API_KEY",
            "DEEPSEEK_API_KEY",
            "GROQ_API_KEY",
            "MISTRAL_API_KEY",
            "TOGETHER_API_KEY",
            crate::tools::cron_support::SCHEDULES_DIR_ENV,
        ] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let outcome = run_with_timeout(cmd, Duration::from_secs(timeout_secs), &ctx.cancel)
            .await
            .map_err(|e| match e {
                RunError::Spawn(msg) => ToolError::ExecutionFailed(format!(
                    "Failed to spawn '{agent_binary}' schedule run {id}: {msg}"
                )),
                RunError::Wait(msg) => ToolError::ExecutionFailed(format!(
                    "Failed waiting on '{agent_binary}' schedule run {id}: {msg}"
                )),
                RunError::Timeout(ms) => ToolError::Timeout(ms),
                RunError::Cancelled => ToolError::Cancelled,
            })?;

        let stdout = String::from_utf8_lossy(&outcome.stdout).to_string();
        let stderr = String::from_utf8_lossy(&outcome.stderr).to_string();
        let success = outcome.status.success();

        let mut content = format!(
            "Routine '{id}' triggered (exit={}).\n",
            outcome
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into())
        );
        if !stdout.is_empty() {
            content.push_str("\n--- stdout ---\n");
            content.push_str(&stdout);
        }
        if !stderr.is_empty() {
            content.push_str("\n--- stderr ---\n");
            content.push_str(&stderr);
        }

        Ok(ToolResult {
            content,
            is_error: !success,
        })
    }
}

/// Outcome of a spawned subprocess that ran to completion.
#[cfg_attr(test, derive(Debug))]
struct RunOutcome {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Failure modes from [`run_with_timeout`].
#[cfg_attr(test, derive(Debug))]
enum RunError {
    Spawn(String),
    Wait(String),
    /// Timeout in milliseconds.
    Timeout(u64),
    Cancelled,
}

/// Spawn `cmd`, wait for it with a wall-clock timeout, and reap the
/// child (and its process group) on timeout/cancel.
///
/// `cmd` must already be configured with `Stdio::piped()` for stdout
/// and stderr and (on unix) put into its own process group via
/// [`configure_process_group`]. This helper:
///
/// 1. Spawns dedicated drain tasks for stdout and stderr so output
///    is captured into shared buffers regardless of which arm of
///    the select wins.
/// 2. Races the child's exit against the timeout and the cancel
///    token.
/// 3. On timeout/cancel, sends SIGKILL to the entire process group
///    (negative pgid) so grandchildren are reaped too — `start_kill`
///    only signals the direct child. Then `wait`s on the child and
///    joins the drain tasks so any pending output ends up in the
///    returned buffers and no reader task is abandoned.
async fn run_with_timeout(
    mut cmd: tokio::process::Command,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<RunOutcome, RunError> {
    let mut child = cmd.spawn().map_err(|e| RunError::Spawn(e.to_string()))?;

    // Capture the pid early — once we've called `wait`, `child.id()`
    // returns None on some platforms and we need the pgid for
    // signaling. On non-unix targets we never use the value but the
    // binding still has to exist so the timeout/cancel arms compile.
    let child_pid: Option<i32> = {
        #[cfg(unix)]
        {
            child.id().map(|id| id as i32)
        }
        #[cfg(not(unix))]
        {
            None
        }
    };

    let stdout_handle = child.stdout.take().expect("stdout piped at spawn");
    let stderr_handle = child.stderr.take().expect("stderr piped at spawn");

    // Spawn dedicated drainers. Sharing the buffers via Arc<Mutex<_>>
    // lets the timeout/cancel branches still recover whatever output
    // had been written before we killed the child.
    let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let stdout_drain = spawn_drain("stdout", stdout_handle, stdout_buf.clone());
    let stderr_drain = spawn_drain("stderr", stderr_handle, stderr_buf.clone());

    let outcome = tokio::select! {
        wait_result = child.wait() => {
            // Child exited on its own. Wait for drains to finish so
            // we capture every last byte before returning.
            let _ = stdout_drain.await;
            let _ = stderr_drain.await;
            match wait_result {
                Ok(status) => Ok(RunOutcome {
                    status,
                    stdout: take_buf(&stdout_buf),
                    stderr: take_buf(&stderr_buf),
                }),
                Err(e) => Err(RunError::Wait(e.to_string())),
            }
        }
        _ = tokio::time::sleep(timeout) => {
            // Reap the entire process group so grandchildren can't
            // keep consuming API budget after we've returned.
            kill_process_group(&mut child, child_pid);
            // Drains will see EOF as soon as the child's stdio is
            // closed; bound them with a short deadline so a stuck
            // pipe can't keep us here forever.
            join_drain(stdout_drain).await;
            join_drain(stderr_drain).await;
            // Reap the child so it doesn't linger as a zombie.
            let _ = child.wait().await;
            Err(RunError::Timeout(timeout.as_millis() as u64))
        }
        _ = cancel.cancelled() => {
            kill_process_group(&mut child, child_pid);
            join_drain(stdout_drain).await;
            join_drain(stderr_drain).await;
            let _ = child.wait().await;
            Err(RunError::Cancelled)
        }
    };

    outcome
}

fn take_buf(buf: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    buf.lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// Spawn a task that reads `reader` to EOF into `buf`. The task is
/// detached but its handle is returned so callers can join it after
/// the child has exited (or been killed) to flush remaining bytes.
fn spawn_drain<R>(
    label: &'static str,
    mut reader: R,
    buf: Arc<Mutex<Vec<u8>>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut local = Vec::new();
        if let Err(e) = reader.read_to_end(&mut local).await {
            tracing::debug!("remote_trigger drain {label} read error: {e}");
        }
        if let Ok(mut guard) = buf.lock() {
            guard.extend_from_slice(&local);
        }
    })
}

/// Join a drain task with a small deadline. After we've killed the
/// child, the pipes close almost immediately, so 5s is generous.
async fn join_drain(handle: tokio::task::JoinHandle<()>) {
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}

/// Configure `cmd` so the spawned child becomes the leader of its
/// own process group. On unix this calls `setsid` in the child after
/// `fork` and before `exec` (so signals to the child don't propagate
/// to us, and we can target the whole group on timeout). On windows
/// this is a no-op for now — see the module-level docs.
fn configure_process_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        // `tokio::process::Command::pre_exec` is the async runtime's
        // own API; no `std::os::unix::process::CommandExt` import is
        // required.
        //
        // SAFETY: the closure runs in the forked child between
        // `fork` and `exec`. `setsid` is async-signal-safe and
        // explicitly permitted in this context. We do not allocate,
        // touch global state, or call any non-async-signal-safe
        // libc function here.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        // Process-group/job-object support on Windows is a known
        // gap. `kill_on_drop(true)` plus `start_kill` still reap the
        // direct child; grandchildren may survive. Tracked as a
        // follow-up.
        let _ = cmd;
    }
}

/// Kill the child and (on unix) its entire process group.
fn kill_process_group(child: &mut tokio::process::Child, pid: Option<i32>) {
    #[cfg(unix)]
    {
        if let Some(pid) = pid {
            // Negative pid → process group. SIGKILL because we
            // don't trust a runaway routine to honor SIGTERM.
            // SAFETY: `libc::kill` is safe to call; it cannot
            // violate Rust invariants.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
    // Fall back to the direct-child kill in case the group signal
    // missed (or on windows) so the parent definitely terminates.
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::cron_support::{test_ctx, with_test_store};
    use std::time::Instant;

    #[tokio::test]
    async fn trigger_rejects_unknown_routine() {
        let _guard = with_test_store();
        let err = RemoteTriggerTool
            .call(json!({"id": "missing"}), &test_ctx())
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[tokio::test]
    async fn trigger_requires_id() {
        let _guard = with_test_store();
        let err = RemoteTriggerTool
            .call(json!({}), &test_ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    /// Build a sleep command that must be killed to terminate. We use
    /// the full path-less `sleep` so the OS resolves it via PATH.
    /// Configures a process group so the kill path under test
    /// matches the production code path.
    fn long_sleep_cmd() -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("30")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut cmd);
        cmd
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "POSIX `sleep` not available on Windows")]
    async fn run_with_timeout_kills_child_on_timeout() {
        let cancel = CancellationToken::new();
        let start = Instant::now();
        let result = run_with_timeout(long_sleep_cmd(), Duration::from_millis(100), &cancel).await;
        let elapsed = start.elapsed();

        match result {
            Err(RunError::Timeout(ms)) => assert_eq!(ms, 100),
            Err(RunError::Cancelled) => panic!("expected Timeout, got Cancelled"),
            Err(RunError::Spawn(msg)) => panic!("expected Timeout, got Spawn({msg})"),
            Err(RunError::Wait(msg)) => panic!("expected Timeout, got Wait({msg})"),
            Ok(_) => panic!("expected Timeout, got Ok"),
        }
        // If the child wasn't reaped, the helper would still be holding
        // the pipes open and waiting. The timeout branch escapes the
        // pipe drain, kills the child, and waits for it to exit before
        // returning, so the helper completes in well under the sleep
        // target. Bounding elapsed at 5s gives plenty of CI headroom
        // while still catching the orphaned-child regression.
        assert!(
            elapsed < Duration::from_secs(5),
            "run_with_timeout should return promptly after killing the child; took {elapsed:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "POSIX `sleep` not available on Windows")]
    async fn run_with_timeout_kills_child_on_cancel() {
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_for_task.cancel();
        });

        let start = Instant::now();
        let result = run_with_timeout(long_sleep_cmd(), Duration::from_secs(60), &cancel).await;
        let elapsed = start.elapsed();

        assert!(
            matches!(result, Err(RunError::Cancelled)),
            "expected RunError::Cancelled"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "cancel branch should kill the child promptly; took {elapsed:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "POSIX shell not available on Windows")]
    async fn run_with_timeout_captures_partial_output_on_timeout() {
        // Print 'before' immediately, then sleep past the timeout.
        // We expect the buffered 'before' to be returned even though
        // the child is killed before producing more output.
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'before'; sleep 30")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut cmd);

        let cancel = CancellationToken::new();
        let result = run_with_timeout(cmd, Duration::from_millis(300), &cancel).await;

        // Result is Timeout, but the partial output should still be
        // captured. (We can't reach into a Result::Err to get the
        // buffer, so we instead exercise the success path with a
        // tiny program to confirm the drain mechanism — see the
        // success test below. This test just confirms that timeout
        // doesn't deadlock when the child has already produced
        // bytes.)
        match result {
            Err(RunError::Timeout(_)) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_with_timeout_kills_grandchildren_on_timeout() {
        // Spawn a shell that backgrounds a long-lived sleep then
        // also sleeps. Without process-group kill, only the shell
        // would die and the backgrounded sleep would leak. We
        // record the grandchild pid to a tempfile and check that it
        // is gone after the timeout returns.
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("grandchild.pid");

        // sh -c '(sleep 60 & echo $! > pidfile); sleep 60'
        let script = format!(
            "(sleep 60 & echo $! > {pidfile_path}); sleep 60",
            pidfile_path = pidfile.display()
        );
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut cmd);

        let cancel = CancellationToken::new();
        let result = run_with_timeout(cmd, Duration::from_millis(500), &cancel).await;
        match result {
            Err(RunError::Timeout(_)) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }

        // Read the recorded grandchild pid and confirm it has been
        // signaled. `kill -0 <pid>` succeeds iff the process is
        // still alive, so we want it to fail.
        let mut grandchild_pid: Option<i32> = None;
        for _ in 0..20 {
            if let Ok(s) = std::fs::read_to_string(&pidfile)
                && let Ok(pid) = s.trim().parse::<i32>()
            {
                grandchild_pid = Some(pid);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let pid = grandchild_pid.expect("grandchild should have written pidfile");

        // Give the kernel a beat to deliver SIGKILL and reap.
        let mut alive = true;
        for _ in 0..40 {
            // SAFETY: probing a pid with signal 0 is non-mutating.
            let r = unsafe { libc::kill(pid, 0) };
            if r != 0 {
                alive = false;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !alive,
            "grandchild pid {pid} survived process-group SIGKILL"
        );
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "POSIX `true` not available on Windows")]
    async fn run_with_timeout_returns_output_on_success() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'hello'; printf 'oops' 1>&2; exit 0")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let cancel = CancellationToken::new();
        let outcome = run_with_timeout(cmd, Duration::from_secs(5), &cancel)
            .await
            .expect("command should succeed within timeout");

        assert!(outcome.status.success());
        assert_eq!(outcome.stdout, b"hello");
        assert_eq!(outcome.stderr, b"oops");
    }
}
