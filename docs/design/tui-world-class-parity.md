# World-class TUI parity plan

**Status:** active execution track  
**Bar:** modern TUI at a **world-class agent-screen** product bar, then close remaining product gaps.

Related: `docs/tui/ACCEPTANCE.md`, `docs/tui/KEYBINDINGS.md`, `docs/design/tui-modern-plan-of-attack.md`.

---

## 1. Mission

Make the fullscreen TUI the daily driver that delivers a complete agent-screen on:

- Trust (cancel, modes, terminal restore, clipboard, no flicker)
- Scrollback as a document (blocks, fold, copy, free/follow)
- Composer control (multiline, queue, **interject / send-now**)
- HITL modals that never orphan or hang
- Tasks / session chrome sufficient for multi-agent work

Then close **engine / product** gaps that are not pure chrome (checkpoints, grants, sandbox profiles, headless flags, foreign-session later).

**Keep forever:** multi-provider, MIT, no default telemetry, embeddable lib, security-scan, protected-dir / bypass-lock invariants, 3-crate layout.

---

## 2. Definition of “parity or better”

### Agent-screen contract (must pass)

| # | Behavior | Better-than-parity stretch |
|---|---|---|
| A1 | **Esc never cancels a turn** (modal dismiss / clear draft / double-quit only). **Ctrl+C cancels.** | Draft-aware: Ctrl+C with text clears draft first, second cancels |
| A2 | Mid-stream **Free scroll never jumps**; jump pill + End follow | Unread line count on pill |
| A3 | **Mode badge** always visible; Shift+Tab mid-turn affects next permission | Always-approve chord + YOLO pin respect |
| A4 | Permission / plan / question modals **survive turn complete**; never ghost | Scrollable long tool input; origin attribution |
| A5 | **Queue** mid-turn; empty Enter sends when idle; queue survives abort | Queue pane + send-now selected row |
| A6 | **Interject**: cancel current turn and send composer now (Ctrl+Enter + alts) | Hold-queue when blocked on bg tasks (hint) |
| A7 | Typed **blocks** (user / assistant md / thinking / tool / system / subagent) | Fold, expand-all thinking, raw toggle |
| A8 | **Copy** last reply / focused block via native → tmux → OSC 52 | Block `y` / metadata `Y` |
| A9 | Multiline composer; history recall on ↑ when empty | `!` shell mode later |
| A10 | Clipboard + `/terminal-setup` honest on tmux/SSH | Per-terminal chord remediation |
| A11 | Tasks pane: needs-input first, kill confirm, live status | Attach/detach lite |
| A12 | Idle: no continuous repaint; stream coalesced ≤10 fps | Resize storm stable |
| A13 | Panic/exit restores terminal (no focus-seq leak) | SIGTSTP safe |
| A14 | Unknown slash rejected; skills expand; modern routes **full** slash set (or palette) | Ctrl+P command palette |

### Full feature parity scope (active)

| Surface | Status |
|---------|--------|
| Agent-screen chords + composer | Done (local WIP) |
| Fold / history / queue pane / y copy | Done (local WIP) |
| **Full classic slash bridge** + Tab complete | Done (local WIP) |
| **`!` shell passthrough** | Done (local WIP) |
| First-run setup hero + modern default | Done (local WIP) |
| Mouse drag text selection | Remaining |
| In-TUI command palette (Ctrl+P) | Done (local) |
| Full theme pack in modern chrome | Remaining (skin + classic themes partial) |
| Session picker / multi-session dashboard | Remaining (engine+E1) |
| File-snapshot rewind, foreign sessions, media | Product gaps after agent-screen |

**Bar:** modern TUI is the only interactive surface; headless (`-p`) covers scripts/CI.

---

## 3. Execution waves

| Wave | Name | Outcome | Target |
|---|---|---|---|
| **T0** | Trust bar | A1, A3 (verify), A10, A12, A13 + dogfood ACCEPTANCE on ≥3 terminals | 1 week |
| **T1** | Document scrollback | A2, A7, A8 block copy | 2–3 weeks |
| **T2** | Composer & control | A5–A6, A9, A14 routing | 2 weeks |
| **T3** | HITL + cards | A4 polish, edit diffs, bash spill | 1–2 weeks |
| **T4** | Agents & sessions chrome | A11, session picker modal | 2 weeks |
| **T5** | Polish | Themes, selection drag, in-TUI help, vim optional | 2–3 weeks |
| **E1+** | Engine gaps | Grants, sandbox profiles, headless flags, file checkpoints | after T2 solid |

One PR ≈ one vertical slice ≤ ~1.5k non-test LOC. Tests in same PR. `fake_engine` for loop-spanning behavior.

---

## 4. T0 checklist (start now)

- [x] Split Esc vs Ctrl+C in `ui/modern/run.rs` (Esc ≠ cancel mid-turn)
- [x] Update KEYBINDINGS + tests (delete “Esc cancels turn” codification)
- [x] Shared clipboard helper: native CLI tools + OSC 52 fallback; wire `/copy` + modern path
- [x] `/terminal-setup` reports clipboard routes + truecolor + sync + tmux
- [ ] Dogfood ACCEPTANCE rows on real terminals (SUPPORT.md)
- [ ] Confirm idle dirty-flag / coalescer (regression tests already exist)

---

## 5. Post-TUI product gaps (lose them after agent-screen)

| Pri | Gap |
|:---:|---|
| P1 | Persistent project allow/deny grants |
| P1 | Sandbox named profiles + docs |
| P1 | Headless `--tools` / denylist / continue-last polish |
| P1 | File checkpoint rewind (not only message peel) |
| P1 | Modern wires every classic slash (or palette) |
| P2 | Foreign session resume (other coding agents’ session stores) |
| P2 | External OTEL double opt-in |
| P2 | Subagent personas polish |
| P3 | Media / diagrams / voice |

---

## 6. Success metrics

- Team uses **only** modern TUI for real work for 2 consecutive weeks
- ACCEPTANCE green on kitty + (wezterm|ghostty) + tmux nested
- A1–A14 checked in KEYBINDINGS / ACCEPTANCE
- No regression on headless (`-p`) / serve / ACP
- Multi-provider + no default telemetry + security invariants unchanged

---

## 7. Immediate next PR

**Done (local):** agent-screen core · setup polish · **full classic `/` bridge** (stdout capture) · **Tab complete** · **`!` shell** · y/Y · queue pane · fold · history · interject · composer.

**Next:** mouse text selection · Ctrl+P palette polish · theme pack in modern chrome · ACCEPTANCE dogfood · multi-session dashboard.
