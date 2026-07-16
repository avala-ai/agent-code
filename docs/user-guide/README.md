# Agent Code user guide

Learn how to install, configure, and extend Agent Code — the open-source terminal coding agent from Avala AI.

Primary formats:

- **This index** — GitHub-friendly overview and links
- **Mintlify pages** under `docs/*.mdx` — site navigation via [`docs.json`](../docs.json)
- **mdBook** under `docs/src/` — offline book build

---

## Tier 1 — Essential (first day)

| Guide | Description |
|-------|-------------|
| [Overview](../overview.mdx) | What Agent Code is and how the agent loop works |
| [Installation](../installation.mdx) | curl, cargo, brew, Docker, data directories |
| [Quickstart](../quickstart.mdx) | First session, first task, first config |
| [Authentication](../authentication.mdx) | API keys, Codex/xAI subscription login, reuse of existing sessions |
| [Keyboard shortcuts](../tui/KEYBINDINGS.md) | Modern TUI bindings (Esc vs Ctrl+C, modes, modals, queue) |
| [Slash commands](../reference/commands.mdx) | Full `/command` reference |
| [Configuration](../configuration/settings.mdx) | `config.toml`, env vars, CLI flags, precedence |

---

## Tier 2 — Core features

| Guide | Description |
|-------|-------------|
| [Tools](../concepts/tools.mdx) | Built-in tool catalog and execution model |
| [Permissions](../concepts/permissions.mdx) | Modes, rules, protected paths |
| [Plan mode](../plan-mode.mdx) | Read-only planning and approval |
| [Sessions](../concepts/sessions.mdx) | Persist, resume, fork, rewind, compact |
| [Memory](../concepts/memory.mdx) | Cross-session notes and team memory |
| [Skills](../extending/skills.mdx) | SKILL.md workflows and discovery paths |
| [Plugins](../extending/plugins.mdx) | Bundle skills, hooks, MCP |
| [Hooks](../configuration/hooks.mdx) | Lifecycle scripts around tools and sessions |
| [MCP servers](../configuration/mcp-servers.mdx) | External tool servers |
| [Providers](../configuration/providers.mdx) | Multi-provider setup and fallbacks |
| [Themes](../configuration/themes.mdx) | Classic / modern appearance |

---

## Tier 3 — Advanced

| Guide | Description |
|-------|-------------|
| [Headless & scripting](../headless.mdx) | `-p`, output formats, CI, `--serve` |
| [ACP / IDE](../extending/ide-bridge.mdx) | Agent Client Protocol stdio |
| [Sandbox](../sandbox.mdx) | OS-level isolation (bwrap / Seatbelt) |
| [Terminal support](../terminal-support.mdx) | tmux, truecolor, clipboard, troubleshooting |
| [TUI product bar](../tui/README.md) | Modern TUI docs, acceptance, support matrix |
| [Security scan](../guides/security-scan.mdx) | Whole-repo AMR vulnerability scanning |
| [Performance](../guides/performance.mdx) | Cost, context, model choice |
| [Architecture](../architecture/compaction.mdx) | Compaction, tool execution, providers, MCP |
| [Troubleshooting](../troubleshooting.mdx) | Common failures and fixes |
| [FAQ](../faq.mdx) | Short answers |

---

## For contributors

| Doc | Description |
|-----|-------------|
| [CONTRIBUTING.md](../../CONTRIBUTING.md) | PR and CI expectations |
| [AGENTS.md](../../AGENTS.md) | Non-obvious repo rules for coding agents |
| [ARCHITECTURE.md](../../ARCHITECTURE.md) | Engine structure |
| [SECURITY.md](../../SECURITY.md) | Security invariants |
| [World-class TUI plan](../design/tui-world-class-parity.md) | Parity bar and execution waves |
