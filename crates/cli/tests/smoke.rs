//! Smoke tests — verify the compiled binary starts and responds to basic flags.
//!
//! These tests must pass in CI without any API keys configured.
//! Tests that require an API key are skipped when none is available.

use assert_cmd::Command;
use predicates::prelude::*;

fn agent() -> Command {
    Command::cargo_bin("agent").expect("binary should exist")
}

#[test]
fn version_flag() {
    agent()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("agent"));
}

#[test]
fn help_flag() {
    agent()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("AI"))
        .stdout(predicate::str::contains("--model"))
        .stdout(predicate::str::contains("--prompt"));
}

#[test]
fn unknown_flag_fails() {
    agent()
        .arg("--this-flag-does-not-exist")
        .assert()
        .failure()
        .stderr(predicate::str::contains("error").or(predicate::str::contains("unexpected")));
}

#[test]
fn permissions_overlay_flag_appears_in_help() {
    agent()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--permissions-overlay"));
}
