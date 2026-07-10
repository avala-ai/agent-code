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

use crate::config::{PermissionMode, PermissionRule, PermissionsConfig};
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
    /// Per-agent permission overlay. When `Some`, this is serialised to
    /// a temp TOML file and passed to the spawned subagent via
    /// `--permissions-overlay`, replacing its effective permissions for
    /// the run. When `None`, the subagent inherits the parent's
    /// permission config (or falls back to `read_only` → `--permission-mode plan`).
    #[serde(default)]
    pub permissions: Option<PermissionsConfig>,
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
                permissions: None,
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
                permissions: None,
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
                permissions: None,
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
        if let Some(config_dir) = crate::config::agent_config_dir() {
            let user_dir = config_dir.join("agents");
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
/// permission_mode: ask
/// allow: ["Bash(git *)", "FileRead"]
/// deny: ["Bash(rm *)"]
/// ask: ["Bash(npm *)"]
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
    let mut perm_mode: Option<PermissionMode> = None;
    let mut allow_list: Vec<String> = Vec::new();
    let mut deny_list: Vec<String> = Vec::new();
    let mut ask_list: Vec<String> = Vec::new();

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
                "include_tools" => include_tools = parse_list_literal(value),
                "exclude_tools" => exclude_tools = parse_list_literal(value),
                "permission_mode" => perm_mode = parse_permission_mode(value),
                "allow" => allow_list = parse_list_literal(value),
                "deny" => deny_list = parse_list_literal(value),
                "ask" => ask_list = parse_list_literal(value),
                _ => {}
            }
        }
    }

    let system_prompt = if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    };

    let permissions = build_permissions_config(perm_mode, &allow_list, &deny_list, &ask_list);

    Some(AgentDefinition {
        name,
        description,
        system_prompt,
        model,
        include_tools,
        exclude_tools,
        read_only,
        max_turns,
        permissions,
    })
}

