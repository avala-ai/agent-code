# Architecture

This document describes how rs-code is organized and how the major subsystems interact.

## Overview

rs-code is a terminal-based AI coding agent. The user types a request, the agent calls an LLM, the LLM responds with text and tool calls, the agent executes the tools, feeds results back, and repeats until the task is done.

```
User input
    │
    ▼
┌──────────┐     ┌───────────┐     ┌───────────┐
│  REPL    │────▶│  Query    │────▶│  LLM API  │
│  (ui/)   │     │  Engine   │     │  (llm/)   │
└──────────┘     │  (query/) │◀────└───────────┘
                 └─────┬─────���
                       │ tool calls
                 ┌─────▼─────┐
                 │  Tools    │
                 │  (tools/) │
                 └──────────��┘
```

## Directory Structure

```
src/
├── main.rs              Entry point, CLI parsing, initialization
├── error.rs             Unified error types (LlmError, ToolError, etc.)
│
├── config/              Configuration loading
│   ├── mod.rs           Layered config: user → project → env → CLI
│   └── schema.rs        Config struct definitions (ApiConfig, Permissions, etc.)
│
├── llm/                 LLM communication
│   ├── client.rs        HTTP streaming client with caching and retries
│   ├── message.rs       Message types (User, Assistant, System, ContentBlock)
│   ├── normalize.rs     Message validation (tool result pairing, alternation)
│   ├── retry.rs         Retry state machine with fallback model support
│   └── stream.rs        SSE parser that accumulates content blocks
│
├── query/               Agent loop
│   ├── mod.rs           Core loop: compact → call LLM → execute tools → repeat
│   └── source.rs        Query source tagging for cost attribution
│
├── tools/               Tool implementations
│   ├── mod.rs           Tool trait definition
│   ├── registry.rs      Tool collection and lookup
│   ├── executor.rs      Concurrent/serial tool batching
│   ├── streaming_executor.rs  Execute tools during streaming
│   ├── mcp_proxy.rs     Bridge MCP server tools into the local pool
│   ├── bash.rs          Shell command execution
│   ├── file_read.rs     File reading with binary detection
│   ├── file_write.rs    File creation/overwrite
│   ├── file_edit.rs     Search-and-replace editing
│   ├── grep.rs          Regex search via ripgrep
│   ├── glob.rs          File pattern matching
│   ├── agent.rs         Subagent spawning with worktree isolation
│   ├── web_fetch.rs     HTTP GET with HTML stripping
│   ├── web_search.rs    Web search with result extraction
│   ├── lsp_tool.rs      Language server diagnostics
│   ├── notebook_edit.rs Jupyter notebook editing
│   ├── ask_user.rs      Interactive prompts
│   ├── tool_search.rs   Tool discovery by keyword
│   ├── send_message.rs  Inter-agent communication
│   ├── plan_mode.rs     Read-only mode toggle
│   ├── worktree.rs      Git worktree management
│   ├── tasks.rs         Progress tracking
│   ├── todo_write.rs    Todo list management
│   └── sleep_tool.rs    Async pause
│
├── permissions/         Permission system
│   ├── mod.rs           Rule matching, glob patterns, mode enforcement
│   └── tracking.rs      Denial tracking for reporting
│
├��─ services/            Cross-cutting services
│   ├── tokens.rs        Token estimation (hybrid: API counts + heuristic)
│   ├── compact.rs       History compaction (micro, LLM, auto-trigger)
│   ├── context_collapse.rs  Non-destructive history snipping
│   ├── budget.rs        Cost and token budget enforcement
│   ├── cache_tracking.rs    Prompt cache hit/miss monitoring
│   ├── file_cache.rs    In-memory file content cache (50MB LRU)
│   ├── session.rs       Session save/load/list
│   ├── session_env.rs   Environment detection at startup
│   ├── git.rs           Git operations and diff parsing
│   ├── background.rs    Async task execution
│   ├── coordinator.rs   Multi-agent type definitions
│   ├── diagnostics.rs   Environment health checks
│   ├── telemetry.rs     Structured observability attributes
│   ├── plugins.rs       Plugin loading from TOML manifests
│   ├── bridge.rs        IDE bridge protocol and lock files
│   ├── lsp.rs           Language Server Protocol client
│   └── mcp/             Model Context Protocol
│       ├── client.rs    High-level MCP client
│       ├── transport.rs Stdio and SSE transports
│       └── types.rs     JSON-RPC and MCP type definitions
│
├── commands/            Slash command system
│   └── mod.rs           26 built-in commands + skill routing
│
├── hooks/               Lifecycle hooks
│   └── mod.rs           Pre/post tool use, session events
│
├��─ skills/              Custom workflow loading
│   └── mod.rs           Frontmatter parsing, template expansion
│
├── memory/              Persistent context
│   └── mod.rs           Project + user memory loading and injection
│
├── state/               Session state
│   └── mod.rs           AppState: messages, usage, cost, plan mode
│
└── ui/                  Terminal interface
    ├── repl.rs          Interactive readline loop with streaming output
    ├── render.rs        Markdown rendering with syntax highlighting
    ├── activity.rs      Animated status indicators
    ├── keymap.rs        Vi/Emacs mode detection
    └── keybindings.rs   Customizable keyboard shortcuts
```

