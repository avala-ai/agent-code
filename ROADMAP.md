# Roadmap

This document tracks the planned improvements for agent-code on the path to v1.0 and beyond. Items are organized by phase and priority. Each phase can be worked on independently, though some items have noted dependencies.

Status key: **Planned** | **In Progress** | **Done**

---

## Current State (v0.9.7)

| Area | Status |
|------|--------|
| Rust workspace (lib + cli) | 23.7K LOC, 90 source files |
| Built-in tools | 32 (file ops, search, shell, web, LSP, MCP, agents, tasks) |
| Slash commands | 32 (session, context, git, agent control, config, diagnostics) |
| Bundled skills | 8 (commit, review, test, explain, debug, pr, refactor, init) |
| LLM providers | 12 (Anthropic, OpenAI, xAI, Google, DeepSeek, Groq, Mistral, Together, Zhipu, Ollama, Bedrock, Vertex) |
| Tests | 227 (215 unit + 12 async) |
| Platforms | Linux x86_64/aarch64, macOS x86_64/aarch64 |
| Install methods | cargo, homebrew (custom tap), curl script, prebuilt binaries |

---

## Phase 1: Documentation

**Priority: Critical** | **Target: v0.10**

Good documentation is the highest-leverage improvement. Every other phase benefits from having clear, complete docs.

### 1.1 CHANGELOG

