# AGENTS.md

Instructions for AI coding agents working in this repository. Read this file in full before making changes. It captures the non-obvious rules — conventions you cannot derive from just grepping the code.

This repo is **agent-code** (Avala AI): a Rust terminal coding agent, shipped as both a CLI and an embeddable library. Because this project *is itself* an agent, changes here affect the safety posture of every user who runs it. Move carefully.

**Throughput (parallel agents / multi-crate work):** read **[SPEED.md](SPEED.md)** — blast radius, non-overlapping lanes, **one `git worktree` per concurrent writer**, atomic commits. Does not lower the security or CI bar below.

---

## 1. Repo layout

```
crates/
  lib/        # agent-code-lib — LLM providers, tools, query loop, memory,
              #   permissions, MCP client, skills, compaction. The engine.
  cli/        # agent-code    — binary: REPL, TUI, slash commands, streaming output.
  eval/       # agent-code-eval — behavioral evaluation harness (not published).
client/       # Flutter desktop/web client that talks to `agent --serve`.
packages/     # TypeScript client library (`agent_code_client`).
npm/          # npm installer wrapper.
docs/         # Mintlify + mdBook docs (architecture, guides, reference).
evals/        # Eval fixtures and scenarios consumed by `crates/eval`.
scripts/      # E2E shell harnesses.
.github/workflows/  # CI — `ci.yml` is the canonical gate.
```

Supporting docs you should read before a non-trivial change:
`README.md`, `ARCHITECTURE.md`, `SECURITY.md`, `CONTRIBUTING.md`, `ROADMAP.md`, `RELEASING.md`, `CHANGELOG.md`, `SPEED.md` (when multi-agent or multi-crate).

---

## 2. Build, test, lint — the CI gate

Every PR must pass `ci.yml`. Run these locally before pushing; they are the exact commands CI runs.

```bash
cargo check   --all-targets                   # compile
cargo test    --all-targets                   # unit + integration + doc tests
cargo clippy  --all-targets -- -D warnings    # lint — warnings are errors
cargo fmt     --all -- --check                # format gate
```

Release builds (matrix of targets) use:

```bash
cargo build --release --target <triple>
```

Benchmarks live in `crates/lib/benches/`:

```bash
cargo bench --bench compaction
cargo bench --bench token_estimation
cargo bench --bench startup
```

### Running a single test

```bash
cargo test -p agent-code-lib <module>::<test_name>
cargo test -p agent-code     <test_name>          # CLI crate
```

### What tests need

- Unit and integration tests in `crates/lib/tests/` and `crates/cli/tests/` are **hermetic** — no network, no API keys. Keep them that way.
- Eval runs in `crates/eval/` and `evals-nightly.yml` may require `ANTHROPIC_API_KEY`. Do not add key requirements to the default `cargo test` path.
- If you add a test that needs network, gate it with `#[ignore]` and document how to run it.

### Format and lint config

There is **no `rustfmt.toml` and no `clippy.toml`** — defaults only. Do not add one without discussion; style changes cascade through the whole tree. The `-D warnings` flag means any new clippy lint must be fixed, not suppressed. Prefer fixing the code; use `#[allow(...)]` only with a one-line comment explaining why.

---

## 3. Security rules — non-negotiable

This project's security posture is load-bearing. Breaking any of the following is a hard blocker for merge.

### Protected directories

`crates/lib/src/permissions/mod.rs` defines `PROTECTED_DIRS`:

```rust
const PROTECTED_DIRS: &[&str] = &[
    ".git/", ".git\\",
    ".husky/", ".husky\\",
    "node_modules/", "node_modules\\",
];
```

Write tools (`FileWrite`, `FileEdit`, `MultiEdit`, `NotebookEdit`) are **unconditionally** blocked from these paths — the check runs before any permission rule. If you extend the write-tool set, add the new tool to `is_write_tool()` in the same file. If you add a new protected path, add both forward- and back-slash variants.

### Permission system invariants

- Every tool call goes through the permission check. No exceptions. Do not add a "trusted" bypass.
- Read-only tools skip the ask prompt; write/exec tools do not. When adding a new tool, classify it correctly.
- Plan mode must remain read-only. If your change adds a tool, explicitly decide whether plan mode can call it and test that branch.
- The `--dangerously-skip-permissions` flag exists for automation use cases. It can be globally disabled via `disable_bypass_permissions = true` in settings. **Never weaken or remove that setting's effect.**

### Bash tool destructive-command detection

The bash tool warns on patterns like `rm -rf`, `git reset --hard`, `DROP TABLE`, etc. and blocks writes to system paths (`/etc`, `/usr`, `/bin`, `/sbin`, `/boot`, `/sys`, `/proc`). Do not narrow these checks. If you add a new one, add a test in the same file.

### Skills

Skills can embed shell blocks. The `disable_skill_shell_execution` setting strips them. Any new skill-loading code path must respect that setting.

### Secrets and telemetry

- **Never** write API keys to config files. Env vars only. The config schema should not grow a `*_api_key` field.
- **Never** add telemetry that leaves the machine without explicit opt-in. The only outbound network calls allowed by default are to the user-configured LLM provider and MCP servers.
- Do not commit `.env` files, API keys, or session transcripts. The `.gitignore` blocks most of this; do not bypass it with `git add -f`.

### MCP servers

MCP servers run user-configured external processes. Any change to MCP loading must preserve the allowlist/denylist configuration surface.

---

## 4. Coding conventions

These are the patterns the existing code uses. Follow them; do not introduce parallel conventions.

