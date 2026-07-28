# Modern TUI keybindings

Interactive sessions use the fullscreen TUI.

## Global

| Key | Action |
|---|---|
| `Enter` | Submit prompt · while a turn runs: queue it · on an empty prompt while idle: send next queued prompt |
| `Ctrl+Enter` (alt: `Ctrl+I`) | **Send now / interject**: cancel the live turn and send the composer (or the head of the queue if empty) |
| `Shift+Tab` | Cycle mode: Manual → Normal → AcceptEdits → Auto → Plan (applies mid-turn) |
| `Esc` | **Never cancels a turn.** Modal: deny/dismiss only · non-empty prompt: clear draft · idle empty: press twice within 1.5 s to quit · mid-turn empty: no-op (status hints Ctrl+C) |
| `Ctrl+C` (also `Cmd+C` / Super+C) | Modal: deny/dismiss **and** cancel turn · mid-turn with draft: clear draft first · mid-turn empty: **cancel turn** · idle empty: press twice within 1.5 s to quit · **not** `Ctrl+Shift+C` (that is copy) |
| `Ctrl+D` | Quit (empty prompt only) |
| `Ctrl+T` | Toggle tasks/agents pane |
| `↑`/`↓` | Move the tasks-pane selection (pane open, empty composer) |
| `Enter` | Open the selected background task's output — on a folded group heading, unfold it (pane open, empty composer) |
| `Space` | Fold / unfold the selected group in the tasks pane. A folded group stays selectable through its heading (`▸ agents (3)`) |
| `Ctrl+P` / `?` | **Command palette** — filter slash commands, Enter fills `/cmd ` |
| `Ctrl+.` / `Ctrl+X` | **Keyboard shortcuts** overlay |
| `Ctrl+Shift+C` | Copy mouse selection, else last assistant reply |
| `Ctrl+;` / `Ctrl+'` | Toggle **queue pane** (full list) |
| `Ctrl+L` | Force full redraw |
| `/resume` | Open the session picker (filter, `↑↓`, Enter resumes) |
| `Ctrl+F` | Search the transcript (smart case; `↓`/`Ctrl+N` next, `↑`/`Ctrl+P` prev, `Enter` closes keeping the match position, `Esc` closes restoring the original position) |

## Prompt editing (composer)

Rounded bordered field with `❯` prefix. Height grows with content.

| Key | Normal mode (default) | Multiline mode |
|---|---|---|
| `Enter` | **Send** | Insert newline |
| `Alt+Enter` / `Shift+Enter` | Insert newline | **Send** |
| `Ctrl+Enter` / `Ctrl+I` | Interject (cancel + send now) | same |
| `Ctrl+M` | **Empty composer / block selected:** model picker · **drafting:** toggle multiline | Toggle off |
| Paste (bracketed) | Insert at cursor (newlines kept) | same |
| `Backspace` / `←` / `→` | Edit / move cursor | same |
| `↑` / `↓` | Scroll transcript (or move lines if draft is multi-line) | Move within draft |
| `Home` / `End` | Line start/end when drafting; transcript top/bottom if empty | same |
| `Alt+↑` | Pop newest queued prompt into editor | same |
| `Alt+-` | Delete newest queued prompt | same |
| `Tab` | Complete the `@path` mention under the cursor, else a partial `/command` | same |
| `Shift+Tab` | Cycle permission mode (Manual → Normal → AcceptEdits → Auto → Plan) | same |

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
| Thinking status | Status bar: `waiting for model…` → `thinking N.Ns…` → `answering…` · collapsed header: **Thought for Xs** |
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

### Slash commands

**Every** built-in is available: type `/` + name, **Tab** to complete, or **Ctrl+P**
for the filterable command palette. Output is captured into the transcript
(alt-screen safe). `CommandResult::Prompt` commands inject a model turn
(e.g. `/diff`, `/review`).

Fast-path locals (no engine lock): `/help` `/clear` `/copy` `/cost` `/usage`
`/version` `/status` `/plan` `/theme` `/permissions` `/queue` `/tasks` `/model`
`/effort` `/terminal-setup` `/minimal` `/fullscreen` `/stats` `/exit`

**Model:** `/model` or empty-composer `Ctrl+M` opens the in-TUI picker
(↑/↓ · Enter · Tab for effort). `/model <id> [effort]` and `/effort <level>`
switch without the picker. Effort shows on the header badge.

