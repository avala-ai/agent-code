//! Render a [`ScanReport`] as JSON (machine) or Markdown (human).

use std::fmt::Write as _;

use super::types::{Finding, ScanReport, Severity};

/// Pretty-printed JSON, suitable for `--output json` or CI ingestion.
pub fn to_json(report: &ScanReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
}

/// Human-readable Markdown summary.
pub fn to_markdown(report: &ScanReport) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# Security scan — {} findings", report.findings.len());
    let _ = writeln!(md);
    let _ = writeln!(
        md,
        "Profile `{}` over `{}`{}.",
        report.profile,
        report.repo_root,
        if report.incremental {
            format!(
                " (incremental since {})",
                report.base_commit.as_deref().unwrap_or("?")
            )
        } else {
            String::new()
        }
    );
    let _ = writeln!(md);
    let _ = writeln!(md, "| Metric | Value |");
    let _ = writeln!(md, "|---|---|");
    let _ = writeln!(
        md,
        "| Files scanned (had signals) | {} |",
        report.scanned_files
    );
    let _ = writeln!(
        md,
        "| Files dropped (no signals) | {} |",
        report.dropped_files
    );
    let _ = writeln!(md, "| Signals | {} |", report.signals);
    let _ = writeln!(md, "| Shards (MAP workers) | {} |", report.shards);
    if report.worker_failures > 0 {
        let _ = writeln!(
            md,
            "| Workers failed (coverage gap) | {} |",
            report.worker_failures
        );
    }
    let _ = writeln!(
        md,
        "| P0 / P1 / P2 | {} / {} / {} |",
        count(&report.findings, Severity::P0),
        count(&report.findings, Severity::P1),
        count(&report.findings, Severity::P2)
    );
    let _ = writeln!(md, "| Cost | ${:.4} |", report.cost_usd);
    let _ = writeln!(
        md,
        "| Tokens (in/out) | {} / {} |",
        report.tokens.input, report.tokens.output
    );
    let _ = writeln!(md, "| Duration | {} ms |", report.duration_ms);
    let _ = writeln!(md);

    if !report.chains.is_empty() {
        let _ = writeln!(md, "## Attack chains\n");
        for c in &report.chains {
            let _ = writeln!(
                md,
                "- **[{}] {}** — {} _(members: {})_",
                c.combined_severity,
                c.chain_id,
                c.narrative,
                c.member_finding_ids.join(", ")
            );
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "## Findings\n");
    if report.findings.is_empty() {
        let _ = writeln!(md, "_No findings._");
    } else {
        for f in &report.findings {
            let loc = match f.line_range {
                Some((a, b)) if a == b => format!("{}:{}", f.file, a),
                Some((a, b)) => format!("{}:{}-{}", f.file, a, b),
                None => f.file.clone(),
            };
            let cwe = f
                .cwe
                .as_deref()
                .map(|c| format!(" ({c})"))
                .unwrap_or_default();
            let _ = writeln!(md, "### [{}] {}{}\n", f.severity, f.title, cwe);
            let _ = writeln!(md, "- **Location:** `{loc}`");
            let _ = writeln!(md, "- **Confidence:** {:.2}", f.confidence);
            if !f.root_cause.is_empty() {
                let _ = writeln!(md, "- **Root cause:** {}", f.root_cause);
            }
            if !f.exploit_preconditions.is_empty() {
                let _ = writeln!(md, "- **Preconditions:** {}", f.exploit_preconditions);
            }
            if !f.evidence.is_empty() {
                let _ = writeln!(md, "- **Evidence:** {}", f.evidence);
            }
            let _ = writeln!(md);
        }
    }

    md
}

fn count(findings: &[Finding], sev: Severity) -> usize {
    findings.iter().filter(|f| f.severity == sev).count()
}

#[cfg(test)]
mod tests {
    use super::super::types::{AttackChain, TokenTotals};
    use super::*;

    fn sample() -> ScanReport {
        ScanReport {
            profile: "security".into(),
            repo_root: "/repo".into(),
            scanned_files: 2,
            dropped_files: 40,
            signals: 5,
            shards: 2,
            worker_failures: 0,
            findings: vec![Finding {
                id: "f1".into(),
                cwe: Some("CWE-78".into()),
                file: "app.py".into(),
                line_range: Some((10, 10)),
                severity: Severity::P0,
                confidence: 0.9,
                title: "OS command injection".into(),
                root_cause: "user input flows to os.system".into(),
                exploit_preconditions: "reachable endpoint".into(),
                evidence: "os.system('ping ' + host)".into(),
                selector_id: Some("rce.py_dangerous_call".into()),
                shard_id: Some("shard-0000".into()),
            }],
            chains: vec![AttackChain {
                chain_id: "c1".into(),
                member_finding_ids: vec!["f1".into()],
                combined_severity: Severity::P0,
                narrative: "leak + rce".into(),
                combined_preconditions: String::new(),
            }],
            cost_usd: 1.2345,
            tokens: TokenTotals {
                input: 100,
                output: 50,
            },
            duration_ms: 4200,
            incremental: false,
            base_commit: None,
        }
    }

    #[test]
    fn json_roundtrips() {
        let r = sample();
        let json = to_json(&r);
        let back: ScanReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.findings.len(), 1);
        assert_eq!(back.findings[0].severity, Severity::P0);
    }

    #[test]
    fn markdown_has_key_sections() {
        let md = to_markdown(&sample());
        assert!(md.contains("# Security scan"));
        assert!(md.contains("## Attack chains"));
        assert!(md.contains("## Findings"));
        assert!(md.contains("OS command injection"));
        assert!(md.contains("`app.py:10`"));
        assert!(md.contains("CWE-78"));
    }

    #[test]
    fn markdown_handles_no_findings() {
        let mut r = sample();
        r.findings.clear();
        r.chains.clear();
        let md = to_markdown(&r);
        assert!(md.contains("_No findings._"));
    }
}
