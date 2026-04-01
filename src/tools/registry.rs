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
        registry.register(Arc::new(super::notebook_edit::NotebookEditTool));
        registry.register(Arc::new(super::lsp_tool::LspTool));
        registry.register(Arc::new(super::mcp_resources::ListMcpResourcesTool));
        registry.register(Arc::new(super::mcp_resources::ReadMcpResourceTool));
        registry.register(Arc::new(super::plan_mode::EnterPlanModeTool));
        registry.register(Arc::new(super::plan_mode::ExitPlanModeTool));
        registry.register(Arc::new(super::repl_tool::ReplTool));
        registry.register(Arc::new(super::send_message::SendMessageTool));
        registry.register(Arc::new(super::skill_tool::SkillTool));
        registry.register(Arc::new(super::sleep_tool::SleepTool));
        registry.register(Arc::new(super::tasks::TaskCreateTool));
        registry.register(Arc::new(super::tasks::TaskUpdateTool));
        registry.register(Arc::new(super::tasks::TaskGetTool));
        registry.register(Arc::new(super::tasks::TaskListTool));
        registry.register(Arc::new(super::tasks::TaskStopTool));
        registry.register(Arc::new(super::tasks::TaskOutputTool));
        registry.register(Arc::new(super::todo_write::TodoWriteTool));
        registry.register(Arc::new(super::tool_search::ToolSearchTool));
        registry.register(Arc::new(super::worktree::EnterWorktreeTool));
        registry.register(Arc::new(super::worktree::ExitWorktreeTool));
        registry.register(Arc::new(super::web_fetch::WebFetchTool));
        registry.register(Arc::new(super::web_search::WebSearchTool));
        registry.register(Arc::new(super::ask_user::AskUserQuestionTool));
        registry.register(Arc::new(super::powershell::PowerShellTool));
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

    /// Get only always-loaded (core) tool schemas.
    /// Deferred tools are discoverable via ToolSearch but not sent on every request.
    pub fn core_schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .iter()
            .filter(|t| t.is_enabled() && !is_deferred(t.name()))
            .map(|t| ToolSchema::from(t.as_ref()))
            .collect()
    }

    /// Get deferred tool names (for the ToolSearch system prompt).
    pub fn deferred_names(&self) -> Vec<&str> {
        self.tools
            .iter()
            .filter(|t| t.is_enabled() && is_deferred(t.name()))
            .map(|t| t.name())
            .collect()
    }
}

/// Tools that are deferred — not sent on every request to save prompt tokens.
/// These are discoverable via ToolSearch and loaded on demand.
const DEFERRED_TOOLS: &[&str] = &[
    "NotebookEdit",
    "LSP",
    "ListMcpResources",
    "ReadMcpResource",
    "EnterWorktree",
    "ExitWorktree",
    "Sleep",
    "TodoWrite",
    "REPL",
    "SendMessage",
    "TaskStop",
    "TaskOutput",
    "TaskGet",
];

fn is_deferred(name: &str) -> bool {
    DEFERRED_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_tools_count() {
        let reg = ToolRegistry::default_tools();
        assert!(reg.all().len() >= 30);
    }

    #[test]
    fn test_get_by_name() {
        let reg = ToolRegistry::default_tools();
        assert!(reg.get("Bash").is_some());
        assert!(reg.get("FileRead").is_some());
        assert!(reg.get("NonExistent").is_none());
    }

    #[test]
    fn test_schemas_returns_enabled() {
        let reg = ToolRegistry::default_tools();
        let schemas = reg.schemas();
        assert!(!schemas.is_empty());
    }

    #[test]
    fn test_core_schemas_excludes_deferred() {
        let reg = ToolRegistry::default_tools();
        let core = reg.core_schemas();
        let all = reg.schemas();
        assert!(core.len() < all.len());
    }

    #[test]
    fn test_deferred_names() {
        let reg = ToolRegistry::default_tools();
        let deferred = reg.deferred_names();
        assert!(deferred.contains(&"NotebookEdit"));
        assert!(deferred.contains(&"Sleep"));
        assert!(!deferred.contains(&"Bash"));
        assert!(!deferred.contains(&"FileRead"));
    }

    #[test]
    fn test_register_custom_tool() {
        let mut reg = ToolRegistry::new();
        assert_eq!(reg.all().len(), 0);
        reg.register(Arc::new(super::super::bash::BashTool));
        assert_eq!(reg.all().len(), 1);
        assert!(reg.get("Bash").is_some());
    }
}
