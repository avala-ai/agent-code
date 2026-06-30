//! Background task execution.
//!
//! Manages tasks that run asynchronously while the user continues
//! interacting with the agent. Tasks output to files and notify
//! the user when complete.
//!
//! # Task model
//!
//! Tasks are kind-tagged (see [`TaskKind`]) so the same queue can
//! carry shell commands, subagent runs, MCP monitors, and idle-time
//! "dream" jobs. Each kind carries kind-specific data in
//! [`TaskPayload`]; the [`crate::tools::tasks::executor`] module
//! defines the per-kind executor trait and registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::subagent_colors::SubagentColor;

/// Unique task identifier.
pub type TaskId = String;

/// Status of a background task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed(String),
    Killed,
}

/// What kind of work a task represents.
///
/// All kinds share the same queue, persistence layout, and lifecycle —
/// the kind determines which executor runs the work and how the
/// kind-specific [`TaskPayload`] is interpreted.
///
/// `LocalShell` is the legacy default: a record without a `kind`
/// field on disk deserializes as `LocalShell` so older state files
/// keep working. See [`TaskKind::default`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// User-issued shell command run via the Bash tool.
    ///
    /// Marked `#[default]` so legacy task records (pre-8.13) without
    /// a `kind` field round-trip cleanly through serde.
    #[default]
    LocalShell,
    /// An Agent-tool subagent run.
    LocalAgent,
    /// A multi-step skill / workflow run.
    LocalWorkflow,
    /// A "watch an MCP server" task.
    MonitorMcp,
    /// A cloud-runtime / RemoteTrigger run. Stub for 8.14.
    RemoteAgent,
    /// Idle-time background work.
    Dream,
}

impl TaskKind {
    /// Stable, human-friendly label used in `/tasks` output and
    /// surfaced through the `TaskList` / `TaskGet` tool results.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::LocalShell => "LocalShell",
            Self::LocalAgent => "LocalAgent",
            Self::LocalWorkflow => "LocalWorkflow",
            Self::MonitorMcp => "MonitorMcp",
            Self::RemoteAgent => "RemoteAgent",
            Self::Dream => "Dream",
        }
    }

    /// Parse a kind from its `as_str` form (case-insensitive). Used by
    /// `TaskCreate` so the model can pass `"local_agent"` / `"LocalAgent"`
    /// interchangeably.
    pub fn parse(s: &str) -> Option<Self> {
        let normalized = s.replace('_', "").to_ascii_lowercase();
        match normalized.as_str() {
            "localshell" => Some(Self::LocalShell),
            "localagent" => Some(Self::LocalAgent),
            "localworkflow" => Some(Self::LocalWorkflow),
            "monitormcp" => Some(Self::MonitorMcp),
            "remoteagent" => Some(Self::RemoteAgent),
            "dream" => Some(Self::Dream),
            _ => None,
        }
    }
}

/// Kind-specific data carried alongside a task record.
///
/// Serialized with the standard tagged-enum form so the persisted
/// shape is `{ "kind": "local_agent", "payload": { ... } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum TaskPayload {
    /// A shell command launched via the Bash tool.
    LocalShell {
        /// Command to run.
        command: String,
        /// Working directory the command was launched from.
        cwd: PathBuf,
    },
    /// A subagent run dispatched through the Agent tool.
    LocalAgent {
        /// Optional subagent kind (e.g. a named agent profile).
        subagent_kind: Option<String>,
        /// Prompt the subagent should execute.
        prompt: String,
        /// Parent session id, when one is available.
        parent_session: Option<String>,
    },
    /// A multi-step skill / workflow execution.
    LocalWorkflow {
        /// Slug of the skill / workflow to run.
        workflow: String,
        /// Free-form arguments forwarded to the workflow.
        args: serde_json::Value,
    },
    /// Watch an MCP server for events.
    MonitorMcp {
        /// Configured MCP server name.
        server_name: String,
        /// Optional tool the watcher expects to fire.
        expected_tool: Option<String>,
        /// How long to keep watching before giving up.
        timeout: Duration,
    },
    /// A cloud-runtime / RemoteTrigger run. Stub for 8.14.
    RemoteAgent {
        /// Stored routine id to trigger.
        routine_id: String,
        /// Optional wall-clock cap on the run.
        timeout: Option<Duration>,
    },
    /// Idle-time background work. Free-form payload so the dream
    /// executor can stash whatever signal it needs to resume.
    Dream { note: Option<String> },
}

