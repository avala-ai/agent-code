
The agent loop is the core execution engine. It handles the cycle of calling the LLM, executing tools, and managing context.

## Turn lifecycle

Each turn follows this sequence:

```
1. Budget check        → stop if cost/token limit exceeded
2. Message normalize   → pair orphaned tool results, merge consecutive user messages
3. Auto-compact        → if context nears window limit:
                           microcompact → LLM summary → context collapse → aggressive trim
4. Build request       → system prompt + history + tool schemas
5. Stream response     → display text in real-time, collect content blocks
6. Error recovery      → rate limit retry, prompt-too-long compact, max-output continue
7. Extract tool calls  → parse tool_use blocks from response
8. Permission check    → allow/deny/ask per tool and pattern
9. Execute tools       → concurrent batch (read-only) or serial (mutations)
10. Inject results     → add tool results to history
11. Loop               → back to step 1 until no tool calls
```

## Compaction strategies

Long sessions exceed the context window. The agent uses three strategies, tried in order:

<Steps>
  <Step title="Microcompact">
    Clears the content of old tool results, replacing with `[Old tool result cleared]`.
    Keeps tool_use/tool_result pairing intact. Cheapest — no API call needed.
  </Step>
  <Step title="LLM summary">
    Calls the LLM to generate a concise summary of older messages. Replaces them
    with a compact boundary marker + summary. Costs one API call but frees significant context.
  </Step>
  <Step title="Context collapse">
    Removes message groups from the middle of the conversation, keeping the first
    (context/summary) and last (recent) groups. The full history stays in memory
    for session persistence — only the API-facing view is collapsed.
  </Step>
</Steps>

## Error recovery

The agent handles these error conditions automatically:

| Error | Recovery |
|-------|----------|
| **Rate limited (429)** | Wait `retry_after` ms, retry up to 5 times |
| **Overloaded (529)** | 5s backoff, retry up to 5 times, then fall back to smaller model |
| **Prompt too long (413)** | Reactive microcompact, then context collapse |
| **Max output tokens** | Inject continuation message, retry up to 3 times |
| **Stream interrupted** | Exponential backoff with retry |

## Token budget

The agent tracks token usage and estimated cost:

- Auto-compact fires at `context_window - 20K reserved - 13K buffer`
- Budget enforcement stops execution when cost or token limits are reached
- Diminishing progress detection stops after 3 turns with minimal output

Configure limits:

```toml
[api]
max_cost_usd = 5.0  # Stop after $5 spent this session
```

## Extended thinking

When using models that support it, the agent sends a thinking budget with each request. The budget scales by model:

| Model | Thinking budget |
|-------|----------------|
| Opus | 32,000 tokens |
| Sonnet | 16,000 tokens |
| Haiku | 8,000 tokens |

Thinking content is displayed briefly in the terminal but not stored in conversation history.
