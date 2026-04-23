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

### 1.10 Compatibility Matrix

Living document comparing agent-code feature coverage against the broader terminal coding-agent landscape. Helps prospective users understand tradeoffs and gives us an honest internal gap tracker.

- [ ] Create `docs/compat-matrix.mdx` with rows for features (tools, sandboxing, MCP, LSP, multi-agent, providers, platforms, session resume, image input, profiles, etc.) and columns for agent-code plus 3–5 peer projects
- [ ] Wire into Mintlify nav under "Reference"
- [ ] Add a CI check that fails if a new `feature:` tag is added to source without a matching row in the matrix
- [ ] Refresh per release as part of the CHANGELOG workflow

### 1.11 Public SDK Documentation

`crates/lib` is already a reusable library, but it is not marketed or documented as a stable embedding surface. Give it a first-class SDK story so third-party Rust apps can embed the agent loop.

- [ ] Introduce a top-level `agent_code::sdk` facade module re-exporting the minimal public types (session, tool trait, provider trait, config)
- [ ] Publish a `docs/sdk.mdx` quickstart with an end-to-end embedding example
- [ ] Add an `examples/embed-agent/` crate in the workspace that builds in CI
- [ ] Declare a semver contract for the `sdk` module (separate from internal types)
- [ ] Annotate non-SDK modules with `#[doc(hidden)]` where appropriate

---

## Phase 2: Skills and Extensibility

**Priority: High** | **Target: v0.11**

### 2.1 New Bundled Skills — Done

- [x] **`security-review`** — OWASP vulnerability scan with severity ratings
- [x] **`advisor`** — Architecture and dependency health analysis
- [x] **`bughunter`** — Systematic bug search with reproduction steps
- [x] **`plan`** — Structured implementation planning with risk flags
- [x] **`remember`** — Save insights to user memory with correct two-step write discipline
- [x] **`stuck`** — Step back and try a different angle when the agent is looping
- [x] **`simplify`** — Review-then-simplify pass flagging dead weight in the current diff
- [x] **`batch`** — Apply the same change across multiple git worktrees
- [x] **`skillify`** — Extract a productive session into a reusable skill file

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

### 3.5 Self-Management Commands — Done

- [x] **`/update`** — Check GitHub releases API, notify if newer version
- [x] **`/uninstall`** — Remove binary, config, and data directories with confirmation

### 3.6 Productivity Commands — Done

- [x] **`/effort`** — Rate task complexity XS/S/M/L/XL with risks
- [x] **`/break-cache`** — Force next request to skip the prompt cache
- [x] **`/heapdump`** — Process memory snapshot (hidden, for bug reports)
- [x] **`/btw`** — Quick-capture notes to user memory without a turn
- [x] **`/rename`** — Label the current session; shown in `/sessions`
- [x] **`/add-dir`** — Track additional directories alongside cwd
- [x] **`/thinkback`** — View extended-thinking blocks from a recent turn
- [x] **`/usage`** — Per-turn token timeline with cache-hit rate

### 3.7 PR-Workflow Commands — Done

- [x] **`/pr-comments`** — Fetch + triage PR review comments via `gh`
- [x] **`/autofix-pr`** — Checkout PR → lint/test → fix → push, worktree-isolated
- [x] **`/perf-issue`** — Report-only perf regression audit on the current diff

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

### 4.3 Model Routing and Automatic Fallback

Automatically detect model unavailability and route to alternatives without user intervention.

**Availability State Machine:**

Each model tracks health via a 3-state machine:
- `healthy` — model is available, use normally
- `sticky_retry` — transient error occurred; allow one retry per turn, then skip
- `terminal` — permanent unavailability (quota exhausted, model deprecated, capacity limit)

State transitions:
```text
healthy ──[transient error]──▶ sticky_retry ──[consumed]──▶ skip this turn
healthy ──[permanent error]──▶ terminal ──[never recovers within session]
sticky_retry ──[turn boundary]──▶ healthy (reset for next turn)
```

**Composite Routing Strategy:**