impl TaskPayload {
    /// Map a payload back to its [`TaskKind`].
    pub fn kind(&self) -> TaskKind {
        match self {
            Self::LocalShell { .. } => TaskKind::LocalShell,
            Self::LocalAgent { .. } => TaskKind::LocalAgent,
            Self::LocalWorkflow { .. } => TaskKind::LocalWorkflow,
            Self::MonitorMcp { .. } => TaskKind::MonitorMcp,
            Self::RemoteAgent { .. } => TaskKind::RemoteAgent,
            Self::Dream { .. } => TaskKind::Dream,
        }
    }
}

/// Metadata for a running or completed background task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: TaskId,
    pub description: String,
    pub status: TaskStatus,
    pub output_file: PathBuf,
    /// What kind of work this task represents. Defaults to
    /// `LocalShell` so records persisted before the kind field existed
    /// continue to round-trip through serde.
    #[serde(default)]
    pub kind: TaskKind,
    /// Kind-specific payload. `None` is the documented default for
    /// legacy records — the task simply has no extra data attached.
    #[serde(default)]
    pub payload: Option<TaskPayload>,
    /// Stable display color for this task, when one applies.
    ///
    /// Set for `LocalAgent` runs by
    /// [`crate::services::subagent_colors::SubagentColorManager`] so
    /// `/tasks` and tool output can show each spawned subagent in a
    /// distinct color. `None` for shell tasks and other non-subagent
    /// kinds. Round-trips through serde so persisted task state keeps
    /// its color across reloads.
    #[serde(default)]
    pub subagent_color: Option<SubagentColor>,
    /// Whether this task's terminal completion has already been
    /// surfaced to the user. Set by [`TaskManager::drain_completions`]
    /// so a polling caller (the REPL loop) reports each finished task
    /// exactly once. `#[serde(default)]` so legacy records load as
    /// un-notified.
    #[serde(default)]
    pub notified: bool,
    /// OS process id of the task's running process, when it owns one
    /// (`LocalShell`, and the background `LocalAgent` subprocess). Used
    /// after a restart to probe whether an adopted task is still alive.
    /// `None` for tasks without a process or before the child spawns.
    #[serde(default)]
    pub pid: Option<u32>,
    /// Wall-clock instants are not portable across processes; skip
    /// them in the persisted form.
    #[serde(skip, default = "std::time::Instant::now")]
    pub started_at: std::time::Instant,
    #[serde(skip, default)]
    pub finished_at: Option<std::time::Instant>,
}

