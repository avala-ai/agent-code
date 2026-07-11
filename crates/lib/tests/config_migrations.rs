//! Integration tests for the settings migrations framework.
//!
//! Each test stages a JSON fixture in a tempdir, drives
//! `load_and_migrate`, and asserts both the in-memory result and the
//! on-disk side effects (rewrite, atomic temp file gone, backup
//! rotation, rollback on failure).
//!
//! Fixtures live in `tests/fixtures/config_migrations/`. Goldens are
//! checked verbatim against `serde_json::to_value` of the parsed file
//! so the tests are stable across whitespace differences in pretty
//! printing.

use std::fs;
use std::path::{Path, PathBuf};

use agent_code_lib::config::migrations::{
    CURRENT_SCHEMA_VERSION, MAX_BACKUPS, Migration, SCHEMA_VERSION_KEY, backup_path,
    load_and_migrate, load_and_migrate_toml, load_and_migrate_with, registered_migrations,
    run_migrations,
};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tempfile::TempDir;

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("config_migrations")
        .join(name)
}

fn read_fixture(name: &str) -> String {
    fs::read_to_string(fixture_path(name)).unwrap_or_else(|e| panic!("reading fixture {name}: {e}"))
}

fn read_fixture_value(name: &str) -> Value {
    serde_json::from_str(&read_fixture(name))
        .unwrap_or_else(|e| panic!("parsing fixture {name} as JSON: {e}"))
}

fn stage_settings(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("settings.json");
    fs::write(&path, contents).expect("staging settings.json");
    path
}

// ---- Golden migrations ----

#[test]
fn v0_file_without_schema_version_migrates_to_current_and_rewrites() {
    let tmp = TempDir::new().unwrap();
    let path = stage_settings(tmp.path(), &read_fixture("v0_input.json"));

    let (value, outcome) = load_and_migrate(&path).expect("migrate succeeds");

    assert_eq!(outcome.from_version, 0);
    assert_eq!(outcome.to_version, CURRENT_SCHEMA_VERSION);
    assert!(outcome.rewrote, "v0 file should be rewritten");
    assert_eq!(
        outcome.backup_path.as_deref(),
        Some(backup_path(&path, 1).as_path())
    );

    // In-memory matches golden.
    let expected = read_fixture_value("v0_to_current_expected.json");
    assert_eq!(value, expected);

    // On-disk file matches golden after parsing back (whitespace
    // is normalized via the JSON value comparison).
    let on_disk: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk, expected);

    // Backup contains the original pre-migration bytes.
    let bak = fs::read_to_string(backup_path(&path, 1)).unwrap();
    assert_eq!(bak.trim(), read_fixture("v0_input.json").trim());

    // Atomic temp file must not survive a successful migration.
    assert!(
        !path.with_file_name("settings.json.tmp").exists(),
        "temp file was not cleaned up by atomic rename"
    );
}

#[test]
fn v1_file_migrates_to_current() {
    let tmp = TempDir::new().unwrap();
    let path = stage_settings(tmp.path(), &read_fixture("v1_input.json"));

    let (value, outcome) = load_and_migrate(&path).expect("migrate succeeds");

    assert_eq!(outcome.from_version, 1);
    assert_eq!(outcome.to_version, CURRENT_SCHEMA_VERSION);
    assert!(outcome.rewrote);

    let expected = read_fixture_value("v1_to_current_expected.json");
    assert_eq!(value, expected);
}

// ---- No-op fast path ----

