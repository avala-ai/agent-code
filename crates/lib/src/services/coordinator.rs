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
}
