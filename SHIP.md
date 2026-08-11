# SHIP.md — Throughput without quality debt

**Audience:** humans and coding agents shipping in this repository.

**Purpose:** raise *landed, correct* change rate via parallel work and tight feedback
loops — without weakening [`AGENTS.md`](AGENTS.md), [`SECURITY.md`](SECURITY.md), or
the CI gate.

**Not a goal:** commit count, token spend, or “vibe” volume. Those are lagging noise.
Optimize for **green PRs merged per week** and **time from idea → first green CI**.

---

## 1. Relationship to the other root docs

| Doc | Job |
|-----|-----|
| [`AGENTS.md`](AGENTS.md) | Non-obvious conventions, CI gate, security rules |
| [`SECURITY.md`](SECURITY.md) | Security posture (load-bearing for an agent product) |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Human contribution flow |
| **This file** | How to *structure work* so many agents ship in parallel without thrashing |

**When to read this file (opt-in):** multi-agent work, multi-crate / cross-lane changes,
or a PR that is about to bloat. **Skip** for tiny single-crate edits (even if a few files
change for tests). Root [`AGENTS.md`](AGENTS.md) uses the same threshold.

---

## 2. Blast radius (size the work first)

Before editing, name the blast radius out loud (or in the agent prompt):

| Size | Typical scope | Parallel? |
|------|---------------|-----------|
| **Tiny** | 1–3 files, one concern, &lt;~150 LOC | Alone or as a satellite next to a hard task |
| **Medium** | One crate / one package slice | One agent or one PR |
| **Large** | lib + CLI contract, or Rust + Flutter client | **Stack** sequential PRs — never one mega-PR |
| **Dangerous** | Permissions, tool sandbox, auth, secrets, install path, release | Serial, human-in-loop; no parallel writers |

Rules:

1. Prefer **many tiny/medium PRs** over one fat PR reviewers cannot hold in their head.
2. If an agent run exceeds the expected radius (wrong crates, surprise rewrites), **stop**,
   re-scope, and continue — do not “finish the wander.”
3. **Unknown blast radius** → explore read-only first, then re-plan. Do not start four writers.

---

## 3. Parallel lanes (the real speedup)

Throughput comes from **non-overlapping writers**, not from more tokens on the same files.

### Default lane map

| Lane | Conflict scope (write exclusive) | Notes |
|------|----------------------------------|-------|
| **lib** | `crates/lib/` | Engine: providers, tools, query loop, permissions, agent tools |
| **cli** | `crates/cli/` | Binary, TUI, slash commands — depends on lib |
| **eval** | `crates/eval/`, `evals/` | Harness + fixtures; keep default tests hermetic |
| **client** | `client/` | Flutter desktop/web UI for `agent --serve` |
| **dart-client** | `packages/` | **Dart** client package (`agent_code_client` via pub) — not TypeScript |
| **npm / install** | `npm/`, `install.sh` | Release/install path — serialize with care |
| **docs** | `docs/`, root prose (`AGENTS.md`, this file, CHANGELOG) | — |
| **scripts / ci** | `scripts/`, `.github/workflows/` | Prefer dedicated PRs |

There is no separate “agent harness” lane under `.claude/` for product work. Harness/tooling
behavior for the product lives under **`crates/lib/`** (e.g. tools/agent integration). Optional
local Claude config under `.claude/` is lead-only if present and is not a parallel product lane.

**Lane ownership ≠ permission.** Conflict scope does not override AGENTS.md security rules or
protected-path policy.

Hard rules:

1. **Two writers never share a file.** Split by directory first; if a shared file is
   unavoidable, **serialize** edits through one owner.
2. **Contract changes stack, they do not race.** `agent-code-lib` API → CLI / client consumers
   is a **stack** (lib lands first), not parallel PRs against `main` that both edit the surface.
3. **One `git worktree` per concurrent lane (required).** Do not run two writing agents in
   the same working tree.
4. **Lead owns integration.** When multiple lanes land, one person/agent rebases the stack,
   runs the CI gate commands, and writes the PR narrative.

### `git worktree` — default for parallel / multi-agent work

Resolve the **upstream** remote (the repo PRs target — usually `avala-ai/agent-code`). On a
fork, that is often `upstream`, not `origin`.

