<p align="center">
  <img src="https://siuhyr0peaacfwst.public.blob.vercel-storage.com/avala-marketing-site/news/avala-bot-no-bg.png" alt="Agent Code" width="200">
</p>

<h1 align="center">Agent Code</h1>

<p align="center">
  AI coding agent for the terminal. Built in Rust.<br>
  <a href="https://github.com/avala-ai">Avala AI</a>
</p>

<p align="center">
  <a href="https://crates.io/crates/agent-code"><img src="https://img.shields.io/crates/v/agent-code.svg" alt="crates.io"></a>
  <a href="https://github.com/avala-ai/agent-code/actions"><img src="https://github.com/avala-ai/agent-code/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://codecov.io/gh/avala-ai/agent-code"><img src="https://codecov.io/gh/avala-ai/agent-code/branch/main/graph/badge.svg" alt="Coverage"></a>
  <a href="https://github.com/avala-ai/agent-code/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
</p>

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/avala-ai/agent-code/main/install.sh | bash
```

Or: `cargo install agent-code` / `brew install avala-ai/tap/agent-code`

## Quickstart

```bash
agent                          # interactive mode (runs setup wizard on first launch)
agent --prompt "fix the tests" # one-shot mode
agent --model gpt-4.1-mini     # use a specific model
```

The agent reads your codebase, runs commands, edits files, and handles multi-step tasks. Type `?` for keyboard shortcuts.

## 12 Providers

Works with any LLM. Set one env var and go:

| Provider | Env Variable | Default Model |
|----------|-------------|---------------|
| OpenAI | `OPENAI_API_KEY` | gpt-5.4 |
| Anthropic | `ANTHROPIC_API_KEY` | claude-sonnet-4 |
| xAI | `XAI_API_KEY` | grok-3 |
| Google | `GOOGLE_API_KEY` | gemini-2.5-flash |
| DeepSeek | `DEEPSEEK_API_KEY` | deepseek-chat |
| Groq | `GROQ_API_KEY` | llama-3.3-70b |
| Mistral | `MISTRAL_API_KEY` | mistral-large |
| Together | `TOGETHER_API_KEY` | meta-llama-3.1-70b |
| Zhipu (z.ai) | `ZHIPU_API_KEY` | glm-4.7 |
| Ollama | (none) | qwen3:latest |
| AWS Bedrock | `AGENT_CODE_USE_BEDROCK` | claude-sonnet-4 |
| Google Vertex | `AGENT_CODE_USE_VERTEX` | claude-sonnet-4 |

Plus any OpenAI-compatible endpoint: `agent --api-base-url http://localhost:8080/v1`

## Input Modes

| Prefix | Action |
|--------|--------|
| (none) | Chat with the agent |
| `!` | Run shell command directly |
| `/` | Slash commands (tab-complete) |
| `@` | Attach file to prompt |
| `&` | Run prompt in background |
| `?` | Toggle shortcuts panel |
| `\` + Enter | Multi-line input |

## 32 Built-in Tools

File ops, search, shell, git, web, LSP, MCP, notebooks, tasks, and more. Tools execute during LLM streaming for faster turns. [Full list →](docs/reference/tools.mdx)

## 12 Bundled Skills

| Skill | Purpose |
|-------|---------|
| `/commit` | Create well-crafted git commits |
| `/review` | Review diff for bugs and security issues |
| `/test` | Run tests and fix failures |
| `/explain` | Explain how code works |
| `/debug` | Debug errors with root cause analysis |
| `/pr` | Create pull requests |
| `/refactor` | Refactor code for quality |
| `/init` | Initialize project configuration |
| `/security-review` | OWASP-oriented vulnerability scan |
| `/advisor` | Architecture and dependency health analysis |
| `/bughunter` | Systematic bug search |
| `/plan` | Structured implementation planning |

Add custom skills as markdown files in `.agent/skills/` or `~/.config/agent-code/skills/`.

## Configuration

```toml
# ~/.config/agent-code/config.toml

[api]
model = "gpt-4.1-mini"

[permissions]
default_mode = "ask"   # ask | allow | deny | accept_edits | plan

[features]
token_budget = true
extract_memories = true
auto_theme = true

[security]
mcp_server_allowlist = ["github", "filesystem"]
disable_bypass_permissions = true
```

## Architecture

```
crates/
  lib/   agent-code-lib    Engine: providers, tools, query loop, memory
  cli/   agent-code        Binary: REPL, TUI, commands, setup wizard
```

The engine is a reusable library. The binary is a thin wrapper.

## 42 Slash Commands

Session management, context control, git operations, agent coordination, configuration, diagnostics, and more. [Full list →](docs/reference/commands.mdx)

Highlights: `/release-notes`, `/summary`, `/feedback`, `/share`, `/doctor`, `/plan`, `/model`, `/cost`, `/scroll`, `/rewind`, `/fork`

## Security

Protected directories (`.git/`, `.husky/`, `node_modules/`) are blocked from writes regardless of permission settings. Destructive shell commands trigger warnings. [Learn more →](SECURITY.md)

## Platforms

| Platform | Architecture | Install |
|----------|-------------|---------|
| Linux | x86_64, aarch64 | curl, cargo, homebrew, prebuilt binary |
| macOS | x86_64, Apple Silicon | curl, cargo, homebrew, prebuilt binary |
| Windows | x86_64 | cargo, prebuilt binary (.zip) |
| Docker | any | `docker run ghcr.io/avala-ai/agent-code` |

## Contributing

```bash
git clone https://github.com/avala-ai/agent-code.git
cd agent-code
cargo build
cargo test    # 220+ tests
cargo clippy  # zero warnings
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines and [ROADMAP.md](ROADMAP.md) for planned improvements.

## License

MIT
