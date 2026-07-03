//! Security-scan CVE-recall benchmark.
//!
//! Measures how many real, published vulnerabilities `agent security-scan`
//! finds, using the methodology established by public security-scanner evals:
//!
//! - Each case is a real CVE checked out at the commit *immediately before* its
//!   fix, so the flaw still exists in the tree.
//! - Cases should be published *after* the scanning model's training cutoff, so
//!   the answer was not memorized during pre-training.
//! - The score is **recall**: the fraction of cases in which at least one of the
//!   scan's findings describes the target vulnerability. Everything else
//!   (including false positives) is ignored here, matching the standard eval.
//!
//! Two graders decide "did a finding describe the target":
//! - [`Grader::Heuristic`] — deterministic and free: a finding on the target
//!   file whose CWE matches. Good for CI signal and unit tests.
//! - [`Grader::Llm`] — a semantic judge (an `agent -p` call) that reads the
//!   target and the findings and answers yes/no. Higher fidelity, costs tokens.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One benchmark case: a real CVE, pinned to the commit before its patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CveCase {
    /// CVE identifier (or a stable slug).
    pub id: String,
    /// Target CWE, e.g. `CWE-89`.
    pub cwe: String,
    /// Primary language of the vulnerable code.
    pub language: String,
    /// Git URL to clone.
    pub repo: String,
    /// Commit SHA immediately BEFORE the fix (flaw still present).
    pub commit: String,
    /// Repo-relative path of the file the vulnerability lives in.
    pub file: String,
    /// One-line description of the target vulnerability (for the LLM judge).
    pub description: String,
    /// Advisory publication date (post-cutoff safeguard); informational.
    #[serde(default)]
    pub published: Option<String>,
}

/// How a finding is judged to match the target vulnerability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grader {
    /// Deterministic: a finding on the target file with a matching CWE.
    Heuristic,
    /// Semantic: an LLM judge decides if any finding describes the target.
    Llm,
}

/// The subset of a scan report the benchmark grades on. Matches the JSON that
/// `agent security-scan --format json` emits.
#[derive(Debug, Clone, Deserialize)]
pub struct ScanReport {
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub chains: Vec<serde_json::Value>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub worker_failures: usize,
    /// Files the selectors routed to a MAP worker (coverage numerator).
    #[serde(default)]
    pub scanned_files: usize,
    #[serde(default)]
    pub shards: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub cwe: Option<String>,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub evidence: String,
}

/// A finding trimmed to the fields worth keeping in the benchmark report, so a
/// result is diagnosable (why did a case miss?) without re-running the scan.
#[derive(Debug, Clone, Serialize)]
pub struct FindingBrief {
    pub cwe: Option<String>,
    pub file: String,
    pub title: String,
}

impl From<&Finding> for FindingBrief {
    fn from(f: &Finding) -> Self {
        FindingBrief {
            cwe: f.cwe.clone(),
            file: f.file.clone(),
            title: f.title.clone(),
        }
    }
}

/// Per-case outcome.
#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub language: String,
    pub found: bool,
    pub num_findings: usize,
    pub cost_usd: f64,
    /// Files the scan actually analyzed — a coverage signal. A miss with a low
    /// count is a selector gap; a miss with high coverage is a depth/recall gap.
    pub scanned_files: usize,
    pub shards: usize,
    pub worker_failures: usize,
    /// When the case missed, a short reason so results are triageable in bulk.
    pub miss_reason: Option<String>,
    /// The scan's findings (trimmed), for offline analysis.
    pub findings: Vec<FindingBrief>,
    /// Set when the case could not be evaluated (clone/scan error). A case with
    /// an error counts as not-found (it did not surface the vuln).
    pub error: Option<String>,
}

