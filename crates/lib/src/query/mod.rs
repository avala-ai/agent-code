//! Query engine: the core agent loop.
//!
//! Implements the agentic cycle:
//!
//! 1. Auto-compact if context nears the window limit
//! 2. Microcompact stale tool results
//! 3. Call LLM with streaming
//! 4. Accumulate response content blocks
//! 5. Handle errors (prompt-too-long, rate limits, max-output-tokens)
//! 6. Extract tool_use blocks
//! 7. Execute tools (concurrent/serial batching)
//! 8. Inject tool results into history
//! 9. Repeat from step 1 until no tool_use or max turns

pub mod source;

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::hooks::{HookEvent, HookRegistry};
use crate::llm::message::*;
use crate::llm::provider::{Provider, ProviderError, ProviderRequest};
use crate::llm::stream::StreamEvent;
use crate::permissions::PermissionChecker;
use crate::services::compact::{self, CompactTracking, MAX_OUTPUT_TOKENS_RECOVERY_LIMIT};
use crate::services::tokens;
use crate::state::AppState;
use crate::tools::ToolContext;
use crate::tools::executor::{execute_tool_calls, extract_tool_calls};
use crate::tools::registry::ToolRegistry;

/// Configuration for the query engine.
pub struct QueryEngineConfig {
    pub max_turns: Option<usize>,
    pub verbose: bool,
    /// Whether this is a non-interactive (one-shot) session.
    pub unattended: bool,
}

/// The query engine orchestrates the agent loop.
pub struct QueryEngine {
    llm: Arc<dyn Provider>,
    tools: ToolRegistry,
    file_cache: Arc<tokio::sync::Mutex<crate::services::file_cache::FileCache>>,
    permissions: Arc<PermissionChecker>,
    state: AppState,
    config: QueryEngineConfig,
    cancel: CancellationToken,
    hooks: HookRegistry,
    cache_tracker: crate::services::cache_tracking::CacheTracker,
    denial_tracker: Arc<tokio::sync::Mutex<crate::permissions::tracking::DenialTracker>>,
    extraction_state: Arc<tokio::sync::Mutex<crate::memory::extraction::ExtractionState>>,
    session_allows: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    permission_prompter: Option<Arc<dyn crate::tools::PermissionPrompter>>,
}

/// Callback for streaming events to the UI.
pub trait StreamSink: Send + Sync {
    fn on_text(&self, text: &str);
    fn on_tool_start(&self, tool_name: &str, input: &serde_json::Value);
    fn on_tool_result(&self, tool_name: &str, result: &crate::tools::ToolResult);
    fn on_thinking(&self, _text: &str) {}
    fn on_turn_complete(&self, _turn: usize) {}
    fn on_error(&self, error: &str);
    fn on_usage(&self, _usage: &Usage) {}
    fn on_compact(&self, _freed_tokens: u64) {}
    fn on_warning(&self, _msg: &str) {}
}

/// A no-op stream sink for non-interactive mode.
pub struct NullSink;
impl StreamSink for NullSink {
    fn on_text(&self, _: &str) {}
    fn on_tool_start(&self, _: &str, _: &serde_json::Value) {}
    fn on_tool_result(&self, _: &str, _: &crate::tools::ToolResult) {}
    fn on_error(&self, _: &str) {}
}

impl QueryEngine {
    pub fn new(
        llm: Arc<dyn Provider>,
        tools: ToolRegistry,
        permissions: PermissionChecker,
        state: AppState,
        config: QueryEngineConfig,
    ) -> Self {
        Self {
            llm,
            tools,
            file_cache: Arc::new(tokio::sync::Mutex::new(
                crate::services::file_cache::FileCache::new(),
            )),
            permissions: Arc::new(permissions),
            state,
            config,
            cancel: CancellationToken::new(),
            hooks: HookRegistry::new(),
            cache_tracker: crate::services::cache_tracking::CacheTracker::new(),
            denial_tracker: Arc::new(tokio::sync::Mutex::new(
                crate::permissions::tracking::DenialTracker::new(100),
            )),
            extraction_state: Arc::new(tokio::sync::Mutex::new(
                crate::memory::extraction::ExtractionState::new(),
            )),
            session_allows: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            permission_prompter: None,
        }
    }

