# Design: Async Agent Runtime — Background Turns, Surfacing, Promotion & Durability

> Status: design / in progress. Owner: @emal. Last updated: 2026-06-29.
> Scope: the interactive async-agent experience. Complements the top-level `ROADMAP.md`
> (this is the focused engineering spec for the background-turn work; it does not restate
> already-shipped items).

This document is grounded in a file-by-file audit of `origin/main`. Every claim below carries
`file:line` evidence so we build only what is genuinely missing. Design patterns are informed by a
survey of prior-art terminal agents, referenced neutrally as **Ref A** (actor/channel model),
**Ref B** (background-job + result injection), **Ref C** (native-binary agent with restart-adopt).

---

## 1. Corrected baseline — what already exists on `main`

The background/agent subsystem is **substantially built**. Confirmed **SHIPPED** (do not rebuild):

| Capability | Evidence |
|---|---|
| Task registry + lifecycle (`TaskManager`: `register`, `register_with_color`, `spawn_shell`, `set_status`, `list`, `read_output`) | `services/background.rs:206-405` |
| `TaskKind`/`TaskPayload` (LocalShell, LocalAgent, LocalWorkflow, MonitorMcp, RemoteAgent, Dream) + serde round-trip | `services/background.rs:50-163`; `tests/task_kind_integration.rs` |
| Executor registry + dispatch | `tools/tasks/executor.rs:129-174` |
| **LocalShell** executor (spawns `bash -c`, output→file, status) | `tools/tasks/executors/local_shell.rs`; `background.rs:251-296` |
| **LocalAgent** executor (forwards to `AgentTool`, registers queue entry, drives status) | `tools/tasks/executors/local_agent.rs:27-141` |
| Task tools `TaskCreate/Update/Get/List/Stop/Output` (agent-callable, real) | `tools/tasks/tools.rs` |
| `/tasks` slash command (local snapshot, no LLM turn) | `commands/mod.rs:1438-1450`, `format_task_list` `:2429` |
| Agent tool subprocess + worktree isolation, env passthrough, color/id propagation | `tools/agent.rs:147-190, 247-272` |
| Multi-agent `Coordinator` (`spawn_agent/run_agent/run_team/create_team/send_message`) | `services/coordinator.rs:547-815` |
| Desktop notifier (`notify_task_complete`, osascript/notify-send/powershell) | `services/notifier.rs:171` |
| Scheduling (cron parse/match, executor, daemon, remote-trigger tool) | `schedule/cron.rs`, `cli/daemon.rs`, `tools/remote_trigger.rs` |
| Sandboxing (mac seatbelt, linux bwrap; win noop) | `sandbox/` ; `tests/sandbox_integration.rs` |
| Output styles (disk-loaded), structured JSONL output, `/rewind` (basic), 15 hook events, `--serve` HTTP+SSE | `output_styles/`, `cli/output.rs`, `commands/mod.rs` rewind, `config/schema.rs:641+`, `cli/serve.rs` |

**Correction to earlier notes:** task metadata is **not** persisted to disk — only task *output*
files are written (`background.rs:277,290,439-445`). `TaskManager::new()` has no load logic
(`background.rs:206-211`; `state/mod.rs:157`). So durability/adopt is genuinely absent, not partial.

---

## 2. Genuine gaps (what we will build)

Ranked. Each carries evidence and a verdict.

### Tier 1 — finish the half-built background path  *(small, high-confidence)*
1. **REPL `&` prefix is a stub** — prints a banner then runs the turn **synchronously**
   (`run_turn_with_sink`), blocking the REPL. → route to a `LocalAgent` background task.
   _Evidence:_ `cli/src/ui/repl.rs:810-838` (`// TODO: spawn actual background agent turn`).
2. **Agent tool `run_in_background` declared-but-ignored** — schema advertises it; `call()` never
   reads it (always blocks with a 5-min timeout). The **Bash** tool already implements the exact
   pattern to copy. _Evidence:_ `tools/agent.rs:83-86` (schema) vs `:103-243` (call); `tools/bash/mod.rs` `run_background`.
