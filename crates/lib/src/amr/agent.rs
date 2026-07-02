//! Worker execution behind a small trait.
//!
//! MAP and REDUCE are just bounded agent turns. Putting them behind
//! [`AmrAgent`] keeps the orchestrator provider-agnostic and, crucially,
//! unit-testable: tests drive the pipeline with [`ClosureAgent`] (canned
//! responses, no network), while production uses [`EngineAgent`], which
//! runs a real in-process [`QueryEngine`] exactly the way the scheduler's
//! one-shot path does.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::config::Config;
use crate::llm::message::{ContentBlock, Message};
use crate::llm::provider::Provider;
use crate::output_styles::AgentKind;
use crate::permissions::PermissionChecker;
use crate::query::{QueryEngine, QueryEngineConfig, StreamSink};
use crate::state::AppState;
use crate::tools::registry::ToolRegistry;

use super::AmrError;

/// Read-only tool set handed to MAP workers: they may inspect the code but
/// never mutate the repository they are scanning.
pub const READ_ONLY_TOOLS: &[&str] = &["FileRead", "Grep", "Glob"];

/// A [`StreamSink`] that accumulates the assistant's streamed text. AMR
/// workers run headless, so the streamed text is captured here rather than
/// reconstructed from history.
#[derive(Default)]
struct CapturingSink {
    text: std::sync::Mutex<String>,
}

impl CapturingSink {
    fn take(&self) -> String {
        std::mem::take(&mut self.text.lock().unwrap())
    }
}

impl StreamSink for CapturingSink {
    fn on_text(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    // A MAP/REDUCE worker streams reasoning text and then calls a tool; the
    // final JSON answer arrives in a LATER turn. Clear the buffer when a tool
    // call starts so `take()` returns only the text streamed after the last
    // tool call (the final turn). Otherwise an earlier preamble's fenced block
    // or `{...}` span could be parsed as the answer instead of the real JSON,
    // causing false worker failures or stale findings.
    fn on_tool_start(&self, _tool_name: &str, _input: &serde_json::Value) {
        self.text.lock().unwrap().clear();
    }
    fn on_tool_result(&self, _tool_name: &str, _result: &crate::tools::ToolResult) {}
    fn on_error(&self, _error: &str) {}
}

/// The outcome of one agent turn.
#[derive(Debug, Clone, Default)]
pub struct AgentRun {
    /// Concatenated assistant text from the final turn.
    pub text: String,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Per-invocation knobs.
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// Model override for this call (e.g. a cheaper model for MAP).
    pub model: Option<String>,
    pub max_turns: usize,
    /// When true, restrict the tool set to [`READ_ONLY_TOOLS`].
    pub read_only: bool,
    /// Project root the worker's permissions are scoped to.
    pub project_root: PathBuf,
}

impl RunOpts {
    pub fn map_worker(project_root: PathBuf, model: Option<String>, max_turns: usize) -> Self {
        Self {
            model,
            max_turns,
            read_only: true,
            project_root,
        }
    }

