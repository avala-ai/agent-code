
```
rc [OPTIONS]
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `-p, --prompt <TEXT>` | — | Execute a single prompt and exit (non-interactive) |
| `-m, --model <MODEL>` | `claude-sonnet-4-20250514` | Model to use |
| `--api-base-url <URL>` | auto-detected | API endpoint URL |
| `--api-key <KEY>` | from env | API key (prefer env var) |
| `--provider <NAME>` | `auto` | LLM provider: `anthropic`, `openai`, or `auto` |
| `--permission-mode <MODE>` | `ask` | Permission mode: `ask`, `allow`, `deny`, `plan`, `accept_edits` |
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
ANTHROPIC_API_KEY=sk-ant-... rc

# One-shot with OpenAI
OPENAI_API_KEY=sk-... rc --model gpt-4o --prompt "explain main.rs"

# Local Ollama
rc --api-base-url http://localhost:11434/v1 --model llama3 --api-key x

# CI: fix tests without asking
rc --dangerously-skip-permissions --prompt "fix the failing tests"

# Read-only exploration
rc --permission-mode plan

# Debug: see what the LLM receives
rc --dump-system-prompt
```
