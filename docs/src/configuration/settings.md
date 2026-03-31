
Configuration loads from three layers (highest priority first):

1. **CLI flags and environment variables**
2. **Project config** — `.rc/settings.toml` in your repo
3. **User config** — `~/.config/agent-code/config.toml`

## Full config reference

```toml
# ~/.config/agent-code/config.toml

[api]
base_url = "https://api.anthropic.com/v1"
model = "claude-sonnet-4-20250514"
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

Create `.rc/settings.toml` in your repo root for project-specific settings. These override user config but are overridden by CLI flags.

Initialize with:

```bash
rc
> /init
Created .rc/settings.toml
```

## Environment variables

| Variable | Purpose |
|----------|---------|
| `AGENT_CODE_API_KEY` | API key (highest priority) |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `AGENT_CODE_API_BASE_URL` | API endpoint override |
| `AGENT_CODE_MODEL` | Model override |
| `EDITOR` | Determines vi/emacs REPL mode |