    pub fn reduce_worker(project_root: PathBuf, model: Option<String>, max_turns: usize) -> Self {
        Self {
            model,
            max_turns,
            // The reducer reasons over conclusions and does not read the
            // repo, but read-only tools are harmless and let it spot-check.
            read_only: true,
            project_root,
        }
    }
}

/// Something that can run a bounded agent turn and return its text + cost.
#[async_trait]
pub trait AmrAgent: Send + Sync {
    async fn run(&self, prompt: &str, opts: &RunOpts) -> Result<AgentRun, AmrError>;
}

/// Production agent: an in-process [`QueryEngine`] over the configured LLM.
pub struct EngineAgent {
    llm: Arc<dyn Provider>,
    base_config: Config,
}

impl EngineAgent {
    pub fn new(llm: Arc<dyn Provider>, base_config: Config) -> Self {
        Self { llm, base_config }
    }
}

/// Derive a worker engine's config from the base config:
/// - apply the per-call model override (a cheaper model for MAP, say);
/// - run non-interactively (never prompt for permission);
/// - disable end-of-turn background memory extraction. Leaving it on would fire
///   an extra, unaccounted LLM request per shard (undercounting cost and adding
///   rate-limit exposure) and distill memories from the untrusted repository
///   being scanned into the user's persistent memory store.
fn worker_config(base: &Config, opts: &RunOpts) -> Config {
    let mut config = base.clone();
    if let Some(model) = &opts.model {
        config.api.model = model.clone();
    }
    config.permissions.default_mode = crate::config::PermissionMode::Allow;
    config.features.extract_memories = false;
    config
}

#[async_trait]
impl AmrAgent for EngineAgent {
    async fn run(&self, prompt: &str, opts: &RunOpts) -> Result<AgentRun, AmrError> {
        let config = worker_config(&self.base_config, opts);

        // Confined workers get a registry that only contains the read tools,
        // so the executor cannot dispatch a write/exec/network tool even if the
        // model emits a call for a hidden name. A read scope on the permission
        // checker (below) then bounds which paths those tools may touch.
        let registry = if opts.read_only {
            ToolRegistry::read_only_file_tools()
        } else {
            ToolRegistry::default_tools()
        };

        let mut permission_checker = PermissionChecker::from_config(&config.permissions)
            .with_project_root(opts.project_root.clone());
        if opts.read_only {
            // Confine the worker's reads to the scan target so injected
            // instructions in scanned code cannot exfiltrate local files.
            permission_checker = permission_checker.with_read_scope(opts.project_root.clone());
        }
        let app_state = AppState::new(config.clone());

        let mut engine = QueryEngine::new(
            self.llm.clone(),
            registry,
            permission_checker,
            app_state,
            QueryEngineConfig {
                max_turns: Some(opts.max_turns),
                verbose: false,
                unattended: true,
                agent_kind: AgentKind::Subagent,
            },
        );

        // Capture the streamed assistant text directly. A headless run with
        // a no-op sink can leave the final turn's text out of message
        // history, so the sink is the source of truth and history is only a
        // fallback.
        let sink = CapturingSink::default();
        let result = engine.run_turn_with_sink(prompt, &sink).await;
        let captured = sink.take();
        let state = engine.state();
        if let Err(e) = result {
            return Err(AmrError::Worker(e.to_string()));
        }
        let text = if captured.trim().is_empty() {
            last_assistant_text(&state.messages)
        } else {
            captured
        };
        Ok(AgentRun {
            text,
            cost_usd: state.total_cost_usd,
            input_tokens: state.total_usage.input_tokens,
            output_tokens: state.total_usage.output_tokens,
        })
    }
}

/// Concatenate the text blocks of the last assistant message.
fn last_assistant_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::Assistant(a) => {
                let text: String = a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if text.is_empty() { None } else { Some(text) }
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// Test/eval agent: computes its reply from the prompt with a closure, so a
/// pipeline can be exercised deterministically with no LLM in the loop.
pub struct ClosureAgent<F>
where
    F: Fn(&str) -> String + Send + Sync,
{
    reply: F,
    cost_per_call: f64,
}

impl<F> ClosureAgent<F>
where
    F: Fn(&str) -> String + Send + Sync,
{
    pub fn new(reply: F) -> Self {
        Self {
            reply,
            cost_per_call: 0.0,
        }
    }

    pub fn with_cost(reply: F, cost_per_call: f64) -> Self {
        Self {
            reply,
            cost_per_call,
        }
    }
}

#[async_trait]
impl<F> AmrAgent for ClosureAgent<F>
where
    F: Fn(&str) -> String + Send + Sync,
{
    async fn run(&self, prompt: &str, _opts: &RunOpts) -> Result<AgentRun, AmrError> {
        Ok(AgentRun {
            text: (self.reply)(prompt),
            cost_usd: self.cost_per_call,
            input_tokens: 0,
            output_tokens: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn closure_agent_echoes_reply_and_cost() {
        let agent = ClosureAgent::with_cost(|p| format!("saw:{p}"), 0.25);
        let opts = RunOpts::map_worker(PathBuf::from("."), None, 5);
        let run = agent.run("hello", &opts).await.unwrap();
        assert_eq!(run.text, "saw:hello");
        assert_eq!(run.cost_usd, 0.25);
    }

    #[test]
    fn worker_config_hardens_the_base_config() {
        let mut base = Config::default();
        base.features.extract_memories = true;
        base.permissions.default_mode = crate::config::PermissionMode::Ask;
        let opts = RunOpts::map_worker(PathBuf::from("/repo"), Some("cheap-model".into()), 10);

        let cfg = worker_config(&base, &opts);
        // No per-shard background memory extraction: it would add an unaccounted
        // LLM call per shard and pull memories from the scanned repo into the
        // user's store.
        assert!(!cfg.features.extract_memories);
        // Non-interactive: workers never prompt.
        assert!(matches!(
            cfg.permissions.default_mode,
            crate::config::PermissionMode::Allow
        ));
        // The per-call model override is applied.
        assert_eq!(cfg.api.model, "cheap-model");
    }

    #[test]
    fn read_only_tool_list_is_read_only() {
        assert!(READ_ONLY_TOOLS.contains(&"FileRead"));
        assert!(!READ_ONLY_TOOLS.contains(&"FileWrite"));
        assert!(!READ_ONLY_TOOLS.contains(&"Bash"));
    }

    #[test]
    fn capturing_sink_accumulates_and_drains() {
        let sink = CapturingSink::default();
        sink.on_text("hello ");
        sink.on_text("world");
        assert_eq!(sink.take(), "hello world");
        assert_eq!(sink.take(), "", "take drains the buffer");
    }

    #[test]
    fn capturing_sink_keeps_only_text_after_the_last_tool_call() {
        let sink = CapturingSink::default();
        // A multi-turn worker's pre-tool preamble — note it contains a fenced
        // block the AMR parser would otherwise latch onto.
        sink.on_text("Let me inspect this.\n```py\neval(x)\n```\n");
        sink.on_tool_start("FileRead", &serde_json::json!({"file_path": "a.py"}));
        // The real answer, streamed in the turn after the tool result.
        sink.on_text("```json\n{\"findings\":[]}\n```");
        assert_eq!(
            sink.take(),
            "```json\n{\"findings\":[]}\n```",
            "only text after the last tool call is captured, not the preamble"
        );
    }
}
