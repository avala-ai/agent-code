//! Multi-agent coordinator.
//!
//! Routes tasks to specialized agents based on the task type.
//! The coordinator acts as an orchestrator, spawning agents with
//! appropriate configurations and aggregating their results.
//!
//! # Agent types
//!
//! - `general-purpose`: default agent with full tool access
//! - `explore`: fast read-only agent for codebase exploration
//! - `plan`: planning agent restricted to analysis tools
//!
//! Agents are defined as configurations that customize the tool
//! set, system prompt, and permission mode.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Definition of a specialized agent type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Unique agent type name.
    pub name: String,
    /// Description of what this agent specializes in.
    pub description: String,
    /// System prompt additions for this agent type.
    pub system_prompt: Option<String>,
    /// Model override (if different from default).
    pub model: Option<String>,
    /// Tools to include (if empty, use all).
    pub include_tools: Vec<String>,
    /// Tools to exclude.
    pub exclude_tools: Vec<String>,
    /// Whether this agent runs in read-only mode.
    pub read_only: bool,
    /// Maximum turns for this agent type.
    pub max_turns: Option<usize>,
}

/// Registry of available agent types.
pub struct AgentRegistry {
    agents: HashMap<String, AgentDefinition>,
}

impl AgentRegistry {
    /// Create the registry with built-in agent types.
    pub fn with_defaults() -> Self {
        let mut agents = HashMap::new();

        agents.insert(
            "general-purpose".to_string(),
            AgentDefinition {
                name: "general-purpose".to_string(),
                description: "General-purpose agent with full tool access.".to_string(),
                system_prompt: None,
                model: None,
                include_tools: Vec::new(),
                exclude_tools: Vec::new(),
                read_only: false,
                max_turns: None,
            },
        );

        agents.insert(
            "explore".to_string(),
            AgentDefinition {
                name: "explore".to_string(),
                description: "Fast read-only agent for searching and understanding code."
                    .to_string(),
                system_prompt: Some(
                    "You are a fast exploration agent. Focus on finding information \
                     quickly. Use Grep, Glob, and FileRead to answer questions about \
                     the codebase. Do not modify files."
                        .to_string(),
                ),
                model: None,
                include_tools: vec![
                    "FileRead".into(),
                    "Grep".into(),
                    "Glob".into(),
                    "Bash".into(),
                    "WebFetch".into(),
                ],
                exclude_tools: Vec::new(),
                read_only: true,
                max_turns: Some(20),
            },
        );

        agents.insert(
            "plan".to_string(),
            AgentDefinition {
                name: "plan".to_string(),
                description: "Planning agent that designs implementation strategies.".to_string(),
                system_prompt: Some(
                    "You are a software architect agent. Design implementation plans, \
                     identify critical files, and consider architectural trade-offs. \
                     Do not modify files directly."
                        .to_string(),
                ),
                model: None,
                include_tools: vec![
                    "FileRead".into(),
                    "Grep".into(),
                    "Glob".into(),
                    "Bash".into(),
                ],
                exclude_tools: Vec::new(),
                read_only: true,
                max_turns: Some(30),
            },
        );

        Self { agents }
    }

    /// Look up an agent definition by type name.
    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.get(name)
    }

    /// Register a custom agent type.
    pub fn register(&mut self, definition: AgentDefinition) {
        self.agents.insert(definition.name.clone(), definition);
    }

    /// List all available agent types.
    pub fn list(&self) -> Vec<&AgentDefinition> {
        let mut agents: Vec<_> = self.agents.values().collect();
        agents.sort_by_key(|a| &a.name);
        agents
    }

    /// Load agent definitions from disk (`.agent/agents/` and `~/.config/agent-code/agents/`).
    /// Each `.md` file is parsed for YAML frontmatter with agent configuration.
    pub fn load_from_disk(&mut self, cwd: Option<&std::path::Path>) {
        // Project-level agents.
        if let Some(cwd) = cwd {
            let project_dir = cwd.join(".agent").join("agents");
            self.load_agents_from_dir(&project_dir);
        }

        // User-level agents.
        if let Some(config_dir) = dirs::config_dir() {
            let user_dir = config_dir.join("agent-code").join("agents");
            self.load_agents_from_dir(&user_dir);
        }
    }

    fn load_agents_from_dir(&mut self, dir: &std::path::Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md")
                && let Some(def) = parse_agent_file(&path)
            {
                self.agents.insert(def.name.clone(), def);
            }
        }
    }
}