/// Classify why a case missed, from the target and the scan findings. Returns
/// `None` when the case was found. Used only for triage — not for scoring.
pub fn classify_miss(case: &CveCase, findings: &[Finding]) -> Option<&'static str> {
    if heuristic_found(case, findings) {
        return None;
    }
    if findings.is_empty() {
        return Some("no-findings");
    }
    let file_hit = findings.iter().any(|f| file_matches(&f.file, &case.file));
    let cwe_hit = findings
        .iter()
        .any(|f| f.cwe.as_deref().is_some_and(|c| cwe_matches(c, &case.cwe)));
    Some(match (file_hit, cwe_hit) {
        // Right file, right class somewhere, but not on the same finding.
        (true, true) => "file-and-cwe-split-across-findings",
        (true, false) => "right-file-wrong-cwe",
        (false, true) => "right-cwe-wrong-file",
        (false, false) => "unrelated-findings",
    })
}

/// Aggregate benchmark report.
#[derive(Debug, Serialize)]
pub struct BenchReport {
    pub total: usize,
    pub found: usize,
    pub recall: f64,
    pub total_cost_usd: f64,
    pub avg_cost_usd: f64,
    /// language -> (found, total)
    pub per_language: BTreeMap<String, (usize, usize)>,
    pub cases: Vec<CaseResult>,
}

/// Load cases from a JSON manifest (an array of [`CveCase`]).
pub fn load_cases(path: &Path) -> Result<Vec<CveCase>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading benchmark manifest {}", path.display()))?;
    let cases: Vec<CveCase> =
        serde_json::from_str(&text).context("parsing benchmark manifest as JSON array")?;
    Ok(cases)
}

