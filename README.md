<p align="center">
  <img src="https://siuhyr0peaacfwst.public.blob.vercel-storage.com/avala-marketing-site/news/avala-bot-no-bg.png" alt="Agent Code" width="120">
</p>

<h1 align="center">Agent Code</h1>

<p align="center">
  <strong>Open-source AI coding agent for the terminal.</strong><br>
  Fullscreen TUI, multi-provider, headless &amp; ACP — written in Rust.<br>
  <a href="https://github.com/avala-ai">Avala AI</a>
</p>

<p align="center">
  <a href="https://crates.io/crates/agent-code"><img src="https://img.shields.io/crates/v/agent-code.svg" alt="crates.io"></a>
  <a href="https://github.com/avala-ai/agent-code/actions"><img src="https://img.shields.io/github/actions/workflow/status/avala-ai/agent-code/ci.yml?branch=main&label=CI" alt="CI"></a>
  <a href="https://github.com/avala-ai/agent-code/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
  <a href="https://codecov.io/gh/avala-ai/agent-code"><img src="https://codecov.io/gh/avala-ai/agent-code/branch/main/graph/badge.svg" alt="Coverage"></a>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quickstart">Quickstart</a> ·
  <a href="#ways-to-run">Ways to run</a> ·
  <a href="#documentation">Documentation</a> ·
  <a href="#configuration">Configuration</a> ·
  <a href="#repository-layout">Layout</a> ·
  <a href="#development">Development</a> ·
  <a href="#license">License</a>
</p>

---

