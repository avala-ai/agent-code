//! Session persistence.
//!
//! Saves and restores conversation state across sessions. Each session
//! gets a unique ID and is stored as a JSON file in the sessions
//! directory (`~/.config/agent-code/sessions/`).
//!
//! Every write goes through an exclusive per-session lock (cross-process
//! via `fs2` / `flock`) and an atomic temp+rename, so concurrent writers
//! — interactive exit, `/fork`, `/rename`/`/tag`, and the scheduled-run
//! process — cannot leave a half-written file or silently drop each
//! other's changes mid-read-modify-write.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use crate::config::atomic::atomic_write_secret;
use crate::config::ApiAuthMode;
use crate::llm::message::Message;
use crate::services::secret_masker;

/// The resolved provider a session conversation was run against.
///
/// Stored on disk at save time from the **live** engine config (already
/// resolved endpoints and auth modes — not the raw project file). At
/// resume, the same shape is taken from the live engine again, so the
/// comparison is resolved-vs-resolved and does not re-open the failed
/// "destination file vs running config" approach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderIdentity {
    /// Resolved API base URL (trailing slashes normalized on compare).
    pub base_url: String,
    /// Auth mode the conversation used.
    pub auth_mode: ApiAuthMode,
}

impl ProviderIdentity {
    /// Snapshot from a live (already resolved) `ApiConfig`.
    pub fn from_api(base_url: &str, auth_mode: ApiAuthMode) -> Self {
        Self {
            base_url: base_url.to_string(),
            auth_mode,
        }
    }

    /// Whether two identities name the same service binding.
    pub fn matches(&self, other: &Self) -> bool {
        normalize_base_url(&self.base_url) == normalize_base_url(&other.base_url)
            && self.auth_mode == other.auth_mode
    }
}

fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Serializable session state persisted to disk.
///
/// Auto-saved on exit, restored via `/resume <id>`. Stored as JSON
/// in `~/.config/agent-code/sessions/`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionData {
    /// Unique session identifier.
    pub id: String,
    /// Timestamp when the session was created.
    pub created_at: String,
    /// Timestamp of the last update.
    pub updated_at: String,
    /// Working directory at session start.
    pub cwd: String,
    /// Model used in this session.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Total turns completed.
    pub turn_count: usize,
    /// Total cost in USD.
    #[serde(default)]
    pub total_cost_usd: f64,
    /// Total input tokens.
    #[serde(default)]
    pub total_input_tokens: u64,
    /// Total output tokens.
    #[serde(default)]
    pub total_output_tokens: u64,
    /// Whether plan mode was active.
    #[serde(default)]
    pub plan_mode: bool,
    /// Optional human-readable label set via `/rename`. Not used for
    /// lookup — the session ID is still the primary key — but shown
    /// in `/sessions` and the resume picker.
    #[serde(default)]
    pub label: Option<String>,
    /// Tags for filtering sessions (e.g. "wip", "perf", "rust").
    /// Distinct from `label`: label is a single human-readable name,
    /// tags are a set for categorization. Managed via `/tag`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Provider the conversation was saved under. `None` for sessions
    /// written before this field existed — resume treats that as
    /// "unknown" rather than assuming the current process is safe.
    #[serde(default)]
    pub provider: Option<ProviderIdentity>,
}

/// Sessions directory path.
fn sessions_dir() -> Option<PathBuf> {
    crate::config::agent_config_dir().map(|d| d.join("sessions"))
}

/// Path of the session JSON and of its sibling advisory lock file.
fn session_paths(session_id: &str) -> Result<(PathBuf, PathBuf), String> {
    let dir = sessions_dir().ok_or("Could not determine sessions directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create sessions dir: {e}"))?;
    let path = dir.join(format!("{session_id}.json"));
    let lock = dir.join(format!("{session_id}.json.lock"));
    Ok((path, lock))
}

/// RAII exclusive lock for one session file.
///
/// Serializes every in-process and cross-process writer of the same
/// session id (interactive save, `/rename`, `/tag`, `/fork`, cron).
/// The kernel releases the lock if the holder dies, so there is no
/// stale-lock cleanup path.
struct SessionLockGuard {
    _file: std::fs::File,
}

impl SessionLockGuard {
    fn acquire(lock_path: &Path) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| format!("Failed to open session lock {}: {e}", lock_path.display()))?;
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|e| format!("Failed to lock session {}: {e}", lock_path.display()))?;
        Ok(Self { _file: file })
    }
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self._file);
    }
}

/// Run `f` while holding the exclusive lock for `session_id`.
fn with_session_lock<T>(
    session_id: &str,
    f: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    let (path, lock_path) = session_paths(session_id)?;
    let _guard = SessionLockGuard::acquire(&lock_path)?;
    f(&path)
}

/// Serialize session data to pretty JSON and apply the secret masker.
///
/// Extracted so wire-up tests can verify the persistence boundary
/// without touching the real filesystem.
pub(crate) fn serialize_masked(data: &SessionData) -> Result<String, String> {
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize session: {e}"))?;
    Ok(secret_masker::mask(&json))
}

/// Atomically write masked JSON to `path` (temp file + rename).
fn write_session_file_atomic(path: &Path, data: &SessionData) -> Result<(), String> {
    let json = serialize_masked(data)?;
    atomic_write_secret(path, json.as_bytes())
        .map_err(|e| format!("Failed to write session file {}: {e}", path.display()))
}

