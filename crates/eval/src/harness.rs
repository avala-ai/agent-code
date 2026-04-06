use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::policy::EvalPolicy;
use crate::rig::TestRig;

/// Definition of a single behavioral eval.
pub struct EvalDef {
    /// Unique eval name (e.g., "creates_new_file").
    pub name: &'static str,
    /// Pass/fail policy tier.
    pub policy: EvalPolicy,
    /// Path to fixture directory (relative to repo root).
    pub fixture: Option<&'static str>,
    /// Prompt to send to the agent.
    pub prompt: &'static str,
    /// Maximum turns the agent can take.
    pub max_turns: usize,
    /// Assertion function. Returns Ok(()) on pass, Err on fail.
    pub assert_fn: fn(&TestRig) -> Result<()>,
}

/// Result of a single eval attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalAttempt {
    pub passed: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub tools_used: Vec<String>,
    pub turn_count: usize,
}

/// Result of running an eval with retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub name: String,
    pub policy: String,
    pub attempts: Vec<EvalAttempt>,
    pub passes: usize,
    pub total: usize,
    pub verdict: EvalVerdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvalVerdict {
    Pass,
    Fail,
    Flaky,
}

impl std::fmt::Display for EvalVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalVerdict::Pass => write!(f, "PASS"),
            EvalVerdict::Fail => write!(f, "FAIL"),
            EvalVerdict::Flaky => write!(f, "FLAKY"),
        }
    }
}

/// Run a single eval with best-of-N retry logic.
pub async fn run_eval(
    def: &EvalDef,
    agent_binary: &str,
    retries: usize,
    env: &[(&str, &str)],
) -> EvalResult {
    let mut attempts = Vec::new();
    let mut passes = 0;
    let repo_root = find_repo_root().unwrap_or_default();

    for attempt_num in 0..retries {
        tracing::info!(
            "  Attempt {}/{} for '{}'",
            attempt_num + 1,
            retries,
            def.name
        );

        let start = Instant::now();

        let result = run_single_attempt(def, agent_binary, &repo_root, env).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(rig) => {
                // Run assertions.
                match (def.assert_fn)(&rig) {
                    Ok(()) => {
                        passes += 1;
                        attempts.push(EvalAttempt {
                            passed: true,
                            error: None,
                            duration_ms,
                            tools_used: rig.tools_used.clone(),
                            turn_count: rig.turn_count,
                        });
                    }
                    Err(e) => {
                        attempts.push(EvalAttempt {
                            passed: false,
                            error: Some(e.to_string()),
                            duration_ms,
                            tools_used: rig.tools_used.clone(),
                            turn_count: rig.turn_count,
                        });
                    }
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                // Transient API errors get a free retry (don't count as failure).
                if is_transient_error(&err_str) {
                    tracing::warn!("  Transient error (free retry): {err_str}");
                    attempts.push(EvalAttempt {
                        passed: false,
                        error: Some(format!("TRANSIENT: {err_str}")),
                        duration_ms,
                        tools_used: Vec::new(),
                        turn_count: 0,
                    });
                    // Don't count this attempt against the total.
                    continue;
                }

                attempts.push(EvalAttempt {
                    passed: false,
                    error: Some(err_str),
                    duration_ms,
                    tools_used: Vec::new(),
                    turn_count: 0,
                });
            }
        }
    }

    let total = attempts.iter().filter(|a| !is_transient_attempt(a)).count();
    let verdict = if def.policy.passed(passes, total.max(1)) {
        EvalVerdict::Pass
    } else if passes > 0 {
        EvalVerdict::Flaky
    } else {
        EvalVerdict::Fail
    };

    EvalResult {
        name: def.name.to_string(),
        policy: format!("{:?}", def.policy),
        attempts,
        passes,
        total,
        verdict,
    }
}

async fn run_single_attempt(
    def: &EvalDef,
    agent_binary: &str,
    repo_root: &str,
    env: &[(&str, &str)],
) -> Result<TestRig> {
    let mut rig = if let Some(fixture) = def.fixture {
        let fixture_path = Path::new(repo_root).join(fixture);
        TestRig::with_fixture(&fixture_path)?
    } else {
        TestRig::new()?
    };

    rig.run_agent(agent_binary, def.prompt, def.max_turns, env)
        .await
        .context("Agent execution failed")?;

    Ok(rig)
}

fn is_transient_error(err: &str) -> bool {
    let patterns = [
        "rate limit",
        "Rate limit",
        "429",
        "timeout",
        "Timeout",
        "connection reset",
        "connection refused",
        "503",
        "502",
        "overloaded",
    ];
    patterns.iter().any(|p| err.contains(p))
}

fn is_transient_attempt(attempt: &EvalAttempt) -> bool {
    attempt
        .error
        .as_ref()
        .map(|e| e.starts_with("TRANSIENT:"))
        .unwrap_or(false)
}

fn find_repo_root() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}
