# Modern TUI Overhaul

**Branch:** `feat/tui-modern-overhaul`  
**Issue:** [#385](https://github.com/avala-ai/agent-code/issues/385)  
**Status:** Scaffold + v1 shell

## Problem

The interactive path was a **line-oriented rustyline REPL** with ad-hoc raw mode during turns (`ui/repl.rs` ~2.6k LOC, now removed). Ratatui was only used for inline styling helpers (`ui/tui.rs`), not a real full-screen app. Result: fragile resize/raw-mode interaction and weak discoverability of plan mode / tasks / permissions.

## Goals

1. Full-screen **alt-screen** TUI (ratatui + crossterm) as the primary interactive surface.
2. **Modern chrome**: mode cycle, plan review, task visibility, clear status.
3. **Engine reuse** via `query::Session` + `StreamSink` (no second agent loop).
4. **Classic REPL removed** — modern is the only interactive surface (headless `-p` / serve / ACP remain).
5. **Visual regression tests** using ratatui `TestBackend` (no live TTY required).

## Non-goals (this branch v1)

- Pixel-perfect clones of other products
- Rewriting slash-command handlers (reuse `commands/` progressively)
- Cloud/desktop product surfaces
- Weakening permission/sandbox invariants

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│  modern TUI (alt screen)                                 │
│  Header │ Transcript (scroll) │ Status │ Input │ Overlays │
└──────────────────┬───────────────────────────────────────┘
                   │ UiEvent / EngineEvent channels
┌──────────────────▼───────────────────────────────────────┐
│  Session (Arc<Mutex<QueryEngine>>)                       │
│  spawn_turn → StreamSink → channel → App                 │
└──────────────────────────────────────────────────────────┘
```

### Session modes (Shift+Tab)

Canonical cycle: **Manual → Normal → AcceptEdits → Plan → Manual** (§3.3 / #404).
No always-approve/YOLO mode in the cycle — auto-allow is a config choice
(`[permissions] default_mode`), and sandbox bypass stays engine-enforced via
`security.disable_bypass_permissions`.

| Mode | Engine effect |
|------|----------------|
| Manual | Force `PermissionMode::Ask` — prompt on every tool call |
| Normal | Current permissions / config `default_mode` |
| Accept edits | Auto-allow write tools; other mutations follow config |
| Plan | `state.plan_mode = true` (read-only tools) |

### Module layout

```
crates/cli/src/ui/modern/
  mod.rs          // public run_modern_tui
  app.rs          // App state + reduce
  mode.rs         // SessionMode
  sink.rs         // channel StreamSink
  render.rs       // draw(frame, app)
  run.rs          // terminal + event loop
```

Target layout (M1+) is documented in `docs/design/tui-modern-plan-of-attack.md`.

## Visual testing strategy

1. **Unit / snapshot (CI, hermetic)** — `TestBackend`, deterministic `App` state  
2. **Scripted key paths (CI)** — drive `App::handle_key` without a real terminal  
3. **Manual / dogfood** — `cargo run -p agent-code`  
4. **Future** — optional VHS/asciinema fixtures for docs  

No network in default `cargo test`.

## Migration plan

| Phase | Deliverable |
|-------|-------------|
| 0 | Branch + design + config/flag + empty shell that draws |
| 1 | Session-backed turns + streaming transcript + cancel |
| 2 | Mode cycle + plan badge + plan review overlay |
| 3 | Permission / AskUserQuestion overlays |
| 4 | Task dock, tool cards collapse, slash command palette |
| 5 | Modern-only interactive (classic REPL removed) |

## Entry points

```bash
agent
```

Interactive sessions always open the modern fullscreen TUI.

## Related docs

- Execution plan: `docs/design/tui-modern-plan-of-attack.md`
- Product bar / waves: `docs/design/tui-world-class-parity.md`
- Acceptance checklist: `docs/tui/ACCEPTANCE.md`
