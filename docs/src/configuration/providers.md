
agent-code works with any LLM that speaks the Anthropic Messages API or OpenAI Chat Completions API. The provider is auto-detected from your model name and base URL.

## Anthropic (Claude)

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
agent
```

Supported models: Claude Opus, Sonnet, Haiku (all versions).

Features enabled: prompt caching, extended thinking, cache_control breakpoints.

## OpenAI (GPT)

```bash
export OPENAI_API_KEY="sk-..."
agent --model gpt-4o
```

Supported models: GPT-4o, GPT-4, o1, o3, and others.

## xAI (Grok)

```bash
export XAI_API_KEY="xai-..."
agent --model grok-3
```

Supported models: Grok-3, Grok-3-mini, Grok-2, and others.

Auto-detected from `XAI_API_KEY` env var, `x.ai` in the base URL, or model names starting with `grok`. Uses the OpenAI-compatible wire format.

You can also set it explicitly:

```bash
agent --provider xai --model grok-3
```

## Ollama (local)

```bash
agent --api-base-url http://localhost:11434/v1 --model llama3 --api-key unused
```

No API key needed (pass any string). Start Ollama first: `ollama serve`.

## Groq

```bash
agent --api-base-url https://api.groq.com/openai/v1 --api-key gsk_... --model llama-3.3-70b-versatile
```

## Together AI

```bash
agent --api-base-url https://api.together.xyz/v1 --api-key ... --model meta-llama/Llama-3-70b-chat-hf
```

## DeepSeek

```bash
agent --api-base-url https://api.deepseek.com/v1 --api-key ... --model deepseek-chat
```

## OpenRouter

```bash
agent --api-base-url https://openrouter.ai/api/v1 --api-key ... --model anthropic/claude-sonnet-4
```

OpenRouter lets you access any model through a single API key.

## Explicit provider selection

If auto-detection doesn't work for your setup, force it:

```bash
agent --provider anthropic  # Use Anthropic wire format
agent --provider openai     # Use OpenAI wire format
agent --provider xai        # Use xAI (Grok) via OpenAI wire format
```

## Auto-detection logic

The provider is chosen by checking (in order):

1. `--provider` flag (if set)
2. Base URL contains `anthropic.com` → Anthropic
3. Base URL contains `openai.com` → OpenAI
4. Base URL contains `x.ai` → xAI
5. Base URL is `localhost` → OpenAI-compatible
6. Model name starts with `claude`/`opus`/`sonnet`/`haiku` → Anthropic
7. Model name starts with `gpt`/`o1`/`o3` → OpenAI
8. Model name starts with `grok` → xAI
9. Default → OpenAI-compatible (most common API shape)

## API key resolution

Keys are checked in this order (first found wins):

1. `--api-key` CLI flag
2. `AGENT_CODE_API_KEY` env var
3. `ANTHROPIC_API_KEY` env var
4. `OPENAI_API_KEY` env var
5. `XAI_API_KEY` env var
6. Config file (`api.api_key`)
