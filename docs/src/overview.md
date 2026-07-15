
# What is Agent Code?

**Agent Code** is an open-source, AI-powered coding agent for the terminal. You describe what you want; it reads your codebase, runs commands, edits files, and iterates until the task is done.

It is written in **Rust** (single static binary), **MIT licensed**, **multi-provider**, and **does not phone home** unless you opt into telemetry.

## What can it do?

<CardGroup cols={2}>
  <Card title="Read and understand code" icon="magnifying-glass">
    Search, read, trace dependencies, and explain how systems work.
  </Card>
  <Card title="Write and edit code" icon="pen">
    Create files, targeted edits, apply_patch, refactors, and multi-file changes.
  </Card>
  <Card title="Run commands" icon="terminal">
    Shell, tests, git, builds — with permissions and optional OS sandbox.
  </Card>
  <Card title="Multi-step agent work" icon="diagram-project">
    Tool loops, subagents, plan mode, skills, MCP, and session resume.
  </Card>
</CardGroup>

## How it works

1. You type a request (TUI, `-p`, ACP, or HTTP)
2. The query engine sends context + history to your configured LLM
3. The model streams text and tool calls
4. Tools run under the permission system (and optional sandbox)
5. Results feed back into the model until the turn finishes

```
You: "add input validation to the signup endpoint"

Agent:
  → FileRead src/routes/signup.rs
  → Grep "validate" src/
  → FileEdit src/routes/signup.rs
  → Bash "cargo test"
  ✓ Tests pass. Validation added.
```

## Surfaces

| Surface | Command | Role |
|---------|---------|------|
| Interactive TUI | `agent` | Fullscreen transcript, modals, queue, tasks |
| Headless | `agent -p "…"` | Scripts and CI |
| HTTP | `agent --serve` | Local API / Flutter client |
| ACP | `agent acp` | IDE stdio bridge |
| Security scan | `agent security-scan` | Whole-repo AMR scan |

## Key features

- **Modern fullscreen TUI** with streaming, HITL modals, mode badge, prompt queue
- **30+ tools** — files, shell, search, worktrees, web, LSP, MCP, subagents, monitors, cron
- **Multi-provider** — Anthropic, OpenAI, Azure, xAI, Google, OpenRouter, local OpenAI-compatible, and more
- **Skills & plugins** — reusable workflows and project conventions (`AGENTS.md`)
- **Permissions** — ask / allow / plan / accept_edits, protected dirs, bypass lock
- **Sessions** — save, resume, fork, rewind, compact
- **Security-scan** — plan → shard → map → reduce vulnerability hunting
- **Privacy default** — no product telemetry unless you opt in

## Where to go next

- [Install](./installation.md) · [Quickstart](./quickstart.md) · [User guide index](./user-guide/README.md)
- [Keyboard shortcuts](./keyboard-shortcuts.md) · [Authentication](./authentication.md)
- [World-class TUI plan](./design/tui-world-class-parity.md) (product bar for the interactive UI)
