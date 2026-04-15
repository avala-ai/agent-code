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
///
/// Central coordinator that drives the LLM → tools → LLM cycle.
/// Manages conversation history, context compaction, tool execution,
/// error recovery, and hook dispatch. Create via [`QueryEngine::new`].
pub struct QueryEngine {
    llm: Arc<dyn Provider>,
    tools: ToolRegistry,
    file_cache: Arc<tokio::sync::Mutex<crate::services::file_cache::FileCache>>,
    permissions: Arc<PermissionChecker>,
    state: AppState,
    config: QueryEngineConfig,
    /// Shared handle so the signal handler always cancels the current token.
    cancel_shared: Arc<std::sync::Mutex<CancellationToken>>,
    /// Per-turn cancellation token (cloned from cancel_shared at turn start).
    cancel: CancellationToken,
    hooks: HookRegistry,
    cache_tracker: crate::services::cache_tracking::CacheTracker,
    denial_tracker: Arc<tokio::sync::Mutex<crate::permissions::tracking::DenialTracker>>,
    extraction_state: Arc<tokio::sync::Mutex<crate::memory::extraction::ExtractionState>>,
    session_allows: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    permission_prompter: Option<Arc<dyn crate::tools::PermissionPrompter>>,
    /// Cached system prompt (rebuilt only when inputs change).
    cached_system_prompt: Option<(u64, String)>, // (hash, prompt)
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
        let cancel = CancellationToken::new();
        let cancel_shared = Arc::new(std::sync::Mutex::new(cancel.clone()));
        Self {
            llm,
            tools,
            file_cache: Arc::new(tokio::sync::Mutex::new(
                crate::services::file_cache::FileCache::new(),
            )),
            permissions: Arc::new(permissions),
            state,
            config,
            cancel,
            cancel_shared,
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
            cached_system_prompt: None,
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
        let shared = self.cancel_shared.clone();
        tokio::spawn(async move {
            let mut pending = false;
            loop {
                if tokio::signal::ctrl_c().await.is_ok() {
                    let token = shared.lock().unwrap().clone();
                    if token.is_cancelled() && pending {
                        // Second Ctrl+C after cancel — hard exit.
                        std::process::exit(130);
                    }
                    token.cancel();
                    pending = true;
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
        // Reset cancellation token for this turn. The shared handle is
        // updated so the signal handler always cancels the current token.
        self.cancel = CancellationToken::new();
        *self.cancel_shared.lock().unwrap() = self.cancel.clone();

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

            let mut model = self.state.config.api.model.clone();

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
                    match compact::compact_with_llm(
                        &mut self.state.messages,
                        &*self.llm,
                        &model,
                        self.cancel.clone(),
                    )
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
            // Memoize: only rebuild system prompt when inputs change.
            let prompt_hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                self.state.config.api.model.hash(&mut h);
                self.state.cwd.hash(&mut h);
                self.state.config.mcp_servers.len().hash(&mut h);
                self.tools.all().len().hash(&mut h);
                h.finish()
            };
            let system_prompt = if let Some((cached_hash, ref cached)) = self.cached_system_prompt
                && cached_hash == prompt_hash
            {
                cached.clone()
            } else {
                let prompt = build_system_prompt(&self.tools, &self.state);
                self.cached_system_prompt = Some((prompt_hash, prompt.clone()));
                prompt
            };
            // Use core schemas (deferred tools loaded on demand via ToolSearch).
            let tool_schemas = self.tools.core_schemas();

            // Escalate max_tokens after a max_output recovery (8k → 64k).
            let base_tokens = self.state.config.api.max_output_tokens.unwrap_or(16384);
            let effective_tokens = if max_output_recovery_count > 0 {
                base_tokens.max(65536) // Escalate to at least 64k after first recovery
            } else {
                base_tokens
            };

            let request = ProviderRequest {
                messages: self.state.history().to_vec(),
                system_prompt: system_prompt.clone(),
                tools: tool_schemas.clone(),
                model: model.clone(),
                max_tokens: effective_tokens,
                temperature: None,
                enable_caching: self.state.config.features.prompt_caching,
                tool_choice: Default::default(),
                metadata: None,
                cancel: self.cancel.clone(),
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
                            // Switch to a smaller/cheaper model for this turn.
                            let fallback = get_fallback_model(&model);
                            sink.on_warning(&format!("Falling back from {model} to {fallback}"));
                            model = fallback;
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
                            // Two-stage recovery: context collapse first, then microcompact.
                            if let ProviderError::RequestTooLarge(body) = &e {
                                let gap = compact::parse_prompt_too_long_gap(body);

                                // Stage 1: Context collapse (snip middle messages).
                                let effective = compact::effective_context_window(&model);
                                if let Some(collapse) =
                                    crate::services::context_collapse::collapse_to_budget(
                                        self.state.history(),
                                        effective,
                                    )
                                {
                                    info!(
                                        "Reactive collapse: snipped {} messages, freed ~{} tokens",
                                        collapse.snipped_count, collapse.tokens_freed
                                    );
                                    self.state.messages = collapse.api_messages;
                                    sink.on_compact(collapse.tokens_freed);
                                    continue;
                                }

                                // Stage 2: Aggressive microcompact.
                                let freed = compact::microcompact(&mut self.state.messages, 1);
                                if freed > 0 {
                                    sink.on_compact(freed);
                                    info!(
                                        "Reactive microcompact freed ~{freed} tokens (gap: {gap:?})"
                                    );
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

            // Step 4: Stream response. Start executing read-only tools
            // as their input completes (streaming tool execution).
            let mut content_blocks = Vec::new();
            let mut usage = Usage::default();
            let mut stop_reason: Option<StopReason> = None;
            let mut got_error = false;
            let mut error_text = String::new();

            // Streaming tool handles: tools kicked off during streaming.
            let mut streaming_tool_handles: Vec<(
                String,
                String,
                tokio::task::JoinHandle<crate::tools::ToolResult>,
            )> = Vec::new();

            let mut cancelled = false;
            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Some(StreamEvent::TextDelta(text)) => {
                                sink.on_text(&text);
                            }
                            Some(StreamEvent::ContentBlockComplete(block)) => {
                                if let ContentBlock::ToolUse {
                                    ref id,
                                    ref name,
                                    ref input,
                                } = block
                                {
                                    sink.on_tool_start(name, input);

                                    // Start read-only tools immediately during streaming.
                                    if let Some(tool) = self.tools.get(name)
                                        && tool.is_read_only()
                                        && tool.is_concurrency_safe()
                                    {
                                        let tool = tool.clone();
                                        let input = input.clone();
                                        let cwd = std::path::PathBuf::from(&self.state.cwd);
                                        let cancel = self.cancel.clone();
                                        let perm = self.permissions.clone();
                                        let tool_id = id.clone();
                                        let tool_name = name.clone();

                                        let handle = tokio::spawn(async move {
                                            match tool
                                                .call(
                                                    input,
                                                    &ToolContext {
                                                        cwd,
                                                        cancel,
                                                        permission_checker: perm.clone(),
                                                        verbose: false,
                                                        plan_mode: false,
                                                        file_cache: None,
                                                        denial_tracker: None,
                                                        task_manager: None,
                                                        session_allows: None,
                                                        permission_prompter: None,
                                                        sandbox: None,
                                                    },
                                                )
                                                .await
                                            {
                                                Ok(r) => r,
                                                Err(e) => crate::tools::ToolResult::error(e.to_string()),
                                            }
                                        });

                                        streaming_tool_handles.push((tool_id, tool_name, handle));
                                    }
                                }
                                if let ContentBlock::Thinking { ref thinking, .. } = block {
                                    sink.on_thinking(thinking);
                                }
                                content_blocks.push(block);
                            }
                            Some(StreamEvent::Done {
                                usage: u,
                                stop_reason: sr,
                            }) => {
                                usage = u;
                                stop_reason = sr;
                                sink.on_usage(&usage);
                            }
                            Some(StreamEvent::Error(msg)) => {
                                got_error = true;
                                error_text = msg.clone();
                                sink.on_error(&msg);
                            }
                            Some(_) => {}
                            None => break,
                        }
                    }
                    _ = self.cancel.cancelled() => {
                        warn!("Turn cancelled by user");
                        cancelled = true;
                        // Abort any in-flight streaming tool handles.
                        for (_, _, handle) in streaming_tool_handles.drain(..) {
                            handle.abort();
                        }
                        break;
                    }
                }
            }

            if cancelled {
                sink.on_warning("Cancelled");
                self.state.is_query_active = false;
                return Ok(());
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
                cwd: cwd.clone(),
                cancel: self.cancel.clone(),
                permission_checker: self.permissions.clone(),
                verbose: self.config.verbose,
                plan_mode: self.state.plan_mode,
                file_cache: Some(self.file_cache.clone()),
                denial_tracker: Some(self.denial_tracker.clone()),
                task_manager: Some(self.state.task_manager.clone()),
                session_allows: Some(self.session_allows.clone()),
                permission_prompter: self.permission_prompter.clone(),
                sandbox: Some(std::sync::Arc::new(
                    crate::sandbox::SandboxExecutor::from_session_config(&self.state.config, &cwd),
                )),
            };

            // Collect streaming tool results first.
            let streaming_ids: std::collections::HashSet<String> = streaming_tool_handles
                .iter()
                .map(|(id, _, _)| id.clone())
                .collect();

            let mut streaming_results = Vec::new();
            for (id, name, handle) in streaming_tool_handles.drain(..) {
                match handle.await {
                    Ok(result) => streaming_results.push(crate::tools::executor::ToolCallResult {
                        tool_use_id: id,
                        tool_name: name,
                        result,
                    }),
                    Err(e) => streaming_results.push(crate::tools::executor::ToolCallResult {
                        tool_use_id: id,
                        tool_name: name,
                        result: crate::tools::ToolResult::error(format!("Task failed: {e}")),
                    }),
                }
            }

            // Fire pre-tool-use hooks.
            for call in &tool_calls {
                self.hooks
                    .run_hooks(&HookEvent::PreToolUse, Some(&call.name), &call.input)
                    .await;
            }

            // Execute remaining tools (ones not started during streaming).
            let remaining_calls: Vec<_> = tool_calls
                .iter()
                .filter(|c| !streaming_ids.contains(&c.id))
                .cloned()
                .collect();

            let mut results = streaming_results;
            if !remaining_calls.is_empty() {
                let batch_results = execute_tool_calls(
                    &remaining_calls,
                    self.tools.all(),
                    &tool_ctx,
                    &self.permissions,
                )
                .await;
                results.extend(batch_results);
            }

            // Step 9: Inject tool results + fire post-tool-use hooks.
            for result in &results {
                // Handle plan mode state transitions.
                if !result.result.is_error {
                    match result.tool_name.as_str() {
                        "EnterPlanMode" => {
                            self.state.plan_mode = true;
                            info!("Plan mode enabled");
                        }
                        "ExitPlanMode" => {
                            self.state.plan_mode = false;
                            info!("Plan mode disabled");
                        }
                        _ => {}
                    }
                }

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

/// Get a fallback model (smaller/cheaper) for retry on overload.
fn get_fallback_model(current: &str) -> String {
    let lower = current.to_lowercase();
    if lower.contains("opus") {
        // Opus → Sonnet
        current.replace("opus", "sonnet")
    } else if (lower.contains("gpt-5.4") || lower.contains("gpt-4.1"))
        && !lower.contains("mini")
        && !lower.contains("nano")
    {
        format!("{current}-mini")
    } else if lower.contains("large") {
        current.replace("large", "small")
    } else {
        // Already a small model, keep it.
        current.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that cancelling via the shared handle cancels the current
    /// turn's token (regression: the signal handler previously held a
    /// stale clone that couldn't cancel subsequent turns).
    #[test]
    fn cancel_shared_propagates_to_current_token() {
        let root = CancellationToken::new();
        let shared = Arc::new(std::sync::Mutex::new(root.clone()));

        // Simulate turn reset: create a new token and update the shared handle.
        let turn1 = CancellationToken::new();
        *shared.lock().unwrap() = turn1.clone();

        // Cancelling via the shared handle should cancel turn1.
        shared.lock().unwrap().cancel();
        assert!(turn1.is_cancelled());

        // New turn: replace the token. The old cancellation shouldn't affect it.
        let turn2 = CancellationToken::new();
        *shared.lock().unwrap() = turn2.clone();
        assert!(!turn2.is_cancelled());

        // Cancelling via shared should cancel turn2.
        shared.lock().unwrap().cancel();
        assert!(turn2.is_cancelled());
    }

    /// Verify that the streaming loop breaks on cancellation by simulating
    /// the select pattern used in run_turn_with_sink.
    #[tokio::test]
    async fn stream_loop_responds_to_cancellation() {
        let cancel = CancellationToken::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(10);

        // Simulate a slow stream: send one event, then cancel before more arrive.
        tx.send(StreamEvent::TextDelta("hello".into()))
            .await
            .unwrap();

        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            // Small delay, then cancel.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel2.cancel();
        });

        let mut events_received = 0u32;
        let mut cancelled = false;

        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Some(_) => events_received += 1,
                        None => break,
                    }
                }
                _ = cancel.cancelled() => {
                    cancelled = true;
                    break;
                }
            }
        }

        assert!(cancelled, "Loop should have been cancelled");
        assert_eq!(
            events_received, 1,
            "Should have received exactly one event before cancel"
        );
    }

