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
| `Enter` | Open the selected background task's output (pane open, empty composer) |
| `Ctrl+P` / `?` | **Command palette** — filter slash commands, Enter fills `/cmd ` |
| `Ctrl+.` / `Ctrl+X` | **Keyboard shortcuts** overlay |
| `Ctrl+Shift+C` | Copy mouse selection, else last assistant reply |
| `Ctrl+;` / `Ctrl+'` | Toggle **queue pane** (full list) |
| `Ctrl+L` | Force full redraw |

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
