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

## 15 Providers

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
| OpenRouter | `OPENROUTER_API_KEY` | anthropic/claude-sonnet-4 |
| Cohere | `COHERE_API_KEY` | command-r-plus |
| Perplexity | `PERPLEXITY_API_KEY` | sonar-pro |

Plus any OpenAI-compatible endpoint: `agent --api-base-url http://localhost:8080/v1`

Already signed in with OpenAI Codex? Run `codex login`, then start agent-code with
`agent --auth-mode codex_chatgpt --model gpt-5.4` to reuse that ChatGPT session.

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

## 24 Bundled Skills

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
| `/pentest` | White-box penetration test with proof-of-concept gating |
| `/advisor` | Architecture and dependency health analysis |
| `/bughunter` | Systematic bug search |
| `/plan` | Structured implementation planning |
| `/changelog` | Generate changelog entries from commit history |
| `/release` | Cut a versioned release |
| `/benchmark` | Run and compare benchmarks |
| `/coverage` | Measure and report test coverage |
| `/migrate` | Apply codebase-wide migrations |
| `/docs` | Generate or update documentation |
| `/remember` | Save a specific insight to user memory (two-step write discipline) |
| `/stuck` | Step back and try a different angle when the agent is looping |
| `/simplify` | Review-then-simplify pass: flag dead weight in the current diff |
| `/batch` | Apply the same change across multiple git worktrees |
| `/skillify` | Extract the successful workflow from this session into a reusable skill |

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
  eval/  agent-code-eval   Evaluation harness for benchmarking the engine

packages/
  agent_code_client        Dart client library for talking to the engine

client/                    Cross-platform Flutter desktop/web GUI (see client/README.md)
```

The engine is a reusable library. The CLI binary is a thin wrapper. The Flutter client in `client/` is a separate front-end that talks to the engine via the `agent_code_client` package.

## 63 Slash Commands

Session management, context control, git operations, agent coordination, configuration, diagnostics, and more. [Full list →](docs/reference/commands.mdx)

Highlights: `/release-notes`, `/summary`, `/feedback`, `/share`, `/update`, `/uninstall`, `/doctor`, `/plan`, `/model`, `/cost`, `/usage`, `/scroll`, `/rewind`, `/fork`, `/rename`, `/add-dir`, `/btw`, `/effort`, `/thinkback`, `/break-cache`, `/pr-comments`, `/autofix-pr`, `/perf-issue`

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
cargo test    # 650+ tests
cargo clippy  # zero warnings
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines and [ROADMAP.md](ROADMAP.md) for planned improvements.

## License

MIT
