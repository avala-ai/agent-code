# Releasing

How to cut a new release of agent-code.

## Prerequisites

- Push access to `main`
- GitHub secrets configured:
  - `CARGO_REGISTRY_TOKEN` - crates.io publish token
  - `HOMEBREW_TAP_TOKEN` - GitHub PAT with `contents:write` on `avala-ai/homebrew-tap`
- npm publishing uses an **OIDC trusted publisher** (no `NPM_TOKEN` secret). The trusted
  publisher must be configured on npmjs.com for `@avala-ai/agent-code` (Publisher: GitHub
  Actions, repo `avala-ai/agent-code`, workflow file `release.yml`) before the tag is pushed,
  or the `publish-npm` job's OIDC exchange is rejected.

## Release naming standard

Release PRs follow the naming and body shape used by recent releases such as #244 and #249. Commit messages still use Conventional Commits, but release PR titles do not.

| Item | Format |
|------|--------|
| Branch | `release/vX.Y.Z` |
| PR title | `Release vX.Y.Z` |
| Release prep commit | `chore(release): prepare vX.Y.Z` |
| Required PR label | `run-e2e` |
| Tag | `vX.Y.Z` |

Release PRs are regular PRs, not drafts. Use checklist items in the PR body for anything still pending.

Release PR bodies use these sections, in this order:

1. `Summary`
2. `Highlights`
3. `Verification (RELEASING.md section 4)`
4. `After merge`

## Steps

### 1. Create a release branch

```bash
git checkout main && git pull
git checkout -b release/vX.Y.Z
```

### 2. Bump versions

Update the version in these files:

| File | Field |
|------|-------|
| `crates/lib/Cargo.toml` | `version = "X.Y.Z"` |
| `crates/cli/Cargo.toml` | `version = "X.Y.Z"` in `[package]` and the `agent-code-lib` path-dependency |
| `crates/eval/Cargo.toml` | `agent-code-lib = { path = "...", version = "X.Y.Z" }` path-dependency only |
| `Cargo.lock` | `version = "X.Y.Z"` for `agent-code` and `agent-code-lib` package stanzas |
| `npm/package.json` | `"version": "X.Y.Z"` |

The eval crate is `publish = false` so its own package version does not need bumping, but its `agent-code-lib` dependency pin must match. Otherwise `cargo check --all-targets` fails with `failed to select a version for the requirement agent-code-lib = "^OLD"`.

After editing the manifests, run a Cargo command such as `cargo check --all-targets` or `cargo metadata` and confirm `Cargo.lock` updated. If only workspace package versions changed, the lockfile diff should only touch the `agent-code` and `agent-code-lib` version lines.

### 3. Stamp the CHANGELOG

In `CHANGELOG.md`:

1. Leave `## [Unreleased]` with `*No changes yet.*`
2. Add a new section below it: `## [X.Y.Z] - YYYY-MM-DD`
3. Summarize the merged PRs since the previous release under Keep-a-Changelog headings (`Added`, `Changed`, `Fixed`, etc.)
4. Update the comparison links at the bottom:

```markdown
[Unreleased]: https://github.com/avala-ai/agent-code/compare/vX.Y.Z...HEAD
[X.Y.Z]: https://github.com/avala-ai/agent-code/compare/vPREVIOUS...vX.Y.Z
```

### 4. Verify

Run the full CI gate locally before pushing when the local environment supports it:

```bash
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The release PR must also pass GitHub Actions CI and the `run-e2e` label-triggered workflow before merge.

### 5. Commit and push

```bash
git add -A
git commit -m "chore(release): prepare vX.Y.Z"
git push -u origin release/vX.Y.Z
```

### 6. Open the release PR

Create a regular PR and add `run-e2e` immediately:

```bash
gh pr create --title "Release vX.Y.Z" --body-file /tmp/release-pr.md
gh pr edit --add-label run-e2e
```

Use this body skeleton:

```markdown
## Summary

Cuts **vX.Y.Z**. Bumps `crates/lib`, `crates/cli`, `crates/eval` (path-dep only), `Cargo.lock`, and `npm/package.json` from OLD -> X.Y.Z. Stamps the CHANGELOG with the changes merged since the vOLD release.

## Highlights

**Feature or fix group** (#123, #124):
- User-facing summary.
- Important compatibility, safety, or migration notes.

Full changelog is in `CHANGELOG.md` under the `[X.Y.Z]` heading.

## Verification (RELEASING.md section 4)

- [x] `run-e2e` label added
- [ ] `cargo check --all-targets`
- [ ] `cargo test --all-targets`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --all -- --check`
- [ ] GitHub Actions CI passed
- [ ] `run-e2e` workflow passed

## After merge

Follow RELEASING.md section 7: tag `vX.Y.Z` on main and push. Release automation handles Linux/macOS/Windows binaries, crates.io publish, npm publish, Docker image publish, and Homebrew tap update.
```

Merge after the changelog, lockfile, CI, E2E, and approval are complete.

### 7. Tag and push

```bash
git checkout main && git pull
git tag vX.Y.Z
git push origin vX.Y.Z
```

### 8. What happens automatically

The tag triggers these workflows:

| Workflow | What it does |
|----------|-------------|
| **Release** (`release.yml`) | Builds Linux/macOS/Windows binaries, creates GitHub Release, publishes to crates.io |
| **Release** (`release.yml`) | Updates `avala-ai/homebrew-tap` formula with new version + SHA256 checksums |
| **Docker** (`docker.yml`) | Builds and pushes `ghcr.io/avala-ai/agent-code:X.Y.Z` + `:latest` |
| **Release** (`release.yml`) | `publish-npm` job publishes `@avala-ai/agent-code` to npm via OIDC trusted publishing + provenance |
| **CI** (`ci.yml`) | Standard checks on the main branch push |

### 9. Verify the release

```bash
# GitHub Release created?
gh release view vX.Y.Z

# Binaries uploaded? (5 artifacts)
gh release view vX.Y.Z --json assets --jq '.assets[].name'

# crates.io published?
cargo search agent-code

# Docker image pushed?
docker pull ghcr.io/avala-ai/agent-code:X.Y.Z

# npm published?
npm view agent-code version

# Homebrew tap updated?
gh api repos/avala-ai/homebrew-tap/contents/Formula/agent-code.rb --jq '.content' | base64 -d | grep version
```

## Version numbering

We use [Semantic Versioning](https://semver.org/):

- **Patch** (0.11.1): bug fixes, doc updates, no new features
- **Minor** (0.12.0): new features, backward compatible
- **Major** (1.0.0): breaking changes (reserved for v1.0 milestone)

## Release branches

Release branches (`release/vX.Y.Z`) are preserved permanently. They serve as snapshots for hotfixes:

```bash
# Hotfix on an old release
git checkout release/v0.11.0
git checkout -b hotfix/fix-something
# ... fix, test, commit ...
git tag v0.11.1
git push origin v0.11.1
# Also cherry-pick the fix to main
```

## Rollback

If a release has a critical issue:

1. **Don't delete the tag** - downstream users may have pinned it
2. Cut a patch release (vX.Y.1) with the fix
3. If the npm package is broken: `npm unpublish agent-code@X.Y.Z` (within 72 hours)
