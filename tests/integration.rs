//! Integration tests for the rc binary.

use std::process::Command;

#[test]
fn test_version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_rc"))
        .arg("--version")
        .output()
        .expect("Failed to run rc");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rc"));
}

#[test]
fn test_help_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_rc"))
        .arg("--help")
        .output()
        .expect("Failed to run rc");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AI-powered"));
    assert!(stdout.contains("--prompt"));
}

#[test]
fn test_dump_system_prompt() {
    // This should work even without an API key since it exits before connecting.
    let output = Command::new(env!("CARGO_BIN_EXE_rc"))
        .arg("--dump-system-prompt")
        .env("RC_API_KEY", "test-key")
        .output()
        .expect("Failed to run rc");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AI coding agent"));
    assert!(stdout.contains("Available Tools"));
}

#[test]
fn test_missing_api_key_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_rc"))
        .arg("--prompt")
        .arg("test")
        .env_remove("RC_API_KEY")
        .output()
        .expect("Failed to run rc");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("API key"));
}
