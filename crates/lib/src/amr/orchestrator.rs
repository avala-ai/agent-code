//! ORCHESTRATOR: the deterministic master that sequences the pipeline.
//!
//! ```text
//!   PLAN*  →  SHARD  →  BATCH  →  MAP (concurrent)  →  REDUCE  →  report
//! ```
//!
//! `PLAN` is a baseline profile in the vertical slice (the selectors ship
//! with the profile); authoring per-repo selectors with an agent is future
//! work. Everything the orchestrator does between the agent calls is pure
//! and deterministic, so a rerun with an unchanged tree produces the same
//! shards and only pays for the diff.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;

use super::agent::{AmrAgent, RunOpts};
use super::batch::{BatchConfig, batch_signals};
use super::profile::{Profile, security_profile};
use super::reduce;
use super::types::{Batch, Finding, ScanReport, Severity, TokenTotals};
use super::{AmrError, cache, shard};

/// Everything a scan needs. Populated from CLI flags.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// Profile name. Only `"security"` is built in today.
    pub profile: String,
    pub repo_root: PathBuf,
    /// Model override for MAP workers (the bulk of the tokens).
    pub map_model: Option<String>,
    /// Model override for the REDUCE worker.
    pub reduce_model: Option<String>,
    pub max_signals_per_shard: usize,
    pub max_concurrency: usize,
    pub map_max_turns: usize,
    pub reduce_max_turns: usize,
    /// Informational threshold used for exit-code / summary, not for
    /// dropping findings.
    pub severity_threshold: Severity,
    pub incremental: bool,
    pub max_file_bytes: usize,
}

impl ScanConfig {
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            profile: "security".to_string(),
            repo_root: repo_root.into(),
            map_model: None,
            reduce_model: None,
            max_signals_per_shard: 40,
            max_concurrency: 6,
            map_max_turns: 12,
            reduce_max_turns: 8,
            severity_threshold: Severity::P2,
            incremental: false,
            max_file_bytes: 1_000_000,
        }
    }
}

fn resolve_profile(name: &str) -> Result<Profile, AmrError> {
    match name {
        "security" => Ok(security_profile()),
        other => Err(AmrError::Invalid(format!(
            "unknown profile `{other}` (only `security` is available)"
        ))),
    }
}

