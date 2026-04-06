use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use agent_code_eval::policy::EvalPolicy;
use agent_code_eval::runner;

/// Behavioral evaluation runner for agent-code.
///
/// Runs evals that test agent behavior with live LLMs.
/// Each eval: sets up a workspace, sends a prompt, asserts on results.
///
/// Usage:
///   eval_runner --list                          # Show all evals
///   eval_runner                                 # Run all evals
///   eval_runner --eval creates_new_file         # Run one eval
///   eval_runner --policy always_passes          # Run only AlwaysPasses evals
///   eval_runner --retries 2                     # Custom retry count
#[derive(Parser, Debug)]
#[command(name = "eval_runner", version, about)]
struct Cli {
    /// List all evals without running them.
    #[arg(long)]
    list: bool,

    /// Run only evals matching this name substring.
    #[arg(long)]
    eval: Option<String>,

    /// Filter by policy: always_passes or usually_passes.
    #[arg(long)]
    policy: Option<String>,

    /// Number of retries per eval (default: 4).
    #[arg(long, default_value = "4")]
    retries: usize,

    /// Path to agent binary (default: target/release/agent).
    #[arg(long, default_value = "target/release/agent")]
    agent: String,

    /// Write results to this JSONL file.
    #[arg(long)]
    results: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    if cli.list {
        runner::list_evals();
        return Ok(());
    }

    let policy_filter = cli.policy.as_deref().map(|p| match p {
        "always_passes" | "AlwaysPasses" => EvalPolicy::AlwaysPasses,
        "usually_passes" | "UsuallyPasses" => EvalPolicy::UsuallyPasses,
        _ => {
            eprintln!("Unknown policy: {p}. Use always_passes or usually_passes.");
            std::process::exit(1);
        }
    });

    // Collect env vars for the agent process.
    let env_pairs: Vec<(&str, &str)> = Vec::new();

    let results = runner::run_evals(
        &cli.agent,
        cli.eval.as_deref(),
        policy_filter,
        cli.retries,
        &env_pairs,
        cli.results.as_ref(),
    )
    .await?;

    // Exit with failure if any AlwaysPasses eval failed.
    let blocking_failures = results
        .iter()
        .filter(|r| {
            r.policy == "AlwaysPasses" && r.verdict != agent_code_eval::harness::EvalVerdict::Pass
        })
        .count();

    if blocking_failures > 0 {
        eprintln!("{blocking_failures} AlwaysPasses eval(s) failed.");
        std::process::exit(1);
    }

    Ok(())
}
