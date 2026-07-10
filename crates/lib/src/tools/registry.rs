//! Tool registry: collects tools and dispatches by name.

use std::sync::Arc;

use super::{Tool, ToolSchema};

/// Filter applied to `schemas()` to hide tools from the model.
///
/// Derived from `permissions.allowed_tools` and `permissions.disallowed_tools`
/// in config. Applied at schema-send time so a hidden tool never enters
/// the model's context. Distinct from the permission-check system, which
/// runs at call time — this one prevents the model from even seeing the
/// tool exists.
#[derive(Debug, Clone, Default)]
pub struct ToolVisibilityFilter {
    allowed: Vec<String>,
    disallowed: Vec<String>,
}

impl ToolVisibilityFilter {
    pub fn new(allowed: Vec<String>, disallowed: Vec<String>) -> Self {
        Self {
            allowed,
            disallowed,
        }
    }

    /// True when this filter would include `name` in the visible set.
    pub fn allows(&self, name: &str) -> bool {
        // Deny wins — if any disallow pattern matches, the tool is
        // hidden regardless of the allowlist.
        if self.disallowed.iter().any(|p| glob_match(p, name)) {
            return false;
        }
        // Empty allowlist means "no allowlist configured" — every tool
        // that passes the denylist is visible.
        if self.allowed.is_empty() {
            return true;
        }
        self.allowed.iter().any(|p| glob_match(p, name))
    }
}

/// Match `pattern` against `name`. Supports only trailing `*` as a
/// wildcard (e.g. `mcp__*`). An exact match is the common case; the
/// wildcard is there so operators can hide an entire MCP server's
/// tools with one rule.
fn glob_match(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        pattern == name
    }
}

