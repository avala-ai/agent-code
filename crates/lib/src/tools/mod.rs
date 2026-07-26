//! Tool system.
//!
//! Tools are the primary way the agent interacts with the environment.
//! Each tool implements the `Tool` trait and is registered in the
//! `ToolRegistry` for dispatch by name.
//!
//! # Architecture
//!
//! - `Tool` trait — defines the interface for all tools
//! - `ToolRegistry` — collects tools and dispatches by name
//! - `ToolExecutor` — manages concurrent/serial tool execution
//! - Individual tool modules — concrete implementations
//!
//! # Tool execution flow
//!
//! 1. Input validation (schema check)
//! 2. Permission check (allow/ask/deny)
//! 3. Tool execution (`call`)
//! 4. Result mapping (to API format)

pub mod agent;
pub mod apply_patch;
pub mod ask_user;
pub mod bash;
pub mod bash_parse;
pub mod brief;
pub mod config_tool;
pub mod cron_create;
pub mod cron_delete;
pub mod cron_list;
pub mod cron_support;
pub mod event_sink;
pub mod executor;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob;
pub mod grep;
pub mod lsp_tool;
pub mod mcp_auth;
pub mod mcp_proxy;
pub mod mcp_resources;
pub mod monitor;
pub mod multi_edit;
pub mod notebook_edit;
pub mod plan_mode;
pub mod plugin_exec;
pub mod powershell;
pub mod registry;
pub mod remote_trigger;
pub mod repl_tool;
pub mod send_message;
pub mod skill_tool;
pub mod sleep_tool;
pub mod tasks;
pub mod todo_write;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod worktree;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::error::ToolError;
use crate::permissions::{PermissionChecker, PermissionDecision};

/// The core trait that all tools must implement.
///
/// Tools are the bridge between the LLM's intentions and the local
/// environment. Each tool defines its input schema (for the LLM),
/// permission requirements, concurrency behavior, and execution logic.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name used in API tool_use blocks.
    fn name(&self) -> &'static str;

    /// Human-readable description sent to the LLM.
    fn description(&self) -> &'static str;

    /// System prompt instructions for this tool.
    fn prompt(&self) -> String {
        self.description().to_string()
    }

    /// JSON Schema for the tool's input parameters.
    fn input_schema(&self) -> serde_json::Value;

    /// Execute the tool with validated input.
    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError>;

    /// Whether this tool only reads state (no mutations).
    fn is_read_only(&self) -> bool {
        false
    }

    /// Whether this tool can safely run concurrently with other tools.
    /// Read-only tools are typically concurrency-safe.
    fn is_concurrency_safe(&self) -> bool {
        self.is_read_only()
    }

    /// Whether this tool is destructive (deletes data, force-pushes, etc.).
    fn is_destructive(&self) -> bool {
        false
    }

    /// Whether this tool is currently enabled in the environment.
    fn is_enabled(&self) -> bool {
        true
    }

    /// Maximum result size in characters before truncation.
    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    /// Check permissions for executing this tool with the given input.
    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        checker: &PermissionChecker,
    ) -> PermissionDecision {
        if self.is_read_only() {
            // Reads default to Allow, but an explicit Deny rule must still
            // block them — `check_read` consults exactly those rules (a
            // configured `{tool: "FileRead", pattern: "*.secret", Deny}`
            // was previously ignored because only the scope check ran).
            if let PermissionDecision::Deny(reason) = checker.check_read(self.name(), input) {
                return PermissionDecision::Deny(reason);
            }
            // Then honor a read scope when one is set (confined workers
            // such as the AMR scan map phase). With no scope this returns
            // Allow, so the interactive agent is unaffected.
            checker.check_read_scope(self.name(), input)
        } else {
            checker.check(self.name(), input)
        }
    }

    /// Validate tool input shape and content BEFORE any side-effecting
    /// step (PreToolUse hooks, permission prompts, audit logging) runs.
    ///
    /// Tools override this to reject obviously-malformed or
    /// allow-list-violating inputs at the engine boundary. The query
    /// loop calls `validate_input` first; if it returns `Err`, the
    /// call short-circuits with no hook fired and no permission
    /// check, so PreToolUse audit hooks never see disallowed inputs.
    /// Default returns `Ok(())` so existing tools don't need updating.
    ///
    /// Use [`ToolError::InvalidInput`] for shape errors and to keep
    /// the rejection message visible to the model.
    fn validate_input(&self, _input: &serde_json::Value) -> Result<(), ToolError> {
        Ok(())
    }

    /// Extract a file path from the input, if applicable (for permission matching).
    fn get_path(&self, _input: &serde_json::Value) -> Option<PathBuf> {
        None
    }
}

