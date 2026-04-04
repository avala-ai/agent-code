# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

[Unreleased]: https://github.com/avala-ai/agent-code/compare/v0.9.7...HEAD
[0.9.7]: https://github.com/avala-ai/agent-code/releases/tag/v0.9.7
