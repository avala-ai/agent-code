# Releasing

How to cut a new release of agent-code.

## Prerequisites

- Push access to `main`
- GitHub secrets configured:
  - `CARGO_REGISTRY_TOKEN` — crates.io publish token
  - `NPM_TOKEN` — npm publish token (Automation type)
  - `HOMEBREW_TAP_TOKEN` — GitHub PAT with `contents:write` on `avala-ai/homebrew-tap`

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
| `crates/cli/Cargo.toml` | `version = "X.Y.Z"` (appears twice — package and the `agent-code-lib` path-dependency) |
| `crates/eval/Cargo.toml` | `agent-code-lib = { path = "...", version = "X.Y.Z" }` (path-dependency, not the eval package version) |
| `npm/package.json` | `"version": "X.Y.Z"` |

The eval crate is `publish = false` so its own package version doesn't need bumping, but its `agent-code-lib` dependency pin must match — otherwise `cargo check --all-targets` fails with `failed to select a version for the requirement agent-code-lib = "^OLD"`.

### 3. Stamp the CHANGELOG

In `CHANGELOG.md`:

1. Replace `## [Unreleased]` content with `*No changes yet.*`
2. Add a new section: `## [X.Y.Z] - YYYY-MM-DD`
3. Move the unreleased items into the new section
4. Update the comparison links at the bottom:

```markdown
[Unreleased]: https://github.com/avala-ai/agent-code/compare/vX.Y.Z...HEAD
[X.Y.Z]: https://github.com/avala-ai/agent-code/compare/vPREVIOUS...vX.Y.Z
```

### 4. Verify

```bash
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

### 5. Commit and push

```bash
git add -A
git commit -m "Bump version to X.Y.Z"
git push -u origin release/vX.Y.Z
```

### 6. Open and merge the release PR

```bash
gh pr create --title "Release vX.Y.Z" --body "Version bump + changelog stamp"
```

Merge after CI passes.

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
| **npm** (`npm.yml`) | Publishes `agent-code` to npm (triggered by the GitHub Release) |
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

1. **Don't delete the tag** — downstream users may have pinned it
2. Cut a patch release (vX.Y.1) with the fix
3. If the npm package is broken: `npm unpublish agent-code@X.Y.Z` (within 72 hours)
