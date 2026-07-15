//! First-run onboarding flow.
//!
//! On a fresh install we want a single deliberate moment to:
//!
//! 1. Welcome the user with a small original ASCII mark and the
//!    binary version, so they know what they just launched.
//! 2. Let them pick a colour theme — including accessibility-first
//!    options (Okabe-Ito colourblind-safe palettes and ANSI-only
//!    fallbacks for terminals without truecolour).
//! 3. Show a *live* diff preview that re-paints with the highlighted
//!    theme's colours so the choice is visible before it commits.
//! 4. Persist the choice into `~/.config/agent-code/config.toml`
//!    under `[ui].theme` and drop a sentinel
//!    `.onboarding-complete` file next to it so the prompt only
//!    fires once.
//!
//! The same picker is reachable mid-session via the `/theme` slash
//! command, so the flow is implemented once and reused.
//!
//! Skip rules: when stdout is not an interactive TTY (CI, serve mode,
//! piped input) we never enter the picker — we still drop the sentinel
//! so subsequent interactive launches don't re-prompt.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use agent_code_lib::config::{agent_config_dir, atomic::atomic_write_secret};
use crossterm::style::Stylize;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal,
};

use super::theme::Theme;

/// Sentinel file dropped into the user's config dir after the onboarding
/// flow completes (or is intentionally skipped). Hidden so it doesn't
/// clutter `ls`.
const SENTINEL_NAME: &str = ".onboarding-complete";

/// Default theme written when onboarding is skipped non-interactively.
pub const DEFAULT_THEME: &str = "auto";

/// Resolve the directory that holds `config.toml` and our sentinel.
///
/// Use the shared helper so `$XDG_CONFIG_HOME` is honored consistently on every
/// platform, including Windows test sandboxes.
fn config_dir() -> Option<PathBuf> {
    agent_config_dir()
}

/// Path to the onboarding sentinel file. `None` only if the platform
/// has no notion of a user config dir (effectively never on supported
/// platforms; we fail open in that case).
pub fn sentinel_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(SENTINEL_NAME))
}

/// True when the sentinel exists — i.e. onboarding has already run.
pub fn already_onboarded() -> bool {
    sentinel_path().map(|p| p.exists()).unwrap_or(true)
}

/// Atomically write the sentinel file. Failure is non-fatal — we just
/// log and let the next launch re-prompt rather than crashing the
/// agent on a read-only home directory.
pub fn mark_onboarded() {
    if let Some(path) = sentinel_path()
        && let Some(parent) = path.parent()
    {
        if !parent.exists() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = atomic_write_secret(&path, b"") {
            tracing::debug!("onboarding sentinel write failed: {e}");
        }
    }
}

/// Outcome of [`run_first_run`].
#[derive(Debug, Clone)]
pub struct OnboardingResult {
    /// Theme name that was selected (or the default when skipped).
    pub theme: String,
    /// Whether the user actually saw the picker. False when skipped
    /// because stdout was not a TTY or the user pressed Escape.
    pub interactive: bool,
}

/// Run the first-run flow: welcome screen + theme picker. Always
/// drops the sentinel before returning so we don't re-prompt next
/// launch even if the user bailed out early.
pub fn run_first_run() -> OnboardingResult {
    if !is_interactive() {
        mark_onboarded();
        return OnboardingResult {
            theme: DEFAULT_THEME.to_string(),
            interactive: false,
        };
    }

    print_welcome();

    let chosen =
        pick_theme(default_theme_for_picker()).unwrap_or_else(|| DEFAULT_THEME.to_string());

    if let Err(e) = persist_theme(&chosen) {
        tracing::warn!("could not persist theme '{chosen}': {e}");
    }
    mark_onboarded();

    OnboardingResult {
        theme: chosen,
        interactive: true,
    }
}

/// Re-open the theme picker mid-session for the `/theme` slash
/// command. Returns `Some(name)` if the user confirmed a choice and
/// it was persisted; `None` if they cancelled.
pub fn rerun_theme_picker(current: &str) -> Option<String> {
    if !is_interactive() {
        return None;
    }
    let chosen = pick_theme(current.to_string())?;
    if let Err(e) = persist_theme(&chosen) {
        tracing::warn!("could not persist theme '{chosen}': {e}");
        return None;
    }
    Some(chosen)
}