## Key Design Decisions

**Single crate, not a workspace.** The project is one binary with well-separated modules. A workspace adds complexity that isn't justified at this scale. If a module needs to be extracted as a library later (e.g., the MCP client), that refactor is straightforward.

**Trait objects for tools (`Arc<dyn Tool>`).** Adding a tool means implementing the trait and registering it. No central enum to modify. The dynamic dispatch cost is negligible compared to I/O and LLM latency.

**Async everywhere with tokio.** All tool execution, API calls, and I/O are async. The `select!` macro handles timeout and cancellation. `CancellationToken` propagates Ctrl+C through the tool chain.

**Layered configuration.** User settings, project settings, CLI flags, and environment variables merge with clear priority. No surprises about which value wins.

**Permission checks before every tool call.** The executor checks permissions, validates input, and enforces plan mode before any tool's `call()` method runs. Read-only tools skip the ask prompt by default.

**Compaction as a first-class concern.** Long sessions will exceed the context window. The system has three compaction strategies (microcompact stale results, LLM-based summarization, context collapse) that activate automatically based on token estimates.

## Data Flow

### A Single Turn

1. User types a message in the REPL
2. Message is appended to conversation history
3. Budget check: stop if cost or token limit exceeded
4. Message normalization: pair orphaned tool results, merge consecutive user messages
5. Auto-compact check: if tokens exceed threshold, run micro/LLM/collapse compaction
6. Build system prompt (tools, environment, memory, guidelines)
7. Send to LLM API via streaming SSE
8. Accumulate response: text deltas displayed in real-time, content blocks collected
9. If response contains tool_use blocks:
   a. Fire pre-tool-use hooks
   b. Execute tools (concurrent batch for read-only, serial for mutations)
   c. Fire post-tool-use hooks
   d. Inject tool results into history
   e. Go to step 3
10. If no tool_use blocks: turn is complete

### Error Recovery

- **Rate limited (429):** Wait retry_after_ms, retry up to 5 times
- **Overloaded (529):** Exponential backoff, fall back to smaller model after 3 attempts
- **Prompt too long (413):** Reactive microcompact, then context collapse
- **Max output tokens:** Inject continuation message, retry up to 3 times
- **Stream interrupted:** Retry with backoff

## Testing

```bash
cargo test              # 31 tests (27 unit + 4 integration)
cargo clippy            # Zero warnings
cargo fmt --check       # Formatting
```

Integration tests run the compiled binary and verify CLI flags, system prompt output, and error handling.