Evaluate strategies in priority order; first match wins:
1. **OverrideStrategy** — honor user `/model` override regardless of routing (always wins)
2. **FallbackStrategy** — check availability state; if unhealthy, select next model in chain
3. **CostStrategy** — route to cheaper model when task is simple (e.g., file listing vs. architecture refactor)
4. **DefaultStrategy** — use configured model

**Fallback Chain Configuration:**

```toml
[routing]
fallback_chain = ["primary-model", "secondary-model", "tertiary-model"]
cost_threshold_usd = 0.50  # Switch to cheaper model after this session cost

[routing.availability]
retry_on_transient = true
terminal_on_quota = true
```

**Cross-feature interaction:** Model switches invalidate prompt cache (see 7.13). Every provider's cache is model-specific. The routing service should notify the cache strategy when a fallback triggers, so the cache layer can log the cost impact and pre-warm the new model's cache.

**Implementation Tasks:**
- [ ] Add `ModelAvailabilityService` to `crates/lib/src/services/` with health state tracking per model
- [ ] Add `ModelRouterService` to `crates/lib/src/services/` with ordered strategy evaluation
- [ ] Add `fallback_chain` and `routing` sections to `ConfigSchema`
- [ ] Wire router into `query/mod.rs` loop — on LLM error, consult router before retrying
- [ ] Add `resetTurn()` call at turn boundary to clear sticky consumption flags
- [ ] Log routing decisions with reason (e.g., "Model X unavailable (quota). Falling back to Y")
- [ ] Notify cache strategy on model switch (see 7.13) to track cache invalidation cost
- [ ] Add `/routing` slash command showing current model health states
- [ ] Tests: unit tests for state machine transitions, integration tests for chain traversal

**Reasoning-Effort Controls:**

Providers increasingly expose a reasoning/thinking budget knob (Anthropic `thinking.budget_tokens`, OpenAI `reasoning.effort`, xAI reasoning modes). Wire this into the routing layer so it can be set per-mode and per-turn.

- [ ] Add `reasoning_effort: low | medium | high | none` to `ConfigSchema` with provider-specific mapping
- [ ] Per-mode defaults: plan mode → `high`, build mode → `medium`, one-shot headless → `low`
- [ ] `--effort <level>` flag for single-shot invocations; `/effort <level>` slash command for mid-session override
- [ ] Report the active effort level in `/cost` alongside token usage so users can correlate cost spikes with deep reasoning

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

### 5.5 Per-Agent Permissions — Done

- [x] Extend permission system to support per-agent rule sets (not just global) — `AgentDefinition.permissions: Option<PermissionsConfig>`
- [x] Plan mode gets its own permission model (read-only by default, configurable) — via `read_only: true` + optional `permissions` overlay in the agent markdown file
- [x] Subagents can have restricted tool access — `--permissions-overlay <path>` CLI flag, temp TOML written by the coordinator on spawn, gated by `security.disable_bypass_permissions`

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

### 7.4 Process-Level Sandboxing — Mostly Done

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
- [x] Add `crates/lib/src/sandbox/mod.rs` — trait `SandboxStrategy` with `fn wrap_command(&self, cmd: Command, policy: &SandboxPolicy) -> Command`
- [x] Add `crates/lib/src/sandbox/seatbelt.rs` — macOS implementation using `sandbox-exec -f <profile>`
- [x] Add `crates/lib/src/sandbox/bwrap.rs` — Linux implementation using `bwrap` binary
- [ ] Add `crates/lib/src/sandbox/windows.rs` — Windows Low Integrity via `icacls` + restricted token (deferred — Windows falls back to NoopStrategy)
- [x] Add `crates/lib/src/sandbox/policy.rs` — `SandboxPolicy` struct parsed from config TOML
- [x] Add `SandboxConfig` section to `ConfigSchema` with `enabled`, `strategy`, `allowed_write_paths`, `forbidden_paths`, `secret_patterns`
- [x] Wire into process-creation sites: `bash.rs` (Command::new), `agent.rs` (build_agent_command), `powershell.rs` — NOT executor.rs (which calls trait methods, not subprocesses)
- [x] Create `SandboxedCommand` wrapper that intercepts `std::process::Command` construction and prepends sandbox args (bwrap/sandbox-exec)
- [x] Per-tool override: `sandbox.tools.<ToolName>` config for network/sandbox toggles
- [x] Auto-detect platform and select strategy in `SandboxStrategy::detect()`
- [x] Secret masking: scan project + allowed paths for files matching `secret_patterns`, mask them
- [x] Git worktree detection: resolve `.git` files to actual git directories, add to allowed paths
- [x] Symlink resolution: resolve both canonical and symlink paths to prevent bypass
- [x] Fallback behavior: if sandbox binary not found (`bwrap` / `sandbox-exec`), warn and run unsandboxed with degraded security notice
- [ ] `/sandbox` slash command showing current strategy, allowed paths, masked files
- [ ] Add `--no-sandbox` CLI flag to disable (requires `disable_bypass_permissions = false`)
- [x] Tests: unit tests for policy parsing, integration tests for each platform strategy
- [ ] Benchmark: measure sandbox overhead per tool call (target: <10ms)