/// Permission prompt response from the UI layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResponse {
    AllowOnce,
    AllowSession,
    /// Approve and remember across sessions. Recorded by exact key over
    /// the full normalized operation
    /// ([`crate::tools::executor::persistent_grant_key`]), so it covers
    /// this call and nothing adjacent to it — see
    /// [`crate::permissions::grants`].
    AllowAlways,
    Deny,
}

/// Trait for prompting the user for permission decisions.
/// Implemented by the CLI's UI layer; the lib engine uses this abstraction.
///
/// **Signature note (PR #415):** `origin` is the requesting agent id when the
/// ask comes from a background/subagent context (`None` for the lead session).
/// UI impls should surface it (e.g. "from research-1") when `Some`.
pub trait PermissionPrompter: Send + Sync {
    fn ask(
        &self,
        tool_name: &str,
        description: &str,
        input_preview: Option<&str>,
        origin: Option<&str>,
    ) -> PermissionResponse;
}

/// Default prompter that always allows (for non-interactive/testing).
pub struct AutoAllowPrompter;
impl PermissionPrompter for AutoAllowPrompter {
    fn ask(&self, _: &str, _: &str, _: Option<&str>, _: Option<&str>) -> PermissionResponse {
        PermissionResponse::AllowOnce
    }
}

/// One choice in an [`AskUserQuestion`](ask_user::AskUserQuestionTool) prompt.
#[derive(Debug, Clone)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// One question with labeled options.
#[derive(Debug, Clone)]
pub struct UserQuestion {
    pub question: String,
    pub options: Vec<QuestionOption>,
}

/// Trait for multi-choice questions (modern TUI modal / classic stdin).
///
/// Returns one selected **label** per question, in order. Fail closed when
/// the UI is gone — do not hang on stdin under alt-screen raw mode.
pub trait QuestionAsker: Send + Sync {
    fn ask(&self, questions: &[UserQuestion]) -> Result<Vec<String>, String>;
}

