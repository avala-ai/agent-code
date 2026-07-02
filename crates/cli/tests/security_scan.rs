//! Integration tests for `agent security-scan` (Agentic MapReduce).
//!
//! These are hermetic — they must pass in CI with no API key and no
//! network. The clean-repo scan reaches the deterministic shard stage,
//! finds zero signals, and short-circuits before any LLM worker runs, so
//! the dummy key is never used. The full MAP/REDUCE path against a live
//! model is covered by section N of `scripts/e2e-tests.sh`.

use assert_cmd::Command;
use predicates::prelude::*;

/// A key value good enough to build a provider object. It is never used
/// because the clean-repo scans below make no LLM calls.
const DUMMY_KEY: &str = "test-dummy-key-not-used";

fn agent() -> Command {
    Command::cargo_bin("agent").expect("binary should exist")
}

#[test]
fn security_scan_help_lists_scan_flags() {
    agent()
        .args(["security-scan", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--batch-size"))
        .stdout(predicate::str::contains("--severity-threshold"))
        .stdout(predicate::str::contains("--incremental"))
        .stdout(predicate::str::contains("--format"))
        .stdout(predicate::str::contains("--map-model"));
}

#[test]
fn security_scan_appears_in_top_level_help() {
    agent()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("security-scan"));
}

#[test]
fn clean_repo_reports_no_findings_offline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("safe.py"),
        "def add(a, b):\n    return a + b\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("README.md"), "# just docs\n").unwrap();

    let assert = agent()
        .arg("security-scan")
        .arg(dir.path())
        .args(["--format", "json"])
        .env("AGENT_CODE_API_KEY", DUMMY_KEY)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("scan should print a valid JSON report");

    assert_eq!(report["profile"].as_str(), Some("security"));
    assert_eq!(
        report["findings"].as_array().map(|a| a.len()),
        Some(0),
        "a clean repo has no findings"
    );
    assert_eq!(
        report["shards"].as_u64(),
        Some(0),
        "no signals means no MAP shards"
    );
    assert!(
        report["dropped_files"].as_u64().unwrap_or(0) >= 1,
        "clean files should be dropped before MAP"
    );
    assert_eq!(
        report["cost_usd"].as_f64(),
        Some(0.0),
        "no workers, no cost"
    );
}

#[test]
fn markdown_format_renders_no_findings_offline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("safe.py"), "x = 1\n").unwrap();

    agent()
        .arg("security-scan")
        .arg(dir.path())
        .args(["--format", "markdown"])
        .env("AGENT_CODE_API_KEY", DUMMY_KEY)
        .assert()
        .success()
        .stdout(predicate::str::contains("# Security scan"))
        .stdout(predicate::str::contains("_No findings._"));
}

#[test]
fn invalid_severity_threshold_fails() {
    let dir = tempfile::tempdir().unwrap();
    agent()
        .arg("security-scan")
        .arg(dir.path())
        .args(["--severity-threshold", "NOPE"])
        .env("AGENT_CODE_API_KEY", DUMMY_KEY)
        .assert()
        .failure()
        .stderr(predicate::str::contains("severity"));
}

#[test]
fn unknown_profile_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("safe.py"), "x = 1\n").unwrap();
    agent()
        .arg("security-scan")
        .arg(dir.path())
        .args(["--profile", "does-not-exist"])
        .env("AGENT_CODE_API_KEY", DUMMY_KEY)
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown profile"));
}

#[test]
fn invalid_format_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("safe.py"), "x = 1\n").unwrap();
    agent()
        .arg("security-scan")
        .arg(dir.path())
        .args(["--format", "xml"])
        .env("AGENT_CODE_API_KEY", DUMMY_KEY)
        .assert()
        .failure()
        .stderr(predicate::str::contains("format"));
}
