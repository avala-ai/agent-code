//! E2E tests for the `agent schedule` subcommand.
//!
//! These tests validate the full schedule lifecycle via the compiled
//! binary. No API key required — only tests CRUD operations that
//! don't invoke the LLM.
//!
//! # Windows isolation
//!
//! Several tests that assert empty state or per-test counts are
//! `#[cfg_attr(target_os = "windows", ignore)]` because the `dirs`
//! crate on Windows resolves `config_dir()` via the
//! `SHGetKnownFolderPath` API (`FOLDERID_RoamingAppData`) rather than
//! reading `$HOME` / `$XDG_CONFIG_HOME`. That means `agent_with_home`
//! cannot redirect the schedules directory on Windows, so parallel
//! test processes share the real user profile and clobber each
//! other's schedules. Proper fix is an `AGENT_CODE_CONFIG_DIR`
//! override plumbed through every `dirs::config_dir()` call — tracked
//! separately. Until then, these tests still run on Linux CI where
//! they are the source of truth.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn agent() -> Command {
    Command::cargo_bin("agent").expect("binary should exist")
}

/// Set HOME to a temp dir so schedule files don't pollute the real config.
/// Creates a minimal config.toml so the setup wizard doesn't trigger.
fn agent_with_home(home: &TempDir) -> Command {
    // Create config dir and a minimal config so the setup wizard is skipped.
    let config_dir = home.path().join(".config").join("agent-code");
    std::fs::create_dir_all(&config_dir).unwrap();
    if !config_dir.join("config.toml").exists() {
        std::fs::write(config_dir.join("config.toml"), "# minimal\n").unwrap();
    }

    let mut cmd = agent();
    cmd.env("HOME", home.path());
    cmd.env("XDG_CONFIG_HOME", home.path().join(".config"));
    cmd
}

// ---- Help & discovery ----

