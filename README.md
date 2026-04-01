<p align="center">
  <h1 align="center">agent-code</h1>
  <p align="center">
    A fast, open-source AI coding agent for the terminal.<br>
    Built in pure Rust by <a href="https://github.com/avala-ai">Avala AI</a>.
  </p>
</p>

<p align="center">
  <a href="https://crates.io/crates/agent-code"><img src="https://img.shields.io/crates/v/agent-code.svg" alt="crates.io"></a>
  <a href="https://github.com/avala-ai/agent-code/actions"><img src="https://github.com/avala-ai/agent-code/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/avala-ai/agent-code/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
</p>

---

`agent-code` is an AI-powered coding agent that lives in your terminal. It reads your codebase, executes commands, edits files, and handles multi-step engineering tasks autonomously. Think of it as a senior engineer that works alongside you in the shell.

```
$ cargo install agent-code
$ export AGENT_CODE_API_KEY="your-api-key"
$ agent
```

## Why agent-code

- **Fast.** Single static binary, ~8MB. Starts instantly. Streams responses as they arrive.
- **Private.** Runs locally. No telemetry. Your code never leaves your machine except for the LLM API call.
- **Extensible.** Connect external tools via MCP servers, write custom skills as markdown files, build plugins as TOML packages.
- **Safe.** Every tool call goes through a permission system. Plan mode locks the agent to read-only. Configurable rules per tool and pattern.
- **Provider-agnostic.** Works with 9 providers out of the box. Swap models with `--model` or `/model`.

## Supported Models

agent-code works with any LLM provider. Set one environment variable and go:

| Provider | Env Variable | Models |
|----------|-------------|--------|
| **Anthropic** | `ANTHROPIC_API_KEY` | Claude Opus, Sonnet, Haiku |
| **OpenAI** | `OPENAI_API_KEY` | GPT-5.4, GPT-5.4-mini, GPT-5.4-nano, GPT-4.1, GPT-4o, o1, o3 |
| **xAI** | `XAI_API_KEY` | Grok-3, Grok-3-mini, Grok-2 |
| **Google** | `GOOGLE_API_KEY` | Gemini 2.5 Flash, Gemini 2.5 Pro |
| **DeepSeek** | `DEEPSEEK_API_KEY` | DeepSeek-V3, DeepSeek-Chat |
| **Groq** | `GROQ_API_KEY` | Llama 3.3 70B, Mixtral |
| **Mistral** | `MISTRAL_API_KEY` | Mistral Large, Codestral |
| **Together** | `TOGETHER_API_KEY` | Llama 3, Qwen, and 100+ open models |
| **Ollama** | (local) | Any model via `--api-base-url http://localhost:11434/v1` |

Plus any OpenAI-compatible endpoint via `--api-base-url`.

## Install

**One-line install** (Linux/macOS):
```bash
curl -fsSL https://raw.githubusercontent.com/avala-ai/agent-code/main/install.sh | bash
```

**From crates.io:**
```bash
cargo install agent-code
```

**Homebrew:**
```bash
brew install avala-ai/tap/agent-code
```

**From source:**
```bash
git clone https://github.com/avala-ai/agent-code.git
cd agent-code
cargo build --release
# Binary is at target/release/agent
```

**From GitHub Releases:**

