# SPEED.md — Throughput without quality debt

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

Read this when you are about to open a multi-file change, spawn parallel agents, or feel
the PR is growing into a review-hostile blob. Skip it for one-line fixes.

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

| Lane | Owns (write) | Notes |
|------|----------------|-------|
| **lib** | `crates/lib/` | Engine: providers, tools, query loop, permissions |
| **cli** | `crates/cli/` | Binary, TUI, slash commands — depends on lib |
| **eval** | `crates/eval/`, `evals/` | Harness + fixtures; keep default tests hermetic |
| **client** | `client/` | Flutter desktop/web UI for `agent --serve` |
| **ts-client** | `packages/` | TypeScript client library |
| **npm / install** | `npm/`, `install.sh` | Release/install path — serialize with care |
| **docs** | `docs/`, root prose (`AGENTS.md`, this file, CHANGELOG) | — |
| **scripts / ci** | `scripts/`, `.github/workflows/` | Prefer dedicated PRs |
| **agent harness** | `.claude/` | lead only when multi-agent |

Hard rules:

1. **Two writers never share a file.** Split by directory first; if a shared file is
   unavoidable, **serialize** edits through one owner.
2. **Contract changes stack, they do not race.** `agent-code-lib` API → CLI / client consumers
   is a **stack** (lib lands first), not parallel PRs against `main` that both edit the surface.
3. **One `git worktree` per concurrent lane (required).** Do not run two writing agents in
   the same working tree. Shared dirt causes silent overwrites, false “fixes,” and unreviewable
   mixed diffs. **New** parallel work should use `git worktree` from a single primary clone
   (see below).
4. **Lead owns integration.** When multiple lanes land, one person/agent rebases the stack,
   runs the CI gate commands, and writes the PR narrative.

### `git worktree` — default for parallel / multi-agent work

From the primary clone (example paths; pick a sibling dir you control):

```bash
# One lane = one branch = one worktree, always branched from current origin/main
git fetch origin main
git worktree add -b fix/lib-<slug>    ../agent-code-wt-lib    origin/main
git worktree add -b fix/client-<slug> ../agent-code-wt-client origin/main

# Agent A cwd: ../agent-code-wt-lib     (writes crates/lib/ only)
# Agent B cwd: ../agent-code-wt-client  (writes client/ only)

# After PR merge or abandon:
git worktree remove ../agent-code-wt-lib
git branch -d fix/lib-<slug>   # if fully merged
```

Rules of use:

- **Branch from `origin/main`** (or the stack base), never from a dirty feature branch,
  unless you are intentionally stacking.
- **Agent cwd is the worktree root** for that lane — not the primary tree.
- **Do not** `git worktree add` into a path that already has uncommitted work.
- List with `git worktree list`; prune stale entries with `git worktree prune` after deletes.

Single-lane, single-agent work may stay in the primary tree. The moment a second writer starts,
**spin a worktree** — do not “just use another pane” on the same checkout.

### Prompt shape for a parallel agent

Keep prompts short; put judgment in the PR body later:

```text
Lane: lib only. Cwd: <worktree path>. Write only under crates/lib/.
Goal: <one sentence>.
Out of scope: crates/cli, client/, packages/.
Done when: cargo test -p agent-code-lib <relevant> green + AGENTS.md security self-check.
Commit atomically as you go (see §4). Do not open the PR unless asked.
```

---

## 4. Commit rhythm inside a PR (atomic, not ceremonial)

Agents **should** commit as they go. Humans should too. The PR is the review unit; commits
are the undo/log unit.

1. **Atomic commits** — one concern per commit. Prefer Conventional Commit subjects when
   natural: `fix(lib): …`, `test(cli): …`, `docs: …`. Put the long reasoning in the
   **commit body or PR body**, not a 40-line subject.
2. **Only stage your lane’s files.** Other agents’ dirt is not yours to “helpfully” fix
   unless you own that lane.
3. **Do not amend** shared history or force-push unless the human asked.
4. **Batch review-bot findings** into one follow-up commit when possible
   (`fix: address review — <theme>`), not five serial micro-pushes for five nits.
5. **Tests with the fix.** Same PR (same commit when small). Keep default `cargo test`
   hermetic (no network/API keys) — see AGENTS.md §2.

PR description stays the place for narrative: risk, security impact, how to verify. That
quality bar does **not** require a single sprawling commit.

---

## 5. Fast feedback loops (close the loop without full-tree waits)

During the loop, run **scoped** commands on **touched crates** only. Full CI gate before push.

| Loop | Prefer |
|------|--------|
| Lib unit/integration | `cargo test -p agent-code-lib <module>::…` |
| CLI | `cargo test -p agent-code <test_name>` |
| Compile check | `cargo check -p agent-code-lib` / `-p agent-code` |
| Format / clippy (pre-PR) | `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` |
| Flutter client | package-local analyze/test commands under `client/` |

**Close the loop before expanding scope.** If verification is red, fix or shrink — do not
open a second parallel feature on a red base.

---

## 6. What to parallelize vs serialize

**Parallelize (high ROI):**

- Independent bugs in different crates/packages
- Tests / fixtures while implementation settles (if files do not overlap)
- Docs and changelog for a finished change
- Hypothesis debugging on isolated worktrees
- Cleanup that cannot conflict: dead code in one crate, duplicate tests

**Serialize (quality / safety):**

- Anything under AGENTS.md §3 security rules (permissions, sandbox, secrets, protected paths)
- Release / install / npm publish paths
- Cross-crate public API renames (stack: lib → consumers)
- CI workflow changes that gate everyone

**Anti-patterns (slow and low quality):**

- One agent “finishes the product” across lib + CLI + Flutter in one branch
- Parallel writers in the same directory “to go faster”
- Optimizing for commit count or merging before the CI gate is green
- Re-explaining security rules in every prompt instead of linking AGENTS.md / SECURITY.md

---

## 7. Cleanup is a product (scheduled, small, separate)

Debt paydown keeps agents fast later. Prefer **dedicated small PRs**:

- dead code / unused exports
- duplicate tests
- clippy/fmt-only fixes
- AGENTS.md fixes when a command was wrong

Do **not** hide large refactors inside feature PRs. Opportunistic cleanup is OK only when it
is high-confidence, in-lane, and does not obscure the feature diff.

---

## 8. Metrics (track these, not vanity)

Per person or team, weekly:

1. **PRs merged** with CI green (primary)
2. **Median time** idea → first green CI on the PR
3. **Median files changed** on non-merge commits (keep low; spikes need a stack)
4. **Bot/review findings that are real** vs noise (encode the real ones in AGENTS.md)

If (1) and (2) improve while security incidents stay flat, the system is working.
If commit count rises and incident rate rises, stop and re-read §2 and AGENTS.md §3.

---

## 9. Standing rules for agents (checklist)

1. Declare **lane + out-of-scope paths** before writing.
2. Prefer **stacked or sequential PRs** over a multi-domain blob.
3. Commit **atomically** in-lane; leave other agents’ files alone.
4. Verify with **scoped** cargo commands; full gate before open PR.
5. Self-check **security rules** (AGENTS.md §3 / SECURITY.md).
6. When you learn something the next agent will need, write it into AGENTS.md — do not only
   put it in chat.

---

## 10. See also

- Root [`AGENTS.md`](AGENTS.md) — CI gate, security, conventions
- [`SECURITY.md`](SECURITY.md) — security posture
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — system shape
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — contribution flow
