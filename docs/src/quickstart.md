
## Install

<CodeGroup>
```bash cargo (recommended)
cargo install agent-code
```

```bash homebrew
brew install avala-ai/tap/agent-code
```

```bash from source
git clone https://github.com/avala-ai/agent-code.git
cd agent-code
cargo build --release
# Binary: target/release/agent
```
</CodeGroup>

## Set your API key

agent-code works with any LLM provider. Set the key for the one you use:

<CodeGroup>
```bash Anthropic (Claude)
export ANTHROPIC_API_KEY="sk-ant-..."
```

```bash OpenAI (GPT)
export OPENAI_API_KEY="sk-..."
```

```bash Any provider
export AGENT_CODE_API_KEY="your-key"
export AGENT_CODE_API_BASE_URL="https://api.your-provider.com/v1"
```
</CodeGroup>

## Start the agent

```bash
agent
```

You'll see:

```
 agent  session a1b2c3d
Type your message, or /help for commands. Ctrl+C to cancel, Ctrl+D to exit.

>
```

## Try it out

Type a natural language request:

```
> what files are in this project?
```

The agent will use the `Glob` and `FileRead` tools to explore and answer.

Try something more complex:

```
> add a health check endpoint to the API server that returns the git commit hash
```

The agent will:
1. Read the existing code to understand the project structure
2. Find how other endpoints are defined
3. Write the new endpoint
4. Run tests if they exist

## Slash commands

Type `/help` to see all available commands:

```
> /help

Available commands:

  /help           Show this help message
  /clear          Clear conversation history
  /cost           Show session cost and token usage
  /model          Show or change the current model
  /commit         Commit current changes
  /review         Review current diff for issues
  /plan           Toggle plan mode (read-only)
  /doctor         Check environment health
  ...
```

## One-shot mode

For scripting and CI, use `--prompt` to run a single task and exit:

```bash
agent --prompt "fix the failing tests" --dangerously-skip-permissions
```

## Next steps

<CardGroup cols={2}>
  <Card title="Configuration" icon="gear" href="/configuration/settings">
    Configure models, permissions, and behavior.
  </Card>
  <Card title="Tools" icon="wrench" href="/reference/tools">
    See all 31 built-in tools.
  </Card>
  <Card title="Skills" icon="wand-magic-sparkles" href="/extending/skills">
    Create custom reusable workflows.
  </Card>
  <Card title="MCP Servers" icon="plug" href="/configuration/mcp-servers">
    Connect external tool servers.
  </Card>
</CardGroup>