/// Parse an agent definition from a markdown file with YAML frontmatter.
///
/// Expected format:
/// ```markdown
/// ---
/// name: my-agent
/// description: A specialized agent
/// model: gpt-4.1-mini
/// read_only: false
/// max_turns: 20
/// include_tools: [FileRead, Grep, Glob]
/// exclude_tools: [Bash]
/// ---
///
/// System prompt additions go here...
/// ```
fn parse_agent_file(path: &std::path::Path) -> Option<AgentDefinition> {
    let content = std::fs::read_to_string(path).ok()?;

    // Parse YAML frontmatter.
    if !content.starts_with("---") {
        return None;
    }
    let end = content[3..].find("---")?;
    let frontmatter = &content[3..3 + end];
    let body = content[3 + end + 3..].trim();

    let mut name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("custom")
        .to_string();
    let mut description = String::new();
    let mut model = None;
    let mut read_only = false;
    let mut max_turns = None;
    let mut include_tools = Vec::new();
    let mut exclude_tools = Vec::new();

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" => name = value.to_string(),
                "description" => description = value.to_string(),
                "model" => model = Some(value.to_string()),
                "read_only" => read_only = value == "true",
                "max_turns" => max_turns = value.parse().ok(),
                "include_tools" => {
                    include_tools = value
                        .trim_matches(|c| c == '[' || c == ']')
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "exclude_tools" => {
                    exclude_tools = value
                        .trim_matches(|c| c == '[' || c == ']')
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
    }

    let system_prompt = if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    };

    Some(AgentDefinition {
        name,
        description,
        system_prompt,
        model,
        include_tools,
        exclude_tools,
        read_only,
        max_turns,
    })
}

// ---- Coordinator Runtime ----

/// A running agent instance.
#[derive(Debug, Clone)]
pub struct AgentInstance {
    /// Unique instance ID.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Agent type definition.
    pub definition: AgentDefinition,
    /// Current status.
    pub status: AgentStatus,
    /// Messages received from other agents.
    pub inbox: Vec<AgentMessage>,
}

/// Status of a running agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    /// Agent is waiting to be started.
    Pending,
    /// Agent is currently executing.
    Running,
    /// Agent completed successfully.
    Completed,
    /// Agent failed with an error.
    Failed(String),
}

/// A message sent between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// ID of the sending agent.
    pub from: String,
    /// Message content.
    pub content: String,
    /// Timestamp.
    pub timestamp: String,
}

/// Result from a completed agent.
#[derive(Debug, Clone)]
pub struct AgentResult {
    /// Agent instance ID.
    pub agent_id: String,
    /// Agent name.
    pub agent_name: String,
    /// Output text from the agent.
    pub output: String,
    /// Whether the agent succeeded.
    pub success: bool,
}

/// Team definition for multi-agent orchestration.
#[derive(Debug, Clone)]
pub struct Team {
    /// Team ID.
    pub id: String,
    /// Team name.
    pub name: String,
    /// Agent instances in this team.
    pub agents: Vec<String>,
    /// Working directory for the team.
    pub cwd: PathBuf,
}

/// Orchestrates multiple agent instances, routing messages and
/// collecting results.
pub struct Coordinator {
    /// Agent registry for looking up definitions.
    registry: AgentRegistry,
    /// Running agent instances, keyed by ID.
    instances: Arc<Mutex<HashMap<String, AgentInstance>>>,
    /// Active teams.
    teams: Arc<Mutex<HashMap<String, Team>>>,
    /// Working directory.
    cwd: PathBuf,
}

/// Build a subprocess command for running an agent.
///
/// Shared by `run_agent()` and `run_team()` to avoid duplication.
fn build_agent_command(
    definition: &AgentDefinition,
    prompt: &str,
    cwd: &std::path::Path,
) -> tokio::process::Command {
    let full_prompt = if let Some(ref sys) = definition.system_prompt {
        format!("{sys}\n\n{prompt}")
    } else {
        prompt.to_string()
    };

    let agent_binary = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "agent".to_string());

    let mut cmd = tokio::process::Command::new(agent_binary);
    cmd.arg("--prompt")
        .arg(full_prompt)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(ref model) = definition.model {
        cmd.arg("--model").arg(model);
    }
    if let Some(max_turns) = definition.max_turns {
        cmd.arg("--max-turns").arg(max_turns.to_string());
    }
    if definition.read_only {
        cmd.arg("--permission-mode").arg("plan");
    }

    // Pass through API keys so subagents use the same provider.
    for var in &[
        "AGENT_CODE_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "AGENT_CODE_API_BASE_URL",
        "AGENT_CODE_MODEL",
    ] {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }

    cmd
}

