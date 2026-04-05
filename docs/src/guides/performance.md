
## Model Selection

The single biggest performance lever is model choice.

| Priority | Model Type | Examples | Tradeoff |
|----------|-----------|----------|----------|
| Speed | Small/fast | GPT-4.1-mini, Haiku, Gemini Flash | Faster, cheaper, less capable |
| Balance | Mid-tier | Sonnet, GPT-4.1 | Good for most tasks |
| Quality | Large | Opus, GPT-5.4 | Slower, expensive, best reasoning |

Switch mid-session with `/model` — use a fast model for exploration, switch to a capable model for complex changes.

## Context Management

### Monitor usage

```
> /context
Context: ~45000 tokens (23% of 200000 window)
Auto-compact at: 167000 tokens
Messages: 24
```

### Manual compaction

If context is getting large, compact proactively:

```
> /compact
Freed ~12000 estimated tokens.
```

### Start fresh when stuck

If the agent seems confused or repetitive, clear context:

```
> /clear
```

Or start a new session — previous sessions are auto-saved and can be resumed with `/resume`.

## Cost Control

### Set a budget

```toml
[api]
max_cost_usd = 5.0
```

The agent stops when the budget is reached. Check spending with `/cost`.

### Reduce cost per turn

- **Shorter prompts**: be specific, avoid repeating instructions the agent already has
- **Use AGENTS.md**: persistent context loads once instead of being re-explained each session
- **Smaller models for simple tasks**: `/model gpt-4.1-mini` for quick edits, switch back for complex work
- **Plan mode**: `/plan` for exploration uses only read tools (cheaper turns)

## Token Budget

Enable budget tracking to get warnings before hitting limits:

```toml
[features]
token_budget = true
```

The agent shows a warning when approaching the auto-compact threshold.

## Compaction Tuning

The default thresholds work well for most use cases. For very long sessions:

- **Microcompact** fires first and is free — it clears old tool results
- **LLM summary** costs one API call but frees significant context
- **Context collapse** is a last resort — removes middle messages entirely

If sessions are compacting too aggressively, consider using a model with a larger context window (200K+).

## Tool Execution Speed

### Streaming execution

Tools execute as soon as their input is parsed from the LLM response stream — they don't wait for the full response. This is automatic and requires no configuration.

### Parallel reads

Read-only tools (FileRead, Grep, Glob, WebFetch) run in parallel. The agent naturally batches reads when exploring code. No tuning needed.

### Bash timeout

Long-running shell commands can be given explicit timeouts:

```json
{"command": "npm test", "timeout": 60000}
```

The default timeout is 120 seconds. Background mode (`run_in_background: true`) has no timeout.

## Session Persistence

Sessions auto-save on exit. For long-running work:

- Use `/fork` to create a checkpoint before risky changes
- Use `/resume <id>` to return to a previous state
- Use `/export` or `/share` to save a readable copy

## Benchmarking

Run the built-in benchmarks to measure performance on your machine:

```bash
cargo bench                          # All benchmarks
cargo bench -- microcompact          # Compaction only
cargo bench -- estimate_tokens       # Token estimation only
```

Results with HTML reports are generated in `target/criterion/`.
