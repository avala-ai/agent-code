# Coding-agent harness comparison (July 2026)

**Subjects:** agent-code and four peer terminal coding agents (anonymized)  
**Method:** local repo stats (agent-code), shallow clones of open peers, public package/binary artifacts for closed peers.  
**Not:** official closed-source trees; no third-party binaries committed here.

Peers are labeled by **technical profile only** (no product branding):

| Label | Profile |
|---|---|
| **Peer-CR** | Closed-source Rust single-binary pager |
| **Peer-OR** | Open-source Apache Rust monorepo (~100 crates) |
| **Peer-TS** | Open-source MIT TypeScript/Bun multi-surface agent |
| **Peer-CN** | Closed-source native (compiled TS/JS) agent binary |

---

## 1. At a glance

| | **agent-code** | **Peer-CR** | **Peer-OR** | **Peer-TS** | **Peer-CN** |
|---|---|---|---|---|---|
| **Version sampled** | 0.25.3 | ~0.2.9x | main / ~0.14x line | ~1.17.x | ~2.1.2xx |
| **License** | MIT | Proprietary | Apache-2.0 | MIT | Proprietary |
| **Open source?** | Yes | No | Yes | Yes | No |
| **Primary language** | **Rust** | **Rust** | **Rust** | **TypeScript (Bun)** | **TS → native ELF** |
| **Ship form** | cargo/brew/npm/docker | single static binary (~120–155 MB) | multi-crate | bun monorepo / desktop | npm wrapper + platform binary (~247 MB linux) |
| **Binary size (linux x64)** | release ~13 MB class | **~153 MB** static-pie | multi-crate (varies) | TS/bundled | **~247 MB** dyn ELF |

---

## 2. Implementation stack (evidence-based)

| Layer | agent-code | Peer-CR | Peer-OR | Peer-TS | Peer-CN |
|---|---|---|---|---|---|
| **Runtime** | Rust + tokio | Rust + tokio | Rust + tokio (+ heavy build) | Bun | Bun-compiled native |
| **Interactive UI** | rustyline classic + experimental **ratatui modern** | **ratatui + crossterm** + custom widgets | ratatui TUI crate | OpenTUI + SolidJS | Fullscreen TUI (framework strings in binary) |
| **UI model** | Classic print stream; modern App + sink | Typed **scrollback blocks** + event loop | Widget-heavy chat/exec cells | Session + Solid components | Fullscreen transcript + agents view |
| **Workspace** | 3 crates: lib / cli / eval | ~47 first-party crates (binary strings) | **~100** crates | Many packages (cli, desktop, sdk, web) | Thin installer + fat binary |
| **LOC order (approx)** | ~95k Rust | closed | ~310k `.rs` | ~250k TS/TSX | fat binary |

See also `docs/design/reference-pager-binary-forensics.md` (Peer-CR).

---

## 3. Tools / capabilities matrix

Legend: ✅ strong · ◐ partial · ❌ weak/absent · 🔒 product-gated

| Capability | agent-code | Peer-CR | Peer-OR | Peer-TS | Peer-CN |
|---|:---:|:---:|:---:|:---:|:---:|
| File read/write/edit | ✅ | ✅ | ✅ | ✅ | ✅ |
| Grep / glob | ✅ | ✅ | ✅ | ✅ | ✅ |
| Shell | ✅ | ✅ | ✅ sandboxed | ✅ | ✅ |
| **apply_patch** | ❌ | ◐ | ✅ first-class | ✅ | ❌ (file edit) |
| Web fetch/search | ✅ | ✅ | ◐ | ✅ | ✅ |
| Plan mode | ✅ | ✅ | ◐ | ✅ Build/Plan | ✅ |
| Typed subagents | ✅ | ✅ + dashboard | ✅ | ✅ | ✅ + bg default |
| Background agents | ✅ | ✅ | ✅ | ✅ | ✅ **process UI** |
| MCP | ✅ | ✅ | ✅ client+server | ✅ | ✅ |
| Skills / plugins | ✅ | ✅ | ✅ | ✅ | ✅ |
| Cron / schedule | ✅ | ✅ | ◐ | ◐ | ✅ |
| Worktrees | ✅ | ✅ | ✅ | ✅ | ✅ |
| OS sandbox | ✅ seatbelt/bwrap | ✅ | ✅ **deepest** | ◐ permissions-first | ✅ |
| ACP / IDE | ✅ | ✅ | ✅ app-server | ✅ | ✅ |
| Multi-provider | ✅ **14+** | vendor-centric | vendor-centric + local | ✅ **very broad** | vendor + cloud hosts |
| Headless / CI | `-p`, `--serve` | `-p` | print / exec | headless | `-p` stream-json |
| Desktop GUI | Flutter client | product desktop | app surface | **Desktop package** | desktop / remote |
| Security scan product | ✅ **AMR security-scan** | ❌ | ❌ | ❌ | skill-level review |
| Eval harness | ✅ crates/eval | 🔒 | heavy tests | product | 🔒 |
| Telemetry default | **opt-in only** | product | product | privacy-first claim | product |

