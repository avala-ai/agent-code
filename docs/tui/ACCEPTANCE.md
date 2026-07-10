# Modern TUI acceptance (Appendix C)

**Gate for:** modern-as-default product bar — issue **#396** (default flipped; classic removed)  
**Status:** ready for dogfood recording (not verified on this host — `TERM=dumb`)  
**How to run:** build release binary, then on each terminal in [SUPPORT.md](./SUPPORT.md):

```bash
cargo build -p agent-code --release
./target/release/agent
# optional: AGENT_CODE_TUI=modern in config [ui] tui = "modern"
```

Mark each item **pass / fail / skip** per terminal. Failures block default flip unless waived with a SUPPORT note.

---

## Product bar checklist

Copy this block into a per-terminal result section in SUPPORT.md.

### Modes & cancel
- [ ] Mode badge visible in every state (idle / streaming / modal)
- [ ] Shift+Tab mid-turn changes badge immediately and affects the **next** permission decision
- [ ] Ctrl+C cancels a long tool ≤150 ms wall-clock; UI returns to Idle cleanly
- [ ] Esc never cancels a turn (clears prompt / closes modal only)

### Scroll & stream
- [ ] Stream a long answer, PgUp mid-stream, read ~30 s: viewport never jumps
- [ ] Jump-to-bottom pill shows and counts new lines; End / Enter on pill returns Follow
- [ ] Idle ~60 s: no continuous repaints (frame counter / `top` spot-check)

### Terminal hygiene
- [ ] tmux: no obvious flicker during heavy streaming (sync-update)
- [ ] After exit: no `^[[I` / `^[[O` focus-seq leak in the shell
- [ ] OSC 52 copy works (or documented fallback); tmux needs passthrough

### HITL
- [ ] Permission modal: long tool input scrollable; `y` once / `a` session / `n` deny
- [ ] Allow-session suppresses an identical follow-up prompt
- [ ] Plan approval modal after ExitPlanMode: approve / keep planning / dismiss
- [ ] AskUserQuestion modal: options selectable; no stdin hang under modern TUI
- [ ] Bg-subagent permission shows origin attribution when present

### Queue & agents
- [ ] Two prompts queued mid-turn survive MaxTurns / error; still sendable
- [ ] Two subagents: tasks pane order (needs-input first); attach/detach keeps work alive
- [ ] Kill subagent confirms and reflects failed/killed state

### Robustness
- [ ] Large bash output: UI stays responsive (live tail if wired; spill/open if present)
- [ ] Resize storm (drag corner): no panic, no corruption, correct reflow
- [ ] Panic restore: terminal leaves alt-screen/raw mode (`/debug-panic` if available)
- [x] classic REPL removed; interactive path always modern

---

## Classic regression (required)

```bash
./target/release/agent -p "reply with only: ok"
# expect normal print path, process exits 0
```

- [ ] Classic one-shot path OK on Linux CI-equivalent
- [ ] Classic interactive REPL starts and quits with Ctrl+D / Ctrl+C

---

## Sign-off

| Role | Name | Date | Verdict |
|---|---|---|---|
| Engine track | complete (PR #415 M0–HITL surface) | 2026-07-10 | n/a — not a dogfood signer |
| UI track | | | |
| Product / default flip | | | ☑ modern default · classic removed |

Do **not** flip `[ui] tui` / auto default until SUPPORT matrix has at least: kitty, wezterm **or** ghostty, tmux nested, VS Code, one macOS terminal, Windows Terminal (or explicit skip with reason).