/// Split a YAML-ish inline list value like `[a, b, "c d"]` into items.
/// Trims brackets, splits on commas, strips surrounding quotes.
fn parse_list_literal(value: &str) -> Vec<String> {
    value
        .trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse a permission mode keyword (`ask`, `allow`, `deny`, `plan`,
/// `accept_edits`). Returns `None` for unrecognised values.
fn parse_permission_mode(value: &str) -> Option<PermissionMode> {
    match value.trim().trim_matches(|c| c == '"' || c == '\'') {
        "allow" => Some(PermissionMode::Allow),
        "deny" => Some(PermissionMode::Deny),
        "ask" => Some(PermissionMode::Ask),
        "plan" => Some(PermissionMode::Plan),
        "accept_edits" => Some(PermissionMode::AcceptEdits),
        _ => None,
    }
}

/// Parse a single permission entry like `Bash(git *)` into a tool name
/// plus optional pattern. Bare tool names like `Grep` yield `(Grep, None)`.
fn parse_permission_entry(entry: &str) -> (String, Option<String>) {
    let trimmed = entry.trim();
    if let Some(open) = trimmed.find('(')
        && let Some(close) = trimmed.rfind(')')
        && close > open
    {
        let tool = trimmed[..open].trim().to_string();
        let pattern = trimmed[open + 1..close].trim().to_string();
        let pattern = if pattern.is_empty() {
            None
        } else {
            Some(pattern)
        };
        return (tool, pattern);
    }
    (trimmed.to_string(), None)
}

/// Build a `PermissionsConfig` from the parts collected out of an agent
/// file's frontmatter. Returns `None` when the agent file specified no
/// permission-related fields — callers can then leave the subagent to
/// inherit the parent's config.
fn build_permissions_config(
    mode: Option<PermissionMode>,
    allow: &[String],
    deny: &[String],
    ask: &[String],
) -> Option<PermissionsConfig> {
    if mode.is_none() && allow.is_empty() && deny.is_empty() && ask.is_empty() {
        return None;
    }
    let mut rules: Vec<PermissionRule> = Vec::new();
    for entry in allow {
        let (tool, pattern) = parse_permission_entry(entry);
        rules.push(PermissionRule {
            tool,
            pattern,
            action: PermissionMode::Allow,
        });
    }
    for entry in deny {
        let (tool, pattern) = parse_permission_entry(entry);
        rules.push(PermissionRule {
            tool,
            pattern,
            action: PermissionMode::Deny,
        });
    }
    for entry in ask {
        let (tool, pattern) = parse_permission_entry(entry);
        rules.push(PermissionRule {
            tool,
            pattern,
            action: PermissionMode::Ask,
        });
    }
    Some(PermissionsConfig {
        default_mode: mode.unwrap_or(PermissionMode::Ask),
        rules,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
    })
}

/// Serialise a `PermissionsConfig` to a TOML snippet suitable for use
/// as the body of a `--permissions-overlay` file.
pub fn permissions_to_toml(perms: &PermissionsConfig) -> Result<String, String> {
    let mut root = toml::value::Table::new();
    let perm_value = toml::Value::try_from(perms).map_err(|e| e.to_string())?;
    root.insert("permissions".to_string(), perm_value);
    toml::to_string(&toml::Value::Table(root)).map_err(|e| e.to_string())
}

/// Build the effective permission overlay for an agent definition.
///
/// Merges explicit `permissions` frontmatter with `include_tools` /
/// `exclude_tools` (mapped onto `allowed_tools` / `disallowed_tools` so
/// the child process hides those tools from the model). Returns `None`
/// when the definition carries no permission-related constraints.
///
/// When `definition.read_only` is true, the overlay's `default_mode` is
/// forced to [`PermissionMode::Plan`]. The child CLI applies
/// `--permission-mode` first and then **replaces** the whole permissions
/// config with the overlay; without this force, a visibility-only overlay
/// would default to `Ask` and one-shot subagents (no prompter) would
/// auto-allow mutating tools such as `Bash`.
pub fn effective_permissions(definition: &AgentDefinition) -> Option<PermissionsConfig> {
    let has_include = !definition.include_tools.is_empty();
    let has_exclude = !definition.exclude_tools.is_empty();
    if definition.permissions.is_none() && !has_include && !has_exclude {
        return None;
    }

    let mut perms = definition.permissions.clone().unwrap_or_default();
    // Visibility-only overlays (no explicit frontmatter permissions) must
    // not leave `Ask` as default: one-shot subagents have no prompter and
    // would auto-allow every mutating tool. Read-only → Plan; otherwise
    // Deny + explicit Allow rules for listed tools.
    if definition.permissions.is_none() {
        if definition.read_only {
            perms.default_mode = PermissionMode::Plan;
        } else {
            perms.default_mode = PermissionMode::Deny;
        }
    }
    if definition.read_only {
        perms.default_mode = PermissionMode::Plan;
    }
    if has_include {
        // Include list is authoritative for visibility: only listed tools
        // reach the child's schema. Append rather than clobber if frontmatter
        // already set an allowlist, so both sources compose.
        if perms.allowed_tools.is_empty() {
            perms.allowed_tools = definition.include_tools.clone();
        } else {
            for tool in &definition.include_tools {
                if !perms.allowed_tools.iter().any(|t| t == tool) {
                    perms.allowed_tools.push(tool.clone());
                }
            }
        }
    }
    if has_exclude {
        for tool in &definition.exclude_tools {
            if !perms.disallowed_tools.iter().any(|t| t == tool) {
                perms.disallowed_tools.push(tool.clone());
            }
        }
    }
    // When default is Deny and we only have an allowlist (visibility),
    // grant Allow rules for each listed tool so the child can run them
    // without a prompter.
    if matches!(perms.default_mode, PermissionMode::Deny) && !perms.allowed_tools.is_empty() {
        for tool in &perms.allowed_tools {
            let already = perms.rules.iter().any(|r| r.tool == *tool);
            if !already {
                perms.rules.push(PermissionRule {
                    tool: tool.clone(),
                    pattern: None,
                    action: PermissionMode::Allow,
                });
            }
        }
    }
    Some(perms)
}

/// Compose a child permissions overlay onto a host permissions config.
///
/// Host **restrictions** always win:
/// - Rules are ordered host-first so first-match evaluation never lets an
///   overlay `Allow` shadow a project `Deny` (or other host rule).
/// - `allowed_tools` is the intersection when both sides set a list; an
///   overlay cannot re-expose a tool the host omitted from its allowlist.
/// - `disallowed_tools` is the union (either side can forbid).
///
/// Overlay still wins for `default_mode` (typed subagents set Plan/Deny).
pub fn compose_permissions_overlay(
    host: &PermissionsConfig,
    overlay: &PermissionsConfig,
) -> PermissionsConfig {
    // Host rules first: PermissionChecker first-match must not let an
    // overlay Allow(Bash) shadow host Deny(Bash(rm *)).
    let mut rules = host.rules.clone();
    for r in &overlay.rules {
        if !rules
            .iter()
            .any(|x| x.tool == r.tool && x.pattern == r.pattern && x.action == r.action)
        {
            rules.push(r.clone());
        }
    }
    // Visibility: intersect when both sides restrict; never replace a host
    // allowlist with a looser overlay list.
    let allowed_tools = match (
        host.allowed_tools.is_empty(),
        overlay.allowed_tools.is_empty(),
    ) {
        (false, false) => overlay
            .allowed_tools
            .iter()
            .filter(|t| host.allowed_tools.iter().any(|h| h == *t))
            .cloned()
            .collect(),
        (false, true) => host.allowed_tools.clone(),
        (true, false) => overlay.allowed_tools.clone(),
        (true, true) => Vec::new(),
    };
    let mut disallowed_tools = host.disallowed_tools.clone();
    for t in &overlay.disallowed_tools {
        if !disallowed_tools.iter().any(|x| x == t) {
            disallowed_tools.push(t.clone());
        }
    }
    PermissionsConfig {
        default_mode: overlay.default_mode,
        rules,
        allowed_tools,
        disallowed_tools,
    }
}

/// Write a permissions overlay TOML to a temp file and return its path.
///
/// The file is intentionally leaked: it is tiny, the OS reclaims `/tmp`
/// on reboot, and early deletion races the child process's read.
pub fn write_permissions_overlay(perms: &PermissionsConfig) -> Result<PathBuf, String> {
    let toml_body = permissions_to_toml(perms)?;
    let filename = format!(
        "agent-code-perms-{}.toml",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("overlay")
    );
    let path = std::env::temp_dir().join(filename);
    std::fs::write(&path, toml_body).map_err(|e| format!("write permissions overlay: {e}"))?;
    Ok(path)
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

/// Apply agent-definition flags (model, turns, read-only, permissions)
/// onto an already-built `agent --prompt` command.
///
/// Used by both the coordinator and the Agent tool so typed subagents
/// share one code path for overlays and include/exclude tool lists.
pub fn apply_agent_definition(
    cmd: &mut tokio::process::Command,
    definition: &AgentDefinition,
    model_override: Option<&str>,
) {
    if let Some(model) = model_override.or(definition.model.as_deref()) {
        cmd.arg("--model").arg(resolve_model_alias(model));
    }
    if let Some(max_turns) = definition.max_turns {
        cmd.arg("--max-turns").arg(max_turns.to_string());
    }
    if definition.read_only {
        cmd.arg("--permission-mode").arg("plan");
    }

    // Per-agent permissions + include/exclude tool visibility.
    if let Some(perms) = effective_permissions(definition) {
        match write_permissions_overlay(&perms) {
            Ok(path) => {
                cmd.arg("--permissions-overlay").arg(&path);
            }
            Err(e) => {
                warn!(
                    "Failed to write permissions overlay ({e}); subagent will inherit parent permissions"
                );
            }
        }
    }
}

/// Expand short model aliases used by agents (`sonnet` / `opus` / `haiku`).
pub fn resolve_model_alias(model: &str) -> String {
    match model.trim().to_ascii_lowercase().as_str() {
        "sonnet" | "claude-sonnet" => "claude-sonnet-5".into(),
        "opus" | "claude-opus" => "claude-opus-4-8".into(),
        "haiku" | "claude-haiku" => "claude-haiku-4-5".into(),
        "gpt" | "gpt-latest" => "gpt-5.4".into(),
        "grok" => "grok-4.3".into(),
        other => other.to_string(),
    }
}

/// Prefix the user prompt with the agent type's system-prompt addition.
pub fn compose_agent_prompt(definition: &AgentDefinition, prompt: &str) -> String {
    if let Some(ref sys) = definition.system_prompt {
        format!("{sys}\n\n{prompt}")
    } else {
        prompt.to_string()
    }
}

/// Build a subprocess command for running an agent.
///
/// Shared by `run_agent()` and `run_team()` to avoid duplication.
fn build_agent_command(
    definition: &AgentDefinition,
    prompt: &str,
    cwd: &std::path::Path,
) -> tokio::process::Command {
    let full_prompt = compose_agent_prompt(definition, prompt);

    let agent_binary = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "agent".to_string());

    let mut cmd = tokio::process::Command::new(agent_binary);
    cmd.arg("--prompt")
        .arg(full_prompt)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    apply_agent_definition(&mut cmd, definition, None);

    // Pass through API keys so subagents use the same provider.
    for var in &[
        "AGENT_CODE_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "XAI_API_KEY",
        "GOOGLE_API_KEY",
        "DEEPSEEK_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "TOGETHER_API_KEY",
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

    // ---- Per-agent permissions ----

    #[test]
    fn parse_permission_entry_bare_tool() {
        let (tool, pat) = parse_permission_entry("Grep");
        assert_eq!(tool, "Grep");
        assert!(pat.is_none());
    }

    #[test]
    fn parse_permission_entry_with_pattern() {
        let (tool, pat) = parse_permission_entry("Bash(git *)");
        assert_eq!(tool, "Bash");
        assert_eq!(pat.as_deref(), Some("git *"));
    }

    #[test]
    fn parse_permission_entry_strips_surrounding_whitespace() {
        let (tool, pat) = parse_permission_entry("  FileRead(src/**)  ");
        assert_eq!(tool, "FileRead");
        assert_eq!(pat.as_deref(), Some("src/**"));
    }

    #[test]
    fn build_permissions_returns_none_when_empty() {
        let p = build_permissions_config(None, &[], &[], &[]);
        assert!(p.is_none());
    }

    #[test]
    fn build_permissions_default_mode_falls_back_to_ask() {
        let allow = vec!["Grep".to_string()];
        let p = build_permissions_config(None, &allow, &[], &[]).unwrap();
        assert_eq!(p.default_mode, PermissionMode::Ask);
        assert_eq!(p.rules.len(), 1);
        assert_eq!(p.rules[0].tool, "Grep");
        assert_eq!(p.rules[0].action, PermissionMode::Allow);
    }

    #[test]
    fn build_permissions_orders_allow_deny_ask() {
        let allow = vec!["Bash(git *)".to_string()];
        let deny = vec!["Bash(rm *)".to_string()];
        let ask = vec!["FileRead".to_string()];
        let p = build_permissions_config(Some(PermissionMode::Deny), &allow, &deny, &ask).unwrap();
        assert_eq!(p.default_mode, PermissionMode::Deny);
        assert_eq!(p.rules.len(), 3);
        assert_eq!(p.rules[0].action, PermissionMode::Allow);
        assert_eq!(p.rules[0].pattern.as_deref(), Some("git *"));
        assert_eq!(p.rules[1].action, PermissionMode::Deny);
        assert_eq!(p.rules[1].pattern.as_deref(), Some("rm *"));
        assert_eq!(p.rules[2].action, PermissionMode::Ask);
        assert!(p.rules[2].pattern.is_none());
    }

    #[test]
    fn parse_agent_file_reads_permission_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("my-agent.md");
        let content = "---\n\
             name: my-agent\n\
             description: test\n\
             permission_mode: deny\n\
             allow: [\"Bash(git *)\", \"FileRead\"]\n\
             deny: [\"Bash(rm *)\"]\n\
             ---\n\
             \n\
             System prompt body.\n";
        std::fs::write(&path, content).unwrap();

        let def = parse_agent_file(&path).unwrap();
        assert_eq!(def.name, "my-agent");
        let perms = def.permissions.expect("permissions should be parsed");
        assert_eq!(perms.default_mode, PermissionMode::Deny);
        assert_eq!(perms.rules.len(), 3);
        assert_eq!(perms.rules[0].tool, "Bash");
        assert_eq!(perms.rules[0].pattern.as_deref(), Some("git *"));
        assert_eq!(perms.rules[0].action, PermissionMode::Allow);
        assert_eq!(perms.rules[1].tool, "FileRead");
        assert!(perms.rules[1].pattern.is_none());
        assert_eq!(perms.rules[2].tool, "Bash");
        assert_eq!(perms.rules[2].pattern.as_deref(), Some("rm *"));
        assert_eq!(perms.rules[2].action, PermissionMode::Deny);
    }

    #[test]
    fn parse_agent_file_without_permissions_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("basic.md");
        let content = "---\n\
             name: basic\n\
             description: no perms\n\
             ---\n\
             \n\
             body\n";
        std::fs::write(&path, content).unwrap();

        let def = parse_agent_file(&path).unwrap();
        assert!(def.permissions.is_none());
    }

    #[test]
    fn permissions_to_toml_round_trip() {
        let perms = PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![
                PermissionRule {
                    tool: "Bash".into(),
                    pattern: Some("git *".into()),
                    action: PermissionMode::Allow,
                },
                PermissionRule {
                    tool: "FileRead".into(),
                    pattern: None,
                    action: PermissionMode::Allow,
                },
            ],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        };
        let s = permissions_to_toml(&perms).unwrap();
        assert!(s.contains("[permissions]"));
        let value: toml::Value = toml::from_str(&s).unwrap();
        let parsed: PermissionsConfig = value
            .get("permissions")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(parsed.default_mode, PermissionMode::Deny);
        assert_eq!(parsed.rules.len(), 2);
    }

    #[test]
    fn effective_permissions_none_when_unconstrained() {
        let def = AgentRegistry::with_defaults()
            .get("general-purpose")
            .unwrap()
            .clone();
        assert!(effective_permissions(&def).is_none());
    }

    #[test]
    fn effective_permissions_maps_include_tools() {
        let def = AgentRegistry::with_defaults()
            .get("explore")
            .unwrap()
            .clone();
        let perms = effective_permissions(&def).expect("explore has include_tools");
        assert!(perms.allowed_tools.contains(&"FileRead".into()));
        assert!(perms.allowed_tools.contains(&"Grep".into()));
        assert!(perms.allowed_tools.contains(&"Glob".into()));
        assert!(perms.allowed_tools.contains(&"Bash".into()));
        assert!(perms.allowed_tools.contains(&"WebFetch".into()));
        // read_only explore must keep Plan mode so the overlay does not
        // clobber --permission-mode plan with default Ask.
        assert_eq!(perms.default_mode, PermissionMode::Plan);
    }

    #[test]
    fn effective_permissions_read_only_forces_plan_mode() {
        let def = AgentRegistry::with_defaults().get("plan").unwrap().clone();
        let perms = effective_permissions(&def).expect("plan has include_tools");
        assert_eq!(perms.default_mode, PermissionMode::Plan);

        // Even if frontmatter/default asked for Ask/Allow, read_only wins.
        let mut custom = def;
        custom.permissions = Some(PermissionsConfig {
            default_mode: PermissionMode::Allow,
            rules: vec![],
            allowed_tools: vec!["FileRead".into()],
            disallowed_tools: vec![],
        });
        let perms = effective_permissions(&custom).unwrap();
        assert_eq!(perms.default_mode, PermissionMode::Plan);
    }

    #[test]
    fn effective_permissions_merges_exclude_and_frontmatter() {
        let def = AgentDefinition {
            name: "custom".into(),
            description: "t".into(),
            system_prompt: None,
            model: None,
            include_tools: vec!["FileRead".into()],
            exclude_tools: vec!["Bash".into()],
            read_only: true,
            max_turns: None,
            permissions: Some(PermissionsConfig {
                default_mode: PermissionMode::Ask,
                rules: vec![],
                allowed_tools: vec!["Grep".into()],
                disallowed_tools: vec![],
            }),
        };
        let perms = effective_permissions(&def).unwrap();
        // Existing allowlist is preserved and include_tools appended.
        assert!(perms.allowed_tools.contains(&"Grep".into()));
        assert!(perms.allowed_tools.contains(&"FileRead".into()));
        assert!(perms.disallowed_tools.contains(&"Bash".into()));
        assert_eq!(perms.default_mode, PermissionMode::Plan);
    }

    #[test]
    fn resolve_model_alias_expands_short_names() {
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-5");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-8");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5");
        assert_eq!(resolve_model_alias("claude-sonnet-5"), "claude-sonnet-5");
    }

    #[test]
    fn compose_permissions_overlay_keeps_host_rules() {
        let host = PermissionsConfig {
            default_mode: PermissionMode::Ask,
            rules: vec![PermissionRule {
                tool: "Bash".into(),
                pattern: Some("rm *".into()),
                action: PermissionMode::Deny,
            }],
            allowed_tools: vec![],
            disallowed_tools: vec![],
        };
        let overlay = PermissionsConfig {
            default_mode: PermissionMode::Plan,
            rules: vec![],
            allowed_tools: vec!["FileRead".into(), "Grep".into()],
            disallowed_tools: vec![],
        };
        let merged = compose_permissions_overlay(&host, &overlay);
        assert_eq!(merged.default_mode, PermissionMode::Plan);
        assert!(merged.allowed_tools.contains(&"FileRead".into()));
        assert!(
            merged
                .rules
                .iter()
                .any(|r| r.tool == "Bash" && r.action == PermissionMode::Deny),
            "host project rules must survive overlay"
        );
    }

    #[test]
    fn compose_permissions_overlay_host_deny_beats_overlay_allow() {
        // First-match: host Deny must appear before overlay Allow so a
        // project Bash(rm *) rule is never shadowed by include_tools Allow.
        let host = PermissionsConfig {
            default_mode: PermissionMode::Ask,
            rules: vec![PermissionRule {
                tool: "Bash".into(),
                pattern: Some("rm *".into()),
                action: PermissionMode::Deny,
            }],
            allowed_tools: vec![],
            disallowed_tools: vec![],
        };
        let overlay = PermissionsConfig {
            default_mode: PermissionMode::Deny,
            rules: vec![PermissionRule {
                tool: "Bash".into(),
                pattern: None,
                action: PermissionMode::Allow,
            }],
            allowed_tools: vec!["Bash".into()],
            disallowed_tools: vec![],
        };
        let merged = compose_permissions_overlay(&host, &overlay);
        let deny_pos = merged
            .rules
            .iter()
            .position(|r| {
                r.tool == "Bash"
                    && r.pattern.as_deref() == Some("rm *")
                    && r.action == PermissionMode::Deny
            })
            .expect("host deny present");
        let allow_pos = merged
            .rules
            .iter()
            .position(|r| {
                r.tool == "Bash" && r.pattern.is_none() && r.action == PermissionMode::Allow
            })
            .expect("overlay allow present");
        assert!(
            deny_pos < allow_pos,
            "host Deny must precede overlay Allow (first-match)"
        );

        // End-to-end with PermissionChecker: rm is denied, other bash allowed.
        let checker = crate::permissions::PermissionChecker::from_config(&merged);
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "rm -rf /tmp/x"})),
            crate::permissions::PermissionDecision::Deny(_)
        ));
        assert!(matches!(
            checker.check("Bash", &serde_json::json!({"command": "ls"})),
            crate::permissions::PermissionDecision::Allow
        ));
    }

    #[test]
    fn compose_permissions_overlay_intersects_allowlists() {
        let host = PermissionsConfig {
            default_mode: PermissionMode::Ask,
            rules: vec![],
            allowed_tools: vec!["Bash".into(), "FileRead".into()],
            disallowed_tools: vec![],
        };
        let overlay = PermissionsConfig {
            default_mode: PermissionMode::Plan,
            rules: vec![],
            // Tries to drop FileRead and re-add Grep (not on host list).
            allowed_tools: vec!["FileRead".into(), "Grep".into(), "Bash".into()],
            disallowed_tools: vec![],
        };
        let merged = compose_permissions_overlay(&host, &overlay);
        assert!(merged.allowed_tools.contains(&"Bash".into()));
        assert!(merged.allowed_tools.contains(&"FileRead".into()));
        assert!(
            !merged.allowed_tools.contains(&"Grep".into()),
            "overlay must not re-expose tools outside the host allowlist"
        );
        assert_eq!(merged.allowed_tools.len(), 2);
    }

    #[test]
    fn compose_agent_prompt_prefixes_system() {
        let def = AgentDefinition {
            name: "x".into(),
            description: "d".into(),
            system_prompt: Some("You are specialized.".into()),
            model: None,
            include_tools: vec![],
            exclude_tools: vec![],
            read_only: false,
            max_turns: None,
            permissions: None,
        };
        let full = compose_agent_prompt(&def, "do work");
        assert!(full.starts_with("You are specialized."));
        assert!(full.contains("do work"));
    }
}
