
Configuration loads from three layers (highest priority first):

1. **CLI flags and environment variables**
2. **Project config** — `.agent/settings.toml` in your repo
3. **User config** — `~/.config/agent-code/config.toml`

## Full config reference

```toml
# ~/.config/agent-code/config.toml

[api]
base_url = "https://api.anthropic.com/v1"
model = "claude-sonnet-4-20250514"
auth_mode = "api_key"        # "api_key" or "codex_chatgpt"
# api_key is resolved from env: AGENT_CODE_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY
max_output_tokens = 16384
thinking = "enabled"          # "enabled", "disabled", or omit for default
effort = "high"               # "low", "medium", "high"
max_cost_usd = 10.0           # Stop session after this spend
timeout_secs = 120
max_retries = 3

[permissions]
default_mode = "ask"          # "ask", "allow", "deny", "plan", "accept_edits"

[[permissions.rules]]
tool = "Bash"
pattern = "git *"
action = "allow"

[[permissions.rules]]
tool = "Bash"
pattern = "rm *"
action = "deny"

[ui]
markdown = true
syntax_highlight = true
theme = "dark"

# MCP servers (see MCP Servers page)
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path"]

# Lifecycle hooks (see Hooks page)
[[hooks]]
event = "post_tool_use"
tool_name = "FileWrite"
[hooks.action]
type = "shell"
command = "cargo fmt"
```

## Project config

Create `.agent/settings.toml` in your repo root for project-specific settings. These override user config but are overridden by CLI flags.

Initialize with:

```bash
agent
> /init
Created .agent/settings.toml
```

## Environment variables

| Variable | Purpose |
|----------|---------|
| `AGENT_CODE_API_KEY` | API key (highest priority) |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `AGENT_CODE_API_BASE_URL` | API endpoint override |
| `AGENT_CODE_MODEL` | Model override |
| `AGENT_CODE_AUTH_MODE` | `api_key` or `codex_chatgpt` |
| `AGENT_CODE_CODEX_HOME` | Codex home for `codex_chatgpt` auth |
| `CODEX_HOME` | Fallback Codex home for `codex_chatgpt` auth |
| `EDITOR` | Determines vi/emacs REPL mode |

## Codex ChatGPT auth

If you are already signed in with OpenAI Codex, agent-code can reuse that
ChatGPT session without writing an API key to agent-code config:

```bash
codex login
agent --auth-mode codex_chatgpt --model gpt-5.4
```

Or configure it:

```toml
[api]
auth_mode = "codex_chatgpt"
model = "gpt-5.4"
```

This reads `$CODEX_HOME/auth.json` (or `~/.codex/auth.json`) and uses the
Codex ChatGPT backend. Set `codex_home = "/path/to/.codex"` under `[api]`
or `AGENT_CODE_CODEX_HOME` when the Codex home is not the default.