/// Registry of available tools, keyed by name.
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    visibility: ToolVisibilityFilter,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            visibility: ToolVisibilityFilter::default(),
        }
    }

    /// Install a visibility filter. Applies to `schemas()` and
    /// `core_schemas()`. Call sites should derive the filter from
    /// `config.permissions.allowed_tools` and `.disallowed_tools`.
    pub fn set_visibility(&mut self, filter: ToolVisibilityFilter) {
        self.visibility = filter;
    }

    /// Visibility filter currently installed (mostly for diagnostics).
    pub fn visibility(&self) -> &ToolVisibilityFilter {
        &self.visibility
    }

    /// Create a registry with all default built-in tools. Visibility
    /// filter starts empty; install one via [`Self::set_visibility`].
    pub fn default_tools() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(super::agent::AgentTool));
        registry.register(Arc::new(super::apply_patch::ApplyPatchTool));
        registry.register(Arc::new(super::bash::BashTool));
        registry.register(Arc::new(super::file_read::FileReadTool));
        registry.register(Arc::new(super::file_write::FileWriteTool));
        registry.register(Arc::new(super::file_edit::FileEditTool));
        registry.register(Arc::new(super::multi_edit::MultiEditTool));
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
        registry.register(Arc::new(super::monitor::MonitorTool));
        registry.register(Arc::new(super::todo_write::TodoWriteTool));
        registry.register(Arc::new(super::tool_search::ToolSearchTool));
        registry.register(Arc::new(super::worktree::EnterWorktreeTool));
        registry.register(Arc::new(super::worktree::ExitWorktreeTool));
        registry.register(Arc::new(super::web_fetch::WebFetchTool));
        registry.register(Arc::new(super::web_search::WebSearchTool));
        registry.register(Arc::new(super::ask_user::AskUserQuestionTool));
        registry.register(Arc::new(super::powershell::PowerShellTool));
        registry.register(Arc::new(super::brief::BriefTool));
        registry.register(Arc::new(super::config_tool::ConfigTool));
        registry.register(Arc::new(super::cron_create::CronCreateTool));
        registry.register(Arc::new(super::cron_delete::CronDeleteTool));
        registry.register(Arc::new(super::cron_list::CronListTool));
        registry.register(Arc::new(super::mcp_auth::McpAuthTool));
        registry.register(Arc::new(super::remote_trigger::RemoteTriggerTool));
        registry
    }

    /// A registry containing only the local read tools (`FileRead`, `Grep`,
    /// `Glob`). Confined workers (the AMR scan map phase) use this so the
    /// executor physically cannot dispatch a write, exec, or network tool
    /// even if the model emits a tool-use block for a hidden name. Combine
    /// with a read scope on the permission checker to also bound the paths
    /// these tools may touch.
    pub fn read_only_file_tools() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(super::file_read::FileReadTool));
        registry.register(Arc::new(super::grep::GrepTool));
        registry.register(Arc::new(super::glob::GlobTool));
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

    /// Get tool schemas for the API request. Honors the visibility
    /// filter so tools hidden by `permissions.allowed_tools` /
    /// `permissions.disallowed_tools` never reach the model.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .iter()
            .filter(|t| t.is_enabled() && self.visibility.allows(t.name()))
            .map(|t| ToolSchema::from(t.as_ref()))
            .collect()
    }

    /// Get only always-loaded (core) tool schemas. Honors the
    /// visibility filter. Deferred tools are discoverable via
    /// ToolSearch but not sent on every request.
    pub fn core_schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .iter()
            .filter(|t| {
                t.is_enabled() && !is_deferred(t.name()) && self.visibility.allows(t.name())
            })
            .map(|t| ToolSchema::from(t.as_ref()))
            .collect()
    }

    /// Get deferred tool names (for the ToolSearch system prompt).
    /// Honors the visibility filter so a hidden tool can't be
    /// revived by ToolSearch either.
    pub fn deferred_names(&self) -> Vec<&str> {
        self.tools
            .iter()
            .filter(|t| t.is_enabled() && is_deferred(t.name()) && self.visibility.allows(t.name()))
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

    // ---- ToolVisibilityFilter ----

    #[test]
    fn visibility_empty_filter_allows_everything() {
        let f = ToolVisibilityFilter::default();
        assert!(f.allows("Bash"));
        assert!(f.allows("FileRead"));
        assert!(f.allows("some_random_name"));
    }

    #[test]
    fn visibility_allowlist_restricts_to_listed() {
        let f = ToolVisibilityFilter::new(vec!["FileRead".into(), "Grep".into()], vec![]);
        assert!(f.allows("FileRead"));
        assert!(f.allows("Grep"));
        assert!(!f.allows("Bash"));
        assert!(!f.allows("FileWrite"));
    }

    #[test]
    fn visibility_disallow_wins_over_allow() {
        // Even when a tool is on the allowlist, if it's also on the
        // denylist it's hidden. This is the safe default — operators
        // can safely add entries to the denylist without auditing
        // the allowlist for overlaps.
        let f =
            ToolVisibilityFilter::new(vec!["Bash".into(), "FileRead".into()], vec!["Bash".into()]);
        assert!(!f.allows("Bash"));
        assert!(f.allows("FileRead"));
    }

    #[test]
    fn visibility_wildcard_matches_prefix() {
        let f = ToolVisibilityFilter::new(vec!["mcp__*".into()], vec![]);
        assert!(f.allows("mcp__github__create_issue"));
        assert!(f.allows("mcp__any"));
        assert!(!f.allows("Bash"));
    }

    #[test]
    fn visibility_disallow_wildcard_hides_whole_namespace() {
        let f = ToolVisibilityFilter::new(vec![], vec!["mcp__*".into()]);
        assert!(!f.allows("mcp__github__create_issue"));
        assert!(f.allows("Bash"));
        assert!(f.allows("FileRead"));
    }

    #[test]
    fn visibility_only_trailing_star_is_wildcard() {
        // A `*` not at the end is treated literally — we deliberately
        // keep the matcher minimal. Operators can still list tools
        // explicitly; wildcard support exists for the common "hide
        // every tool from this MCP server" case.
        let f = ToolVisibilityFilter::new(vec!["foo*bar".into()], vec![]);
        assert!(!f.allows("foobar"));
        assert!(!f.allows("foo_anything_bar"));
    }

    #[test]
    fn schemas_honor_visibility_filter() {
        let mut reg = ToolRegistry::default_tools();
        let all_before = reg.schemas().len();
        reg.set_visibility(ToolVisibilityFilter::new(
            vec![],
            vec!["Bash".into(), "WebFetch".into()],
        ));
        let filtered = reg.schemas();
        assert_eq!(filtered.len(), all_before - 2);
        assert!(filtered.iter().all(|s| s.name != "Bash"));
        assert!(filtered.iter().all(|s| s.name != "WebFetch"));
    }

    #[test]
    fn core_schemas_honor_visibility_filter() {
        let mut reg = ToolRegistry::default_tools();
        reg.set_visibility(ToolVisibilityFilter::new(
            vec!["FileRead".into(), "Grep".into()],
            vec![],
        ));
        let core = reg.core_schemas();
        // Allowlist of only two tools — the core set is now those two
        // (both are non-deferred, so they survive the dual filter).
        assert_eq!(core.len(), 2);
        assert!(core.iter().any(|s| s.name == "FileRead"));
        assert!(core.iter().any(|s| s.name == "Grep"));
    }

    #[test]
    fn deferred_names_honor_visibility_filter() {
        let mut reg = ToolRegistry::default_tools();
        reg.set_visibility(ToolVisibilityFilter::new(vec![], vec!["Sleep".into()]));
        let deferred = reg.deferred_names();
        assert!(!deferred.contains(&"Sleep"));
        // Other deferred tools are still listed.
        assert!(deferred.contains(&"NotebookEdit"));
    }
}