**Future: gVisor (runsc) for Linux**

bubblewrap provides namespace isolation (process, mount, network) but does NOT filter syscalls. A compromised process inside bwrap can still make arbitrary syscalls. gVisor intercepts all syscalls in userspace via its Go-based kernel, providing the strongest isolation available on Linux. Deferred because it requires Docker runtime (`--runtime=runsc`), adding a heavy dependency. Ship bwrap first, add gVisor as an optional upgrade for high-security environments.

### 7.5 Behavioral Evaluation Framework — Done

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
        model: [default-model, fallback-model]
    steps:
      - run: cargo run --bin eval_runner -- --policy usually_passes --retries 3
      - uses: actions/upload-artifact@v4
```

**Automatic Promotion:**

A scheduled job reviews nightly results weekly:
- If a `UsuallyPasses` eval has passed 100% for 7+ consecutive days → auto-promote to `AlwaysPasses`
- If an `AlwaysPasses` eval fails 2+ times in a week → auto-demote to `UsuallyPasses` with alert

**Implementation Tasks:**
- [x] Create `evals/` directory at repo root with harness module
- [x] Add `eval_runner` binary to `crates/eval/` (separate from main CLI binary)
- [x] Implement `TestRig` with workspace setup, tool log capture, activity logging
- [x] Implement `eval_def!` macro for declarative eval definitions (named `eval_def!` not `eval_test!`)
- [x] Implement best-of-N retry logic with transient error detection
- [ ] Implement breakpoint mechanism: intercept tool dispatch, pause, inject hint
- [x] Add `EvalPolicy` enum and tier enforcement (`AlwaysPasses`, `UsuallyPasses`)
- [x] Write 10 seed evals: file creation, file editing, multi-file creation, grep usage, bash execution, error recovery, coding task, and more
- [x] Add `evals/fixtures/` with starter project templates (`empty_project`, `rust_project`)
- [x] Create `.github/workflows/evals-nightly.yml` with nightly schedule (2 AM UTC)
- [x] Create `.github/workflows/evals-promotion.yml` for weekly auto-promotion/demotion
- [x] Add trustworthiness tracking: JSONL log of pass/fail per eval per night
- [x] Add `cargo run --release -p agent-code-eval -- --list` to show all evals
- [x] Add `cargo run --release -p agent-code-eval -- --policy <tier>` to filter by policy

### 7.6 Advanced History Compression — Mostly Done

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
- [x] Add `FileCompressionRecord` struct to `services/compact.rs`
- [x] Add `CompressionLevel` enum: `Full`, `Partial`, `Summary`, `Excluded`
- [x] Implement content hashing (12-byte SHA256 slice) for change detection
- [x] Implement protected file mechanism: lock recent 2-turn file reads (`PROTECTED_TURN_WINDOW = 2`)
- [x] Add `SecretMasker` module with shared regex patterns (reusable across all write boundaries)
- [x] Apply `SecretMasker` at ALL persistence points, not just compression:
  - [x] Before LLM summarization (compression path)
  - [x] In `session.rs` when serializing session JSON to disk
  - [x] In `output_store.rs` when writing large tool results to disk
  - [ ] In `/share` command when exporting transcripts
- [x] Add compression state serialization to session persistence (`save()`/`load()` helpers)
- [ ] Wire into existing auto-compact: after summarization, update file records (needs query-loop integration)
- [ ] Add `/compression` slash command showing file compression states
- [x] Tests: verify secrets are redacted in summaries, session files, disk outputs, and exports (38 tests)

### 7.7 Non-Interactive Structured Output

**Priority: Medium** | **Target: v1.1**

The current `--prompt` one-shot mode outputs plain text. For CI/CD pipelines, GitHub Actions, and tool-chaining, structured JSON streaming is essential.

**JSON Streaming Format:**

Each event is a single JSON line (JSONL) written to stdout:

```jsonl
{"type":"session_start","session_id":"abc-123","model":"your-preferred-model","timestamp":"2026-04-06T12:00:00Z"}
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

