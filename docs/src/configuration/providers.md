
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
```

## Auto-detection logic

The provider is chosen by checking (in order):

1. `--provider` flag (if set)
2. Base URL contains `anthropic.com` → Anthropic
3. Base URL contains `openai.com` → OpenAI
4. Base URL is `localhost` → OpenAI-compatible
5. Model name starts with `claude`/`opus`/`sonnet`/`haiku` → Anthropic
6. Model name starts with `gpt`/`o1`/`o3` → OpenAI
7. Default → OpenAI-compatible (most common API shape)