#[test]
fn current_version_file_is_not_rewritten_and_no_backup_is_created() {
    let tmp = TempDir::new().unwrap();
    let original = read_fixture("current_input.json");
    let path = stage_settings(tmp.path(), &original);
    let mtime_before = fs::metadata(&path).unwrap().modified().unwrap();

    // Sleep-free: instead of timing-based assertions, check that no
    // backups appear and that the bytes on disk are byte-identical
    // to what we wrote (rewrite would re-pretty-print).
    let (value, outcome) = load_and_migrate(&path).expect("no-op load succeeds");

    assert_eq!(outcome.from_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(outcome.to_version, CURRENT_SCHEMA_VERSION);
    assert!(
        !outcome.rewrote,
        "current-version file must not be rewritten"
    );
    assert!(outcome.backup_path.is_none());

    // No backup file should appear.
    for n in 1..=MAX_BACKUPS {
        assert!(
            !backup_path(&path, n).exists(),
            "unexpected backup at slot {n}"
        );
    }

    // File bytes are exactly what we wrote.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, original);

    // The mtime should also be untouched.
    let mtime_after = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(mtime_before, mtime_after);

    assert_eq!(value[SCHEMA_VERSION_KEY], json!(CURRENT_SCHEMA_VERSION));
}

// ---- Backup rotation ----

#[test]
fn four_loads_keep_only_three_backups() {
    // Each "load" simulates a separate run that finds the file in a
    // pre-current state and migrates it. We intentionally migrate the
    // same file four times, so we can observe rotation behavior.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.json");

    for i in 0..4 {
        // Force a v0 file each iteration so a rewrite + backup happens.
        // Vary one field so backups have distinguishable contents.
        let body = format!("{{\n  \"api\": {{ \"model\": \"m-{i}\", \"token\": \"t-{i}\" }}\n}}\n");
        fs::write(&path, &body).unwrap();
        let (_value, outcome) = load_and_migrate(&path).expect("migrate succeeds");
        assert!(outcome.rewrote);
    }

    // Exactly three backup slots survive.
    for n in 1..=MAX_BACKUPS {
        assert!(
            backup_path(&path, n).exists(),
            "expected backup at slot {n}"
        );
    }
    let beyond = backup_path(&path, MAX_BACKUPS + 1);
    assert!(
        !beyond.exists(),
        "rotation should have dropped slot {} ({})",
        MAX_BACKUPS + 1,
        beyond.display()
    );

    // Slot 1 is freshest (the most recent pre-migration body, model "m-3").
    let bak1 = fs::read_to_string(backup_path(&path, 1)).unwrap();
    assert!(
        bak1.contains("\"m-3\""),
        "bak.1 should hold the latest pre-migration body, got {bak1:?}"
    );

    // Slot 3 is oldest survivor (model "m-1"). The original "m-0"
    // archive was rotated out and dropped.
    let bak3 = fs::read_to_string(backup_path(&path, 3)).unwrap();
    assert!(
        bak3.contains("\"m-1\""),
        "bak.3 should hold the oldest survivor, got {bak3:?}"
    );
}

#[test]
fn backup_paths_sit_next_to_settings_file() {
    let tmp = TempDir::new().unwrap();
    let path = stage_settings(tmp.path(), &read_fixture("v0_input.json"));
    let _ = load_and_migrate(&path).unwrap();

    let bak1 = backup_path(&path, 1);
    assert_eq!(bak1.parent(), path.parent());
    assert_eq!(bak1.file_name().unwrap(), "settings.json.bak.1");
}

// ---- Failure rollback ----

/// A stub migration that always errors. Used to drive the failure path
/// of `load_and_migrate` without needing a real schema bump.
struct AlwaysFails;

impl Migration for AlwaysFails {
    fn from_version(&self) -> u32 {
        0
    }
    fn to_version(&self) -> u32 {
        1
    }
    fn description(&self) -> &'static str {
        "test-only failing migration"
    }
    fn migrate(&self, _value: &mut Value) -> Result<()> {
        Err(anyhow!("synthetic failure"))
    }
}

#[test]
fn migration_failure_does_not_corrupt_file_or_create_backup() {
    let tmp = TempDir::new().unwrap();
    let original = "{ \"api\": { \"model\": \"untouched\" } }\n";
    let path = stage_settings(tmp.path(), original);

    let migrations: Vec<Box<dyn Migration>> = vec![Box::new(AlwaysFails)];
    let err = load_and_migrate_with(&path, &migrations).unwrap_err();
    assert!(
        format!("{err:#}").contains("synthetic failure"),
        "expected migration error, got: {err:#}"
    );

    // Original bytes intact.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(
        after, original,
        "file must not be touched on migration failure"
    );

    // No backups, no temp turds.
    for n in 1..=MAX_BACKUPS {
        assert!(
            !backup_path(&path, n).exists(),
            "no backup should be created when migration fails"
        );
    }
    assert!(!path.with_file_name("settings.json.tmp").exists());
}

