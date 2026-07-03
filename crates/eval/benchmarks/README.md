# Security-scan CVE-recall benchmark

Measures how many real, published vulnerabilities `agent security-scan` finds.

## Methodology

- Each case is a real CVE, pinned to the commit **immediately before** its fix,
  so the flaw still exists in the tree.
- Cases should be published **after** the scanning model's training cutoff, so
  the answer was not memorized during pre-training. Record the advisory date in
  `published`.
- Score is **recall**: the fraction of cases where at least one scan finding
  describes the target vulnerability. False positives are ignored here (a
  precision benchmark is separate).

## Manifest format (`security-scan-cves.json`)

A JSON array of cases:

```json
[
  {
    "id": "CVE-2025-XXXXX",
    "cwe": "CWE-89",
    "language": "python",
    "repo": "https://github.com/org/repo",
    "commit": "<sha of the commit BEFORE the fix>",
    "file": "path/to/vulnerable_file.py",
    "description": "SQL injection in the user-lookup endpoint",
    "published": "2025-11-03"
  }
]
```

Target ~50 cases across ~14 languages (Go, Rust, Python, Ruby, Java, C#, JS, C,
Swift, Dart, Elixir, …), spanning RCE, SQLi, path traversal, SSRF, auth bypass,
memory-safety, and DoS.

## Running

```bash
cargo build --release -p agent-code
# Deterministic grader (free): a finding on the target file with a matching CWE.
cargo run -p agent-code-eval -- --bench crates/eval/benchmarks/security-scan-cves.json \
  --grader heuristic --model gpt-5.5 --results bench-report.json

# Semantic grader (costs tokens): an LLM judges each case for a match.
cargo run -p agent-code-eval -- --bench crates/eval/benchmarks/security-scan-cves.json \
  --grader llm --model gpt-5.5
```

The harness clones each repo at its pre-patch commit, runs the scan, grades
recall, and reports recall + `$`/case, with a per-language breakdown and the
list of missed cases.
