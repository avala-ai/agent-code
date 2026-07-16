# Modern TUI keybindings

Interactive sessions use the fullscreen TUI.

## Global

| Key | Action |
|---|---|
| `Enter` | Submit prompt · while a turn runs: queue it · on an empty prompt while idle: send next queued prompt |
| `Ctrl+Enter` (alt: `Ctrl+I`) | **Send now / interject**: cancel the live turn and send the composer (or the head of the queue if empty) |
| `Shift+Tab` | Cycle mode: Manual → Normal → AcceptEdits → Plan (applies mid-turn) |
| `Esc` | **Never cancels a turn.** Modal: deny/dismiss only · non-empty prompt: clear draft · idle empty: press twice within 1.5 s to quit · mid-turn empty: no-op (status hints Ctrl+C) |
| `Ctrl+C` (also `Ctrl+Shift+C`, `Cmd+C`) | Modal: deny/dismiss **and** cancel turn · mid-turn with draft: clear draft first · mid-turn empty: **cancel turn** · idle empty: press twice within 1.5 s to quit |
| `Ctrl+D` | Quit (empty prompt only) |
| `Ctrl+T` | Toggle tasks/agents pane |
| `Ctrl+;` / `Ctrl+'` | Toggle **queue pane** (full list) |
| `Ctrl+L` | Force full redraw |

## Prompt editing (composer)

Rounded bordered field with `❯` prefix. Height grows with content.

| Key | Normal mode (default) | Multiline mode (`Ctrl+M`) |
|---|---|---|
| `Enter` | **Send** | Insert newline |
| `Alt+Enter` / `Shift+Enter` | Insert newline | **Send** |
| `Ctrl+Enter` / `Ctrl+I` | Interject (cancel + send now) | same |
| `Ctrl+M` | Toggle multiline mode | Toggle off |
| Paste (bracketed) | Insert at cursor (newlines kept) | same |
| `Backspace` / `←` / `→` | Edit / move cursor | same |
| `↑` / `↓` | Scroll transcript (or move lines if draft is multi-line) | Move within draft |
| `Home` / `End` | Line start/end when drafting; transcript top/bottom if empty | same |
| `Alt+↑` | Pop newest queued prompt into editor | same |
| `Alt+-` | Delete newest queued prompt | same |
| `Shift+Tab` | Cycle permission mode (Manual → Normal → AcceptEdits → Plan) | same |

## Transcript / scrollback

| Key | Action |
|---|---|
| `↑` / `↓` | Scroll (Free — stream never jumps). Empty composer: browse **prompt history** |
| `PageUp` / `PageDown` | Page |
| `Ctrl+U` | Half page up |
| `Home` / `End` | Transcript top/bottom when draft empty; line bounds when drafting |
| `Shift+←` / `Shift+→` | Jump to previous / next **user turn** (select + scroll) |
| `←` / `→` (empty composer) | Select previous / next transcript block (`▌` marker) |
| `e` (empty + block selected) | Expand / collapse tool body, thinking, long assistant |
| `Ctrl+E` | Expand / collapse **all** thinking blocks |
| `y` (block selected) | **Copy block body** (clipboard cascade) |
| `Y` (block selected) | **Copy block metadata** (e.g. tool name · detail) |
| Mouse wheel | Scroll |
| Click bottom transcript row | Jump to live tail (Follow) |

Tool results start collapsed (`… +N more · e expand`).

## Queue pane (`Ctrl+;` or `/queue`)

| Key | Action |
|---|---|
| `↑` / `↓` | Move selection |
| `Enter` (empty composer) | **Send now** selected row (cancels live turn if needed) |
| `Backspace` | Drop selected row |
| `Ctrl+;` | Close pane |

Compact chips still show above the composer when the pane is closed.

## Modals

| Key | Permission | Plan review | Question |
|---|---|---|---|
| `y` / `1` | Allow once | — | — |
| `a` / `2` | Allow for session | Approve | — |
| `n` / `3` | Deny | — | — |
| `k` | — | Keep planning | — |
| `↑` `↓` `Enter` | — | — | Move / select |
| `1`–`9` | — | — | Select option N |
| `Esc` | Deny (turn continues) | Reject | Dismiss ask (turn continues) |
| `Ctrl+C` | Deny + cancel turn | Reject | Dismiss + cancel turn |

## Slash commands

### Slash commands (full classic bridge)

**Every** built-in from classic REPL is available: type `/` + name, **Tab** to complete.
Output is captured into the transcript (alt-screen safe). `CommandResult::Prompt`
commands inject a model turn (e.g. `/diff`, `/review`).

Fast-path locals (no engine lock): `/help` `/clear` `/copy` `/cost` `/usage`
`/version` `/status` `/plan` `/theme` `/permissions` `/queue` `/tasks` `/model`
`/terminal-setup` `/minimal` `/fullscreen` `/stats` `/exit`

Plus user-invocable **skills**. Truly unknown `/names` are rejected with a hint.

### Input prefixes

| Prefix | Action |
|--------|--------|
| `!cmd` | Run shell now; stream into transcript + inject into engine context |
| (plain text) | Agent turn (queued mid-stream) |

`/copy` and `y`/`Y` use the clipboard cascade: native → tmux buffer → OSC 52.
