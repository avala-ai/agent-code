
## API Connection

### "API key required"

The agent could not find an API key. Set one of:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."   # Anthropic
export OPENAI_API_KEY="sk-..."          # OpenAI
export AGENT_CODE_API_KEY="..."         # Any provider
```

Or add it to your config file:

```toml
# ~/.config/agent-code/config.toml
[api]
# api_key is resolved from env vars — don't put keys in config files
```

### "Connection refused" or timeout

- Check your internet connection
- Verify the API base URL: `agent --dump-system-prompt 2>&1 | head -1`
- For local models (Ollama): ensure the server is running (`ollama serve`)
- For corporate proxies: set `HTTPS_PROXY` environment variable

### Rate limited (429)

The agent retries automatically up to 5 times with backoff. If it persists:

- Switch to a less busy model: `/model`
- Wait a few minutes and retry
- Check your API plan's rate limits

## Permission Issues

### "Denied by rule" on a command you want to run

Check your permission rules:

```
> /permissions
```

Add an allow rule:

```toml
# .agent/settings.toml
[[permissions.rules]]
tool = "Bash"
pattern = "your-command *"
action = "allow"
```

### "Write to .git/ is blocked"

This is a built-in safety measure. The agent cannot write to `.git/`, `.husky/`, or `node_modules/` regardless of permission settings. This prevents repository corruption and dependency tampering.

If you need to modify git config, run the command yourself with `!`:

```
> !git config user.name "Your Name"
```

## Context Window

### "Prompt too long" or context exceeded

The agent auto-compacts when approaching the limit. If it still fails:

- Run `/compact` to manually free context
- Start a new session for a fresh context window
- Use `/snip 0-10` to remove old messages
- Switch to a model with a larger context window

### Agent seems to forget earlier instructions

Long conversations get compacted automatically. Important context may be summarized. To preserve critical instructions:

- Put them in `AGENTS.md` (loaded every session)
- Use `/memory` to check what context is loaded
- Start a fresh session with `/clear` if context is corrupted

## Tool Errors

### Bash command fails silently

- Check the command works manually: `!your-command`
- The agent may not have the right PATH. Set it in your shell profile.
- On Windows, some commands need PowerShell syntax

### "File content has changed" on edit

Another process modified the file between the agent reading and editing it. The agent will re-read and retry. If it persists:

- Close other editors or watchers on the file
- Disable auto-formatting hooks temporarily

### Grep/Glob returns no results

- Verify `ripgrep` is installed: `!rg --version`
- Check the working directory: `/files`
- The pattern may need escaping — try a simpler pattern first

## MCP Servers

### MCP server stuck "connecting"

- Check the server command works: `!npx -y @modelcontextprotocol/server-name`
- Verify the config in `/mcp`
- Check server logs in the terminal where agent-code runs

### MCP tools not appearing

- Run `/mcp` to verify the server is connected
- The server may not have registered tools yet — restart the agent
- Check `mcp_server_allowlist` in security config isn't blocking it

## Installation

### "command not found: agent"

The binary isn't in your PATH.

```bash
# Cargo install location
export PATH="$HOME/.cargo/bin:$PATH"

# Or find it
which agent || find / -name agent -type f 2>/dev/null | head -5
```

### Build fails from source

```bash
# Ensure Rust is up to date
rustup update stable

# Clean and rebuild
cargo clean
cargo build --release

# Check dependencies
cargo check --all-targets
```

### ripgrep not found

Grep and some other tools require `rg` (ripgrep):

```bash
# Linux
sudo apt-get install ripgrep

# macOS
brew install ripgrep

# Windows
choco install ripgrep
```

## Sessions

### Can't resume a session

- List available sessions: `/sessions`
- Session files are in `~/.config/agent-code/sessions/`
- Old sessions may have been cleaned up
- Session format may be incompatible after an upgrade — start fresh

## Still Stuck?

- Run `/doctor` for a full environment health check
- Check [GitHub Issues](https://github.com/avala-ai/agent-code/issues) for known problems
- Open a new issue with: agent version (`agent --version`), OS, and steps to reproduce
