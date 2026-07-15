# agent-code modern TUI

Docs for the fullscreen pager track (default interactive UI; `ui.tui = "modern"`).
Classic rustyline REPL remains available via `--tui classic` / `ui.tui = "classic"`.

| Doc | Purpose |
|---|---|
| [AUDIT.md](./AUDIT.md) | M0 reality map: modules, StreamSink gaps, version stance |
| [SUPPORT.md](./SUPPORT.md) | Terminal support matrix (filled in M10) |
| [ACCEPTANCE.md](./ACCEPTANCE.md) | Appendix C product-bar checklist (filled in M10) |
| [KEYBINDINGS.md](./KEYBINDINGS.md) | Modern TUI keybindings reference |

## Design sources (repo)

- `docs/design/tui-modern-plan-of-attack.md` — execution plan  
- `docs/design/tui-modern-overhaul.md` — design sketch  
- `docs/design/harness-comparison-2026-07.md` — anonymized peer matrix  
- `docs/design/reference-pager-binary-forensics.md` — public-binary architecture notes  

## Issue tree

Epic **#385**. Milestones M0–M10: **#386–#396**. Agent-ready leaves under M0: **#397–#401**, docs **#408**.

## Parallel ownership (PR #415 handoff)

| Track | Paths | Status |
|---|---|---|
| **Engine (M0 + HITL surface)** | `crates/lib/**`, `docs/tui/**` | **Done** — see AUDIT.md |
| **UI (M1–M10)** | `crates/cli/src/ui/modern/**` | M1–M9 landed; M10 dogfood + default flip |

Shared interface: additive `StreamSink` / `QuestionAsker` / `PermissionPrompter` in lib.

## Default flip checklist (#396)

1. Fill [SUPPORT.md](./SUPPORT.md) matrix on real terminals  
2. Green [ACCEPTANCE.md](./ACCEPTANCE.md) product bar  
3. Default is modern; classic remains opt-in via `--tui classic`

## fake_engine test harness (#406)

`crates/cli/src/ui/modern/fake_engine.rs` drives the **real**
`run::event_loop` in integration tests — do not test the loop by poking
`App` reducers alone when the behavior spans the loop (turn reaping,
modal FIFO, queue auto-send, mid-turn mode).

How it works (all under `#[tokio::test(start_paused = true)]`, hermetic):

- **Fake the provider, not the loop.** `ScriptedProvider` plays a
  per-turn script of raw `StreamEvent`s with virtual-time delays; every
  real layer above it runs: `QueryEngine`, tool execution, permission
  prompter → modal → response, `ChannelSink`, coalescer, `event_loop`.
- **Scripted terminal.** `ScriptedTerm::play(vec![(at, Event)…])` feeds
  crossterm events at virtual offsets; closing the script quits the loop
  (same as production EOF).
- **TestBackend frames.** `run_script(harness, script)` renders to a
  ratatui `TestBackend` and returns the final `App` + frame count.
- **Mid-turn assertions.** `Harness` exposes the engine's lock-free
  handles (`live_plan`, `permissions`) so tests can assert engine state
  *while the turn still holds the engine mutex* (see
  `shift_tab_mid_turn_applies_live_mode`).

Start from an existing test: `end_to_end_prompt_streams_and_completes`
is the minimal template; `permission_modal_allow_once_runs_tool` shows
the HITL round-trip; `ctrl_c_cancels_streaming_turn_quickly` shows the
virtual-time latency-bound pattern.