/// Manages background task lifecycle.
pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<TaskId, TaskInfo>>>,
    next_id: Arc<Mutex<u64>>,
    /// Cancellation handles for tasks that own a live process or future
    /// (currently `LocalShell`). [`Self::kill`] fires the token so the
    /// spawned runtime can terminate the work — and, on Unix, the whole
    /// process group — instead of merely flipping a status field.
    cancels: Arc<Mutex<HashMap<TaskId, tokio_util::sync::CancellationToken>>>,
    /// When set, each task's [`TaskInfo`] is journaled to
    /// `<persist_dir>/<id>.json` on every transition, and [`Self::adopt`]
    /// can reload tasks from it after a restart. `None` keeps the manager
    /// purely in-memory (the default; used by tests).
    persist_dir: Option<PathBuf>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            persist_dir: None,
        }
    }

    /// Construct a manager that journals task state to `dir` so tasks
    /// survive a process restart (see [`Self::adopt`]).
    pub fn with_persistence(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            persist_dir: Some(dir),
            ..Self::new()
        }
    }

    /// Journal a task record if persistence is enabled. Best-effort:
    /// a failed write is logged, never fatal.
    fn persist(&self, info: &TaskInfo) {
        if let Some(dir) = &self.persist_dir {
            persist_info(dir, info);
        }
    }

    /// Remove a task's journal file if persistence is enabled.
    fn unpersist(&self, id: &str) {
        if let Some(dir) = &self.persist_dir {
            let _ = std::fs::remove_file(journal_path(dir, id));
        }
    }

    /// Spawn a background shell command.
    pub async fn spawn_shell(
        &self,
        command: &str,
        description: &str,
        cwd: &Path,
    ) -> Result<TaskId, String> {
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(command).current_dir(cwd);
        let payload = TaskPayload::LocalShell {
            command: command.to_string(),
            cwd: cwd.to_path_buf(),
        };
        Ok(self
            .spawn_command(cmd, description, TaskKind::LocalShell, payload, None)
            .await)
    }

    /// Spawn an arbitrary prebuilt command as a tracked background task.
    ///
    /// The caller constructs `cmd` (program, args, cwd, env); this
    /// method owns the rest of the lifecycle: it registers a queue
    /// entry, enforces piped stdio and — on Unix — an isolated process
    /// group so [`Self::kill`] can terminate the whole tree, runs the
    /// process, captures its output to the task's output file, and
    /// records the terminal status. Used by [`Self::spawn_shell`] and by
    /// the Agent tool's background path (a subagent subprocess).
    pub async fn spawn_command(
        &self,
        mut cmd: tokio::process::Command,
        description: &str,
        kind: TaskKind,
        payload: TaskPayload,
        color: Option<SubagentColor>,
    ) -> TaskId {
        let id = self.allocate_id(id_prefix_for(kind)).await;
        let output_file = task_output_path(&id);
        if let Some(parent) = output_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let info = TaskInfo {
            id: id.clone(),
            description: description.to_string(),
            status: TaskStatus::Running,
            output_file,
            kind,
            payload: Some(payload),
            subagent_color: color,
            notified: false,
            pid: None,
            started_at: std::time::Instant::now(),
            finished_at: None,
        };
        self.persist(&info);
        self.tasks.lock().await.insert(id.clone(), info);

        // Register a cancellation handle so `kill()` can terminate the
        // live process (and its children) rather than only flipping the
        // status field.
        let cancel = tokio_util::sync::CancellationToken::new();
        self.cancels.lock().await.insert(id.clone(), cancel.clone());

        // Capture output and isolate the process group for killability.
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);

        let task_id = id.clone();
        let tasks = self.tasks.clone();
        let cancels = self.cancels.clone();
        let persist_dir = self.persist_dir.clone();
        tokio::spawn(async move {
            run_command_task(&task_id, cmd, &tasks, cancel, persist_dir).await;
            // Drop the cancel handle once the task is terminal.
            cancels.lock().await.remove(&task_id);
        });

        debug!("Background {kind:?} task {id} started: {description}");
        id
    }

    /// Register a non-shell task in the queue.
    ///
    /// Used by the kind-specific executors (`LocalAgent`, `MonitorMcp`,
    /// etc.) — they handle their own runtime, but still want a queue
    /// entry so the task is visible to `/tasks`, `TaskList`, and
    /// `TaskGet`. The caller is responsible for transitioning the
    /// status when the work finishes; see [`Self::set_status`].
    pub async fn register(
        &self,
        description: &str,
        kind: TaskKind,
        payload: TaskPayload,
    ) -> TaskId {
        self.register_with_color(description, kind, payload, None)
            .await
    }

    /// Register a non-shell task with an explicit display color.
    ///
    /// Same as [`Self::register`] but additionally stamps a
    /// [`SubagentColor`] on the [`TaskInfo`]. Used by the
    /// `LocalAgent` executor to record the color the
    /// [`crate::services::subagent_colors::SubagentColorManager`]
    /// allocated for this run, so `/tasks` and downstream renderers
    /// can paint the entry without re-doing the lookup.
    pub async fn register_with_color(
        &self,
        description: &str,
        kind: TaskKind,
        payload: TaskPayload,
        color: Option<SubagentColor>,
    ) -> TaskId {
        let id = self.allocate_id(id_prefix_for(kind)).await;
        let output_file = task_output_path(&id);
        if let Some(parent) = output_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let info = TaskInfo {
            id: id.clone(),
            description: description.to_string(),
            status: TaskStatus::Running,
            output_file,
            kind,
            payload: Some(payload),
            subagent_color: color,
            notified: false,
            pid: None,
            started_at: std::time::Instant::now(),
            finished_at: None,
        };
        self.persist(&info);
        self.tasks.lock().await.insert(id.clone(), info);
        debug!("Registered {kind:?} task {id}: {description}");
        id
    }

    /// Update the status of an existing task. Used by the kind-specific
    /// executors when their externally-driven work completes.
    pub async fn set_status(&self, id: &str, status: TaskStatus) -> Result<(), String> {
        let snapshot = {
            let mut tasks = self.tasks.lock().await;
            let info = tasks
                .get_mut(id)
                .ok_or_else(|| format!("Task '{id}' not found"))?;
            let now_finished = matches!(
                status,
                TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Killed,
            );
            info.status = status;
            if now_finished && info.finished_at.is_none() {
                info.finished_at = Some(std::time::Instant::now());
            }
            info.clone()
        };
        self.persist(&snapshot);
        Ok(())
    }

    /// Get the status of a task.
    pub async fn get_status(&self, id: &str) -> Option<TaskInfo> {
        self.tasks.lock().await.get(id).cloned()
    }

    /// Read the output of a completed task.
    pub async fn read_output(&self, id: &str) -> Result<String, String> {
        let tasks = self.tasks.lock().await;
        let info = tasks
            .get(id)
            .ok_or_else(|| format!("Task '{id}' not found"))?;
        std::fs::read_to_string(&info.output_file)
            .map_err(|e| format!("Failed to read output: {e}"))
    }

    /// List all tasks.
    pub async fn list(&self) -> Vec<TaskInfo> {
        self.tasks.lock().await.values().cloned().collect()
    }

    /// Kill a running task.
    ///
    /// Flips the status to [`TaskStatus::Killed`] and, when the task
    /// owns a live process/future (it registered a cancellation handle
    /// in [`Self::spawn_shell`]), fires that handle so the spawned
    /// runtime actually terminates the work — on Unix the entire
    /// process group, so child processes are not orphaned. Tasks
    /// without a handle (externally-driven kinds such as `LocalAgent`)
    /// just get the status transition; their executor observes it.
    pub async fn kill(&self, id: &str) -> Result<(), String> {
        let snapshot = {
            let mut tasks = self.tasks.lock().await;
            let info = tasks
                .get_mut(id)
                .ok_or_else(|| format!("Task '{id}' not found"))?;
            if info.status == TaskStatus::Running {
                info.status = TaskStatus::Killed;
                info.finished_at = Some(std::time::Instant::now());
            }
            info.clone()
        };
        self.persist(&snapshot);
        // Signal the live runtime (if any) outside the tasks lock.
        if let Some(cancel) = self.cancels.lock().await.get(id) {
            cancel.cancel();
        }
        Ok(())
    }

    /// Collect newly-finished tasks for user notification, exactly once
    /// each.
    ///
    /// Returns terminal tasks (`Completed` / `Failed`) that have not yet
    /// been surfaced, and marks them `notified` so a polling caller
    /// (the REPL loop) never reports the same completion twice.
    /// `Killed` tasks are user-initiated, so they are not surfaced here.
    pub async fn drain_completions(&self) -> Vec<TaskInfo> {
        let mut drained = Vec::new();
        {
            let mut tasks = self.tasks.lock().await;
            for info in tasks.values_mut() {
                if !info.notified
                    && matches!(info.status, TaskStatus::Completed | TaskStatus::Failed(_))
                {
                    info.notified = true;
                    drained.push(info.clone());
                }
            }
        }
        // Persist the notified flip so a restart doesn't re-surface these.
        for info in &drained {
            self.persist(info);
        }
        drained
    }

    /// Reload journaled tasks after a restart.
    ///
    /// For each task in the persistence directory: a still-`Running`
    /// task whose process is gone is reclassified `Failed`; one whose
    /// process is somehow still alive (an orphan) stays `Running` so the
    /// user can see and kill it. Already-notified terminal tasks are
    /// pruned (their completion was surfaced in a prior session).
    /// Terminal-but-unnotified tasks are loaded so the next
    /// [`Self::drain_completions`] surfaces them exactly once. Returns
    /// the number of tasks adopted into the live set.
    pub async fn adopt(&self) -> usize {
        let Some(dir) = self.persist_dir.clone() else {
            return 0;
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return 0,
        };

        let mut adopted = 0usize;
        let mut tasks = self.tasks.lock().await;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let info: TaskInfo = match std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
            {
                Some(i) => i,
                None => continue,
            };

            // Don't clobber a task already live in this session.
            if tasks.contains_key(&info.id) {
                continue;
            }
            // Prune already-surfaced terminal tasks.
            let terminal = !matches!(info.status, TaskStatus::Running);
            if terminal && info.notified {
                let _ = std::fs::remove_file(&path);
                continue;
            }

            let alive = info.pid.map(pid_alive).unwrap_or(false);
            let info = reclassify_on_adopt(info, alive);
            persist_info(&dir, &info);
            tasks.insert(info.id.clone(), info);
            adopted += 1;
        }

        if adopted > 0 {
            info!("Adopted {adopted} background task(s) from a previous session");
        }
        adopted
    }

    async fn allocate_id(&self, prefix: &str) -> TaskId {
        let mut next = self.next_id.lock().await;
        let id = format!("{prefix}{next}");
        *next += 1;
        id
    }
}