/// Run a full scan and return the report.
pub async fn run_scan(agent: Arc<dyn AmrAgent>, cfg: &ScanConfig) -> Result<ScanReport, AmrError> {
    let started = Instant::now();
    let profile = resolve_profile(&cfg.profile)?;
    let repo_abs = std::fs::canonicalize(&cfg.repo_root).unwrap_or_else(|_| cfg.repo_root.clone());

    // --- incremental gating -------------------------------------------------
    let mut existing_cache = cache::ScanCache::load(&cfg.repo_root);
    let (files_filter, base_commit) = if cfg.incremental {
        match existing_cache.base_commit.clone() {
            Some(base) => match cache::changed_files_since(&cfg.repo_root, &base) {
                Some(changed) => (Some(changed.into_iter().collect::<Vec<_>>()), Some(base)),
                None => (None, None),
            },
            None => (None, None),
        }
    } else {
        (None, None)
    };
    let incremental_run = files_filter.is_some();
    let changed_set: std::collections::BTreeSet<PathBuf> = files_filter
        .as_ref()
        .map(|v| v.iter().cloned().collect())
        .unwrap_or_default();

    // --- SHARD --------------------------------------------------------------
    let shard_input = shard::ShardInput {
        repo_root: cfg.repo_root.clone(),
        files: files_filter,
        max_file_bytes: cfg.max_file_bytes,
    };
    let shard_out = shard::collect_signals(&profile, &shard_input)?;

    // --- BATCH --------------------------------------------------------------
    let batches = batch_signals(
        shard_out.signals.clone(),
        &BatchConfig {
            max_signals_per_shard: cfg.max_signals_per_shard,
        },
    );

    // --- MAP (concurrent) ---------------------------------------------------
    let mut cost = 0.0;
    let mut tokens = TokenTotals::default();
    let mut map_findings: Vec<Finding> = Vec::new();
    let mut worker_failures = 0usize;

    if !batches.is_empty() {
        let sem = Arc::new(Semaphore::new(cfg.max_concurrency.max(1)));
        let mut set = tokio::task::JoinSet::new();
        for batch in batches.iter() {
            let prompt = build_map_prompt(&profile, &repo_abs, batch);
            let opts = RunOpts::map_worker(
                cfg.repo_root.clone(),
                cfg.map_model.clone(),
                cfg.map_max_turns,
            );
            let agent = agent.clone();
            let sem = sem.clone();
            let shard_id = batch.id.clone();
            set.spawn(async move {
                let _permit = sem.acquire_owned().await;
                let res = agent.run(&prompt, &opts).await;
                (shard_id, res)
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((shard_id, Ok(run))) => {
                    cost += run.cost_usd;
                    tokens.add(run.input_tokens, run.output_tokens);
                    let wf = reduce::parse_worker_findings(&run.text);
                    for mut f in wf.findings {
                        if f.shard_id.is_none() {
                            f.shard_id = Some(shard_id.clone());
                        }
                        f.file = relativize(&f.file, &repo_abs);
                        map_findings.push(f);
                    }
                }
                Ok((_, Err(_))) | Err(_) => worker_failures += 1,
            }
        }
    }

    // --- incremental merge + dedup -----------------------------------------
    let mut deduped = if incremental_run {
        let merged = cache::merge_incremental(&existing_cache, &changed_set, &map_findings);
        reduce::dedup_findings(merged)
    } else {
        reduce::dedup_findings(map_findings)
    };
    reduce::assign_ids(&mut deduped);

    // Persist MAP-stage findings for the next incremental run.
    existing_cache.base_commit = cache::head_commit(&cfg.repo_root).or(existing_cache.base_commit);
    existing_cache.findings_by_file.clear();
    existing_cache.index_findings(&deduped);
    let _ = existing_cache.save(&cfg.repo_root);

    // --- REDUCE -------------------------------------------------------------
    let (mut final_findings, chains) = if deduped.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let prompt = reduce::build_reduce_prompt(&profile, &deduped);
        let opts = RunOpts::reduce_worker(
            cfg.repo_root.clone(),
            cfg.reduce_model.clone(),
            cfg.reduce_max_turns,
        );
        match agent.run(&prompt, &opts).await {
            Ok(run) => {
                cost += run.cost_usd;
                tokens.add(run.input_tokens, run.output_tokens);
                let result = reduce::parse_reduce_result(&run.text, &deduped);
                (result.findings, result.chains)
            }
            // A failed reducer must not lose the MAP work.
            Err(_) => (deduped.clone(), Vec::new()),
        }
    };
    reduce::assign_ids(&mut final_findings);
    reduce::prioritize(&mut final_findings);

    // Worker failures are carried in the report (and surfaced on stderr by
    // the CLI) rather than logged, so `--format json` keeps stdout clean.
    Ok(ScanReport {
        profile: profile.name.to_string(),
        repo_root: repo_abs.to_string_lossy().to_string(),
        scanned_files: shard_out.scanned_files,
        dropped_files: shard_out.dropped_files,
        signals: shard_out.signals.len(),
        shards: batches.len(),
        worker_failures,
        findings: final_findings,
        chains,
        cost_usd: cost,
        tokens,
        duration_ms: started.elapsed().as_millis(),
        incremental: incremental_run,
        base_commit,
    })
}

/// Assemble the MAP worker prompt for one shard: the profile preamble, the
/// concrete signals (with absolute paths so read-only tools resolve
/// regardless of cwd), and a strict output-format instruction.
fn build_map_prompt(profile: &Profile, repo_abs: &std::path::Path, batch: &Batch) -> String {
    let mut signals = String::new();
    for s in &batch.signals {
        let abs = repo_abs.join(&s.file);
        let line = s.line.map(|l| format!(":{l}")).unwrap_or_default();
        signals.push_str(&format!(
            "- {}{} [{}] {}\n",
            abs.display(),
            line,
            s.selector_id,
            s.evidence
        ));
    }
    format!(
        "{preamble}\n\nSEVERITY RUBRIC:\n{rubric}\n\nREPOSITORY ROOT: {root}\nSHARD: {id}\n\n\
Investigate each candidate signal below. Read the real code with your read-only tools \
(use the absolute paths given):\n{signals}\n\
Respond with ONLY a single fenced ```json block matching this schema:\n\
{{\"findings\": [ {{\"id\": string, \"cwe\": string|null, \"file\": string, \
\"line_range\": [int,int]|null, \"severity\": \"P0\"|\"P1\"|\"P2\", \"confidence\": number, \
\"title\": string, \"root_cause\": string, \"exploit_preconditions\": string, \"evidence\": string}} ]}}\n\
If the shard contains no real vulnerabilities, return {{\"findings\": []}}.",
        preamble = profile.investigate_preamble,
        rubric = profile.severity_rubric,
        root = repo_abs.display(),
        id = batch.id,
        signals = signals,
    )
}