    /// Load hooks from configuration into the registry.
    pub fn load_hooks(&mut self, hook_defs: &[crate::hooks::HookDefinition]) {
        for def in hook_defs {
            self.hooks.register(def.clone());
        }
        if !hook_defs.is_empty() {
            tracing::info!("Loaded {} hooks from config", hook_defs.len());
        }
    }

    /// Get a reference to the app state.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Get a mutable reference to the app state.
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    /// Install a Ctrl+C handler that triggers the cancellation token.
    /// Call this once at startup. Subsequent Ctrl+C signals during a
    /// turn will cancel the active operation instead of killing the process.
    pub fn install_signal_handler(&self) {
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            loop {
                if tokio::signal::ctrl_c().await.is_ok() {
                    if cancel.is_cancelled() {
                        // Second Ctrl+C — hard exit.
                        std::process::exit(130);
                    }
                    cancel.cancel();
                }
            }
        });
    }

    /// Run a single turn: process user input through the full agent loop.
    pub async fn run_turn(&mut self, user_input: &str) -> crate::error::Result<()> {
        self.run_turn_with_sink(user_input, &NullSink).await
    }

    /// Run a turn with a stream sink for real-time UI updates.
    pub async fn run_turn_with_sink(
        &mut self,
        user_input: &str,
        sink: &dyn StreamSink,
    ) -> crate::error::Result<()> {
        // Reset cancellation token for this turn.
        self.cancel = CancellationToken::new();

        // Add the user message to history.
        let user_msg = user_message(user_input);
        self.state.push_message(user_msg);

        let max_turns = self.config.max_turns.unwrap_or(50);
        let mut compact_tracking = CompactTracking::default();
        let mut retry_state = crate::llm::retry::RetryState::default();
        let retry_config = crate::llm::retry::RetryConfig::default();
        let mut max_output_recovery_count = 0u32;

        // Agent loop: budget check → normalize → compact → call LLM → execute tools → repeat.
        for turn in 0..max_turns {
            self.state.turn_count = turn + 1;
            self.state.is_query_active = true;

            // Budget check before each turn.
            let budget_config = crate::services::budget::BudgetConfig::default();
            match crate::services::budget::check_budget(
                self.state.total_cost_usd,
                self.state.total_usage.total(),
                &budget_config,
            ) {
                crate::services::budget::BudgetDecision::Stop { message } => {
                    sink.on_warning(&message);
                    self.state.is_query_active = false;
                    return Ok(());
                }
                crate::services::budget::BudgetDecision::ContinueWithWarning {
                    message, ..
                } => {
                    sink.on_warning(&message);
                }
                crate::services::budget::BudgetDecision::Continue => {}
            }

            // Normalize messages for API compatibility.
            crate::llm::normalize::ensure_tool_result_pairing(&mut self.state.messages);
            crate::llm::normalize::strip_empty_blocks(&mut self.state.messages);
            crate::llm::normalize::remove_empty_messages(&mut self.state.messages);
            crate::llm::normalize::cap_document_blocks(&mut self.state.messages, 500_000);
            crate::llm::normalize::merge_consecutive_user_messages(&mut self.state.messages);

            debug!("Agent turn {}/{}", turn + 1, max_turns);

            let model = self.state.config.api.model.clone();

            // Step 1: Auto-compact if context is too large.
            if compact::should_auto_compact(self.state.history(), &model, &compact_tracking) {
                let token_count = tokens::estimate_context_tokens(self.state.history());
                let threshold = compact::auto_compact_threshold(&model);
                info!("Auto-compact triggered: {token_count} tokens >= {threshold} threshold");

                // Microcompact first: clear stale tool results.
                let freed = compact::microcompact(&mut self.state.messages, 5);
                if freed > 0 {
                    sink.on_compact(freed);
                    info!("Microcompact freed ~{freed} tokens");
                }

                // Check if microcompact was enough.
                let post_mc_tokens = tokens::estimate_context_tokens(self.state.history());
                if post_mc_tokens >= threshold {
                    // Full LLM-based compaction: summarize older messages.
                    info!("Microcompact insufficient, attempting LLM compaction");
                    match compact::compact_with_llm(&mut self.state.messages, &*self.llm, &model)
                        .await
                    {
                        Some(removed) => {
                            info!("LLM compaction removed {removed} messages");
                            compact_tracking.was_compacted = true;
                            compact_tracking.consecutive_failures = 0;
                        }
                        None => {
                            compact_tracking.consecutive_failures += 1;
                            warn!(
                                "LLM compaction failed (attempt {})",
                                compact_tracking.consecutive_failures
                            );
                            // Fallback: context collapse (snip middle messages).
                            let effective = compact::effective_context_window(&model);
                            if let Some(collapse) =
                                crate::services::context_collapse::collapse_to_budget(
                                    self.state.history(),
                                    effective,
                                )
                            {
                                info!(
                                    "Context collapse snipped {} messages, freed ~{} tokens",
                                    collapse.snipped_count, collapse.tokens_freed
                                );
                                self.state.messages = collapse.api_messages;
                                sink.on_compact(collapse.tokens_freed);
                            } else {
                                // Last resort: aggressive microcompact.
                                let freed2 = compact::microcompact(&mut self.state.messages, 2);
                                if freed2 > 0 {
                                    sink.on_compact(freed2);
                                }
                            }
                        }
                    }
                }
            }

            // Inject compaction reminder if compacted and feature enabled.
            if compact_tracking.was_compacted && self.state.config.features.compaction_reminders {
                let reminder = user_message(
                    "<system-reminder>Context was automatically compacted. \
                     Earlier messages were summarized. If you need details from \
                     before compaction, ask the user or re-read the relevant files.</system-reminder>",
                );
                self.state.push_message(reminder);
                compact_tracking.was_compacted = false; // Only remind once per compaction.
            }

            // Step 2: Check token warning state.
            let warning = compact::token_warning_state(self.state.history(), &model);
            if warning.is_blocking {
                sink.on_warning("Context window nearly full. Consider starting a new session.");
            } else if warning.is_above_warning {
                sink.on_warning(&format!("Context {}% remaining", warning.percent_left));
            }

            // Step 3: Build and send the API request.
            let system_prompt = build_system_prompt(&self.tools, &self.state);
            // Use core schemas (deferred tools loaded on demand via ToolSearch).
            let tool_schemas = self.tools.core_schemas();

            let request = ProviderRequest {
                messages: self.state.history().to_vec(),
                system_prompt: system_prompt.clone(),
                tools: tool_schemas.clone(),
                model: model.clone(),
                max_tokens: self.state.config.api.max_output_tokens.unwrap_or(16384),
                temperature: None,
                enable_caching: true,
                tool_choice: Default::default(),
                metadata: None,
            };

            let mut rx = match self.llm.stream(&request).await {
                Ok(rx) => {
                    retry_state.reset();
                    rx
                }
                Err(e) => {
                    let retryable = match &e {
                        ProviderError::RateLimited { retry_after_ms } => {
                            crate::llm::retry::RetryableError::RateLimited {
                                retry_after: *retry_after_ms,
                            }
                        }
                        ProviderError::Overloaded => crate::llm::retry::RetryableError::Overloaded,
                        ProviderError::Network(_) => {
                            crate::llm::retry::RetryableError::StreamInterrupted
                        }
                        other => crate::llm::retry::RetryableError::NonRetryable(other.to_string()),
                    };

                    match retry_state.next_action(&retryable, &retry_config) {
                        crate::llm::retry::RetryAction::Retry { after } => {
                            warn!("Retrying in {}ms", after.as_millis());
                            tokio::time::sleep(after).await;
                            continue;
                        }
                        crate::llm::retry::RetryAction::FallbackModel => {
                            sink.on_warning("Falling back to smaller model");
                            // TODO: switch model and retry
                            continue;
                        }
                        crate::llm::retry::RetryAction::Abort(reason) => {
                            // Unattended retry: in non-interactive mode, retry
                            // capacity errors with longer backoff instead of aborting.
                            if self.config.unattended
                                && self.state.config.features.unattended_retry
                                && matches!(
                                    &e,
                                    ProviderError::Overloaded | ProviderError::RateLimited { .. }
                                )
                            {
                                warn!("Unattended retry: waiting 30s for capacity");
                                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                                continue;
                            }
                            // Before giving up, try reactive compact for size errors.
                            if let ProviderError::RequestTooLarge(body) = &e {
                                let gap = compact::parse_prompt_too_long_gap(body);
                                let freed = compact::microcompact(&mut self.state.messages, 1);
                                if freed > 0 {
                                    sink.on_compact(freed);
                                    info!("Reactive compact freed ~{freed} tokens (gap: {gap:?})");
                                    continue;
                                }
                            }
                            sink.on_error(&reason);
                            self.state.is_query_active = false;
                            return Err(crate::error::Error::Other(e.to_string()));
                        }
                    }
                }
            };

            // Step 4: Stream response, submitting tool_use blocks for
            // overlapped execution as they complete.
            let mut content_blocks = Vec::new();
            let mut usage = Usage::default();
            let mut stop_reason: Option<StopReason> = None;
            let mut got_error = false;
            let mut error_text = String::new();
            let mut _pending_tool_count = 0usize;

            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::TextDelta(text) => {
                        sink.on_text(&text);
                    }
                    StreamEvent::ContentBlockComplete(block) => {
                        if let ContentBlock::ToolUse {
                            ref name,
                            ref input,
                            ..
                        } = block
                        {
                            sink.on_tool_start(name, input);
                            _pending_tool_count += 1;
                        }
                        if let ContentBlock::Thinking { ref thinking, .. } = block {
                            sink.on_thinking(thinking);
                        }
                        content_blocks.push(block);
                    }
                    StreamEvent::Done {
                        usage: u,
                        stop_reason: sr,
                    } => {
                        usage = u;
                        stop_reason = sr;
                        sink.on_usage(&usage);
                    }
                    StreamEvent::Error(msg) => {
                        got_error = true;
                        error_text = msg.clone();
                        sink.on_error(&msg);
                    }
                    _ => {}
                }
            }

            // Step 5: Record the assistant message.
            let assistant_msg = Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                content: content_blocks.clone(),
                model: Some(model.clone()),
                usage: Some(usage.clone()),
                stop_reason: stop_reason.clone(),
                request_id: None,
            });
            self.state.push_message(assistant_msg);
            self.state.record_usage(&usage, &model);

            // Token budget tracking per turn.
            if self.state.config.features.token_budget && usage.total() > 0 {
                let turn_total = usage.input_tokens + usage.output_tokens;
                if turn_total > 100_000 {
                    sink.on_warning(&format!(
                        "High token usage this turn: {} tokens ({}in + {}out)",
                        turn_total, usage.input_tokens, usage.output_tokens
                    ));
                }
            }

            // Record cache and telemetry.
            let _cache_event = self.cache_tracker.record(&usage);
            {
                let mut span = crate::services::telemetry::api_call_span(
                    &model,
                    turn + 1,
                    &self.state.session_id,
                );
                crate::services::telemetry::record_usage(&mut span, &usage);
                span.finish();
                tracing::debug!(
                    "API call: {}ms, {}in/{}out tokens",
                    span.duration_ms().unwrap_or(0),
                    usage.input_tokens,
                    usage.output_tokens,
                );
            }

            // Step 6: Handle stream errors.
            if got_error {
                // Check if it's a prompt-too-long error in the stream.
                if error_text.contains("prompt is too long")
                    || error_text.contains("Prompt is too long")
                {
                    let freed = compact::microcompact(&mut self.state.messages, 1);
                    if freed > 0 {
                        sink.on_compact(freed);
                        continue;
                    }
                }

                // Check for max-output-tokens hit (partial response).
                if content_blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { .. }))
                    && error_text.contains("max_tokens")
                    && max_output_recovery_count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
                {
                    max_output_recovery_count += 1;
                    info!(
                        "Max output tokens recovery attempt {}/{}",
                        max_output_recovery_count, MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
                    );
                    let recovery_msg = compact::max_output_recovery_message();
                    self.state.push_message(recovery_msg);
                    continue;
                }
            }

            // Step 6b: Handle max_tokens stop reason (escalate and continue).
            if matches!(stop_reason, Some(StopReason::MaxTokens))
                && !got_error
                && content_blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { .. }))
                && max_output_recovery_count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
            {
                max_output_recovery_count += 1;
                info!(
                    "Max tokens stop reason — recovery attempt {}/{}",
                    max_output_recovery_count, MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
                );
                let recovery_msg = compact::max_output_recovery_message();
                self.state.push_message(recovery_msg);
                continue;
            }

            // Step 7: Extract tool calls from the response.
            let tool_calls = extract_tool_calls(&content_blocks);

            if tool_calls.is_empty() {
                // No tools requested — turn is complete.
                info!("Turn complete (no tool calls)");
                sink.on_turn_complete(turn + 1);
                self.state.is_query_active = false;

                // Fire background memory extraction (fire-and-forget).
                // Only runs if feature enabled and memory directory exists.
                if self.state.config.features.extract_memories
                    && crate::memory::ensure_memory_dir().is_some()
                {
                    let extraction_messages = self.state.messages.clone();
                    let extraction_state = self.extraction_state.clone();
                    let extraction_llm = self.llm.clone();
                    let extraction_model = model.clone();
                    tokio::spawn(async move {
                        crate::memory::extraction::extract_memories_background(
                            extraction_messages,
                            extraction_state,
                            extraction_llm,
                            extraction_model,
                        )
                        .await;
                    });
                }

                return Ok(());
            }

            // Step 8: Execute tool calls with pre/post hooks.
            info!("Executing {} tool call(s)", tool_calls.len());
            let cwd = PathBuf::from(&self.state.cwd);
            let tool_ctx = ToolContext {
                cwd,
                cancel: self.cancel.clone(),
                permission_checker: self.permissions.clone(),
                verbose: self.config.verbose,
                plan_mode: self.state.plan_mode,
                file_cache: Some(self.file_cache.clone()),
                denial_tracker: Some(self.denial_tracker.clone()),
                task_manager: Some(self.state.task_manager.clone()),
                session_allows: Some(self.session_allows.clone()),
                permission_prompter: self.permission_prompter.clone(),
            };

            // Fire pre-tool-use hooks.
            for call in &tool_calls {
                self.hooks
                    .run_hooks(&HookEvent::PreToolUse, Some(&call.name), &call.input)
                    .await;
            }

            let results =
                execute_tool_calls(&tool_calls, self.tools.all(), &tool_ctx, &self.permissions)
                    .await;

            // Step 9: Inject tool results + fire post-tool-use hooks.
            for result in &results {
                sink.on_tool_result(&result.tool_name, &result.result);

                // Fire post-tool-use hooks.
                self.hooks
                    .run_hooks(
                        &HookEvent::PostToolUse,
                        Some(&result.tool_name),
                        &serde_json::json!({
                            "tool": result.tool_name,
                            "is_error": result.result.is_error,
                        }),
                    )
                    .await;

                let msg = tool_result_message(
                    &result.tool_use_id,
                    &result.result.content,
                    result.result.is_error,
                );
                self.state.push_message(msg);
            }

            // Continue the loop — the model will see the tool results.
        }

        warn!("Max turns ({max_turns}) reached");
        sink.on_warning(&format!("Agent stopped after {max_turns} turns"));
        self.state.is_query_active = false;
        Ok(())
    }

    /// Cancel the current operation.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Get a cloneable cancel token for use in background tasks.
    pub fn cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancel.clone()
    }
}

