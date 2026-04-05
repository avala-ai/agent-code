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
- [ ] Add `///` doc comments to ~30 key public structs and functions
- [ ] Publish rustdoc via CI (GitHub Pages or docs.rs)

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

### 2.4 Remote Skill Discovery (Stretch)

- [ ] Skill index fetched from configurable URL
- [ ] `agent skill install <name>` to download to user skills directory
- [ ] Offline fallback to cached index
- [ ] Depends on: 2.1 (solid bundled skills first)

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

### 3.4 Headless Mode — Partially Done

- [x] **`agent --serve`** — Headless HTTP API server (POST /message, GET /status, /messages, /health)
- [x] Bridge lock file for IDE discovery
- [x] Graceful shutdown with Ctrl+C
- [ ] **`agent attach <session-id>`** — Reconnect to a running headless session
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
- [ ] Fallback chain configuration: try provider A, fall back to B on failure

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

### 7.3 Interactive Tutorial System

- [ ] `/powerup` command with step-by-step interactive lessons
- [ ] Lesson structure: explanation, try-it prompt, success verification
- [ ] Ship 5 lessons covering core features

### 7.4 Voice Mode

- [ ] STT integration for voice input
- [ ] TTS integration for spoken responses
- [ ] Push-to-talk keybinding

### 7.5 Scheduled Agents

- [ ] Cron-based agent execution (`agent cron add "0 9 * * *" --prompt "run tests"`)
- [ ] Remote trigger via webhook (`agent trigger --listen :8080`)
- [ ] Background daemon for scheduled runs

### 7.6 Web and Desktop Clients

- [ ] Headless API server mode (`agent serve`) as the foundation
- [ ] Web client connecting via HTTP + SSE
- [ ] Desktop client via Tauri v2 wrapping the web client
- [ ] All clients share the same backend

### 7.7 GitHub Integration

- [ ] Comprehensive PR lifecycle management (`/pr create`, `/pr review`, `/pr merge`)
- [ ] Issue triage and auto-labeling
- [ ] CI status monitoring and fix suggestions

---

## Performance Targets

| Metric | Current | Target (v1.0) |
|--------|---------|---------------|
| Startup time | ~200ms | <150ms |
| Tool dispatch overhead | unmeasured | <1ms per tool call |
| Binary size (release) | ~15MB | <12MB (strip + LTO) |
| Context compaction latency | unmeasured | <500ms for microcompact |
| Tests | 232+ | 400+ |
| Test coverage | Codecov active | >70% |
| Supported platforms | 6 (Linux, macOS, Windows, Docker, npm) | 7 (+ homebrew-core) |

---

## Contributing

Want to help? Pick any unchecked item above and open an issue to discuss the approach before starting. See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup.

Priority areas where contributions are most welcome:
1. **GitHub Copilot provider** (Phase 4.1) — Copilot token-based access for GitHub users
2. **Provider features** (Phase 4.2) — Bedrock setup wizard, health checks, fallback chains
3. **Rustdoc comments** (Phase 1.9) — `///` docs on key public types
4. **Remote skill discovery** (Phase 2.4) — Skill index and `agent skill install`

---

*Last updated: 2026-04-05*