**`--bare` hermetic mode:**

For fully scripted `-p`/`--prompt` invocations in CI, a hermetic mode that disables every user-configurable surface so the run is reproducible across machines.

- [ ] `--bare` flag that skips loading: hooks, LSP, plugins, skills from user/project dirs, MCP servers
- [ ] Refuses to launch without an explicit API key in env (no interactive auth fallback)
- [ ] Implies `--output-format json` unless overridden
- [ ] Document the exact set of disabled surfaces in `docs/reference/bare-mode.mdx`

**`defer` permission decision:**

- [ ] Extend the permission decision enum with `Defer` in addition to `Allow`/`Deny`/`Ask`
- [ ] In headless mode, a `Defer` decision pauses the session, serializes state, and exits with a new exit code (8) signaling "resumable pause"
- [ ] `agent --resume <session-id>` picks up where a deferred session left off, replaying the pending tool call
- [ ] Useful for CI that wants a human to approve a destructive step mid-pipeline

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
{"type":"llm_request","data":{"model":"your-preferred-model","input_tokens":15234,"latency_ms":2340},"timestamp":"..."}

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
          model: your-preferred-model
          max-cost-usd: 1.0
          output-format: json
        env:
          LLM_API_KEY: ${{ secrets.LLM_API_KEY }}
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

### 7.11 OAuth Authentication

**Priority: Medium** | **Target: v1.2**

API key management is friction for new users. Adding OAuth-based authentication with OS keychain storage simplifies onboarding for users who already have provider accounts.

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

### 7.13 Provider Prompt Caching — Done

- [x] System prompt cached with `cache_control: { type: "ephemeral" }` (`anthropic.rs:86-95`, `client.rs:189-198`)
- [x] Conversation history breakpoints via `messages_to_api_params_cached()` (`message.rs:459-504`)
- [x] Tool definitions cached with `cache_control` on last tool (`anthropic.rs:74-84`, `client.rs:176-187`) — PR #78
- [x] `Usage` struct tracks `cache_read_input_tokens` and `cache_creation_input_tokens` (`message.rs:204-230`)
- [x] SSE stream parser extracts cache token fields (`stream.rs:82-93, 318-326`)
- [x] `CacheTracker` service detects cache hits, breaks, and partial hits via fingerprinting (`cache_tracking.rs`)
- [x] `/cost` command displays cache hit percentage per model (`commands/mod.rs:402-446`)
- [x] `prompt-caching-2024-07-31` beta header enabled by default (`anthropic.rs:61-71`)
- [x] `features.prompt_caching` config toggle (default: true) to disable for unsupported providers — PR #78
- [x] System prompt rebuild uses hash-based cache to avoid unnecessary recomputation (`query/mod.rs:64`)

### 7.14 Local LLM Auto-Discovery — Partially Done

**Priority: Medium** | **Target: v1.2**

