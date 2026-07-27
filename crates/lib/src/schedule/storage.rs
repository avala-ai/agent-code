//! Schedule persistence.
//!
//! Each schedule is stored as a JSON file in
//! `~/.config/agent-code/schedules/<name>.json`. The store handles
//! CRUD operations and persists execution history.
//!
//! # Path safety
//!
//! Names flow into filenames, so anything that escapes the schedules
//! directory — `..`, path separators, NUL bytes, control characters —
//! is rejected at the [`ScheduleStore`] boundary by
//! [`validate_schedule_name`]. Tools layered on top (e.g.
//! `CronCreate`, `CronDelete`, `RemoteTrigger`) may add nicer
//! per-tool error messages, but the store enforces the invariant on
//! every public method so a missing tool-side check cannot punch a
//! hole in containment.
//!
//! On top of validation, [`ScheduleStore::save`] writes via a
//! temp-file-plus-rename pattern with `O_NOFOLLOW`-equivalent
//! semantics: a pre-planted symlink at the target is replaced
//! atomically by `rename`, never followed by the underlying write.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// A persisted schedule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique schedule name (used as filename).
    pub name: String,
    /// Cron expression (5-field).
    pub cron: String,
    /// Prompt to send to the agent on each run.
    pub prompt: String,
    /// Working directory for the agent session.
    pub cwd: String,
    /// Whether this schedule is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional permission mode override.
    pub permission_mode: Option<String>,
    /// Maximum cost (USD) per run.
    pub max_cost_usd: Option<f64>,
    /// Maximum turns per run.
    pub max_turns: Option<usize>,
    /// When this schedule was created.
    pub created_at: DateTime<Utc>,
    /// Last execution time (if any).
    pub last_run_at: Option<DateTime<Utc>>,
    /// Last execution result.
    pub last_result: Option<RunResult>,
    /// Webhook secret for HTTP trigger (if set).
    pub webhook_secret: Option<String>,
}

impl Schedule {
    /// Digest of everything that decides *what this routine does* when
    /// fired: the schedule, the prompt, where it runs, and the limits
    /// and overrides it runs under.
    ///
    /// Durable permission grants for id-addressed tools (`RemoteTrigger`,
    /// `CronDelete`) embed this. An id is a mutable handle — `save`
    /// creates *or updates* the record under the same name — so without
    /// it an approval for `{"id":"daily"}` would carry over to a routine
    /// that has since been given a different prompt, cwd, model or
    /// permission mode, and fire it without asking again.
    ///
    /// `created_at` is included so deleting and recreating a routine
    /// under the same id also invalidates the grant. The run bookkeeping
    /// (`last_run_at`, `last_result`) is excluded: it changes on every
    /// execution, and folding it in would re-prompt after each run.
    ///
    /// A digest, not the values: prompts and webhook secrets are
    /// sensitive, and grant keys are persisted into the config directory.
    pub fn binding_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        // Length-prefix every field so adjacent values cannot blur into
        // each other ("ab"+"c" vs "a"+"bc").
        fn part(h: &mut Sha256, bytes: &[u8]) {
            h.update((bytes.len() as u64).to_le_bytes());
            h.update(bytes);
        }
        // Absent and "present but empty/zero" are different routines to
        // the executor — `max_turns: None` becomes the 25-turn default
        // while `Some(0)` permits none — so presence is hashed as its
        // own byte. Folding an absent value to a default (`""`, zero
        // bytes) made the two indistinguishable, and a grant approved
        // for one then covered the other.
        fn opt(h: &mut Sha256, present: bool, bytes: &[u8]) {
            part(h, &[u8::from(present)]);
            part(h, bytes);
        }
        part(&mut h, self.name.as_bytes());
        part(&mut h, self.cron.as_bytes());
        part(&mut h, self.prompt.as_bytes());
        part(&mut h, self.cwd.as_bytes());
        part(&mut h, &[u8::from(self.enabled)]);
        part(&mut h, b"|model|");
        opt(
            &mut h,
            self.model.is_some(),
            self.model.as_deref().unwrap_or("").as_bytes(),
        );
        part(&mut h, b"|mode|");
        opt(
            &mut h,
            self.permission_mode.is_some(),
            self.permission_mode.as_deref().unwrap_or("").as_bytes(),
        );
        part(&mut h, b"|limits|");
        opt(
            &mut h,
            self.max_cost_usd.is_some(),
            self.max_cost_usd
                .map(|v| v.to_bits().to_le_bytes())
                .unwrap_or_default()
                .as_slice(),
        );
        opt(
            &mut h,
            self.max_turns.is_some(),
            self.max_turns
                .map(|v| (v as u64).to_le_bytes())
                .unwrap_or_default()
                .as_slice(),
        );
        part(&mut h, b"|created|");
        part(&mut h, self.created_at.to_rfc3339().as_bytes());
        part(&mut h, b"|secret|");
        opt(
            &mut h,
            self.webhook_secret.is_some(),
            self.webhook_secret.as_deref().unwrap_or("").as_bytes(),
        );
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

fn default_true() -> bool {
    true
}

/// Result of one execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub success: bool,
    pub turns: usize,
    pub cost_usd: f64,
    /// First 500 chars of the response.
    pub summary: String,
    /// Session ID for `/resume`.
    pub session_id: String,
}