/// Build the system prompt from tool definitions, app state, and memory.
pub fn build_system_prompt(tools: &ToolRegistry, state: &AppState) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are an AI coding agent. You help users with software engineering tasks \
         by reading, writing, and searching code. Use the tools available to you to \
         accomplish tasks.\n\n",
    );

    // Environment context.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let is_git = std::path::Path::new(&state.cwd).join(".git").exists();
    prompt.push_str(&format!(
        "# Environment\n\
         - Working directory: {}\n\
         - Platform: {}\n\
         - Shell: {shell}\n\
         - Git repository: {}\n\n",
        state.cwd,
        std::env::consts::OS,
        if is_git { "yes" } else { "no" },
    ));

    // Inject memory context (project + user + on-demand relevant).
    let mut memory = crate::memory::MemoryContext::load(Some(std::path::Path::new(&state.cwd)));

    // On-demand: surface relevant memories based on recent conversation.
    let recent_text: String = state
        .messages
        .iter()
        .rev()
        .take(5)
        .filter_map(|m| match m {
            crate::llm::message::Message::User(u) => Some(
                u.content
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");

    if !recent_text.is_empty() {
        memory.load_relevant(&recent_text);
    }

    let memory_section = memory.to_system_prompt_section();
    if !memory_section.is_empty() {
        prompt.push_str(&memory_section);
    }

    // Tool documentation.
    prompt.push_str("# Available Tools\n\n");
    for tool in tools.all() {
        if tool.is_enabled() {
            prompt.push_str(&format!("## {}\n{}\n\n", tool.name(), tool.prompt()));
        }
    }

    // Available skills.
    let skills = crate::skills::SkillRegistry::load_all(Some(std::path::Path::new(&state.cwd)));
    let invocable = skills.user_invocable();
    if !invocable.is_empty() {
        prompt.push_str("# Available Skills\n\n");
        for skill in invocable {
            let desc = skill.metadata.description.as_deref().unwrap_or("");
            let when = skill.metadata.when_to_use.as_deref().unwrap_or("");
            prompt.push_str(&format!("- `/{}`", skill.name));
            if !desc.is_empty() {
                prompt.push_str(&format!(": {desc}"));
            }
            if !when.is_empty() {
                prompt.push_str(&format!(" (use when: {when})"));
            }
            prompt.push('\n');
        }
        prompt.push('\n');
    }

    // Guidelines and safety framework.
    prompt.push_str(
        "# Using tools\n\n\
         Use dedicated tools instead of shell commands when available:\n\
         - File search: Glob (not find or ls)\n\
         - Content search: Grep (not grep or rg)\n\
         - Read files: FileRead (not cat/head/tail)\n\
         - Edit files: FileEdit (not sed/awk)\n\
         - Write files: FileWrite (not echo/cat with redirect)\n\
         - Reserve Bash for system commands and operations that require shell execution.\n\
         - Break complex tasks into steps. Use multiple tool calls in parallel when independent.\n\
         - Use the Agent tool for complex multi-step research or tasks that benefit from isolation.\n\n\
         # Working with code\n\n\
         - Read files before editing them. Understand existing code before suggesting changes.\n\
         - Prefer editing existing files over creating new ones to avoid file bloat.\n\
         - Only make changes that were requested. Don't add features, refactor, add comments, \
           or make \"improvements\" beyond the ask.\n\
         - Don't add error handling for scenarios that can't happen. Don't design for \
           hypothetical future requirements.\n\
         - When referencing code, include file_path:line_number.\n\
         - Be careful not to introduce security vulnerabilities (command injection, XSS, SQL injection, \
           OWASP top 10). If you notice insecure code you wrote, fix it immediately.\n\
         - Don't add docstrings, comments, or type annotations to code you didn't change.\n\
         - Three similar lines of code is better than a premature abstraction.\n\n\
         # Git safety protocol\n\n\
         - NEVER update the git config.\n\
         - NEVER run destructive git commands (push --force, reset --hard, checkout ., restore ., \
           clean -f, branch -D) unless the user explicitly requests them.\n\
         - NEVER skip hooks (--no-verify, --no-gpg-sign) unless the user explicitly requests it.\n\
         - NEVER force push to main/master. Warn the user if they request it.\n\
         - Always create NEW commits rather than amending, unless the user explicitly requests amend. \
           After hook failure, the commit did NOT happen — amend would modify the PREVIOUS commit.\n\
         - When staging files, prefer adding specific files by name rather than git add -A or git add ., \
           which can accidentally include sensitive files.\n\
         - NEVER commit changes unless the user explicitly asks.\n\n\
         # Committing changes\n\n\
         When the user asks to commit:\n\
         1. Run git status and git diff to see all changes.\n\
         2. Run git log --oneline -5 to match the repository's commit message style.\n\
         3. Draft a concise (1-2 sentence) commit message focusing on \"why\" not \"what\".\n\
         4. Do not commit files that likely contain secrets (.env, credentials.json).\n\
         5. Stage specific files, create the commit.\n\
         6. If pre-commit hook fails, fix the issue and create a NEW commit.\n\
         7. When creating commits, include a co-author attribution line at the end of the message.\n\n\
         # Creating pull requests\n\n\
         When the user asks to create a PR:\n\
         1. Run git status, git diff, and git log to understand all changes on the branch.\n\
         2. Analyze ALL commits (not just the latest) that will be in the PR.\n\
         3. Draft a short title (under 70 chars) and detailed body with summary and test plan.\n\
         4. Push to remote with -u flag if needed, then create PR using gh pr create.\n\
         5. Return the PR URL when done.\n\n\
         # Executing actions safely\n\n\
         Consider the reversibility and blast radius of every action:\n\
         - Freely take local, reversible actions (editing files, running tests).\n\
         - For hard-to-reverse or shared-state actions, confirm with the user first:\n\
           - Destructive: deleting files/branches, dropping tables, rm -rf, overwriting uncommitted changes.\n\
           - Hard to reverse: force-pushing, git reset --hard, amending published commits.\n\
           - Visible to others: pushing code, creating/commenting on PRs/issues, sending messages.\n\
         - When you encounter an obstacle, do not use destructive actions as a shortcut. \
           Identify root causes and fix underlying issues.\n\
         - If you discover unexpected state (unfamiliar files, branches, config), investigate \
           before deleting or overwriting — it may be the user's in-progress work.\n\n\
         # Response style\n\n\
         - Be concise. Lead with the answer or action, not the reasoning.\n\
         - Skip filler, preamble, and unnecessary transitions.\n\
         - Don't restate what the user said.\n\
         - If you can say it in one sentence, don't use three.\n\
         - Focus output on: decisions that need input, status updates, and errors that change the plan.\n\
         - When referencing GitHub issues or PRs, use owner/repo#123 format.\n\
         - Only use emojis if the user explicitly requests it.\n\n\
         # Memory\n\n\
         You can save information across sessions by writing memory files.\n\
         - Save to: ~/.config/agent-code/memory/ (one .md file per topic)\n\
         - Each file needs YAML frontmatter: name, description, type (user/feedback/project/reference)\n\
         - After writing a file, update MEMORY.md with a one-line pointer\n\
         - Memory types: user (role, preferences), feedback (corrections, confirmations), \
           project (decisions, deadlines), reference (external resources)\n\
         - Do NOT store: code patterns, git history, debugging solutions, anything derivable from code\n\
         - Memory is a hint — always verify against current state before acting on it\n",
    );

    // Detailed tool usage examples and workflow patterns.
    prompt.push_str(
        "# Tool usage patterns\n\n\
         Common patterns for effective tool use:\n\n\
         **Read before edit**: Always read a file before editing it. This ensures you \
         understand the current state and can make targeted changes.\n\
         ```\n\
         1. FileRead file_path → understand structure\n\
         2. FileEdit old_string, new_string → targeted change\n\
         ```\n\n\
         **Search then act**: Use Glob to find files, Grep to find content, then read/edit.\n\
         ```\n\
         1. Glob **/*.rs → find Rust files\n\
         2. Grep pattern path → find specific code\n\
         3. FileRead → read the match\n\
         4. FileEdit → make the change\n\
         ```\n\n\
         **Parallel tool calls**: When you need to read multiple independent files or run \
         independent searches, make all the tool calls in one response. Don't serialize \
         independent operations.\n\n\
         **Test after change**: After editing code, run tests to verify the change works.\n\
         ```\n\
         1. FileEdit → make change\n\
         2. Bash cargo test / pytest / npm test → verify\n\
         3. If tests fail, read the error, fix, re-test\n\
         ```\n\n\
         # Error recovery\n\n\
         When something goes wrong:\n\
         - **Tool not found**: Use ToolSearch to find the right tool name.\n\
         - **Permission denied**: Explain why the action is needed, ask the user to approve.\n\
         - **File not found**: Use Glob to find the correct path. Check for typos.\n\
         - **Edit failed (not unique)**: Provide more surrounding context in old_string, \
           or use replace_all=true if renaming.\n\
         - **Command failed**: Read the full error message. Don't retry the same command. \
           Diagnose the root cause first.\n\
         - **Context too large**: The system will auto-compact. If you need specific \
           information from before compaction, re-read the relevant files.\n\
         - **Rate limited**: The system will auto-retry with backoff. Just wait.\n\n\
         # Common workflows\n\n\
         **Bug fix**: Read the failing test → read the source code it tests → \
         identify the bug → fix it → run the test → confirm it passes.\n\n\
         **New feature**: Read existing patterns in the codebase → create or edit files → \
         add tests → run tests → update docs if needed.\n\n\
         **Code review**: Read the diff → identify issues (bugs, security, style) → \
         report findings with file:line references.\n\n\
         **Refactor**: Search for all usages of the symbol → plan the changes → \
         edit each file → run tests to verify nothing broke.\n\n",
    );

    // MCP server instructions (dynamic, per-server).
    if !state.config.mcp_servers.is_empty() {
        prompt.push_str("# MCP Servers\n\n");
        prompt.push_str(
            "Connected MCP servers provide additional tools. MCP tools are prefixed \
             with `mcp__{server}__{tool}`. Use them like any other tool.\n\n",
        );
        for (name, entry) in &state.config.mcp_servers {
            let transport = if entry.command.is_some() {
                "stdio"
            } else if entry.url.is_some() {
                "sse"
            } else {
                "unknown"
            };
            prompt.push_str(&format!("- **{name}** ({transport})\n"));
        }
        prompt.push('\n');
    }

    // Deferred tools listing.
    let deferred = tools.deferred_names();
    if !deferred.is_empty() {
        prompt.push_str("# Deferred Tools\n\n");
        prompt.push_str(
            "These tools are available but not loaded by default. \
             Use ToolSearch to load them when needed:\n",
        );
        for name in &deferred {
            prompt.push_str(&format!("- {name}\n"));
        }
        prompt.push('\n');
    }

    // Task management guidance.
    prompt.push_str(
        "# Task management\n\n\
         - Use TaskCreate to break complex work into trackable steps.\n\
         - Mark tasks as in_progress when starting, completed when done.\n\
         - Use the Agent tool to spawn subagents for parallel independent work.\n\
         - Use EnterPlanMode/ExitPlanMode for read-only exploration before making changes.\n\
         - Use EnterWorktree/ExitWorktree for isolated changes in git worktrees.\n\n\
         # Output formatting\n\n\
         - All text output is displayed to the user. Use GitHub-flavored markdown.\n\
         - Use fenced code blocks with language hints for code: ```rust, ```python, etc.\n\
         - Use inline `code` for file names, function names, and short code references.\n\
         - Use tables for structured comparisons.\n\
         - Use bullet lists for multiple items.\n\
         - Keep paragraphs short (2-3 sentences).\n\
         - Never output raw HTML or complex formatting — stick to standard markdown.\n",
    );

    prompt
}
