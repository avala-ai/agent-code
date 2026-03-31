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

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::LlmError;
use crate::hooks::{HookEvent, HookRegistry};
use crate::llm::client::{CompletionRequest, LlmClient};
use crate::llm::message::*;
use crate::llm::stream::StreamEvent;
use crate::permissions::PermissionChecker;
use crate::services::compact::{
    self, CompactTracking, MAX_OUTPUT_TOKENS_RECOVERY_LIMIT,
};
use crate::services::tokens;
use crate::state::AppState;
use crate::tools::executor::{execute_tool_calls, extract_tool_calls};
use crate::tools::registry::ToolRegistry;
use crate::tools::ToolContext;

/// Maximum consecutive rate-limit retries before giving up.
const MAX_RATE_LIMIT_RETRIES: u32 = 5;

/// Configuration for the query engine.
pub struct QueryEngineConfig {
    pub max_turns: Option<usize>,
    pub verbose: bool,
}

/// The query engine orchestrates the agent loop.
pub struct QueryEngine {
    llm: LlmClient,
    tools: ToolRegistry,
    permissions: Arc<PermissionChecker>,
    state: AppState,
    config: QueryEngineConfig,
    cancel: CancellationToken,
    hooks: HookRegistry,
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
        llm: LlmClient,
        tools: ToolRegistry,
        permissions: PermissionChecker,
        state: AppState,
        config: QueryEngineConfig,
    ) -> Self {
        Self {
            llm,
            tools,
            permissions: Arc::new(permissions),
            state,
            config,
            cancel: CancellationToken::new(),
            hooks: HookRegistry::new(),
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
        // Add the user message to history.
        let user_msg = user_message(user_input);
        self.state.push_message(user_msg);

        let max_turns = self.config.max_turns.unwrap_or(50);
        let mut compact_tracking = CompactTracking::default();
        let mut rate_limit_retries = 0u32;
        let mut max_output_recovery_count = 0u32;

        // Agent loop: compact → call LLM → execute tools → repeat.
        for turn in 0..max_turns {
            self.state.turn_count = turn + 1;
            self.state.is_query_active = true;

            debug!("Agent turn {}/{}", turn + 1, max_turns);

            let model = self.state.config.api.model.clone();

            // Step 1: Auto-compact if context is too large.
            if compact::should_auto_compact(
                self.state.history(),
                &model,
                &compact_tracking,
            ) {
                let token_count = tokens::estimate_context_tokens(self.state.history());
                let threshold = compact::auto_compact_threshold(&model);
                info!(
                    "Auto-compact triggered: {token_count} tokens >= {threshold} threshold"
                );

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
                        &self.llm,
                        &model,
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
                            // Fallback: aggressive microcompact.
                            let freed2 =
                                compact::microcompact(&mut self.state.messages, 2);
                            if freed2 > 0 {
                                sink.on_compact(freed2);
                            }
                        }
                    }
                }
            }

            // Step 2: Check token warning state.
            let warning = compact::token_warning_state(self.state.history(), &model);
            if warning.is_blocking {
                sink.on_warning("Context window nearly full. Consider starting a new session.");
            } else if warning.is_above_warning {
                sink.on_warning(&format!(
                    "Context {}% remaining",
                    warning.percent_left
                ));
            }

            // Step 3: Build and send the API request.
            let system_prompt = build_system_prompt(&self.tools, &self.state);
            let tool_schemas = self.tools.schemas();

            let request = CompletionRequest {
                messages: self.state.history(),
                system_prompt: &system_prompt,
                tools: &tool_schemas,
                max_tokens: self.state.config.api.max_output_tokens,
            };

            let mut rx = match self.llm.stream_completion(request).await {
                Ok(rx) => {
                    rate_limit_retries = 0; // Reset on success.
                    rx
                }
                Err(e) => {
                    match &e {
                        LlmError::RateLimited { retry_after_ms } => {
                            rate_limit_retries += 1;
                            if rate_limit_retries > MAX_RATE_LIMIT_RETRIES {
                                sink.on_error(&format!(
                                    "Rate limited {MAX_RATE_LIMIT_RETRIES} times, giving up"
                                ));
                                self.state.is_query_active = false;
                                return Err(e.into());
                            }
                            warn!(
                                "Rate limited (attempt {rate_limit_retries}/{MAX_RATE_LIMIT_RETRIES}), \
                                 waiting {retry_after_ms}ms"
                            );
                            tokio::time::sleep(
                                std::time::Duration::from_millis(*retry_after_ms),
                            )
                            .await;
                            continue;
                        }
                        LlmError::Api { status, body } if *status == 413 => {
                            // Prompt too long — try reactive compact.
                            warn!("Prompt too long (413), attempting reactive compact");
                            let gap = compact::parse_prompt_too_long_gap(body);
                            let freed = compact::microcompact(&mut self.state.messages, 1);
                            if freed > 0 {
                                sink.on_compact(freed);
                                info!(
                                    "Reactive microcompact freed ~{freed} tokens (gap: {:?})",
                                    gap
                                );
                                continue; // Retry with smaller context.
                            }
                            sink.on_error("Context too large and compaction failed");
                            self.state.is_query_active = false;
                            return Err(e.into());
                        }
                        _ => {
                            sink.on_error(&e.to_string());
                            self.state.is_query_active = false;
                            return Err(e.into());
                        }
                    }
                }
            };

            // Step 4: Accumulate content blocks from the stream.
            let mut content_blocks = Vec::new();
            let mut usage = Usage::default();
            let mut got_error = false;
            let mut error_text = String::new();

            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::TextDelta(text) => {
                        sink.on_text(&text);
                    }
                    StreamEvent::ContentBlockComplete(block) => {
                        if let ContentBlock::ToolUse {
                            ref name, ref input, ..
                        } = block
                        {
                            sink.on_tool_start(name, input);
                        }
                        if let ContentBlock::Thinking { ref thinking, .. } = block {
                            sink.on_thinking(thinking);
                        }
                        content_blocks.push(block);
                    }
                    StreamEvent::Done {
                        usage: u,
                        stop_reason: _,
                    } => {
                        usage = u;
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
                stop_reason: None,
                request_id: None,
            });
            self.state.push_message(assistant_msg);
            self.state.record_usage(&usage, &model);

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
                if content_blocks.iter().any(|b| matches!(b, ContentBlock::Text { .. }))
                    && error_text.contains("max_tokens")
                {
                    if max_output_recovery_count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT {
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
            }

            // Step 7: Extract tool calls from the response.
            let tool_calls = extract_tool_calls(&content_blocks);

            if tool_calls.is_empty() {
                // No tools requested — turn is complete.
                info!("Turn complete (no tool calls)");
                sink.on_turn_complete(turn + 1);
                self.state.is_query_active = false;
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
            };

            // Fire pre-tool-use hooks.
            for call in &tool_calls {
                self.hooks
                    .run_hooks(
                        &HookEvent::PreToolUse,
                        Some(&call.name),
                        &call.input,
                    )
                    .await;
            }

            let results = execute_tool_calls(
                &tool_calls,
                self.tools.all(),
                &tool_ctx,
                &self.permissions,
            )
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
    prompt.push_str(&format!(
        "# Environment\n\
         - Working directory: {}\n\
         - Platform: {}\n\
         - Shell: bash\n\n",
        state.cwd,
        std::env::consts::OS,
    ));

    // Inject memory context (project + user).
    let memory = crate::memory::MemoryContext::load(
        Some(std::path::Path::new(&state.cwd)),
    );
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

    // Guidelines.
    prompt.push_str(
        "# Guidelines\n\n\
         - Read files before editing them. Understand existing code before suggesting modifications.\n\
         - Prefer editing existing files over creating new ones.\n\
         - Use the appropriate dedicated tool instead of shell commands when possible:\n\
           - File search: use Glob (not find or ls)\n\
           - Content search: use Grep (not grep or rg)\n\
           - Read files: use FileRead (not cat/head/tail)\n\
           - Edit files: use FileEdit (not sed/awk)\n\
           - Write files: use FileWrite (not echo/cat)\n\
         - Be careful not to introduce security vulnerabilities.\n\
         - Only make changes that were requested. Don't add features or refactor beyond the ask.\n\
         - Keep responses concise. Lead with the answer, not the reasoning.\n\
         - When referencing code, include file_path:line_number.\n\
         - For git operations: prefer new commits over amending, never force-push without asking.\n",
    );

    prompt
}