/// CRUD operations for schedules.
pub struct ScheduleStore {
    dir: PathBuf,
}

impl ScheduleStore {
    /// Open or create the schedule store.
    pub fn open() -> Result<Self, String> {
        let dir = schedules_dir().ok_or("Could not determine config directory")?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create schedules dir: {e}"))?;
        Ok(Self { dir })
    }

    /// Open a store at a specific directory (for testing).
    pub fn open_at(dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create schedules dir: {e}"))?;
        Ok(Self { dir })
    }

    /// Save a schedule (creates or updates).
    ///
    /// Validates the schedule name as a filename component and writes
    /// atomically: serialize → write to a uniquely-named temp file in
    /// the schedules directory → `rename` over the target. The temp
    /// file is opened with `create_new`, so it cannot land on a
    /// pre-existing symlink. `rename` replaces the target inode
    /// itself, so a symlink planted at `<dir>/<name>.json` is
    /// destroyed rather than followed.
    pub fn save(&self, schedule: &Schedule) -> Result<(), String> {
        let path = self.path_for(&schedule.name)?;
        let json = serde_json::to_string_pretty(schedule)
            .map_err(|e| format!("Serialization error: {e}"))?;
        write_atomic_no_follow(&self.dir, &path, json.as_bytes())?;
        debug!("Schedule saved: {}", path.display());
        Ok(())
    }

