# Roadmap

This document tracks the planned improvements for agent-code on the path to v1.0 and beyond. Items are organized by phase and priority. Each phase can be worked on independently, though some items have noted dependencies.

Status key: **Planned** | **In Progress** | **Done**

---

## Current State

| Area | Status |
|------|--------|
| Rust workspace (lib + cli) | 25K+ LOC, 100+ source files |
| Built-in tools | 32 (file ops, search, shell, web, LSP, MCP, agents, tasks) |
| Slash commands | 43 (session, context, git, agent control, config, diagnostics, history, sharing, update) |
| Bundled skills | 12 (commit, review, test, explain, debug, pr, refactor, init, security-review, advisor, bughunter, plan) |
| LLM providers | 14 (Anthropic, OpenAI, Azure OpenAI, xAI, Google, DeepSeek, Groq, Mistral, Together, Zhipu, Ollama, Bedrock, Vertex, OpenRouter) |
| Tests | 232+ (unit + integration + smoke + benchmarks) |
| Platforms | Linux x86_64/aarch64, macOS x86_64/aarch64, Windows x86_64, Docker |
| Install methods | cargo, homebrew (custom tap), curl script, prebuilt binaries, Docker, npm |
| Docs | 30+ pages (Mintlify + mdBook), tutorials, architecture, FAQ, security |
| Server mode | Headless HTTP API (`agent --serve`) with SSE streaming |
| Multi-agent | Coordinator with spawn, run_team, messaging |

---

## Phase 1: Documentation

**Priority: Critical** | **Target: v0.10**

Good documentation is the highest-leverage improvement. Every other phase benefits from having clear, complete docs.

### 1.1 CHANGELOG — Done