/// Run a prebuilt command to completion as a background task, honoring
/// cancellation.
///
/// `cmd` is expected to already have piped stdio and (on Unix) its own
/// process group — [`TaskManager::spawn_command`] configures both.
/// Output is drained concurrently with the wait so a chatty command
/// cannot deadlock by filling the pipe buffer. On cancellation the
/// child is killed — on Unix the whole process group, so descendants
/// spawned by the command are not orphaned. The task's terminal status
/// is written under the `tasks` lock; a `Killed` status set by
/// [`TaskManager::kill`] is preserved (not overwritten with an exit
/// result from the race).
async fn run_command_task(
    task_id: &str,
    mut cmd: tokio::process::Command,
    tasks: &Arc<Mutex<HashMap<TaskId, TaskInfo>>>,
    cancel: tokio_util::sync::CancellationToken,
    persist_dir: Option<PathBuf>,
) {
    use tokio::io::AsyncReadExt;

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            finalize_shell_task(
                task_id,
                tasks,
                TaskStatus::Failed(e.to_string()),
                &e.to_string(),
                &persist_dir,
            )
            .await;
            return;
        }
    };

    // Record the OS pid so a restart can probe whether an adopted task
    // is still alive. Also used for the Unix process-group kill below.
    let pid = child.id();
    {
        let mut t = tasks.lock().await;
        if let Some(info) = t.get_mut(task_id) {
            info.pid = pid;
            if let Some(dir) = &persist_dir {
                persist_info(dir, info);
            }
        }
    }

    // Drain stdout/stderr concurrently with the wait. Reader tasks own
    // the pipe handles so they don't borrow `child`.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let out_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(p) = stdout_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(p) = stderr_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    });

    let mut exit_status = None;
    let mut was_killed = false;
    tokio::select! {
        res = child.wait() => { exit_status = res.ok(); }
        _ = cancel.cancelled() => {
            // Terminate the whole process group on Unix; fall back to
            // killing just the child elsewhere.
            #[cfg(unix)]
            if let Some(pid) = pid {
                unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL); }
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
            was_killed = true;
        }
    }

    let out_buf = out_task.await.unwrap_or_default();
    let err_buf = err_task.await.unwrap_or_default();

    let mut content = String::new();
    let stdout = String::from_utf8_lossy(&out_buf);
    let stderr = String::from_utf8_lossy(&err_buf);
    if !stdout.is_empty() {
        content.push_str(&stdout);
    }
    if !stderr.is_empty() {
        content.push_str("\nstderr:\n");
        content.push_str(&stderr);
    }

    let status = if was_killed {
        // `kill()` already set `Killed`; keep it.
        TaskStatus::Killed
    } else {
        match exit_status {
            Some(s) if s.success() => TaskStatus::Completed,
            Some(s) => TaskStatus::Failed(format!("Exit code: {}", s.code().unwrap_or(-1))),
            None => TaskStatus::Failed("process wait failed".to_string()),
        }
    };

    finalize_shell_task(task_id, tasks, status, &content, &persist_dir).await;
}