Download prebuilt binaries for Linux (x86_64, aarch64) and macOS (x86_64, aarch64) from the [Releases page](https://github.com/avala-ai/agent-code/releases).

## Quick Start

```bash
# Set your API key
export AGENT_CODE_API_KEY="your-api-key"

# Interactive mode
agent

# One-shot: ask a question and exit
agent --prompt "find all TODO comments in this repo"

# Use a specific model
agent --model claude-opus-4-20250514

# Print the system prompt (useful for debugging)
agent --dump-system-prompt
```

Once inside the REPL, type naturally. The agent reads files, runs commands, and edits code to accomplish what you ask. Type `/help` to see all available commands.

## What It Can Do

The agent has 23 built-in tools and 26 slash commands. Here are the highlights:

**Read and understand code:**
```
> explain how the authentication middleware works
> find where the database connection pool is configured
> what changed in the last 3 commits?
```

**Write and edit code:**
```
> add input validation to the signup endpoint
> refactor this function to use async/await
> fix the failing test in tests/auth_test.rs
```

**Run commands and manage git:**
```
> run the test suite and fix any failures
> /commit  (reviews diff and creates a commit)
> /review  (analyzes diff for bugs and issues)
```

**Multi-step tasks:**
```
> add a new API endpoint for user preferences with tests, migration, and docs
```

The agent handles this by reading existing patterns, creating the migration, writing the endpoint, adding tests, and updating documentation, all in one turn.

## Architecture

```
                              ┌─────────────┐
                              │   CLI / REPL │
                              └──────┬───────┘
                                     │
                    ┌────────────────▼────────────────┐
                    │          Query Engine            │
                    │  stream → tools → loop → compact │
                    └──┬──────────┬──────────┬────────┘
                       │          │          │
              ┌────────▼──┐ ┌────▼─────┐ ┌──▼───────┐
              │   Tools   │ │ Perms    │ │  Hooks   │
              │  23 built │ │ allow    │ │ pre/post │
              │  + MCP    │ │ deny/ask │ │ shell    │
              └────��──────┘ └──────────┘ └──────────┘
                       │          │          │
   ┌───────────────────▼──────────▼──────────▼──────┐
   │                    Services                     │
   │  LLM  Compact  MCP  LSP  Git  Session  Budget  │
   │  Cache  Bridge  Coordinator  Telemetry  Plugins │
   └─────────────────────────────────────────────────┘
```

The core loop: receive user input, call the LLM with conversation history and tool definitions, stream the response, execute any tool calls the model makes, feed results back, repeat until done. Compaction keeps the context window in check during long sessions.

## Configuration

Configuration loads from three layers (highest priority first):

1. **CLI flags and environment variables** (`AGENT_CODE_API_KEY`, `--model`, etc.)
2. **Project config** (`.rc/settings.toml` in your repo)
3. **User config** (`~/.config/agent-code/config.toml`)

```toml
# ~/.config/agent-code/config.toml

[api]
base_url = "https://api.anthropic.com/v1"
model = "claude-sonnet-4-20250514"

[permissions]
default_mode = "ask"   # "ask", "allow", or "deny"

[[permissions.rules]]
tool = "Bash"
pattern = "git *"
action = "allow"       # auto-approve git commands

[[permissions.rules]]
tool = "FileWrite"
pattern = "/tmp/*"
action = "allow"

# Connect an MCP server for additional tools
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/docs"]
```

## Tools

| Tool | What it does |
|------|-------------|
| **Agent** | Spawn subagents for parallel work (with optional git worktree isolation) |
| **Bash** | Run shell commands with timeout and cancellation |
| **FileRead** | Read files with line numbers (handles binary, PDF, images gracefully) |
| **FileWrite** | Create or overwrite files, auto-creates parent directories |
| **FileEdit** | Targeted search-and-replace with uniqueness validation |
| **Grep** | Regex search powered by ripgrep, with context lines and filtering |
| **Glob** | Find files by pattern, sorted by modification time |
| **WebFetch** | Fetch URLs with HTML-to-text conversion |
| **WebSearch** | Web search with result extraction |
| **LSP** | Query language servers for diagnostics (falls back to linters) |
| **NotebookEdit** | Edit Jupyter notebook cells (replace, insert, delete) |
| **AskUserQuestion** | Prompt the user with structured choices during execution |
| **ToolSearch** | Discover tools by keyword or direct selection |
| **SendMessage** | Inter-agent communication for multi-agent workflows |
| **EnterPlanMode / ExitPlanMode** | Toggle read-only mode for safe exploration |
| **EnterWorktree / ExitWorktree** | Manage isolated git worktrees |
| **TaskCreate / TaskUpdate** | Track progress on multi-step work |
| **TodoWrite** | Structured todo list management |
| **Sleep** | Async pause with cancellation support |
| **McpProxy** | Bridge to any MCP server tool |

## Commands

| Command | What it does |
|---------|-------------|
| `/help` | Show all commands and loaded skills |
| `/clear` | Reset conversation history |
| `/compact` | Free context by clearing stale tool results |
| `/cost` | Token usage and estimated cost for the session |
| `/context` | Context window usage and auto-compact threshold |
| `/model [name]` | Show or switch the active model |
| `/diff` | Show current git changes |
| `/status` | Show git status |
| `/commit [msg]` | Review diff and create a commit |
| `/review` | Analyze diff for bugs, security issues, code quality |
| `/branch [name]` | Show or switch git branch |
| `/plan` | Toggle plan mode (read-only tools only) |
| `/resume <id>` | Restore a previous session |
| `/sessions` | List saved sessions |
| `/export` | Export conversation to markdown |
| `/init` | Create `.rc/settings.toml` for this project |
| `/doctor` | Check environment (tools, config, git, disk) |
| `/mcp` | List connected MCP servers |
| `/memory` | Show loaded memory context |
| `/skills` | List available custom skills |
| `/agents` | List agent types (general, explore, plan) |
| `/plugins` | List loaded plugins |
| `/hooks` | Show hook configuration |

## Extending agent-code

### Skills

Skills are reusable workflows defined as markdown files with YAML frontmatter. Drop them in `.rc/skills/` or `~/.config/agent-code/skills/`.

```markdown
---
description: Run tests and fix failures
userInvocable: true
---

Run the test suite. If any tests fail, read the failing test and the
source code it tests, then fix the issue. Run the tests again to verify.
```

Invoke with `/skill-name` or let the agent pick it up contextually.

### MCP Servers

Connect any [Model Context Protocol](https://modelcontextprotocol.io/) server to add tools. Servers communicate over stdio or HTTP.

```toml
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_..." }
```

### Plugins

Plugins bundle skills, hooks, and configuration into installable packages. A plugin is a directory with a `plugin.toml` manifest.

```
my-plugin/
  plugin.toml
  skills/
    deploy.md
    rollback.md
```

### Hooks

Run shell commands or HTTP requests at lifecycle points:

```toml
# .rc/settings.toml
[[hooks]]
event = "post_tool_use"
tool_name = "FileWrite"
action = { type = "shell", command = "cargo fmt" }
```

### Memory

Persistent context that carries across sessions:

- **Project memory:** `.rc/CONTEXT.md` in your repo root. Loaded automatically.
- **User memory:** `~/.config/agent-code/memory/MEMORY.md`. Personal preferences and patterns.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code style, and PR process.

```bash
git clone https://github.com/avala-ai/agent-code.git
cd agent-code
cargo build
cargo test
cargo clippy
```

## License

MIT. See [LICENSE](LICENSE).

---

Built by [Avala AI](https://github.com/avala-ai).