/// Context passed to every tool during execution.
///
/// Provides the working directory, cancellation token, permission
/// checker, file cache, and other shared state. Created by the
/// executor before each tool call.
pub struct ToolContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Cancellation token for cooperative cancellation.
    pub cancel: CancellationToken,
    /// Permission checker instance.
    pub permission_checker: Arc<PermissionChecker>,
    /// Whether to produce verbose output.
    pub verbose: bool,
    /// Plan mode: only read-only tools allowed.
    pub plan_mode: bool,
    /// File content cache for avoiding redundant reads.
    pub file_cache: Option<Arc<tokio::sync::Mutex<crate::services::file_cache::FileCache>>>,
    /// Permission denial tracker for reporting.
    pub denial_tracker:
        Option<Arc<tokio::sync::Mutex<crate::permissions::tracking::DenialTracker>>>,
    /// Shared background task manager.
    pub task_manager: Option<Arc<crate::services::background::TaskManager>>,
    /// Per-session stable color assignments for spawned subagents.
    ///
    /// Populated for the Agent tool path (and the LocalAgent task
    /// executor) so each spawned subagent gets a distinct, stable
    /// color in `/tasks` and downstream UI. Optional so test-only
    /// `ToolContext`s can pass `None`.
    pub subagent_colors: Option<Arc<crate::services::subagent_colors::SubagentColorManager>>,
    /// Tools allowed for the rest of the session (via "Allow for session" prompt).
    pub session_allows: Option<Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>>,
    /// Grants that outlive the session (via "always allow"). Separate
    /// from `session_allows` because these are written to disk and must
    /// survive a restart; `None` disables the feature entirely.
    pub persistent_grants: Option<Arc<tokio::sync::Mutex<crate::permissions::grants::GrantStore>>>,
    /// Permission prompter for interactive approval.
    pub permission_prompter: Option<Arc<dyn PermissionPrompter>>,
    /// Multi-choice question asker (modern modal / tests). When `None`,
    /// [`ask_user::AskUserQuestionTool`] falls back to stdin (classic REPL).
    pub question_asker: Option<Arc<dyn QuestionAsker>>,
    /// Origin agent id for permission attribution (subagent / bg task).
    pub agent_origin: Option<String>,
    /// Process-level sandbox executor.
    ///
    /// `None` means sandboxing is unavailable for this context
    /// (e.g. parallel read-only retry paths); subprocess-spawning tools
    /// should treat `None` as "pass through unchanged". The main query
    /// loop populates this from [`crate::config::SandboxConfig`].
    pub sandbox: Option<Arc<crate::sandbox::SandboxExecutor>>,
    /// Name of the active disk-loaded output style, if any.
    ///
    /// The `Agent` tool propagates this to spawned subagents via the
    /// `AGENT_CODE_DISK_OUTPUT_STYLE` env var so a style with
    /// `applies_to: [subagent]` actually reaches the child. `None` means
    /// the parent has no active disk style (built-in or default).
    pub active_disk_output_style: Option<String>,
    /// Caps the number of concurrently-running background subagents the
    /// `Agent` tool may spawn. `None` leaves subagent spawns unbounded
    /// (e.g. in tests / non-interactive contexts).
    pub agent_limiter:
        Option<std::sync::Arc<crate::services::agent_control::AgentExecutionLimiter>>,
    /// Live tool-event channel (stdout chunks, etc.). Optional so tests
    /// and one-shot paths can omit it.
    pub tool_events: Option<event_sink::ToolEventTx>,
    /// Id of the tool call currently executing (for correlating events).
    pub active_call_id: Option<String>,
    /// Config-level provider defaults for spawned subagents (the `[api]`
    /// `subagent_model` / `subagent_base_url` / `subagent_auth_mode`
    /// keys). The Agent tool folds these under per-call and
    /// per-definition overrides when resolving a child's endpoint.
    /// `None` → subagents inherit the parent's provider settings.
    pub subagent_api_defaults: Option<crate::services::coordinator::SubagentEndpoint>,
    /// Live plan-mode flag shared with the engine (flipped by
    /// EnterPlanMode/ExitPlanMode and the UI's mode toggle). When set,
    /// [`Self::plan_mode_now`] reads it so a toggle that lands
    /// mid-batch gates the very next tool call; the `plan_mode` bool
    /// is only the snapshot taken when this context was built and lags
    /// one agent-loop iteration.
    pub live_plan_mode: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Session id of the owning engine, for session-scoped tool state
    /// (e.g. the active-plan pointer, so a session resumed in a new
    /// process finds its plan). `None` in minimal/test contexts falls
    /// back to process-scoped state.
    pub session_id: Option<String>,
}

impl ToolContext {
    /// Minimal context for unit tests (`cwd = "."`, allow-all permissions).
    ///
    /// Prefer this (or [`Self::minimal`]) over hand-rolling every optional
    /// field — the struct grows with engine features and field-list
    /// construction is a common source of merge noise in tool tests.
    pub fn for_tests() -> Self {
        Self::minimal(PathBuf::from("."), CancellationToken::new())
    }

    /// Build a minimal context with the given cwd and cancel token.
    ///
    /// All optional fields are `None` / false; permissions allow everything.
    pub fn minimal(cwd: PathBuf, cancel: CancellationToken) -> Self {
        Self {
            cwd,
            cancel,
            permission_checker: Arc::new(PermissionChecker::allow_all()),
            verbose: false,
            plan_mode: false,
            file_cache: None,
            denial_tracker: None,
            task_manager: None,
            subagent_colors: None,
            session_allows: None,
            persistent_grants: None,
            permission_prompter: None,
            question_asker: None,
            agent_origin: None,
            sandbox: None,
            active_disk_output_style: None,
            agent_limiter: None,
            tool_events: None,
            active_call_id: None,
            subagent_api_defaults: None,
            live_plan_mode: None,
            session_id: None,
        }
    }