- **Error handling**: `thiserror`-based enums. Top-level `Error`, subsystem variants (`LlmError`, `ToolError`, `PermissionError`, `ConfigError`). Prefer returning `Result` with the right subsystem error over `anyhow` in library code. `anyhow` is acceptable in the `cli` crate and tests.
- **Async runtime**: `tokio`. Use `tokio::select!` for cancellation and `CancellationToken` to propagate Ctrl+C. All tool calls, provider calls, and I/O are async.
- **Tool trait**: implement `Tool: Send + Sync` with `name()`, `description()`, `call()`. Register in the tool registry. Tool dispatch is dynamic (`Arc<dyn Tool>`) — **do not** introduce a central `enum` of tools.
- **Provider trait**: new LLM providers implement the provider trait in `crates/lib/src/providers/`. Normalize provider-specific payloads to the internal message format; do not leak provider quirks into the query loop.
- **Compaction**: treat token budget as a first-class concern. The three strategies (microcompact, LLM summary, context collapse) exist for a reason — do not bypass them with "just read the whole file" shortcuts.
- **Configuration**: layered `user → project → env → CLI flags`. Do not invert this precedence. New config keys go in `ConfigSchema` with a doc comment.
- **No new crates in the workspace** without discussion — the 3-crate split (`lib`, `cli`, `eval`) is intentional.
- **Comments**: default to none. Write one only when the *why* is non-obvious. Do not narrate what the code does.

---

## 5. Commit and PR conventions

### Commit messages

Conventional Commits, lowercase type, short subject. Observed prefixes: `feat`, `fix`, `docs`, `test`, `perf`, `ci`, `refactor`, `chore`. Scope is optional but encouraged: `fix(ci): …`, `feat(tools): …`.

Examples from recent history:

```
feat: implement interactive /uninstall command
fix(ci): skip bash-dependent tests on Windows and add job timeout
fix: make Escape and Ctrl+C actually interrupt agent during streaming
```

No DCO sign-off is required. **Do not** add `Co-Authored-By: Claude` or any Anthropic/Claude attribution in commits made in this repo — use only the human author. No `Generated with ...` trailers.

### Pull requests

The PR template (`.github/PULL_REQUEST_TEMPLATE.md`) requires:

- A one-to-three-bullet summary
- A test plan checklist covering the full CI gate: `cargo check --all-targets`, `cargo test --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check`, and new tests for new functionality

One approval merges. Prefer squash or rebase merges over merge commits. Do not force-push to `main`.

### What PRs get rejected for

From `CONTRIBUTING.md` and practice:

- Adding vendor lock-in to a specific AI provider
- Telemetry without opt-in
- Dependencies under restrictive licenses (GPL, AGPL)
- Large refactors without a prior issue
- Weakening any security invariant in §3

---

## 6. Release process (summary)

Full details in `RELEASING.md`; that file is authoritative for release branch names, PR title/body/label, version bump list, changelog, verification, and tag steps. At a glance:

1. Branch `release/vX.Y.Z` from `main`
2. Bump versions in `crates/lib/Cargo.toml`, `crates/cli/Cargo.toml`, the `crates/eval/Cargo.toml` `agent-code-lib` path dependency, `Cargo.lock`, and `npm/package.json`
3. Stamp `CHANGELOG.md` (Keep-a-Changelog format)
4. Commit as `chore(release): prepare vX.Y.Z`
5. Open a regular PR titled `Release vX.Y.Z`, add the `run-e2e` label immediately, and use the release PR body skeleton from `RELEASING.md`
6. Require the full CI gate, GitHub Actions CI, and the `run-e2e` workflow before merge
7. After merge, tag on `main` with `git tag vX.Y.Z && git push origin vX.Y.Z`

Tag push triggers: `release.yml` (binaries + crates.io), `docker.yml` (`ghcr.io/avala-ai/agent-code:X.Y.Z`), the npm publish workflow, and the Homebrew tap update. If you are making a release, do not run these workflows manually — let the tag drive them.

Semantic versioning: patch = bug fixes, minor = features, major reserved for v1.0+.

---

## 7. Things to not do

A concentrated list. If you are about to do any of these, stop and ask.

- Do **not** bypass or weaken the permission check, protected-directory check, or destructive-command detection
- Do **not** write API keys to config files or commit `.env`
- Do **not** add telemetry that phones home by default
- Do **not** introduce a central tool enum — tools are dynamic trait objects
- Do **not** add `rustfmt.toml` or `clippy.toml` without discussion
- Do **not** suppress clippy warnings with blanket `#[allow]` — fix the underlying issue
- Do **not** add network-requiring tests to the default `cargo test` path
- Do **not** add Agent-Code, Claude/Anthropic, Codex, etc attribution to commits in this repo
- Do **not** force-push to `main` or skip the CI gate
- Do **not** land large refactors without a prior issue or roadmap entry
- Do **not** add dependencies under GPL/AGPL or other restrictive licenses
- Do **not** modify `.git/`, `.husky/`, or `node_modules/` — even tests go around these

---

## 8. Where to ask when stuck

- Architecture questions → `ARCHITECTURE.md`
- Security model → `SECURITY.md` and `crates/lib/src/permissions/`
- Roadmap / what's planned → `ROADMAP.md`
- Release process → `RELEASING.md`
- Provider-specific behavior → `crates/lib/src/providers/`
- Tool behavior → `crates/lib/src/tools/`
- Slash commands → `crates/cli/src/commands/`

If a doc disagrees with the code, trust the code and file an issue to update the doc.