/// Default highlight position for the picker — index of the current
/// theme name in [`THEME_OPTIONS`], falling back to "Auto" (0).
fn default_theme_for_picker() -> String {
    "auto".to_string()
}

/// True when stdout is a terminal we can drive interactively.
fn is_interactive() -> bool {
    io::stdout().is_terminal() && io::stdin().is_terminal()
}

// ---------------------------------------------------------------------------
// Welcome screen
// ---------------------------------------------------------------------------

const DECOR_LINE: &str = "…………………………………………………………………………………………………………………………………………………………";

/// Original 9-line "AC" monogram. Designed to read as the letters
/// "AC" inside a soft geometric frame; intentionally lowercase to
/// stay close to the unix-tool aesthetic. Width is ~50 cols.
const LOGO: &[&str] = &[
    "      .─────────────────────────────────────.     ",
    "     │                                       │    ",
    "     │     ╔═╗  ╔═╗                          │    ",
    "     │    ╔╝ ╚╗ ║                            │    ",
    "     │    ║   ║ ║                            │    ",
    "     │    ╠═══╣ ║                            │    ",
    "     │    ║   ║ ║                            │    ",
    "     │    ║   ║ ╚═╝                          │    ",
    "     `─────────────────────────────────────'     ",
];

fn print_welcome() {
    let version = env!("CARGO_PKG_VERSION");
    let t = super::theme::current();
    println!();
    println!(
        "  {}{}",
        "Welcome to agent-code v".with(t.text),
        version.with(t.accent),
    );
    println!("  {}", DECOR_LINE.with(t.muted));
    println!();
    for line in LOGO {
        println!("  {}", line.with(t.accent));
    }
    println!();
    println!("  {}", DECOR_LINE.with(t.muted));
    println!();
    println!(
        "  {}",
        "Fullscreen TUI · multi-provider · MIT · no default telemetry".with(t.muted)
    );
    println!();
    println!(
        "  {}",
        "Pick a look — live preview updates as you move ↑/↓.".with(t.text)
    );
    println!(
        "  {}",
        "Enter confirms · Esc skips (uses Auto).".with(t.muted)
    );
    println!();
}

// ---------------------------------------------------------------------------
// Theme picker
// ---------------------------------------------------------------------------

/// One row in the picker: display label, theme name persisted to
/// settings, and which short hint to show beneath the option list.
struct PickerOption {
    label: &'static str,
    value: &'static str,
}

// TODO(8.15.5): expose `[ui].inherit_fg` here once the picker grows
// a secondary toggle row below the theme list. Doing so today would
// require a small state machine on top of the current arrow-key
// loop; for now users opt in by editing
// `~/.config/agent-code/config.toml` directly. See
// `docs/configuration/settings.mdx` for the option description.

/// Order shown to the user. `auto` first because it is the safest
/// default — it follows the terminal's own background detection.
const THEME_OPTIONS: &[PickerOption] = &[
    PickerOption {
        label: "Auto (match terminal)",
        value: "auto",
    },
    PickerOption {
        label: "Dark mode",
        value: "midnight",
    },
    PickerOption {
        label: "Light mode",
        value: "daybreak",
    },
    PickerOption {
        label: "Dark mode (colorblind-friendly)",
        value: "dark-colorblind",
    },
    PickerOption {
        label: "Light mode (colorblind-friendly)",
        value: "light-colorblind",
    },
    PickerOption {
        label: "Dark mode (ANSI colors only)",
        value: "dark-ansi",
    },
    PickerOption {
        label: "Light mode (ANSI colors only)",
        value: "light-ansi",
    },
];

/// Tiny canned diff used in the live preview. Designed to be
/// language-agnostic, short enough to fit in five rendered rows, and
/// to exercise the additions slot, removals slot, and a context line.
const PREVIEW_BEFORE: &str =
    "fn render(view: &View) {\n    let title = view.heading();\n    println!(\"{title}\");\n}\n";