- [x] Create `CHANGELOG.md` using [Keep a Changelog](https://keepachangelog.com/) format
- [x] Reconstruct history from git tags (Added / Changed / Fixed / Removed per version)
- [x] Add `/release-notes` command that reads and displays the current version's entry

### 1.2 Troubleshooting Guide — Done

- [x] Create `docs/troubleshooting.mdx`
- [x] Cover: API connection failures, permission denials, MCP server crashes, context window exceeded, tool execution errors, installation problems
- [x] Each issue: symptoms, cause, fix

### 1.3 Tutorials (4 pages) — Done

- [x] **First Project** — setting up agent-code on an existing codebase
- [x] **Custom Skills** — writing, testing, and sharing a custom skill
- [x] **MCP Integration** — connecting and using MCP servers
- [x] **Multi-Provider** — switching between LLM providers, configuring fallbacks
- [x] Tutorials navigation group added to `docs/mint.json`

### 1.4 FAQ — Done

- [x] Create `docs/faq.mdx` with 18 questions
- [x] Categories: general, installation, usage, cost, security, extensibility

### 1.5 Security Documentation — Done

- [x] Expanded `SECURITY.md` with protected dirs, skill safety, enterprise config, bypass prevention
- [x] Created `docs/security.mdx` with full security model reference
- [x] Documented destructive command detection in Bash tool

### 1.6 Architecture Deep-Dives (4 pages) — Done

- [x] **Context Compaction** — microcompact, LLM summary, context collapse
- [x] **Tool Execution** — permission flow, batching, streaming executor
- [x] **Provider Abstraction** — Anthropic/OpenAI normalization
- [x] **MCP Protocol** — stdio/SSE transports, proxying, resources
- [x] Architecture navigation group added to `docs/mint.json`

### 1.7 Performance Tuning Guide — Done

- [x] Created `docs/guides/performance.mdx`
- [x] Covers: model selection, context management, cost control, benchmarking

### 1.8 README Enhancements — Done

- [x] Add CI badge
- [x] Expand feature table with tool count, command count, skill count
- [x] Add platforms table with Docker
- [x] Add security section, skills table, commands section
- [x] Link to CONTRIBUTING.md and ROADMAP.md

### 1.9 Rustdoc for Library Crate — Done

- [x] Top-level rustdoc with module table, quick example, custom Tool example
- [x] Add `///` doc comments to all key public structs, enums, and traits
- [x] Publish rustdoc via CI (GitHub Pages at `/api/`)

---

## Phase 2: Skills and Extensibility

**Priority: High** | **Target: v0.11**

### 2.1 New Bundled Skills — Done

- [x] **`security-review`** — OWASP vulnerability scan with severity ratings
- [x] **`advisor`** — Architecture and dependency health analysis
- [x] **`bughunter`** — Systematic bug search with reproduction steps
- [x] **`plan`** — Structured implementation planning with risk flags

### 2.2 Skill Safety Setting — Done

- [x] Add `disable_skill_shell_execution: bool` to `SecurityConfig`
- [x] `Skill::expand_safe()` strips fenced shell blocks when enabled
- [x] Non-shell code blocks preserved

### 2.3 Plugin Executable Support — Done

- [x] `PluginExecTool` implementing the `Tool` trait
- [x] `discover_plugin_executables()` scans `bin/` directories
- [x] Cross-platform: Unix permission check, Windows `.exe` check
- [x] Tools namespaced as `plugin__<name>__<binary>`
- [x] `PluginRegistry::executable_tools()` returns discovered tools

### 2.4 Remote Skill Discovery — Done

- [x] Skill index fetched from configurable URL (default: GitHub)
- [x] `/skill install <name>` to download to user skills directory
- [x] `/skill search [query]`, `/skill installed`, `/skill remove <name>`
- [x] Offline fallback to cached index (1h cache, 24h stale warning)

---

## Phase 3: New Commands

**Priority: High** | **Target: v0.11**

All commands are added to `crates/cli/src/commands/mod.rs`.

### 3.1 Session Commands — Done

- [x] **`/summary`** — Delegates to agent for session summary
- [x] **`/share`** — Exports session as shareable markdown with metadata
- [x] **`/feedback`** — Saves feedback to `~/.local/share/agent-code/feedback/`

### 3.2 Info Commands — Done

- [x] **`/release-notes`** — Reads CHANGELOG.md, displays current version's entry

### 3.3 Cost Enhancements — Done

- [x] Per-model token breakdown in `/cost` when multiple models used
- [x] Cache hit percentage per model
- [x] Single-model sessions show inline cache hit rate

### 3.4 Headless Mode — Done

- [x] **`agent --serve`** — Headless HTTP API server (POST /message, GET /status, /messages, /health)
- [x] Bridge lock file for IDE discovery
- [x] Graceful shutdown with Ctrl+C
- [x] **`agent --attach [session-id]`** — Reconnect by session ID prefix or interactive selection
- [x] SSE event stream for real-time updates (GET /events with 10 event types)

### 3.5 Self-Management Commands — Partially Done

- [x] **`/update`** — Check GitHub releases API, notify if newer version
- [ ] **`/uninstall`** — Remove binary, config, and data directories with confirmation

---

## Phase 4: Provider Expansion

**Priority: Medium** | **Target: v0.12**

### 4.1 New Providers — Done (4/5)

- [x] **Azure OpenAI** — separate from generic OpenAI, with Azure-specific auth (AD tokens, managed identity, `AZURE_OPENAI_API_KEY`)
- [x] **OpenRouter** — single API key for any model (`OPENROUTER_API_KEY`)
- [x] **Cohere** — Command R+ models
- [x] **Perplexity** — search-augmented generation
- [ ] **GitHub Copilot** — Copilot token-based access for GitHub users

### 4.2 Provider Features

- [ ] Interactive setup wizard for Bedrock (AWS credential chain, region, model selection)
- [ ] Provider health check in `/doctor` output

---

## Phase 5: Testing and Quality

**Priority: Medium** | **Target: v0.11**

### 5.1 Smoke Tests — Done

- [x] `crates/cli/tests/smoke.rs` — `--version`, `--help`, unknown flags
- [x] CI-safe (no API key required)

### 5.2 Integration Tests — Done

- [x] `crates/lib/tests/skills_integration.rs` — 6 tests: bundled loading, finding by name, custom skill from temp dir, override, directory skills
- [x] `crates/lib/tests/config_integration.rs` — 5 tests: defaults, TOML parsing, security config, features, MCP entries

### 5.3 Benchmarks — Done

- [x] `crates/lib/benches/compaction.rs` — microcompact at 10, 50, 100, 500 turns
- [x] `crates/lib/benches/token_estimation.rs` — estimate_tokens + estimate_context_tokens

### 5.4 Coverage Reporting — Done

- [x] cargo-tarpaulin in CI with Cobertura XML output
- [x] Codecov upload
- [x] Coverage badge in README

### 5.5 Per-Agent Permissions (Stretch)

- [ ] Extend permission system to support per-agent rule sets (not just global)
- [ ] Plan mode gets its own permission model (read-only by default, configurable)
- [ ] Subagents can have restricted tool access

---

## Phase 6: Distribution and CI

**Priority: Medium** | **Target: v1.0**

### 6.1 Windows Support — Done

- [x] `windows-latest` in CI test matrix
- [x] `x86_64-pc-windows-msvc` in release builds (packaged as `.zip`)
- [x] Fixed unused variable warning for Windows compilation

### 6.2 Docker Image — Done

- [x] Multi-stage `Dockerfile` (Rust builder + slim Debian runtime with git, rg, python3, node)
- [x] `.github/workflows/docker.yml` — build and push to `ghcr.io/avala-ai/agent-code` on tags
- [x] `.dockerignore` for clean build context

### 6.3 Protected Directories — Done

- [x] Built-in deny rules for `.git/`, `.husky/`, `node_modules/`
- [x] Enforced before user-configured rules
- [x] Read access unaffected
- [x] Cross-platform path handling (forward and backslash)

### 6.4 Self-Update Mechanism — Done

- [x] `crates/cli/src/update.rs` with GitHub releases API check
- [x] Background check on REPL startup (24h cooldown, 5s timeout)
- [x] `/update` command (alias: `/upgrade`) for manual check
- [x] Post-session notification if newer version found

### 6.5 npm Wrapper Package — Done

- [x] `npm/` directory with `package.json`, `install.js`
- [x] `npm install -g @avala-ai/agent-code` downloads prebuilt binary
- [x] Publish workflow in `release.yml` (publishes on tag push)
- [x] Published to npm as `@avala-ai/agent-code` (scoped — unscoped name conflicts with `agentcode`)

### 6.6 Homebrew Core (Stretch)

- [ ] Submit formula to `homebrew/homebrew-core` (requires stable release history and passing tests)
- [ ] Depends on: v1.0 release

---

## Phase 7: Advanced Features (Post v1.0)

These are tracked for future exploration. Not committed to a timeline.

### 7.1 Agent Client Protocol (ACP) — Done

- [x] Implement ACP v1 for IDE integrations (Zed, VS Code, JetBrains)
- [x] Map ACP sessions to internal agent-code sessions
- [x] `agent acp` command to start ACP stdio server (JSON-RPC 2.0, 5 methods, 9 event types)

### 7.2 Multi-Agent Orchestration — Done

- [x] `Coordinator` runtime with `spawn_agent()`, `run_agent()`, `run_team()`
- [x] Parallel multi-agent execution via `tokio::spawn`
- [x] `send_message()` for inter-agent messaging (by ID or name)
- [x] `create_team()` / `list_teams()` for team management
- [x] Shared `build_agent_command()` helper for subprocess spawning

### 7.3 Interactive Tutorial System — Done

- [x] `/powerup` command with step-by-step interactive lessons
- [x] Lesson structure: explanation, try-it prompt, success verification
- [x] Ship 5 lessons covering core features

### 7.4 Process-Level Sandboxing

**Priority: Critical** | **Target: v1.1**

The current permission system is policy enforcement within the same process — it trusts that tools respect the rules. A malicious tool result or prompt injection could bypass permission checks because no OS-level boundary exists. Process-level sandboxing ensures that even if the agent is compromised, damage is contained.

**Architecture:**

```text
┌─────────────────────────────────┐
│  Agent Process (unsandboxed)    │
│  ├─ REPL, config, LLM client   │
│  └─ Tool Dispatch               │
│       │                          │
│       ▼                          │
│  ┌────────────────────────┐     │
│  │  Sandbox Executor      │     │
│  │  Selects strategy:     │     │
│  │  ├─ macOS: seatbelt    │     │
│  │  ├─ Linux: bubblewrap  │     │
│  │  └─ Windows: integrity │     │
│  └────────────────────────┘     │
│       │                          │
│       ▼                          │
│  ┌────────────────────────┐     │
│  │  Sandboxed subprocess  │     │
│  │  - No network (opt)    │     │
│  │  - Write-scoped to cwd │     │
│  │  - Secret files masked │     │
│  └────────────────────────┘     │
└─────────────────────────────────┘
```

**macOS Seatbelt Strategy:**

Use `sandbox-exec` with an SBPL (Scheme) policy:
- Start from `(deny default)` — block all operations
- Selectively allow: file reads everywhere, file writes only within project directory and allowed paths
- Network access conditionally appended (enabled for `WebFetch`, disabled for `Bash`)
- Secret file masking: regex patterns to block `.env`, `.env.*`, credential files
- Git worktree support: auto-detect `.git` directory and grant read/write to the underlying git directory for both worktrees and main repos
- Dual-path resolution: resolve both symbolic and real paths to prevent symlink bypass attacks

```text
(version 1)
(deny default)
(import "system.sb")
(allow file-read* (subpath "/"))
(allow file-write* (subpath "${PROJECT_DIR}"))
(deny file-read* (regex #"^.*/\.env(\..+)?$"))
```

**Linux bubblewrap (bwrap) Strategy:**

Use namespace isolation via `bwrap`:
- `--unshare-all` — isolate user, IPC, UTS, mount, PID, and network namespaces
- `--dev /dev` — minimal device stubs instead of host /dev
- `--proc /proc` — isolated process info
- Secret masking: find `.env*` files (maxdepth 3 for performance) and bind to `/dev/null`
- Forbidden path handling: `--tmpfs` + `--remount-ro` for directories, `/dev/null` binding for files
- Network: conditionally add `--share-net` for tools that need it

**Windows Low Integrity Strategy:**

Use Windows Mandatory Integrity Control:
- Spawn sandboxed process with Low Mandatory Level (SID `S-1-16-4096`)
- Grant Modify access (`(OI)(CI)(M)`) to Low integrity SID on project directory via `icacls`
- Deny Low Integrity access (`(OI)(CI)(F)`) to secret files
- Reject UNC paths (`\\server\share`) to prevent credential theft via SMB
- Never modify ACLs on `%SystemRoot%`, `%ProgramFiles%` — prevent host corruption
- Use manifest file for forbidden paths to avoid command-line length limits

**Policy Configuration (TOML):**

```toml
[sandbox]
enabled = true
strategy = "auto"  # auto-detect platform, or: "seatbelt", "bwrap", "none"

# Paths the sandbox can write to (beyond project directory)
allowed_write_paths = ["/tmp", "~/.cache/agent-code"]

# Paths the sandbox can never read
forbidden_paths = ["~/.ssh", "~/.aws/credentials", "~/.gnupg"]

# Secret file patterns to mask (bound to /dev/null)
secret_patterns = [".env", ".env.*", "credentials.json", "*.pem"]

# Per-tool sandbox overrides
[sandbox.tools]
Bash = { network = false }     # No network for shell commands
WebFetch = { network = true }  # Network required
FileRead = { sandbox = false } # Skip sandbox for reads (performance)
```

**Implementation Tasks:**
- [ ] Add `crates/lib/src/sandbox/mod.rs` — trait `SandboxStrategy` with `fn wrap_command(&self, cmd: Command, policy: &SandboxPolicy) -> Command`
- [ ] Add `crates/lib/src/sandbox/seatbelt.rs` — macOS implementation using `sandbox-exec -f <profile>`
- [ ] Add `crates/lib/src/sandbox/bwrap.rs` — Linux implementation using `bwrap` binary
- [ ] Add `crates/lib/src/sandbox/windows.rs` — Windows Low Integrity via `icacls` + restricted token
- [ ] Add `crates/lib/src/sandbox/policy.rs` — `SandboxPolicy` struct parsed from config TOML
- [ ] Add `SandboxConfig` section to `ConfigSchema` with `enabled`, `strategy`, `allowed_write_paths`, `forbidden_paths`, `secret_patterns`
- [ ] Wire into `tools/executor.rs` — wrap tool subprocess commands through sandbox before execution
- [ ] Per-tool override: `sandbox.tools.<ToolName>` config for network/sandbox toggles
- [ ] Auto-detect platform and select strategy in `SandboxStrategy::detect()`
- [ ] Secret masking: scan project + allowed paths for files matching `secret_patterns`, mask them
- [ ] Git worktree detection: resolve `.git` files to actual git directories, add to allowed paths
- [ ] Symlink resolution: resolve both canonical and symlink paths to prevent bypass
- [ ] Fallback behavior: if sandbox binary not found (`bwrap` / `sandbox-exec`), warn and run unsandboxed with degraded security notice
- [ ] `/sandbox` slash command showing current strategy, allowed paths, masked files
- [ ] Add `--no-sandbox` CLI flag to disable (requires `disable_bypass_permissions = false`)
- [ ] Tests: unit tests for policy parsing, integration tests for each platform strategy
- [ ] Benchmark: measure sandbox overhead per tool call (target: <10ms)

### 7.5 Behavioral Evaluation Framework

**Priority: High** | **Target: v1.1**

Unit and integration tests verify that code compiles and functions run correctly. Behavioral evals verify that the agent actually accomplishes tasks when given a prompt and a live LLM. This is a fundamentally different category of testing — it answers "does the agent work?" rather than "does the code compile?"

**Architecture:**

```text
┌──────────────────────────────────────────────────┐
│  Eval Harness (eval_runner binary)               │
│  ├─ Load eval definitions from evals/            │
│  ├─ For each eval:                               │
│  │   ├─ Set up temp workspace (fixture files)    │
│  │   ├─ Run agent with prompt + auto-approve     │
│  │   ├─ Capture: tool calls, files changed, exit │
│  │   ├─ Run assertions                           │
│  │   └─ Retry up to N times (LLM non-determinism)│
│  └─ Report: pass/fail/flaky per eval             │
└──────────────────────────────────────────────────┘
```

**Eval Definition Format:**

```rust
// evals/file_editing.rs
eval_test! {
    name: "creates_new_file_when_asked",
    policy: EvalPolicy::AlwaysPasses,
    fixture: "evals/fixtures/empty_project/",
    prompt: "Create a file called hello.py that prints 'Hello, world!'",
    max_turns: 5,
    assert: |rig| {
        assert!(rig.workspace.join("hello.py").exists());
        let content = std::fs::read_to_string(rig.workspace.join("hello.py"))?;
        assert!(content.contains("Hello, world!"));
        assert!(rig.tool_log.contains_call("FileWrite"));
    }
}
```

**Two Policy Tiers:**

| Tier | Pass Requirement | CI Behavior | Promotion Path |
|------|------------------|-------------|----------------|
| `AlwaysPasses` | 100% (all retries) | Blocks merge | Promoted from `UsuallyPasses` after 7 days at 100% |
| `UsuallyPasses` | 50%+ (best-of-N) | Monitored nightly, does not block | New evals start here |

**Best-of-N Retry Logic:**

LLMs are non-deterministic — the same prompt may produce different tool call sequences:
- Each eval runs up to 4 times
- `AlwaysPasses`: must pass all 4
- `UsuallyPasses`: must pass 2+ of 4 (50% bar)
- On transient API errors (rate limit, timeout), retry is free (doesn't count as failure)

**Trustworthiness Filtering:**

Before including a `UsuallyPasses` eval in CI, verify stability:
- Must pass 60%+ on nightly runs (2 of 3 every night)
- Must maintain 80%+ aggregate over trailing 6 days
- Evals below threshold are excluded from CI with a `flaky` label

**Test Rig Features:**

```rust
struct TestRig {
    workspace: PathBuf,            // Temp directory with fixture files
    tool_log: ToolLog,             // Captured tool calls with arguments
    activity_log: Vec<Event>,      // Timestamped agent events (JSONL)
    session_history: Vec<Message>, // Full conversation for debugging
}

impl TestRig {
    /// Pause agent before specific tool calls (tests model steering)
    fn set_breakpoint(&mut self, tools: &[&str]);
    /// Inject a user hint between tool calls
    fn add_user_hint(&mut self, hint: &str);
    /// Read captured tool call log
    fn tool_calls(&self) -> &[CapturedToolCall];
}
```

**Breakpoint & Steering Tests:**

```rust
eval_test! {
    name: "responds_to_corrective_hint",
    policy: EvalPolicy::UsuallyPasses,
    prompt: "Refactor the User struct to use builder pattern",
    setup: |rig| {
        rig.set_breakpoint(&["FileWrite"]);
        rig.add_user_hint("Actually, use the typestate pattern instead of builder");
    },
    assert: |rig| {
        let content = std::fs::read_to_string(rig.workspace.join("src/user.rs"))?;
        assert!(content.contains("impl User<Unvalidated>"));
        assert!(!content.contains("UserBuilder"));
    }
}
```

**Nightly CI Pipeline:**

```yaml
# .github/workflows/evals-nightly.yml
schedule:
  - cron: "0 2 * * *"  # 2 AM UTC daily
jobs:
  evals:
    strategy:
      matrix:
        model: [claude-sonnet-4-6, gpt-4.1]
    steps:
      - run: cargo run --bin eval_runner -- --policy usually_passes --retries 3
      - uses: actions/upload-artifact@v4
```

**Automatic Promotion:**

A scheduled job reviews nightly results weekly:
- If a `UsuallyPasses` eval has passed 100% for 7+ consecutive days → auto-promote to `AlwaysPasses`
- If an `AlwaysPasses` eval fails 2+ times in a week → auto-demote to `UsuallyPasses` with alert

**Implementation Tasks:**
- [ ] Create `evals/` directory at repo root with harness module
- [ ] Add `eval_runner` binary to `crates/cli/` (separate from main CLI binary)
- [ ] Implement `TestRig` with workspace setup, tool log capture, activity logging
- [ ] Implement `eval_test!` macro for declarative eval definitions
- [ ] Implement best-of-N retry logic with transient error detection
- [ ] Implement breakpoint mechanism: intercept tool dispatch, pause, inject hint
- [ ] Add `EvalPolicy` enum and tier enforcement
- [ ] Write 10 seed evals: file creation, file editing, grep usage, bash execution, multi-file refactor, test writing, error recovery, permission denial handling, plan mode, multi-turn conversation
- [ ] Add `evals/fixtures/` with starter project templates
- [ ] Create `.github/workflows/evals-nightly.yml` with matrix strategy
- [ ] Create `.github/workflows/evals-promotion.yml` for weekly auto-promotion
- [ ] Add trustworthiness tracking: JSONL log of pass/fail per eval per night
- [ ] Add `cargo run --bin eval_runner -- --list` to show all evals with status
- [ ] Add `cargo run --bin eval_runner -- --eval <name>` to run a single eval

### 7.6 Advanced History Compression

**Priority: High** | **Target: v1.1**

The current compaction system (auto/reactive/microcompact) is solid but operates at message granularity. File-level tracking and secret masking improve both accuracy and security.

**File-Level Compression Tracking:**

Track each file mentioned in the conversation at one of four fidelity levels:

| Level | Description | When |
|-------|-------------|------|
| `Full` | Complete file contents in context | Recently read (within last 2 turns) |
| `Partial` | Key sections only (functions referenced, changed lines) | Older reads, still relevant |
| `Summary` | LLM-generated 2-3 sentence summary of file's role | Old reads, rarely referenced |
| `Excluded` | File removed from context entirely | Stale, never referenced again |

```rust
struct FileCompressionRecord {
    path: PathBuf,
    level: CompressionLevel,
    content_hash: [u8; 12],  // 12-byte SHA256 slice for change detection
    line_range: Option<(usize, usize)>,  // For Partial level
    last_referenced_turn: usize,
}
```

**Content Hash Change Detection:**

When the agent re-reads a file:
1. Compute 12-byte SHA256 of current content
2. Compare to stored hash
3. If changed: reset to `Full` level (file was modified, agent needs fresh context)
4. If unchanged: keep current level (no new information)

This prevents re-injecting stale file content after edits, while avoiding unnecessary re-reads of unchanged files.

**Protected File Mechanism:**

Files read in the last 2 turns (4 history items) are locked at `Full` level and cannot be compressed. This prevents the compactor from summarizing files the agent is actively working on.

**Secret Masking in Compression:**

Before passing history to the LLM for summarization, redact sensitive patterns:

```rust
const SECRET_PATTERNS: &[&str] = &[
    r"(?i)(api[_-]?key|secret|password|token)\s*[:=]\s*\S+",
    r"AKIA[0-9A-Z]{16}",                    // AWS access keys
    r"sk-[a-zA-Z0-9]{20,}",                 // Provider API keys
    r"ghp_[a-zA-Z0-9]{36}",                 // GitHub PATs
    r"-----BEGIN (RSA |EC )?PRIVATE KEY-----", // PEM keys
];
```

Replace matched patterns with `[REDACTED:<type>]` before compression. The summary never contains raw secrets, even if they appeared in tool results.

**Compression State Persistence:**

Save compression records to `~/.cache/agent-code/sessions/<id>/compression_state.json` so session restoration respects file compression levels.

**Implementation Tasks:**
- [ ] Add `FileCompressionRecord` struct to `services/compact.rs`
- [ ] Add `CompressionLevel` enum: `Full`, `Partial`, `Summary`, `Excluded`
- [ ] Implement content hashing (12-byte SHA256 slice) for change detection
- [ ] Implement protected file mechanism: lock recent 2-turn file reads
- [ ] Add `SecretMasker` that applies regex patterns before LLM summarization
- [ ] Add compression state serialization to session persistence
- [ ] Wire into existing auto-compact: after summarization, update file records
- [ ] Add `/compression` slash command showing file compression states
- [ ] Tests: verify secrets are redacted in summaries, verify hash change detection, verify protected files resist compression

### 7.7 Non-Interactive Structured Output

**Priority: Medium** | **Target: v1.1**

The current `--prompt` one-shot mode outputs plain text. For CI/CD pipelines, GitHub Actions, and tool-chaining, structured JSON streaming is essential.

**JSON Streaming Format:**

Each event is a single JSON line (JSONL) written to stdout:

```jsonl
{"type":"session_start","session_id":"abc-123","model":"claude-sonnet-4-6","timestamp":"2026-04-06T12:00:00Z"}
{"type":"text_delta","content":"I'll create the file now.","turn":1}
{"type":"tool_call","tool":"FileWrite","input":{"file_path":"hello.py","content":"print('hello')"},"turn":1}
{"type":"tool_result","tool":"FileWrite","output":"File written successfully","is_error":false,"turn":1}
{"type":"text_delta","content":"Done! I created hello.py.","turn":1}
{"type":"turn_complete","turn":1,"input_tokens":1234,"output_tokens":567,"cost_usd":0.003}
{"type":"session_end","turns":1,"total_cost_usd":0.003,"exit_code":0}
```

**Event Types:**

| Event | Fields | When |
|-------|--------|------|
| `session_start` | session_id, model, timestamp | Session begins |
| `text_delta` | content, turn | LLM streams text |
| `thinking` | content, turn | Extended thinking block |
| `tool_call` | tool, input, turn | Tool invocation starts |
| `tool_result` | tool, output, is_error, turn | Tool completes |
| `permission_request` | tool, message, turn | Permission needed (auto-denied in non-interactive) |
| `turn_complete` | turn, input_tokens, output_tokens, cost_usd | Turn finishes |
| `error` | message, code, turn | Error occurred |
| `session_end` | turns, total_cost_usd, exit_code | Session ends |

**CLI Interface:**

```bash
# JSON streaming to stdout
agent --prompt "fix the tests" --output-format json

# Pipe to jq for filtering
agent --prompt "fix the tests" --output-format json | jq 'select(.type == "tool_call")'

# Human-readable feedback on stderr, structured data on stdout
agent --prompt "review this PR" --output-format json 2>/dev/null | process_results.py
```

**Exit Codes:**

| Code | Meaning |
|------|---------|
| 0 | Success — agent completed normally |
| 1 | Fatal configuration error |
| 2 | Fatal input error |
| 3 | Tool execution failure (unrecoverable) |
| 4 | LLM error (all retries and fallbacks exhausted) |
| 5 | Cost limit exceeded |
| 6 | Turn limit exceeded |
| 7 | Permission denied (non-interactive, no auto-approve) |

**Implementation Tasks:**
- [ ] Add `OutputFormat` enum (`Text`, `Json`) to CLI config
- [ ] Add `--output-format` CLI flag (default: `text`)
- [ ] Create `JsonStreamFormatter` in `crates/cli/src/output/` that writes JSONL to stdout
- [ ] Route all agent events through formatter: text deltas, tool calls, tool results, turns, errors
- [ ] Human-readable status messages go to stderr in JSON mode
- [ ] Implement exit code enumeration matching table above
- [ ] Add `--resume <session-id>` support in non-interactive mode
- [ ] Handle Ctrl+C in non-TTY mode: detect `\u0003` on stdin or SIGINT
- [ ] Add permission auto-denial in JSON mode (with `permission_request` event)
- [ ] Tests: verify JSONL output parses correctly, verify exit codes, verify stderr/stdout separation

### 7.8 Remote Agent Protocol

**Priority: Medium** | **Target: v1.2**

Current subagents run as local subprocesses. A remote agent protocol enables out-of-process agents running on other machines, in containers, or as cloud services — enabling distributed workloads, specialized agents, and team-shared agent pools.

**Protocol Design:**

Use JSON-RPC 2.0 over multiple transports:

| Transport | Use Case | Configuration |
|-----------|----------|---------------|
| Stdio | Local subprocess (current behavior) | `command`, `args` |
| HTTP/REST | Remote agents, cloud services | `url` |
| WebSocket | Long-running agents with bidirectional streaming | `url` (ws:// or wss://) |

**Agent Card (Discovery):**

Remote agents publish an Agent Card describing capabilities:

```json
{
  "name": "security-auditor",
  "description": "Specialized agent for security analysis",
  "version": "1.0.0",
  "capabilities": ["file_read", "grep", "bash"],
  "max_turn_duration_secs": 1800,
  "authentication": {
    "type": "bearer_token",
    "token_env": "SECURITY_AGENT_TOKEN"
  },
  "endpoint": "https://agents.internal.company.com/security-auditor"
}
```

**Agent Card Resolution:**
- URL: fetch card from `<endpoint>/.well-known/agent-card.json`
- Inline: embed card JSON directly in config
- Local: load from `.agent-code/agents/<name>.json`

**Streaming Progress:**

Remote agents stream progress updates back to the coordinator:
- `progress` — intermediate status text
- `tool_call` — tool invocation notification (for audit logging)
- `complete` — final result with summary
- `error` — failure with error details

**Session State Persistence:**

Maintain per-remote-agent `context_id` and `task_id` across invocations so follow-up prompts continue the same conversation.

**Configuration:**

```toml
[agents.security-auditor]
type = "remote"
url = "https://agents.internal.company.com/security-auditor"
auth = { type = "bearer_token", token_env = "SECURITY_AGENT_TOKEN" }
timeout_secs = 1800  # 30 min for long-running audits

[agents.test-runner]
type = "remote"
url = "ws://localhost:9090/agent"
auth = { type = "none" }
timeout_secs = 600
```

**Implementation Tasks:**
- [ ] Define `AgentCard` struct in `crates/lib/src/agents/`
- [ ] Add `RemoteAgentClient` with HTTP and WebSocket transports
- [ ] Add Agent Card resolution: URL fetch, inline JSON, local file
- [ ] Add authentication: bearer token, API key header, OAuth2
- [ ] Implement streaming progress reassembly (accumulate chunks into final result)
- [ ] Add session state persistence for multi-turn remote conversations
- [ ] Wire into `Coordinator` — `spawn_agent()` selects local vs remote based on config
- [ ] Add 30-minute timeout for remote agents (vs 5-minute default for local)
- [ ] Add `/agents` slash command showing local + remote agents with health status
- [ ] Add remote agent health check: periodic ping to verify endpoint is responsive
- [ ] Tests: mock HTTP/WebSocket server for remote agent protocol tests

### 7.9 DevTools Debugging Server

**Priority: Medium** | **Target: v1.2**

For developers building on top of agent-code, debugging complex sessions, or developing custom tools/skills, a dedicated debugging server provides real-time inspection without cluttering the REPL.

**Architecture:**

```text
┌─────────────────┐     WebSocket      ┌──────────────────────┐
│  Agent Process   │──────────────────▶│  DevTools UI          │
│  (with devtools  │                    │  (browser at          │
│   server enabled)│◀──────────────────│   localhost:25417)    │
└─────────────────┘     Commands       └──────────────────────┘
```

**Data Streams:**

| Stream | Buffer Size | Content |
|--------|-------------|---------|
| Tool Calls | 2000 entries | Tool name, input, output, duration, error, turn |
| LLM Requests | 500 entries | Model, tokens in/out, cache hits, latency, streaming chunks |
| Permission Events | 1000 entries | Tool, decision, reason, rule matched |
| Console Logs | 5000 entries | Level, message, timestamp, source |
| Agent Events | 1000 entries | Turn start/end, compaction, model switch, session events |

All buffers are bounded FIFO — oldest entries evicted when full.

**WebSocket Protocol:**

```jsonl
// Server → Client: real-time events
{"type":"tool_call","data":{"tool":"Bash","input":{"command":"cargo test"},"turn":3},"timestamp":"..."}
{"type":"llm_request","data":{"model":"claude-sonnet-4-6","input_tokens":15234,"latency_ms":2340},"timestamp":"..."}

// Client → Server: commands
{"type":"command","action":"pause"}
{"type":"command","action":"resume"}
{"type":"command","action":"get_state"}
```

**CLI Integration:**

```bash
agent --devtools                       # Enable on default port (25417)
agent --devtools --devtools-port 9999  # Custom port
```

**Implementation Tasks:**
- [ ] Add `crates/lib/src/services/devtools.rs` — WebSocket server using `tokio-tungstenite`
- [ ] Implement bounded event buffers for each data stream
- [ ] Add WebSocket message protocol (server→client events, client→server commands)
- [ ] Add port auto-negotiation with file-based port discovery
- [ ] Wire into tool executor, LLM client, permission checker for event emission
- [ ] Add `--devtools` and `--devtools-port` CLI flags
- [ ] Add `[devtools]` config section
- [ ] Add pause/resume commands via WebSocket
- [ ] Create minimal HTML viewer page served at `http://localhost:<port>/` (single-file, no build step)
- [ ] Tests: WebSocket connection, event emission, buffer overflow

### 7.10 GitHub Automation

**Priority: Medium** | **Target: v1.2**

Extend beyond CLI into GitHub-native workflows for community scaling and CI/CD integration.

**GitHub Action:**

A first-party GitHub Action that runs agent-code in CI:

```yaml
# .github/workflows/pr-review.yml
on:
  pull_request:
    types: [opened, synchronize]

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: avala-ai/agent-code-action@v1
        with:
          prompt: "Review this PR for security issues and test coverage gaps."
          model: claude-sonnet-4-6
          max-cost-usd: 1.0
          output-format: json
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

**Automated Issue Triage:**

A reusable workflow that uses agent-code to analyze new issues and apply labels:

```yaml
# .github/workflows/issue-triage.yml
on:
  issues:
    types: [opened, reopened]
  schedule:
    - cron: "0 * * * *"  # Hourly for untriaged backlog
```

Triage rules:
- Apply area labels (area/core, area/tools, area/docs, area/security, area/platform)
- Apply kind labels (kind/bug, kind/feature, kind/question, kind/docs)
- Apply priority labels (P1-P3 only — never assign P0, route to manual triage instead)
- Demote priority if issue lacks reproduction steps (P1→P2, P2→P3)
- Never remove maintainer-applied labels
- Add `status/bot-triaged` to all processed issues

**PR Auditing:**

Automated PR-to-issue linking and label synchronization:
- Link PRs to issues via commit messages and branch names
- Sync labels from linked issues to PRs
- Run on 15-minute schedule for near-real-time updates

**Implementation Tasks:**
- [ ] Create `action/` directory with `action.yml` (GitHub Action metadata)
- [ ] Action wraps Docker image: pull `ghcr.io/avala-ai/agent-code`, run with `--prompt` + `--output-format json`
- [ ] Add `GITHUB_TOKEN` integration: agent can read PR diffs, post comments, apply labels via `gh` CLI
- [ ] Create reusable workflow templates: `pr-review.yml`, `issue-triage.yml`, `pr-audit.yml`
- [ ] Add issue triage prompt templates with label taxonomy
- [ ] Create documentation in `docs/tutorials/github-action.mdx`
- [ ] Tests: Action integration tests using `act` (local GitHub Actions runner)

### 7.11 OAuth and Free-Tier Authentication

**Priority: Medium** | **Target: v1.2**

API key management is the #1 friction point for new users. Adding OAuth-based authentication with a managed free tier removes this barrier entirely.

**Authentication Flow:**

```text
User runs `agent` for the first time
    │
    ▼
CLI detects no API key configured
    │
    ▼
Presents auth options:
  1. Sign in with account (OAuth — opens browser)
  2. Enter API key manually
  3. Use environment variable
    │
    ▼ (Option 1)
Opens browser to auth endpoint
    │
    ▼
User authenticates → redirect to localhost callback
    │
    ▼
CLI receives OAuth token, stores in OS keychain
    │
    ▼
Token used for API requests (auto-refresh on expiry)
```

**Token Storage:**

| Platform | Storage |
|----------|---------|
| macOS | Keychain Services (`security` CLI) |
| Linux | `libsecret` / `gnome-keyring` / `kwallet` (fallback: encrypted file) |
| Windows | Windows Credential Manager (`cmdkey`) |

Never store tokens in plaintext config files.

**Token Lifecycle:**

- Access tokens: 1-hour TTL, auto-refresh on 401 response
- Refresh tokens: stored in keychain alongside access token
- If refresh fails: prompt re-authentication via browser

**Free Tier Limits:**

```toml
[free_tier]
requests_per_minute = 30
requests_per_day = 500
context_window = 200_000  # tokens
models = ["fast-model"]   # Subset of available models
```

**Implementation Tasks:**
- [ ] Add `crates/lib/src/auth/` module with `AuthProvider` trait
- [ ] Implement `OAuthProvider` with PKCE flow (browser-based)
- [ ] Implement `ApiKeyProvider` (extract current behavior into trait)
- [ ] Add OS keychain integration: macOS (Security.framework), Linux (libsecret), Windows (WinCred)
- [ ] Add token refresh middleware in LLM client
- [ ] Add auth selection UI on first run (interactive menu)
- [ ] Add `/auth` slash command (login, logout, status, switch)
- [ ] Add `--auth` CLI flag to force re-authentication
- [ ] Implement rate limit display: show remaining quota in session footer
- [ ] Tests: mock OAuth server for auth flow tests, keychain read/write tests

### 7.12 Stateful Epic Workflows

**Priority: High** | **Target: v1.1**

For large feature requests that span multiple files, directories, and sessions, a structured workflow prevents the agent from losing track of progress and ensures methodical implementation.

**The `/epic` Command:**

```bash
/epic "Add Stripe billing with usage-based pricing"
```

Instead of immediate implementation, the agent:
1. Generates `.agent-code/epics/<id>/spec.md` — requirements, constraints, open questions
2. Generates `.agent-code/epics/<id>/plan.md` — sequenced checklist of implementation steps
3. Presents the plan for user review and approval
4. Implements step-by-step, checking off items as they complete

**Epic Directory Structure:**

```text
.agent-code/epics/
└── stripe-billing-2026-04-06/
    ├── spec.md          # Requirements and constraints
    ├── plan.md          # Sequenced implementation checklist
    ├── progress.json    # Machine-readable step completion status
    └── context.md       # Accumulated context from implementation
```

**Plan Format:**

```markdown
# Plan: Add Stripe Billing

## Steps

- [ ] 1. Add `stripe` crate to Cargo.toml with webhook and checkout features
- [ ] 2. Create `src/billing/mod.rs` with `BillingService` trait
- [ ] 3. Implement `StripeBillingService` with usage metering
- [x] 4. Add webhook handler for `invoice.paid` and `invoice.payment_failed`
- [ ] 5. Write integration tests with Stripe test mode
- [ ] 6. Add `/billing` CLI command for status display
```

**Worktree-Isolated Implementation (Rust Advantage):**

Each epic step executes in an isolated Git worktree via the existing `Agent` + `EnterWorktree` tools:
- Sub-agent spawns into a clean worktree branch
- Implements one plan step at a time
- Runs tests and linter in isolation
- Main agent reviews the diff via `SendMessage`
- Only merges back when tests pass — never trashes the developer's working directory

**Cross-Session Resumption:**

```bash
/epic list                    # Show all epics with progress
/epic resume stripe-billing   # Continue from last checkpoint
/epic status                  # Show current epic's remaining steps
```

Epics persist to disk. The agent reads `progress.json` on resume to pick up exactly where it left off, even across different sessions.

**Implementation Tasks:**
- [ ] Add `/epic` slash command with subcommands: `create`, `resume`, `list`, `status`, `abandon`
- [ ] Implement spec generation: agent analyzes codebase and generates requirements document
- [ ] Implement plan generation: agent breaks spec into sequenced, testable steps
- [ ] Add `.agent-code/epics/` directory management with UUID-based epic IDs
- [ ] Add `progress.json` format: step index, status, timestamp, worktree branch, commit SHA
- [ ] Wire into `Coordinator` + `EnterWorktree`: each step spawns isolated sub-agent
- [ ] Implement diff review gate: main agent reviews sub-agent's changes before merge
- [ ] Add test verification gate: merge only if tests pass in worktree
- [ ] Cross-session resume: read progress.json, rebuild context from spec + plan + completed steps
- [ ] Add context accumulation: append key decisions and learnings to `context.md` per step
- [ ] Tests: epic creation, step execution, resume after interruption, worktree cleanup

### 7.13 Provider Prompt Caching

**Priority: High** | **Target: v1.1**

Every turn currently sends the full system prompt, memory, project context, and tool definitions — burning tokens and increasing latency. API-level prompt caching can reduce costs by up to 90% for long sessions.

**Cacheable Segments:**

| Segment | Change Frequency | Cache Strategy |
|---------|-----------------|----------------|
| System prompt | Never (within session) | Cache with `ephemeral` block |
| Tool definitions | Never | Cache with system prompt |
| Memory files (MEMORY.md + topics) | Rarely | Cache, invalidate on memory write |
| Project context (.agent-code/CONTEXT.md) | Never | Cache with system prompt |
| Large file reads (>10KB) | Per-read | Cache per content hash |
| Conversation history (older turns) | Per-turn | Sliding window cache |

**Anthropic Prompt Caching:**

Tag cacheable segments with `cache_control: { type: "ephemeral" }` in the API request:

```json
{
  "system": [
    {
      "type": "text",
      "text": "<system prompt + tools + memory + context>",
      "cache_control": { "type": "ephemeral" }
    }
  ],
  "messages": [...]
}
```

**Cache Hit Tracking:**

The API response includes `cache_read_input_tokens` and `cache_creation_input_tokens`. Track and display these:

```text
Turn 3: 45,231 tokens (38,000 cached, 7,231 new) — $0.002 (90% savings)
```

**Cache Invalidation:**

- System prompt cache: invalidated only on tool definition change or memory write
- File content cache: keyed by SHA256 hash of content — auto-invalidates on file modification
- Conversation cache: sliding window — old turns fall off naturally

**Implementation Tasks:**
- [ ] Add `CacheStrategy` to `llm/client.rs` — tag system prompt, tools, memory with cache control headers
- [ ] Implement per-provider cache support: Anthropic (`cache_control`), Google (Context Caching API), OpenAI (automatic)
- [ ] Add cache hit tracking: parse `cache_read_input_tokens` from API response
- [ ] Display cache hit rate in `/cost` output and per-turn summary
- [ ] Add content-hash-based cache keys for large file reads
- [ ] Implement cache invalidation on memory write, tool definition change, file modification
- [ ] Add `[caching]` config section with `enabled` toggle and `min_segment_tokens` threshold
- [ ] Tests: verify cache headers are sent, verify cache hit rate calculation, verify invalidation triggers

### 7.14 Local LLM Auto-Discovery

**Priority: Medium** | **Target: v1.1**

When no API key is configured, silently detect locally running LLM servers and offer them as zero-configuration fallbacks.

**Discovery Protocol:**

On startup (if no API key found), probe default ports:

| Port | Server | Detection |
|------|--------|-----------|
| 11434 | Ollama | `GET /api/tags` → list available models |
| 1234 | LM Studio | `GET /v1/models` → OpenAI-compatible |
| 8080 | llama.cpp server | `GET /health` |
| 5000 | text-generation-webui | `GET /v1/models` |

**User Prompt:**

```text
No API key found, but a local Ollama server was detected with these models:
  1. qwen2.5-coder:32b (recommended for coding)
  2. llama3.1:70b
  3. codestral:22b

Use local model? [1/2/3/n]:
```

**Auto-Configuration:**

If user accepts, auto-populate config without requiring manual setup:

```toml
# Auto-generated — detected Ollama at localhost:11434
[api]
model = "qwen2.5-coder:32b"
base_url = "http://localhost:11434/v1"
# No API key needed for local models
```

**Implementation Tasks:**
- [ ] Add `crates/lib/src/services/discovery.rs` with port-scanning logic
- [ ] Implement probe functions for Ollama, LM Studio, llama.cpp, text-generation-webui
- [ ] Add model listing from each server's API
- [ ] Add interactive prompt on first run when no API key is found
- [ ] Auto-generate config file from selected local model
- [ ] Add `--discover-local` CLI flag to manually trigger discovery
- [ ] Add timeout (500ms per port) to prevent slow startup
- [ ] Tests: mock local server responses, verify config generation

### 7.15 Conversation Branching

**Priority: Medium** | **Target: v1.2**

Agents frequently go down rabbit holes — attempting failing approaches that pollute the context window. Conversation branching lets users fork, rewind, and try different approaches without losing prior successful context.

**Commands:**

```bash
/session branch fix-attempt-2      # Create named branch from current point
/session checkout 5                # Rewind to turn 5 (keep later turns in a branch)
/session branches                  # List all branches with turn counts
/session merge fix-attempt-2       # Merge a branch's successful context back
/session diff fix-attempt-2        # Show what changed between current and branch
```

**Data Model:**

```rust
struct SessionTree {
    trunk: Vec<Message>,           // Main conversation
    branches: HashMap<String, SessionBranch>,
}

struct SessionBranch {
    name: String,
    fork_point: usize,             // Turn number where branch diverged
    messages: Vec<Message>,        // Messages after fork point
    created_at: DateTime<Utc>,
    status: BranchStatus,          // Active, Merged, Abandoned
}
```

**Branching Workflow:**

```text
Turn 1 → Turn 2 → Turn 3 → Turn 4 (failing approach)
                      │
                      └──▶ Branch "alt-approach"
                            Turn 3b → Turn 4b (working approach)
                            
/session checkout 2                    # Rewind to Turn 2
/session merge alt-approach            # Bring in Turn 3b + 4b from branch
```

**Implementation Tasks:**
- [ ] Add `SessionTree` and `SessionBranch` structs to `services/session.rs`
- [ ] Implement `/session branch <name>` — fork at current turn
- [ ] Implement `/session checkout <turn>` — rewind, auto-branch current state
- [ ] Implement `/session branches` — list with fork points and status
- [ ] Implement `/session merge <name>` — replay branch messages onto trunk
- [ ] Implement `/session diff <name>` — show tool calls and file changes between branches
- [ ] Persist branches to session JSON (backward-compatible: old sessions have no branches)
- [ ] Tests: branch creation, checkout, merge, concurrent branches

### 7.16 Proactive Security Scanning

**Priority: Medium** | **Target: v1.2**

The current permission system protects the OS from the agent. Proactive security scanning protects the *codebase* from vulnerabilities — both in dependencies and in generated code.

**Automated Dependency Auditing:**

Run security scanners automatically before commits:

```toml
[security.scanning]
enabled = true
on_commit = true         # Scan before every /commit
on_session_start = false  # Optional: scan at session start

# Auto-detect scanner based on project type
# Supported: cargo-audit, npm audit, pip-audit, osv-scanner
```

**Scanning Pipeline:**

```text
Developer runs /commit
    │
    ▼
Pre-commit hook triggers security scan
    │
    ├─ cargo audit (Rust projects)
    ├─ npm audit (Node.js projects)
    ├─ pip-audit (Python projects)
    └─ osv-scanner (universal)
    │
    ▼
If vulnerabilities found:
    │
    ├─ Low/Medium: warn and continue
    └─ High/Critical: block commit, generate remediation patch
        │
        ▼
    Agent auto-generates fix:
    ├─ Bump dependency version
    ├─ Apply security patch
    └─ Add vulnerability comment explaining risk
```

**Code Pattern Scanning:**

Beyond dependencies, scan generated code for common vulnerability patterns:

| Pattern | Detection | Auto-Fix |
|---------|-----------|----------|
| SQL injection | Raw string interpolation in queries | Parameterized query |
| Command injection | User input in shell commands | Escape/allowlist |
| Hardcoded secrets | Regex patterns for API keys, passwords | Environment variable |
| Path traversal | Unsanitized file paths | Canonicalization |
| Insecure deserialization | `pickle.loads`, `eval()` | Safe alternatives |

**Implementation Tasks:**
- [ ] Add `[security.scanning]` config section
- [ ] Implement scanner auto-detection based on project files (Cargo.toml, package.json, requirements.txt)
- [ ] Add `cargo audit` / `npm audit` / `pip-audit` / `osv-scanner` integration
- [ ] Wire into `/commit` skill as pre-commit check
- [ ] Implement severity-based blocking (High/Critical blocks, Low/Medium warns)
- [ ] Add auto-remediation: parse audit JSON, generate dependency bump patch
- [ ] Add code pattern scanning: regex-based detection of OWASP top 10 patterns in modified files
- [ ] Add `/security-scan` slash command for manual scanning
- [ ] Tests: mock audit output, verify blocking behavior, verify remediation patches

### 7.17 Hierarchical Context Resolution

**Priority: Medium** | **Target: v1.1**

Currently, project context is loaded from a single root-level file. For monorepos and deep directory structures, context should be merged hierarchically based on the agent's current working focus.

**Resolution Order:**

When working on `frontend/components/Button.tsx`, load and merge:

```text
.agent-code/CONTEXT.md          ← Global rules (code style, architecture)
frontend/.agent-code/CONTEXT.md  ← Frontend-specific (React patterns, component conventions)
frontend/components/.agent-code/CONTEXT.md  ← Component-specific (if exists)
```

**Merge Strategy:**

- Later (more specific) files override earlier (more general) ones
- Sections are merged by heading — a frontend CONTEXT.md can override the "Code Style" section without affecting "Architecture"
- Array values (e.g., "tools to use") are concatenated, not replaced

**Auto-Detection:**

The agent determines working focus from:
1. Files recently read or edited (last 3 turns)
2. Current `/cd` directory
3. Explicit `@` file references in the prompt

**Implementation Tasks:**
- [ ] Modify context loader in `services/` to traverse directory tree upward
- [ ] Implement section-based merge strategy (by markdown heading)
- [ ] Add working focus detection from recent file operations
- [ ] Cache resolved context per directory (invalidate on file change)
- [ ] Add `/context` slash command showing resolved context with source annotations
- [ ] Tests: multi-level merge, section override, array concatenation

### 7.18 Shell Passthrough Mode

**Priority: Medium** | **Target: v1.1**

When input begins with `!`, execute the command directly as a subprocess and silently append the output to the conversation context. This keeps developers in control without burning tokens for trivial commands, while giving the agent the context of build failures or test output.

**Behavior:**

```text
> ! cargo test
running 45 tests
test auth::test_login ... ok
test billing::test_charge ... FAILED

45 tests, 1 failure

> The billing test is failing — can you see why?
(Agent now has the test output in context without needing to run Bash tool)
```

**Implementation:**

- Detect `!` prefix in REPL input
- Spawn subprocess with inherited stdin/stdout/stderr (user sees output in real-time)
- Capture stdout+stderr into buffer
- Append as a `System` message to conversation: `[Shell output from: cargo test]\n<output>`
- Truncate output at 50KB (same as Bash tool limit)
- Do NOT count as a turn or consume LLM tokens

**Implementation Tasks:**
- [ ] Add `!` prefix detection in REPL input handler
- [ ] Spawn subprocess with `Stdio::inherit()` for real-time output
- [ ] Capture output via `tee`-like approach (display + buffer simultaneously)
- [ ] Append captured output as system message to conversation history
- [ ] Add output truncation at 50KB with "[truncated]" marker
- [ ] Skip permission check (user explicitly invoked the command)
- [ ] Tests: verify output capture, verify context injection, verify truncation

### 7.19 Voice Mode

- [ ] STT integration for voice input
- [ ] TTS integration for spoken responses
- [ ] Push-to-talk keybinding

### 7.20 Scheduled Agents

- [ ] Cron-based agent execution (`agent cron add "0 9 * * *" --prompt "run tests"`)
- [ ] Remote trigger via webhook (`agent trigger --listen :8080`)
- [ ] Background daemon for scheduled runs

### 7.21 Web and Desktop Clients

- [ ] Headless API server mode (`agent serve`) as the foundation
- [ ] Web client connecting via HTTP + SSE
- [ ] Desktop client via Tauri v2 wrapping the web client
- [ ] All clients share the same backend

---

## Performance Targets — In Progress

| Metric | Before | After | Target (v1.0) | Status |
|--------|--------|-------|---------------|--------|
| Startup time | ~200ms (blocked by sync curl) | <150ms (async key check) | <150ms | **Done** |
| Tool dispatch overhead | unmeasured | unmeasured | <1ms per tool call | Planned |
| Binary size (release) | ~15MB | est. <12MB (codegen-units=1, panic=abort, narrowed tokio) | <12MB | **Done** |
| Context compaction latency | unmeasured | benchmarked | <500ms for microcompact | **Done** |
| Tests | 232+ | 400+ | 400+ | **Done** |
| Test coverage | Codecov active | Codecov active | >70% | In Progress |
| Supported platforms | 6 (Linux, macOS, Windows, Docker, npm) | 6 | 7 (+ homebrew-core) | Planned |

### Changes Made

- **Startup**: Moved blocking `curl` API key validation (5s timeout) to async background task with 500ms non-blocking window
- **Binary size**: Added `codegen-units = 1` and `panic = "abort"` to release profile; narrowed tokio features from `"full"` to specific needed features
- **Tests**: Added 170+ new unit and integration tests across error types, config schema, memory types, LLM providers, permissions, message normalization, retry logic, token estimation, compaction, pricing, and bash parsing
- **Benchmarks**: Added startup benchmark (config loading) alongside existing compaction and token estimation benchmarks

---

## Contributing

Want to help? Pick any unchecked item above and open an issue to discuss the approach before starting. See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup.

Priority areas where contributions are most welcome:

### Critical (v1.1)
1. **Process-level sandboxing** (7.4) — Linux bubblewrap and macOS seatbelt implementations
2. **Behavioral eval framework** (7.5) — Test harness with best-of-N retry logic
3. **Model routing & fallback** (4.3) — Availability state machine and fallback chains

### High Priority (v1.1)
4. **Epic workflows** (7.12) — Stateful plan-based implementation with worktree isolation
5. **Provider prompt caching** (7.13) — Anthropic cache_control headers, cache hit tracking
6. **Advanced compression** (7.6) — File-level tracking with secret masking
7. **Non-interactive JSON streaming** (7.7) — JSONL output format for CI/CD

### Medium Priority (v1.2)
8. **GitHub Action** (7.10) — First-party action for PR review and issue triage
9. **OAuth authentication** (7.11) — Browser-based auth with OS keychain storage
10. **Local LLM auto-discovery** (7.14) — Zero-config detection of Ollama, LM Studio
11. **Remote agent protocol** (7.8) — JSON-RPC 2.0 with Agent Card discovery
12. **Conversation branching** (7.15) — Fork, rewind, and merge conversation states

---

*Last updated: 2026-04-06*