#[test]
fn parse_failure_surfaces_error_without_touching_file() {
    let tmp = TempDir::new().unwrap();
    let bad = "{not valid json";
    let path = stage_settings(tmp.path(), bad);

    let err = load_and_migrate(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("parsing settings file"),
        "unexpected error: {msg}"
    );

    // File untouched on parse failure.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, bad);
    assert!(!backup_path(&path, 1).exists());
}

// ---- Downgrade detection ----

#[test]
fn higher_schema_version_is_rejected_with_clear_error() {
    let tmp = TempDir::new().unwrap();
    let body = format!("{{ \"schema_version\": {} }}", CURRENT_SCHEMA_VERSION + 7);
    let path = stage_settings(tmp.path(), &body);

    let err = load_and_migrate(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Refusing to load") && msg.contains("upgrade agent-code"),
        "expected downgrade error, got: {msg}"
    );

    // Refusal must not rewrite or back up.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, body);
    assert!(!backup_path(&path, 1).exists());
}

// ---- Pure run_migrations: in-memory only ----

#[test]
fn run_migrations_pure_v0_to_current() {
    let mut v: Value = serde_json::from_str(&read_fixture("v0_input.json")).unwrap();
    let mutated = run_migrations(&mut v, &registered_migrations()).unwrap();
    assert!(mutated);
    let expected = read_fixture_value("v0_to_current_expected.json");
    assert_eq!(v, expected);
}

#[test]
fn run_migrations_pure_no_op_on_current_value() {
    let mut v = read_fixture_value("current_input.json");
    let before = v.clone();
    let mutated = run_migrations(&mut v, &registered_migrations()).unwrap();
    assert!(!mutated);
    assert_eq!(v, before);
}

// ---- TOML runner ----

#[test]
fn toml_v0_file_migrates_to_current_and_rewrites_with_schema_version() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.toml");
    fs::write(
        &path,
        "[api]\nbase_url = \"https://api.example.com/v1\"\nmodel = \"gpt-5.4\"\ntoken = \"sk-legacy\"\n",
    )
    .unwrap();

    let (value, outcome) = load_and_migrate_toml(&path).expect("toml migrate succeeds");

    assert_eq!(outcome.from_version, 0);
    assert_eq!(outcome.to_version, CURRENT_SCHEMA_VERSION);
    assert!(outcome.rewrote);
    assert_eq!(
        outcome.backup_path.as_deref(),
        Some(backup_path(&path, 1).as_path())
    );

    // schema_version stamped in the rewritten file.
    let schema_version_in_value = value
        .get("schema_version")
        .and_then(|v| v.as_integer())
        .expect("schema_version present in migrated value");
    assert_eq!(schema_version_in_value as u32, CURRENT_SCHEMA_VERSION);

    // api.token renamed to api.api_key.
    let api_key = value
        .get("api")
        .and_then(|v| v.get("api_key"))
        .and_then(|v| v.as_str());
    assert_eq!(api_key, Some("sk-legacy"));
    let token_present = value.get("api").and_then(|v| v.get("token")).is_some();
    assert!(!token_present);

    // On disk also reflects the migration.
    let on_disk: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        on_disk
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .unwrap() as u32,
        CURRENT_SCHEMA_VERSION
    );

    // Backup was written with the original pre-migration TOML bytes.
    let bak1 = fs::read_to_string(backup_path(&path, 1)).unwrap();
    assert!(bak1.contains("token = \"sk-legacy\""));

    // Atomic temp file does not survive a successful migration.
    assert!(!path.with_file_name("settings.toml.tmp").exists());
}

#[test]
fn toml_current_version_file_is_not_rewritten() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.toml");
    let body = format!("schema_version = {CURRENT_SCHEMA_VERSION}\n\n[api]\nmodel = \"gpt-5.4\"\n");
    fs::write(&path, &body).unwrap();

    let (_value, outcome) = load_and_migrate_toml(&path).expect("noop succeeds");
    assert!(!outcome.rewrote);
    assert!(outcome.backup_path.is_none());

    // No backup, original bytes intact.
    assert!(!backup_path(&path, 1).exists());
    assert_eq!(fs::read_to_string(&path).unwrap(), body);
}

// ---- Backup-rotation order: rotation must run AFTER successful write ----