Ollama auto-detection already exists in the setup wizard (`ui/setup.rs`) — it probes `localhost:11434/api/tags` on first run when no API key is found. Remaining work: extend detection to other local servers.

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
- [x] Ollama detection in setup wizard (`ui/setup.rs`) — probes `localhost:11434/api/tags`
- [x] Interactive prompt on first run when no API key found
- [ ] Add `crates/lib/src/services/discovery.rs` — extract Ollama detection into reusable service
- [ ] Implement probe functions for LM Studio (`localhost:1234`), llama.cpp (`localhost:8080`), text-generation-webui (`localhost:5000`)
- [ ] Add model listing from each server's API
- [ ] Auto-generate config file from selected local model
- [ ] Add `--discover-local` CLI flag to manually trigger discovery
- [ ] Add timeout (500ms per port) to prevent slow startup
- [ ] Tests: mock local server responses, verify config generation

### 7.15 Conversation Branching — Partially Done

**Priority: Medium** | **Target: v1.2**

Basic forking exists via `/fork` command (`commands/mod.rs:1128`, `features.fork_conversation` config flag). It saves the current conversation with a new session ID and allows resuming with `/resume`. The remaining work is the advanced branching model: named branches, checkout to specific turns, merge, and diff.

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
- [x] `/fork` command saves current conversation and creates new session ID (`commands/mod.rs:1128`)
- [x] `/resume` command restores forked sessions
- [x] `fork_conversation` feature flag in config (default: true)
- [ ] Add `SessionTree` and `SessionBranch` structs to `services/session.rs`
- [ ] Upgrade `/fork` to support named branches (`/session branch <name>`)
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

**Priority: Medium** | **Target: v1.2**

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

### 7.18 Shell Passthrough Context Injection — Done

**Priority: Medium** | **Target: v1.2**

The `!` prefix already exists in the REPL (`crates/cli/src/ui/repl.rs:603`) and runs shell commands directly. However, the current implementation has two limitations:

1. **No context injection** — output is printed but NOT appended to conversation history, so the agent can't reference it in subsequent turns
2. **No real-time streaming** — uses `.output()` (captures after completion) instead of `Stdio::inherit()`, so long-running commands show no output until they finish

**Upgraded Behavior:**

```text
> ! cargo test
running 45 tests                    ← streams in real-time
test auth::test_login ... ok
test billing::test_charge ... FAILED

45 tests, 1 failure

> The billing test is failing — can you see why?
(Agent now has the test output in context without needing to run Bash tool)
```

**Implementation Tasks:**
- [x] Switch from `.output()` to real-time streaming with async line-by-line capture
- [x] Capture stdout+stderr into buffer while displaying in real-time
- [x] Append captured output as system message to conversation history
- [x] Add output truncation with marker
- [x] Do NOT count as a turn or consume LLM tokens
- [x] Tests: comprehensive test suite (commit 78c7d21)

### 7.19 Voice Mode

- [ ] STT integration for voice input
- [ ] TTS integration for spoken responses
- [ ] Push-to-talk keybinding

### 7.20 Scheduled Agents — Done

- [x] Cron-based agent execution (`crates/lib/src/schedule/cron.rs` with cron parsing)
- [x] Remote trigger via webhook (`crates/lib/src/schedule/` with webhook_secret + HTTP server)
- [x] Background daemon for scheduled runs (`crates/cli/src/daemon.rs`)

### 7.21 Web and Desktop Clients — Mostly Done

- [x] Headless API server mode (`agent serve`) as the foundation (`crates/cli/src/serve.rs`)
- [x] Web client connecting via WebSocket (Flutter web build in `client/`)
- [x] Desktop client via Flutter (cross-platform: macOS, Linux, web — `client/` directory)
- [x] Use JSON-RPC over WebSocket for bidirectional IPC between Flutter client and agent-code backend
- [x] All clients share the same backend

**Phased delivery:**

- [x] Phase A — headless `--serve` with WebSocket + JSON-RPC
- [x] Phase B — Flutter desktop/web client connecting over local WebSocket
- [ ] Phase C — static web client served from the daemon, gated behind signed-token auth
- [ ] Security default: bind to `127.0.0.1` only; LAN exposure requires an explicit `--listen` flag plus a generated bearer token stored in the OS keychain
- [ ] Document the session protocol as a stable contract so third-party clients can be built against it

### 7.22 Reversible Edit History

Every file-mutating tool call is recorded to a per-session edit log so the user can roll back or replay changes without leaving the REPL. Complements git worktree isolation for in-flight experimentation.