### Tool counts (order-of-magnitude)

| Product | Built-in tools (approx) | Source |
|---|---|---|
| agent-code | ~32 | `crates/lib/src/tools/*` |
| Peer-CN | ~40 named tool schemas | public SDK typings |
| Peer-TS | ~20 core (+ MCP) | tool package tree |
| Peer-OR | shell + apply_patch + skills/MCP | core crates |
| Peer-CR | full suite | product + tools crate in binary |

---

## 4. TUI / UX comparison

| UX dimension | agent-code | Peer-CR | Peer-OR | Peer-TS | Peer-CN |
|---|---|---|---|---|---|
| **Primary interactive model** | Line REPL (classic) | Fullscreen **pager** | Fullscreen TUI | Fullscreen OpenTUI | Fullscreen TUI |
| **Alt-screen modern path** | Scaffold (`--tui modern`) | Production default | Production | Production | Production |
| **Mode cycle** | Partial | ✅ | ◐ | ✅ | ✅ |
| **Scrollback as document** | Classic no; modern WIP | ✅ typed blocks | ✅ widgets | ✅ | ✅ |
| **Tool cards / grouping** | Basic | ✅ | exec cells | yes | focus/grouping |
| **Agents dashboard** | `/tasks` | dashboard | multi-agent UI | subagent sessions | process UI |
| **Prompt queue** | ◐ | ✅ | ◐ | ◐ | ✅ |
| **Mouse / selection** | Classic no | ✅ advanced | yes | yes | yes |

**Honest ranking of interactive polish (product maturity):**  
1. **Peer-CR** (pager craft) · 2. **Peer-CN** (agents process UX) · 3. **Peer-OR** (solid Rust TUI) · 4. **Peer-TS** (fast multi-surface) · 5. **agent-code** (engine-strong, TUI lagging; modern path started).

---

## 5. Security & permissions

| | agent-code | Peer-CR | Peer-OR | Peer-TS | Peer-CN |
|---|---|---|---|---|---|
| Permission modes | ask/allow/deny/plan/accept_edits | rich product modes | approvals + sandbox | per-tool | manual/auto/bypass/plan |
| Protected paths | `.git`/node_modules hard block | trust + sandbox | sandbox + execpolicy | external_directory | sandbox + classifiers |
| Destructive shell heuristics | ✅ | ✅ | ✅ | ◐ | ✅ |
| Process sandbox | bwrap/seatbelt | sandbox profiles | **deepest OS isolation** | lighter | sandbox + credential mask |
| Bypass lock | `disable_bypass_permissions` | product policy | enterprise | config | org policy |

**Peer-OR** leads on OS isolation depth. **agent-code** is unusually strong for size (protected dirs + sandbox + no default telemetry). Closed peers lead on *UX of* permission (classifiers, never-allow, visible modes).

---

## 6. Extensibility

