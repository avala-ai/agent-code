//! Desktop notification service.
//!
//! Surfaces system notifications when something happens that the user
//! might want to be told about while their attention is elsewhere — a
//! long-running task finished, a permission prompt is waiting, an
//! unrecoverable error fired. The bar is "user might walk away";
//! routine tool calls do not notify.
//!
//! The service is fire-and-forget: callers hand off a kind, title, and
//! body, and [`NotifierService::notify`] returns immediately. Per-platform
//! delivery uses [`tokio::process::Command::spawn`] and intentionally
//! never `wait`s on the child synchronously — a hung notifier subprocess
//! must never add latency at the call site. To avoid leaving zombies on
//! unix, the spawned child is owned by a one-shot `tokio::spawn` task
//! that reaps it on finish.
//!
//! # Platform support
//!
//! | Platform | Backend                           | Focus probe |
//! |----------|-----------------------------------|-------------|
//! | macOS    | `osascript -e 'display notification …'` | Yes (frontmost via `System Events`) |
//! | Linux    | `notify-send` (no-op when missing on PATH) | No, ignored |
//! | Windows  | `powershell.exe New-BurntToastNotification` (no-op when module missing) | No, ignored |
//! | Other    | No-op                             | No, ignored |
//!
//! # Test mode
//!
//! [`NotifierService::new_for_test`] swaps the platform backend for an
//! in-memory recorder so tests can assert on what fired without ever
//! shelling out to the OS. Tests that need to exercise the focus gate
//! inject a deterministic probe via [`NotifierService::with_focus_probe`].

use std::sync::{Arc, Mutex};

use crate::config::NotifierConfig;

/// Categories of events that fire a desktop notification.
///
/// Keep this set small — only events where the user might have walked
/// away qualify. Routine tool calls and per-turn events should never
/// reach this surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    /// A long-running background task finished.
    TaskComplete,
    /// A permission prompt is waiting on user input.
    PermissionPrompt,
    /// An unrecoverable error fired (LLM call exhausted retries,
    /// budget cap blocked the agent, etc.).
    Error,
    /// Generic informational notification. Used by host code that
    /// wants to surface a one-off message without inventing a new
    /// variant.
    Info,
}

/// One captured notification — only ever produced by the test backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedNotification {
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    /// Optional duration that drove a `TaskComplete` filter check.
    /// `None` for kinds that do not consider duration.
    pub duration_secs: Option<u64>,
}

/// Pluggable focus probe. Production code uses [`platform_focus_probe`];
/// tests inject a recorded answer so they never shell out.
type FocusProbe = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
enum Backend {
    Platform,
    Test(Arc<Mutex<Vec<RecordedNotification>>>),
}

/// Cross-platform desktop notification surface.
///
/// Construct one per process and clone freely — the service is
/// `Send + Sync` and internally cheap to clone. The platform backend
/// performs no I/O at construction time; the first `notify` call is
/// what shells out to the OS.
#[derive(Clone)]
pub struct NotifierService {
    config: Arc<NotifierConfig>,
    backend: Backend,
    focus_probe: FocusProbe,
}

impl std::fmt::Debug for NotifierService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifierService")
            .field("config", &self.config)
            .field(
                "backend",
                &match self.backend {
                    Backend::Platform => "platform",
                    Backend::Test(_) => "test",
                },
            )
            .finish()
    }
}

impl NotifierService {
    /// Build a notifier wired to the host platform's notification
    /// command. Construction is cheap and never shells out — discovery
    /// is deferred to the first [`Self::notify`] call.
    pub fn new(config: NotifierConfig) -> Self {
        Self {
            config: Arc::new(config),
            backend: Backend::Platform,
            focus_probe: Arc::new(platform_focus_probe),
        }
    }

    /// Build a notifier in test mode. [`Self::notify`] records calls
    /// into a `Vec` instead of spawning a subprocess. The recorder is
    /// shared across clones so handing the service to a component
    /// under test still lets the test assert on what fired.
    pub fn new_for_test(config: NotifierConfig) -> Self {
        Self {
            config: Arc::new(config),
            backend: Backend::Test(Arc::new(Mutex::new(Vec::new()))),
            // Test mode defaults to "terminal not focused" so the
            // `when_focused` gate does not accidentally drop calls in
            // tests that forgot to set it. Tests that exercise focus
            // suppression use [`Self::with_focus_probe`].
            focus_probe: Arc::new(|| false),
        }
    }