const PREVIEW_AFTER: &str = "fn render(view: &View) {\n    let title = view.heading_styled();\n    writeln!(view.out, \"{title}\")?;\n}\n";

/// Show the picker. Returns `None` on Escape or other cancel.
fn pick_theme(initial: String) -> Option<String> {
    let mut selected = THEME_OPTIONS
        .iter()
        .position(|o| o.value == initial)
        .unwrap_or(1);
    let confirmed_initial = selected;

    if terminal::enable_raw_mode().is_err() {
        // Without raw mode we can't track arrow keys. Fall back to a
        // non-interactive default so we never hang.
        return Some(THEME_OPTIONS[selected].value.to_string());
    }

    let stdout = io::stdout();
    {
        let mut out = stdout.lock();
        writeln!(
            out,
            "  Choose the text style that looks best with your terminal\r"
        )
        .ok();
        writeln!(out, "  To change this later, run /theme\r").ok();
        writeln!(out, "\r").ok();
        out.flush().ok();
    }

    render_picker(selected, confirmed_initial);

    let mut cancelled = false;
    loop {
        let event = match event::read() {
            Ok(e) => e,
            Err(_) => {
                cancelled = true;
                break;
            }
        };
        if let Event::Key(KeyEvent {
            code,
            kind,
            modifiers,
            ..
        }) = event
        {
            // crossterm reports both Press and Release on Windows; act
            // on Press only so a single keystroke moves the cursor
            // exactly once.
            if kind != KeyEventKind::Press {
                continue;
            }
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = if selected == 0 {
                        THEME_OPTIONS.len() - 1
                    } else {
                        selected - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1) % THEME_OPTIONS.len();
                }
                KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                    let idx = (c as usize) - ('1' as usize);
                    if idx < THEME_OPTIONS.len() {
                        selected = idx;
                    }
                }
                KeyCode::Enter => break,
                KeyCode::Esc => {
                    cancelled = true;
                    break;
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    cancelled = true;
                    break;
                }
                _ => {}
            }
            clear_picker();
            render_picker(selected, confirmed_initial);
        }
    }

    let _ = terminal::disable_raw_mode();
    clear_picker();

    if cancelled {
        // Confirm in the visible scrollback so the user knows they
        // bailed and what theme is now in effect.
        println!(
            "  {} {}",
            "→".with(super::theme::current().muted),
            "Skipped — using auto".with(super::theme::current().muted),
        );
        return None;
    }

    let chosen = THEME_OPTIONS[selected].value.to_string();
    println!(
        "  {} {}",
        "→".with(super::theme::current().accent),
        chosen.clone().with(super::theme::current().text).bold(),
    );
    Some(chosen)
}

/// Total number of terminal rows the picker occupies. Constant so
/// `clear_picker` can reverse it without remembering per-call state.
fn picker_height() -> usize {
    // Options + blank + 5 preview rows + footer = constant.
    THEME_OPTIONS.len() + 1 + 5 + 1
}

fn clear_picker() {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for _ in 0..picker_height() {
        let _ = write!(out, "\x1b[A\x1b[2K");
    }
    out.flush().ok();
}

fn render_picker(selected: usize, confirmed_initial: usize) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let theme = Theme::from_name(THEME_OPTIONS[selected].value);

    for (i, opt) in THEME_OPTIONS.iter().enumerate() {
        let is_cursor = i == selected;
        let is_initial = i == confirmed_initial;
        let cursor_marker = if is_cursor { "❯" } else { " " };
        let check = if is_initial { " ✔" } else { "" };
        let number = format!("{}.", i + 1);
        if is_cursor {
            let _ = writeln!(
                out,
                "   {} {} {}{}\r",
                cursor_marker.with(theme.accent).bold(),
                number.with(theme.accent),
                opt.label.with(theme.text).bold(),
                check.with(theme.success),
            );
        } else {
            let _ = writeln!(
                out,
                "   {} {} {}{}\r",
                cursor_marker,
                number,
                opt.label.with(theme.muted),
                check.with(theme.success),
            );
        }
    }

    let _ = writeln!(out, "\r");

    // Live diff preview. Colours come from the *highlighted* theme so
    // the user can compare options before committing.
    let dash = "╌".repeat(40);
    let _ = writeln!(
        out,
        "  {} live diff preview {}\r",
        dash.clone().with(theme.muted),
        dash.with(theme.muted)
    );

    let preview = render_preview(&theme);
    for line in preview.lines() {
        let _ = writeln!(out, "  {line}\r");
    }

    let _ = writeln!(
        out,
        "  {}\r",
        "↑/↓ move · 1-7 jump · Enter confirm · Esc skip".with(theme.muted),
    );

    out.flush().ok();
}

