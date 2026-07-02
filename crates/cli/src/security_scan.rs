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

/// Exit code when one or more MAP workers failed, so the scan could not
/// cover every shard. A "no findings" result in this state is untrustworthy,
/// so CI must not read it as a pass.
const EXIT_INCOMPLETE_COVERAGE: i32 = 3;

pub async fn run(llm: Arc<dyn Provider>, config: Config, args: ScanArgs) -> anyhow::Result<()> {
    let threshold = args
        .severity_threshold
        .parse::<amr::Severity>()
        .map_err(|e| anyhow::anyhow!(e))?;

    // Validate the output format up front so a bad value fails before any
    // scanning work (LLM calls, cache writes) happens.
    match args.format.as_str() {
        "json" | "markdown" | "md" => {}
        other => anyhow::bail!("unknown --format `{other}` (expected markdown or json)"),
    }

    // Resolve `-o` to an absolute path BEFORE any chdir, so the report still
    // lands where the user expects (their invocation directory).
    let output_abs = args.output.as_ref().map(|o| {
        let p = std::path::Path::new(o);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        }
    });

    // Run from inside the scan root. Read-only worker tools resolve a RELATIVE
    // path (e.g. a worker following an import with `FileRead("helper.py")` or
    // `Grep(path: "src")`) against the process working directory, and so does
    // the read-scope permission check. If the scan target is not the invoker's
    // cwd, those relative reads would resolve outside the scope and be denied —
    // shrinking coverage — even though the file is inside the target repo.
    // Making cwd == scan root keeps the tool and the permission gate consistent
    // (no scope escape) while letting relative reads land in-scope. A canonical
    // path that is not a directory is left for `run_scan` to reject.
    let scan_root: std::path::PathBuf = match std::fs::canonicalize(&args.path) {
        Ok(abs) if abs.is_dir() => {
            let _ = std::env::set_current_dir(&abs);
            abs
        }
        _ => std::path::PathBuf::from(&args.path),
    };

    let mut cfg = ScanConfig::new(&scan_root);
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

    if let Some(path) = &output_abs {
        std::fs::write(path, &rendered)?;
        eprintln!("Report written to {}", path.display());
    } else {
        println!("{rendered}");
    }

    // Count findings AND attack chains at or above the threshold: a chain
    // can be over-threshold even when its member findings are not.
    let over = report.items_at_or_above(threshold);
    eprintln!(
        "Scanned {} file(s) with signals, {} shard(s); {} finding(s), {} at/above {}. Cost ${:.4}.",
        report.scanned_files,
        report.shards,
        report.findings.len(),
        over,
        threshold,
        report.cost_usd,
    );
    if report.worker_failures > 0 {
        eprintln!(
            "Warning: {} scan worker(s) failed (map coverage gap or a failed reduce) \
             — the analysis is incomplete, so a clean result cannot be trusted.",
            report.worker_failures
        );
    }

    // Exit code precedence: findings at/above threshold (2) outrank an
    // incomplete scan (3); a scan that could not cover every shard must not
    // report success, or a provider outage would look like a clean gate.
    let exit_code = if over > 0 {
        Some(EXIT_FINDINGS_OVER_THRESHOLD)
    } else if report.worker_failures > 0 {
        Some(EXIT_INCOMPLETE_COVERAGE)
    } else {
        None
    };
    if let Some(code) = exit_code {
        // Flush stdout before exiting so a redirected report is complete.
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        std::process::exit(code);
    }
    Ok(())
}
