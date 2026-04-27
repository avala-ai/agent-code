# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

*No changes yet.*

## [0.20.0] - 2026-04-27

### Added

- **Codex ChatGPT auth** (#248): new `auth_mode = "codex_chatgpt"` / `--auth-mode codex_chatgpt` path that reuses an existing `codex login` session from `$CODEX_HOME/auth.json` or `~/.codex/auth.json` instead of requiring an `OPENAI_API_KEY`. The provider refreshes Codex OAuth tokens while preserving unknown auth-file fields and routes this mode through the ChatGPT Codex Responses backend.

### Changed

- **Dependency refresh** (#247): bumped `rustls-webpki` from 0.103.10 to 0.103.13.

## [0.19.0] - 2026-04-24

### Added

#### Hook system — lifecycle surface expansion

- **`pre_tool_use` veto** (#243): a pre-tool-use hook that exits non-zero now blocks the tool call. The model receives a synthetic error carrying the hook's stderr as the reason, and the denial is recorded on the existing `DenialTracker` so audit pipelines pick it up uniformly. `HookResult` gains a dedicated `stderr` field so block reasons stay separate from stdout.
- **`error` event** (#241): fires when a turn exits unrecoverably (e.g. LLM call failure after retry + compaction). Context carries `session_id`, `turn`, `stage`, `message` — pagers and failover automation can listen without grepping stderr.
- **`permission_denied` event** (#242): fires per blocked tool call, whether blocked by a configured rule or the interactive user prompt. Context carries `tool`, `tool_use_id`, `reason`, `input_summary`, `timestamp`. Batched once per turn, respects `tool_name` hook scoping, and the engine tracks a high-water mark so eviction in the 100-record ring buffer can't cause double-fires.

#### Configuration surface

- **`api_key_helper`** (#236): a shell command whose trimmed stdout becomes the session API key. Runs only when no static key was resolved from file or env — intended for short-lived tokens from vault / 1Password / `aws sts`. Helper errors are categorized (`SpawnFailed` / `NonZeroExit` / `InvalidUtf8`) and only the category string is logged, so subprocess output (which might carry the key) never leaks into diagnostics.
- **`.agent/settings.local.toml`** (#234): new gitignored overlay layer that sits on top of the shared `settings.toml`. Intended for machine-specific or developer-local overrides without mutating the committed file. Walks independently from `settings.toml` so a repo-root shared config and a crate-local overlay can live at different ancestor levels.
- **Hierarchical `AGENTS.md` discovery** (#233): the memory loader now walks from the session cwd up to the repo root (nearest `.git` ancestor), loading every `AGENTS.md` / `.agent/AGENTS.md` / `CLAUDE.md` / `.claude/CLAUDE.md` along the way in outermost→innermost order. In a monorepo a sub-package's `AGENTS.md` now composes with the repo-root one instead of the loader only seeing the cwd-level file.
- **`[session] cleanup_period_days`** (#239): opt-in pruning of session JSON files under `~/.config/agent-code/sessions/`. Files whose `updated_at` is older than N days are deleted at REPL startup. Strictly-less-than boundary, unparseable timestamps are kept, `Some(0)` is a deliberate no-op sentinel, non-JSON files are skipped entirely.
- **`[ui.statusline]`** (#238): customizable between-turn status divider. `enabled = false` suppresses it; `template` swaps the built-in layout with a `{model}`/`{turn}`/`{tokens}`/`{cost}`/`{cwd}`/`{session_id}` format string. Unknown placeholders pass through verbatim so forward-compat templates degrade gracefully.

#### CLI commands

- **`/tasks`** (#240): now lists in-process background tasks directly from the local `TaskManager` instead of bouncing through an LLM turn. Runtime math is factored into a pure `format_task_list(tasks, now)` helper so the table rendering is unit-testable without a tokio runtime.
- **`/tools`** (#231): lists every tool available in the current session (built-ins + MCP + subagents).

#### MCP

- **`McpClient::reconnect_with_backoff`** (#235): exponential-backoff reconnect (1s → 2s → 4s → 8s → 16s → 30s cap, clamped against `u32::MAX` attempt counts). A transient subprocess exit or network hiccup no longer tears down the whole agent loop.

#### Diagnostics

- **`/doctor` deeper config validation** (#237): uses the same ancestor walk the loader uses (so a sub-crate session in a monorepo sees the repo-root `settings.toml`); reports `settings.local.toml` overlay; runs a per-file TOML parse so syntax errors surface immediately; detects unknown top-level sections against an allow-list (catches typos like `[permisions]` that `#[serde(default)]` would otherwise silently ignore); warns when multiple provider API-key env vars are set.

### Changed

- **`HookResult` extended with `stderr`** (#243): shell hooks now capture subprocess stderr into a dedicated field. Existing consumers continue to work — the struct is `Default + Clone`.

## [0.18.0] - 2026-04-23

### Added

#### Session navigation commands

- **`/history`** (#209): shows recent user prompts in the current session with turn numbers. `/history 20` expands the window; default is 10.
- **`/redo`** (alias `/again`, #210): resubmits the most recent real user prompt as a new turn. Skips tool results and compact summaries so the resubmitted text is always something the user actually typed.
- **`/rewind N`** (alias `/undo`, #213): undoes the last N turns (default 1). Replaced the ad-hoc pop loop with a per-turn truncation algorithm that defines a turn as *the most recent real user prompt + everything after it*. Tool results and compact summaries are no longer treated as prompt boundaries, so a session whose tail is a tool result can't be mis-truncated and a compacted session can't be rewound past its summary.
- **`/info`** (#211): one-page snapshot of session state — session id prefix, cwd, additional tracked dirs, model (+ fast model if configured), base URL, turn count, token totals, cost, message count, mode flags (plan/brief/style/fast/sandbox), OS/arch/shell, MCP server count, hook count.
- **`/search <substring>`** (alias `/find`, #212): greps the entire session (text, thinking, tool use, tool results) for a case-insensitive substring and prints up to 20 matches with ±40-char snippets. Character-boundary-aware so multi-byte glyphs don't panic.
- **`/files`** (#201): shows files the session has touched.
- **`/session`** (#204): interactive picker to resume a saved session.
- **`/open <path>`** (#208): opens an existing file in `$VISUAL` or `$EDITOR` without leaving the REPL.
- **`/debug-tool-call`** (aliases `/dtc`, `/last-tool`, #205): inspects the last tool call (or all of them with `list`) — name, input JSON, result text, error state.
- **`/transcript`** (#199): alias for `/scroll` with in-viewer substring search.

#### REPL productivity

- **`/fast`** (#200): toggles between the main model and a configured cheaper/faster model for the rest of the session.
- **`/sandbox on|off|toggle`** (#202): changes the process-level sandbox state at runtime without restarting.
- **`/brief`** (#188): terse-response mode — injects a top-of-prompt directive capping assistant replies.
- **`/output-style`** (#192): swaps preset response voices (concise, explanatory, learning).
- **`/copy`** (#190): copies the last assistant reply to the system clipboard.
- **`/editor`** (#194): composes a prompt in `$EDITOR` / `$VISUAL` — useful for multi-line prompts.
- **`/reload`** (#189): rescans on-disk extensions (skills, rules, agents, hooks, MCP) and drops the cached system prompt so the next turn picks them up.
- **`/tag`** (#180): multi-label filtering for saved sessions.
- **`/keybindings`** (#183): lists REPL shortcuts and points at the override file path.
- **`/ctxviz`** (#191): per-category context breakdown (system prompt, history, tool defs, etc.).
- **`/tokens`** (#174): estimates token count for an arbitrary string.
- **`/install-github-app`** (#181): walks the user through installing and authenticating the `gh` CLI so PR-related commands work.
- **`/thinkback-play`** (#182): replays extended-thinking blocks in sequence from a recent assistant turn.
- **`/skill validate`** (#175): lint-only pass on a skill file — catches malformed YAML frontmatter, missing fields, invalid invocation patterns.
- **Fuzzy command completer** (#195): alias + substring matching in the tab-completion menu.
- **`@path` tab-completion** in REPL input (#198).
- **Richer `?` help panel** with current-session context (#197).
- **User-facing warning registry + startup banner** (#196): surfaces config warnings users would otherwise never notice.

#### Hook system

- **`pre_compact` event** (#206): fires before `/compact` or auto-compaction mutates history, with message count + estimated tokens to be freed. Hooks can archive or snapshot before older messages are replaced.
- **`post_compact` event** (#218): fires after compaction finishes with the *realized* outcome (messages before/after, actual tokens freed). Audit hooks can now pair the estimate with the ground truth across all three compaction paths (microcompact, LLM, context collapse).
- **`session_start`, `session_stop`, `user_prompt_submit`** wired (#214): these events were declared in the schema, listed by `/hooks`, and documented — but never actually fired from the agent loop. Wired in the CLI one-shot path (including before `std::process::exit`), the REPL path, and the scheduled-task executor.
- **`pre_turn`, `post_turn`** events (#185, roadmap 7.28).
- **`/hooks preview <event>`** (#216): lists which configured hooks would fire for a given event. Catches misspelled event names in `settings.toml` — a misspelled event silently deserializes as a hook that never matches anything at runtime. Accepts both canonical `snake_case` and hyphenated/mixed-case forms.

#### Structured JSONL output

- **`turn_start` event** (#217): bookend matching `turn_complete`. Consumers can now render real-time turn progress without racing the first `text_delta` or `tool_call`.
- **`warning`, `compact` events** (#215): previously only stderr plain text — now also emitted as structured JSONL on stdout so automation doesn't lose signal on budget warnings, rate-limit backoffs, and autocompaction.
- **`permission_denied` event** (#219): first-class envelope for policy-blocked tool calls. Detects both shapes emitted by the executor (policy Deny with reason, user Deny at interactive prompt) so consumers no longer have to grep `tool_result.output` for a literal string to distinguish policy violations from genuine tool errors.

#### Tools, agents, rules

- **Monitor tool** (#184): polls background tasks (logs, progress, status) without busy-waiting.
- **Project rules** from `.agent/rules/*.md` (#186): per-project behavioral rules auto-loaded into the system prompt.
- **Per-subagent permission sets** (#187): subagents can be spawned with a scoped permission overlay instead of inheriting the parent's full permissions.

#### Bundled skills

- **`/passes`** (#203): multi-pass planning loop.
- **`loop` skill** (#177): idiomatic iterate-until-done recipe.
- **`verify` skill** (#176): run-the-lint-and-test-gate recipe.
- **`ultrareview` skill** (#178): multi-agent cloud review of the current branch.
- **`commit-push-pr` skill** (#173): safe commit + push + PR-create workflow.

### Changed

- **Compatibility matrix added** (#179, roadmap 1.10): explicit matrix of supported platforms, Rust versions, and feature flags in the docs reference.

### Fixed

- **Windows CI Test job** (#193): unblocked on every PR — previously failed intermittently due to a platform-specific issue.

## [0.17.0] - 2026-04-22

### Added

- **`/pentest` bundled skill** (#137): five-phase white-box penetration test workflow (recon → slice → vuln analysis → exploit-or-discard → report) with a proof-of-concept gating policy — findings without a reproducible PoC are demoted to INFO or dropped. Runs entirely in-session against a target directory and writes a severity-grouped markdown report.
- **`/effort` command** (#150): rates task complexity XS/S/M/L/XL with a one-line justification and top 2 risks. No-arg form rates the task currently being discussed; `/effort <task>` rates a supplied description.
- **`/break-cache` command** (#151): forces the next outgoing request to skip the prompt cache so the cache prefix is rebuilt. One-shot flag on `AppState`, consumed after the next request. Useful for mid-session config changes or debugging cache behavior.
- **`/heapdump` command** (#152, hidden from `/help`): writes a best-effort process memory snapshot (VmRSS / VmSize / VmPeak / per-segment on Linux; `ps -o rss,vsz` on macOS) to a timestamped file under the data directory.
- **`/btw` command** (#153): quick-capture note to user memory without going through the model. Writes a timestamped markdown file with slugified filename and updates `MEMORY.md` index.
- **`/rename` command** (#154): attaches a human-readable label to the current session. Surfaced in `/sessions` listings; empty arg clears the label. New `label: Option<String>` on `SessionData`, serde-defaulted for backward compat.
- **`/add-dir` command** (#155): tracks additional directories alongside cwd in the session's working set. Injected into the system prompt under `# Environment`. Paths are canonicalized on add. Forms: list, add, `--remove <path>`, `--clear`. Session-scoped (not persisted).
- **`remember` bundled skill** (#156): saves an insight to user memory using the two-step write discipline (file + MEMORY.md index) with correct type classification.
- **`stuck` bundled skill** (#157): forces the agent to name the shared assumption behind failed attempts, propose two different approaches, and take one concrete step without retrying what already failed.
- **`simplify` bundled skill** (#158): review-then-simplify pass on the current diff — flags dead weight (unused imports, premature abstractions, speculative error handling, defensive copies, comments restating code).
- **`/thinkback` command** (#159): surfaces the model's extended-thinking blocks from a recent assistant turn. `/thinkback` shows the latest; `/thinkback <n>` walks back N turns (1 = latest).
- **`/usage` command** (#160): per-turn token timeline table (model, input, output, cache read, cache write) plus a cache-hit-rate hint. Complements `/cost` which aggregates.
- **`batch` bundled skill** (#162): applies the same change across multiple git worktrees — one worktree per target, test+lint gate per target, first failure stops the run.
- **`skillify` bundled skill** (#163): extracts the productive workflow from the current session into a reusable `.agent/skills/<name>.md` file with YAML frontmatter and imperative numbered steps.
- **`/pr-comments` command** (#164): fetches inline + issue comments on a PR via `gh`, groups them into (unresolved / action-requested / resolved), and produces a triage list with file:line, author quote, and suggested response or fix per item.
- **`/autofix-pr` command** (#165): checks out a PR in an isolated worktree, detects the toolchain, runs the lint + test gate, applies minimal fixes with re-verification after each, commits, and pushes back. Never force-pushes, never skips hooks, never modifies tests to make them pass.
- **`/perf-issue` command** (#166): report-only performance regression audit on the current diff (or named target). Looks for N+1 queries, missing indexes, sync I/O on hot paths, allocation hotspots, quadratic algorithms, cache invalidation bugs, unbounded growth, and sync-in-async patterns.
- **`/env` command** (#168): lists the environment variables agent-code actually reads — config overrides, 12 provider-native API keys, runtime/logging, shell context. Secrets (`*_API_KEY` / `*_TOKEN` / `*_SECRET`) displayed as `(N chars, ends in …xxxx)` so the user can confirm the right key is set without leaking it in a screenshare.
- **`backport` bundled skill** (#169): cherry-picks a commit or PR onto one or more release branches, each in an isolated worktree. Mechanical conflict resolution only; anything requiring judgment stops on that branch. Pushes to `backport/<source>-onto-<target>` and opens linked PRs.
- **`/issue` command** (#170): drafts a GitHub issue from session context — symptom, reproduction, expected/actual, environment, investigation findings — and opens it via `gh issue create` after user approval. Strips credentials from log excerpts before submission.
- **Configuration profiles** (#171): new `services::profiles` module plus `/profile save|load|list|delete|help` command. Profiles are full `Config` snapshots stored as `<config_dir>/agent-code/profiles/<name>.toml`; loading replaces the runtime config wholesale (no merge). Name validation rejects path escapes, shell metachars, and oversize names so a malicious name can't write outside the profiles dir.

### Changed

- **Docs synced** (#167): README command count 52 → 65 and bundled skill count 18 → 25 (after all Wave 1-3 additions). `ROADMAP.md` §2.1 expanded with 5 new skills; new §3.6 Productivity Commands and §3.7 PR-Workflow Commands sections marked Done. `docs/reference/commands.mdx` (+ mdBook mirror) gained rows for 11 new commands across Session / Context / Git / Diagnostics / Memory sections. `docs/extending/skills.mdx` (+ mdBook mirror) updated with the full 25-skill table.

### Fixed

- **Tarpaulin coverage flake** (#161): switched `cargo tarpaulin` to `--engine llvm` in CI. The default ptrace engine raced with tokio's child-process reaper on cancellation tests, producing spurious ECHILD / multi-terabyte allocation failures. LLVM engine uses source-based coverage with no ptrace.

## [0.16.1] - 2026-04-18

### Fixed

- **Release binary segfault on systems with glibc < 2.39** (#134): pinned Linux release builders from `ubuntu-latest` to `ubuntu-22.04` (glibc 2.35) so the published binaries don't pick up weak `pidfd_spawnp`/`pidfd_getpid` symbols from GLIBC_2.39 that resolve to NULL and segfault when tokio spawns a subprocess on Ubuntu 22.04, Debian 12, RHEL 9, and similar distros.

## [0.14.0] - 2026-04-06

### Added

- **Cross-platform Flutter client** with WebSocket JSON-RPC backend: desktop app (macOS, Linux), web WASM build, bidirectional permission prompting, heartbeat-based dead client detection, auto-update checker. Shared Dart client library at `packages/agent_code_client/`
- **Behavioral evaluation framework**: `crates/eval/` with `eval_test!` macro, `TestRig` with workspace setup and tool log capture, best-of-4 retry logic for LLM non-determinism, two policy tiers (`AlwaysPasses` / `UsuallyPasses`), breakpoint mechanism for model steering tests. Ships with 10 seed evals
- **Shell passthrough context injection**: `!` prefix now streams output in real-time (piped subprocess instead of `.output()`) and injects captured output into conversation history as `is_meta` message. Agent can reference shell output in subsequent turns without copy-pasting. 50KB truncation prevents context bloat
- **Prompt caching for tool definitions**: `cache_control: { type: "ephemeral" }` on the last tool in the tools array, caching the full prefix (system prompt + 32 tools). New `features.prompt_caching` config toggle (default: true)
- **Scheduled agents**: `agent cron add` for recurring agent execution, `agent trigger --listen` for webhook-triggered runs, background daemon for unattended operation
- **`/powerup` interactive tutorials**: 5 step-by-step lessons teaching core features. Arrow-key lesson picker with persistent progress tracking. Aliases: `/tutorial`, `/learn`
- **Homebrew tap auto-update**: release workflow automatically updates `avala-ai/homebrew-tap` formula with new version and SHA256 checksums on every tagged release
- **Shell passthrough test suite**: 40 tests across 3 layers (25 unit, 11 integration, 4 E2E bash). Extracted capture logic into `services/shell_passthrough.rs` for testability
- **Detailed v1.1/v1.2 roadmap**: 15 feature specifications with architecture diagrams, config schemas, and implementation checklists

### Changed

- **Shell passthrough refactored**: `!` handler in `repl.rs` reduced from 92 lines to 19 lines by extracting into `services/shell_passthrough.rs` library module
- **Prompt caching wired to config**: `enable_caching` in query loop now reads `features.prompt_caching` instead of hardcoded `true`

### Fixed

- **E2E D10 coding task**: made resilient to context overflow during long agent sessions

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

[Unreleased]: https://github.com/avala-ai/agent-code/compare/v0.20.0...HEAD
[0.20.0]: https://github.com/avala-ai/agent-code/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/avala-ai/agent-code/compare/v0.18.0...v0.19.0
[0.18.0]: https://github.com/avala-ai/agent-code/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/avala-ai/agent-code/compare/v0.16.1...v0.17.0
[0.16.1]: https://github.com/avala-ai/agent-code/compare/v0.16.0...v0.16.1
[0.13.1]: https://github.com/avala-ai/agent-code/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/avala-ai/agent-code/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/avala-ai/agent-code/compare/v0.11.1...v0.12.0
[0.11.1]: https://github.com/avala-ai/agent-code/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/avala-ai/agent-code/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/avala-ai/agent-code/compare/v0.9.7...v0.10.0
[0.9.7]: https://github.com/avala-ai/agent-code/releases/tag/v0.9.7