Plus user-invocable **skills** (`/name`, Tab completes, arg hints when set).
Skills load from `.agent/skills`, `.agents/`, Claude/Cursor/Grok compat paths,
and `dir/SKILL.md`. Truly unknown `/names` are rejected with a hint.

### Input prefixes

| Prefix | Action |
|--------|--------|
| `!cmd` | Run shell now; stream into transcript + inject into engine context |
| `@path` | **File mention** — Tab completes, contents are inlined for the model |
| (plain text) | Agent turn (queued mid-stream) |

### `@` file mentions

Type `@` followed by a path to reference a file or directory. A mention is
recognised when `@` starts the line or follows whitespace and the token
contains a `/` or a `.` — so `user@example.com` is never a mention.

**Tab** completes the `@` token under the cursor against the session cwd:
directories complete with a trailing `/` so the next Tab drills in, matching is
case-insensitive, `.gitignore` is honoured, and `.git/` and dotfiles are hidden
(type a leading `.` to see dotfiles). Several mentions on one line complete
independently. With more than one match, Tab extends to the longest common
prefix and lists the candidates in the transcript.

On submit, each mention is resolved against the session cwd and its contents
are appended to the message the model receives. The transcript keeps your line
exactly as typed.

| Case | Behaviour |
|---|---|
| Directory | Expanded to a one-level listing (gitignored entries and `.git/` excluded, capped at 100 entries) |
| Missing, unreadable, or binary file | Skipped, noted under the prompt as `@mentions: …` |
| Path outside the workspace (incl. via symlink) or inside `.git/` | Rejected, noted |
| File over **64 KiB** | Truncated with a `… [truncated: …]` marker |
| More than **256 KiB** across all mentions | Remaining mentions skipped, noted |

`/copy` and `y`/`Y` use the clipboard cascade: native → tmux buffer → OSC 52.

## Custom keybindings

Put a `keybindings.json` in your config directory — `~/.config/agent-code/` on
Linux, `~/Library/Application Support/agent-code/` on macOS,
`%APPDATA%\agent-code\` on Windows. `/keybindings` prints the resolved path.

```json
[
  { "key": "ctrl+k", "action": { "type": "command", "command": "tasks" } },
  { "key": "alt+r",  "action": { "type": "prompt",  "prompt": "run the tests" } },
  { "key": "f5",     "action": { "type": "toggle",  "setting": "queue" } }
]
```

Chord syntax is `ctrl+`/`alt+`/`shift+` prefixes plus a key name — a letter,
`f1`–`f12`, or `up` `down` `left` `right` `home` `end` `pageup` `pagedown`
`enter` `tab` `backspace` `delete` `insert`. Prefixes render in
`ctrl+alt+shift` order (`ctrl+shift+p`). A bare letter with no modifier is
ordinary typing and cannot be bound; a bare `shift+letter` is just a capital
letter and cannot be bound either.

`ctrl+c` and `esc` are reserved and cannot be rebound — they are how you get
out of a stuck state, including a binding that turned out to be a mistake.
This covers modified variants the escape hatches also consume (`ctrl+alt+c`,
`esc` with any modifier); entries for them in the file are ignored with a
warning. `ctrl+shift+c` (copy) remains bindable.
Bindings never fire while a permission prompt (or any modal) is open, and a
binding that runs never discards the prompt you were composing.

The file is read once at startup; `/keybindings` lists the bindings active in
the current session — after editing the file, restart to apply.

## Vi mode

`/vim` (or `edit_mode = "vi"` in config) gives the composer vi bindings.
`Esc` leaves insert mode; the prompt marker changes from `❯` to `▪` so the
mode is always visible.

Normal mode: `h` `l` `0` `$` `w` `b` move (`Backspace` moves left too) ·
`i` `a` `I` `A` insert · `x` delete a character · `D` delete to end ·
`C` change to end · `dd` delete the line · `Enter` submits. Motions and
line commands act on the line under the cursor, so a multi-line draft
keeps its other lines. Unrecognised keys do nothing rather than firing a
global shortcut.

`Esc` in normal mode falls through to the usual behaviour, so the
double-press quit is still reachable. `/emacs` turns the vi bindings back
off; the composer then behaves as it does everywhere else in this
document. It does not add Emacs chords — the composer has none of its
own, and `Ctrl+E` / `Ctrl+U` are the transcript controls listed above.

## Images

Mention an image the way you mention a file — `@screenshot.png` — and it is
attached to the turn as an image rather than inlined as text. Supported:
`png`, `jpg`/`jpeg`, `gif`, `webp`. Other binaries are still skipped with a
reason.