| | agent-code | Peer-CR | Peer-OR | Peer-TS | Peer-CN |
|---|---|---|---|---|---|
| Skills / plugins / hooks | ✅ | ✅ | ✅ | ✅ | ✅ |
| Project rules | AGENTS.md | AGENTS.md | AGENTS.md | AGENTS.md | vendor rules files |
| Custom agents | `.agent/agents/*.md` | product + files | agent defs | primary/sub md | vendor agent dirs |
| MCP as server | bridge/serve | yes | dedicated crate | server package | product |
| Embed / protocol | lib crate | closed | app-server | strong SDKs | stream-json SDK |

---

## 7. Distribution & business model

| | agent-code | Peer-CR | Peer-OR | Peer-TS | Peer-CN |
|---|---|---|---|---|---|
| Free OSS use | ✅ | ❌ sub | ✅ OSS (API costs) | ✅ | ❌ sub/API |
| Monetization | OSS | subscription | vendor sub/API | OSS + cloud options | vendor sub |
| Install friction | medium | one curl binary | curl / npm / brew | curl / many pkg mgrs | npm + native |
| Ecosystem gravity | tiny | growing | huge | largest OSS stars | huge |

---

## 8. Strengths / weaknesses (strategic)

### agent-code
**Strengths:** multi-provider; clean MIT Rust engine; security-scan AMR; schedule/cron; embeddable lib; subscription OAuth paths; no default telemetry.  
**Weaknesses:** TUI still classic-first; tiny community; apply_patch missing; multi-agent UX shallow vs peers; brand gravity.

### Peer-CR
**Strengths:** best-in-class **pager TUI** (blocks, selection, modes); Rust single binary; product polish velocity.  
**Weaknesses:** closed source; subscription lock-in; vendor-centric models; cannot fork or embed.

### Peer-OR
**Strengths:** open Apache Rust monorepo; **apply_patch**; deepest sandbox; IDE/app-server; enterprise path.  
**Weaknesses:** provider funnel toward primary vendor; monorepo complexity; pager craft often rated below Peer-CR.

### Peer-TS
**Strengths:** largest OSS community; multi-surface (TUI/desktop/web); Build/Plan agents; broad providers; MIT.  
**Weaknesses:** TS/Bun memory/perf vs Rust; lighter OS sandbox; quality variance with velocity.

### Peer-CN
**Strengths:** agents-as-process UI (bg daemon, attach, workflows); product completeness; enterprise.  
**Weaknesses:** proprietary; vendor-centric; large binary; telemetry/product coupling.

---

## 9. Where agent-code should invest (from this matrix)

1. **Finish modern TUI** (ratatui pager: blocks, modes, tool cards, queue) — closes the largest gap vs Peer-CR/CN.  
2. **Agents/tasks panel** inspired by peer process UIs (state machine UX).  
3. **apply_patch** (Peer-OR/TS pattern) for multi-hunk reliability — **engine track**, not TUI.  
4. **Keep differentiators:** multi-provider, security-scan, cron, MIT, no phone-home.  
5. **Don’t chase:** peer media/voice/remote-control product surfaces unless strategy changes.

---

## 10. Artifact locations (ephemeral analysis)

| Artifact | Path |
|---|---|
| agent-code | local workspace |
| Peer-CR binaries | `/tmp/…` (do not commit) |
| Peer-CR forensics notes | `docs/design/reference-pager-binary-forensics.md` |
| Peer-OR / Peer-TS clones | `/tmp/agent-harness-compare/…` (do not commit) |
| Peer-CN package + binary | `/tmp/agent-harness-compare/…` (do not commit) |

Do not commit third-party binaries into agent-code.

---

## 11. Summary table (one line each)

| Product | One-liner |
|---|---|
| **agent-code** | Open MIT Rust **engine** with multi-provider + security tools; TUI catching up. |
| **Peer-CR** | Closed Rust **ratatui pager** — best terminal craft; subscription. |
| **Peer-OR** | Open Rust monorepo — sandbox + apply_patch + vendor ecosystem. |
| **Peer-TS** | Open TS/Bun monorepo — community + multi-surface + provider breadth. |
| **Peer-CN** | Closed native agent — richest multi-agent process UI. |
