//! Tool executor: manages concurrent and serial tool execution.
//!
//! The executor partitions tool calls into batches:
//! - Read-only (concurrency-safe) tools run in parallel
//! - Mutation tools run serially
//!
//! This mirrors the streaming tool executor pattern where tools
//! begin execution as soon as their input is fully parsed from
//! the stream, maximizing throughput.

use std::sync::Arc;

use crate::llm::message::ContentBlock;
use crate::permissions::{PermissionChecker, PermissionDecision};

use super::{Tool, ToolContext, ToolResult};

/// A pending tool call extracted from the model's response.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Result of executing a tool call.
#[derive(Debug)]
pub struct ToolCallResult {
    pub tool_use_id: String,
    pub tool_name: String,
    pub result: ToolResult,
}

impl ToolCallResult {
    /// Convert to a content block for sending back to the API.
    pub fn to_content_block(&self) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: self.tool_use_id.clone(),
            content: self.result.content.clone(),
            is_error: self.result.is_error,
            extra_content: vec![],
        }
    }
}

/// Extract pending tool calls from assistant content blocks.
pub fn extract_tool_calls(content: &[ContentBlock]) -> Vec<PendingToolCall> {
    content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse { id, name, input } = block {
                Some(PendingToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Execute a batch of tool calls, respecting concurrency constraints.
///
/// Tools that are concurrency-safe run in parallel. Other tools run
/// serially. Results are returned in the same order as the input.
pub async fn execute_tool_calls(
    calls: &[PendingToolCall],
    tools: &[Arc<dyn Tool>],
    ctx: &ToolContext,
    permission_checker: &PermissionChecker,
) -> Vec<ToolCallResult> {
    // Partition into concurrent and serial batches.
    let mut results = Vec::with_capacity(calls.len());

    // Group consecutive concurrency-safe calls together.
    let mut i = 0;
    while i < calls.len() {
        let call = &calls[i];
        let tool = tools.iter().find(|t| t.name() == call.name);

        match tool {
            None => {
                results.push(ToolCallResult {
                    tool_use_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    result: ToolResult::error(format!("Tool '{}' not found", call.name)),
                });
                i += 1;
            }
            Some(tool) => {
                if tool.is_concurrency_safe() {
                    // Collect consecutive concurrency-safe calls.
                    let batch_start = i;
                    while i < calls.len() {
                        let t = tools.iter().find(|t| t.name() == calls[i].name);
                        if t.is_some_and(|t| t.is_concurrency_safe()) {
                            i += 1;
                        } else {
                            break;
                        }
                    }

                    // Execute batch concurrently.
                    let batch = &calls[batch_start..i];
                    let mut handles = Vec::new();

                    for call in batch {
                        let tool = tools
                            .iter()
                            .find(|t| t.name() == call.name)
                            .unwrap()
                            .clone();
                        let call = call.clone();
                        let ctx_cwd = ctx.cwd.clone();
                        let ctx_cancel = ctx.cancel.clone();
                        let ctx_verbose = ctx.verbose;
                        let perm_checker = ctx.permission_checker.clone();

                        let ctx_plan_mode = ctx.plan_mode;
                        let ctx_file_cache = ctx.file_cache.clone();
                        // Read-only tools still go through permission checks.
                        handles.push(tokio::spawn(async move {
                            execute_single_tool(
                                &call,
                                &*tool,
                                &ToolContext {
                                    cwd: ctx_cwd,
                                    cancel: ctx_cancel,
                                    permission_checker: perm_checker.clone(),
                                    verbose: ctx_verbose,
                                    plan_mode: ctx_plan_mode,
                                    file_cache: ctx_file_cache,
                                    denial_tracker: None,
                                    task_manager: None,
                                    session_allows: None,
                                },
                                &perm_checker,
                            )
                            .await
                        }));
                    }

                    for handle in handles {
                        match handle.await {
                            Ok(result) => results.push(result),
                            Err(e) => {
                                results.push(ToolCallResult {
                                    tool_use_id: String::new(),
                                    tool_name: String::new(),
                                    result: ToolResult::error(format!("Task join error: {e}")),
                                });
                            }
                        }
                    }
                } else {
                    // Execute serially.
                    let result = execute_single_tool(call, &**tool, ctx, permission_checker).await;
                    results.push(result);
                    i += 1;
                }
            }
        }
    }

    results
}

/// Execute a single tool call with permission checking.
async fn execute_single_tool(
    call: &PendingToolCall,
    tool: &dyn Tool,
    ctx: &ToolContext,
    permission_checker: &PermissionChecker,
) -> ToolCallResult {
    // Block non-read-only tools in plan mode.
    if ctx.plan_mode && !tool.is_read_only() {
        return ToolCallResult {
            tool_use_id: call.id.clone(),
            tool_name: call.name.clone(),
            result: ToolResult::error(
                "Plan mode active: only read-only tools are allowed. \
                 Use ExitPlanMode to enable mutations."
                    .to_string(),
            ),
        };
    }

    // Check permissions.
    let decision = tool
        .check_permissions(&call.input, permission_checker)
        .await;
    match decision {
        PermissionDecision::Allow => {}
        PermissionDecision::Deny(reason) => {
            if let Some(ref tracker) = ctx.denial_tracker {
                tracker
                    .lock()
                    .await
                    .record(&call.name, &call.id, &reason, &call.input);
            }
            return ToolCallResult {
                tool_use_id: call.id.clone(),
                tool_name: call.name.clone(),
                result: ToolResult::error(format!("Permission denied: {reason}")),
            };
        }
        PermissionDecision::Ask(prompt) => {
            // Check session-level allows first (user previously chose "Allow for session").
            if let Some(ref allows) = ctx.session_allows
                && allows.lock().await.contains(call.name.as_str())
            {
                // Already allowed for this session — skip prompt.
            } else {
                // Prompt the user for permission with detailed TUI modal.
                let description = format!("{}: {}", call.name, prompt);
                let input_preview = serde_json::to_string_pretty(&call.input).ok();
                let response = crate::ui::prompt::ask_permission_detailed(
                    &call.name,
                    &description,
                    input_preview.as_deref(),
                );

                match response {
                    crate::ui::prompt::PermissionResponse::AllowOnce => {
                        // Continue to execution.
                    }
                    crate::ui::prompt::PermissionResponse::AllowSession => {
                        // Record session-level allow so future calls skip the prompt.
                        if let Some(ref allows) = ctx.session_allows {
                            allows.lock().await.insert(call.name.clone());
                        }
                    }
                    crate::ui::prompt::PermissionResponse::Deny => {
                        if let Some(ref tracker) = ctx.denial_tracker {
                            tracker.lock().await.record(
                                &call.name,
                                &call.id,
                                "user denied",
                                &call.input,
                            );
                        }
                        return ToolCallResult {
                            tool_use_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            result: ToolResult::error("Permission denied by user".to_string()),
                        };
                    }
                }
            } // close else block
        }
    }

    // Validate input.
    if let Err(msg) = tool.validate_input(&call.input) {
        return ToolCallResult {
            tool_use_id: call.id.clone(),
            tool_name: call.name.clone(),
            result: ToolResult::error(format!("Invalid input: {msg}")),
        };
    }

    // Execute.
    match tool.call(call.input.clone(), ctx).await {
        Ok(mut result) => {
            // Persist large outputs to disk, replace with truncated + path reference.
            result.content = crate::services::output_store::persist_if_large(
                &result.content,
                tool.name(),
                &call.id,
            );

            // Additional truncation if still over the tool's limit.
            let max = tool.max_result_size_chars();
            if result.content.len() > max {
                result.content.truncate(max);
                result.content.push_str("\n\n(output truncated)");
            }
            ToolCallResult {
                tool_use_id: call.id.clone(),
                tool_name: call.name.clone(),
                result,
            }
        }
        Err(e) => ToolCallResult {
            tool_use_id: call.id.clone(),
            tool_name: call.name.clone(),
            result: ToolResult::error(e.to_string()),
        },
    }
}