/// Write a shell task's output and record its terminal status.
///
/// A `Killed` status set by [`TaskManager::kill`] is never overwritten:
/// if the live record is already terminal we only persist the captured
/// output, avoiding a race where a near-simultaneous natural exit would
/// clobber the user-requested `Killed` state.
async fn finalize_shell_task(
    task_id: &str,
    tasks: &Arc<Mutex<HashMap<TaskId, TaskInfo>>>,
    status: TaskStatus,
    content: &str,
    persist_dir: &Option<PathBuf>,
) {
    let mut tasks = tasks.lock().await;
    if let Some(info) = tasks.get_mut(task_id) {
        let _ = std::fs::write(&info.output_file, content);
        let already_terminal = !matches!(info.status, TaskStatus::Running);
        if !already_terminal {
            info.status = status;
            info.finished_at = Some(std::time::Instant::now());
        }
        if let Some(dir) = persist_dir {
            persist_info(dir, info);
        }
        info!("Background task {} finished: {:?}", task_id, info.status);
    }
}

/// Journal one task record as `<dir>/<id>.json`. Best-effort.
fn persist_info(dir: &Path, info: &TaskInfo) {
    let _ = std::fs::create_dir_all(dir);
    match serde_json::to_vec_pretty(info) {
        Ok(bytes) => {
            let _ = std::fs::write(journal_path(dir, &info.id), bytes);
        }
        Err(e) => debug!("failed to serialize task {} for journal: {e}", info.id),
    }
}

