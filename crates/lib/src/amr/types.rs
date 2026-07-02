//! Core data types passed between AMR stages.
//!
//! The stage boundary types are deliberately small and serde-friendly:
//! a [`Signal`] is what a deterministic selector emits, a [`Finding`] is
//! what a MAP worker reports, an [`AttackChain`] is what the REDUCE agent
//! composes across shards, and [`ScanReport`] is the final artifact.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Triage severity. `P0` is the most severe.
///
/// Serialises to the bare tags `"P0"`, `"P1"`, `"P2"` so worker JSON and
/// report JSON read naturally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// Remotely exploitable, no auth, integrity/confidentiality loss.
    P0,
    /// Exploitable with preconditions or authentication.
    P1,
    /// Requires local access or unusual configuration.
    P2,
}

impl Severity {
    /// Numeric rank where higher is more severe. Used for prioritisation
    /// and for the `--severity-threshold` gate.
    pub fn rank(self) -> u8 {
        match self {
            Severity::P0 => 3,
            Severity::P1 => 2,
            Severity::P2 => 1,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Severity::P0 => "P0",
            Severity::P1 => "P1",
            Severity::P2 => "P2",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for Severity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "P0" => Ok(Severity::P0),
            "P1" => Ok(Severity::P1),
            "P2" => Ok(Severity::P2),
            other => Err(format!(
                "invalid severity `{other}` (expected P0, P1, or P2)"
            )),
        }
    }
}

/// A deterministic selector match: *where* it fired, *which* selector
/// produced it, and *what* evidence triggered it. Files that emit no
/// signals never reach the expensive MAP stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    /// Path relative to the repository root.
    pub file: PathBuf,
    /// 1-based line where the match starts, when known.
    pub line: Option<usize>,
    /// Byte offsets `[start, end)` of the match, when known.
    pub byte_range: Option<(usize, usize)>,
    /// Identifier of the selector that fired (e.g. `dangerous_call.eval`).
    pub selector_id: String,
    /// Compact snippet or description of what matched.
    pub evidence: String,
}

/// A bounded bucket of signals handed to a single MAP worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    /// Stable identifier, e.g. `shard-0007`.
    pub id: String,
    /// The signals assigned to this shard.
    pub signals: Vec<Signal>,
}

impl Batch {
    /// Distinct files referenced by this shard's signals, in stable order.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.signals {
            seen.insert(s.file.clone());
        }
        seen.into_iter().collect()
    }
}

/// One conclusion from a MAP worker. Workers account for every file they
/// were handed and report zero or more of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable-within-a-run identifier. Worker-assigned when present; the
    /// orchestrator backfills a deterministic id when a worker omits it.
    #[serde(default)]
    pub id: String,
    /// CWE identifier when known, e.g. `CWE-89`.
    #[serde(default)]
    pub cwe: Option<String>,
    /// Path (repo-relative) the finding lives in.
    pub file: String,
    /// 1-based `[start, end]` line range, when known.
    #[serde(default)]
    pub line_range: Option<(usize, usize)>,
    pub severity: Severity,
    /// Worker confidence in `[0.0, 1.0]`.
    #[serde(default)]
    pub confidence: f64,
    pub title: String,
    /// Why this is a genuine issue.
    #[serde(default)]
    pub root_cause: String,
    /// What must hold for the issue to be exploitable/relevant.
    #[serde(default)]
    pub exploit_preconditions: String,
    /// Concrete evidence (quoted code, data flow).
    #[serde(default)]
    pub evidence: String,
    /// Selector that surfaced the originating signal, when tracked.
    #[serde(default)]
    pub selector_id: Option<String>,
    /// Shard this finding came from, when tracked.
    #[serde(default)]
    pub shard_id: Option<String>,
}

/// The JSON envelope a MAP worker is asked to emit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerFindings {
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// A cross-shard composition the REDUCE agent identifies: lower-severity
/// findings that combine into a higher-impact one (e.g. an unauthenticated
/// ID leak plus an ID-gated RCE becoming one P0 unauthenticated RCE).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttackChain {
    pub chain_id: String,
    /// [`Finding::id`] values that participate in this chain.
    #[serde(default)]
    pub member_finding_ids: Vec<String>,
    pub combined_severity: Severity,
    pub narrative: String,
    #[serde(default)]
    pub combined_preconditions: String,
}

/// The JSON envelope the REDUCE agent is asked to emit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReduceResult {
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub chains: Vec<AttackChain>,
}

/// Accumulated token counts across every stage of a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
}

impl TokenTotals {
    pub fn add(&mut self, input: u64, output: u64) {
        self.input += input;
        self.output += output;
    }
}

/// The final artifact of a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub profile: String,
    pub repo_root: String,
    /// Files that emitted at least one signal and reached a worker.
    pub scanned_files: usize,
    /// Files considered but dropped for emitting zero signals.
    pub dropped_files: usize,
    /// Total signals across all shards.
    pub signals: usize,
    /// Number of MAP shards fanned out.
    pub shards: usize,
    /// MAP workers that errored and were skipped. Non-zero means coverage
    /// is incomplete, so it is surfaced rather than hidden.
    #[serde(default)]
    pub worker_failures: usize,
    /// Deduped, prioritised standalone findings (highest severity first).
    pub findings: Vec<Finding>,
    /// Cross-shard attack chains.
    pub chains: Vec<AttackChain>,
    pub cost_usd: f64,
    pub tokens: TokenTotals,
    pub duration_ms: u128,
    /// True when this run only scanned the diff since `base_commit`.
    pub incremental: bool,
    #[serde(default)]
    pub base_commit: Option<String>,
}

