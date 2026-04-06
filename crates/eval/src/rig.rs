use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A captured tool call from the agent's execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedToolCall {
    pub name: String,
    pub input: serde_json::Value,
    pub is_error: bool,
}

/// Activity event captured during eval execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub timestamp: String,
    pub event_type: String,
    pub data: serde_json::Value,
}

/// The test rig provides the execution environment for a single eval run.
///
/// ```text
/// ┌────────────────────────────────────────┐
/// │  TestRig                               │
/// │  ├─ workspace: temp dir with fixtures  │
/// │  ├─ tool_log: captured tool calls      │
/// │  ├─ activity_log: timestamped events   │
/// │  ├─ response_text: final agent output  │
/// │  └─ turn_count: number of agent turns  │
/// └────────────────────────────────────────┘
/// ```
pub struct TestRig {
    /// Temporary workspace directory (deleted on drop).
    pub workspace: PathBuf,
    _temp_dir: tempfile::TempDir,

    /// Captured tool calls from the agent run.
    pub tool_log: Vec<CapturedToolCall>,

    /// Timestamped activity events.
    pub activity_log: Vec<ActivityEvent>,

    /// The agent's final response text.
    pub response_text: String,

    /// Number of turns the agent took.
    pub turn_count: usize,

    /// Tools used (deduplicated names).
    pub tools_used: Vec<String>,

    /// Whether the agent run succeeded.
    pub success: bool,

    /// Raw stdout from the agent process.
    pub stdout: String,

    /// Raw stderr from the agent process.
    pub stderr: String,
}

impl TestRig {
    /// Create a new test rig with an empty workspace.
    pub fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let workspace = temp_dir.path().to_path_buf();

        Ok(Self {
            workspace,
            _temp_dir: temp_dir,
            tool_log: Vec::new(),
            activity_log: Vec::new(),
            response_text: String::new(),
            turn_count: 0,
            tools_used: Vec::new(),
            success: false,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    /// Create a test rig and copy fixture files into the workspace.
    pub fn with_fixture(fixture_path: &Path) -> Result<Self> {
        let rig = Self::new()?;

        if fixture_path.exists() {
            copy_dir_recursive(fixture_path, &rig.workspace)?;
        }

        Ok(rig)
    }

    /// Check if the tool log contains a call to the named tool.
    pub fn has_tool_call(&self, name: &str) -> bool {
        self.tool_log.iter().any(|tc| tc.name == name)
    }

    /// Get all calls to a specific tool.
    pub fn calls_to(&self, name: &str) -> Vec<&CapturedToolCall> {
        self.tool_log.iter().filter(|tc| tc.name == name).collect()
    }

    /// Read a file from the workspace.
    pub fn read_file(&self, relative_path: &str) -> Result<String> {
        let path = self.workspace.join(relative_path);
        std::fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))
    }

    /// Check if a file exists in the workspace.
    pub fn file_exists(&self, relative_path: &str) -> bool {
        self.workspace.join(relative_path).exists()
    }

    /// Run the agent binary against this workspace with the given prompt.
    ///
    /// Uses `agent --print --max-turns N --permission-mode auto-approve`
    /// to execute non-interactively and capture output.
    pub async fn run_agent(
        &mut self,
        agent_binary: &str,
        prompt: &str,
        max_turns: usize,
        env: &[(&str, &str)],
    ) -> Result<()> {
        let mut cmd = tokio::process::Command::new(agent_binary);
        cmd.arg("--print")
            .arg("--max-turns")
            .arg(max_turns.to_string())
            .arg("--permission-mode")
            .arg("auto-approve")
            .arg("-C")
            .arg(&self.workspace)
            .arg("-p")
            .arg(prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, val) in env {
            cmd.env(key, val);
        }

        let output = cmd
            .output()
            .await
            .context("Failed to execute agent binary")?;

        self.stdout = String::from_utf8_lossy(&output.stdout).to_string();
        self.stderr = String::from_utf8_lossy(&output.stderr).to_string();
        self.success = output.status.success();
        self.response_text = self.stdout.clone();

        // Parse tool calls from stderr if available (agent logs tool usage to stderr).
        self.parse_tool_log();

        Ok(())
    }

    /// Parse tool calls from the agent's stderr output.
    /// The agent logs tool starts/results in a parseable format.
    fn parse_tool_log(&mut self) {
        for line in self.stderr.lines() {
            // Look for tool call patterns in stderr.
            if let Some(name) = line.strip_prefix("tool_start: ") {
                self.tool_log.push(CapturedToolCall {
                    name: name.trim().to_string(),
                    input: serde_json::Value::Null,
                    is_error: false,
                });
                if !self.tools_used.contains(&name.trim().to_string()) {
                    self.tools_used.push(name.trim().to_string());
                }
            }
        }
    }
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