#[test]
fn schedule_help() {
    agent()
        .args(["schedule", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("add"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("remove"));
}

#[test]
fn daemon_help() {
    agent()
        .args(["daemon", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("webhook-port"));
}

// ---- CRUD lifecycle ----

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "dirs::config_dir doesn't honor $HOME on Windows — see module docs"
)]
fn schedule_list_empty() {
    let home = TempDir::new().unwrap();
    agent_with_home(&home)
        .args(["schedule", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No schedules configured"));
}

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "dirs::config_dir doesn't honor $HOME on Windows — see module docs"
)]
fn schedule_add_and_list() {
    let home = TempDir::new().unwrap();

    // Add a schedule.
    agent_with_home(&home)
        .args([
            "schedule",
            "add",
            "0 9 * * *",
            "--prompt",
            "run tests",
            "--name",
            "daily-tests",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created schedule 'daily-tests'"))
        .stdout(predicate::str::contains("Cron: 0 9 * * *"));

    // List should show it.
    agent_with_home(&home)
        .args(["schedule", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("daily-tests"))
        .stdout(predicate::str::contains("active"))
        .stdout(predicate::str::contains("0 9 * * *"))
        .stdout(predicate::str::contains("1 schedule(s)"));
}

#[test]
fn schedule_add_with_webhook() {
    let home = TempDir::new().unwrap();

    agent_with_home(&home)
        .args([
            "schedule",
            "add",
            "*/30 * * * *",
            "--prompt",
            "check health",
            "--name",
            "health-check",
            "--webhook",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created schedule 'health-check'"))
        .stdout(predicate::str::contains("Webhook: POST /trigger?secret="));
}

#[test]
fn schedule_add_with_options() {
    let home = TempDir::new().unwrap();

    agent_with_home(&home)
        .args([
            "schedule",
            "add",
            "0 */6 * * *",
            "--prompt",
            "security scan",
            "--name",
            "sec-scan",
            "--model",
            "gpt-5.4",
            "--max-cost",
            "0.50",
            "--max-turns",
            "10",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created schedule 'sec-scan'"));

    // Verify it persisted (shows in list).
    agent_with_home(&home)
        .args(["schedule", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sec-scan"))
        .stdout(predicate::str::contains("0 */6 * * *"));
}

#[test]
fn schedule_add_invalid_cron() {
    let home = TempDir::new().unwrap();

    agent_with_home(&home)
        .args([
            "schedule", "add", "bad cron", "--prompt", "test", "--name", "bad",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid cron expression"));
}

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "dirs::config_dir doesn't honor $HOME on Windows — see module docs"
)]
fn schedule_remove() {
    let home = TempDir::new().unwrap();

    // Add then remove.
    agent_with_home(&home)
        .args([
            "schedule",
            "add",
            "0 9 * * *",
            "--prompt",
            "test",
            "--name",
            "temp",
        ])
        .assert()
        .success();

    agent_with_home(&home)
        .args(["schedule", "remove", "temp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed schedule 'temp'"));

    // List should be empty again.
    agent_with_home(&home)
        .args(["schedule", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No schedules configured"));
}

#[test]
fn schedule_remove_nonexistent() {
    let home = TempDir::new().unwrap();

    agent_with_home(&home)
        .args(["schedule", "remove", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn schedule_disable_and_enable() {
    let home = TempDir::new().unwrap();

    // Add a schedule.
    agent_with_home(&home)
        .args([
            "schedule",
            "add",
            "0 9 * * *",
            "--prompt",
            "test",
            "--name",
            "toggle-me",
        ])
        .assert()
        .success();

    // Disable it.
    agent_with_home(&home)
        .args(["schedule", "disable", "toggle-me"])
        .assert()
        .success()
        .stdout(predicate::str::contains("disabled"));

    // Should show as paused in list.
    agent_with_home(&home)
        .args(["schedule", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("paused"));

    // Re-enable.
    agent_with_home(&home)
        .args(["schedule", "enable", "toggle-me"])
        .assert()
        .success()
        .stdout(predicate::str::contains("enabled"));

    // Should show as active again.
    agent_with_home(&home)
        .args(["schedule", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("active"));
}

#[test]
fn schedule_rm_alias() {
    let home = TempDir::new().unwrap();

    agent_with_home(&home)
        .args([
            "schedule",
            "add",
            "0 0 * * *",
            "--prompt",
            "x",
            "--name",
            "alias-test",
        ])
        .assert()
        .success();

    // Use the `rm` alias.
    agent_with_home(&home)
        .args(["schedule", "rm", "alias-test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed"));
}

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "dirs::config_dir doesn't honor $HOME on Windows — see module docs"
)]
fn schedule_ls_alias() {
    let home = TempDir::new().unwrap();

    agent_with_home(&home)
        .args(["schedule", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No schedules configured"));
}

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "dirs::config_dir doesn't honor $HOME on Windows — see module docs"
)]
fn schedule_multiple_entries() {
    let home = TempDir::new().unwrap();

    for (name, cron) in [
        ("alpha", "0 6 * * *"),
        ("beta", "0 12 * * *"),
        ("gamma", "0 18 * * *"),
    ] {
        agent_with_home(&home)
            .args(["schedule", "add", cron, "--prompt", "test", "--name", name])
            .assert()
            .success();
    }

    agent_with_home(&home)
        .args(["schedule", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("alpha"))
        .stdout(predicate::str::contains("beta"))
        .stdout(predicate::str::contains("gamma"))
        .stdout(predicate::str::contains("3 schedule(s)"));
}

// ---- Run without API key should fail gracefully ----

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "dirs::config_dir doesn't honor $HOME on Windows — see module docs"
)]
fn schedule_run_no_api_key() {
    let home = TempDir::new().unwrap();

    agent_with_home(&home)
        .args([
            "schedule",
            "add",
            "0 9 * * *",
            "--prompt",
            "test",
            "--name",
            "run-me",
        ])
        .assert()
        .success();

    // Running requires an API key — should fail, not panic.
    agent_with_home(&home)
        .env_remove("AGENT_CODE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args(["schedule", "run", "run-me"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("API key"));
}
