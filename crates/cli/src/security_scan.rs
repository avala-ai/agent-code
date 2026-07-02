//! `agent security-scan` — whole-repo vulnerability discovery via the
//! Agentic MapReduce engine in `agent-code-lib`.
//!
//! This is a thin adapter: it turns CLI flags into an [`amr::ScanConfig`],
//! wraps the already-constructed provider in an [`EngineAgent`] so MAP and
//! REDUCE workers reuse the engine in-process, runs the scan, and renders
//! the report. All the real work lives in the library.

use std::sync::Arc;

use agent_code_lib::amr::{self, ScanConfig, agent::EngineAgent};
use agent_code_lib::config::Config;
use agent_code_lib::llm::provider::Provider;

/// Parsed CLI arguments for the scan.
pub struct ScanArgs {
    pub path: String,
    pub profile: String,
    pub format: String,
    pub map_model: Option<String>,
    pub reduce_model: Option<String>,
    pub batch_size: usize,
    pub max_concurrency: usize,
    pub severity_threshold: String,
    pub incremental: bool,
    pub output: Option<String>,
}

/// Exit code returned when the scan surfaces findings at or above the
/// configured severity threshold. Distinct from the generic error code so
/// CI can tell "scan found issues" from "scan itself failed".
const EXIT_FINDINGS_OVER_THRESHOLD: i32 = 2;

pub async fn run(llm: Arc<dyn Provider>, config: Config, args: ScanArgs) -> anyhow::Result<()> {
    let threshold = args
        .severity_threshold
        .parse::<amr::Severity>()
        .map_err(|e| anyhow::anyhow!(e))?;

    let mut cfg = ScanConfig::new(&args.path);
    cfg.profile = args.profile;
    cfg.map_model = args.map_model;
    cfg.reduce_model = args.reduce_model;
    cfg.max_signals_per_shard = args.batch_size.max(1);
    cfg.max_concurrency = args.max_concurrency.max(1);
    cfg.severity_threshold = threshold;
    cfg.incremental = args.incremental;

    // MAP/REDUCE workers run the same engine in-process over any provider.
    let agent = Arc::new(EngineAgent::new(llm, config));

    eprintln!(
        "Agentic MapReduce security scan on {} (profile: {}{})",
        args.path,
        cfg.profile,
        if cfg.incremental { ", incremental" } else { "" }
    );

    let report = amr::run_scan(agent, &cfg)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let rendered = match args.format.as_str() {
        "json" => amr::report::to_json(&report),
        "markdown" | "md" => amr::report::to_markdown(&report),
        other => anyhow::bail!("unknown --format `{other}` (expected markdown or json)"),
    };

    if let Some(path) = &args.output {
        std::fs::write(path, &rendered)?;
        eprintln!("Report written to {path}");
    } else {
        println!("{rendered}");
    }

    if report.worker_failures > 0 {
        eprintln!(
            "Warning: {} map worker(s) failed and were skipped — coverage is incomplete.",
            report.worker_failures
        );
    }

    let over = report.findings_at_or_above(threshold);
    eprintln!(
        "Scanned {} file(s) with signals, {} shard(s); {} finding(s), {} at/above {}. Cost ${:.4}.",
        report.scanned_files,
        report.shards,
        report.findings.len(),
        over,
        threshold,
        report.cost_usd,
    );

    if over > 0 {
        // Flush stdout before exiting so a redirected report is complete.
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        std::process::exit(EXIT_FINDINGS_OVER_THRESHOLD);
    }
    Ok(())
}
