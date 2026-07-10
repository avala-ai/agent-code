# Modern TUI — M0 Audit

**Date:** 2026-07-10  
**Branch:** `feat/tui-modern-overhaul`  
**Owner track:** M0 engine surface (handoff from PR #415)  
**Boundary:** this track owns `crates/lib/**` + `docs/tui/**` (+ design doc fixes). Does **not** edit `crates/cli/src/ui/modern/**`.  
**Engine track status (2026-07-10):** **complete** for §3.1/§3.4 UI surface — plan/HITL/subagent/context/tool-output events emit; UI consumes them (M1–M9). Remaining gate is **manual** multi-terminal dogfood ([ACCEPTANCE.md](./ACCEPTANCE.md) / [SUPPORT.md](./SUPPORT.md)) before #396 default flip.

---

## 1. Plan module map (§2.1 → reality)

| Plan path | Status | Actual path / notes |
|---|---|---|
| `ui/repl.rs` | exists | classic rustyline REPL — opt-in via `--tui classic` |
| `ui/modern/mod.rs` | exists | `crates/cli/src/ui/modern/mod.rs` |
| `ui/modern/run.rs` | exists | shell loop + prompter install (UI track) |
| `ui/modern/app.rs` | exists | App + reducers; still `TranscriptItem` not `Block` |
| `ui/modern/mode.rs` | exists | `SessionMode` (not plan’s `Mode` name) |
| `ui/modern/sink.rs` | exists | simplified `EngineEvent` + `ModernPrompter` (UI) |
| `ui/modern/render.rs` | exists | **flat file** — plan wants `render/` dir (M1/M2 UI) |
| `blocks/` | to create | UI track (M2) |
| `layout_cache.rs` | to create | UI track (M2) |
| `markdown.rs` | to create | UI track (M3); `pulldown-cmark` already in deps |
| `focus.rs` / `input.rs` | to create | UI track (M1–M2) |
| `modes.rs` | alias | keep `mode.rs` name; document as SessionMode |
| `queue.rs` | to create | UI track (M5) |
| `terminal_caps.rs` / `clipboard.rs` | to create | UI track (M7) |
| `spill.rs` / `theme.rs` | to create | UI track (M4 / M1) |
| `render/{transcript,statusbar,prompt,toolcard,modal,tasks}.rs` | to create | requires `render.rs` → `render/` split (UI) |

**Naming stance:** keep shipped `SessionMode` / `--tui` / `ui.tui` / `AGENT_CODE_TUI` until product decides rename; plan §9’s `--ui` spelling is aspirational (flag in epic #385).

---

## 2. Engine API map (M0 focus)

### Session / turns
| Symbol | Path | Notes |
|---|---|---|
| `Session::spawn_turn` | `query/session.rs` | Arc sink, cancel without engine lock ✅ |
| `TurnHandle::cancel` | `query/session.rs` | cancels per-turn `CancellationToken` ✅ |
| `TurnStatus` | `query/mod.rs` | Idle/Running/Completed/Aborted/Errored |

### StreamSink (today → target)

| Method | Today | Target (additive) |
|---|---|---|
| `on_text` | ✅ | keep (AssistantDelta) |
| `on_thinking` | ✅ optional | keep |
| `on_tool_start(name, input)` | ✅ full JSON input | keep; also emit call-id variant |
| `on_tool_result(name, result)` | ✅ | keep; also call-id variant |
| `on_tool_output(call_id, chunk)` | ❌ | add (bash stdout stream; may stub until bash pipes) |
| `on_turn_start` / `on_turn_complete` | ✅ | keep; add `on_turn_outcome` |
| `on_usage` | ✅ | keep |
| `on_context_usage(used, max)` | ❌ | add + emit from estimator |
| `on_compact` / `on_warning` / `on_error` | ✅ | keep |
| `on_subagent_update` | ❌ | add (wire from agent tool later) |
| Permission UI | via `PermissionPrompter` | not StreamSink; modern uses `ModernPrompter` |

**Impls that must keep compiling:** `NullSink`, `JsonStreamSink`, `TerminalSink`, `ChannelSink` (modern), `SseBroadcastSink`, `AcpSink`, AMR/test sinks. Prefer **optional trait methods with defaults**.

### Permissions / mode
| Concern | Today | Gap |
|---|---|---|
| Mid-turn Plan | UI updates `state.plan_mode` only when engine `try_lock` succeeds | Turn holds mutex → mode change deferred until turn ends |
| PermissionChecker | immutable `default_mode` in Arc | Config `default_mode` updated but checker not live-rebuilt |
| Session allow | `HashSet` of **tool name only** | Plan wants `(tool, normalized input shape)` |
| Bg origin | not on permission path | Need origin tag for subagent asks |

### Cancel
| Path | Status |
|---|---|
| Between tools | cancel token checked in loop |
| Stream `select!` | `_ = self.cancel.cancelled()` aborts stream ✅ |
| HTTP abort | depends on provider stream dropping `rx` |
| ≤150 ms test | spawn_turn cancel tests exist; add paused-time unit |

### Context meter
| Piece | Status |
|---|---|
| `tokens::estimate_context_tokens` | exists |
| Emit to sink on change | **missing** |
| UI must not re-scan | depends on sink event |

---

## 3. Version / crate stance

| Crate | Stance |
|---|---|
| ratatui **0.30** + crossterm **0.29** | **Stay** — already in tree; UI track owns event-stream feature |
| pulldown-cmark 0.13 | present (lib+cli) |
| similar 3 | present (lib) |
| tempfile, futures | present |
| unicode-width / unicode-segmentation | **add when M2** (UI) |
| insta / proptest | **add when M1 snapshots** (UI dev-deps) |
| tui-textarea | **not present** — UI track vendors or adds in M1 |
| base64 / xxhash | add when M7 clipboard / M3 memo |

---

## 4. Tests inventory (modern + engine)

| Area | What exists |
|---|---|
| modern module | unit tests post-`0a84a69` (prompter, truncate, mode apply) |
| TestBackend snapshots | not yet |
| `Session::spawn_turn` cancel | `query/mod.rs` tests |
| plan_mode enter/exit | `tools/plan_mode.rs` tests |
| effective_permissions Plan force | `coordinator` tests |

---

## 5. M0 work items (this track)

1. ✅ This AUDIT + `docs/tui` stubs  
2. ✅ Additive `StreamSink` methods + wire call-id / context usage / turn outcome  
3. ✅ Live mode controls (plan + permission) readable at every tool permission check without waiting for turn unlock  
4. ✅ Session-allow keys: tool + normalized input  
5. ✅ Cancel latency test under `start_paused` (`cancel_reaches_terminal_within_150ms_virtual`)  
6. ✅ Constrain `ExitPlanMode` writes to plan dir (security follow-up from PR review)  
7. ✅ Wire bash `on_tool_output` chunks via `tools::event_sink` channel + query-loop drain  
8. ✅ Emit `on_subagent_update` on Agent tool start/result (query loop)  
9. ✅ `ApplyPatch` tool (#407) — Begin Patch dialect, multi-file add/update/delete

### UI integration recipe — `Session::apply_live_mode`

**Call site** (replaces `try_lock`-only `apply_mode_to_engine` in `ui/modern/run.rs`):

```rust
// On every Shift+Tab / mode change (including mid-turn):
let plan = matches!(mode, SessionMode::Plan);
let perm = mode.permission_hint().unwrap_or(base_permission_mode);
session.apply_live_mode(plan, perm); // never blocks; always takes effect next tool check

// Optional AppState sync when lock free (badge / EnterPlanMode observers):
if let Ok(mut eng) = session.engine().try_lock() {
    eng.state_mut().plan_mode = plan;
    eng.state_mut().config.permissions.default_mode = perm;
}
```

| Handle | Effect |
|---|---|
| `session.apply_live_mode(plan, perm)` | Updates `live_plan_mode` AtomicBool + `PermissionChecker::set_default_mode` |
| `session.permissions()` | Same `Arc` the tool executor uses |
| `session.live_plan_mode()` | Read current live plan flag |

No existing `StreamSink` / `Session` method signatures were removed; only additive APIs.

---

## 6. Explicit non-touch (UI handoff)

Do not edit:

- `crates/cli/src/ui/modern/**`
- Classic REPL behavior beyond what shared lib APIs require

When `StreamSink` grows methods, keep defaults so modern `ChannelSink` compiles until UI migrates.