- [ ] Append-only edit journal under `.agent-code/edits/<session-id>.log` capturing `{tool_call_id, path, pre_hash, post_hash, patch}`
- [ ] `/undo [n]` reverts the last `n` tool-call groups (default 1); `/redo [n]` re-applies them
- [ ] Grouping boundary = one assistant turn, so a single `/undo` rolls back a multi-file edit atomically
- [ ] Persisted across `--resume`; truncated on explicit `/reset`
- [ ] Refuse to undo past a git commit made during the session (print guidance to use `git revert` instead)
- [ ] Open question: interaction with external edits between tool calls — detect via pre-edit hash and prompt before overwriting

### 7.23 Multimodal Input in TUI

Vision-capable providers already accept image content blocks. Expose that in the REPL so users can drop in screenshots, design specs, or error screenshots without leaving the terminal.

- [ ] Clipboard paste of images (`Ctrl+V` when an image is on the system clipboard) attached to the next user turn
- [ ] Drag-and-drop of image files onto the terminal window on platforms that support `TERM_PROGRAM` file drops
- [ ] Inline `/image <path>` command as a universal fallback
- [ ] Rendered as an ASCII thumbnail in the transcript with the filename; full image shipped as a content block
- [ ] Graceful degradation for text-only providers: replace the image block with `[image: <filename>, <dimensions>]` and surface a one-time warning
- [ ] Token accounting in `/cost` reflects image tokens per provider pricing

### 7.24 Configuration Profiles

A profile is a named bundle of `{model, provider, sandbox policy, permission set, reasoning effort, tool allowlist, skills}` that can be activated at launch or mid-session. Distinct from agents (runtime personas) — profiles are launch-time execution contexts.

- [ ] `~/.config/agent-code/profiles.toml` with `[profiles.<name>]` tables
- [ ] `--profile <name>` CLI flag and `/profile <name>` slash command
- [ ] Profile inheritance: `extends = "base"` for shared defaults
- [ ] Built-in profiles: `safe` (read-only, cheap model), `dev` (full permissions, premium model), `ci` (headless, strict sandbox, low reasoning)
- [ ] `/profile` with no argument prints the active profile and its effective settings
- [ ] Interaction with 7.4 sandboxing: profile pins a sandbox policy id; profile switches re-enter the sandbox

### 7.25 Enterprise TLS and Proxy Support

Corporate environments require custom root CAs and forward proxies. Add first-class support so enterprise users do not have to monkey-patch `reqwest`.

- [ ] Honor `AGENT_CODE_CA_CERTIFICATE` and the standard `SSL_CERT_FILE` env vars, loading a PEM bundle into the HTTP client
- [ ] Respect `http_proxy`, `https_proxy`, `no_proxy` (already partially true via `reqwest`, but verified end-to-end and documented)
- [ ] Config-file equivalents: `[network] ca_certificate = "..."` and `[network] proxy = "..."`
- [ ] `/doctor` reports the resolved CA bundle path and active proxy, masking credentials
- [ ] Docs: enterprise setup page covering ZScaler, Netskope, and self-signed internal gateways
- [ ] `managed-settings.d/` drop-in policy directory: admin config fragments merged alphabetically, sitting above user config in the precedence chain so org policy cannot be accidentally overridden
- [ ] `AGENT_CODE_SUBPROCESS_ENV_SCRUB` env var (and config equivalent): strip known credential env vars (`*_API_KEY`, `*_TOKEN`, `AWS_*`) from the environment inherited by bash-tool and plugin subprocesses

### 7.26 SQLite Session Store

Session history currently lives as on-disk JSON blobs. Moving to SQLite unlocks fast resume, full-text search, and cross-session analytics without reading every file.

- [ ] `sessions.db` at `~/.local/state/agent-code/` with tables for sessions, turns, tool_calls, and artifacts
- [ ] FTS5 virtual table indexing user prompts and assistant responses for `/history search <query>`
- [ ] `/history list`, `/history show <id>`, `/history export <id> --format json`
- [ ] Migration tool reading existing JSON blobs and backfilling the database
- [ ] Gated behind a feature flag until migration has shipped for at least one release
- [ ] Size management: auto-vacuum, retention policy configurable (`sessions.retain_days`)

