# Modern TUI — Final Plan of Attack

**Status:** ready to execute  
**Branch:** `feat/tui-modern-overhaul`  
**Spec source:** implementation report (M0–M10, shared types, budgets)  
**Related:** `docs/design/tui-modern-overhaul.md`, `docs/design/tui-world-class-parity.md`, `docs/tui/ACCEPTANCE.md`

This document is the **execution plan**. The long implementation report remains the behavior contract. When they disagree on *how*, prefer the report; when they disagree on *what already exists*, prefer this audit.

---

## 1. Mission

Ship a fullscreen Rust agent pager at world-class terminal UX, without rewriting the engine or abandoning MIT / multi-provider / no-telemetry differentiators.

**Definition of done (product bar):** Appendix C of the implementation report, recorded in `docs/tui/ACCEPTANCE.md`, then flip `ui.default` / `--ui auto` → modern. Classic stays `--ui classic`.

**Out of scope for this track:** apply_patch (engine-track P1), mermaid/media/voice, marketplace UI, JS/Ink/OpenTUI, rewriting `ui/repl.rs`.

---

## 2. Reality check (scaffold vs target)

### Already on branch (uncommitted scaffold + engine prep)

| Area | Today | Plan target |
|---|---|---|
| Layout | 6 flat modules, ~1.2k LOC | `blocks/`, `render/*`, `layout_cache`, etc. |
| Event loop | poll + 16 ms sleep + always-draw | `EventStream` + `select!` + dirty + coalescer |
| Transcript | `TranscriptItem` strings | typed `Block` + `ToolCard` / groups |
| Modes | Normal→Plan→AcceptEdits→**AlwaysApprove** | Manual→Normal→AcceptEdits→**Plan** |
| Esc / Ctrl+C | **Esc cancels turn** (wrong) | Esc = navigate only; Ctrl+C = cancel |
| Engine path | `Session::spawn_turn` + `ChannelSink` ✅ | keep; extend event surface |
| StreamSink | text/thinking/tool start+result/usage | + ToolOutput stream, PermissionRequest, SubagentUpdate, ContextUsage, full inputs |
| Terminal | alt-screen + raw only | guard + mouse/focus/paste/kitty + sync-update |
| Tests | few unit tests, no TestBackend suite | insta + fake engine + property tests |
| Default | modern-only interactive (classic removed) | |

### Correct foundations to keep

1. **Shell / view / engine split** already sketched: `run.rs` I/O · `app.rs` reducers · `render.rs` draw · `sink.rs` channel.
2. **`Session::spawn_turn` + `StreamSink`** — engine off the draw path. Do not invent a second loop.
3. **ratatui 0.30 + crossterm 0.29** present; `pulldown-cmark 0.13` already in tree.
4. Engine work already started: typed subagents, plan-mode returns, coordinator permissions — complementary to §3.4, not a substitute.

### Critical bugs / anti-patterns to kill in M0–M1 (do not ship)

| Issue | Location | Fix milestone |
|---|---|---|
| Esc cancels turn | `run.rs` `KeyCode::Esc` → `request_cancel` | **M1** (hard rule) |
| Always redraw every loop | `terminal.draw` unconditional | **M1** dirty flag |
| Always-on 80 ms tick | even when idle | **M1** animation-gated |
| Polling crossterm | sleep 16 ms | **M1** `EventStream` |
| Mode set is YOLO-last | `AlwaysApprove` | **M1** align to Manual/Normal/Accept/Plan |
| Mode → engine only toggles `plan_mode` | AcceptEdits/Always not wired | **M0** mid-turn mode at permission points |
| Tool input truncated at sink | `tool_detail` 72 chars | **M0/M4** full JSON for modals/cards |
| No TerminalGuard / panic restore | restore only on clean path | **M1** |
| Mode name collision | plan uses `SessionMode`; report uses `Mode` | **M0 AUDIT** — pick one name, alias if needed |

---

## 3. Strategy principles (non-negotiable)

