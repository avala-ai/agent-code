
agent-code works with 12+ LLM providers. This tutorial shows how to switch between them and set up your preferred workflow.

## Quick switch: one env var

Each provider is activated by setting its API key:

```bash
# Anthropic (Claude)
export ANTHROPIC_API_KEY="sk-ant-..."
agent

# OpenAI (GPT)
export OPENAI_API_KEY="sk-..."
agent

# Google (Gemini)
export GOOGLE_API_KEY="AIza..."
agent
```

The agent auto-detects the provider from which key is set and configures the correct API endpoint.

## Switch mid-session

Use the `/model` command to open the interactive model picker:

```
> /model
```

This shows models available for your current provider. Or specify directly:

```
> /model gpt-4.1-mini
```

## Use a specific provider via config

For permanent setup, edit your config file:

```toml
# ~/.config/agent-code/config.toml

[api]
model = "claude-sonnet-4-20250514"
```

## Local models with Ollama

Run models locally with zero API cost:

```bash
# Install and start Ollama
ollama serve

# Pull a model
ollama pull llama3

# Run agent-code with it
agent --api-base-url http://localhost:11434/v1 --model llama3 --api-key unused
```

The `--api-key unused` is required (Ollama ignores it but the flag is needed).

## Any OpenAI-compatible endpoint

agent-code works with any service that speaks the OpenAI Chat Completions API:

```bash
# OpenRouter (access any model via one key)
agent --api-base-url https://openrouter.ai/api/v1 --api-key sk-or-... --model anthropic/claude-sonnet-4

# Together AI
agent --api-base-url https://api.together.xyz/v1 --api-key ... --model meta-llama/Llama-3-70b-chat-hf

# Groq (fast inference)
agent --api-base-url https://api.groq.com/openai/v1 --api-key gsk_... --model llama-3.3-70b-versatile

# Your own endpoint
agent --api-base-url http://localhost:8080/v1 --api-key ... --model my-model
```

## AWS Bedrock

Access Claude models through your AWS account:

```bash
export AGENT_CODE_USE_BEDROCK=1
export AWS_REGION=us-east-1
# Uses your default AWS credential chain (env vars, ~/.aws/credentials, IAM role)
agent
```

## Google Vertex AI

Access Claude models through Google Cloud:

```bash
export AGENT_CODE_USE_VERTEX=1
export GOOGLE_CLOUD_PROJECT=my-project
export GOOGLE_CLOUD_LOCATION=us-central1
agent
```

## Model recommendations by task

| Task | Recommended | Why |
|------|------------|-----|
| Quick fixes, small edits | GPT-4.1-mini, Haiku, Gemini Flash | Fast, cheap |
| Feature implementation | Sonnet, GPT-4.1 | Good balance |
| Complex architecture | Opus, GPT-5.4 | Maximum reasoning |
| Local/private code | Ollama + Llama 3 | No data leaves your machine |

## Cost tracking

Check what you're spending:

```
> /cost
```

Set a session limit:

```toml
[api]
max_cost_usd = 5.0  # Stop after $5
```

The `/cost` command shows per-model breakdown when you've used multiple models in one session.
