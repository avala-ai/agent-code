# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.10.x  | Yes       |
| 0.9.x   | Security fixes only |

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

### Protected Directories

Write tools (`FileWrite`, `FileEdit`, `MultiEdit`, `NotebookEdit`) are blocked from modifying files in these directories regardless of permission configuration:

| Directory | Reason |
|-----------|--------|
| `.git/` | Prevent repository corruption |
| `.husky/` | Prevent git hook tampering |
| `node_modules/` | Prevent dependency modification |

Read access to these directories is unaffected — the agent can read `.git/config` or inspect `node_modules/` contents.

### Bash Sandbox

The Bash tool includes built-in safety checks:

- **Destructive command detection**: warns before `rm -rf`, `git reset --hard`, `DROP TABLE`, and similar commands
- **System path blocking**: prevents writes to `/etc`, `/usr`, `/bin`, `/sbin`, `/boot`, `/sys`, `/proc`
- **Output truncation**: large outputs are persisted to disk instead of flooding the context

### Skill Safety

Skills are user-defined prompt templates that can contain embedded shell code blocks. For environments where skills may come from untrusted sources:

```toml
[security]
disable_skill_shell_execution = true
```

When enabled, fenced shell blocks (` ```sh `, ` ```bash `, ` ```shell `, ` ```zsh `) in skill templates are stripped and replaced with a notice. Non-shell code blocks are preserved.

### API Key Handling

- API keys are never written to config files (use environment variables)
- Keys are never logged or included in error messages
- Keys are passed to subagent processes via environment only

### MCP Server Security

- MCP servers run as subprocesses with the user's permissions
- Server connections are local only (stdio or localhost HTTP)
- Each server's tools are namespaced to prevent collisions
- Allowlist and denylist restrict which servers can connect:

```toml
[security]
mcp_server_allowlist = ["github", "filesystem"]   # Only these can connect
mcp_server_denylist = ["untrusted-server"]         # These are blocked
```

### Data Handling

- No telemetry is collected or transmitted
- Session data is stored locally in `~/.config/agent-code/`
- Conversation history never leaves the machine except for LLM API calls
- Tool result persistence is local only (`~/.cache/agent-code/`)

### The `--dangerously-skip-permissions` Flag

This flag disables all permission checks. It exists for CI/CD pipelines and automated scripting where interactive prompts are not possible.

**Never use this flag in interactive sessions.** It removes all safety guardrails.

To prevent its use entirely (e.g., in enterprise environments):

```toml
[security]
disable_bypass_permissions = true
```

When set, the flag is rejected with an error even if passed on the command line.

## Enterprise Security Configuration

All security settings live under the `[security]` section:

```toml
[security]
# Block --dangerously-skip-permissions flag
disable_bypass_permissions = true

# Strip shell blocks from skill templates
disable_skill_shell_execution = true

# Restrict MCP server connections
mcp_server_allowlist = ["github", "filesystem"]
mcp_server_denylist = []

# Restrict which env vars the agent can read
env_allowlist = ["PATH", "HOME", "SHELL"]

# Allow access to directories outside the project
additional_directories = ["/shared/docs"]
```

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
