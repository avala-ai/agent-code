# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`/powerup` interactive tutorials**: 5 step-by-step lessons teaching core features (first conversation, editing files, shell & tools, skills & workflows, models & providers). Arrow-key lesson picker with persistent progress tracking. Aliases: `/tutorial`, `/learn`. Reset with `/powerup reset`.

## [0.13.1] - 2026-04-05

### Fixed

- **E2E C2 test**: `jq -e` treats JSON `false` and `0` as falsy — switched to `has()` for boolean/numeric status fields

### Added

- **Coding task E2E tests** (D9, D10): agent writes Python fizzbuzz and bash arithmetic scripts, tests verify the generated code actually runs and produces correct output

## [0.13.0] - 2026-04-05

### Added

- **Azure OpenAI provider**: first-class support with `api-key` header auth, Azure AD token fallback (`AZURE_OPENAI_AD_TOKEN`), configurable API version — 16 providers total
- **SSE event stream**: `GET /events` endpoint on serve mode with 10 real-time event types (text_delta, tool_start, tool_result, thinking, turn_complete, usage, error, compact, warning, done)
- **ACP (Agent Client Protocol)**: `agent --acp` starts a stdio JSON-RPC 2.0 server for IDE integrations (VS Code, Zed, JetBrains) with initialize, message, status, cancel, and shutdown methods
- **Remote skill discovery**: `/skill search`, `/skill install <name>`, `/skill remove <name>`, `/skill installed` with configurable index URL and offline cache fallback
- **`--attach [session-id]`**: connect to a specific serve instance by session ID prefix, with interactive selection when multiple instances are running
- **Provider health check in `/doctor`**: detects active provider, uses provider-specific auth headers for connectivity test, warns on missing env vars
- **Rustdoc via CI**: `cargo doc` builds alongside mdBook and deploys to GitHub Pages at `/api/`
- **Rustdoc comments**: `///` docs on all remaining public types in the llm module
- **E2E test expansion**: 31 → 60+ tests covering ACP protocol, SSE endpoint, CLI flags, protected directories, custom skills, config variants, and edge cases

### Fixed

- **E2E C3 flake**: POST /message uses 240s timeout with automatic retry for LLM cold-start latency

## [0.12.0] - 2026-04-05

### Added

- **Cohere provider**: Command R+, Command R, Command Light (`COHERE_API_KEY`)
- **Perplexity provider**: Sonar Pro, Sonar, Sonar Deep Research with web search (`PERPLEXITY_API_KEY`) — 15 providers total
- **`agent --attach`**: connect to a running `--serve` instance from another terminal with auto-discovery via bridge lock files
- **`/uninstall` command**: shows platform-specific removal instructions with paths
- **Rustdoc comments**: enriched `///` docs on 15 key public types (Message, Config, AppState, QueryEngine, Tool, Skill, etc.)
- **Multi-agent orchestration runtime**: `Coordinator` with `spawn_agent()`, `run_team()`, `send_message()`, team management
- **Headless HTTP server**: `agent --serve` with POST /message, GET /status, /messages, /health endpoints

### Fixed

- **API key priority**: environment variables now correctly override stale keys in config files
- **Serve mode crash**: skip interactive setup wizard when running headless without TTY

## [0.11.1] - 2026-04-05

### Added

- **Release E2E test suite**: 31 automated tests covering CLI flags, serve mode HTTP API, tool verification (FileRead/FileWrite/FileEdit/Grep/Glob/Bash), permission system, skills, config, and edge cases
- **`scripts/e2e-tests.sh`**: standalone bash test harness runnable locally or in CI
- **`release-e2e.yml`** workflow: runs on tag push and manual dispatch via OpenRouter (~$0.03/run)

## [0.11.0] - 2026-04-05

### Added

- **OpenRouter provider**: access any model through a single API key (`OPENROUTER_API_KEY`)
- **npm wrapper package**: `npm install -g agent-code` downloads the correct prebuilt binary
- **Plugin executable support**: plugins can ship executables in `bin/` registered as callable tools
- **Self-update check**: background check on startup + `/update` command, throttled to 24h
- **Rustdoc**: top-level library documentation with module table, examples, and custom Tool guide
- **4 tutorials**: first project, custom skills, MCP integration, multi-provider setup
- **4 architecture deep-dives**: context compaction, tool execution, provider abstraction, MCP protocol
- **Performance tuning guide**: model selection, context management, cost control, benchmarking
- **Security documentation**: expanded SECURITY.md + new docs page with enterprise config
- **FAQ page**: 18 questions across 6 categories
- **Criterion benchmarks**: compaction and token estimation performance suites
- **Coverage reporting**: cargo-tarpaulin + Codecov in CI with README badge