impl Coordinator {
    /// Create a new coordinator.
    pub fn new(cwd: PathBuf) -> Self {
        let mut registry = AgentRegistry::with_defaults();
        registry.load_from_disk(Some(&cwd));

        Self {
            registry,
            instances: Arc::new(Mutex::new(HashMap::new())),
            teams: Arc::new(Mutex::new(HashMap::new())),
            cwd,
        }
    }

    /// Spawn an agent instance.
    ///
    /// Returns the instance ID. The agent is created in `Pending` status
    /// and must be started with `run_agent()`.
    pub async fn spawn_agent(
        &self,
        agent_type: &str,
        name: Option<String>,
    ) -> Result<String, String> {
        let definition = self
            .registry
            .get(agent_type)
            .ok_or_else(|| format!("Unknown agent type: {agent_type}"))?
            .clone();

        let id = uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("agent")
            .to_string();

        let display_name = name.unwrap_or_else(|| format!("{}-{}", definition.name, &id[..4]));

        let instance = AgentInstance {
            id: id.clone(),
            name: display_name.clone(),
            definition,
            status: AgentStatus::Pending,
            inbox: Vec::new(),
        };

        self.instances.lock().await.insert(id.clone(), instance);
        info!("Spawned agent '{display_name}' ({id}) type={agent_type}");

        Ok(id)
    }

    /// Run an agent with the given prompt.
    ///
    /// Executes the agent as a subprocess and returns the result.
    /// The agent's status is updated throughout the lifecycle.
    pub async fn run_agent(&self, agent_id: &str, prompt: &str) -> Result<AgentResult, String> {
        // Single lock acquisition: update status, clone definition and name.
        let (definition, agent_name) = {
            let mut instances = self.instances.lock().await;
            let instance = instances
                .get_mut(agent_id)
                .ok_or_else(|| format!("Agent not found: {agent_id}"))?;
            instance.status = AgentStatus::Running;
            (instance.definition.clone(), instance.name.clone())
        };

        debug!("Running agent '{agent_name}' ({agent_id})");

        let mut cmd = build_agent_command(&definition, prompt, &self.cwd);
        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Spawn failed: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        // Update status.
        {
            let mut instances = self.instances.lock().await;
            if let Some(instance) = instances.get_mut(agent_id) {
                instance.status = if success {
                    AgentStatus::Completed
                } else {
                    AgentStatus::Failed(stderr.clone())
                };
            }
        }

        let result_text = if success {
            stdout
        } else {
            format!("{stdout}\n\nErrors:\n{stderr}")
        };

        Ok(AgentResult {
            agent_id: agent_id.to_string(),
            agent_name,
            output: result_text,
            success,
        })
    }