/// Build a small unified-diff snippet styled with the given theme.
/// Lazy by construction: no syntect lookups happen here, so the
/// picker re-renders cheaply on every cursor move.
fn render_preview(theme: &Theme) -> String {
    use std::fmt::Write as _;
    let before: Vec<&str> = PREVIEW_BEFORE.lines().collect();
    let after: Vec<&str> = PREVIEW_AFTER.lines().collect();

    let mut out = String::new();
    let mut bi = 0usize;
    let mut ai = 0usize;
    let mut row_no = 1usize;
    while bi < before.len() || ai < after.len() {
        let b = before.get(bi).copied();
        let a = after.get(ai).copied();
        match (b, a) {
            (Some(bl), Some(al)) if bl == al => {
                let _ = writeln!(
                    out,
                    "{:>3}  {}",
                    row_no.to_string().with(theme.muted),
                    bl.with(theme.text),
                );
                bi += 1;
                ai += 1;
                row_no += 1;
            }
            (Some(bl), _) if !after.contains(&bl) => {
                let _ = writeln!(
                    out,
                    "{:>3}  {}",
                    row_no.to_string().with(theme.muted),
                    format!("- {bl}").with(theme.diff_remove),
                );
                bi += 1;
                row_no += 1;
            }
            (_, Some(al)) if !before.contains(&al) => {
                let _ = writeln!(
                    out,
                    "{:>3}  {}",
                    row_no.to_string().with(theme.muted),
                    format!("+ {al}").with(theme.diff_add),
                );
                ai += 1;
                row_no += 1;
            }
            (Some(bl), _) => {
                let _ = writeln!(
                    out,
                    "{:>3}  {}",
                    row_no.to_string().with(theme.muted),
                    bl.with(theme.text),
                );
                bi += 1;
                row_no += 1;
            }
            (None, Some(al)) => {
                let _ = writeln!(
                    out,
                    "{:>3}  {}",
                    row_no.to_string().with(theme.muted),
                    format!("+ {al}").with(theme.diff_add),
                );
                ai += 1;
                row_no += 1;
            }
            (None, None) => break,
        }
    }
    // Pad to a fixed 5-line height so the picker doesn't jump.
    while out.lines().count() < 5 {
        out.push('\n');
    }
    // Truncate just in case.
    let mut trimmed: String = out.lines().take(5).collect::<Vec<_>>().join("\n");
    trimmed.push('\n');
    trimmed
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Read the existing user config (if any), set `[ui].theme = "<name>"`
/// while preserving every other key, and write the result back
/// atomically using the secret-preserving helper from agent-code-lib.
fn persist_theme(theme_name: &str) -> Result<(), String> {
    let dir = config_dir().ok_or_else(|| "no user config directory".to_string())?;
    let path = dir.join("config.toml");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {dir:?}: {e}"))?;
    }

    let mut doc: toml::Value = if path.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        toml::from_str(&raw).map_err(|e| format!("parse {path:?}: {e}"))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = doc
        .as_table_mut()
        .ok_or_else(|| "config.toml is not a table".to_string())?;
    let ui = table
        .entry("ui")
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let ui_table = ui
        .as_table_mut()
        .ok_or_else(|| "[ui] section is not a table".to_string())?;
    ui_table.insert(
        "theme".to_string(),
        toml::Value::String(theme_name.to_string()),
    );

    let serialized = toml::to_string_pretty(&doc).map_err(|e| format!("serialize: {e}"))?;
    atomic_write_secret(&path, serialized.as_bytes())
        .map_err(|e| format!("atomic write {path:?}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `agent_config_dir()` resolves to `$XDG_CONFIG_HOME` when set on every
    /// platform. Pinning that env var to a tempdir gives us a clean sandbox per
    /// test.
    struct ConfigSandbox {
        _tmp: tempfile::TempDir,
        // Saved values restored on Drop so a test can't leak its
        // override into the next one.
        prev_xdg: Option<String>,
        prev_home: Option<String>,
    }

    impl ConfigSandbox {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
            let prev_home = std::env::var("HOME").ok();
            // SAFETY: sandbox-using tests hold ENV_LOCK while mutating process
            // env, so no other test in this module observes a partial update.
            unsafe {
                std::env::set_var("XDG_CONFIG_HOME", tmp.path());
                std::env::set_var("HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                prev_xdg,
                prev_home,
            }
        }
    }

    impl Drop for ConfigSandbox {
        fn drop(&mut self) {
            // SAFETY: the owning test still holds ENV_LOCK while this Drop
            // restores process-wide environment values.
            unsafe {
                match &self.prev_xdg {
                    Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                    None => std::env::remove_var("XDG_CONFIG_HOME"),
                }
                match &self.prev_home {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    // Serialise tests that mutate process-wide env vars. Without this
    // they race each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn sentinel_absent_then_present_after_mark() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        assert!(
            !already_onboarded(),
            "fresh sandbox should not be onboarded"
        );
        mark_onboarded();
        assert!(already_onboarded(), "mark_onboarded must drop sentinel");
        let path = sentinel_path().unwrap();
        assert!(path.exists());
    }

    #[test]
    fn mark_onboarded_creates_parent_dir() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        // Sanity: parent dir does not exist yet.
        let dir = config_dir().unwrap();
        assert!(!dir.exists());
        mark_onboarded();
        assert!(dir.exists());
        assert!(sentinel_path().unwrap().exists());
    }

    #[test]
    fn persist_theme_writes_ui_table_and_preserves_other_keys() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        let dir = config_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[api]\nmodel = \"gpt-test\"\n\n[ui]\nmarkdown = false\n",
        )
        .unwrap();
        persist_theme("dark-colorblind").unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: toml::Value = toml::from_str(&raw).unwrap();
        assert_eq!(
            parsed["api"]["model"].as_str(),
            Some("gpt-test"),
            "persist_theme must not clobber unrelated sections"
        );
        assert_eq!(parsed["ui"]["markdown"].as_bool(), Some(false));
        assert_eq!(
            parsed["ui"]["theme"].as_str(),
            Some("dark-colorblind"),
            "theme value must be the persisted choice"
        );
    }

    #[test]
    fn persist_theme_creates_file_when_absent() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        persist_theme("light-ansi").unwrap();
        let raw = std::fs::read_to_string(config_dir().unwrap().join("config.toml")).unwrap();
        assert!(raw.contains("[ui]"));
        assert!(raw.contains("light-ansi"));
    }

    #[test]
    fn render_preview_height_is_stable_across_themes() {
        // Picker layout depends on this constant. If it ever changes
        // we'd misalign clear_picker → confirm visually here.
        for name in Theme::all_names() {
            let theme = Theme::from_name(name);
            let body = render_preview(&theme);
            // 5 lines of body + the trailing newline = 5 line breaks.
            let line_count = body.matches('\n').count();
            assert!(
                (5..=6).contains(&line_count),
                "theme {name} produced {line_count} preview lines (expected 5)",
            );
        }
    }

    #[test]
    fn picker_options_match_supported_theme_names() {
        // Every option the picker advertises must be a name the
        // resolver knows about. Cheap structural check that catches
        // typos in either list.
        let known: std::collections::HashSet<&str> = Theme::all_names().iter().copied().collect();
        for opt in THEME_OPTIONS {
            assert!(
                known.contains(opt.value),
                "picker option {:?} is not in Theme::all_names()",
                opt.value
            );
        }
    }
}
