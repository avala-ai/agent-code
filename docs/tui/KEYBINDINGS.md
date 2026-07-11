# Modern TUI keybindings

The default interactive surface (`ui.tui = "modern"`). Classic REPL: `--tui classic`.

## Global

| Key | Action |
|---|---|
| `Enter` | Submit prompt · while a turn runs: queue it · on an empty prompt while idle: send next queued prompt |
| `Shift+Tab` | Cycle mode: Manual → Normal → AcceptEdits → Plan (applies mid-turn) |
| `Esc` / `Ctrl+C` (also `Ctrl+Shift+C`, `Cmd+C`) | Modal open: deny/dismiss (and stop the turn) · turn streaming: cancel turn · prompt non-empty: clear prompt · else: press twice within 1.5 s to quit |
| `Ctrl+D` | Quit (empty prompt only) |
| `Ctrl+T` | Toggle tasks/agents pane |
| `Ctrl+L` | Force full redraw |

## Prompt editing

| Key | Action |
|---|---|
| Paste (bracketed) | Inserts into the prompt |
| `Backspace` / `←` / `→` | Edit / move cursor |
| `Alt+↑` | Pop newest queued prompt back into the editor |
| `Alt+-` | Delete newest queued prompt |

## Transcript

| Key | Action |
|---|---|
| `↑` / `↓` | Scroll (enters Free scroll; new content never moves your viewport) |
| `PageUp` / `PageDown` | Page |
| `Ctrl+U` | Half page up |
| `Home` / `End` | Top / bottom (End returns to Follow) |
| Mouse wheel | Scroll |
| Click bottom transcript row | Jump to live tail (Follow) |

## Modals

| Key | Permission | Plan review | Question |
|---|---|---|---|
| `y` / `1` | Allow once | — | — |
| `a` / `2` | Allow for session | Approve | — |
| `n` / `3` | Deny | — | — |
| `k` | — | Keep planning | — |
| `↑` `↓` `Enter` | — | — | Move / select |
| `1`–`9` | — | — | Select option N |
| `Esc` / `Ctrl+C` | Deny | Reject | Dismiss (fails the ask) |

## Slash commands

`/help` `/clear` `/model [name]` `/terminal-setup` `/minimal` `/fullscreen`
`/stats` `/exit` (`/quit`), plus any user-invocable **skill** (`/commit`,
`/review`, …) which expands to its prompt. Unknown `/commands` are rejected
with a hint — they are never sent to the model.
