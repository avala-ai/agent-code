# agent-code modern TUI

Docs for the fullscreen pager track (`--tui modern` / `ui.tui = "modern"`).

| Doc | Purpose |
|---|---|
| [AUDIT.md](./AUDIT.md) | M0 reality map: modules, StreamSink gaps, version stance |
| [SUPPORT.md](./SUPPORT.md) | Terminal support matrix (filled in M10) |
| [ACCEPTANCE.md](./ACCEPTANCE.md) | Appendix C product-bar checklist (filled in M10) |

## Design sources (repo)

- `docs/design/tui-modern-plan-of-attack.md` — execution plan  
- `docs/design/tui-modern-overhaul.md` — design sketch  
- `docs/design/harness-comparison-2026-07.md` — anonymized peer matrix  
- `docs/design/reference-pager-binary-forensics.md` — public-binary architecture notes  

## Issue tree

Epic **#385**. Milestones M0–M10: **#386–#396**. Agent-ready leaves under M0: **#397–#401**, docs **#408**.

## Parallel ownership (PR #415 handoff)

| Track | Paths |
|---|---|
| **M0 engine** | `crates/lib/**`, `docs/tui/**` |
| **M1+ UI** | `crates/cli/src/ui/modern/**` only |

Shared interface: additive `StreamSink` in `crates/lib/src/query/`.