**Agent Code** understands your repo, edits files, runs shell commands, searches the web, and drives multi-step engineering work — interactively in a fullscreen TUI, headlessly in scripts and CI, or embedded in editors via the [Agent Client Protocol (ACP)](https://agentclientprotocol.com).

It is **MIT licensed**, multi-provider by default, and does **not** send telemetry home unless you opt in.

## Install

Prebuilt binaries for macOS, Linux, and Windows:

```bash
curl -fsSL https://raw.githubusercontent.com/avala-ai/agent-code/main/install.sh | bash
agent --version
```

Other options:

| Method | Command |
|--------|---------|
| Cargo | `cargo install agent-code` |
| Homebrew | `brew install avala-ai/tap/agent-code` |
| Docker | `docker run --rm -it ghcr.io/avala-ai/agent-code` |
| From source | `cargo build --release -p agent-code` → `target/release/agent` |

See [Installation](docs/installation.mdx) for platform notes and data directories.

## Quickstart

```bash
# Interactive TUI (default) — setup wizard on first launch
agent

# One-shot (headless)
agent -p "fix the failing tests and summarize what changed"

# Pick a model / provider
agent --model claude-sonnet-5
agent --model gpt-5.5 --provider openai
```

**Authentication** — set a provider API key, or sign in with a subscription:

```bash
export ANTHROPIC_API_KEY=…   # or OPENAI_API_KEY, XAI_API_KEY, …
# or
agent login codex            # ChatGPT / Codex subscription
agent login xai              # SuperGrok / X Premium (device / OAuth)
```

In the TUI: type a task and press **Enter**. **Shift+Tab** cycles permission modes. **Ctrl+C** cancels a running turn; **Esc** never cancels (clears draft / dismisses modals). Full bindings: [Keyboard shortcuts](docs/tui/KEYBINDINGS.md).

## Ways to run

| Mode | How | Use when |
|------|-----|----------|
| **Interactive TUI** | `agent` | Daily coding — transcript, modals, queue, tasks pane |
| **Headless** | `agent -p "…"` | Scripts, CI, piping |
| **HTTP API** | `agent --serve` | Local clients / Flutter GUI |
| **ACP** | `agent acp` | IDE integrations (stdio JSON-RPC) |
| **Security scan** | `agent security-scan` | Whole-repo vulnerability MapReduce |

## LLM providers

Works with any major API — set one env var and go:

| Provider | Env | Notes |
|----------|-----|--------|
| Anthropic | `ANTHROPIC_API_KEY` | Claude models |
| OpenAI | `OPENAI_API_KEY` | GPT / o-series |
| Azure OpenAI | Azure endpoint + key | |
| xAI | `XAI_API_KEY` | Grok models |
| Google | `GOOGLE_API_KEY` | Gemini |
| DeepSeek, Groq, Mistral, Together, Zhipu, Cohere, Perplexity | respective `*_API_KEY` | |
| OpenRouter | `OPENROUTER_API_KEY` | Multi-model router |
| Bedrock / Vertex | `AGENT_CODE_USE_BEDROCK` / `AGENT_CODE_USE_VERTEX` | Cloud Claude |
| Ollama / local | `--api-base-url http://localhost:11434/v1` | OpenAI-compatible |

Subscription logins reuse existing sessions when present (`~/.codex/auth.json`, `~/.grok/auth.json`). Details: [Authentication](docs/authentication.mdx).

## What you get

- **Fullscreen modern TUI** — streaming transcript, permission / plan / question modals, prompt queue, mode badge, tasks pane ([TUI docs](docs/tui/README.md))
- **30+ tools** — files, search, shell, apply_patch, worktrees, web, LSP, MCP, subagents, monitors, cron, notebooks, …
- **Skills & plugins** — reusable workflows (`/commit`, `/review`, `/plan`, …) plus project skills under `.agent/skills/`
- **Permissions & sandbox** — ask / allow / plan / accept_edits, protected dirs, destructive-command guards, optional OS sandbox
- **Sessions** — persist, resume, fork, rewind, compact
- **Security-scan** — Agentic MapReduce over whole repos ([guide](docs/guides/security-scan.mdx))
- **No default telemetry** — outbound traffic is the LLM you configure (and MCP servers you add)

## Documentation

| Start here | |
|------------|---|
| [User guide index](docs/user-guide/README.md) | Tiered guide: essentials → features → advanced |
| [Quickstart](docs/quickstart.mdx) | First hour walkthrough |
| [Keyboard shortcuts](docs/tui/KEYBINDINGS.md) | Modern TUI bindings |
| [Slash commands](docs/reference/commands.mdx) | Full `/command` list |
| [Configuration](docs/configuration/settings.mdx) | `config.toml`, env, flags |
| [Security](SECURITY.md) | Permissions model & invariants |

Online / Mintlify site config: [`docs/docs.json`](docs/docs.json). Architecture notes: [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Configuration

```toml
# ~/.config/agent-code/config.toml

[api]
model = "claude-sonnet-5"

[permissions]
default_mode = "ask"   # ask | allow | deny | accept_edits | plan

[ui]
theme = "midnight"

[security]
disable_bypass_permissions = true   # enterprise: lock YOLO flags
```

Precedence: **CLI flags → environment → project config → user config → defaults**.

## Repository layout

```
crates/
  lib/     agent-code-lib   Engine: providers, tools, query loop, memory, permissions
  cli/     agent            Binary: TUI, slash commands, ACP, --serve
  eval/    agent-code-eval  Behavioral evaluation harness

client/                    Flutter desktop/web GUI (talks to --serve)
packages/                  TypeScript / Dart client packages
docs/                      User guides (Mintlify + mdBook sources)
evals/                     Eval fixtures
```

The engine is an embeddable library; the CLI is a thin product surface on top.

## Development

Requirements: Rust (see `rust-toolchain.toml` if present; otherwise recent stable), and for full tool use: `git`, `rg`.

```bash
git clone https://github.com/avala-ai/agent-code.git
cd agent-code

cargo check --all-targets
cargo test  --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check

cargo run -p agent-code --                 # launch TUI
cargo run -p agent-code -- -p "say hi"     # headless smoke
```

See [CONTRIBUTING.md](CONTRIBUTING.md), [AGENTS.md](AGENTS.md) (agent instructions for this repo), [RELEASING.md](RELEASING.md), and [ROADMAP.md](ROADMAP.md).

## Security

- Writes to `.git/`, `.husky/`, and `node_modules/` are **unconditionally blocked**
- Destructive shell patterns warn / block; system paths are protected
- Plan mode is read-only (except the plan surface)
- `--dangerously-skip-permissions` can be globally disabled via config

Details: [SECURITY.md](SECURITY.md) and [Permissions](docs/concepts/permissions.mdx).

## Platforms

| Platform | Arch | Install |
|----------|------|---------|
| Linux | x86_64, aarch64 | curl, cargo, brew, binary, Docker |
| macOS | x86_64, Apple Silicon | curl, cargo, brew, binary |
| Windows | x86_64 | cargo, binary (.zip) |

## License

[MIT](LICENSE) — © Avala AI.