/// Path of a task's journal file.
fn journal_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// Probe whether a process id is still alive.
///
/// On Unix, `kill(pid, 0)` returns 0 when the process exists (and
/// `EPERM` when it exists but we can't signal it) — both mean alive.
/// On other platforms we conservatively report `false` (treat as gone).
fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        // 0 → exists; EPERM → exists but unsignalable. Both mean alive.
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Reclassify a journaled task on adopt.
///
/// A task that was `Running` when its owning process died is no longer
/// recoverable: if its process is gone we mark it `Failed`; if the
/// process is somehow still alive (an orphan) we keep it `Running` so a
/// caller can still see and kill it. Terminal tasks pass through
/// unchanged. Pure so it can be unit-tested with a synthetic liveness
/// value.
fn reclassify_on_adopt(mut info: TaskInfo, alive: bool) -> TaskInfo {
    if matches!(info.status, TaskStatus::Running) && !alive {
        info.status = TaskStatus::Failed("interrupted by restart".to_string());
        info.finished_at = Some(std::time::Instant::now());
    }
    info
}

/// Two-letter id prefix per kind so the `/tasks` table tells them
/// apart at a glance.
const fn id_prefix_for(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::LocalShell => "b",
        TaskKind::LocalAgent => "a",
        TaskKind::LocalWorkflow => "w",
        TaskKind::MonitorMcp => "m",
        TaskKind::RemoteAgent => "r",
        TaskKind::Dream => "d",
    }
}