// POSIX-only: relies on directory mode bits to simulate the atomic
// write failing. Windows has no equivalent way to make a directory
// reject child-file creation here, so the assertion would not hold.
#[cfg(unix)]
#[test]
fn backup_rotation_does_not_run_when_atomic_write_fails() {
    // Simulate an atomic-write failure by making the parent directory
    // read-only after staging the live file. The temp-file open will
    // fail with EACCES, the rename never happens, and crucially the
    // backup chain must not have been touched.
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.toml");

    // Stage a v0 file plus a pre-existing .bak.1 with known contents.
    fs::write(
        &path,
        "[api]\nmodel = \"current-live\"\ntoken = \"sk-live\"\n",
    )
    .unwrap();
    let bak1 = backup_path(&path, 1);
    let prior_bak1_body = "[api]\nmodel = \"prior-backup\"\n";
    fs::write(&bak1, prior_bak1_body).unwrap();

    // Drop directory permissions so the runner can't create the temp
    // file. The live file remains readable since we already opened it
    // for read on the way in.
    let dir_perms_before = fs::metadata(tmp.path()).unwrap().permissions();
    let mut readonly = dir_perms_before.clone();
    readonly.set_mode(0o555);
    fs::set_permissions(tmp.path(), readonly).unwrap();

    // Root (and some sandboxes) bypass directory write bits, so the
    // failure this test depends on never happens — probe and skip
    // instead of failing on such runners.
    if fs::write(tmp.path().join(".probe"), b"x").is_ok() {
        fs::set_permissions(tmp.path(), dir_perms_before).unwrap();
        eprintln!("skipping: read-only dir not enforced (running as root?)");
        return;
    }

    let res = load_and_migrate_toml(&path);

    // Restore directory perms so the TempDir cleanup (and subsequent
    // assertions) can proceed regardless of the test outcome.
    fs::set_permissions(tmp.path(), dir_perms_before).unwrap();

    let err = res.expect_err("write should have failed under read-only dir");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("opening temp file") || msg.contains("Permission denied"),
        "expected temp-file write failure, got: {msg}"
    );

    // Live file untouched.
    let live_after = fs::read_to_string(&path).unwrap();
    assert!(live_after.contains("\"sk-live\""));

    // Critical invariant: prior `.bak.1` must still hold its original
    // bytes — the rotation must not have run.
    let bak1_after = fs::read_to_string(&bak1).unwrap();
    assert_eq!(
        bak1_after, prior_bak1_body,
        ".bak.1 should be untouched when the new write failed"
    );

    // No bak.2 should have been created from the rotation either.
    assert!(
        !backup_path(&path, 2).exists(),
        ".bak.2 should not appear when rotation never ran"
    );
}

// ---- Production wiring: Config::load picks up migrations ----

#[test]
fn config_load_runs_migrations_against_project_settings_toml() {
    // This test exercises the full `Config::load` path against a v0
    // `.agent/settings.toml` planted in a tempdir that we make the
    // current working directory. The migration runner should rewrite
    // the file with `schema_version = CURRENT_SCHEMA_VERSION` and the
    // legacy `api.token` field should be picked up as `api.api_key`
    // through the typed schema.
    use agent_code_lib::config::Config;

    // Serialize against any other test in this binary that might mess
    // with cwd — we set/unset process-global state here.
    static CWD_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = CWD_GUARD.lock().unwrap_or_else(|e| e.into_inner());

    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let settings = agent_dir.join("settings.toml");
    fs::write(
        &settings,
        "[api]\nmodel = \"legacy-gpt\"\ntoken = \"sk-from-token-field\"\n",
    )
    .unwrap();

    let prev_cwd = std::env::current_dir().ok();
    // Some env vars can override loaded settings; clear the relevant
    // ones to avoid masking the migration behavior under test.
    let saved_env: Vec<(&str, Option<String>)> = [
        "AGENT_CODE_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "AGENT_CODE_MODEL",
        "AGENT_CODE_API_BASE_URL",
        "AGENT_CODE_AUTH_MODE",
    ]
    .into_iter()
    .map(|k| (k, std::env::var(k).ok()))
    .collect();
    for (k, _) in &saved_env {
        // SAFETY: protected by CWD_GUARD; other tests in this binary
        // serialize on the same mutex so env-var writes don't race.
        unsafe {
            std::env::remove_var(k);
        }
    }

    std::env::set_current_dir(tmp.path()).unwrap();

    let load_result = Config::load();

    // Restore cwd and env vars before asserting.
    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }
    for (k, v) in saved_env {
        // SAFETY: protected by CWD_GUARD.
        unsafe {
            if let Some(val) = v {
                std::env::set_var(k, val);
            } else {
                std::env::remove_var(k);
            }
        }
    }

    let cfg = load_result.expect("Config::load succeeds");
    assert_eq!(cfg.api.model, "legacy-gpt", "model survived the migration");
    assert_eq!(
        cfg.api.api_key.as_deref(),
        Some("sk-from-token-field"),
        "v1→v2 migration should have moved api.token into api.api_key"
    );

    // The on-disk file was rewritten with the schema version stamp.
    let after: toml::Value = toml::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        after
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .map(|n| n as u32),
        Some(CURRENT_SCHEMA_VERSION),
        "settings.toml should be rewritten with schema_version stamped"
    );
    let api_table = after.get("api").and_then(|v| v.as_table()).unwrap();
    assert!(
        !api_table.contains_key("token"),
        "legacy token field should have been removed"
    );
    assert_eq!(
        api_table.get("api_key").and_then(|v| v.as_str()),
        Some("sk-from-token-field")
    );

    // `.bak.1` holds the pre-migration body.
    let bak1 = fs::read_to_string(backup_path(&settings, 1)).unwrap();
    assert!(bak1.contains("token = \"sk-from-token-field\""));
}