/// Make a worker-reported file path repo-relative when it sits under the
/// scanned root. Workers are handed absolute paths and echo them back, so
/// the report normalizes them for readability.
fn relativize(file: &str, repo_abs: &std::path::Path) -> String {
    match std::path::Path::new(file).strip_prefix(repo_abs) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amr::agent::ClosureAgent;
    use std::fs;

    fn write(dir: &std::path::Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// A fake agent that recognises MAP vs REDUCE prompts by their preamble
    /// and returns canned JSON, so the whole pipeline runs with no network.
    fn scripted_agent() -> Arc<dyn AmrAgent> {
        Arc::new(ClosureAgent::new(|prompt: &str| {
            if prompt.contains("examining ONE shard") {
                // MAP: report one real finding.
                r#"```json
{"findings":[{"id":"","cwe":"CWE-78","file":"app.py","line_range":[3,3],
"severity":"P0","confidence":0.95,"title":"OS command injection",
"root_cause":"user input to os.system","exploit_preconditions":"reachable endpoint",
"evidence":"os.system('ping ' + host)"}]}
```"#
                    .to_string()
            } else if prompt.contains("deduplicated candidate findings") {
                // REDUCE: echo the finding, add a chain.
                r#"```json
{"findings":[{"id":"f-0000","cwe":"CWE-78","file":"app.py","line_range":[3,3],
"severity":"P0","confidence":0.95,"title":"OS command injection",
"root_cause":"user input to os.system","exploit_preconditions":"reachable endpoint","evidence":"os.system"}],
"chains":[{"chain_id":"c1","member_finding_ids":["f-0000"],"combined_severity":"P0",
"narrative":"single-step RCE","combined_preconditions":"none"}]}
```"#
                    .to_string()
            } else {
                r#"{"findings":[]}"#.to_string()
            }
        }))
    }

    #[tokio::test]
    async fn full_pipeline_finds_and_reduces_a_vulnerability() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "app.py",
            "import os\ndef h(r):\n    os.system('ping ' + r.host)\n",
        );
        write(dir.path(), "safe.py", "def add(a,b):\n    return a+b\n");

        let cfg = ScanConfig::new(dir.path());
        let report = run_scan(scripted_agent(), &cfg).await.unwrap();

        assert_eq!(report.profile, "security");
        assert_eq!(report.scanned_files, 1, "only app.py has signals");
        assert!(report.signals >= 1);
        assert_eq!(report.shards, 1);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].severity, Severity::P0);
        assert!(!report.findings[0].id.is_empty(), "ids are backfilled");
        assert_eq!(report.chains.len(), 1);
    }

    #[tokio::test]
    async fn clean_repo_short_circuits_before_any_worker() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "safe.py", "def add(a,b):\n    return a+b\n");

        // Agent that panics if ever called — proves no worker ran.
        let agent: Arc<dyn AmrAgent> = Arc::new(ClosureAgent::new(|_| {
            panic!("no worker should run on a clean repo")
        }));
        let cfg = ScanConfig::new(dir.path());
        let report = run_scan(agent, &cfg).await.unwrap();

        assert_eq!(report.shards, 0);
        assert!(report.findings.is_empty());
        assert_eq!(report.cost_usd, 0.0);
    }

    #[test]
    fn relativize_strips_repo_prefix() {
        let root = std::path::Path::new("/repo/root");
        assert_eq!(relativize("/repo/root/src/a.py", root), "src/a.py");
        // Paths outside the root are left untouched.
        assert_eq!(relativize("already/rel.py", root), "already/rel.py");
    }

    #[tokio::test]
    async fn unknown_profile_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ScanConfig::new(dir.path());
        cfg.profile = "nope".into();
        let err = run_scan(scripted_agent(), &cfg).await.unwrap_err();
        assert!(matches!(err, AmrError::Invalid(_)));
    }
}