/// Load and parse a session JSON at `path` (caller holds the lock).
fn load_session_at(path: &Path) -> Result<SessionData, String> {
    if !path.exists() {
        return Err(format!("Session file '{}' not found", path.display()));
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read session: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse session: {e}"))
}

/// Save the current session to disk.
pub fn save_session(
    session_id: &str,
    messages: &[Message],
    cwd: &str,
    model: &str,
    turn_count: usize,
) -> Result<PathBuf, String> {
    save_session_full(
        session_id, messages, cwd, model, turn_count, 0.0, 0, 0, false, None,
    )
}

/// Save the full session state to disk (including cost and token tracking).
///
/// `provider` is the **resolved** identity of the live engine. When
/// `Some`, it is written so a later resume can refuse to bind the
/// conversation to a different service. When `None`, any previously
/// stored identity is preserved (metadata-only writers like `/rename`).
#[allow(clippy::too_many_arguments)]
pub fn save_session_full(
    session_id: &str,
    messages: &[Message],
    cwd: &str,
    model: &str,
    turn_count: usize,
    total_cost_usd: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    plan_mode: bool,
    provider: Option<ProviderIdentity>,
) -> Result<PathBuf, String> {
    with_session_lock(session_id, |path| {
        // Preserve original created_at, label, tags, and (when the caller
        // did not stamp a new one) provider if the file exists.
        // Re-read under the lock so a concurrent `/rename` or `/tag` is
        // either fully before or fully after this save — never discarded
        // because we sampled metadata mid-write.
        let (created_at, label, tags, prior_provider) = match load_session_at(path) {
            Ok(d) => (d.created_at, d.label, d.tags, d.provider),
            Err(_) => (chrono::Utc::now().to_rfc3339(), None, Vec::new(), None),
        };

        let data = SessionData {
            id: session_id.to_string(),
            created_at,
            updated_at: chrono::Utc::now().to_rfc3339(),
            cwd: cwd.to_string(),
            model: model.to_string(),
            messages: messages.to_vec(),
            turn_count,
            total_cost_usd,
            total_input_tokens,
            total_output_tokens,
            plan_mode,
            label,
            tags,
            provider: provider.or(prior_provider),
        };

        write_session_file_atomic(path, &data)?;
        debug!("Session saved: {}", path.display());
        Ok(path.to_path_buf())
    })
}

/// Whether this session already has a file on disk.
///
/// Distinguishes a never-used new session from one that was persisted
/// and has since been emptied: the first should leave nothing behind,
/// the second must have its file updated or the old conversation comes
/// back on the next resume.
pub fn session_exists(session_id: &str) -> bool {
    sessions_dir()
        .map(|d| d.join(format!("{session_id}.json")).exists())
        .unwrap_or(false)
}

/// Whether `data` may be restored into a process whose live API config
/// is `current`.
///
/// Compares the session's stored **resolved** provider to the process's
/// **resolved** provider. Does not load destination project files — that
/// path was tried and is unsound (running config is rewritten by
/// provider detection; project files are not).
///
/// - Both sides present and equal → `Ok(Compatible::Match)`
/// - Session has no fingerprint (pre-field save) → `Ok(Compatible::Unknown)`
/// - Mismatch → `Err` with a message naming both sides
pub fn check_provider_for_resume(
    data: &SessionData,
    current_base_url: &str,
    current_auth_mode: ApiAuthMode,
) -> Result<ProviderResume, String> {
    let Some(ref stored) = data.provider else {
        return Ok(ProviderResume::Unknown);
    };
    let current = ProviderIdentity::from_api(current_base_url, current_auth_mode);
    if stored.matches(&current) {
        return Ok(ProviderResume::Match);
    }
    Err(format!(
        "this session was saved against {} ({}) but the running process \
         is bound to {} ({}) — resume would send the conversation to a \
         different service or account. Start a new session with the \
         matching provider, or continue here without resuming.",
        stored.base_url,
        stored.auth_mode.as_str(),
        current.base_url,
        current.auth_mode.as_str(),
    ))
}

/// Outcome of [`check_provider_for_resume`] when the resume is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderResume {
    /// Stored identity matches the running process.
    Match,
    /// Session predates provider stamping; caller should warn.
    Unknown,
}

/// Load a session from disk by ID.
pub fn load_session(session_id: &str) -> Result<SessionData, String> {
    // Readers do not take the write lock: an atomic rename means they
    // always see a complete previous or next revision. Taking it would
    // stall `/resume` behind a long serialize of a large transcript.
    let (path, _) = session_paths(session_id)?;
    if !path.exists() {
        return Err(format!("Session '{session_id}' not found"));
    }
    let data = load_session_at(&path)?;
    info!(
        "Session loaded: {} ({} messages)",
        session_id,
        data.messages.len()
    );
    Ok(data)
}

/// List recent sessions, sorted by last update (most recent first).
pub fn list_sessions(limit: usize) -> Vec<SessionSummary> {
    let dir = match sessions_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Vec::new(),
    };

    // Drop sidecar entries for sessions that no longer exist, so a session's
    // cached metadata never outlives its file — this full-read path (used by
    // `/sessions`) does not otherwise touch the index.
    reconcile_index(&dir);

    let mut sessions: Vec<SessionSummary> = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let content = std::fs::read_to_string(entry.path()).ok()?;
            let data: SessionData = serde_json::from_str(&content).ok()?;
            Some(SessionSummary {
                id: data.id,
                cwd: data.cwd,
                model: data.model,
                turn_count: data.turn_count,
                message_count: data.messages.len(),
                updated_at: data.updated_at,
                label: data.label,
                tags: data.tags,
            })
        })
        .collect();

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(limit);
    sessions
}

/// Summary fields only, deserialized WITHOUT materializing the
/// `messages` transcript (serde skips the unknown field), so hot-path
/// callers don't pay to allocate every session's full history.
#[derive(serde::Deserialize)]
struct SessionMetaLite {
    id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    turn_count: usize,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Like [`list_sessions`], but skips parsing the message transcripts —
/// for callers that only need the summary fields on a hot path (e.g.
/// tab-completion). `message_count` is reported as `0` (not read), since
/// counting messages would require deserializing the transcript this
/// path deliberately avoids.
pub fn list_session_summaries(limit: usize) -> Vec<SessionSummary> {
    let dir = match sessions_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Vec::new(),
    };
    list_session_summaries_in(&dir, limit)
}