## [0.10.0] - 2026-04-05

### Added

### Added

- **4 new bundled skills**: `/security-review` (OWASP vulnerability scan), `/advisor` (architecture analysis), `/bughunter` (systematic bug search), `/plan` (implementation planning) — total now 12
- **4 new commands**: `/release-notes`, `/summary`, `/feedback`, `/share` — total now 42
- **Per-model cost breakdown** in `/cost` command with cache hit rate percentages
- **`disable_skill_shell_execution`** security setting — strips shell blocks from skill templates when enabled
- **Protected directories**: writes to `.git/`, `.husky/`, and `node_modules/` are blocked regardless of permission settings
- **Windows support**: CI tests and release builds for `x86_64-pc-windows-msvc`, packaged as `.zip`
- **Docker image**: multi-stage Dockerfile with GHCR publish workflow (`ghcr.io/avala-ai/agent-code`)
- **Troubleshooting guide**: 7 categories covering API, permissions, context, tools, MCP, installation, sessions
- **FAQ page**: 18 questions across 6 categories
- **Integration tests**: 11 new tests for skills and config systems
- **Smoke tests**: end-to-end binary invocation tests
- `CHANGELOG.md` with full release history
- `ROADMAP.md` with phased v1.0 improvement plan

## [0.9.7] - 2026-03-31

Initial public release.

### Added

- **Agent loop** with streaming responses, automatic compaction, and error recovery
- **32 built-in tools**: file operations (read, write, edit, multi-edit), search (grep, glob), shell (bash, powershell), web (fetch, search), LSP diagnostics, MCP proxy, agent spawning, task management, notebooks, plan mode, worktrees
- **32 slash commands**: session management (/resume, /sessions, /export, /clear), context (/cost, /context, /compact, /model), git (/diff, /status, /commit, /review, /branch, /log), agent control (/plan, /permissions, /agents, /tasks), configuration (/init, /doctor, /mcp, /hooks, /plugins, /memory, /skills, /config)
- **8 bundled skills**: /commit, /review, /test, /explain, /debug, /pr, /refactor, /init
- **12 LLM providers**: Anthropic, OpenAI, xAI, Google, DeepSeek, Groq, Mistral, Together, Zhipu, Ollama, AWS Bedrock, Google Vertex — plus any OpenAI-compatible endpoint
- **Permission system** with configurable rules per tool and pattern, 5 modes (ask, allow, deny, plan, accept_edits), and destructive command detection
- **MCP client** with stdio and SSE transports, tool proxying, and resource access
- **Memory system** with project-level (AGENTS.md) and user-level (~/.config/agent-code/memory/) persistent context
- **Session persistence** with auto-save, resume by ID, and markdown export
- **Context management** with three compaction strategies: microcompact, LLM summary, and context collapse
- **Plugin system** loading skills and hooks from plugin directories
- **Hooks** for lifecycle events: session start/stop, pre/post tool use, user prompt submit
- **TOML configuration** with three-layer merge: user config, project config, CLI flags/env
- **Extended thinking** support for models that provide it
- **Token budget** enforcement with cost tracking and auto-compact thresholds
- **Retry logic** with exponential backoff, rate limit handling, and fallback model support
- **Streaming tool execution** during LLM response for faster turns
- **IDE bridge** protocol with lock file discovery for editor integrations
- **Subagent spawning** with optional git worktree isolation
- **Inter-agent messaging** via SendMessage tool
- **Setup wizard** for first-launch configuration
- **Cross-platform support**: Linux (x86_64, aarch64) and macOS (x86_64, Apple Silicon)
- **Installation methods**: cargo install, Homebrew tap, curl script, prebuilt binaries

[Unreleased]: https://github.com/avala-ai/agent-code/compare/v0.13.1...HEAD
[0.13.1]: https://github.com/avala-ai/agent-code/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/avala-ai/agent-code/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/avala-ai/agent-code/compare/v0.11.1...v0.12.0
[0.11.1]: https://github.com/avala-ai/agent-code/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/avala-ai/agent-code/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/avala-ai/agent-code/compare/v0.9.7...v0.10.0
[0.9.7]: https://github.com/avala-ai/agent-code/releases/tag/v0.9.7
