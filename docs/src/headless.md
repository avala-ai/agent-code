
Headless mode runs Agent Code without the interactive TUI. Use it for CI, shell pipelines, bots, and automation.

## One-shot prompt

```bash
agent -p "Summarize the last three commits"
agent --prompt "Fix the failing tests" --model claude-sonnet-5
```

The process streams the answer, runs tools under the configured permission mode, then exits.

### Useful flags

| Flag | Description |
|------|-------------|
| `-p` / `--prompt` | Prompt text (triggers non-interactive mode) |
| `--output-format text\|json` | `text` (default) or JSONL events on stdout |
| `-m` / `--model` | Model id |
| `--provider` | Provider hint (`auto`, `anthropic`, `openai`, `xai`, …) |
| `--permission-mode` | `ask` · `allow` · `deny` · `plan` · `accept_edits` |
| `--dangerously-skip-permissions` | Equivalent to allow-all (blocked if enterprise lock is on) |
| `--max-turns N` | Cap agentic turns |
| `-C` / `--cwd` | Working directory |
| `--no-sandbox` | Disable process sandbox for this run (if enabled) |

Example CI step:

```bash
export ANTHROPIC_API_KEY="…"
agent -p "Run cargo test and fix compile errors" \
  --permission-mode allow \
  --max-turns 40
```

### JSON output

```bash
agent -p "list the public modules in crates/lib" --output-format json
```

Structured events go to **stdout**; human status messages go to **stderr**, so you can pipe cleanly:

```bash
agent -p "…" --output-format json 2>/dev/null | jq .
```

## HTTP server

```bash
agent --serve --port 4096
```

Starts a local HTTP API with SSE streaming for the Flutter client and other tools. Attach with:

```bash
agent --attach
# or agent --attach <session-prefix>
```

## ACP (IDE bridge)

```bash
agent acp
```

Speaks Agent Client Protocol over stdio (JSON-RPC). Editors spawn this process and stream prompts, tool visibility, and permission requests. See [IDE bridge](./extending/ide-bridge.md).

## Permissions in automation

- Prefer explicit `--permission-mode allow` (or a narrow overlay file) over global YOLO in shared configs.
- Org lock: `[security] disable_bypass_permissions = true` ignores bypass flags.
- Protected directories (`.git/`, `node_modules/`, …) remain unwritable even when permissions are open.

## Related

- [CLI flags](./reference/cli-flags.md)
- [Authentication](./authentication.md)
- [Sandbox](./sandbox.md)