    /// Whether plan mode is active *right now*. Reads the live shared
    /// flag when present, falling back to the per-iteration snapshot —
    /// so a Plan toggle that lands mid-batch (while an earlier tool in
    /// the same batch is still running) gates the next call instead of
    /// lagging a full agent-loop iteration.
    pub fn plan_mode_now(&self) -> bool {
        self.live_plan_mode
            .as_ref()
            .map(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
            .unwrap_or(self.plan_mode)
    }

    /// Emit progressive tool output when a live event channel is installed.
    pub fn emit_tool_output(&self, chunk: &str) {
        if let (Some(tx), Some(id)) = (&self.tool_events, &self.active_call_id) {
            tx.emit_output(id, chunk);
        }
    }

    /// Clone this context with a specific active call id (and shared event tx).
    pub fn with_call_id(&self, call_id: impl Into<String>) -> Self {
        Self {
            cwd: self.cwd.clone(),
            cancel: self.cancel.clone(),
            permission_checker: self.permission_checker.clone(),
            verbose: self.verbose,
            plan_mode: self.plan_mode,
            file_cache: self.file_cache.clone(),
            denial_tracker: self.denial_tracker.clone(),
            task_manager: self.task_manager.clone(),
            subagent_colors: self.subagent_colors.clone(),
            session_allows: self.session_allows.clone(),
            persistent_grants: self.persistent_grants.clone(),
            permission_prompter: self.permission_prompter.clone(),
            question_asker: self.question_asker.clone(),
            agent_origin: self.agent_origin.clone(),
            sandbox: self.sandbox.clone(),
            active_disk_output_style: self.active_disk_output_style.clone(),
            agent_limiter: self.agent_limiter.clone(),
            tool_events: self.tool_events.clone(),
            active_call_id: Some(call_id.into()),
            subagent_api_defaults: self.subagent_api_defaults.clone(),
            live_plan_mode: self.live_plan_mode.clone(),
            session_id: self.session_id.clone(),
        }
    }
}

/// Result of a tool execution.
///
/// Contains the output text and whether it represents an error.
/// Injected into the conversation as a `ToolResult` content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The main output content.
    pub content: String,
    /// Whether the result represents an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful result.
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error result.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Schema information for a tool, used when building API requests.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
}

impl<T: Tool + ?Sized> From<&T> for ToolSchema {
    fn from(tool: &T) -> Self {
        Self {
            name: tool.name(),
            description: tool.description(),
            input_schema: tool.input_schema(),
        }
    }
}

#[cfg(test)]
mod default_check_tests {
    use super::*;
    use crate::config::{PermissionMode, PermissionRule, PermissionsConfig};

    struct ReadOnlyProbe;

    #[async_trait]
    impl Tool for ReadOnlyProbe {
        fn name(&self) -> &'static str {
            "FileRead"
        }
        fn description(&self) -> &'static str {
            "test read-only tool"
        }
        fn prompt(&self) -> String {
            String::new()
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn is_read_only(&self) -> bool {
            true
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult, crate::error::ToolError> {
            Ok(ToolResult::success(String::new()))
        }
    }

    /// The DEFAULT read-only permission path must honor explicit Deny
    /// rules — it used to run only the read-scope check, making
    /// `{tool: "FileRead", pattern: "*.secret", Deny}` dead config.
    #[tokio::test]
    async fn default_read_only_check_honors_explicit_deny_rule() {
        let checker = PermissionChecker::from_config(&PermissionsConfig {
            default_mode: PermissionMode::Allow,
            rules: vec![PermissionRule {
                tool: "FileRead".into(),
                pattern: Some("*.secret".into()),
                action: PermissionMode::Deny,
            }],
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
        });
        let denied = ReadOnlyProbe
            .check_permissions(&serde_json::json!({"file_path": "keys.secret"}), &checker)
            .await;
        assert!(matches!(denied, PermissionDecision::Deny(_)));

        let allowed = ReadOnlyProbe
            .check_permissions(&serde_json::json!({"file_path": "src/lib.rs"}), &checker)
            .await;
        assert!(matches!(allowed, PermissionDecision::Allow));
    }
}
