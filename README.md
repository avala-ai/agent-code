<p align="center">
  <img src="https://siuhyr0peaacfwst.public.blob.vercel-storage.com/avala-marketing-site/news/avala-bot.webp" alt="Agent Code" width="200">
</p>

<h1 align="center">Agent Code</h1>

<p align="center">
  AI coding agent for the terminal. Built in Rust.<br>
  <a href="https://github.com/avala-ai">Avala AI</a>
</p>

<p align="center">
  <a href="https://crates.io/crates/agent-code"><img src="https://img.shields.io/crates/v/agent-code.svg" alt="crates.io"></a>
  <a href="https://github.com/avala-ai/agent-code/actions"><img src="https://github.com/avala-ai/agent-code/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
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

File ops, search, shell, git, web, LSP, MCP, notebooks, tasks, and more. Tools execute during LLM streaming for faster turns. [Full list](https://github.com/avala-ai/agent-code/wiki/Tools)

## 8 Bundled Skills

`/commit` `/review` `/test` `/explain` `/debug` `/pr` `/refactor` `/init`

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

## Contributing

```bash
git clone https://github.com/avala-ai/agent-code.git
cd agent-code
cargo build
cargo test    # 200 tests
cargo clippy  # zero warnings
```

## License

MIT