// ---- Permission-mode preservation (POSIX-only) ----

/// A pre-existing 0600 settings file must keep its 0600 mode after a
/// migration rewrite — losing the restrictive mode would leak API
/// keys to anyone with read access on the directory.
#[cfg(unix)]
#[test]
fn migration_preserves_existing_0600_permissions_on_rewrite() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.toml");
    fs::write(&path, "[api]\nmodel = \"legacy\"\ntoken = \"sk-secret\"\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    let (_value, outcome) = load_and_migrate_toml(&path).expect("migrate succeeds");
    assert!(outcome.rewrote);

    let mode_after = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode_after, 0o600,
        "rewritten live file should keep its restrictive 0600 mode"
    );

    // Backup must also be 0600 — otherwise the rotation step turns
    // every old secrets file into a world-readable artifact.
    let bak1 = backup_path(&path, 1);
    let bak_mode = fs::metadata(&bak1).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        bak_mode, 0o600,
        ".bak.1 should inherit the live file's mode"
    );
}

/// When the live file already exists with 0644, the runner should
/// preserve that too — we mirror what we found, not force 0600.
#[cfg(unix)]
#[test]
fn migration_preserves_existing_0644_permissions_on_rewrite() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.toml");
    fs::write(&path, "[api]\nmodel = \"legacy\"\ntoken = \"sk-x\"\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let _ = load_and_migrate_toml(&path).expect("migrate succeeds");
    let mode_after = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode_after, 0o644, "rewritten live file should keep 0644");
}

/// A pre-planted symlink at the deterministic `<path>.tmp` slot must
/// not be followed and truncated — the runner uses an unguessable
/// temp-file name in the same directory instead.
#[cfg(unix)]
#[test]
fn migration_does_not_truncate_pre_planted_tmp_symlink() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.toml");
    fs::write(&path, "[api]\nmodel = \"legacy\"\ntoken = \"sk-x\"\n").unwrap();

    // Plant a "victim" file the symlink will dangle to, with known
    // contents we'll assert remain intact.
    let victim = tmp.path().join("victim.secret");
    let victim_body = "important-do-not-truncate";
    fs::write(&victim, victim_body).unwrap();

    let planted_tmp = path.with_file_name("settings.toml.tmp");
    std::os::unix::fs::symlink(&victim, &planted_tmp).unwrap();

    let _ = load_and_migrate_toml(&path).expect("migrate succeeds despite planted symlink");

    // Victim is byte-identical — old code would have truncated it
    // when opening `<path>.tmp` for write with O_TRUNC.
    let victim_after = fs::read_to_string(&victim).unwrap();
    assert_eq!(
        victim_after, victim_body,
        "planted symlink target must not be truncated"
    );

    // The symlink itself can either survive or be removed by the
    // runner; what matters is that the victim is intact and the
    // live file is correctly migrated.
    let live: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        live.get("schema_version").and_then(|v| v.as_integer()),
        Some(i64::from(CURRENT_SCHEMA_VERSION))
    );
}
