# Design proposal: runtime verification stage for AMR security-scan

Status: **proposal / awaiting review** — not yet implemented.
Author: Emal. Context: the CVE-recall benchmark (#366) shows the scanner finds
*real, related* vulnerabilities but does not always pinpoint and confirm the
*specific* exploitable bug. A runtime-verify stage is the highest-leverage way
to raise precision-weighted recall and to justify severity claims with
evidence rather than model assertion.

## Where it fits in the pipeline

```
plan → shard → batch → MAP (find) → REDUCE (dedupe + chain)  →  ★ VERIFY (new)  → gate
```

VERIFY runs after REDUCE, over the prioritized finding/chain list. It is
**optional and off by default** (a `--verify` flag / `ScanConfig.verify`),
because it executes code and costs time. The gate consumes verified verdicts
when present and falls back to the current behavior when absent.

## What a verifier does per finding

Each candidate finding gets one confined verifier agent whose job is to move it
from *asserted* to *demonstrated*:

1. **Reproduce or refute.** Construct the smallest artifact that exercises the
   sink with attacker-controlled input: a unit-style harness, a crafted input
   file, or a localhost request against the app booted in the sandbox.
2. **Observe, don't trust.** A finding is `Confirmed` only on an observed
   signal — a raised exception at the sink, a file read outside root, an
   out-of-bounds/ASan abort, a reflected marker in the response, a non-empty
   deserialization callback. Otherwise `Unconfirmed` (kept, downgraded) or
   `Refuted` (dropped).
3. **Emit an evidence bundle**: the repro input, the command run, the observed
   output, and a one-line why-it-proves-exploitability.

## Sandbox

Reuse the existing permission/confinement model, tightened for execution:

- **Network**: denied by default; a loopback-only allowance when the finding
  class needs a booted server (SSRF, path traversal via HTTP, XSS).
- **Filesystem**: writes confined to a scratch overlay; the repo is mounted
  read-only. Canary files planted outside the intended root detect traversal.
- **Compute**: hard wall-clock + memory + process caps; verifiers run one at a
  time or with a small bounded pool.
- **Isolation**: a throwaway container/VM per scan when available; degrade to a
  bwrap-style sandbox otherwise. If no sandbox is available, VERIFY is skipped
  with a logged notice (never silently downgraded to "trust the model").

### Class-specific harnesses (deterministic oracles, not LLM opinion)

| CWE class | Oracle |
|---|---|
| Path traversal (CWE-22) | request `..%2f`-style path; confirm a planted canary outside root is read |
| Command injection (CWE-77/78) | inject a token that writes a sentinel file / sleeps; confirm side effect |
| Deserialization (CWE-502) | feed a gadget that triggers an observable callback in the sandbox |
| SSRF (CWE-918) | point the sink at a loopback trap; confirm the trap is hit |
| SQL injection (CWE-89) | boolean/time-based differential against a scratch DB |
| Memory safety (CWE-125/787/416) | build the target under ASan/UBSan; confirm the abort on the crafted input |
| ReDoS (CWE-1333) | measure super-linear time growth across input sizes |

Classes without a cheap deterministic oracle (auth logic, info leak) get a
weaker "reasoned repro" verdict, clearly marked as lower-confidence.

## Data model

```rust
enum Verdict { Confirmed, Unconfirmed, Refuted, NotAttempted }

struct Verification {
    verdict: Verdict,
    evidence: Option<String>,   // repro input + observed signal
    method: &'static str,       // which oracle
    cost_usd: f64,
}
```

Attach `Verification` to each `Finding`. REDUCE severities are then adjusted:
`Confirmed` holds or raises severity; `Refuted` drops the finding; `Unconfirmed`
keeps it but caps severity and flags it in the report.

## Effect on the benchmark

The CVE-recall harness can score two numbers:
- **found** (current): a finding names the target file+class.
- **confirmed**: the same, *and* VERIFY reproduced it.

`confirmed` is the honest, high-precision metric and the one worth publishing —
it is also what Cognition's sandbox-validation approach reports against.

## Rollout

1. Land the data model + a no-op `NotAttempted` verifier (plumbing only).
2. Add the two cheapest deterministic oracles (path traversal, command
   injection) behind `--verify`; wire the benchmark's `confirmed` column.
3. Add remaining oracles incrementally; each ships with a fixture test.
4. Only after `confirmed` recall is competitive do we put a number in the blog
   (per the no-live-weakness constraint).

## Open questions for review

- Container runtime we can assume on the fleet vs. bwrap fallback?
- Per-scan verify budget (wall-clock / $) ceiling and default?
- Do we gate CI on `confirmed` for a small fixture corpus, or keep VERIFY
  out of the default test path (it needs a sandbox)?