/// A cached lite summary plus the source file's validity stamp, keyed by
/// session id in the on-disk index.
#[derive(Clone, Serialize, Deserialize)]
struct IndexedSummary {
    /// Source file mtime in nanoseconds since the Unix epoch. Combined with
    /// `size`, this is the cache-validity stamp: the entry is only trusted
    /// while both still match the file on disk. Nanosecond resolution plus the
    /// size guard makes a same-instant stale hit (e.g. a rename/tag within one
    /// mtime tick) far less likely than a millisecond stamp alone.
    mtime_ns: u128,
    /// Source file size in bytes, part of the validity stamp.
    size: u64,
    cwd: String,
    model: String,
    updated_at: String,
    turn_count: usize,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Name of the sidecar index inside the sessions directory. Deliberately not a
/// `.json` file so it can never collide with a `<id>.json` session (a session
/// id of `index` used to clobber it) and is naturally skipped by the scan.
const INDEX_FILE: &str = "index.cache";

/// Drop sidecar-index entries whose `<id>.json` session file no longer exists,
/// so the cache never persists metadata for a deleted session. A no-op when
/// there is no index. Cheap: it stats the directory names only, not contents.
fn reconcile_index(dir: &std::path::Path) {
    let index_path = dir.join(INDEX_FILE);
    let Ok(text) = std::fs::read_to_string(&index_path) else {
        return;
    };
    let Ok(mut index) =
        serde_json::from_str::<std::collections::HashMap<String, IndexedSummary>>(&text)
    else {
        return;
    };
    let present: std::collections::HashSet<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .collect();
    let before = index.len();
    index.retain(|id, _| present.contains(id));
    if index.len() != before {
        let _ = std::fs::write(
            &index_path,
            serde_json::to_string(&index).unwrap_or_default(),
        );
    }
}

/// The cache-validity stamp for a file: `(mtime_ns, size)`.
fn file_stamp(path: &std::path::Path) -> Option<(u128, u64)> {
    let md = std::fs::metadata(path).ok()?;
    let ns = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((ns, md.len()))
}

/// Testable core of [`list_session_summaries`].
///
/// Backed by a lazy, self-healing sidecar index (`index.json`): each call
/// stats every session file (cheap) but only reads and parses files that are
/// new or whose mtime changed since the index was written. The index is a pure
/// cache derived from the session files — if it is missing, stale, or
/// corrupt, it is transparently rebuilt, so it can never corrupt session data.
fn list_session_summaries_in(dir: &std::path::Path, limit: usize) -> Vec<SessionSummary> {
    let index_path = dir.join(INDEX_FILE);
    let mut index: std::collections::HashMap<String, IndexedSummary> =
        std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();

    let mut present: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut summaries: Vec<SessionSummary> = Vec::new();
    let mut index_changed = false;

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // The index file itself is not a session.
        if path.extension().is_none_or(|ext| ext != "json")
            || path.file_name() == Some(std::ffi::OsStr::new(INDEX_FILE))
        {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let disk_stamp = file_stamp(&path);
        present.insert(id.clone());

        // Fast path: a cached entry whose (mtime_ns, size) still matches.
        let cached = index
            .get(&id)
            .filter(|e| Some((e.mtime_ns, e.size)) == disk_stamp);
        let entry_summary = if let Some(hit) = cached {
            hit.clone()
        } else {
            // Miss: read and parse this one file, then refresh the cache.
            let Some(content) = std::fs::read_to_string(&path).ok() else {
                continue;
            };
            let Some(m) = serde_json::from_str::<SessionMetaLite>(&content).ok() else {
                continue;
            };
            let (mtime_ns, size) = disk_stamp.unwrap_or((0, 0));
            let fresh = IndexedSummary {
                mtime_ns,
                size,
                cwd: m.cwd,
                model: m.model,
                updated_at: m.updated_at,
                turn_count: m.turn_count,
                label: m.label,
                tags: m.tags,
            };
            index.insert(id.clone(), fresh.clone());
            index_changed = true;
            fresh
        };

        summaries.push(SessionSummary {
            id,
            cwd: entry_summary.cwd,
            model: entry_summary.model,
            turn_count: entry_summary.turn_count,
            message_count: 0,
            updated_at: entry_summary.updated_at,
            label: entry_summary.label,
            tags: entry_summary.tags,
        });
    }

    // Drop cache entries for sessions that no longer exist on disk.
    let before = index.len();
    index.retain(|id, _| present.contains(id));
    index_changed |= index.len() != before;

    // Best-effort write-back of the refreshed cache; failure is non-fatal.
    if index_changed && let Ok(json) = serde_json::to_string(&index) {
        let _ = std::fs::write(&index_path, json);
    }

    summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    summaries.truncate(limit);
    summaries
}

/// Result of a prune sweep over the sessions directory.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneStats {
    /// Files older than the threshold that were removed.
    pub removed: usize,
    /// Files that were kept (either fresh, or had an unparseable
    /// `updated_at` and were left alone defensively).
    pub kept: usize,
}

/// Delete sessions whose `updated_at` is older than `days` days.
///
/// A missing or malformed `updated_at` is treated as "don't know —
/// keep it" so we never delete a file whose age we can't determine.
/// Passing `days == 0` is an explicit no-op so `cleanup_period_days =
/// 0` in config behaves the same as an absent value.
pub fn prune_older_than(days: u64) -> Result<PruneStats, String> {
    if days == 0 {
        return Ok(PruneStats::default());
    }
    let dir = sessions_dir().ok_or("Could not determine sessions directory")?;
    if !dir.is_dir() {
        // No sessions have ever been saved on this host.
        return Ok(PruneStats::default());
    }
    prune_older_than_in(&dir, days, chrono::Utc::now())
}

/// Testable variant: operate on a specific directory with a caller-
/// provided "now" so unit tests can control time without touching
/// the real clock or config dir.
pub(crate) fn prune_older_than_in(
    dir: &std::path::Path,
    days: u64,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<PruneStats, String> {
    if days == 0 {
        return Ok(PruneStats::default());
    }
    let threshold = now - chrono::Duration::days(days as i64);
    let mut stats = PruneStats::default();

    let entries = std::fs::read_dir(dir).map_err(|e| format!("Read {dir:?} failed: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }

        // Try to read & parse just enough to check the timestamp.
        // Any failure => keep the file.
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                stats.kept += 1;
                continue;
            }
        };
        let data: SessionData = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(_) => {
                stats.kept += 1;
                continue;
            }
        };
        let updated_at = match chrono::DateTime::parse_from_rfc3339(&data.updated_at) {
            Ok(d) => d.with_timezone(&chrono::Utc),
            Err(_) => {
                stats.kept += 1;
                continue;
            }
        };

        if updated_at < threshold {
            // Best-effort: a failed delete shouldn't abort the sweep.
            if std::fs::remove_file(&path).is_ok() {
                stats.removed += 1;
                // Drop the advisory lock sidecar so prune cannot leave
                // an unbounded set of `<id>.json.lock` files behind.
                let lock = path.with_extension("json.lock");
                let _ = std::fs::remove_file(&lock);
            } else {
                stats.kept += 1;
            }
        } else {
            stats.kept += 1;
        }
    }
    // The sidecar index caches per-session metadata (cwd/model/label/tags).
    // Pruning deletes session files directly, so drop the whole index when
    // anything was removed — otherwise a pruned session's metadata would
    // linger on disk (undermining the cleanup/privacy guarantee) until the
    // next listing rebuilds the index. The index is pure cache and rebuilds
    // lazily, so removing it is safe and cheap.
    if stats.removed > 0 {
        let _ = std::fs::remove_file(dir.join(INDEX_FILE));
    }

    debug!(
        "Session prune: removed {} kept {} (threshold {} days)",
        stats.removed, stats.kept, days
    );
    Ok(stats)
}

/// Brief summary of a session for listing.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub cwd: String,
    pub model: String,
    pub turn_count: usize,
    pub message_count: usize,
    pub updated_at: String,
    pub label: Option<String>,
    pub tags: Vec<String>,
}