1. **One PR per milestone M0→M10.** ≤ ~1,500 LOC non-test. Tests in same PR.
2. **Audit before inventing.** M0 writes `docs/tui/AUDIT.md` mapping §2.1 modules → real paths; no parallel types.
3. **UI never blocks engine; engine never draws.** Channels only (`EngineEvent` / `UiCommand`).
4. **Classic byte-stable.** All new code under `ui/modern/` (+ minimal `agent-code-lib` event surface).
5. **Frame discipline from day one (M1):** dirty · 100 ms / 8 KiB coalescer · non-delta forces flush · animation only while running · resize-only full invalidate.
6. **Input routing as tables** (`input.rs`), not scattered matches — required for §5 key spec tests.
7. **Do not scope-creep into tools/** for apply_patch. Edit card only needs multi-hunk *display*.

---

## 4. Attack sequence (milestones = PRs)

```
M0 ──► M1 ──► M2 ──► M3 ──► M4 ──► M5 ──► M6 ──► M7 ──► M8 ──► M9 ──► M10
pre    loop   blocks  md     cards  queue  modals caps  tasks  mouse default
       chrome scroll         spill        HITL          attach select flip
```

### Compressed critical path (if capacity is tight)

Ship the “feels modern” core first:

| Wave | Milestones | User-visible outcome |
|---|---|---|
| **A — Feel** | M0 + M1 + M2 + M4 + M6 | Mode badge, cancel, follow/free scroll, tool cards, real permission modal |
| **B — Trust** | M5 + M7 | Queue never loses prompts; tmux/SSH safe |
| **C — Agents** | M8 | Tasks pane + attach |
| **D — Polish** | M3 + M9 + M10 | Markdown quality, mouse, default flip |

**Do not reorder A.** M3 can slip after M4 if markdown stays plain-wrapped temporarily. M9 can ship keyboard visual-select first and timebox drag.

---

## 5. Milestone battle cards

### M0 — Audit + engine event surface  ·  **PR gate for everything**

**Owner focus:** `agent-code-lib` + docs only. No chrome redesign.

| Deliverable | Notes |
|---|---|
| `docs/tui/AUDIT.md` | Map every §2.1 module → exists / create; ratatui 0.30 stance; `tui-textarea` compat decision |
| StreamSink completeness | Adapter or trait extensions for §3.1: ToolOutput chunks, PermissionRequest(+full input, origin), QuestionAsked, PlanProposed, SubagentUpdate, ContextUsage, TurnFinished outcome |
| Mid-turn mode | Engine reads mode/permission overlay at **every** permission decision (watch channel or equivalent) |
| Cancel | Token between tools + model stream; abort HTTP; test ≤150 ms virtual |
| Session AllowSession store | Keyed by (tool, normalized input shape) |
| Bg multiplex | Subagent permission/question → lead channel with `origin` |
| Deps | Enable crossterm `event-stream`; add insta, similar, unicode-*, base64, tempfile, futures, xxhash-rust as needed; proptest as dev |

**Exit:** scripted fake tool run emits all events; cancel test green; AUDIT committed.

**Agent order of operations**
1. `rg` StreamSink / spawn_turn / permission decision sites; read existing modern tests.
2. Write AUDIT.md first (findings before code).
3. Land lib changes with unit tests.
4. Do **not** restructure `ui/modern/` in this PR beyond sink event enum if required by adapters.

---

### M1 — Event loop, guards, chrome, modes, cancel  ·  **P0**

**Rewrite heart of `run.rs`.** Keep App/render, restructure toward dirty loop.

| Must ship | Must kill |
|---|---|
| `TerminalGuard` + panic hook | Esc-cancels-turn |
| `EventStream` + `select!` | Always-draw / always-tick |
| Dirty flag + StreamBuffer | AlwaysApprove as cycle end (→ Manual) |
| Status bar: mode badge always | — |
| Shift+Tab mid-turn → SetMode | — |
| Ctrl+C state machine (§5) | — |
| Prompt multi-line (tui-textarea or vendor) | — |
| insta idle/running/modes/quit-armed | — |

**Mode cycle (canonical):** `Manual → Normal → AcceptEdits → Plan → Manual`.

Wire AcceptEdits / Manual through engine surface from M0 (not plan_mode-only).

**Exit:** idle 0 frames; 1k deltas ≤10 fps; cancel ≤150 ms; panic restore test.

---

### M2 — Typed blocks + LayoutCache + Follow/Free + pill  ·  **P0**

Replace `TranscriptItem` with `Block` / `ScrollState` / `LayoutCache`. Virtualize. Tab focus cycle (Tasks stub ok if pane empty).

**Exit:** Free scroll ignores stream; pill counts; Esc → Follow+Prompt; 10k-line scroll budget; property wrap test.

**Rename rule:** delete `TranscriptItem` in this PR; one model only.

---

### M3 — Markdown pipeline  ·  **P0 (can soft-slip after M4)**

`pulldown-cmark` → styled lines; syntect fences; span budget 20k; streaming re-render only active block.

Classic termimad path untouched.

---

### M4 — Tool cards, grouping, spinner waiting-on, spill  ·  **P0**

Classify Bash/Read/Edit/Search/Fetch/Task/Mcp/Other. Group ≥3 consecutive read successes. Spill head/tail + tempfile. Spinner from `WaitingOn`.

**Depends on:** full tool input + ToolOutput stream from M0.

---

### M5 — Prompt queue  ·  **P0**

Enter-while-running queues; chips; empty-Enter steer/mark-next; MaxTurns preserves queue; quit dumps queue to stdout after restore.

**Depends on:** M1 submit path + TurnFinished outcomes.

---

### M6 — HITL modals  ·  **P0 flagship**

Permission (full scrollable args, y/a/n, origin), Plan approve, AskUserQuestion. FIFO queue, bg origins. Focus steal/restore.

**Depends on:** M0 PermissionRequest + AllowSession + bg multiplex; M3 nice-to-have for plan markdown.

---

### M7 — Terminal caps, sync-update, clipboard, /terminal-setup  ·  **P0 trust**

Probe ≤80 ms; synchronized output; OSC 52 + tmux passthrough; focus-event hygiene; guard byte tests.

**Can start parallel after M1** if staffing allows (few App deps); merge after M1 to avoid guard conflicts.

---

### M8 — Tasks pane + attach  ·  **P1**

Needs-input first ordering; attach without killing lead; independent scroll state; kill confirm.

**Depends on:** SubagentUpdate from M0; block model M2.

---

### M9 — Mouse + selection  ·  **P1/P2**

Hit-test via LayoutCache; click expand/pill/mode/tasks; OSC 8; **keyboard visual mode first**, drag timeboxed.

---

### M10 — Perf, skins, golden, default flip  ·  **close-out**

`/minimal` skin; memory caps; perf_smoke + soak; SUPPORT.md matrix; ACCEPTANCE.md Appendix C; flip default.

---

## 6. File evolution map (scaffold → target)

Do not create parallel trees. **Grow and rename in place.**

```
ui/modern/                    # today              # target
  mod.rs                      keep                 keep + re-exports
  run.rs                      rewrite M1           shell loop + TerminalGuard
  app.rs                      evolve M1–M8         AppState + reducers only
  mode.rs                     → modes.rs M1        Mode enum + badges (or keep name, AUDIT decides)
  sink.rs                     evolve M0            EngineEvent full set / adapter
  render.rs                   split M1+            render/mod.rs + submodules
  (new) blocks/, layout_cache, markdown, focus, input, queue,
        terminal_caps, clipboard, spill, theme, render/*
```

`ui/repl.rs` — frozen. Touch only if a shared hook would otherwise force duplication (prefer not).

---

## 7. Engine change budget (§3.4 only)

| Change | Where to look first | Milestone |
|---|---|---|
| StreamSink optional methods / richer callbacks | `query/mod.rs` StreamSink + all impls | M0 |
| Mid-turn permission mode read | permission check path / coordinator | M0 |
| Cancel in stream + tools | TurnHandle, query loop, HTTP client | M0 (verify existing cancel) |
| ContextUsage incremental | usage aggregation on message append | M0–M1 |
| Subagent → lead events | agent tool / local_agent executor | M0–M8 |
| AllowSession store | permissions layer | M0–M6 |

Prefer **optional trait methods with defaults** so classic REPL / JSON / serve sinks stay green without churn.

---

## 8. Testing doctrine (every PR)

| Layer | Tool |
|---|---|
| Pure reducers / input tables | unit tests |
| Draw | `TestBackend` 100×30 + 60×20, insta character rows |
| Integration | `tests/support/fake_engine.rs` + real event loop + `start_paused` |
| Layout / markdown | proptest widths 20..=200 |
| Guards / OSC | capture-writer byte asserts |
| Perf | M10 `perf_smoke` ignored in fast CI, nightly |

**CI gate per PR:** clippy `-D warnings` on touched crates, `cargo test --workspace`, insta reviewed.

---

## 9. First two weeks (concrete attack calendar)

Assume one focused agent (or human) at a time.

| Day | Outcome |
|---|---|
| **D1** | M0 AUDIT.md complete; dep/version decisions locked |
| **D2–D3** | M0 lib events + mid-turn mode + cancel test; PR open |
| **D4–D6** | M1 loop rewrite, guards, modes, cancel semantics, snapshots |
| **D7–D9** | M2 blocks + LayoutCache + Follow/Free + pill |
| **D10–D12** | M4 tool cards + grouping + spill (plain markdown ok) |
| **D13–D15** | M6 permission/plan/question modals |

Checkpoint: **Wave A dogfood** — daily use of the modern TUI for real work. Then M5 queue, M7 caps, M3 polish, M8 agents, M9/M10.

---

## 10. PR hygiene

- Branch naming: `feat/tui-m0-audit`, `feat/tui-m1-event-loop`, … stacked on `feat/tui-modern-overhaul` or sequential merges into it.
- Conventional commits: `feat(tui): …`, `fix(tui): …`, `test(tui): …`.
- PR body: link milestone id + acceptance checklist copy-paste (unchecked → checked).
- Never commit ephemeral third-party binaries or npm vendor trees under `/tmp`.
- No third-party co-author trailers (see AGENTS.md).

---

## 11. Risk register (active)

| Risk | Decision |
|---|---|
| ratatui 0.30 friction | M0: stay 0.30 unless blocked; pin exact; fallback 0.29 documented in AUDIT |
| tui-textarea incompatible | Vendor ~300 LOC editor in M1 (spec already allows) |
| StreamSink API break | Optional methods + adapter in `sink.rs` preferred over big-bang trait break |
| Permission mid-turn incomplete | Block M6 if M0.2 green flags not met |
| Drag selection rabbit hole | M9 keyboard first; drag optional |
| Scope creep apply_patch | Reject in TUI PRs; separate issue |

---

## 12. Success metrics

| Metric | Target |
|---|---|
| Modern TUI product bar | Daily-driver polish without sacrificing engine wins |
| Product bar | Appendix C all green on support matrix |
| Perf | §7 budgets in M10 smoke |
| Classic | golden / scripted session unchanged |
| Default | modern after M10 only |

---

## 13. Immediate next action

1. **Commit or stash** current scaffold + design docs on `feat/tui-modern-overhaul` so M0 has a clean baseline (human decision — do not force-push).
2. Open **M0** work: produce `docs/tui/AUDIT.md` from live `rg` + file reads.
3. Do not start M1 loop rewrite until M0 acceptance boxes are green.

---

*This plan of attack freezes sequencing and scaffold deltas. Behavior details remain in the implementation report and `docs/tui/ACCEPTANCE.md`.*