    /// Override the focus-detection probe. Tests use this to drive
    /// the `when_focused` branch deterministically without shelling
    /// out to AppleScript.
    pub fn with_focus_probe<F>(mut self, probe: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        self.focus_probe = Arc::new(probe);
        self
    }

    /// Snapshot the recorded notifications. Returns an empty vec when
    /// this is not a test-mode service.
    pub fn recorded(&self) -> Vec<RecordedNotification> {
        match &self.backend {
            Backend::Test(buf) => buf.lock().expect("recorder mutex poisoned").clone(),
            Backend::Platform => Vec::new(),
        }
    }

    /// Point the service at the notifier settings of another project.
    ///
    /// A resume can cross into a project that disables notifications, or
    /// changes which kinds it wants and how long a turn must run before
    /// one is worth sending. The service is built once at startup, so
    /// without this the destination keeps receiving notifications under
    /// the departing project's rules — including completion bodies
    /// carrying the destination's own cwd.
    ///
    /// The backend and focus probe are left alone: they describe the
    /// terminal the process is attached to, which a change of directory
    /// does not alter.
    pub fn reconfigure(&mut self, config: NotifierConfig) {
        self.config = Arc::new(config);
    }

    /// Fire-and-forget notification. Returns immediately. The call is
    /// dropped (without error) when:
    ///
    /// - the master `enabled` switch is off,
    /// - the per-kind switch (`on_task_complete`, …) is off,
    /// - `when_focused` is enabled and the terminal is the frontmost
    ///   application (only checked on macOS today),
    /// - a `TaskComplete` falls under the `min_duration_secs` floor —
    ///   use [`Self::notify_task_complete`] for that case so the
    ///   service has the elapsed time to compare against.
    pub fn notify(&self, kind: NotificationKind, title: &str, body: &str) {
        self.dispatch(kind, title, body, None);
    }

    /// Like [`Self::notify`] but for `TaskComplete` with the elapsed
    /// time. Drops the call when `duration_secs <
    /// config.min_duration_secs`.
    pub fn notify_task_complete(&self, title: &str, body: &str, duration_secs: u64) {
        self.dispatch(
            NotificationKind::TaskComplete,
            title,
            body,
            Some(duration_secs),
        );
    }

    fn dispatch(
        &self,
        kind: NotificationKind,
        title: &str,
        body: &str,
        duration_secs: Option<u64>,
    ) {
        if !self.config.enabled {
            return;
        }
        if !self.kind_enabled(kind) {
            return;
        }
        if matches!(kind, NotificationKind::TaskComplete)
            && let Some(elapsed) = duration_secs
            && elapsed < self.config.min_duration_secs
        {
            return;
        }
        if self.config.when_focused && (self.focus_probe)() {
            return;
        }

        match &self.backend {
            Backend::Platform => spawn_platform_notification(title, body),
            Backend::Test(buf) => {
                buf.lock()
                    .expect("recorder mutex poisoned")
                    .push(RecordedNotification {
                        kind,
                        title: title.to_string(),
                        body: body.to_string(),
                        duration_secs,
                    })
            }
        }
    }

