# Reference pager binary forensics

**Date:** 2026-07-09  
**Purpose:** Architecture insights for agent-code’s modern TUI (branch `feat/tui-modern-overhaul`).  
**Method:** Download **public** release install artifacts for a closed-source terminal coding agent and extract **string / path** evidence from stripped binaries.  
**Not:** Official source, decompilation to recreate proprietary code, or redistribution of binaries.

---

## 1. What was examined

Public install CDN artifacts for a multi-platform **static native binary** product (linux/macOS/Windows, x86_64 and aarch64). Sampled stable + alpha channels in mid-2026.

| Observation | Approx size (linux x64) |
|---|---|
| Static-pie ELF, stripped | ~150–155 MB |
| Same first-party crate path prefixes on every OS | shared monorepo, not per-OS rewrites |

**Conclusion on packaging:** one fat native binary per OS/arch — not a Node/npm app at runtime.

---

## 2. Language and UI stack (evidence strength)

| Claim | Evidence | Confidence |
|---|---|---|
| Implemented in **Rust** | First-party `crates/…/*.rs` path strings; crates.io paths; tokio/rustls/serde massively present | Very high |
| Primary TUI = **ratatui + crossterm** | String hits for `ratatui`, `crossterm`; layout solver deps; changelog notes about ratatui buffers | Very high |
| Custom ratatui extensions | First-party inline/textarea widget crates | High |
| Async runtime **tokio** | Hundreds of hits | Very high |
| Markdown: pulldown-cmark + syntect | Hits + first-party markdown crates | High |
| Clipboard / browser / ACP | arboard, chromiumoxide, agent-client-protocol strings | High |

**Not published as official architecture docs.** This is binary/string forensics only.

---

## 3. Internal crate map (first-party)

Observed split (names cleaned to roles):

### Presentation / TUI
- **pager** — main fullscreen app (agent view, scrollback, dispatch, slash, mouse)
- **pager-render** — draw pipeline, clipboard, terminal probes, theme
- **pager-minimal** — minimal skin
- **pager-bin** — entry
- custom ratatui widgets (inline, textarea)
- markdown + mermaid helpers

### Shell / session host
- **shell** — agent host, auth, extensions, subagents, config reload
- shell-base / session-support

### Agent engine
- **agent** — loop, plugins, prompts, skills, AGENTS.md
- tools, subagent-resolution, chat-state, hunk-tracker, codebase-graph

### Platform services
- MCP, hooks, memory, sandbox, workspace, auth, config, models
- compaction, update, browser-tools, plugin marketplace
- ACP, fsnotify, worktree, telemetry, crash-handler

Matches a **large monorepo** with a clear split: **pager (UI) ≠ shell (host) ≠ agent (loop)**.

---

## 4. Pager architecture (from path strings)

### Event / app core
```
pager/src/app/
  event_loop.rs
  app_view.rs
  mouse.rs
  signal_handler.rs
  agent_view/     # input, queue, render, selection, paste, media, modals, session
  dispatch/       # router, prompt, queue, permissions, modes, turn, transcript, dashboard
```

### Scrollback = typed blocks (key insight)
```
scrollback/
  entry.rs, render.rs, scrollback_pane.rs
  text_selection.rs, table_geometry.rs
  blocks/   # user, thinking, markdown, tool/{edit,execute,search,other}
  state/    # layout, nav, selection
```

### Mapping for agent-code modern TUI
| Reference pattern | agent-code mapping |
|---|---|
| event_loop | `ui/modern/run.rs` |
| dispatch (router, modes, permissions) | `input.rs` + `app.rs` reducers |
| agent_view (queue, modals, paste) | `render/prompt.rs`, `queue.rs`, `render/modal.rs` |
| scrollback blocks + layout | `blocks/`, `layout_cache.rs`, `render/transcript.rs` |
| pager-minimal as separate render layer | skin = render config, not a fork (M10) |

---

## 5. Takeaways for agent-code

1. **Stay on Rust + ratatui** — production-validated for this product class.  
2. **Typed scrollback blocks** — not a flat string log.  
3. **Shell/render/engine separation** — UI never blocks engine; engine never draws.  
4. **Minimal skin is config**, not a second app.  
5. Expect stock widgets to run out of road at prompt editor / inline cards — vendor thin widgets when needed.

## 6. Ethics / legal

- Binaries are **proprietary**. Do not commit them to this repo.  
- Insights only; never port strings/paths into shipped product branding.  
- Local analysis under `/tmp/…` is ephemeral.

---

*Blueprint: closed-source Rust monorepo, static native binaries, dedicated pager stack (ratatui/crossterm + typed blocks), separate shell and agent crates. agent-code already chose the right tech stack; this track finishes the presentation layer.*