3. **No completion surfacing in the REPL** — `drain_completions()` has **zero consumers**, and
   `notifier.notify_task_complete()` is **never invoked from the CLI**. Background work finishes
   invisibly until the user types `/tasks`. _Evidence:_ `background.rs:408-415` (no callers);
   `notifier.rs:171` (no CLI callers); REPL loop `repl.rs:676-971` (no drain).
4. **`kill()` is a status-flip only** — it sets `TaskStatus::Killed` but never terminates the
   child process / tokio task → **orphaned processes**. _Evidence:_ `background.rs:395-405` (no
   stored child handle, no signal). This is a correctness defect, not just a missing feature.
5. **No `notified` de-dup** — `drain_completions` returns *all* completed tasks every call, so any
   poller would re-notify. _Evidence:_ `TaskInfo` has no `notified` field (`background.rs:166-197`).

### Tier 2 — net-new capabilities  *(larger; bring us level-or-ahead of all three references)*
6. **Promotion** (foreground turn → background) — no code anywhere. Requires a spawnable turn (#10).
7. **Steering** (inject input into an in-flight turn) — only binary Ctrl-C cancel exists
   (`query/mod.rs:419-436`); no mid-turn input path.
8. **Durable tasks + adopt-on-restart** — persist `TaskInfo` to disk, reload on startup, probe
   pids, reclassify (alive→running / dead→failed), replay un-surfaced completions. (Ref C parity.)
9. **Subagent concurrency** — Agent tool is `is_concurrency_safe() == false` (`agent.rs:96`); no
   parallel subagents, no cap. Add parallel dispatch + an execution limiter (RAII guard).
10. **Spawnable turn + light event seam** — `run_turn_with_sink` borrows `&mut self` for the whole
    turn (`query/mod.rs:444`); no submission/event/`watch<Status>` architecture. This is the
    *enabler* for #6/#7/#9 — build only the minimum needed, keeping `StreamSink` as an adapter.

### Tier 3 — finish stubbed executors / wiring  *(opportunistic)*
11. `LocalWorkflow`, `MonitorMcp`, `RemoteAgent`, `Dream` executors return `NotImplemented`
    (`executors/*.rs`). Implement (Workflow first — deterministic orchestration) or hide from the
    schema until implemented.
12. **Coordinator is unwired to the CLI** and its `inbox`/`send_message` mailbox has **no consumer**
    (`coordinator.rs:385,766`; zero refs in `crates/cli`). Wire inter-agent results → parent
    conversation injection (shares the surfacing path from #3).

### Already done — explicitly out of scope
Scheduling, sandboxing, output styles, structured output, basic rewind/undo, hooks, headless serve,
microcompact, `/tasks` read command. Do **not** duplicate.

---

## 3. Differentiation target

Covering Tier 1 + Tier 2 gives agent-code, in one open/hookable runtime: working background turns
**with** result injection (Ref B), promotion + steering (Ref B/Ref C), restart-adopt durability
(Ref C), and a unified job/kill model that actually terminates work — plus a hook event on every
job lifecycle transition (unique: leverages the existing 15-event hook system). No cloud/remote
dependency, no telemetry.

---

## 4. PR plan (each PR: unit + e2e tests, `cargo test --all-targets` + `clippy -D warnings` green)

Test harness note: `crates/{lib,cli}/tests/` already exist with ~14 integration suites and a
provider/test scaffold; reuse them. Background lifecycle is offline-testable with `true`/`false`/
`sleep` shell tasks and a stub agent binary.

- **PR 1 — Completion surfacing + kill correctness + dedup** *(Tier 1: #3,#4,#5)*
  - Add `notified` to `TaskInfo`; `drain_completions` only returns un-notified terminal tasks and marks them.
  - Store the child handle/process-group in `spawn_shell`; `kill()` sends termination (and cancels the LocalAgent path).
  - REPL loop: after each turn and on idle tick, drain → print a toast line **and** inject a synthetic
    `<task id=… status=…>…</task>` result into the conversation; fire `notify_task_complete`.
  - Tests: unit — dedup gating, kill terminates a `sleep 30` task (assert process gone), injection format.
    e2e — `/tasks` + a `LocalShell` task transitions and surfaces exactly once.
- **PR 2 — `&` prefix + Agent `run_in_background`** *(Tier 1: #1,#2)*
  - `&`-prefixed REPL input creates a `LocalAgent` task (non-blocking) and returns a handle line.
  - Agent tool reads `run_in_background`; when true, registers a `LocalAgent` task and returns a running handle immediately.
  - Tests: unit — Agent tool returns handle without awaiting (timing). e2e — REPL `& echo hi` returns
    promptly, `/tasks` shows it, completion surfaces (depends on PR 1).
- **PR 3 — Durable tasks + adopt** *(Tier 2: #8)*
  - Journal `TaskInfo` (+pid, output_file) to `~/.cache/agent-code/tasks/*.json` on each transition;
    load + adopt on startup; replay un-notified completions.
  - Tests: unit — journal round-trip, adopt reclassifies by pid liveness. e2e — spawn bg task, kill &
    relaunch process, task is adopted and surfaces.
- **PR 4 — Spawnable turn + minimal event seam** *(Tier 2: #10)*
  - Extract the turn body to a form runnable on `tokio::spawn` with a child `CancellationToken`; add a
    `watch<TurnStatus>` mirrored from a single emit point; keep `StreamSink` as an adapter (behavior-preserving).
  - Tests: unit — status mirrors to watch, `is_final`. e2e — regression: one-shot `-p` output unchanged.
- **PR 5 — Promotion + steering** *(Tier 2: #6,#7)* — builds on PR 4.
- **PR 6 — Subagent concurrency + Coordinator/mailbox wiring** *(Tier 2: #9, Tier 3: #12)*.
- **PR 7+ — Stubbed executors** *(Tier 3: #11)*, Workflow first.

---

## 5. Progress

**Status: core roadmap complete.** Every gap with real production value has shipped. What
remains (below) is optional, net-new scaffolding, not unfinished work.

Shipped:
- [x] PR 1 — completion surfacing + kill correctness + dedup
- [x] PR 2 — `&` prefix + Agent `run_in_background`
- [x] PR 3 — durable tasks + adopt-on-restart
- [x] PR 4 — spawnable turn + `watch<TurnStatus>` event seam
- [x] PR 5 — turn steering (engine + `Session` + REPL); promotion *primitive* via `Session::spawn_turn`
- [x] PR 6 — subagent concurrency cap (`AgentExecutionLimiter`, queues past the cap)
- [x] PR 7 — `TaskCompleted` hook + real hook-context delivery (stdin JSON + env + HTTP body)
- [x] PR 8 — `LocalWorkflow` executor (resolve skill slug → expand → run as subagent)
- [x] PR 9 — `&/skill`: run a skill as a background task (first production trigger for `LocalWorkflow`)
- [x] PR 10 — `/tasks` management surface: `output` / `kill` / `clear` (+ `&/` and `/` skill tab-completion)
- [x] infra — npm publishing via OIDC trusted publishing + provenance

Deliberately **not** built (decisions, not omissions):
- **REPL promote-hotkey** (foreground turn → background mid-flight): dropped. The interactive turn
  borrows the single engine lock for its whole duration; a clean promote would require re-architecting
  `run_repl` around `Session` ownership for marginal value. The spawn primitive exists if revisited.
- **Stubbed executors** (`MonitorMcp` / `RemoteAgent` / `Dream`) and the **multi-agent `Coordinator`**:
  these remain test-only scaffolding. The executor registry has no production dispatch driver (real
  background work goes through `spawn_shell` / `spawn_background_agent`), and `Coordinator` is
  instantiated only in tests. Each is a net-new feature with its own design — to be picked up
  intentionally if/when the product calls for MCP-watch, cloud-trigger, idle-time, or
  agent-team capabilities, not as leftover roadmap debt.
