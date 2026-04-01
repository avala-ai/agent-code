# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Reporting a Vulnerability

If you discover a security vulnerability in agent-code, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

Instead, email **security@avala.ai** with:

1. Description of the vulnerability
2. Steps to reproduce
3. Potential impact
4. Suggested fix (if any)

We will acknowledge your report within 48 hours and provide a timeline for a fix within 5 business days.

## Security Model

agent-code executes shell commands and modifies files on behalf of the user. The security model is designed to prevent the AI agent from taking actions the user hasn't approved.

### Permission System

Every tool call passes through a permission check before execution:

- **Ask mode** (default): prompts the user before mutations
- **Allow mode**: auto-approves all operations
- **Deny mode**: blocks all mutations
- **Plan mode**: restricts to read-only tools

Configure per-tool rules in `.agent/settings.toml`:

```toml
[permissions]
default_mode = "ask"

[[permissions.rules]]
tool = "Bash"
pattern = "git *"
action = "allow"

[[permissions.rules]]
tool = "Bash"
pattern = "rm *"
action = "deny"
```

### Bash Sandbox

The Bash tool includes built-in safety checks:

- **Destructive command detection**: warns before `rm -rf`, `git reset --hard`, `DROP TABLE`, and similar commands
- **System path blocking**: prevents writes to `/etc`, `/usr`, `/bin`, `/sbin`, `/boot`, `/sys`, `/proc`
- **Output truncation**: large outputs are persisted to disk instead of flooding the context

### API Key Handling

- API keys are never written to config files (use environment variables)
- Keys are never logged or included in error messages
- Keys are passed to subagent processes via environment only

### MCP Server Security

- MCP servers run as subprocesses with the user's permissions
- Server connections are local only (stdio or localhost HTTP)
- Each server's tools are namespaced to prevent collisions

### Data Handling

- No telemetry is collected or transmitted
- Session data is stored locally in `~/.config/agent-code/`
- Conversation history never leaves the machine except for LLM API calls
- Tool result persistence is local only (`~/.cache/agent-code/`)

## Threat Model

### In scope

- Agent executing unintended destructive commands
- Prompt injection via tool results or file contents
- API key leakage through logs or error messages
- MCP server executing malicious tools

### Out of scope

- Security of the LLM API endpoint itself
- Security of the user's local machine beyond what agent-code touches
- Attacks requiring physical access to the machine
- Social engineering of the user