    /// Load a schedule by name.
    pub fn load(&self, name: &str) -> Result<Schedule, String> {
        let path = self.path_for(name)?;
        if !path.exists() {
            return Err(format!("Schedule '{name}' not found"));
        }
        let content = std::fs::read_to_string(&path).map_err(|e| format!("Read error: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("Parse error: {e}"))
    }

    /// List all schedules, sorted by name.
    pub fn list(&self) -> Vec<Schedule> {
        let mut schedules: Vec<Schedule> = std::fs::read_dir(&self.dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| {
                let content = std::fs::read_to_string(entry.path()).ok()?;
                serde_json::from_str(&content).ok()
            })
            .collect();
        schedules.sort_by(|a, b| a.name.cmp(&b.name));
        schedules
    }

    /// Remove a schedule by name.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let path = self.path_for(name)?;
        if !path.exists() {
            return Err(format!("Schedule '{name}' not found"));
        }
        std::fs::remove_file(&path).map_err(|e| format!("Delete error: {e}"))?;
        debug!("Schedule removed: {name}");
        Ok(())
    }

    /// Find a schedule by webhook secret.
    pub fn find_by_secret(&self, secret: &str) -> Option<Schedule> {
        self.list()
            .into_iter()
            .find(|s| s.webhook_secret.as_deref() == Some(secret))
    }

    /// Compute the on-disk path for a schedule name, applying
    /// validation and a defense-in-depth containment check.
    ///
    /// Validation rejects names containing path separators, parent
    /// references, NUL bytes, control characters, or anything outside
    /// ASCII-graphic. After joining we verify the resulting path is
    /// still inside the schedules directory by comparing against the
    /// canonicalized parent — this catches edge cases like a name
    /// that passes the lexical check but produces a surprising path
    /// component on the host filesystem.
    fn path_for(&self, name: &str) -> Result<PathBuf, String> {
        validate_schedule_name(name)?;
        let path = self.dir.join(format!("{name}.json"));

        // Defense in depth: the schedules dir may itself live behind a
        // symlink (e.g. `/tmp` on macOS), so we compare canonical
        // forms of the parent rather than insisting on a string
        // prefix of `self.dir`.
        let dir_canonical = std::fs::canonicalize(&self.dir)
            .map_err(|e| format!("Failed to canonicalize schedules dir: {e}"))?;
        let parent = path
            .parent()
            .ok_or_else(|| "Schedule path has no parent".to_string())?;
        let parent_canonical = std::fs::canonicalize(parent)
            .map_err(|e| format!("Failed to canonicalize schedule parent: {e}"))?;
        if parent_canonical != dir_canonical {
            return Err(format!(
                "Schedule name '{name}' resolves outside the schedules directory"
            ));
        }

        Ok(path)
    }
}

/// Maximum permitted length of a schedule name as a filename
/// component.
pub(crate) const MAX_SCHEDULE_NAME_LEN: usize = 64;

/// Validate a schedule name before it is used as a filesystem path
/// component.
///
/// The store derives the on-disk path as `<dir>/<name>.json`, so any
/// input that contains path separators, parent-directory references,
/// NUL bytes, or other control characters could escape the schedules
/// directory or produce surprising filenames. Reject those inputs
/// up-front with a specific error so the caller can self-correct.
///
/// This is the canonical validator: tools layered on top of the
/// store may run an early check for nicer error messages, but the
/// store enforces the invariant on every public method so a missing
/// tool-side check cannot punch a hole in containment.
pub(crate) fn validate_schedule_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Routine name must not be empty.".into());
    }
    if name.len() > MAX_SCHEDULE_NAME_LEN {
        return Err(format!(
            "Routine name must be at most {MAX_SCHEDULE_NAME_LEN} characters (got {}).",
            name.len()
        ));
    }
    if name == "." || name == ".." || name.contains("..") {
        return Err(
            "Routine name must not contain '..' or be a parent-directory reference.".into(),
        );
    }
    for ch in name.chars() {
        if ch == '/' || ch == '\\' {
            return Err(format!(
                "Routine name must not contain path separators ('/' or '\\'); got {ch:?}."
            ));
        }
        if ch == '\0' {
            return Err("Routine name must not contain NUL bytes.".into());
        }
        if ch.is_control() {
            return Err(format!(
                "Routine name must not contain control characters; got {ch:?}."
            ));
        }
        if !ch.is_ascii() || !ch.is_ascii_graphic() {
            return Err(format!(
                "Routine name must be ASCII-printable (letters, digits, '-', '_', '.'); got {ch:?}."
            ));
        }
    }
    Ok(())
}