    // ------------------------------------------------------------------
    // End-to-end regression tests for #103.
    //
    // These tests build a real QueryEngine with a mock Provider and
    // exercise run_turn_with_sink directly, verifying that cancellation
    // actually interrupts the streaming loop (not just the select!
    // pattern in isolation).
    // ------------------------------------------------------------------

    use crate::llm::provider::{Provider, ProviderError, ProviderRequest};

    /// A provider whose stream yields one TextDelta and then hangs forever.
    /// Simulates the real bug: a slow LLM response the user wants to interrupt.
    struct HangingProvider;

    #[async_trait::async_trait]
    impl Provider for HangingProvider {
        fn name(&self) -> &str {
            "hanging-mock"
        }

        async fn stream(
            &self,
            _request: &ProviderRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, ProviderError> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::TextDelta("thinking...".into())).await;
                // Hang forever without closing the channel or sending Done.
                let _tx_holder = tx;
                std::future::pending::<()>().await;
            });
            Ok(rx)
        }
    }

    /// A provider whose spawned streaming task honors `ProviderRequest::cancel`.
    /// When it exits cleanly via cancellation it flips `exit_flag` to true.
    /// Used to prove that the cancel token reaches the provider's own task
    /// (the anthropic/openai/azure SSE loops), not just the query engine's
    /// outer select.
    struct CancelAwareHangingProvider {
        exit_flag: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Provider for CancelAwareHangingProvider {
        fn name(&self) -> &str {
            "cancel-aware-mock"
        }

        async fn stream(
            &self,
            request: &ProviderRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, ProviderError> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let cancel = request.cancel.clone();
            let exit_flag = self.exit_flag.clone();
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::TextDelta("thinking...".into())).await;
                // Mirror the real provider pattern: race the "next SSE chunk"
                // future against the cancel token. A real provider's
                // `byte_stream.next().await` would sit where `pending` does.
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        exit_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    _ = std::future::pending::<()>() => unreachable!(),
                }
            });
            Ok(rx)
        }
    }

    /// A provider that completes a turn normally: emits text and a Done event.
    struct CompletingProvider;

    #[async_trait::async_trait]
    impl Provider for CompletingProvider {
        fn name(&self) -> &str {
            "completing-mock"
        }

        async fn stream(
            &self,
            _request: &ProviderRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, ProviderError> {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::TextDelta("hello".into())).await;
                let _ = tx
                    .send(StreamEvent::ContentBlockComplete(ContentBlock::Text {
                        text: "hello".into(),
                    }))
                    .await;
                let _ = tx
                    .send(StreamEvent::Done {
                        usage: Usage::default(),
                        stop_reason: Some(StopReason::EndTurn),
                    })
                    .await;
                // Channel closes when tx drops.
            });
            Ok(rx)
        }
    }

    fn build_engine(llm: Arc<dyn Provider>) -> QueryEngine {
        use crate::config::Config;
        use crate::permissions::PermissionChecker;
        use crate::state::AppState;
        use crate::tools::registry::ToolRegistry;

        let config = Config::default();
        let permissions = PermissionChecker::from_config(&config.permissions);
        let state = AppState::new(config);

        QueryEngine::new(
            llm,
            ToolRegistry::default_tools(),
            permissions,
            state,
            QueryEngineConfig {
                max_turns: Some(1),
                verbose: false,
                unattended: true,
            },
        )
    }

    /// Schedule a cancellation after `delay_ms` via the shared handle
    /// (same path the signal handler uses).
    fn schedule_cancel(engine: &QueryEngine, delay_ms: u64) {
        let shared = engine.cancel_shared.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            shared.lock().unwrap().cancel();
        });
    }

    /// Builds a mock provider whose stream yields one TextDelta and then hangs.
    /// Verifies the turn returns promptly once cancel fires.
    #[tokio::test]
    async fn run_turn_with_sink_interrupts_on_cancel() {
        let mut engine = build_engine(Arc::new(HangingProvider));
        schedule_cancel(&engine, 100);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn_with_sink("test input", &NullSink),
        )
        .await;

        assert!(
            result.is_ok(),
            "run_turn_with_sink should return promptly on cancel, not hang"
        );
        assert!(
            result.unwrap().is_ok(),
            "cancelled turn should return Ok(()), not an error"
        );
        assert!(
            !engine.state().is_query_active,
            "is_query_active should be reset after cancel"
        );
    }

    /// Regression test for the original #103 bug: the signal handler held
    /// a stale clone of the cancellation token, so Ctrl+C only worked on
    /// the *first* turn. This test cancels turn 1, then runs turn 2 and
    /// verifies it is ALSO cancellable via the same shared handle.
    #[tokio::test]
    async fn cancel_works_across_multiple_turns() {
        let mut engine = build_engine(Arc::new(HangingProvider));

        // Turn 1: cancel mid-stream.
        schedule_cancel(&engine, 80);
        let r1 = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn_with_sink("turn 1", &NullSink),
        )
        .await;
        assert!(r1.is_ok(), "turn 1 should cancel promptly");
        assert!(!engine.state().is_query_active);

        // Turn 2: cancel again via the same shared handle.
        // With the pre-fix stale-token bug, the handle would be pointing
        // at turn 1's already-used token and turn 2 would hang forever.
        schedule_cancel(&engine, 80);
        let r2 = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn_with_sink("turn 2", &NullSink),
        )
        .await;
        assert!(
            r2.is_ok(),
            "turn 2 should also cancel promptly — regression would hang here"
        );
        assert!(!engine.state().is_query_active);

        // Turn 3: one more for good measure.
        schedule_cancel(&engine, 80);
        let r3 = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn_with_sink("turn 3", &NullSink),
        )
        .await;
        assert!(r3.is_ok(), "turn 3 should still be cancellable");
        assert!(!engine.state().is_query_active);
    }

    /// Verifies that a previously-cancelled token does not poison subsequent
    /// turns. A fresh run_turn_with_sink on the same engine should complete
    /// normally even after a prior cancel.
    #[tokio::test]
    async fn cancel_does_not_poison_next_turn() {
        // Turn 1: hangs and gets cancelled.
        let mut engine = build_engine(Arc::new(HangingProvider));
        schedule_cancel(&engine, 80);
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn_with_sink("turn 1", &NullSink),
        )
        .await
        .expect("turn 1 should cancel");

        // Swap the provider to one that completes normally by rebuilding
        // the engine (we can't swap llm on an existing engine, so this
        // simulates the isolated "fresh turn" behavior). The key property
        // being tested is that the per-turn cancel reset correctly
        // initializes a non-cancelled token.
        let mut engine2 = build_engine(Arc::new(CompletingProvider));

        // Pre-cancel engine2 to simulate a leftover cancelled state, then
        // verify run_turn_with_sink still works because it resets the token.
        engine2.cancel_shared.lock().unwrap().cancel();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine2.run_turn_with_sink("hello", &NullSink),
        )
        .await;

        assert!(result.is_ok(), "completing turn should not hang");
        assert!(
            result.unwrap().is_ok(),
            "turn should succeed — the stale cancel flag must be cleared on turn start"
        );
        // Message history should contain the user + assistant messages.
        assert!(
            engine2.state().messages.len() >= 2,
            "normal turn should push both user and assistant messages"
        );
    }

    /// Verifies that cancelling BEFORE any event arrives still interrupts
    /// the turn cleanly (edge case: cancellation races with the first recv).
    #[tokio::test]
    async fn cancel_before_first_event_interrupts_cleanly() {
        let mut engine = build_engine(Arc::new(HangingProvider));
        // Very short delay so cancel likely fires before or during the
        // first event. The test is tolerant of either ordering.
        schedule_cancel(&engine, 1);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn_with_sink("immediate", &NullSink),
        )
        .await;

        assert!(result.is_ok(), "early cancel should not hang");
        assert!(result.unwrap().is_ok());
        assert!(!engine.state().is_query_active);
    }

    /// Verifies the sink receives cancellation feedback via on_warning.
    #[tokio::test]
    async fn cancelled_turn_emits_warning_to_sink() {
        use std::sync::Mutex;

        /// Captures sink events for assertion.
        struct CapturingSink {
            warnings: Mutex<Vec<String>>,
        }

        impl StreamSink for CapturingSink {
            fn on_text(&self, _: &str) {}
            fn on_tool_start(&self, _: &str, _: &serde_json::Value) {}
            fn on_tool_result(&self, _: &str, _: &crate::tools::ToolResult) {}
            fn on_error(&self, _: &str) {}
            fn on_warning(&self, msg: &str) {
                self.warnings.lock().unwrap().push(msg.to_string());
            }
        }

        let sink = CapturingSink {
            warnings: Mutex::new(Vec::new()),
        };

        let mut engine = build_engine(Arc::new(HangingProvider));
        schedule_cancel(&engine, 100);

        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn_with_sink("test", &sink),
        )
        .await
        .expect("should not hang");

        let warnings = sink.warnings.lock().unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("Cancelled")),
            "expected 'Cancelled' warning in sink, got: {:?}",
            *warnings
        );
    }

    /// Regression test for the #125-followup: the cancellation token must
    /// reach the *provider's own spawned streaming task*, not just the query
    /// engine's outer select loop. This is what was broken — the existing
    /// `HangingProvider` test above passed even while Escape-interrupt was
    /// completely dead in production, because that provider ignores the
    /// token and the query loop exits cleanly on its own. This test fails
    /// if `ProviderRequest::cancel` is dropped on the floor anywhere between
    /// `query::mod.rs` and the provider's spawn.
    #[tokio::test]
    async fn provider_stream_task_observes_cancellation() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let exit_flag = Arc::new(AtomicBool::new(false));
        let provider = Arc::new(CancelAwareHangingProvider {
            exit_flag: exit_flag.clone(),
        });
        let mut engine = build_engine(provider);
        schedule_cancel(&engine, 50);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            engine.run_turn_with_sink("test input", &NullSink),
        )
        .await;
        assert!(result.is_ok(), "engine should exit promptly on cancel");

        // Give the provider's spawned task a moment to observe the cancel
        // after the engine returns. In release builds this is effectively
        // instantaneous; the sleep just makes the test tolerant of scheduler
        // variance on slow CI runners.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(
            exit_flag.load(Ordering::SeqCst),
            "provider's streaming task should have observed cancel via \
             ProviderRequest::cancel and exited; if this flag is false, \
             the token is being dropped somewhere in query::mod.rs or the \
             provider is ignoring it"
        );
    }
}
