
Type `/` followed by a command name in the REPL. Unknown commands are passed to the agent as prompts. Skill names work as commands too.

## Session

| Command | Description |
|---------|-------------|
| `/help` | Show all commands and loaded skills |
| `/exit` | Exit the REPL |
| `/clear` | Reset conversation history |
| `/resume <id>` | Restore a previous session by ID |
| `/sessions` | List recent saved sessions |
| `/export` | Export conversation to markdown file |
| `/share` | Export session as shareable markdown with metadata |
| `/summary` | Ask the agent to summarize the session |
| `/version` | Show agent-code version |

## Context

| Command | Description |
|---------|-------------|
| `/cost` | Token usage and estimated session cost |
| `/context` | Context window usage and auto-compact threshold |
| `/compact` | Free context by clearing stale tool results |
| `/model [name]` | Show or switch the active model (interactive picker) |
| `/verbose` | Toggle verbose output (shows token counts) |

## Git

| Command | Description |
|---------|-------------|
| `/diff` | Show current git changes |
| `/status` | Show git status |
| `/commit [msg]` | Review diff and create a commit |
| `/review` | Analyze diff for bugs and issues |
| `/branch [name]` | Show or switch git branch |
| `/log` | Show recent git history |

## Agent Control

| Command | Description |
|---------|-------------|
| `/plan` | Toggle plan mode (read-only) |
| `/permissions` | Show permission mode and rules |
| `/agents` | List available agent types |
| `/tasks` | List background tasks |

## Configuration

| Command | Description |
|---------|-------------|
| `/init` | Create `.agent/settings.toml` for this project |
| `/doctor` | Check environment health (tools, config, git) |
| `/config` | Show current configuration |
| `/mcp` | List connected MCP servers |
| `/hooks` | Show hook configuration |
| `/plugins` | List loaded plugins |
| `/memory` | Show loaded memory context |
| `/skills` | List available skills |
| `/theme` | Show and configure color theme |
| `/color [name]` | Switch color theme mid-session |
| `/features` | Show enabled feature flags |

## History

| Command | Description |
|---------|-------------|
| `/scroll` | Scrollable view of conversation history |
| `/transcript` | Show conversation transcript with message indices |
| `/rewind` | Undo the last assistant turn |
| `/snip <range>` | Remove messages by index (e.g., `/snip 3-7`) |
| `/fork` | Branch the conversation from this point |

## Diagnostics

| Command | Description |
|---------|-------------|
| `/stats` | Session statistics (turns, tools used, cost) |
| `/files` | List working directory contents |
| `/release-notes` | Show release notes for the current version |
| `/feedback <msg>` | Submit feedback or suggestions |
| `/bug` | Report a bug (opens GitHub issues link) |

## Editing

| Command | Description |
|---------|-------------|
| `/vim` | Switch to vi editing mode |
| `/emacs` | Switch to emacs editing mode |