```bash
UPSTREAM=$(git remote -v | awk '/avala-ai\/agent-code/ {print $1; exit}')
UPSTREAM=${UPSTREAM:-origin}
git fetch "$UPSTREAM" main

# --no-track so feature branches do not track main (plain `git push` stays sane)
git worktree add --no-track -b fix/lib-<slug>    ../agent-code-wt-lib    "$UPSTREAM/main"
git worktree add --no-track -b fix/client-<slug> ../agent-code-wt-client "$UPSTREAM/main"

# Agent A cwd: ../agent-code-wt-lib     (writes crates/lib/ only)
# Agent B cwd: ../agent-code-wt-client  (writes client/ only)

# First publish:
#   git push -u origin HEAD

# After PR merge or abandon — squash-safe cleanup:
git worktree remove ../agent-code-wt-lib
gh pr view <n> --json state --jq .state   # MERGED, or confirm abandon
git branch -D fix/lib-<slug>              # -d often fails after squash merge
```

Rules of use:

- **Branch from `$UPSTREAM/main`** (or the stack base), never from a dirty unrelated feature branch
  unless stacking.
- **Agent cwd is the worktree root** for that lane — not the primary tree.
- **Do not** `git worktree add` into a path that already has uncommitted work.
- List with `git worktree list`; prune stale entries with `git worktree prune` after deletes.

Single-lane, single-agent work may stay in the primary tree. The moment a second writer starts,
**spin a worktree**.

### Prompt shape for a parallel agent

```text
Lane: lib only. Cwd: <worktree path>. Write only under crates/lib/.
Goal: <one sentence>.
Out of scope: crates/cli, client/, packages/.
Done when: cargo test -p agent-code-lib <relevant> green + AGENTS.md security self-check.
Commit atomically as you go (see §4). Do not open the PR unless asked.
```

---

## 4. Commit rhythm inside a PR (atomic, not ceremonial)

Agents **should** commit as they go **inside their own worktree**.

1. **Atomic commits** — one concern per commit. Prefer Conventional Commit subjects when
   natural: `fix(lib): …`, `test(cli): …`, `docs: …`.
2. **Only stage your lane’s files.**
3. **History rewrites:** no casual amend/force-push.
   - Allowed: recovery on **feature** branches when the human explicitly asked or stack recovery requires it.
   - **Never** force-push `main` (or the default branch), even if a human casually asks —
     protected-branch rules win; require an explicit exceptional override naming the branch.
4. **Batch review-bot findings** into one follow-up commit when possible.
5. **Tests with the fix.** Keep default `cargo test` hermetic (no network/API keys) — AGENTS.md §2.

PR description: risk, security impact, how to verify.

---

## 5. Fast feedback loops (close the loop without full-tree waits)

| Loop | Prefer |
|------|--------|
| Lib unit/integration | `cargo test -p agent-code-lib <module>::…` |
| CLI | `cargo test -p agent-code <test_name>` |
| Compile check | `cargo check -p agent-code-lib` / `-p agent-code` |
| Format / clippy (pre-PR) | `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` |
| Flutter client | package-local analyze/test under `client/` |
| Dart client (`packages/`) | `dart pub get` / `dart analyze` / `dart test` in the package |

**Close the loop before expanding scope.**

---

## 6. What to parallelize vs serialize

**Parallelize:** independent bugs in different crates/packages; docs; isolated hypothesis worktrees;
in-lane cleanup.

**Serialize:** AGENTS.md §3 security surface; release/install/npm; cross-crate public API renames
(stack lib → consumers); CI workflow changes that gate everyone.

**Anti-patterns:** one branch across lib + CLI + Flutter; two writers one tree; commit vanity;
re-explaining security instead of linking AGENTS.md / SECURITY.md.

---

## 7. Cleanup is a product (scheduled, small, separate)

Prefer dedicated small PRs: dead code, duplicate tests, clippy/fmt-only, AGENTS.md command fixes.
Do not hide large refactors inside feature PRs.

---

## 8. Metrics (track these, not vanity)

1. **PRs merged** with CI green  
2. **Median time** idea → first green CI  
3. **Median files changed** on non-merge commits  
4. **Real** review findings encoded into AGENTS.md  

---

## 9. Standing rules for agents (checklist)

1. Declare **lane + out-of-scope paths** before writing.  
2. Prefer **stacked or sequential PRs** over a multi-domain blob.  
3. Commit **atomically** in-lane (own worktree).  
4. Verify with **scoped** cargo/dart commands; full gate before open PR.  
5. Self-check **security rules** (AGENTS.md §3 / SECURITY.md).  
6. Encode learnings into AGENTS.md — not only chat.

---

## 10. See also

- Root [`AGENTS.md`](AGENTS.md) — CI gate, security, conventions  
- [`SECURITY.md`](SECURITY.md) — security posture  
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — system shape  
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — contribution flow (including forks)  