/// Atomic, symlink-safe write: stage the bytes in a uniquely-named
/// temp file inside `dir` (opened with `create_new`, so it cannot
/// land on a pre-existing symlink), `fsync` it, then `rename` over
/// `target`. `rename` replaces the destination inode, so a symlink
/// pre-planted at `target` is destroyed rather than followed.
///
/// On failure the temp file is best-effort cleaned up.
fn write_atomic_no_follow(dir: &Path, target: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;

    // Build a unique temp name in the same directory so `rename` is
    // atomic (same filesystem). Mix pid + nanos + a counter so
    // concurrent writers don't collide.
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("schedule");
    let tmp_name = format!(".{stem}.{pid}.{nanos}.{counter}.tmp");
    let tmp_path = dir.join(tmp_name);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 0o600: schedules can carry webhook secrets, so keep them
        // user-readable only. NOFOLLOW is implicit for create_new
        // (the file does not exist yet), but set it explicitly to
        // signal intent and harden against TOCTOU.
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }

    let mut file = match options.open(&tmp_path) {
        Ok(f) => f,
        Err(e) => {
            return Err(format!(
                "Failed to create temp file {}: {e}",
                tmp_path.display()
            ));
        }
    };
    if let Err(e) = file.write_all(data) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Write error: {e}"));
    }
    if let Err(e) = file.sync_all() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Sync error: {e}"));
    }
    drop(file);

    if let Err(e) = std::fs::rename(&tmp_path, target) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Rename error: {e}"));
    }
    Ok(())
}

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn schedules_dir() -> Option<PathBuf> {
    crate::config::agent_config_dir().map(|d| d.join("schedules"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_schedule(name: &str) -> Schedule {
        Schedule {
            name: name.to_string(),
            cron: "0 9 * * *".to_string(),
            prompt: "run tests".to_string(),
            cwd: "/tmp/project".to_string(),
            enabled: true,
            model: None,
            permission_mode: None,
            max_cost_usd: None,
            max_turns: None,
            created_at: Utc::now(),
            last_run_at: None,
            last_result: None,
            webhook_secret: None,
        }
    }

    /// Absent and "present but zero/empty" are different routines to the
    /// executor (`max_turns: None` becomes the 25-turn default, `Some(0)`
    /// permits none), so they must not share a grant fingerprint.
    #[test]
    fn absent_and_zero_optionals_fingerprint_differently() {
        let base = test_schedule("routine");
        let mut zero_turns = base.clone();
        zero_turns.max_turns = Some(0);
        assert_ne!(
            base.binding_fingerprint(),
            zero_turns.binding_fingerprint(),
            "max_turns None vs Some(0) collided"
        );

        let mut zero_cost = base.clone();
        zero_cost.max_cost_usd = Some(0.0);
        assert_ne!(
            base.binding_fingerprint(),
            zero_cost.binding_fingerprint(),
            "max_cost_usd None vs Some(0.0) collided"
        );

        // The same hole existed for every optional string: an absent
        // value hashed exactly like a present empty one.
        for mutate in [
            (|s: &mut Schedule| s.model = Some(String::new())) as fn(&mut Schedule),
            |s: &mut Schedule| s.permission_mode = Some(String::new()),
            |s: &mut Schedule| s.webhook_secret = Some(String::new()),
        ] {
            let mut empty = base.clone();
            mutate(&mut empty);
            assert_ne!(
                base.binding_fingerprint(),
                empty.binding_fingerprint(),
                "an absent optional collided with a present empty one"
            );
        }

        // Unchanged input still yields a stable key.
        assert_eq!(
            base.binding_fingerprint(),
            base.clone().binding_fingerprint()
        );
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        let sched = test_schedule("daily-tests");
        store.save(&sched).unwrap();

        let loaded = store.load("daily-tests").unwrap();
        assert_eq!(loaded.name, "daily-tests");
        assert_eq!(loaded.cron, "0 9 * * *");
        assert_eq!(loaded.prompt, "run tests");
        assert!(loaded.enabled);
    }

    #[test]
    fn test_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        store.save(&test_schedule("beta")).unwrap();
        store.save(&test_schedule("alpha")).unwrap();

        let list = store.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alpha"); // sorted
        assert_eq!(list[1].name, "beta");
    }

    #[test]
    fn test_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        store.save(&test_schedule("temp")).unwrap();
        assert!(store.load("temp").is_ok());

        store.remove("temp").unwrap();
        assert!(store.load("temp").is_err());
    }

    #[test]
    fn test_remove_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        assert!(store.remove("nope").is_err());
    }

    #[test]
    fn test_find_by_secret() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        let mut sched = test_schedule("webhook-job");
        sched.webhook_secret = Some("s3cret".to_string());
        store.save(&sched).unwrap();

        let found = store.find_by_secret("s3cret").unwrap();
        assert_eq!(found.name, "webhook-job");
        assert!(store.find_by_secret("wrong").is_none());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut sched = test_schedule("roundtrip");
        sched.model = Some("gpt-5.4".to_string());
        sched.max_cost_usd = Some(1.0);
        sched.max_turns = Some(10);
        sched.last_result = Some(RunResult {
            started_at: Utc::now(),
            finished_at: Utc::now(),
            success: true,
            turns: 3,
            cost_usd: 0.05,
            summary: "All tests passed".to_string(),
            session_id: "abc12345".to_string(),
        });

        let json = serde_json::to_string(&sched).unwrap();
        let loaded: Schedule = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.model.as_deref(), Some("gpt-5.4"));
        assert!(loaded.last_result.unwrap().success);
    }

    #[test]
    fn validate_schedule_name_accepts_normal_id() {
        validate_schedule_name("nightly-cleanup").unwrap();
        validate_schedule_name("cron-abc123").unwrap();
        validate_schedule_name("a.b_c-1").unwrap();
    }

    #[test]
    fn validate_schedule_name_rejects_separators_and_dotdot() {
        assert!(validate_schedule_name("foo/bar").is_err());
        assert!(validate_schedule_name("foo\\bar").is_err());
        assert!(validate_schedule_name("..").is_err());
        assert!(validate_schedule_name("../etc").is_err());
        assert!(validate_schedule_name("").is_err());
        assert!(validate_schedule_name("foo bar").is_err()); // ASCII non-graphic
        assert!(validate_schedule_name("foo\0bar").is_err());
        assert!(validate_schedule_name("foo\nbar").is_err());
    }

    #[test]
    fn store_load_rejects_traversal_at_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();

        // Even if a tool forgot to validate, the store rejects.
        for bad in [
            "../escape",
            "foo/bar",
            "foo\\bar",
            "..",
            "",
            "tab\tname",
            "with space",
        ] {
            assert!(store.load(bad).is_err(), "load should reject {bad:?}");
            assert!(store.remove(bad).is_err(), "remove should reject {bad:?}");
        }
    }

    #[test]
    fn store_save_rejects_traversal_at_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();

        let mut sched = test_schedule("..");
        let err = store.save(&sched).unwrap_err();
        assert!(err.contains("'..'") || err.contains("parent"), "got: {err}");

        sched.name = "foo/bar".into();
        assert!(store.save(&sched).is_err());
    }

    /// A pre-planted symlink at the target path must NOT be followed
    /// by `save`. Verify the file the symlink points to is unchanged
    /// after `save` runs.
    #[cfg(unix)]
    #[test]
    fn save_does_not_follow_pre_planted_symlink() {
        use std::os::unix::fs::symlink;

        let outer = tempfile::tempdir().unwrap();
        let target_outside = outer.path().join("forbidden.txt");
        std::fs::write(&target_outside, b"original-secret").unwrap();

        let dir = outer.path().join("schedules");
        std::fs::create_dir_all(&dir).unwrap();
        // Plant a symlink: <dir>/nightly.json -> ../forbidden.txt
        let symlink_path = dir.join("nightly.json");
        symlink(&target_outside, &symlink_path).unwrap();

        let store = ScheduleStore::open_at(dir.clone()).unwrap();
        let sched = test_schedule("nightly");
        store.save(&sched).expect("save should succeed");

        // The forbidden file outside the dir is untouched.
        let after = std::fs::read_to_string(&target_outside).unwrap();
        assert_eq!(
            after, "original-secret",
            "symlink target outside the schedules dir was clobbered"
        );

        // The schedule file inside the dir now contains the schedule
        // JSON (the symlink itself was replaced by `rename`).
        let new_meta = std::fs::symlink_metadata(&symlink_path).unwrap();
        assert!(
            new_meta.file_type().is_file(),
            "expected a regular file after save, got {:?}",
            new_meta.file_type()
        );
        let on_disk = std::fs::read_to_string(&symlink_path).unwrap();
        assert!(
            on_disk.contains("\"nightly\""),
            "schedule JSON should be present at the target"
        );
    }
}
