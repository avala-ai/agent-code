# Terminal support matrix

**Status:** dogfood in progress — cells blank until filled on real hardware.  
**Related:** [ACCEPTANCE.md](./ACCEPTANCE.md) · issue **#396** · PR **#415**

## How to record a result

1. Open the terminal under test (native, not nested unless testing tmux).
2. `cargo build -p agent-code --release && ./target/release/agent`
3. Run the ACCEPTANCE checklist (10–20 minutes).
4. Fill one row below: `Y` = pass, `N` = fail, `-` = skip/N/A, `?` = not run.
5. Paste a short notes line (version strings, fail details, tmux config).

### Suggested probes

| Probe | How |
|---|---|
| Fullscreen | alt-screen fills window; resize keeps layout |
| Sync update | stream long markdown under tmux — no full-frame flash |
| OSC 52 | select text / copy command if implemented; paste in browser |
| Kitty keys | Shift+Enter newline if enhanced keyboard enabled |
| Mouse | wheel scrolls Free; click jump-pill if M9 landed |
| Focus leak | exit agent, type in shell — no garbage CSI sequences |

### tmux prerequisites (copy into notes if used)

```tmux
set -g allow-passthrough on
set -g focus-events on
set -g set-clipboard on
```

---

## Matrix

| Terminal | OS | Fullscreen | Sync | OSC52 | Kitty keys | Mouse | Focus leak OK | Notes |
|---|---|---|---|---|---|---|---|---|
| kitty | Linux | ? | ? | ? | ? | ? | ? | |
| wezterm | Linux | ? | ? | ? | ? | ? | ? | |
| ghostty | Linux | ? | ? | ? | ? | ? | ? | |
| alacritty | Linux | ? | ? | ? | ? | ? | ? | |
| GNOME Terminal | Linux | ? | ? | ? | ? | ? | ? | |
| tmux@kitty | Linux | ? | ? | ? | ? | ? | ? | |
| iTerm2 | macOS | ? | ? | ? | ? | ? | ? | |
| Terminal.app | macOS | ? | ? | ? | ? | ? | ? | |
| VS Code integrated | * | ? | ? | ? | ? | ? | ? | |
| Windows Terminal | Windows | ? | ? | ? | ? | ? | ? | |
| WSL + WT | Windows | ? | ? | ? | ? | ? | ? | |

### CI / headless host (this workspace)

| Environment | Result |
|---|---|
| Linux GPU host, `TERM=dumb` (agent harness) | **Cannot dogfood** interactive fullscreen — no real TTY. Engine unit tests + `cargo clippy/test` only. |

Classic rustyline REPL has been removed; modern is the only interactive surface.

---

## Failure log (append)

```
### YYYY-MM-DD — <terminal> @ <os>
- FAIL: <checklist item>
- repro: ...
- workaround: ...
```
