
## Requirements

- A supported LLM credential (API key or subscription login — see [Authentication](./authentication.md))
- For full tooling: `git` and `rg` (ripgrep) on `PATH`
- Optional sandbox: `bwrap` on Linux

## Install methods

### One-liner (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/avala-ai/agent-code/main/install.sh | bash
agent --version
```

### Cargo

```bash
cargo install agent-code
# installs `agent` to ~/.cargo/bin
```

### Homebrew

```bash
brew install avala-ai/tap/agent-code
```

### Prebuilt binaries

Download from [GitHub Releases](https://github.com/avala-ai/agent-code/releases):

| Platform | Architecture | Archive |
|----------|-------------|---------|
| Linux | x86_64 | `agent-linux-x86_64.tar.gz` |
| Linux | aarch64 | `agent-linux-aarch64.tar.gz` |
| macOS | x86_64 | `agent-macos-x86_64.tar.gz` |
| macOS | Apple Silicon | `agent-macos-aarch64.tar.gz` |
| Windows | x86_64 | `agent-windows-x86_64.zip` |

```bash
# Example: macOS Apple Silicon
curl -L https://github.com/avala-ai/agent-code/releases/latest/download/agent-macos-aarch64.tar.gz | tar xz
sudo mv agent /usr/local/bin/
```

### Docker

```bash
docker run --rm -it -v "$PWD":/work -w /work ghcr.io/avala-ai/agent-code
```

### From source

```bash
git clone https://github.com/avala-ai/agent-code.git
cd agent-code
cargo build --release -p agent-code
# binary: target/release/agent
```

## Verify

```bash
agent --version
agent -p "reply with only: ok" --permission-mode allow
```

## Uninstall

```bash
# curl installer / manual
rm "$(which agent)"

# Cargo
cargo uninstall agent-code

# Homebrew
brew uninstall agent-code
```

Interactive uninstall helper: `agent` → `/uninstall` (when available in your build).

## Data locations

| What | Path |
|------|------|
| User config | `~/.config/agent-code/config.toml` |
| Sessions | `~/.config/agent-code/sessions/` |
| Memory | `~/.config/agent-code/memory/` |
| Skills | `~/.config/agent-code/skills/` |
| Plugins | `~/.config/agent-code/plugins/` |
| History | `~/.local/share/agent-code/history.txt` |
| Tool / task cache | `~/.cache/agent-code/` |

Project overlays often live under `.agent/` in the repo (skills, agents, team memory).

## Next

- [Quickstart](./quickstart.md)
- [Authentication](./authentication.md)
- [Terminal support](./terminal-support.md) for TUI issues under tmux/SSH