impl ScanReport {
    /// Count of standalone findings at or above `threshold`.
    pub fn findings_at_or_above(&self, threshold: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity.rank() >= threshold.rank())
            .count()
    }

    /// Count of standalone findings AND attack chains at or above
    /// `threshold`. This is the correct value for a CI exit gate: a reducer
    /// can compose a P0 chain from members that are each individually below
    /// the threshold, and that composed vulnerability must still gate.
    pub fn items_at_or_above(&self, threshold: Severity) -> usize {
        self.findings_at_or_above(threshold)
            + self
                .chains
                .iter()
                .filter(|c| c.combined_severity.rank() >= threshold.rank())
                .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ranks_p0_highest() {
        assert!(Severity::P0.rank() > Severity::P1.rank());
        assert!(Severity::P1.rank() > Severity::P2.rank());
    }

    #[test]
    fn severity_roundtrips_through_string() {
        for s in [Severity::P0, Severity::P1, Severity::P2] {
            assert_eq!(s.to_string().parse::<Severity>().unwrap(), s);
        }
        assert_eq!("p1".parse::<Severity>().unwrap(), Severity::P1);
        assert!("P9".parse::<Severity>().is_err());
    }

    #[test]
    fn severity_serialises_to_bare_tag() {
        assert_eq!(serde_json::to_string(&Severity::P0).unwrap(), "\"P0\"");
        let s: Severity = serde_json::from_str("\"P2\"").unwrap();
        assert_eq!(s, Severity::P2);
    }

    #[test]
    fn batch_files_are_unique_and_sorted() {
        let sig = |f: &str, id: &str| Signal {
            file: PathBuf::from(f),
            line: None,
            byte_range: None,
            selector_id: id.to_string(),
            evidence: String::new(),
        };
        let batch = Batch {
            id: "shard-0".into(),
            signals: vec![
                sig("src/b.rs", "s1"),
                sig("src/a.rs", "s2"),
                sig("src/b.rs", "s3"),
            ],
        };
        assert_eq!(
            batch.files(),
            vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")]
        );
    }

    #[test]
    fn worker_findings_parses_minimal_json() {
        // Only the required fields present — optional ones default.
        let json = r#"{"findings":[{"id":"f1","file":"a.py","severity":"P0","title":"eval on user input"}]}"#;
        let wf: WorkerFindings = serde_json::from_str(json).unwrap();
        assert_eq!(wf.findings.len(), 1);
        assert_eq!(wf.findings[0].severity, Severity::P0);
        assert_eq!(wf.findings[0].confidence, 0.0);
        assert!(wf.findings[0].cwe.is_none());
    }

    #[test]
    fn empty_worker_findings_is_valid() {
        let wf: WorkerFindings = serde_json::from_str(r#"{"findings":[]}"#).unwrap();
        assert!(wf.findings.is_empty());
    }

    #[test]
    fn report_counts_by_threshold() {
        let f = |sev: Severity| Finding {
            id: "x".into(),
            cwe: None,
            file: "a".into(),
            line_range: None,
            severity: sev,
            confidence: 0.9,
            title: "t".into(),
            root_cause: String::new(),
            exploit_preconditions: String::new(),
            evidence: String::new(),
            selector_id: None,
            shard_id: None,
        };
        let report = ScanReport {
            profile: "security".into(),
            repo_root: ".".into(),
            scanned_files: 3,
            dropped_files: 10,
            signals: 5,
            shards: 2,
            worker_failures: 0,
            findings: vec![f(Severity::P0), f(Severity::P1), f(Severity::P2)],
            chains: vec![],
            cost_usd: 0.0,
            tokens: TokenTotals::default(),
            duration_ms: 0,
            incremental: false,
            base_commit: None,
        };
        assert_eq!(report.findings_at_or_above(Severity::P1), 2);
        assert_eq!(report.findings_at_or_above(Severity::P0), 1);
    }

    #[test]
    fn items_at_or_above_includes_chains() {
        let finding = Finding {
            id: "f".into(),
            cwe: None,
            file: "a".into(),
            line_range: None,
            severity: Severity::P2,
            confidence: 0.9,
            title: "t".into(),
            root_cause: String::new(),
            exploit_preconditions: String::new(),
            evidence: String::new(),
            selector_id: None,
            shard_id: None,
        };
        let chain = AttackChain {
            chain_id: "c1".into(),
            member_finding_ids: vec!["f".into()],
            combined_severity: Severity::P0,
            narrative: "n".into(),
            combined_preconditions: String::new(),
        };
        let report = ScanReport {
            profile: "security".into(),
            repo_root: ".".into(),
            scanned_files: 1,
            dropped_files: 0,
            signals: 1,
            shards: 1,
            worker_failures: 0,
            findings: vec![finding],
            chains: vec![chain],
            cost_usd: 0.0,
            tokens: TokenTotals::default(),
            duration_ms: 0,
            incremental: false,
            base_commit: None,
        };
        // The lone finding is P2; the composed chain is P0 and must gate.
        assert_eq!(report.findings_at_or_above(Severity::P1), 0);
        assert_eq!(report.items_at_or_above(Severity::P1), 1);
    }
}
