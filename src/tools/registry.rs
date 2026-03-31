//! Tool registry: collects tools and dispatches by name.

use std::sync::Arc;

use super::{Tool, ToolSchema};

/// Registry of available tools, keyed by name.
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Create a registry with all default built-in tools.
    pub fn default_tools() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(super::agent::AgentTool));
        registry.register(Arc::new(super::bash::BashTool));
        registry.register(Arc::new(super::file_read::FileReadTool));
        registry.register(Arc::new(super::file_write::FileWriteTool));
        registry.register(Arc::new(super::file_edit::FileEditTool));
        registry.register(Arc::new(super::grep::GrepTool));
        registry.register(Arc::new(super::glob::GlobTool));
        registry.register(Arc::new(super::web_fetch::WebFetchTool));
        registry.register(Arc::new(super::ask_user::AskUserQuestionTool));
        registry
    }

    /// Register a new tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Get all registered tools.
    pub fn all(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    /// Get tool schemas for the API request.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .iter()
            .filter(|t| t.is_enabled())
            .map(|t| ToolSchema::from(t.as_ref()))
            .collect()
    }
}