/// Directory where task journals (`<id>.json`) and captured output
/// (`<id>.out`) live. Shared across sessions so [`TaskManager::adopt`]
/// can recover tasks left by a previous run.
pub fn tasks_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-code")
        .join("tasks")
}

/// Path where task output is stored.
fn task_output_path(id: &TaskId) -> PathBuf {
    tasks_dir().join(format!("{id}.out"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll until a task leaves `Running`, or give up after `timeout_ms`.
    #[cfg(unix)]
    async fn wait_terminal(mgr: &TaskManager, id: &str, timeout_ms: u64) -> TaskStatus {
        let start = std::time::Instant::now();
        loop {
            match mgr.get_status(id).await {
                Some(info) if !matches!(info.status, TaskStatus::Running) => return info.status,
                _ => {}
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                return mgr
                    .get_status(id)
                    .await
                    .map(|i| i.status)
                    .unwrap_or(TaskStatus::Running);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn shell_payload(cmd: &str) -> TaskPayload {
        TaskPayload::LocalShell {
            command: cmd.to_string(),
            cwd: PathBuf::from("."),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_shell_completes_and_surfaces_exactly_once() {
        let mgr = TaskManager::new();
        let id = mgr
            .spawn_shell("echo hello", "echo", Path::new("."))
            .await
            .unwrap();

        let status = wait_terminal(&mgr, &id, 5_000).await;
        assert_eq!(status, TaskStatus::Completed);

        let out = mgr.read_output(&id).await.unwrap();
        assert!(out.contains("hello"), "unexpected output: {out:?}");

        // drain_completions surfaces the finished task once, then never
        // again (notified de-dup).
        let first = mgr.drain_completions().await;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, id);
        assert!(mgr.drain_completions().await.is_empty());
    }

    #[tokio::test]
    async fn drain_skips_running_and_killed_surfaces_failed() {
        let mgr = TaskManager::new();

        let running = mgr
            .register("run", TaskKind::LocalAgent, shell_payload("noop"))
            .await;
        let failed = mgr
            .register("fail", TaskKind::LocalShell, shell_payload("noop"))
            .await;
        mgr.set_status(&failed, TaskStatus::Failed("boom".into()))
            .await
            .unwrap();
        let killed = mgr
            .register("kill", TaskKind::LocalShell, shell_payload("noop"))
            .await;
        mgr.set_status(&killed, TaskStatus::Killed).await.unwrap();

        let ids: Vec<String> = mgr
            .drain_completions()
            .await
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert!(ids.contains(&failed), "failed task should surface");
        assert!(!ids.contains(&running), "running task must not surface");
        assert!(!ids.contains(&killed), "killed task must not surface");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_command_uses_kind_prefix_and_captures_output() {
        // The generic path the Agent background mode relies on: run an
        // arbitrary command as a LocalAgent task, capture its output,
        // and surface it once.
        let mgr = TaskManager::new();
        let mut cmd = tokio::process::Command::new("echo");
        cmd.arg("agent-said-hi");
        let payload = TaskPayload::LocalAgent {
            subagent_kind: Some("demo".into()),
            prompt: "noop".into(),
            parent_session: None,
        };
        let id = mgr
            .spawn_command(cmd, "demo", TaskKind::LocalAgent, payload, None)
            .await;

        assert!(
            id.starts_with('a'),
            "LocalAgent id should use 'a' prefix: {id}"
        );
        assert_eq!(wait_terminal(&mgr, &id, 5_000).await, TaskStatus::Completed);
        let out = mgr.read_output(&id).await.unwrap();
        assert!(out.contains("agent-said-hi"), "output: {out:?}");
        assert_eq!(mgr.drain_completions().await.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kill_actually_terminates_the_process() {
        // The command waits, then writes a sentinel. A real kill stops
        // the process before the sentinel is ever created; a status-only
        // flip would leave the process running and the file would appear.
        let sentinel =
            std::env::temp_dir().join(format!("agentcode-killtest-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&sentinel);
        let cmd = format!("sleep 2; touch {}", sentinel.display());

        let mgr = TaskManager::new();
        let id = mgr
            .spawn_shell(&cmd, "sleeper", Path::new("."))
            .await
            .unwrap();
        // Let the process actually start.
        tokio::time::sleep(Duration::from_millis(250)).await;

        mgr.kill(&id).await.unwrap();
        let status = wait_terminal(&mgr, &id, 5_000).await;
        assert_eq!(status, TaskStatus::Killed);

        // Wait past the command's own sleep; the sentinel must not exist.
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert!(
            !sentinel.exists(),
            "process survived kill — sentinel was created"
        );
        let _ = std::fs::remove_file(&sentinel);
    }

    // ---- durability / adopt ----

    fn unique_persist_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentcode-persist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mk_info(id: &str, status: TaskStatus) -> TaskInfo {
        TaskInfo {
            id: id.into(),
            description: "t".into(),
            status,
            output_file: tasks_dir().join(format!("{id}.out")),
            kind: TaskKind::LocalShell,
            payload: None,
            subagent_color: None,
            notified: false,
            pid: None,
            started_at: std::time::Instant::now(),
            finished_at: None,
        }
    }

    #[test]
    fn reclassify_running_dead_becomes_failed() {
        let out = reclassify_on_adopt(mk_info("b1", TaskStatus::Running), false);
        assert!(matches!(out.status, TaskStatus::Failed(_)));
        assert!(out.finished_at.is_some());
    }

    #[test]
    fn reclassify_running_alive_stays_running() {
        let out = reclassify_on_adopt(mk_info("b2", TaskStatus::Running), true);
        assert_eq!(out.status, TaskStatus::Running);
    }

    #[test]
    fn reclassify_terminal_passes_through() {
        let out = reclassify_on_adopt(mk_info("b3", TaskStatus::Completed), false);
        assert_eq!(out.status, TaskStatus::Completed);
    }

    #[cfg(unix)]
    #[test]
    fn pid_alive_detects_self_and_missing() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0x7FFF_FFFE));
    }

    #[tokio::test]
    async fn register_persists_a_journal_when_enabled() {
        let dir = unique_persist_dir();
        let mgr = TaskManager::with_persistence(dir.clone());
        let id = mgr
            .register("t", TaskKind::LocalAgent, TaskPayload::Dream { note: None })
            .await;
        assert!(journal_path(&dir, &id).exists(), "register should journal");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn adopt_reclassifies_dead_running_task_and_replays_once() {
        let dir = unique_persist_dir();
        let mut info = mk_info("b777", TaskStatus::Running);
        info.pid = Some(0x7FFF_FFFE); // implausible pid → not alive
        persist_info(&dir, &info);

        let mgr = TaskManager::with_persistence(dir.clone());
        assert_eq!(mgr.adopt().await, 1);

        let got = mgr.get_status("b777").await.unwrap();
        assert!(
            matches!(got.status, TaskStatus::Failed(_)),
            "expected Failed after adopt, got {:?}",
            got.status
        );

        // The reclassified, un-notified task surfaces exactly once.
        assert_eq!(mgr.drain_completions().await.len(), 1);
        assert!(mgr.drain_completions().await.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn adopt_prunes_already_notified_terminal_task() {
        let dir = unique_persist_dir();
        let mut info = mk_info("b888", TaskStatus::Completed);
        info.notified = true;
        persist_info(&dir, &info);

        let mgr = TaskManager::with_persistence(dir.clone());
        assert_eq!(
            mgr.adopt().await,
            0,
            "notified terminal task should be pruned, not adopted"
        );
        assert!(mgr.get_status("b888").await.is_none());
        assert!(
            !journal_path(&dir, "b888").exists(),
            "pruned task's journal should be removed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