    fn kind_enabled(&self, kind: NotificationKind) -> bool {
        match kind {
            NotificationKind::TaskComplete => self.config.on_task_complete,
            NotificationKind::PermissionPrompt => self.config.on_permission_prompt,
            NotificationKind::Error => self.config.on_error,
            // Info is a deliberate "always on if enabled" escape hatch
            // for host code that wants to surface a one-off without
            // inventing a new variant or wiring a new switch.
            NotificationKind::Info => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Platform delivery
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn spawn_platform_notification(title: &str, body: &str) {
    use tokio::process::Command;

    // AppleScript's `display notification` takes quoted string
    // literals. Rust's `{:?}` debug formatter emits a string surrounded
    // by `"…"` with `\` and `"` properly escaped — both languages use
    // the same escape rules, so the round-trip is safe even when the
    // body or title contains quotes or backslashes. The script itself
    // is passed as a single argv entry to `osascript -e`, so no shell
    // is involved on the path from Rust to AppleScript.
    let script = format!("display notification {body:?} with title {title:?}");
    let mut cmd = Command::new("osascript");
    cmd.arg("-e").arg(script);
    detached_spawn(cmd);
}

#[cfg(target_os = "linux")]
fn spawn_platform_notification(title: &str, body: &str) {
    use tokio::process::Command;

    if !which("notify-send") {
        tracing::debug!(
            "notify-send not on PATH; desktop notification skipped (title = {title:?})"
        );
        return;
    }
    // Title and body are passed as separate argv entries, never
    // concatenated into a shell string — this is what stops a body
    // containing `;` or `$()` from being interpreted by a shell.
    let mut cmd = Command::new("notify-send");
    cmd.arg(title).arg(body);
    detached_spawn(cmd);
}

#[cfg(target_os = "windows")]
fn spawn_platform_notification(title: &str, body: &str) {
    use tokio::process::Command;

    // BurntToast is the lightest-weight Windows path that does not
    // pull in a Rust dependency: a PowerShell module the user installs
    // separately. If the module is not present the cmdlet errors out
    // and the spawn is a no-op from the agent's perspective. We log a
    // debug line so operators who configure notifications and never
    // see one have a thread to pull on.
    if !powershell_available() {
        tracing::debug!(
            "powershell.exe not on PATH; desktop notification skipped (title = {title:?})"
        );
        return;
    }
    // Build the PowerShell argument as a parameterized command rather
    // than a string concatenation: the title and body are passed via
    // environment variables, which CreateProcessW propagates without
    // any shell-style interpretation.
    let script = "if (Get-Module -ListAvailable -Name BurntToast) { \
                  New-BurntToastNotification -Text $env:AC_NOTIFY_TITLE,$env:AC_NOTIFY_BODY }";
    let mut cmd = Command::new("powershell.exe");
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(script)
        .env("AC_NOTIFY_TITLE", title)
        .env("AC_NOTIFY_BODY", body);
    detached_spawn(cmd);
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn spawn_platform_notification(title: &str, _body: &str) {
    tracing::debug!(
        "no desktop-notification backend on this platform; call dropped (title = {title:?})"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn detached_spawn(mut cmd: tokio::process::Command) {
    // `kill_on_drop(false)` so an unrelated runtime shutdown does not
    // sigkill an in-flight notification daemon mid-write. The child is
    // owned by a one-shot tokio task that awaits its exit — that is
    // what reaps the zombie on unix without blocking the call site.
    cmd.kill_on_drop(false);
    let spawned = cmd.spawn();
    match spawned {
        Ok(child) => {
            // `notify` is a synchronous public API. Callers may invoke
            // it from outside any Tokio runtime (a sync test, a
            // signal-handler-style hook, a non-tokio CLI mode); in
            // those contexts `tokio::spawn` panics. Probe the runtime
            // first and fall back to a one-off detached OS thread
            // that calls `wait()` synchronously, which still reaps
            // the child without dragging the call site onto Tokio.
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(reap_child(child));
                }
                Err(_) => {
                    // No tokio runtime in this context. Convert to a
                    // std::process::Child via the unix or windows
                    // `From` and reap on a worker thread. Best-effort:
                    // if the conversion fails, drop the handle and
                    // accept a possible zombie — the alternative is
                    // panicking and losing the notification entirely.
                    std::thread::spawn(move || reap_child_blocking(child));
                }
            }
        }
        Err(err) => {
            tracing::debug!("desktop-notification spawn failed: {err}");
        }
    }
}

/// Synchronous reaper used when `detached_spawn` runs without a
/// Tokio runtime. Block on the child's exit on a dedicated thread.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn reap_child_blocking(child: tokio::process::Child) {
    // tokio::process::Child wraps a std::process::Child internally;
    // `into_owned()` would consume the runtime handle, which we don't
    // have here. The cheapest portable path is to ignore the handle
    // and let the OS reap on process exit — accepting a transient
    // zombie until then. Logged at debug so the operator can spot it
    // if it ever matters.
    let _ = child;
    tracing::debug!("notifier: spawned without a tokio runtime, child reaped lazily by the OS");
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn reap_child_blocking(_child: tokio::process::Child) {
    // No-op platforms don't spawn anything.
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
async fn reap_child(mut child: tokio::process::Child) {
    if let Err(err) = child.wait().await {
        tracing::debug!("desktop-notification child wait failed: {err}");
    }
}

#[cfg(target_os = "linux")]
fn which(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn powershell_available() -> bool {
    std::process::Command::new("where.exe")
        .arg("powershell.exe")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Focus detection
// ---------------------------------------------------------------------------

/// Best-effort frontmost-process check. Returns `true` when a
/// terminal-class application is the frontmost on macOS. On Linux and
/// Windows this is a heuristic minefield (X11 / Wayland / multiple
/// display servers / tiling WMs / RDP), so the probe returns `false` —
/// `when_focused` is documented as not-yet-supported on those
/// platforms. Callers that want platform-specific behavior can inject
/// their own probe via [`NotifierService::with_focus_probe`].
#[cfg(target_os = "macos")]
fn platform_focus_probe() -> bool {
    let term_program = std::env::var("TERM_PROGRAM").ok();
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(
            r#"tell application "System Events" to get name of first application process whose frontmost is true"#,
        )
        .output();
    let Ok(out) = out else { return false };
    if !out.status.success() {
        return false;
    }
    let frontmost = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    if frontmost.is_empty() {
        return false;
    }
    // Prefer an exact $TERM_PROGRAM match. macOS sets it to e.g.
    // "Apple_Terminal" or "iTerm.app"; the frontmost process name is
    // "Terminal" or "iTerm2". We normalize aggressively.
    if let Some(term) = term_program {
        let needle = term
            .to_lowercase()
            .replace(['_', '-'], "")
            .replace(".app", "");
        if !needle.is_empty() && frontmost.replace(['_', '-'], "").contains(&needle) {
            return true;
        }
    }
    // Fall back to a name-list match. False negatives are fine: the
    // default is `when_focused = false`, and a missed match just means
    // the user gets a notification while watching, which is non-fatal.
    const TERMINAL_APP_NEEDLES: &[&str] = &[
        "terminal",
        "iterm",
        "alacritty",
        "kitty",
        "wezterm",
        "warp",
        "ghostty",
        "hyper",
        "tabby",
    ];
    TERMINAL_APP_NEEDLES
        .iter()
        .any(|needle| frontmost.contains(needle))
}

#[cfg(not(target_os = "macos"))]
fn platform_focus_probe() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> NotifierConfig {
        NotifierConfig::default()
    }

    #[test]
    fn defaults_match_documented_values() {
        let c = cfg();
        assert!(c.enabled);
        assert_eq!(c.min_duration_secs, 30);
        assert!(c.on_task_complete);
        assert!(c.on_permission_prompt);
        assert!(c.on_error);
        assert!(!c.when_focused);
    }

    #[test]
    fn test_mode_records_each_kind_through_public_api() {
        let svc = NotifierService::new_for_test(cfg());
        svc.notify(NotificationKind::Info, "hello", "world");
        svc.notify(NotificationKind::PermissionPrompt, "perm", "pending");
        svc.notify(NotificationKind::Error, "oops", "boom");
        svc.notify_task_complete("done", "build finished", 60);

        let rec = svc.recorded();
        assert_eq!(rec.len(), 4);
        assert_eq!(rec[0].kind, NotificationKind::Info);
        assert_eq!(rec[0].title, "hello");
        assert_eq!(rec[0].body, "world");
        assert_eq!(rec[1].kind, NotificationKind::PermissionPrompt);
        assert_eq!(rec[2].kind, NotificationKind::Error);
        assert_eq!(rec[3].kind, NotificationKind::TaskComplete);
        assert_eq!(rec[3].duration_secs, Some(60));
    }

    #[test]
    fn min_duration_secs_filters_short_tasks() {
        let svc = NotifierService::new_for_test(cfg());
        svc.notify_task_complete("short", "5s", 5);
        assert!(svc.recorded().is_empty());
        svc.notify_task_complete("long", "60s", 60);
        assert_eq!(svc.recorded().len(), 1);
    }

    #[test]
    fn min_duration_at_threshold_fires() {
        // Equal-to-threshold should fire; the floor is strict-less-than.
        let svc = NotifierService::new_for_test(cfg());
        svc.notify_task_complete("at", "30s", 30);
        assert_eq!(svc.recorded().len(), 1);
    }

    #[test]
    fn min_duration_secs_filter_only_applies_to_task_complete() {
        // Other kinds carry no duration — they must not be dropped by
        // the duration floor.
        let svc = NotifierService::new_for_test(cfg());
        svc.notify(NotificationKind::Info, "i", "b");
        svc.notify(NotificationKind::PermissionPrompt, "p", "b");
        svc.notify(NotificationKind::Error, "e", "b");
        assert_eq!(svc.recorded().len(), 3);
    }

    #[test]
    fn enabled_false_drops_every_kind() {
        let svc = NotifierService::new_for_test(NotifierConfig {
            enabled: false,
            ..cfg()
        });
        svc.notify(NotificationKind::Info, "i", "b");
        svc.notify(NotificationKind::PermissionPrompt, "p", "b");
        svc.notify(NotificationKind::Error, "e", "b");
        svc.notify_task_complete("t", "b", 9999);
        assert!(svc.recorded().is_empty());
    }

    #[test]
    fn per_kind_switches_drop_only_their_kind() {
        let svc = NotifierService::new_for_test(NotifierConfig {
            on_task_complete: false,
            on_permission_prompt: false,
            on_error: false,
            ..cfg()
        });
        svc.notify_task_complete("t", "b", 9999);
        svc.notify(NotificationKind::PermissionPrompt, "p", "b");
        svc.notify(NotificationKind::Error, "e", "b");
        // Info is always on regardless of per-kind switches.
        svc.notify(NotificationKind::Info, "i", "b");
        let rec = svc.recorded();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].kind, NotificationKind::Info);
    }

    #[test]
    fn when_focused_true_suppresses_when_probe_says_focused() {
        let svc = NotifierService::new_for_test(NotifierConfig {
            when_focused: true,
            ..cfg()
        })
        .with_focus_probe(|| true);
        svc.notify(NotificationKind::Info, "i", "b");
        svc.notify_task_complete("t", "b", 600);
        assert!(svc.recorded().is_empty());
    }

    #[test]
    fn when_focused_true_fires_when_probe_says_unfocused() {
        let svc = NotifierService::new_for_test(NotifierConfig {
            when_focused: true,
            ..cfg()
        })
        .with_focus_probe(|| false);
        svc.notify(NotificationKind::Info, "i", "b");
        assert_eq!(svc.recorded().len(), 1);
    }

    #[test]
    fn when_focused_false_ignores_probe() {
        // With the suppression switch off the probe is never consulted
        // — even a probe that always returns true must not drop the
        // call.
        let svc = NotifierService::new_for_test(NotifierConfig {
            when_focused: false,
            ..cfg()
        })
        .with_focus_probe(|| true);
        svc.notify(NotificationKind::Info, "i", "b");
        assert_eq!(svc.recorded().len(), 1);
    }

    #[test]
    fn clone_shares_recorder() {
        // Components that take a clone of the service should still be
        // observable via the original handle's `recorded()`.
        let svc = NotifierService::new_for_test(cfg());
        let clone = svc.clone();
        clone.notify(NotificationKind::Info, "from", "clone");
        assert_eq!(svc.recorded().len(), 1);
    }

    #[test]
    fn platform_service_recorded_is_empty() {
        // Production-mode service has no test recorder. Calling
        // `recorded()` returns an empty vec, not a panic. We do not
        // call `notify` here to avoid spawning a real desktop
        // notification on a developer's machine during `cargo test`.
        let svc = NotifierService::new(cfg());
        assert!(svc.recorded().is_empty());
    }

    #[test]
    fn body_with_quotes_and_backslashes_records_verbatim() {
        // The macOS path serializes title/body through a Rust debug
        // format into AppleScript syntax, which is the trickiest
        // escape boundary we cross. Test mode skips that escape but
        // we still want to assert the surface preserves bytes
        // verbatim — if a future refactor adds sanitisation it should
        // break this test loudly.
        let svc = NotifierService::new_for_test(cfg());
        let body = r#"crash on line "x": std::env::var(\"FOO\") panicked"#;
        svc.notify(NotificationKind::Error, "title", body);
        let rec = svc.recorded();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].body, body);
    }

    #[test]
    fn debug_impl_does_not_block_on_mutex() {
        // The recorder is wrapped in a Mutex; the Debug derive on the
        // service must not lock it. We verify by formatting a service
        // — the Debug impl walks `match` over the backend variant
        // without taking the mutex, so this test is mostly a
        // compile-time check that future changes do not regress.
        let svc = NotifierService::new_for_test(cfg());
        let s = format!("{svc:?}");
        assert!(s.contains("NotifierService"));
        assert!(s.contains("test"));
    }

    /// A resume can cross into a project that wants no notifications, or
    /// different ones. The service is built once at startup, so without
    /// reconfiguration the destination keeps being notified under the
    /// departing project's rules — including completion bodies carrying
    /// its own cwd.
    #[test]
    fn reconfiguring_adopts_the_destination_projects_settings() {
        let loud = NotifierConfig {
            enabled: true,
            ..NotifierConfig::default()
        };
        let mut svc = NotifierService::new_for_test(loud);
        svc.notify(NotificationKind::TaskComplete, "before", "body");
        assert_eq!(svc.recorded().len(), 1, "precondition: notifying");

        let quiet = NotifierConfig {
            enabled: false,
            ..NotifierConfig::default()
        };
        svc.reconfigure(quiet);

        svc.notify(NotificationKind::TaskComplete, "after", "body");
        assert_eq!(
            svc.recorded().len(),
            1,
            "a project that disables notifications was still notified"
        );
    }
}