### 7.27 `@`-Mention File Picker

Typing `@` in the REPL opens an inline fuzzy finder over workspace files. Selecting a file inlines its path into the prompt and pre-reads it into context, removing the "ask the agent to read X" round-trip.

- [ ] Fuzzy picker component in the TUI, backed by a ripgrep-powered file walker respecting `.gitignore`
- [ ] Selected paths rendered as chips in the input line; submission inlines them as `@path/to/file`
- [ ] Agent loop pre-reads `@`-mentioned files into the next user turn as tool-free context
- [ ] Supports directories (reads a shallow listing) and glob patterns (`@src/**/*.rs` reads matching files up to a size cap)
- [ ] Configurable size cap and file-count cap to prevent context blowout

### 7.28 Turn-Lifecycle Hooks

User-configured hooks that execute on agent lifecycle events, enabling OS notifications, Slack pings, metrics emission, and arbitrary automation. Complements existing hook support by standardizing the event surface.

- [ ] Event set: `turn_start`, `turn_end`, `tool_call_start`, `tool_call_end`, `error`, `permission_prompt`, `cwd_changed`, `file_changed`, `instructions_loaded`, `task_created`, `stop_failure`, `permission_denied`, `post_compact`
- [ ] `cwd_changed` / `file_changed` enable reactive env management (direnv-style auto-reload) and targeted context refresh after external edits
- [ ] `instructions_loaded` fires when `CLAUDE.md`, `AGENTS.md`, or project rules enter context — useful for attaching provenance telemetry
- [ ] `post_compact` fires after compaction and lets hooks re-inject pinned context the compactor dropped
- [ ] `stop_failure` fires on terminal API errors (rate limit, auth) so external supervisors can retry or page
- [ ] Hooks configured in `settings.json` under `hooks.lifecycle` with a shell command and filter predicates
- [ ] JSON payload delivered on stdin with `event`, `session_id`, `turn_id`, `tool_name`, `duration_ms`, `exit_code`, plus `agent_id` / `agent_type` for subagent events
- [ ] Conditional `if` field on hooks (permission-rule syntax) for fine-grained filtering
- [ ] Built-in recipes page: macOS `terminal-notifier`, Linux `notify-send`, Slack webhook, Prometheus pushgateway
- [ ] Hooks run with a strict timeout; failures logged but do not block the agent loop

### 7.29 Project Rules

Plain-text per-project steering notes injected as high-priority system instructions. Distinct from `CLAUDE.md` (long-form documentation) and Skills (code + prompts) — rules are short, composable, and turn-scoped.

- [ ] `.agent-code/rules/*.md` directory with one rule per file, YAML frontmatter for metadata (title, priority, `glob:` activation)
- [ ] Glob-matched activation: a rule with `glob: "crates/lib/**/*.rs"` is injected only when the turn references a matching file
- [ ] `/rules list`, `/rules enable <name>`, `/rules disable <name>` for ad-hoc toggling
- [ ] Ordering: rules are injected after `CLAUDE.md` but before the user turn, with priority-based ordering inside that block
- [ ] Rules count against a configurable token budget; overflow is reported in `/cost`
- [ ] Open question: promotion path from a rule that proves useful to a full Skill

### 7.30 MCP Elicitation

MCP servers sometimes need to ask the user for structured input mid-task (a form field, a confirmation, a browser URL to complete OAuth). Today our MCP client can only receive tool results — it has no surface for the server to request input.

- [ ] Implement the MCP `elicitation/create` request on the client side, matching the current MCP spec shape
- [ ] TUI elicitation modal: render form fields (text, boolean, choice) with validation and a cancel path
- [ ] Non-interactive mode: elicitation requests emit a `permission_request`-style event in the JSON stream (§7.7) and auto-cancel unless `--auto-elicit` is set
- [ ] Browser-flow elicitation: servers may request `{type: "open_url"}` — we open the URL locally, collect the result token on a loopback callback, and return it
- [ ] Per-server `elicitation.allowed: bool` setting so admins can disable the surface

