
```
agent [OPTIONS]
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `-p, --prompt <TEXT>` | — | Execute a single prompt and exit (non-interactive) |
| `-m, --model <MODEL>` | `claude-sonnet-4-20250514` | Model to use |
| `--api-base-url <URL>` | auto-detected | API endpoint URL |
| `--api-key <KEY>` | from env | API key (prefer env var) |
| `--provider <NAME>` | `auto` | LLM provider: `anthropic`, `openai`, or `auto` |
| `--permission-mode <MODE>` | config `default_mode` | Permission mode: `ask`, `allow`, `deny`, `plan`, `accept_edits`, `auto`. Omitting the flag keeps the configured `[permissions] default_mode`; an unrecognised value is an error |
| `--dangerously-skip-permissions` | false | Skip all permission checks |
| `-C, --cwd <DIR>` | current dir | Working directory |
| `--max-turns <N>` | 50 | Maximum agent turns per request |
| `-v, --verbose` | false | Enable verbose output |
| `--dump-system-prompt` | false | Print the system prompt and exit |
| `-h, --help` | — | Show help |
| `--version` | — | Show version |

## Environment variables

| Variable | Equivalent flag | Description |
|----------|----------------|-------------|
| `AGENT_CODE_API_KEY` | `--api-key` | API key (highest priority) |
| `ANTHROPIC_API_KEY` | `--api-key` | Anthropic API key |
| `OPENAI_API_KEY` | `--api-key` | OpenAI API key |
| `AGENT_CODE_API_BASE_URL` | `--api-base-url` | API endpoint URL |
| `AGENT_CODE_MODEL` | `--model` | Model name |

## Examples

```bash
# Interactive mode with Anthropic
ANTHROPIC_API_KEY=sk-ant-... agent

# One-shot with OpenAI
OPENAI_API_KEY=sk-... agent --model gpt-4o --prompt "explain main.rs"

# Local Ollama
agent --api-base-url http://localhost:11434/v1 --model llama3 --api-key x

# CI: fix tests without asking
agent --dangerously-skip-permissions --prompt "fix the failing tests"

# Read-only exploration
agent --permission-mode plan

# Debug: see what the LLM receives
agent --dump-system-prompt
```
