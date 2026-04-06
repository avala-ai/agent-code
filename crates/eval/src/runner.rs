use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json;

use crate::harness::{self, EvalResult, EvalVerdict};
use crate::policy::EvalPolicy;
use crate::registry;

/// Run all evals (or a filtered subset) and produce results.
pub async fn run_evals(
    agent_binary: &str,
    filter: Option<&str>,
    policy_filter: Option<EvalPolicy>,
    retries: usize,
    env: &[(&str, &str)],
    results_file: Option<&PathBuf>,
) -> Result<Vec<EvalResult>> {
    let all_evals = registry::all_evals();

    let evals: Vec<_> = all_evals
        .iter()
        .filter(|e| {
            if let Some(f) = filter {
                e.name.contains(f)
            } else {
                true
            }
        })
        .filter(|e| {
            if let Some(p) = policy_filter {
                e.policy == p
            } else {
                true
            }
        })
        .collect();

    if evals.is_empty() {
        println!("No evals matched the filter.");
        return Ok(Vec::new());
    }

    println!("Running {} evals (retries: {})...\n", evals.len(), retries);

    let mut results = Vec::new();

    for eval in &evals {
        println!("━━━ {} ({:?}) ━━━", eval.name, eval.policy);

        let result = harness::run_eval(eval, agent_binary, retries, env).await;

        let icon = match result.verdict {
            EvalVerdict::Pass => "✓",
            EvalVerdict::Fail => "✗",
            EvalVerdict::Flaky => "~",
        };

        println!(
            "  {} {} — {}/{} passed\n",
            icon, result.verdict, result.passes, result.total
        );

        // Show errors for failed attempts.
        for (i, attempt) in result.attempts.iter().enumerate() {
            if let Some(err) = &attempt.error {
                println!("    Attempt {}: {}", i + 1, err);
            }
        }

        results.push(result);
    }

    // Summary.
    let passed = results
        .iter()
        .filter(|r| r.verdict == EvalVerdict::Pass)
        .count();
    let failed = results
        .iter()
        .filter(|r| r.verdict == EvalVerdict::Fail)
        .count();
    let flaky = results
        .iter()
        .filter(|r| r.verdict == EvalVerdict::Flaky)
        .count();

    println!("\n═══════════════════════════════════════");
    println!(
        "  Results: {} passed, {} failed, {} flaky",
        passed, failed, flaky
    );
    println!("═══════════════════════════════════════\n");

    // Write results to JSONL file if requested.
    if let Some(path) = results_file {
        write_results_jsonl(&results, path)?;
    }

    Ok(results)
}

/// List all registered evals.
pub fn list_evals() {
    let evals = registry::all_evals();
    println!("{} evals registered:\n", evals.len());
    println!("{:<40} {:<16} {}", "NAME", "POLICY", "MAX TURNS");
    println!("{}", "─".repeat(70));
    for eval in &evals {
        println!(
            "{:<40} {:<16} {}",
            eval.name,
            format!("{:?}", eval.policy),
            eval.max_turns,
        );
    }
}

fn write_results_jsonl(results: &[EvalResult], path: &PathBuf) -> Result<()> {
    use std::io::Write;

    let parent = path.parent().context("Invalid results path")?;
    std::fs::create_dir_all(parent)?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    let timestamp = Utc::now().to_rfc3339();

    for result in results {
        let entry = serde_json::json!({
            "timestamp": timestamp,
            "name": result.name,
            "policy": result.policy,
            "passes": result.passes,
            "total": result.total,
            "verdict": result.verdict.to_string(),
        });
        writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    }

    Ok(())
}
