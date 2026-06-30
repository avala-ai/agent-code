
Hooks let you run shell commands or HTTP requests at specific points in the agent's lifecycle. Use them for auto-formatting, linting, notifications, or custom validation.

## Configuration

```toml
# .agent/settings.toml

# Auto-format Rust files after any write
[[hooks]]
event = "post_tool_use"
tool_name = "FileWrite"
[hooks.action]
type = "shell"
command = "cargo fmt"

# Lint after edits
[[hooks]]
event = "post_tool_use"
tool_name = "FileEdit"
[hooks.action]
type = "shell"
command = "cargo clippy --quiet"

# Notify on session start
[[hooks]]
event = "session_start"
[hooks.action]
type = "http"
url = "https://hooks.slack.com/services/T.../B.../..."
method = "POST"
```

## Hook events

| Event | When it fires |
|-------|--------------|
| `session_start` | Session begins |
| `session_stop` | Session ends |
| `user_prompt_submit` | User submits input (incl. steered mid-turn input) |
| `pre_turn` / `post_turn` | Around each agent turn |
| `pre_tool_use` | Before a tool executes (non-zero exit / failure vetoes the call) |
| `post_tool_use` | After a tool completes |
| `file_changed` | After a file-mutating tool completes |
| `pre_compact` / `post_compact` | Around history compaction |
| `task_completed` | A background task (`bash … &` or a spawned subagent) finished |
| `stop` | Agent finished responding; about to yield to the user |
| `notification` | Agent needs user attention (budget / context full) |
| `permission_denied` | A tool call was denied (per-denial, batched per turn) |
| `cwd_changed` / `config_change` / `error` | Working dir changed / extensions reloaded / turn errored |

## Hook context

Every hook receives the event's context (which task finished, which tool ran, the prompt, etc.):

- **stdin** — the full context as a single JSON line.
- **environment** — `AGENT_CODE_HOOK_EVENT`, `AGENT_CODE_HOOK_TOOL` (when applicable), and `AGENT_CODE_HOOK_CONTEXT` (the JSON, when small enough to pass safely; large contexts omit it and set `AGENT_CODE_HOOK_CONTEXT_TRUNCATED=1` — use stdin instead).
- **HTTP** — `http` hooks receive the context as the request body (`POST`).

The `task_completed` context carries `id`, `kind`, `status`, `description`, and `duration_secs`.

## Hook actions

### Shell

Run a command in the project directory:

```toml
[hooks.action]
type = "shell"
command = "make lint"
```

### HTTP

Send a request to a URL:

```toml
[hooks.action]
type = "http"
url = "https://example.com/webhook"
method = "POST"
```

## Filtering by tool

Use `tool_name` to run hooks only for specific tools:

```toml
[[hooks]]
event = "pre_tool_use"
tool_name = "Bash"
[hooks.action]
type = "shell"
command = "echo 'Bash command about to run'"
```

Without `tool_name`, the hook fires for all tools.

## Commands

```
> /hooks
Hook system active. Configure hooks in .agent/settings.toml:
  [[hooks]]
  event = "pre_tool_use"
  action = { type = "shell", command = "./check.sh" }
```