/// Normalize a CWE identifier to its bare number: `CWE-89`, `cwe89`, `89` -> `89`.
fn normalize_cwe(cwe: &str) -> String {
    cwe.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn cwe_matches(a: &str, b: &str) -> bool {
    let (na, nb) = (normalize_cwe(a), normalize_cwe(b));
    !na.is_empty() && na == nb
}

/// A finding's file matches the target when either path is a suffix of the
/// other, or the base names are equal. Scan and advisory paths can differ by a
/// leading directory (e.g. a monorepo subpath), so an exact match is too strict.
fn file_matches(finding_file: &str, target_file: &str) -> bool {
    let ff = finding_file.trim_start_matches("./");
    let tf = target_file.trim_start_matches("./");
    if ff.is_empty() || tf.is_empty() {
        return false;
    }
    if ff.ends_with(tf) || tf.ends_with(ff) {
        return true;
    }
    let base = |p: &str| Path::new(p).file_name().map(|s| s.to_os_string());
    base(ff) == base(tf)
}

/// Deterministic recall: any finding on the target file with a matching CWE.
pub fn heuristic_found(case: &CveCase, findings: &[Finding]) -> bool {
    findings.iter().any(|f| {
        file_matches(&f.file, &case.file)
            && f.cwe.as_deref().is_some_and(|c| cwe_matches(c, &case.cwe))
    })
}

/// Build the LLM judge prompt: given the target and the findings, decide match.
pub fn judge_prompt(case: &CveCase, findings: &[Finding]) -> String {
    let findings_json = serde_json::to_string_pretty(findings).unwrap_or_else(|_| "[]".into());
    format!(
        "You are grading a security scanner. TARGET VULNERABILITY:\n\
         - CWE: {cwe}\n- file: {file}\n- description: {desc}\n\n\
         SCANNER FINDINGS (JSON):\n{findings}\n\n\
         Does ANY finding describe the TARGET vulnerability — the same class of \
         bug in roughly the same location? Ignore unrelated findings. Answer with \
         a single word: YES or NO.",
        cwe = case.cwe,
        file = case.file,
        desc = case.description,
        findings = findings_json,
    )
}

/// Clone `case.repo` and check out `case.commit` into `dest`.
///
/// Uses a partial (blobless) clone to avoid pulling full history blobs, then
/// checks out the exact pre-patch commit. Returns an error the caller records
/// as a non-found case rather than aborting the whole run.
pub fn checkout_case(case: &CveCase, dest: &Path) -> Result<()> {
    run_git(&[
        "clone",
        "--filter=blob:none",
        "--no-checkout",
        &case.repo,
        &dest.to_string_lossy(),
    ])
    .with_context(|| format!("cloning {}", case.repo))?;
    run_git(&["-C", &dest.to_string_lossy(), "checkout", &case.commit])
        .with_context(|| format!("checking out {} in {}", case.commit, case.repo))?;
    Ok(())
}

fn run_git(args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .context("spawning git")?;
    if !out.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Run `agent security-scan <dir> --format json` and parse the report.
pub async fn run_scan(
    agent_binary: &str,
    dir: &Path,
    model: Option<&str>,
    env: &[(&str, &str)],
) -> Result<ScanReport> {
    let mut cmd = tokio::process::Command::new(agent_binary);
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    cmd.arg("security-scan")
        .arg(dir)
        .args(["--format", "json", "--severity-threshold", "P2"]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let out = cmd.output().await.context("spawning agent security-scan")?;
    // The scan exits non-zero (2/3) when it finds issues or coverage is
    // incomplete; that is not a harness error, so we parse stdout regardless.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: ScanReport = serde_json::from_str(stdout.trim()).with_context(|| {
        format!(
            "parsing scan JSON (stderr: {})",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;
    Ok(report)
}

/// Run one case end to end: checkout, scan, grade.
pub async fn run_case(
    case: &CveCase,
    agent_binary: &str,
    grader: Grader,
    model: Option<&str>,
    env: &[(&str, &str)],
) -> CaseResult {
    let mut result = CaseResult {
        id: case.id.clone(),
        language: case.language.clone(),
        found: false,
        num_findings: 0,
        cost_usd: 0.0,
        scanned_files: 0,
        shards: 0,
        worker_failures: 0,
        miss_reason: None,
        findings: Vec::new(),
        error: None,
    };

    let tmp = match tempfile::tempdir() {
        Ok(t) => t,
        Err(e) => {
            result.error = Some(format!("tempdir: {e}"));
            result.miss_reason = Some("error".into());
            return result;
        }
    };
    if let Err(e) = checkout_case(case, tmp.path()) {
        result.error = Some(format!("checkout: {e}"));
        result.miss_reason = Some("error".into());
        return result;
    }

    let report = match run_scan(agent_binary, tmp.path(), model, env).await {
        Ok(r) => r,
        Err(e) => {
            result.error = Some(format!("scan: {e}"));
            result.miss_reason = Some("error".into());
            return result;
        }
    };
    result.num_findings = report.findings.len();
    result.cost_usd = report.cost_usd;
    result.scanned_files = report.scanned_files;
    result.shards = report.shards;
    result.worker_failures = report.worker_failures;
    result.findings = report.findings.iter().map(FindingBrief::from).collect();

    result.found = match grader {
        Grader::Heuristic => heuristic_found(case, &report.findings),
        Grader::Llm => match llm_judge(agent_binary, case, &report.findings, model, env).await {
            Ok(v) => v,
            Err(e) => {
                result.error = Some(format!("judge: {e}"));
                // Fall back to the deterministic grader so a judge outage does
                // not silently zero the case.
                heuristic_found(case, &report.findings)
            }
        },
    };
    // Triage tag for misses (heuristic-based; independent of the grader used).
    if !result.found && result.error.is_none() {
        result.miss_reason = classify_miss(case, &report.findings).map(String::from);
    }
    result
}

/// Ask an LLM (via `agent -p`) whether any finding matches the target.
async fn llm_judge(
    agent_binary: &str,
    case: &CveCase,
    findings: &[Finding],
    model: Option<&str>,
    env: &[(&str, &str)],
) -> Result<bool> {
    let mut cmd = tokio::process::Command::new(agent_binary);
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    cmd.args(["-p", &judge_prompt(case, findings)]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let out = cmd.output().await.context("spawning judge")?;
    let answer = String::from_utf8_lossy(&out.stdout).to_uppercase();
    Ok(answer.contains("YES"))
}

/// Run the whole benchmark and aggregate recall + cost.
pub async fn run_bench(
    cases: &[CveCase],
    agent_binary: &str,
    grader: Grader,
    model: Option<&str>,
    env: &[(&str, &str)],
) -> BenchReport {
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let r = run_case(case, agent_binary, grader, model, env).await;
        results.push(r);
    }
    aggregate(results)
}

/// Aggregate per-case results into a report. Pure — unit-tested.
pub fn aggregate(cases: Vec<CaseResult>) -> BenchReport {
    let total = cases.len();
    let found = cases.iter().filter(|c| c.found).count();
    let total_cost_usd = cases.iter().map(|c| c.cost_usd).sum();
    let mut per_language: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for c in &cases {
        let entry = per_language.entry(c.language.clone()).or_insert((0, 0));
        entry.1 += 1;
        if c.found {
            entry.0 += 1;
        }
    }
    BenchReport {
        total,
        found,
        recall: if total == 0 {
            0.0
        } else {
            found as f64 / total as f64
        },
        total_cost_usd,
        avg_cost_usd: if total == 0 {
            0.0
        } else {
            total_cost_usd / total as f64
        },
        per_language,
        cases,
    }
}

/// Render a short human summary of a report.
pub fn summarize(report: &BenchReport) -> String {
    // Map -0.0 (e.g. from summing an empty case set) to a clean 0.0.
    let norm = |x: f64| if x == 0.0 { 0.0 } else { x };
    let mut s = format!(
        "recall {}/{} = {:.0}%  |  ${:.2} total, ${:.2}/case\n",
        report.found,
        report.total,
        report.recall * 100.0,
        norm(report.total_cost_usd),
        norm(report.avg_cost_usd),
    );
    for (lang, (f, t)) in &report.per_language {
        s.push_str(&format!("  {lang}: {f}/{t}\n"));
    }
    let missed: Vec<&CaseResult> = report.cases.iter().filter(|c| !c.found).collect();
    if !missed.is_empty() {
        s.push_str("  missed:\n");
        for c in &missed {
            s.push_str(&format!(
                "    {:<18} {:<12} reason={} coverage={}f/{}sh findings={}\n",
                c.id,
                c.language,
                c.miss_reason.as_deref().unwrap_or("?"),
                c.scanned_files,
                c.shards,
                c.num_findings,
            ));
        }
    }
    // Roll up miss reasons so a run's failure profile is visible at a glance.
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for c in &missed {
        *reasons.entry(c.miss_reason.as_deref().unwrap_or("?")).or_insert(0) += 1;
    }
    if !reasons.is_empty() {
        let parts: Vec<String> = reasons.iter().map(|(r, n)| format!("{r}={n}")).collect();
        s.push_str(&format!("  miss profile: {}\n", parts.join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(cwe: &str, file: &str) -> Finding {
        Finding {
            cwe: Some(cwe.into()),
            file: file.into(),
            title: "t".into(),
            evidence: String::new(),
        }
    }

    fn case(cwe: &str, file: &str) -> CveCase {
        CveCase {
            id: "CVE-x".into(),
            cwe: cwe.into(),
            language: "python".into(),
            repo: "https://example.com/r".into(),
            commit: "abc".into(),
            file: file.into(),
            description: "d".into(),
            published: None,
        }
    }

    #[test]
    fn normalize_and_match_cwe() {
        assert!(cwe_matches("CWE-89", "89"));
        assert!(cwe_matches("cwe89", "CWE-89"));
        assert!(cwe_matches("CWE-89", "CWE-89"));
        assert!(!cwe_matches("CWE-89", "CWE-78"));
        assert!(!cwe_matches("", "89"));
    }

    #[test]
    fn file_matches_by_suffix_and_basename() {
        assert!(file_matches("src/api/users.py", "users.py"));
        assert!(file_matches("users.py", "src/api/users.py"));
        assert!(file_matches("./api/users.py", "api/users.py"));
        assert!(file_matches("pkg/a/db.go", "other/prefix/pkg/a/db.go"));
        assert!(!file_matches("a/users.py", "b/orders.py"));
        assert!(!file_matches("", "users.py"));
    }

    #[test]
    fn heuristic_needs_file_and_cwe() {
        let c = case("CWE-89", "api/users.py");
        // Right file, right CWE -> found.
        assert!(heuristic_found(
            &c,
            &[finding("CWE-89", "server/api/users.py")]
        ));
        // Right file, wrong CWE -> not found.
        assert!(!heuristic_found(&c, &[finding("CWE-78", "api/users.py")]));
        // Right CWE, wrong file -> not found.
        assert!(!heuristic_found(&c, &[finding("CWE-89", "api/orders.py")]));
        // No findings -> not found.
        assert!(!heuristic_found(&c, &[]));
    }

    fn case_result(id: &str, language: &str, found: bool, cost_usd: f64) -> CaseResult {
        CaseResult {
            id: id.into(),
            language: language.into(),
            found,
            num_findings: 0,
            cost_usd,
            scanned_files: 0,
            shards: 0,
            worker_failures: 0,
            miss_reason: None,
            findings: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn aggregate_computes_recall_and_per_language() {
        let results = vec![
            case_result("a", "python", true, 1.0),
            case_result("b", "python", false, 0.5),
            case_result("c", "go", true, 2.0),
        ];
        let r = aggregate(results);
        assert_eq!((r.total, r.found), (3, 2));
        assert!((r.recall - 2.0 / 3.0).abs() < 1e-9);
        assert!((r.total_cost_usd - 3.5).abs() < 1e-9);
        assert_eq!(r.per_language["python"], (1, 2));
        assert_eq!(r.per_language["go"], (1, 1));
    }

    #[test]
    fn classify_miss_categorizes_by_file_and_cwe() {
        let c = case("CWE-22", "lib/rack/static.rb");
        // Found on the right file + CWE -> not a miss.
        assert_eq!(
            classify_miss(&c, &[finding("CWE-22", "lib/rack/static.rb")]),
            None
        );
        // No findings at all.
        assert_eq!(classify_miss(&c, &[]), Some("no-findings"));
        // Right CWE class but a different file (the real rack pilot miss).
        assert_eq!(
            classify_miss(&c, &[finding("CWE-22", "lib/rack/directory.rb")]),
            Some("right-cwe-wrong-file")
        );
        // Right file but a different CWE.
        assert_eq!(
            classify_miss(&c, &[finding("CWE-79", "lib/rack/static.rb")]),
            Some("right-file-wrong-cwe")
        );
        // Findings exist but relate to neither the file nor the class.
        assert_eq!(
            classify_miss(&c, &[finding("CWE-89", "app/db.rb")]),
            Some("unrelated-findings")
        );
    }

    #[test]
    fn judge_prompt_includes_target_and_findings() {
        let p = judge_prompt(&case("CWE-89", "users.py"), &[finding("CWE-89", "users.py")]);
        assert!(p.contains("CWE-89"));
        assert!(p.contains("users.py"));
        assert!(p.contains("YES or NO"));
    }

    #[test]
    fn load_cases_parses_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cases.json");
        std::fs::write(
            &path,
            r#"[{"id":"CVE-1","cwe":"CWE-89","language":"python",
                "repo":"https://x/r","commit":"abc","file":"a.py","description":"sqli"}]"#,
        )
        .unwrap();
        let cases = load_cases(&path).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].cwe, "CWE-89");
    }

    #[test]
    fn shipped_manifest_is_well_formed() {
        // The curated benchmark that ships in this crate must always parse and
        // meet the basic invariants the harness relies on.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("benchmarks/security-scan-cves.json");
        let cases = load_cases(&path).unwrap();
        assert_eq!(cases.len(), 50, "expected 50 curated cases");
        for c in &cases {
            assert!(c.id.starts_with("CVE-"), "bad id: {}", c.id);
            assert!(
                !normalize_cwe(&c.cwe).is_empty(),
                "case {} has no numeric CWE",
                c.id
            );
            assert_eq!(c.commit.len(), 40, "case {} commit is not a full SHA", c.id);
            assert!(c.commit.chars().all(|ch| ch.is_ascii_hexdigit()));
            assert!(c.repo.starts_with("https://github.com/"), "case {}", c.id);
            assert!(!c.file.is_empty(), "case {} has empty file", c.id);
        }
    }
}
