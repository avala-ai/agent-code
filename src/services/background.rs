//! Background task execution.
//!
//! Manages tasks that run asynchronously while the user continues
//! interacting with the agent. Tasks output to files and notify
//! the user when complete.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info};

/// Unique task identifier.
pub type TaskId = String;

/// Status of a background task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Completed,
    Failed(String),
    Killed,
}

/// Metadata for a running or completed background task.
#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub id: TaskId,
    pub description: String,
    pub status: TaskStatus,
    pub output_file: PathBuf,
    pub started_at: std::time::Instant,
    pub finished_at: Option<std::time::Instant>,
}

/// Manages background task lifecycle.
pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<TaskId, TaskInfo>>>,
    next_id: Arc<Mutex<u64>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    /// Spawn a background shell command.
    pub async fn spawn_shell(
        &self,
        command: &str,
        description: &str,
        cwd: &Path,
    ) -> Result<TaskId, String> {
        let id = self.allocate_id("b").await;
        let output_file = task_output_path(&id);

        // Ensure output directory exists.
        if let Some(parent) = output_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let info = TaskInfo {
            id: id.clone(),
            description: description.to_string(),
            status: TaskStatus::Running,
            output_file: output_file.clone(),
            started_at: std::time::Instant::now(),
            finished_at: None,
        };

        self.tasks.lock().await.insert(id.clone(), info);

        // Spawn the process.
        let task_id = id.clone();
        let tasks = self.tasks.clone();
        let command = command.to_string();
        let cwd = cwd.to_path_buf();

        tokio::spawn(async move {
            let result = tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&command)
                .current_dir(&cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;

            let mut tasks = tasks.lock().await;
            if let Some(info) = tasks.get_mut(&task_id) {
                info.finished_at = Some(std::time::Instant::now());

                match result {
                    Ok(output) => {
                        let mut content = String::new();
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stdout.is_empty() {
                            content.push_str(&stdout);
                        }
                        if !stderr.is_empty() {
                            content.push_str("\nstderr:\n");
                            content.push_str(&stderr);
                        }
                        let _ = std::fs::write(&info.output_file, &content);

                        if output.status.success() {
                            info.status = TaskStatus::Completed;
                        } else {
                            info.status = TaskStatus::Failed(format!(
                                "Exit code: {}",
                                output.status.code().unwrap_or(-1)
                            ));
                        }
                    }
                    Err(e) => {
                        info.status = TaskStatus::Failed(e.to_string());
                        let _ = std::fs::write(&info.output_file, e.to_string());
                    }
                }

                info!("Background task {} finished: {:?}", task_id, info.status);
            }
        });

        debug!("Background task {id} started: {description}");
        Ok(id)
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

    /// Kill a running task (best-effort).
    pub async fn kill(&self, id: &str) -> Result<(), String> {
        let mut tasks = self.tasks.lock().await;
        let info = tasks
            .get_mut(id)
            .ok_or_else(|| format!("Task '{id}' not found"))?;
        if info.status == TaskStatus::Running {
            info.status = TaskStatus::Killed;
            info.finished_at = Some(std::time::Instant::now());
        }
        Ok(())
    }

    /// Collect notifications for newly completed tasks.
    pub async fn drain_completions(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.lock().await;
        tasks
            .values()
            .filter(|t| matches!(t.status, TaskStatus::Completed | TaskStatus::Failed(_)))
            .cloned()
            .collect()
    }

    async fn allocate_id(&self, prefix: &str) -> TaskId {
        let mut next = self.next_id.lock().await;
        let id = format!("{prefix}{next}");
        *next += 1;
        id
    }
}

/// Path where task output is stored.
fn task_output_path(id: &TaskId) -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rust-code")
        .join("tasks");
    dir.join(format!("{id}.out"))
}
