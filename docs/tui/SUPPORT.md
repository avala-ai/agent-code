# Terminal support matrix

**Status:** stub — fill cells during M10 dogfood.  
Do not treat unchecked rows as verified.

| Terminal | OS | Fullscreen | Sync update | OSC 52 | Kitty keys | Mouse | Notes |
|---|---|---|---|---|---|---|---|
| kitty | Linux | ☐ | ☐ | ☐ | ☐ | ☐ | |
| wezterm | Linux | ☐ | ☐ | ☐ | ☐ | ☐ | |
| ghostty | Linux | ☐ | ☐ | ☐ | ☐ | ☐ | |
| alacritty | Linux | ☐ | ☐ | ☐ | ☐ | ☐ | |
| GNOME Terminal | Linux | ☐ | ☐ | ☐ | ☐ | ☐ | |
| tmux (nested) | * | ☐ | ☐ | ☐ | ☐ | ☐ | passthrough required for OSC 52 |
| iTerm2 | macOS | ☐ | ☐ | ☐ | ☐ | ☐ | |
| Terminal.app | macOS | ☐ | ☐ | ☐ | ☐ | ☐ | |
| VS Code terminal | * | ☐ | ☐ | ☐ | ☐ | ☐ | |
| Windows Terminal | Windows | ☐ | ☐ | ☐ | ☐ | ☐ | |
| WSL | Windows | ☐ | ☐ | ☐ | ☐ | ☐ | |

Classic REPL (`--tui classic`) remains the fallback for broken environments.