    /// Run multiple agents in parallel and collect all results.
    pub async fn run_team(
        &self,
        tasks: Vec<(&str, &str, &str)>, // (agent_type, name, prompt)
    ) -> Vec<AgentResult> {
        let mut handles = Vec::new();

        for (agent_type, name, prompt) in tasks {
            let agent_id = match self.spawn_agent(agent_type, Some(name.to_string())).await {
                Ok(id) => id,
                Err(e) => {
                    warn!("Failed to spawn agent '{name}': {e}");
                    continue;
                }
            };

            let coordinator_instances = Arc::clone(&self.instances);
            let cwd = self.cwd.clone();
            let prompt = prompt.to_string();
            let agent_id_clone = agent_id.clone();

            // Each agent runs in its own tokio task.
            let handle = tokio::spawn(async move {
                // We need to re-create a minimal coordinator for the subprocess call.
                // This is because the coordinator borrows self which can't move into spawn.
                let definition = {
                    let instances = coordinator_instances.lock().await;
                    instances.get(&agent_id_clone).map(|i| i.definition.clone())
                };

                let Some(definition) = definition else {
                    return AgentResult {
                        agent_id: agent_id_clone,
                        agent_name: "unknown".into(),
                        output: "Agent not found".into(),
                        success: false,
                    };
                };

                let agent_name = {
                    let instances = coordinator_instances.lock().await;
                    instances
                        .get(&agent_id_clone)
                        .map(|i| i.name.clone())
                        .unwrap_or_default()
                };

                // Update status.
                {
                    let mut instances = coordinator_instances.lock().await;
                    if let Some(inst) = instances.get_mut(&agent_id_clone) {
                        inst.status = AgentStatus::Running;
                    }
                }

                let mut cmd = build_agent_command(&definition, &prompt, &cwd);

                match cmd.output().await {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        let success = output.status.success();

                        {
                            let mut instances = coordinator_instances.lock().await;
                            if let Some(inst) = instances.get_mut(&agent_id_clone) {
                                inst.status = if success {
                                    AgentStatus::Completed
                                } else {
                                    AgentStatus::Failed(stderr.clone())
                                };
                            }
                        }

                        AgentResult {
                            agent_id: agent_id_clone,
                            agent_name,
                            output: if success {
                                stdout
                            } else {
                                format!("{stdout}\nErrors:\n{stderr}")
                            },
                            success,
                        }
                    }
                    Err(e) => {
                        {
                            let mut instances = coordinator_instances.lock().await;
                            if let Some(inst) = instances.get_mut(&agent_id_clone) {
                                inst.status = AgentStatus::Failed(e.to_string());
                            }
                        }
                        AgentResult {
                            agent_id: agent_id_clone,
                            agent_name,
                            output: format!("Spawn failed: {e}"),
                            success: false,
                        }
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all agents to complete.
        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => warn!("Agent task panicked: {e}"),
            }
        }

        info!(
            "Team completed: {}/{} succeeded",
            results.iter().filter(|r| r.success).count(),
            results.len()
        );
        results
    }

    /// Send a message to a running agent.
    pub async fn send_message(&self, to: &str, from: &str, content: &str) -> Result<(), String> {
        let mut instances = self.instances.lock().await;

        // Find by ID or name.
        let instance = instances
            .values_mut()
            .find(|i| i.id == to || i.name == to)
            .ok_or_else(|| format!("Agent not found: {to}"))?;

        instance.inbox.push(AgentMessage {
            from: from.to_string(),
            content: content.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

        debug!("Message from '{from}' to '{to}': {content}");
        Ok(())
    }

    /// List all agent instances.
    pub async fn list_agents(&self) -> Vec<AgentInstance> {
        self.instances.lock().await.values().cloned().collect()
    }

    /// Get agent registry.
    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    /// Create a new team.
    pub async fn create_team(&self, name: &str, agent_types: &[&str]) -> Result<String, String> {
        let team_id = uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("team")
            .to_string();

        let mut agent_ids = Vec::new();
        for agent_type in agent_types {
            let id = self.spawn_agent(agent_type, None).await?;
            agent_ids.push(id);
        }

        let team = Team {
            id: team_id.clone(),
            name: name.to_string(),
            agents: agent_ids,
            cwd: self.cwd.clone(),
        };

        self.teams.lock().await.insert(team_id.clone(), team);
        info!(
            "Created team '{name}' ({team_id}) with {} agents",
            agent_types.len()
        );

        Ok(team_id)
    }

    /// List active teams.
    pub async fn list_teams(&self) -> Vec<Team> {
        self.teams.lock().await.values().cloned().collect()
    }
}

#[cfg(test)]
mod coordinator_tests {
    use super::*;

    #[test]
    fn test_agent_status_eq() {
        assert_eq!(AgentStatus::Pending, AgentStatus::Pending);
        assert_eq!(AgentStatus::Running, AgentStatus::Running);
        assert_eq!(AgentStatus::Completed, AgentStatus::Completed);
        assert_ne!(AgentStatus::Pending, AgentStatus::Running);
    }

    #[tokio::test]
    async fn test_spawn_agent() {
        let coord = Coordinator::new(std::env::temp_dir());
        let id = coord
            .spawn_agent("general-purpose", Some("test-agent".into()))
            .await;
        assert!(id.is_ok());

        let agents = coord.list_agents().await;
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "test-agent");
        assert_eq!(agents[0].status, AgentStatus::Pending);
    }

    #[tokio::test]
    async fn test_spawn_unknown_type() {
        let coord = Coordinator::new(std::env::temp_dir());
        let result = coord.spawn_agent("nonexistent", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_message() {
        let coord = Coordinator::new(std::env::temp_dir());
        let id = coord
            .spawn_agent("general-purpose", Some("receiver".into()))
            .await
            .unwrap();

        let result = coord.send_message(&id, "sender", "hello").await;
        assert!(result.is_ok());

        let agents = coord.list_agents().await;
        assert_eq!(agents[0].inbox.len(), 1);
        assert_eq!(agents[0].inbox[0].content, "hello");
    }

    #[tokio::test]
    async fn test_send_message_by_name() {
        let coord = Coordinator::new(std::env::temp_dir());
        coord
            .spawn_agent("explore", Some("explorer".into()))
            .await
            .unwrap();

        let result = coord.send_message("explorer", "lead", "search for X").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_team() {
        let coord = Coordinator::new(std::env::temp_dir());
        let team_id = coord
            .create_team("my-team", &["general-purpose", "explore"])
            .await;
        assert!(team_id.is_ok());

        let teams = coord.list_teams().await;
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].agents.len(), 2);

        let agents = coord.list_agents().await;
        assert_eq!(agents.len(), 2);
    }
}