/// Set (or clear) the human-readable label on a session. Pass `None`
/// to clear. Returns the path of the written file.
///
/// Implemented as load-modify-save under the per-session lock so a
/// concurrent transcript save re-reads the new label instead of
/// overwriting it with a pre-rename snapshot.
pub fn set_session_label(session_id: &str, label: Option<String>) -> Result<PathBuf, String> {
    with_session_lock(session_id, |path| {
        let mut data = load_session_at(path)?;
        data.label = label;
        data.updated_at = chrono::Utc::now().to_rfc3339();
        write_session_file_atomic(path, &data)?;
        Ok(path.to_path_buf())
    })
}

/// Normalize a user-supplied tag: trim, lowercase, reject empty /
/// whitespace-only / punctuation-containing tags. Returns a normalized
/// copy on success, or an error string explaining why it was rejected.
pub fn normalize_tag(raw: &str) -> Result<String, String> {
    let t = raw.trim().to_ascii_lowercase();
    if t.is_empty() {
        return Err("tag is empty".to_string());
    }
    if t.len() > 32 {
        return Err(format!("tag '{t}' exceeds 32 characters"));
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "tag '{t}' contains disallowed characters (allowed: letters, digits, '-', '_')"
        ));
    }
    Ok(t)
}

/// Add a tag to the session. Idempotent: if the tag is already present,
/// returns `Ok(false)`; on add returns `Ok(true)`.
pub fn add_session_tag(session_id: &str, tag: &str) -> Result<bool, String> {
    let normalized = normalize_tag(tag)?;
    with_session_lock(session_id, |path| {
        let mut data = load_session_at(path)?;
        if data.tags.iter().any(|t| t == &normalized) {
            return Ok(false);
        }
        data.tags.push(normalized);
        data.tags.sort();
        data.updated_at = chrono::Utc::now().to_rfc3339();
        write_session_file_atomic(path, &data)?;
        Ok(true)
    })
}

/// Remove a tag from the session. Returns `Ok(false)` if the tag
/// wasn't present, `Ok(true)` if it was removed.
pub fn remove_session_tag(session_id: &str, tag: &str) -> Result<bool, String> {
    let normalized = normalize_tag(tag)?;
    with_session_lock(session_id, |path| {
        let mut data = load_session_at(path)?;
        let before = data.tags.len();
        data.tags.retain(|t| t != &normalized);
        if data.tags.len() == before {
            return Ok(false);
        }
        data.updated_at = chrono::Utc::now().to_rfc3339();
        write_session_file_atomic(path, &data)?;
        Ok(true)
    })
}

