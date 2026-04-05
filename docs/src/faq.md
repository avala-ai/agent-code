
## General

### What is agent-code?

An open-source AI coding agent for the terminal, built in Rust. You describe tasks in natural language and the agent reads your code, runs commands, edits files, and iterates until the task is done.

### How is it different from ChatGPT or Claude in a browser?

agent-code runs locally in your terminal with direct access to your filesystem, shell, git, and development tools. It can read files, make edits, run tests, and fix errors in a loop — not just generate text.

### Is it free?

agent-code itself is free and open source (MIT license). You pay for the LLM API you choose — Anthropic, OpenAI, or any other provider. Local models via Ollama are completely free.

### Which LLM should I use?

For coding tasks, we recommend Claude Sonnet 4 (Anthropic) or GPT-4.1 (OpenAI) as a good balance of quality and cost. For complex architecture work, use Claude Opus or GPT-5.4. For quick tasks, Haiku or GPT-4.1-mini are fast and cheap.

## Installation

### What are the system requirements?

- Any modern Linux, macOS, or Windows machine
- `git` and `rg` (ripgrep) for full functionality
- An API key from any supported LLM provider (or Ollama for local models)

### Can I use it on Windows?

Yes. Install via `cargo install agent-code` or download the prebuilt binary from [GitHub Releases](https://github.com/avala-ai/agent-code/releases). Windows builds are tested in CI.

### Can I run it in Docker?

Yes. See the [Dockerfile](https://github.com/avala-ai/agent-code/blob/main/Dockerfile) or pull the image:

```bash
docker run -it -e ANTHROPIC_API_KEY="sk-ant-..." ghcr.io/avala-ai/agent-code
```

## Usage

### How do I switch between models mid-session?

Use the `/model` command. It opens an interactive picker based on your configured provider:

```
> /model
```

Or specify directly: `/model gpt-4.1-mini`

### How do I use it with a local model?

```bash
# Start Ollama
ollama serve

# Run agent-code with local model
agent --api-base-url http://localhost:11434/v1 --model llama3 --api-key unused
```

### What does plan mode do?

Plan mode (`/plan`) restricts the agent to read-only operations. It can search, read files, and analyze code, but cannot edit files or run shell commands. Useful for exploring unfamiliar codebases safely.

### How do I give the agent project context?

Create an `AGENTS.md` file in your project root with instructions, conventions, and architecture notes. This is loaded automatically at the start of every session.

### Can it access the internet?

Yes. The `WebFetch` and `WebSearch` tools allow the agent to fetch URLs and search the web. These go through the normal permission system.

## Cost

### How much does it cost per session?

It depends on the model, task complexity, and conversation length. Typical sessions:

| Task | Model | Approximate Cost |
|------|-------|-----------------|
| Quick fix | Sonnet/GPT-4.1 | $0.02 - $0.10 |
| Feature implementation | Sonnet/GPT-4.1 | $0.10 - $0.50 |
| Complex refactor | Opus/GPT-5.4 | $0.50 - $2.00 |

### How do I set a spending limit?

```toml
# ~/.config/agent-code/config.toml
[api]
max_cost_usd = 5.0  # Stop after $5 spent
```

Or check usage anytime with `/cost`.

## Security

### Can the agent delete my files?

The agent asks for permission before destructive operations (default mode). You can also:

- Use plan mode for read-only: `agent --permission-mode plan`
- Block specific commands: add deny rules in config
- Protected directories (`.git/`, `.husky/`, `node_modules/`) are always blocked from writes

### Does it send my code to third parties?

Your code is sent to whichever LLM provider you configure (Anthropic, OpenAI, etc.) as part of the conversation context. It is not sent anywhere else. For maximum privacy, use a local model via Ollama.

### Can I restrict which tools the agent uses?

Yes, via permission rules:

```toml
[permissions]
default_mode = "ask"

[[permissions.rules]]
tool = "Bash"
pattern = "rm *"
action = "deny"
```

## Extensibility

### How do I create a custom skill?

Create a markdown file in `.agent/skills/`:

```markdown
---
description: My custom workflow
userInvocable: true
---

Do the thing step by step...
```

Then invoke it with `/my-skill`. See the [Skills guide](extending/skills) for details.

### Can I connect external tools via MCP?

Yes. Add MCP servers to your config:

```toml
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
```

See the [MCP guide](configuration/mcp-servers) for details.

### Can I use it as a library in my own project?

Yes. The engine is published as `agent-code-lib` on crates.io:

```toml
[dependencies]
agent-code-lib = "0.9"
```

The binary is a thin wrapper around this library.
