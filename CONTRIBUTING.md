# Contributing to agent-code

Thank you for your interest in contributing.

## Development Setup

```bash
# Clone
git clone https://github.com/avala-ai/agent-code.git
cd agent-code

# Build
cargo build

# Run tests
cargo test

# Lint
cargo clippy --all-targets

# Format
cargo fmt --all
```

## Pull Request Process

1. Fork the repository and create a feature branch from `main`
2. Make your changes with clear, incremental commits
3. Ensure all checks pass: `cargo test && cargo clippy && cargo fmt --check`
4. Open a PR against `main` with a clear description
5. One approval is required before merging
6. PRs are squash-merged or rebased (no merge commits)

## Branch Rules

- `main` is protected — no direct pushes
- All changes go through pull requests
- CI must pass (check, test, format) before merge
- Stale review dismissal is enabled — push new commits to re-trigger review

## Code Style

- Run `cargo fmt` before committing
- Fix all `cargo clippy` warnings
- Write tests for new functionality
- Document public APIs with `///` doc comments
- Keep modules focused and files under ~300 lines where practical

## What We Accept

- Bug fixes with test cases
- New tools (implement the `Tool` trait)
- New commands (add to `commands/mod.rs`)
- Performance improvements with benchmarks
- Documentation improvements

## What We Don't Accept

- Changes that add vendor lock-in to any specific AI provider
- Telemetry that sends data to external services without opt-in
- Dependencies with restrictive licenses (GPL, AGPL)
- Large refactors without prior discussion in an issue

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
