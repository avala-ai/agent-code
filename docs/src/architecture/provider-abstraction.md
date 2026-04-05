
## The Problem

Different LLM providers use different APIs:

- **Anthropic**: Messages API with `content` blocks, `tool_use`/`tool_result` types, prompt caching
- **OpenAI**: Chat Completions API with `messages`, `tool_calls`, `function` format

agent-code needs to work identically regardless of which provider is configured.

## Architecture

```
User prompt
    │
    ▼
Query Engine (provider-agnostic)
    │
    ▼
Provider Detection (auto from model name + base URL)
    │
    ├── Anthropic wire format → Anthropic Messages API
    └── OpenAI wire format → OpenAI Chat Completions API
    │
    ▼
SSE Stream → Normalize → Unified ContentBlock types
    │
    ▼
Tool execution (same code path regardless of provider)
```

## Provider Detection

`detect_provider()` in `llm/provider.rs` determines the provider from:

1. **Model name**: `claude-*` → Anthropic, `gpt-*` → OpenAI, `grok-*` → xAI, etc.
2. **Base URL**: `api.anthropic.com` → Anthropic, `api.openai.com` → OpenAI, etc.
3. **Environment**: `AGENT_CODE_USE_BEDROCK` → Bedrock, `AGENT_CODE_USE_VERTEX` → Vertex

Each provider maps to a `WireFormat`:

| Wire Format | Providers |
|-------------|-----------|
| `Anthropic` | Anthropic, Bedrock, Vertex |
| `OpenAi` | OpenAI, xAI, Google, DeepSeek, Groq, Mistral, Together, Zhipu, Ollama, any compatible |

## Wire Formats

### Anthropic (`llm/anthropic.rs`)

- Sends `messages` with `content` as array of typed blocks
- Tool calls appear as `tool_use` content blocks in assistant messages
- Tool results are `tool_result` content blocks in user messages
- Supports `cache_control` breakpoints for prompt caching
- Extended thinking via `thinking` content blocks

### OpenAI (`llm/openai.rs`)

- Sends `messages` with `content` as string or array
- Tool calls appear in `tool_calls` array on assistant messages
- Tool results are separate messages with `role: "tool"`
- Supports streaming via SSE with `[DONE]` sentinel

## Message Normalization

`llm/normalize.rs` ensures messages are valid before sending:

- **Tool pairing**: every `tool_use` block must have a matching `tool_result` in the next user message
- **Alternation**: user and assistant messages must alternate (APIs reject consecutive same-role messages)
- **Empty handling**: empty content arrays are removed or filled with placeholder text

This runs after every turn, before the next API call.

## Stream Parsing

`llm/stream.rs` handles SSE (Server-Sent Events) parsing:

1. Read `data:` lines from the HTTP response stream
2. Parse JSON deltas (content block starts, text deltas, tool input deltas)
3. Accumulate into complete `ContentBlock` instances
4. Emit blocks to the UI (real-time text display) and executor (tool dispatch)

The stream parser handles both Anthropic's `content_block_delta` events and OpenAI's `choices[0].delta` format through the wire format abstraction.

## Error Recovery

| Error | Recovery |
|-------|----------|
| Rate limited (429) | Wait `retry_after` ms, retry up to 5 times |
| Overloaded (529) | 5s exponential backoff, fall back to smaller model after 3 attempts |
| Prompt too long (413) | Reactive compaction, then retry |
| Max output tokens | Inject continuation message, retry up to 3 times |
| Stream interrupted | Reconnect with exponential backoff |

The retry state machine in `llm/retry.rs` tracks attempts per error type and supports model fallback (e.g., Opus → Sonnet on overload).

**Source**: `llm/provider.rs` (detection), `llm/anthropic.rs` (Anthropic format), `llm/openai.rs` (OpenAI format), `llm/normalize.rs` (validation), `llm/stream.rs` (SSE parsing), `llm/retry.rs` (error recovery)
