
# What is agent-code?

agent-code is an open-source, AI-powered coding agent for the terminal. You describe what you want in natural language, and the agent reads your codebase, runs commands, edits files, and iterates until the task is done.

It's built in pure Rust for speed, safety, and a single static binary with zero runtime dependencies.

## What can it do?

<CardGroup cols={2}>
  <Card title="Read and understand code" icon="magnifying-glass">
    Searches your codebase, reads files, traces dependencies, and explains how things work.
  </Card>
  <Card title="Write and edit code" icon="pen">
    Creates files, makes targeted edits, refactors code, adds features, and fixes bugs.
  </Card>
  <Card title="Run commands" icon="terminal">
    Executes shell commands, runs tests, manages git operations, and handles build tools.
  </Card>
  <Card title="Multi-step tasks" icon="diagram-project">
    Chains together reading, writing, and running to complete complex engineering tasks autonomously.
  </Card>
</CardGroup>

## How it works

The core loop is simple:

1. You type a request
2. The agent sends your request + conversation history to an LLM
3. The LLM responds with text and tool calls
4. The agent executes the tools (file reads, edits, shell commands, etc.)
5. Tool results are fed back to the LLM
6. Repeat until the task is done

Every tool call passes through a permission system before executing. You stay in control.

```
You: "add input validation to the signup endpoint"

Agent:
  → FileRead src/routes/signup.rs
  → Grep "validate" src/
  → FileEdit src/routes/signup.rs (add validation logic)
  → Bash "cargo test"
  → FileEdit src/routes/signup.rs (fix test failure)
  → Bash "cargo test"
  ✓ All tests pass. Validation added.
```

## Key features

- **32 built-in tools** for file operations, shell commands, code search, web access, language servers, and more
- **42 slash commands** for git, session management, diagnostics, sharing, and agent control
- **12 bundled skills** including security review, architecture advisor, bug hunter, and implementation planner
- **12 LLM providers** — Anthropic, OpenAI, xAI, Google, DeepSeek, Groq, Mistral, Together, Zhipu, Ollama, AWS Bedrock, Google Vertex
- **Permission system** with configurable rules per tool, pattern matching, and protected directories
- **MCP support** for connecting external tool servers
- **Memory system** for persistent context across sessions
- **Plugin system** for bundling skills, hooks, and configuration
- **Session persistence** with save, resume, share, fork, and rewind
- **Plan mode** for safe read-only exploration
- **Extended thinking** for complex reasoning tasks
- **Cross-platform** — Linux, macOS, and Windows

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
        │  31 built │ │ allow    │ │ pre/post │
        │  + MCP    │ │ deny/ask │ │ shell    │
        └───────────┘ └──────────┘ └──────────┘
```

## Next steps

<CardGroup cols={2}>
  <Card title="Quickstart" icon="rocket" href="/quickstart">
    Get up and running in 2 minutes.
  </Card>
  <Card title="Installation" icon="download" href="/installation">
    All the ways to install agent-code.
  </Card>
</CardGroup>
