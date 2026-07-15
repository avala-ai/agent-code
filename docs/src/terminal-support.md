
The modern TUI uses alternate-screen mode, mouse reporting, bracketed paste, and optional synchronized output. Most issues come from multiplexers (tmux/zellij), SSH, or missing truecolor.

## Live diagnostics

```text
/terminal-setup
```

Reports detected capabilities (truecolor, sync-output, kitty keyboard, tmux) and remediation lines. Also available as part of first-run hygiene.

## Recommended terminals

Well-tested for fullscreen TUIs: **Kitty**, **WezTerm**, **Ghostty**, **iTerm2**, **Alacritty**, **Windows Terminal**, modern VS Code / Cursor integrated terminals.

## Truecolor

```bash
# ~/.bashrc or ~/.zshrc
export COLORTERM=truecolor
```

Inside **tmux**:

```tmux
set -g default-terminal "tmux-256color"
set -as terminal-features ",*:RGB"
```

## tmux clipboard & passthrough

```tmux
set -g set-clipboard on
set -g allow-passthrough on
set -g focus-events on
```

Reload: `tmux source-file ~/.tmux.conf`.

Without passthrough, OSC 52 clipboard writes and some capability queries may not reach the outer terminal.

## Clipboard routes

Agent Code prefers native tools when available:

| Platform | Tools |
|----------|--------|
| macOS | `pbcopy` |
| Linux Wayland | `wl-copy`, then `xclip` / `xsel` |
| Linux X11 | `xclip` / `xsel` |
| Windows | `clip` |

Slash: `/copy` copies the last assistant message. Modern TUI will grow block-level copy + OSC 52 fallback as part of the world-class TUI track.

## Keyboard protocol

Shift+Enter / disambiguated chords need the **Kitty keyboard protocol** (or equivalent). If your terminal lacks it, `/terminal-setup` suggests alts (e.g. Alt+Enter for newline).

| Problem | Fix |
|---------|-----|
| Ctrl+C does nothing | Some hosts steal it; try Ctrl+Shift+C / Cmd+C |
| Esc seems to cancel | It should **not** in modern TUI — update to latest; use Ctrl+C to cancel |
| Flicker under heavy stream | Prefer terminals with sync-output; avoid nested tmux without RGB |
| Focus junk after exit (`^[[I`) | Upgrade; restore path disables focus reporting |

## SSH

Remote sessions often lose `TERM_PROGRAM` and clipboard. Prefer:

- Truecolor-capable `TERM` forwarded
- Copy via terminal-native selection (Shift+Insert) when OSC 52 is blocked
- Run Agent Code locally against a remote checkout when possible

## Related

- [TUI overview](./tui/README.md)
- [Keyboard shortcuts](../tui/KEYBINDINGS.md)
- [Support matrix](./tui/SUPPORT.md)