- [ ] Create `CHANGELOG.md` using [Keep a Changelog](https://keepachangelog.com/) format
- [ ] Reconstruct history from git tags (Added / Changed / Fixed / Removed per version)
- [ ] Add `/release-notes` command that reads and displays the current version's entry

### 1.2 Troubleshooting Guide

- [ ] Create `docs/troubleshooting.mdx`
- [ ] Cover: API connection failures, permission denials, MCP server crashes, context window exceeded, tool execution errors, installation problems
- [ ] Each issue: symptoms, cause, fix

### 1.3 Tutorials (4 pages)

- [ ] **First Project** — setting up agent-code on an existing codebase
- [ ] **Custom Skills** — writing, testing, and sharing a custom skill
- [ ] **MCP Integration** — connecting and using MCP servers
- [ ] **Multi-Provider** — switching between LLM providers, configuring fallbacks
- [ ] Add Tutorials navigation group to `docs/mint.json`

### 1.4 FAQ

- [ ] Create `docs/faq.mdx` with 15-20 questions
- [ ] Categories: installation, usage, providers, cost/tokens, security, troubleshooting

### 1.5 Security Documentation

- [ ] Expand `SECURITY.md` beyond the current template
- [ ] Create `docs/security.mdx` covering: permission model deep-dive, sandbox architecture, MCP trust boundaries, `--dangerously-skip-permissions` implications, enterprise security configuration
- [ ] Document the destructive command detection system in the Bash tool

### 1.6 Architecture Deep-Dives (4 pages)

- [ ] **Context Compaction** — microcompact, LLM summary, context collapse strategies and when each fires
- [ ] **Tool Execution** — permission flow, read-only batching vs serial mutations, streaming executor
- [ ] **Provider Abstraction** — how `normalize.rs` bridges Anthropic Messages API and OpenAI Chat Completions
- [ ] **MCP Protocol** — stdio/SSE transports, tool proxying, resource access, error handling
- [ ] Add Architecture navigation group to `docs/mint.json`

### 1.7 Performance Tuning Guide

- [ ] Create `docs/guides/performance.mdx`
- [ ] Cover: context management, token budget configuration, model selection for speed vs quality, auto-compact thresholds, deferred tool optimization

### 1.8 README Enhancements

- [ ] Add CI/coverage/docs badges
- [ ] Add "What's New" section linking to CHANGELOG
- [ ] Expand feature table with tool count, command count, skill count
- [ ] Add "Community" section with links to discussions/issues

### 1.9 Rustdoc for Library Crate

- [ ] Add top-level rustdoc to `crates/lib/src/lib.rs` with feature overview and examples
- [ ] Add `///` doc comments to ~30 key public structs and functions
- [ ] Publish rustdoc via CI (GitHub Pages or docs.rs)

---

## Phase 2: Skills and Extensibility

**Priority: High** | **Target: v0.11**

### 2.1 New Bundled Skills

Add 4 new skills to `load_bundled()` in `crates/lib/src/skills/mod.rs`:

- [ ] **`security-review`** — Review code for OWASP Top 10 vulnerabilities, input validation, authentication flows, secrets handling, SQL injection, XSS. Report findings with file:line references.
- [ ] **`advisor`** — Analyze project architecture and suggest improvements. Review dependency health, code organization, test coverage, and technical debt. Prioritize recommendations by impact.
- [ ] **`bughunter`** — Systematically search for bugs. Read error logs, run tests, trace edge cases, check error handling paths, verify null/empty input handling. Report with reproduction steps.
- [ ] **`plan`** — Create a comprehensive implementation plan. Explore the codebase to understand existing patterns. Identify all files to modify. Design the solution with tradeoffs. List dependencies and risks. Estimate effort.

### 2.2 Skill Safety Setting

- [ ] Add `disable_skill_shell_execution: bool` to `SecurityConfig` in `crates/lib/src/config/schema.rs`
- [ ] When enabled, prevent skills from executing embedded shell blocks
- [ ] Document in settings reference

### 2.3 Plugin Executable Support

- [ ] Create `crates/lib/src/tools/plugin_exec.rs` implementing the `Tool` trait for plugin-provided executables
- [ ] Scan `bin/` directories inside plugin folders during plugin loading
- [ ] Register discovered executables as callable tools (JSON stdin/stdout protocol)
- [ ] Document `bin/` support in `docs/extending/plugins.mdx`

### 2.4 Remote Skill Discovery (Stretch)

- [ ] Skill index fetched from configurable URL
- [ ] `agent skill install <name>` to download to user skills directory
- [ ] Offline fallback to cached index
- [ ] Depends on: 2.1 (solid bundled skills first)

---

## Phase 3: New Commands

**Priority: High** | **Target: v0.11**

All commands are added to `crates/cli/src/commands/mod.rs`.

### 3.1 Session Commands

- [ ] **`/summary`** — Summarize the current session: files modified, tools used, key decisions made. Delegates to the agent with conversation context.
- [ ] **`/share`** — Export session to markdown or JSON file. Optionally upload to GitHub Gist and return URL.
- [ ] **`/feedback`** — Collect user feedback text, write to `~/.local/share/agent-code/feedback/` with timestamp.

### 3.2 Info Commands

- [ ] **`/release-notes`** — Read `CHANGELOG.md` and display the current version's entry. Depends on Phase 1.1.

### 3.3 Cost Enhancements

- [ ] Add `per_model_usage: HashMap<String, UsageStats>` to `AppState` in `crates/lib/src/state/mod.rs`
- [ ] Track usage per model ID in `crates/lib/src/query/mod.rs`
- [ ] Enhanced `/cost` output: per-model breakdown table, cache hit rate, estimated remaining budget

### 3.4 Headless Mode (Stretch)

- [ ] **`agent serve`** — Start agent as a headless HTTP API server
- [ ] **`agent attach <session-id>`** — Reconnect to a running headless session
- [ ] SSE event stream for real-time updates
- [ ] Enables web UI and IDE integrations without custom bridge code

### 3.5 Self-Management Commands (Stretch)

- [ ] **`/upgrade`** — Check GitHub releases API, download and replace binary if newer version available
- [ ] **`/uninstall`** — Remove binary, config, and data directories with confirmation

---

## Phase 4: Provider Expansion

**Priority: Medium** | **Target: v0.12**

### 4.1 New Providers

- [ ] **Azure OpenAI** — separate from generic OpenAI, with Azure-specific auth (AD tokens, managed identity)
- [ ] **OpenRouter** — single API key for any model, with model routing
- [ ] **Cohere** — Command R+ models
- [ ] **Perplexity** — search-augmented generation
- [ ] **GitHub Copilot** — Copilot token-based access for GitHub users

### 4.2 Provider Features

- [ ] Interactive setup wizard for Bedrock (AWS credential chain, region, model selection)
- [ ] Provider health check in `/doctor` output
- [ ] Fallback chain configuration: try provider A, fall back to B on failure

---

## Phase 5: Testing and Quality

**Priority: Medium** | **Target: v0.11**

### 5.1 Smoke Tests

- [ ] Create `tests/smoke.rs` — invoke compiled binary with `--version`, `--help`, `--dump-system-prompt`
- [ ] Verify exit codes and output format

### 5.2 Integration Tests

- [ ] `tests/integration/skills_test.rs` — load skills from temp directory, verify expansion and frontmatter parsing
- [ ] `tests/integration/session_test.rs` — save, load, list, and delete sessions
- [ ] `tests/integration/config_test.rs` — layered config merge (user + project + env + CLI)

### 5.3 Benchmarks

- [ ] `crates/lib/benches/compaction.rs` — benchmark micro-compact on conversation histories of various sizes
- [ ] `crates/lib/benches/token_estimation.rs` — benchmark token counting on large message arrays

### 5.4 Coverage Reporting

- [ ] Add `cargo-tarpaulin` step to `.github/workflows/ci.yml`
- [ ] Upload to Codecov or Coveralls
- [ ] Add coverage badge to README

### 5.5 Per-Agent Permissions (Stretch)

- [ ] Extend permission system to support per-agent rule sets (not just global)
- [ ] Plan mode gets its own permission model (read-only by default, configurable)
- [ ] Subagents can have restricted tool access

---

## Phase 6: Distribution and CI

**Priority: Medium** | **Target: v1.0**

### 6.1 Windows Support

- [ ] Add `windows-latest` to CI test matrix in `.github/workflows/ci.yml`
- [ ] Add `x86_64-pc-windows-msvc` target to `.github/workflows/release.yml`
- [ ] Verify PowerShell tool (`crates/lib/src/tools/powershell.rs`) works correctly on Windows
- [ ] Update `install.sh` or create `install.ps1` for Windows

### 6.2 Docker Image

- [ ] Create `Dockerfile` — multi-stage build (Rust builder + slim runtime with git, rg, python3, node)
- [ ] Create `.github/workflows/docker.yml` — build and push to `ghcr.io/avala-ai/agent-code` on release tags
- [ ] Document Docker usage in installation docs

### 6.3 Protected Directories

- [ ] Add built-in deny rules in `crates/lib/src/permissions/mod.rs` for:
  - `.git/` (prevent repository corruption)
  - `.husky/` (prevent hook tampering)
  - `node_modules/` (prevent dependency modification)
- [ ] Make the list configurable via settings

### 6.4 Self-Update Mechanism

- [ ] Create `crates/cli/src/update.rs`
- [ ] Check GitHub releases API for newer version on startup (with 24h cooldown)
- [ ] `agent --update` flag to download and replace binary
- [ ] Print update notification in REPL banner when available

### 6.5 npm Wrapper Package (Stretch)

- [ ] Create `npm/` directory with `package.json`, `install.js`, `index.js`
- [ ] `npm install -g agent-code` downloads appropriate prebuilt binary for platform
- [ ] Depends on: 6.1 (Windows binary needed for cross-platform npm)

### 6.6 Homebrew Core (Stretch)

- [ ] Submit formula to `homebrew/homebrew-core` (requires stable release history and passing tests)
- [ ] Depends on: v1.0 release

---

## Phase 7: Advanced Features (Post v1.0)

These are tracked for future exploration. Not committed to a timeline.

### 7.1 Agent Client Protocol (ACP)

- [ ] Implement ACP v1 for IDE integrations (Zed, VS Code, JetBrains)
- [ ] Map ACP sessions to internal agent-code sessions
- [ ] `agent acp` command to start ACP stdio server

### 7.2 Multi-Agent Orchestration

- [ ] Implement coordinator runtime in `crates/lib/src/services/coordinator.rs` (type definitions already exist)
- [ ] `Coordinator::spawn_agent()` for parallel subagent execution
- [ ] `Coordinator::run_team()` for multi-agent task decomposition with result aggregation
- [ ] Agent-to-agent messaging via `SendMessage` tool

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
| Tests | 227 | 400+ |
| Test coverage | unmeasured | >70% |
| Supported platforms | 4 | 5 (+ Windows) |

---

## Contributing

Want to help? Pick any unchecked item above and open an issue to discuss the approach before starting. See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup.

Priority areas where contributions are most welcome:
1. **Documentation** (Phase 1) — lowest barrier, highest impact
2. **New bundled skills** (Phase 2.1) — just prompt engineering + registration
3. **Smoke and integration tests** (Phase 5) — improve confidence for everyone
4. **Windows CI** (Phase 6.1) — expand the user base

---

*Last updated: 2026-04-04*