### 7.31 Skill and Plugin Validator

A first-class `agent plugin validate` CLI that schema-checks every extension-point frontmatter before load, so authors catch mistakes at write time instead of session start.

- [ ] `agent plugin validate [path]` subcommand that walks a plugin directory and validates each file
- [ ] Validates: skill `SKILL.md` frontmatter, agent frontmatter, slash-command frontmatter, `hooks.json` schema, `manifest.json`
- [ ] Schema-level checks (required fields, enum values) plus semantic checks (referenced tool names exist, glob patterns parse, permission rules are well-formed)
- [ ] Machine-readable output (`--format json`) for editor integration
- [ ] CI recipe: a GitHub Action template that runs `agent plugin validate` on every PR touching a plugin repo
- [ ] Plugin authors get a `manifest.userConfig` option system: typed config declarations with optional OS-keychain storage for sensitive values (API tokens the plugin needs)

### 7.32 TUI Rendering and Transcript Search

The existing TUI re-renders from scratch on most events, causing flicker in long sessions and making it hard to navigate back through a transcript. Ship a virtualized, flicker-free renderer and an in-transcript search.

- [ ] Alt-screen rendering with a virtualized scrollback buffer (only the visible window is redrawn)
- [ ] `NO_FLICKER=1` env var or `tui.no_flicker` setting to opt in while the mode bakes
- [ ] `/` in the REPL enters transcript-search mode; `n` and `N` navigate matches; `Esc` exits
- [ ] `Ctrl+O` toggles a "focus view" that hides system messages and tool results, leaving only user and assistant turns
- [ ] `/recap` slash command: re-reads recent transcript, summarizes what happened, and re-injects the summary at the top of context — useful when returning to a resumed session
- [ ] Status line `refreshInterval` setting so users can run a command periodically (git status, cost, clock) without full re-render
- [ ] Interop with §7.26 SQLite store: search scope can be widened from "this session" to "all sessions"

**Ergonomic additions (small):**

- [ ] Agent `initialPrompt` frontmatter: launches an agent that auto-submits a first user turn (useful for preset research or review agents)
- [ ] `effort: low | medium | high` frontmatter on skills and agents, overriding the session-level reasoning effort from §4.3
- [ ] Bash permission rules support wildcard matching (`Bash(git log:*)`, `Bash(cargo test:*)`) tolerant of leading whitespace and multi-space arguments — verify current matcher and harden if needed

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
1. ~~**Process-level sandboxing** (7.4)~~ — **Done** (macOS seatbelt + Linux bwrap; Windows deferred)
2. ~~**Behavioral eval framework** (7.5)~~ — **Done** (eval_runner, 10 seed evals, nightly CI, promotion/demotion)
3. **Model routing & fallback** (4.3) — Availability state machine and fallback chains

### High Priority (v1.1)
4. **Epic workflows** (7.12) — Stateful plan-based implementation with worktree isolation
5. ~~**Provider prompt caching** (7.13)~~ — **Done** (PR #78)
6. ~~**Advanced compression** (7.6)~~ — **Done** (PR #128 — file tracking, secret masker at all persistence boundaries)
7. **Non-interactive JSON streaming** (7.7) — JSONL output format for CI/CD

### Also shipped (not in original priorities)
- ~~**Shell passthrough** (7.18)~~ — **Done** (real-time streaming + context injection)
- ~~**Scheduled agents** (7.20)~~ — **Done** (cron + webhooks + daemon)
- ~~**Web/desktop clients** (7.21)~~ — **Done** (Flutter client + WebSocket JSON-RPC backend)

### Medium Priority (v1.2)
8. **GitHub Action** (7.10) — First-party action for PR review and issue triage
9. **OAuth authentication** (7.11) — Browser-based auth with OS keychain
10. **Local LLM auto-discovery** (7.14) — Extend to LM Studio, llama.cpp (Ollama already detected)
11. **Remote agent protocol** (7.8) — JSON-RPC 2.0 with Agent Card discovery
12. **Conversation branching** (7.15) — Named branches, checkout, merge (basic `/fork` exists)

---

*Last updated: 2026-04-15*