/// Generate a new session ID.
pub fn new_session_id() -> String {
    Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap_or("session")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::{ContentBlock, Message, UserMessage, user_message};

    fn write_session_file(dir: &std::path::Path, id: &str, updated_at: &str) {
        let json = format!(
            r#"{{"id":"{id}","created_at":"{updated_at}","updated_at":"{updated_at}",
                 "cwd":"/work/{id}","model":"m","turn_count":1,"messages":[]}}"#
        );
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
    }

    #[test]
    fn index_lists_sessions_sorted_and_creates_sidecar() {
        let dir = std::env::temp_dir().join(format!("agent-sess-idx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_session_file(&dir, "a", "2026-06-30T10:00:00Z");
        write_session_file(&dir, "b", "2026-06-30T11:00:00Z");

        let out = list_session_summaries_in(&dir, 10);
        assert_eq!(out.len(), 2);
        // Sorted by updated_at descending.
        assert_eq!(out[0].id, "b");
        assert_eq!(out[1].id, "a");
        // The sidecar index was written.
        assert!(dir.join(INDEX_FILE).exists());

        // A second call (now served from the index) returns the same result.
        let again = list_session_summaries_in(&dir, 10);
        assert_eq!(again.len(), 2);
        assert_eq!(again[0].id, "b");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_drops_entries_for_removed_sessions() {
        let dir = std::env::temp_dir().join(format!("agent-sess-idx-rm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_session_file(&dir, "keep", "2026-06-30T10:00:00Z");
        write_session_file(&dir, "gone", "2026-06-30T11:00:00Z");
        assert_eq!(list_session_summaries_in(&dir, 10).len(), 2);

        // Remove one session file, then relist: it must not appear, and the
        // index must no longer reference it.
        std::fs::remove_file(dir.join("gone.json")).unwrap();
        let out = list_session_summaries_in(&dir, 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "keep");
        let idx = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        assert!(!idx.contains("gone"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_recovers_from_corrupt_sidecar() {
        let dir = std::env::temp_dir().join(format!("agent-sess-idx-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_session_file(&dir, "a", "2026-06-30T10:00:00Z");
        std::fs::write(dir.join(INDEX_FILE), b"not json {").unwrap();

        // A corrupt index must be transparently rebuilt from the files.
        let out = list_session_summaries_in(&dir, 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "a");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reconcile_index_drops_metadata_for_deleted_sessions() {
        // Simulates a session deleted out-of-band (or via a full-read list
        // path): its cached metadata must not survive in the sidecar.
        let dir = std::env::temp_dir().join(format!("agent-sess-idx-recon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_session_file(&dir, "keep", "2026-06-30T10:00:00Z");
        write_session_file(&dir, "gone", "2026-06-30T11:00:00Z");
        let _ = list_session_summaries_in(&dir, 10); // build index with both
        assert!(
            std::fs::read_to_string(dir.join(INDEX_FILE))
                .unwrap()
                .contains("gone")
        );

        // Delete one file and reconcile (what list_sessions now does).
        std::fs::remove_file(dir.join("gone.json")).unwrap();
        reconcile_index(&dir);

        let idx = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        assert!(idx.contains("keep"));
        assert!(
            !idx.contains("gone"),
            "deleted session metadata must be dropped"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prune_clears_the_sidecar_index() {
        // Pruning must not leave a pruned session's cached metadata behind.
        let dir = std::env::temp_dir().join(format!("agent-sess-idx-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_session_file(&dir, "old", "2000-01-01T00:00:00Z");

        // Build the index, then confirm it cached the (soon-pruned) session.
        let _ = list_session_summaries_in(&dir, 10);
        assert!(dir.join(INDEX_FILE).exists());
        assert!(
            std::fs::read_to_string(dir.join(INDEX_FILE))
                .unwrap()
                .contains("old")
        );

        let now = chrono::DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let stats = prune_older_than_in(&dir, 1, now).unwrap();
        assert_eq!(stats.removed, 1);
        // The sidecar is dropped so the pruned session's metadata is gone.
        assert!(!dir.join(INDEX_FILE).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_does_not_collide_with_session_named_index() {
        // A session id of "index" must not be shadowed or clobbered by the
        // sidecar cache, which is a separate non-`.json` file.
        let dir = std::env::temp_dir().join(format!("agent-sess-idx-col-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_session_file(&dir, "index", "2026-06-30T10:00:00Z");
        write_session_file(&dir, "other", "2026-06-30T11:00:00Z");

        let out = list_session_summaries_in(&dir, 10);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|s| s.id == "index"));
        // The session file and the sidecar coexist as distinct files.
        assert!(dir.join("index.json").exists());
        assert!(dir.join(INDEX_FILE).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_reparses_on_mtime_mismatch() {
        let dir = std::env::temp_dir().join(format!("agent-sess-idx-mt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_session_file(&dir, "a", "2026-06-30T10:00:00Z");

        // Seed the index with a deliberately stale mtime and outdated model
        // so the fast path is rejected and the file is re-read.
        let mut stale = std::collections::HashMap::new();
        stale.insert(
            "a".to_string(),
            IndexedSummary {
                mtime_ns: 0,
                size: 0,
                cwd: "/old".to_string(),
                model: "old-model".to_string(),
                updated_at: "2000-01-01T00:00:00Z".to_string(),
                turn_count: 99,
                label: None,
                tags: vec![],
            },
        );
        std::fs::write(dir.join(INDEX_FILE), serde_json::to_string(&stale).unwrap()).unwrap();

        let out = list_session_summaries_in(&dir, 10);
        assert_eq!(out.len(), 1);
        // Values come from the file, not the stale cache entry.
        assert_eq!(out[0].model, "m");
        assert_eq!(out[0].updated_at, "2026-06-30T10:00:00Z");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_meta_lite_deserializes_metadata_and_skips_messages() {
        // A session JSON with a (deliberately arbitrary, non-Message)
        // `messages` array. The lite metadata path must pull the summary
        // fields and ignore the transcript entirely — so it never has to
        // parse the potentially huge/complex message history.
        let json = r#"{
            "id": "sess-123",
            "created_at": "2026-06-30T09:00:00Z",
            "updated_at": "2026-06-30T10:00:00Z",
            "cwd": "/work",
            "model": "some-model",
            "messages": [1, "arbitrary", {"nested": [2, 3]}],
            "turn_count": 4,
            "label": "refactor auth",
            "tags": ["wip", "auth"]
        }"#;
        let m: SessionMetaLite = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "sess-123");
        assert_eq!(m.updated_at, "2026-06-30T10:00:00Z");
        assert_eq!(m.model, "some-model");
        assert_eq!(m.turn_count, 4);
        assert_eq!(m.label.as_deref(), Some("refactor auth"));
        assert_eq!(m.tags, vec!["wip".to_string(), "auth".to_string()]);
    }

    /// Helper: build a session containing the given messages with
    /// fixed, deterministic metadata. Used by wire-up tests.
    fn make_session(messages: Vec<Message>) -> SessionData {
        SessionData {
            id: "fixture".into(),
            created_at: "2026-04-15T00:00:00Z".into(),
            updated_at: "2026-04-15T00:00:00Z".into(),
            cwd: "/work".into(),
            model: "test-model".into(),
            messages,
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        }
    }

    /// Helper: a user message whose sole content block is a tool_result
    /// (simulates the agent receiving tool output that embedded a secret).
    fn tool_result_user_message(tool_use_id: &str, content: &str) -> Message {
        Message::User(UserMessage {
            uuid: uuid::Uuid::new_v4(),
            timestamp: "2026-04-15T00:00:00Z".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: false,
                extra_content: Vec::new(),
            }],
            is_meta: false,
            is_compact_summary: false,
        })
    }

    #[test]
    fn test_new_session_id_format() {
        let id = new_session_id();
        assert!(!id.is_empty());
        assert!(!id.contains('-')); // Should be first segment only.
        assert!(id.len() == 8); // UUID first segment is 8 hex chars.
    }

    #[test]
    fn test_new_session_id_unique() {
        let id1 = new_session_id();
        let id2 = new_session_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_save_and_load_session() {
        // Override sessions dir to a temp directory.
        let dir = tempfile::tempdir().unwrap();
        let session_id = "test-save-load";
        let session_file = dir.path().join(format!("{session_id}.json"));

        let messages = vec![user_message("hello"), user_message("world")];

        // Save manually to temp dir.
        let data = SessionData {
            id: session_id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            cwd: "/tmp".to_string(),
            model: "test-model".to_string(),
            messages: messages.clone(),
            turn_count: 5,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        };
        let json = serde_json::to_string_pretty(&data).unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(&session_file, &json).unwrap();

        // Load it back.
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.id, session_id);
        assert_eq!(loaded.cwd, "/tmp");
        assert_eq!(loaded.model, "test-model");
        assert_eq!(loaded.turn_count, 5);
        assert_eq!(loaded.messages.len(), 2);
    }

    #[test]
    fn test_session_data_serialization_roundtrip() {
        let data = SessionData {
            id: "abc123".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            cwd: "/home/user/project".to_string(),
            model: "claude-sonnet-4".to_string(),
            messages: vec![user_message("test")],
            turn_count: 3,
            total_cost_usd: 0.05,
            total_input_tokens: 1000,
            total_output_tokens: 500,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        };

        let json = serde_json::to_string(&data).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.id, data.id);
        assert_eq!(loaded.model, data.model);
        assert_eq!(loaded.turn_count, data.turn_count);
    }

    #[test]
    fn serialize_masked_redacts_secrets_in_messages() {
        // A tool result leaked an AWS access key into the message history.
        // When the session is serialized for disk, the secret must not
        // survive the persistence boundary.
        let aws_key = "AKIAIOSFODNN7EXAMPLE";
        let data = SessionData {
            id: "sess-1".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message(format!("here is my key {aws_key}"))],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(
            !out.contains(aws_key),
            "raw AWS key survived serialization: {out}",
        );
        assert!(out.contains("[REDACTED:aws_access_key]"));
        // Non-secret metadata must still be present.
        assert!(out.contains("\"cwd\": \"/work\""));
        assert!(out.contains("\"model\": \"test-model\""));
    }

    #[test]
    fn serialize_masked_redacts_generic_credential_assignments() {
        let secret_line = "api_key=verylongprovidersecret1234567890";
        let data = SessionData {
            id: "sess-2".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message(secret_line)],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(!out.contains("verylongprovidersecret1234567890"));
        assert!(out.contains("[REDACTED:credential]"));
    }

    /// Regression probe: masking must never corrupt JSON structure.
    /// Previously, the credential regex's trailing `["']?` could consume
    /// the closing quote of a JSON string value, producing unparseable
    /// output that would break /resume.
    #[test]
    fn serialize_masked_produces_parseable_json_for_unquoted_inner_secret() {
        let data = SessionData {
            id: "probe".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message("api_key=hunter2hunter2")],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        };
        let out = serialize_masked(&data).unwrap();
        // Must still parse back as a SessionData.
        let parsed: Result<SessionData, _> = serde_json::from_str(&out);
        assert!(
            parsed.is_ok(),
            "masked session JSON failed to round-trip: {}\n---\n{out}",
            parsed.err().unwrap(),
        );
        let loaded = parsed.unwrap();
        assert_eq!(loaded.id, "probe");
        assert_eq!(loaded.messages.len(), 1);
    }

    #[test]
    fn serialize_masked_produces_parseable_json_for_multiple_secret_shapes() {
        let shapes = [
            "my api_key=hunter2hunter2",
            "password: sup3rs3cr3tv@lue (truncated)",
            r#"env DATABASE_URL=postgres://user:hunter2hunter2@host/db"#,
            "auth_token = abcdefghijklmn",
            "mixed: api_key=abcd1234efgh5678 and token=xyz12345abcd6789",
        ];
        for shape in shapes {
            let data = SessionData {
                id: "probe".to_string(),
                created_at: "2026-04-15T00:00:00Z".to_string(),
                updated_at: "2026-04-15T00:00:00Z".to_string(),
                cwd: "/work".to_string(),
                model: "test-model".to_string(),
                messages: vec![user_message(shape.to_string())],
                turn_count: 1,
                total_cost_usd: 0.0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                plan_mode: false,
                label: None,
                tags: Vec::new(),
                provider: None,
            };
            let out = serialize_masked(&data).unwrap();
            let parsed: Result<SessionData, _> = serde_json::from_str(&out);
            assert!(
                parsed.is_ok(),
                "shape corrupted JSON: {shape:?}\nerr: {}\nout: {out}",
                parsed.err().unwrap(),
            );
        }
    }

    #[test]
    fn serialize_masked_redacts_secret_in_tool_result_block() {
        // Tool output commonly leaks env vars. When the session is
        // serialized, secrets inside ToolResult content must be scrubbed
        // just like those in plain text blocks.
        let leaked = "export AWS_SECRET_ACCESS_KEY=abcdefghijklmnopqrstuvwxyz1234";
        let data = make_session(vec![tool_result_user_message("call-1", leaked)]);
        let out = serialize_masked(&data).unwrap();
        assert!(
            !out.contains("abcdefghijklmnopqrstuvwxyz1234"),
            "tool_result secret survived serialization",
        );
        assert!(out.contains("REDACTED"));
        // Round-trip must still work.
        let _: SessionData =
            serde_json::from_str(&out).expect("tool_result session must round-trip");
    }

    #[test]
    fn serialize_masked_handles_many_messages_with_mixed_secrets() {
        // Stress: multiple messages, mixed speakers, multiple secret
        // shapes. All must be masked and the result must still parse.
        let messages = vec![
            user_message("AKIAIOSFODNN7EXAMPLE leaked in user message"),
            tool_result_user_message(
                "t1",
                r#"env dump: DATABASE_URL=postgres://user:hunter2hunter2@host/db"#,
            ),
            user_message("auth_token = abcdefghijklmnop"),
            tool_result_user_message("t2", "config.toml says api_key = \"secretprovidervalue\""),
        ];
        let data = make_session(messages);
        let out = serialize_masked(&data).unwrap();

        // No raw secrets remain.
        for needle in [
            "AKIAIOSFODNN7EXAMPLE",
            "hunter2hunter2",
            "abcdefghijklmnop",
            "secretprovidervalue",
        ] {
            assert!(!out.contains(needle), "leaked {needle} in: {out}",);
        }
        // Multiple REDACTED markers present.
        assert!(out.matches("REDACTED").count() >= 4);
        // JSON must round-trip through a real parse.
        let parsed: SessionData =
            serde_json::from_str(&out).expect("mixed-secret session must round-trip");
        assert_eq!(parsed.messages.len(), 4);
    }

    #[test]
    fn serialize_masked_is_idempotent_save_load_save() {
        // Re-saving a loaded session must produce byte-identical JSON
        // (the masker replaced all secrets on the first save; the
        // second save should find nothing to mask).
        let data = make_session(vec![
            user_message("AKIAIOSFODNN7EXAMPLE and api_key=hunter2hunter2"),
            tool_result_user_message(
                "t1",
                "ghp_abcdefghijklmnopqrstuvwxyz0123456789 then password='firstpassword1234'",
            ),
        ]);

        let first = serialize_masked(&data).unwrap();
        let loaded: SessionData = serde_json::from_str(&first).expect("first save must parse");

        // Mirror production: save_session_full re-uses timestamps from
        // in-memory state, so clone the loaded data as the next save's
        // input (keeping everything deterministic for the comparison).
        let second = serialize_masked(&loaded).unwrap();

        assert_eq!(
            first, second,
            "save→load→save is not idempotent\nfirst:\n{first}\nsecond:\n{second}",
        );
    }

    #[test]
    fn serialize_masked_leaves_innocuous_content_intact() {
        let data = SessionData {
            id: "sess-3".to_string(),
            created_at: "2026-04-15T00:00:00Z".to_string(),
            updated_at: "2026-04-15T00:00:00Z".to_string(),
            cwd: "/work".to_string(),
            model: "test-model".to_string(),
            messages: vec![user_message("fn main() { println!(\"hello\"); }")],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        };
        let out = serialize_masked(&data).unwrap();
        assert!(!out.contains("REDACTED"));
        assert!(out.contains("fn main()"));
    }

    #[test]
    fn label_round_trips_through_serde() {
        let data = SessionData {
            id: "id".into(),
            created_at: "2026-04-15T00:00:00Z".into(),
            updated_at: "2026-04-15T00:00:00Z".into(),
            cwd: "/work".into(),
            model: "m".into(),
            messages: vec![],
            turn_count: 1,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: Some("refactor pass".into()),
            tags: Vec::new(),
            provider: None,
        };
        let json = serde_json::to_string(&data).unwrap();
        let back: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.label.as_deref(), Some("refactor pass"));
    }

    #[test]
    fn label_missing_from_old_json_defaults_to_none() {
        // Simulate an older session file written before the label field existed.
        let json = serde_json::json!({
            "id": "old",
            "created_at": "2026-04-15T00:00:00Z",
            "updated_at": "2026-04-15T00:00:00Z",
            "cwd": "/work",
            "model": "m",
            "messages": [],
            "turn_count": 1,
        });
        let data: SessionData = serde_json::from_value(json).unwrap();
        assert!(data.label.is_none());
    }

    #[test]
    fn test_session_summary_fields() {
        let summary = SessionSummary {
            id: "xyz".to_string(),
            cwd: "/tmp".to_string(),
            model: "gpt-4".to_string(),
            turn_count: 10,
            message_count: 20,
            updated_at: "2026-03-31".to_string(),
            label: None,
            tags: Vec::new(),
        };
        assert_eq!(summary.id, "xyz");
        assert_eq!(summary.turn_count, 10);
        assert_eq!(summary.message_count, 20);
    }

    #[test]
    fn normalize_tag_accepts_safe_tags() {
        assert_eq!(normalize_tag("wip").unwrap(), "wip");
        assert_eq!(normalize_tag("Perf").unwrap(), "perf");
        assert_eq!(normalize_tag("  rust  ").unwrap(), "rust");
        assert_eq!(normalize_tag("feat-auth").unwrap(), "feat-auth");
        assert_eq!(normalize_tag("v2_api").unwrap(), "v2_api");
    }

    #[test]
    fn normalize_tag_rejects_bad_tags() {
        assert!(normalize_tag("").is_err());
        assert!(normalize_tag("   ").is_err());
        assert!(normalize_tag("foo bar").is_err());
        assert!(normalize_tag("foo/bar").is_err());
        assert!(normalize_tag("foo.bar").is_err());
        assert!(normalize_tag(&"a".repeat(33)).is_err());
    }

    #[test]
    fn tags_missing_from_old_json_defaults_to_empty() {
        // Sessions written before the tags field existed must still load.
        let json = serde_json::json!({
            "id": "old",
            "created_at": "2026-04-15T00:00:00Z",
            "updated_at": "2026-04-15T00:00:00Z",
            "cwd": "/work",
            "model": "m",
            "messages": [],
            "turn_count": 1,
        });
        let data: SessionData = serde_json::from_value(json).unwrap();
        assert!(data.tags.is_empty());
    }

    // ---- prune_older_than_in ----

    fn write_session(dir: &std::path::Path, id: &str, updated_at: &str) {
        let data = SessionData {
            id: id.to_string(),
            created_at: updated_at.to_string(),
            updated_at: updated_at.to_string(),
            cwd: "/w".into(),
            model: "m".into(),
            messages: Vec::new(),
            turn_count: 0,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            plan_mode: false,
            label: None,
            tags: Vec::new(),
            provider: None,
        };
        let json = serde_json::to_string_pretty(&data).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
    }

    #[test]
    fn prune_also_removes_lock_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::days(60)).to_rfc3339();
        let id = "old-with-lock";
        let path = tmp.path().join(format!("{id}.json"));
        let lock = tmp.path().join(format!("{id}.json.lock"));
        write_session(tmp.path(), id, &old);
        std::fs::write(&lock, "").unwrap();
        assert!(lock.exists());
        let stats = prune_older_than_in(tmp.path(), 30, now).unwrap();
        assert_eq!(stats.removed, 1);
        assert!(!path.exists());
        assert!(
            !lock.exists(),
            "lock sidecar must be removed with the session"
        );
    }

    #[test]
    fn prune_removes_sessions_older_than_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-23T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        // One fresh session (1 day old — should stay).
        write_session(tmp.path(), "fresh", "2026-04-22T12:00:00Z");
        // One stale session (40 days old — should be removed under 30d).
        write_session(tmp.path(), "stale", "2026-03-14T12:00:00Z");

        let stats = prune_older_than_in(tmp.path(), 30, now).unwrap();
        assert_eq!(stats.removed, 1);
        assert_eq!(stats.kept, 1);
        assert!(tmp.path().join("fresh.json").exists());
        assert!(!tmp.path().join("stale.json").exists());
    }

    #[test]
    fn prune_zero_days_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        // Ancient file — would be removed by any non-zero threshold.
        write_session(tmp.path(), "ancient", "2020-01-01T00:00:00Z");
        let stats = prune_older_than_in(tmp.path(), 0, now).unwrap();
        assert_eq!(stats, PruneStats::default());
        assert!(tmp.path().join("ancient.json").exists());
    }

    #[test]
    fn prune_keeps_files_with_unparseable_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        // Write a valid JSON body with a garbage timestamp. Malformed
        // `updated_at` is conservatively treated as "don't know" so we
        // never delete a file whose age we can't determine.
        let data = serde_json::json!({
            "id": "weird",
            "created_at": "not-a-date",
            "updated_at": "also-not-a-date",
            "cwd": "/w",
            "model": "m",
            "messages": [],
            "turn_count": 0,
        });
        std::fs::write(
            tmp.path().join("weird.json"),
            serde_json::to_string(&data).unwrap(),
        )
        .unwrap();
        let stats = prune_older_than_in(tmp.path(), 1, now).unwrap();
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.kept, 1);
        assert!(tmp.path().join("weird.json").exists());
    }

    #[test]
    fn prune_skips_non_json_files() {
        // Stray `.tmp` / `.bak` files in the sessions dir shouldn't be
        // scanned or counted toward the kept/removed totals.
        //
        // `now` is pinned to a fixed instant (like the sibling prune
        // tests) rather than `Utc::now()` so the kept session never ages
        // past the 30-day threshold as the wall clock advances.
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-23T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        std::fs::write(tmp.path().join("leftover.tmp"), b"noise").unwrap();
        write_session(tmp.path(), "current", "2026-04-23T00:00:00Z");
        let stats = prune_older_than_in(tmp.path(), 30, now).unwrap();
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.kept, 1);
    }

    #[test]
    fn prune_boundary_newer_than_threshold_is_kept() {
        // Exactly at the threshold should be kept (strictly less-than
        // delete rule). Verify the boundary so we don't accidentally
        // drift to an off-by-one that deletes a file the user's
        // policy said to keep.
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-23T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        // Exactly 30 days old.
        write_session(tmp.path(), "edge", "2026-03-24T12:00:00Z");
        let stats = prune_older_than_in(tmp.path(), 30, now).unwrap();
        assert_eq!(stats.removed, 0, "file at the exact threshold must be kept");
        assert_eq!(stats.kept, 1);
    }

    /// A `/rename` that interleaves with a transcript save must not be
    /// discarded: the save re-reads label/tags under the same lock.
    #[test]
    fn rename_survives_a_concurrent_save() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::EnvGuard::set("XDG_CONFIG_HOME", tmp.path());
        let id = "rename-race";
        let msg = user_message("hello");
        save_session(id, std::slice::from_ref(&msg), "/work", "m", 1).expect("seed");

        let id_save = id.to_string();
        let id_rename = id.to_string();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let b1 = barrier.clone();
        let b2 = barrier;

        let save = std::thread::spawn(move || {
            b1.wait();
            for i in 0..40 {
                let m = user_message(format!("turn-{i}"));
                save_session(&id_save, &[m], "/work", "m", i + 2).expect("save");
            }
        });
        let rename = std::thread::spawn(move || {
            b2.wait();
            for i in 0..40 {
                set_session_label(&id_rename, Some(format!("label-{i}"))).expect("rename");
            }
        });
        save.join().expect("save thread");
        rename.join().expect("rename thread");

        let data = load_session(id).expect("final load must parse");
        assert!(data.label.is_some(), "label was lost to a concurrent save");
        assert_eq!(data.messages.len(), 1, "transcript should be present");
        // Round-trip through serde again — file must be valid JSON.
        let raw = std::fs::read_to_string(
            tmp.path()
                .join("agent-code")
                .join("sessions")
                .join(format!("{id}.json")),
        )
        .expect("read final");
        let _: SessionData = serde_json::from_str(&raw).expect("final JSON must parse");
    }

    /// Two concurrent full saves must leave a parseable file; neither
    /// truncate-write can produce a partial JSON document.
    #[test]
    fn concurrent_saves_leave_parseable_json() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::EnvGuard::set("XDG_CONFIG_HOME", tmp.path());
        let id = "atomic-race";
        save_session(id, &[user_message("seed")], "/work", "m", 1).expect("seed");

        let id_a = id.to_string();
        let id_b = id.to_string();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let b1 = barrier.clone();
        let b2 = barrier;

        let a = std::thread::spawn(move || {
            b1.wait();
            for i in 0..50 {
                // Large-ish payload so a non-atomic write would be more
                // likely to interleave mid-document.
                let body = "x".repeat(8_192);
                let m = user_message(format!("a-{i}-{body}"));
                save_session(&id_a, &[m], "/work", "m", i).expect("a");
            }
        });
        let b = std::thread::spawn(move || {
            b2.wait();
            for i in 0..50 {
                let body = "y".repeat(8_192);
                let m = user_message(format!("b-{i}-{body}"));
                save_session(&id_b, &[m], "/work", "m", i).expect("b");
            }
        });
        a.join().expect("a");
        b.join().expect("b");

        let data = load_session(id).expect("must load after concurrent writes");
        assert_eq!(data.messages.len(), 1);
        let text = match &data.messages[0] {
            Message::User(u) => u
                .content
                .iter()
                .find_map(|c| match c {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .unwrap_or(""),
            _ => "",
        };
        assert!(
            text.starts_with("a-") || text.starts_with("b-"),
            "unexpected final content: {text:.40}"
        );
    }

    #[test]
    fn write_is_atomic_temp_then_rename() {
        // Structural: production writers must not call std::fs::write on
        // the session path. A partial-write regression is silent until
        // corruption shows up in the wild.
        let src = include_str!("session.rs");
        // Strip the test module so fixtures that write helper files do
        // not count.
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("test module marker");
        assert!(
            !production.contains("std::fs::write(&path"),
            "session writers must use atomic_write_secret, not std::fs::write(&path"
        );
        assert!(
            production.contains("atomic_write_secret"),
            "session writers must call atomic_write_secret"
        );
        assert!(
            production.contains("lock_exclusive"),
            "session writers must take an exclusive per-session lock"
        );
    }

    #[test]
    fn session_exists_tracks_the_file_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::EnvGuard::set("XDG_CONFIG_HOME", tmp.path());
        let id = "exists-check";
        assert!(
            !session_exists(id),
            "no file yet — must report absent so empty new sessions stay unwritten"
        );
        save_session(id, &[], "/tmp", "m", 0).expect("save");
        assert!(
            session_exists(id),
            "after a write the id must report present so /clear can overwrite it"
        );
    }
    #[test]
    fn provider_identity_ignores_trailing_slash() {
        let a = ProviderIdentity::from_api("https://api.example.com/", ApiAuthMode::ApiKey);
        let b = ProviderIdentity::from_api("https://api.example.com", ApiAuthMode::ApiKey);
        assert!(a.matches(&b));
        let c = ProviderIdentity::from_api("https://api.example.com", ApiAuthMode::XaiOauth);
        assert!(!a.matches(&c));
    }

    #[test]
    fn check_provider_match_mismatch_and_unknown() {
        let mut data = make_session(vec![]);
        // Legacy session: no fingerprint.
        assert_eq!(
            check_provider_for_resume(&data, "https://api.x.ai", ApiAuthMode::ApiKey).unwrap(),
            ProviderResume::Unknown
        );

        data.provider = Some(ProviderIdentity::from_api(
            "https://api.x.ai",
            ApiAuthMode::ApiKey,
        ));
        assert_eq!(
            check_provider_for_resume(&data, "https://api.x.ai/", ApiAuthMode::ApiKey).unwrap(),
            ProviderResume::Match
        );

        let err =
            check_provider_for_resume(&data, "https://api.openai.com/v1", ApiAuthMode::ApiKey)
                .expect_err("different base_url must refuse");
        assert!(
            err.contains("api.x.ai") && err.contains("api.openai.com"),
            "refusal must name both sides: {err}"
        );

        let err = check_provider_for_resume(&data, "https://api.x.ai", ApiAuthMode::XaiOauth)
            .expect_err("different auth_mode must refuse");
        assert!(
            err.contains("api_key") && err.contains("xai_oauth"),
            "refusal must name both modes: {err}"
        );
    }

    #[test]
    fn save_stamps_provider_and_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::EnvGuard::set("XDG_CONFIG_HOME", tmp.path());
        let id = "prov-stamp";
        let identity = ProviderIdentity::from_api("https://api.x.ai", ApiAuthMode::ApiKey);
        save_session_full(
            id,
            &[user_message("hi")],
            "/work",
            "grok-4",
            1,
            0.0,
            0,
            0,
            false,
            Some(identity.clone()),
        )
        .expect("save");
        let loaded = load_session(id).expect("load");
        assert_eq!(loaded.provider.as_ref(), Some(&identity));

        // A later save without a provider stamp must keep the old one
        // (metadata-only writers share this path).
        save_session_full(
            id,
            &[user_message("hi")],
            "/work",
            "grok-4",
            2,
            0.0,
            0,
            0,
            false,
            None,
        )
        .expect("resave");
        let again = load_session(id).expect("load again");
        assert_eq!(again.provider.as_ref(), Some(&identity));
        assert_eq!(again.turn_count, 2);
    }

    #[test]
    fn old_session_json_without_provider_still_loads() {
        let json = serde_json::json!({
            "id": "legacy",
            "created_at": "2026-04-15T00:00:00Z",
            "updated_at": "2026-04-15T00:00:00Z",
            "cwd": "/work",
            "model": "m",
            "messages": [],
            "turn_count": 1,
        });
        let data: SessionData = serde_json::from_value(json).unwrap();
        assert!(data.provider.is_none());
        assert_eq!(
            check_provider_for_resume(&data, "https://x", ApiAuthMode::ApiKey).unwrap(),
            ProviderResume::Unknown
        );
    }
}
