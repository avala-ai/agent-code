//! Interactive REPL (Read-Eval-Print Loop).
//!
//! The main user interaction loop. Reads input via rustyline,
//! passes it to the query engine, and streams output to the terminal.
//! Integrates markdown rendering, activity indicators, and permission
//! prompts.

use std::borrow::Cow;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crossterm::style::Stylize;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

use crate::ui::activity::ActivityIndicator;
use agent_code_lib::llm::message::Usage;
use agent_code_lib::query::{QueryEngine, StreamSink};
use agent_code_lib::tools::ToolResult;

/// Write to stdout with LF → CRLF translation. Needed because the escape-key
/// watcher (`spawn_escape_watcher`) holds the terminal in raw mode for the
/// entire duration of a streaming turn, and in raw mode a bare `\n` moves the
/// cursor down one row without returning to column 0 — causing subsequent
/// lines to drift right by the column of the previous line. Any code path
/// that prints during a turn must go through this helper (or the tui
/// renderer, which already uses `\r\n` internally).
fn raw_print(text: &str) {
    let translated = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(translated.as_bytes());
    let _ = out.flush();
}

/// Like `raw_print`, but for stderr.
fn raw_eprint(text: &str) {
    let translated = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(translated.as_bytes());
    let _ = err.flush();
}

/// Tab-completion helper for slash commands.
struct CommandCompleter;

/// Match score for a command candidate against a user-typed partial.
///
/// Higher is better. `None` means no match — candidate is excluded.
/// Priority tiers (decreasing):
///   * 1000 — exact prefix of the name
///   * 500  — substring (contained, not at start) of the name
///   * 100..250 — fuzzy subsequence match against the name (score = partial len scaled)
///   * 50..150 — matches against an alias
fn score_command(name: &str, aliases: &[&str], partial: &str) -> Option<i32> {
    if partial.is_empty() {
        return Some(1000); // surface everything when nothing typed yet
    }
    let p = partial.to_lowercase();
    let n = name.to_lowercase();

    if n.starts_with(&p) {
        return Some(1000);
    }
    if n.contains(&p) {
        return Some(500);
    }
    // Aliases: prefix > contains.
    for a in aliases {
        let al = a.to_lowercase();
        if al.starts_with(&p) {
            return Some(150);
        }
        if al.contains(&p) {
            return Some(100);
        }
    }
    if fuzzy_subsequence(&n, &p) {
        // Reward shorter names (closer match) when scoring subsequences.
        let bonus = 100i32.saturating_sub(n.len() as i32);
        return Some(100 + bonus.max(0));
    }
    None
}

/// Return true if every char of `needle` appears in `haystack` in order
/// (not necessarily contiguous). Used as the lowest-tier fuzzy match.
fn fuzzy_subsequence(haystack: &str, needle: &str) -> bool {
    let mut it = haystack.chars();
    for c in needle.chars() {
        let found = it.by_ref().any(|h| h == c);
        if !found {
            return false;
        }
    }
    true
}

/// Find an `@path-partial` context ending at byte position `pos` in
/// `line`, if any. Returns `(at_byte_idx, partial)` where `at_byte_idx`
/// is the byte position of the `@` and `partial` is the text after it
/// up to `pos`. Returns `None` when the cursor isn't in an `@`-context.
///
/// Rules: the `@` must be at the start of the line or preceded by
/// whitespace (so `email@example.com` doesn't accidentally trigger
/// path completion mid-word).
fn find_at_context(line: &str, pos: usize) -> Option<(usize, &str)> {
    if pos > line.len() {
        return None;
    }
    let prefix = &line[..pos];
    let at_idx = prefix.rfind('@')?;
    // Must be at start, or preceded by whitespace.
    if at_idx > 0 {
        let prev = prefix[..at_idx].chars().next_back()?;
        if !prev.is_whitespace() {
            return None;
        }
    }
    let partial = &prefix[at_idx + 1..];
    // Reject if whitespace is in the partial — that means the cursor
    // has already moved past the path token.
    if partial.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some((at_idx, partial))
}

/// Enumerate file/directory candidates matching an `@` partial path
/// under `cwd`. Returns up to 50 results, directories first (with a
/// trailing `/` in the replacement so Tab-tab descends), then files.
fn complete_at_path(cwd: &str, partial: &str) -> Vec<Pair> {
    // Split partial into (search_dir, prefix_filter).
    let (rel_dir, prefix) = match partial.rfind('/') {
        Some(idx) => (&partial[..idx], &partial[idx + 1..]),
        None => ("", partial),
    };

    // Resolve rel_dir relative to cwd; reject absolute paths or any
    // `..` component to avoid accidentally scanning the filesystem.
    if rel_dir.starts_with('/') || rel_dir.split('/').any(|c| c == "..") {
        return Vec::new();
    }

    let search_dir = if rel_dir.is_empty() {
        std::path::PathBuf::from(cwd)
    } else {
        std::path::PathBuf::from(cwd).join(rel_dir)
    };

    let entries = match std::fs::read_dir(&search_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let prefix_lower = prefix.to_lowercase();
    let mut dirs: Vec<(String, bool)> = Vec::new();
    let mut files: Vec<(String, bool)> = Vec::new();

    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        // Skip dotfiles unless the user typed a `.` prefix.
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            dirs.push((name, true));
        } else {
            files.push((name, false));
        }
    }

    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut pairs: Vec<Pair> = Vec::new();
    for (name, is_dir) in dirs.into_iter().chain(files).take(50) {
        let rel = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };
        let replacement = if is_dir {
            format!("@{rel}/")
        } else {
            format!("@{rel}")
        };
        let display = if is_dir {
            format!("{rel}/  (dir)")
        } else {
            rel.clone()
        };
        pairs.push(Pair {
            display,
            replacement,
        });
    }
    pairs
}

impl Completer for CommandCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // @path completion fires whenever the cursor is inside an @-token,
        // regardless of where that token is in the line.
        if let Some((at_idx, partial)) = find_at_context(line, pos) {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".into());
            let pairs = complete_at_path(&cwd, partial);
            if !pairs.is_empty() {
                return Ok((at_idx, pairs));
            }
        }

        // Otherwise fall back to slash-command completion.
        if !line.starts_with('/') {
            return Ok((0, vec![]));
        }

        let partial = &line[1..pos];

        let mut scored: Vec<(i32, Pair)> = crate::commands::COMMANDS
            .iter()
            .filter(|c| !c.hidden)
            .filter_map(|c| {
                let score = score_command(c.name, c.aliases, partial)?;
                let alias_hint = if c.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (alias: /{})", c.aliases.join(", /"))
                };
                Some((
                    score,
                    Pair {
                        display: format!("/{} — {}{alias_hint}", c.name, c.description),
                        replacement: format!("/{}", c.name),
                    },
                ))
            })
            .collect();

        // Stable sort by score desc, then alphabetical by replacement for
        // deterministic ordering across equal-scored matches.
        scored.sort_by(|(sa, pa), (sb, pb)| {
            sb.cmp(sa).then_with(|| pa.replacement.cmp(&pb.replacement))
        });

        let matches: Vec<Pair> = scored.into_iter().map(|(_, p)| p).collect();

        // Start replacement from position 0 (replacing the whole /partial).
        Ok((0, matches))
    }
}

impl Hinter for CommandCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        if !line.starts_with('/') || pos < 2 {
            return None;
        }

        let partial = &line[1..pos];
        crate::commands::COMMANDS
            .iter()
            .filter(|c| !c.hidden)
            .find(|c| c.name.starts_with(partial) && c.name != partial)
            .map(|c| c.name[partial.len()..].to_string())
    }
}

impl Highlighter for CommandCompleter {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // Show hints in grey.
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}

impl Validator for CommandCompleter {}
impl Helper for CommandCompleter {}

/// Stream sink that writes to the terminal with full rendering.
struct TerminalSink {
    /// Tracks whether we're mid-line (for proper newline handling).
    mid_line: Arc<Mutex<bool>>,
    /// Accumulates the full response text for post-render.
    response_buffer: Arc<Mutex<String>>,
    /// Activity indicator (shown while waiting for LLM).
    indicator: Arc<Mutex<Option<ActivityIndicator>>>,
    /// Whether verbose mode is on (shows usage stats inline).
    verbose: bool,
    /// Accumulated turn state for the summary panel.
    turn_state: super::tui::SharedTurnState,
}

impl TerminalSink {
    fn new(verbose: bool) -> Self {
        Self {
            mid_line: Arc::new(Mutex::new(false)),
            response_buffer: Arc::new(Mutex::new(String::new())),
            indicator: Arc::new(Mutex::new(None)),
            verbose,
            turn_state: super::tui::new_turn_state(),
        }
    }

    /// Start the activity indicator (call when API request begins).
    fn start_indicator(&self) {
        if let Ok(mut guard) = self.indicator.lock()
            && guard.is_none()
        {
            *guard = Some(ActivityIndicator::thinking());
        }
    }

    fn ensure_newline(&self) {
        let mut mid = self.mid_line.lock().unwrap();
        if *mid {
            raw_print("\n");
            *mid = false;
        }
    }

    /// Stop the activity indicator (called when first token arrives).
    fn stop_indicator(&self) {
        if let Ok(mut guard) = self.indicator.lock()
            && let Some(ind) = guard.take()
        {
            ind.stop();
        }
    }

    /// Restart the activity indicator (called between tool execution and next LLM call).
    fn restart_indicator(&self) {
        if let Ok(mut guard) = self.indicator.lock() {
            *guard = Some(ActivityIndicator::thinking());
        }
    }
}

impl StreamSink for TerminalSink {
    fn on_text(&self, text: &str) {
        // First text token: stop the activity indicator.
        self.stop_indicator();

        raw_print(text);
        *self.mid_line.lock().unwrap() = !text.ends_with('\n');

        // Buffer for potential post-processing (markdown render of full blocks).
        self.response_buffer.lock().unwrap().push_str(text);
    }

    fn on_tool_start(&self, tool_name: &str, input: &serde_json::Value) {
        self.stop_indicator();
        self.ensure_newline();
        let detail = summarize_tool_input(tool_name, input);

        // Track in turn state.
        self.turn_state
            .lock()
            .unwrap()
            .add_tool_start(tool_name, &detail);

        // Render inline tool header.
        super::tui::render_tool_block(tool_name, &detail, None, false);
    }

    fn on_tool_result(&self, _tool_name: &str, result: &ToolResult) {
        // Track in turn state.
        self.turn_state
            .lock()
            .unwrap()
            .complete_last_tool(&result.content, result.is_error);

        // Render inline result line.
        let t = super::theme::current();
        if result.is_error {
            let first_line = result.content.lines().next().unwrap_or("");
            raw_eprint(&format!(
                "  {} {}\n",
                "✗".with(t.error),
                first_line.with(t.error)
            ));
        } else {
            let preview: String = result
                .content
                .lines()
                .next()
                .unwrap_or("(ok)")
                .chars()
                .take(80)
                .collect();
            let line_count = result.content.lines().count();
            let suffix = if line_count > 1 {
                format!(" (+{} lines)", line_count - 1)
                    .with(t.muted)
                    .to_string()
            } else {
                String::new()
            };
            raw_eprint(&format!(
                "  {} {}{}\n",
                "✓".with(t.success),
                preview.with(t.muted),
                suffix
            ));
        }
        self.restart_indicator();
    }

    fn on_thinking(&self, text: &str) {
        self.stop_indicator();
        self.turn_state.lock().unwrap().thinking_chars = text.len();
        super::tui::render_thinking_block(text);
    }

    fn on_turn_complete(&self, turn: usize) {
        self.stop_indicator();
        self.ensure_newline();

        // Render the turn summary panel if there were multiple tool calls
        // or at least one success. Skip for single-error turns (noisy).
        let state = self.turn_state.lock().unwrap();
        let has_success = state.tools.iter().any(|t| !t.is_error);
        if state.tools.len() > 1 || has_success {
            super::tui::render_turn_summary(&state, turn);
        }
        drop(state);

        // Clear turn state for next turn.
        self.turn_state.lock().unwrap().clear();
    }

    fn on_error(&self, error: &str) {
        self.stop_indicator();
        self.ensure_newline();
        let t = super::theme::current();
        raw_eprint(&format!(
            "{} {error}\n",
            super::theme::label(" ERROR ", t.error, crossterm::style::Color::White)
        ));
    }

    fn on_usage(&self, usage: &Usage) {
        // Track in turn state for the summary panel.
        {
            let mut state = self.turn_state.lock().unwrap();
            state.tokens_in = usage.input_tokens;
            state.tokens_out = usage.output_tokens;
            state.cache_read = usage.cache_read_input_tokens;
            state.cache_write = usage.cache_creation_input_tokens;
        }
    }

    fn on_compact(&self, freed_tokens: u64) {
        let t = super::theme::current();
        raw_eprint(&format!(
            "  {} {}\n",
            "↻".with(t.accent),
            format!("compacted ~{freed_tokens} tokens").with(t.muted),
        ));
    }

    fn on_warning(&self, msg: &str) {
        let t = super::theme::current();
        raw_eprint(&format!(
            "{} {msg}\n",
            super::theme::label(" WARN ", t.warning, crossterm::style::Color::Black)
        ));
    }
}

/// Spawn a background task that watches for the Escape key during streaming.
/// Returns a guard that stops the watcher on drop (restoring terminal state).
/// Uses crossterm raw mode only while actively polling, and yields quickly
/// to avoid competing with rustyline for stdin.
fn spawn_escape_watcher(engine: &QueryEngine) -> EscapeWatcherGuard {
    let cancel_token = engine.cancel_token();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();

    let handle = std::thread::spawn(move || {
        use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

        // Enable raw mode so we can capture individual keypresses.
        if crossterm::terminal::enable_raw_mode().is_err() {
            return;
        }

        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            // Poll with a short timeout to check the stop flag frequently.
            if event::poll(std::time::Duration::from_millis(100)).unwrap_or(false)
                && let Ok(Event::Key(KeyEvent {
                    code, modifiers, ..
                })) = event::read()
            {
                match code {
                    KeyCode::Esc => {
                        cancel_token.cancel();
                        break;
                    }
                    // Also handle Ctrl+C here since raw mode intercepts it.
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        cancel_token.cancel();
                        break;
                    }
                    _ => {}
                }
            }
        }

        let _ = crossterm::terminal::disable_raw_mode();
    });

    EscapeWatcherGuard {
        stop,
        handle: Some(handle),
    }
}

/// RAII guard that stops the escape watcher thread and restores terminal state.
struct EscapeWatcherGuard {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for EscapeWatcherGuard {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        // Ensure raw mode is off even if thread panicked.
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Run the interactive REPL loop.
pub async fn run_repl(engine: &mut QueryEngine) -> anyhow::Result<()> {
    // Configure editing mode and load custom keybindings.
    let input_mode = super::keymap::InputMode::default();
    let _keybindings = super::keybindings::KeybindingRegistry::load();
    let rl_config = rustyline::Config::builder()
        .edit_mode(input_mode.to_edit_mode())
        .completion_type(rustyline::config::CompletionType::List)
        .bracketed_paste(true)
        .build();
    let mut rl =
        rustyline::Editor::<CommandCompleter, rustyline::history::DefaultHistory>::with_config(
            rl_config,
        )?;
    rl.set_helper(Some(CommandCompleter));

    // Generate a session ID for persistence. Clone for later use since
    // Stylize methods consume the String.
    let session_id = agent_code_lib::services::session::new_session_id();
    let session_id_display = session_id.clone();

    // Initialize session notes and clean up old ones.
    agent_code_lib::memory::session_notes::init_session_notes(&session_id);
    agent_code_lib::memory::session_notes::cleanup_old_notes();

    // Prune old session files when operator has opted in via
    // `[session] cleanup_period_days = N`. Best-effort — a failing
    // sweep must never block REPL startup.
    if let Some(days) = engine.state().config.session.cleanup_period_days
        && days > 0
    {
        match agent_code_lib::services::session::prune_older_than(days) {
            Ok(stats) if stats.removed > 0 => {
                tracing::info!(
                    "Session cleanup: removed {} old session(s), kept {}",
                    stats.removed,
                    stats.kept
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Session cleanup skipped: {e}"),
        }
    }

    // Load project-scoped history (hashed from cwd).
    let history_path = dirs::data_dir().map(|d| {
        let cwd = &engine.state().cwd;
        // Hash the cwd to create a project-specific history file.
        let hash: u64 = cwd
            .bytes()
            .fold(5381u64, |h, b| h.wrapping_mul(33).wrapping_add(b as u64));
        d.join("agent-code")
            .join("history")
            .join(format!("{hash:x}.txt"))
    });
    if let Some(ref path) = history_path {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = rl.load_history(path);
    }

    let verbose = engine.state().config.ui.syntax_highlight; // Use as verbose proxy for now.

    // Welcome message.
    // Render the welcome banner.
    let term_width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80);
    let divider = "─".repeat(term_width.min(100));
    let model = engine.state().config.api.model.clone();
    let cwd = engine.state().cwd.clone();

    // Initialize theme.
    let theme_name = super::theme::resolve_theme(&engine.state().config.ui.theme);
    super::theme::init(&theme_name);
    let t = super::theme::current();

    println!();

    // Simple 3-line robot mascot rendered in the current theme accent color.
    // Replaces the earlier pixel-art crab — see commit history for the
    // rationale (we preferred the minimal look).
    println!(
        "  {}   {} v{}",
        "▐▛██▜▌".with(t.accent).bold(),
        "Agent Code".with(t.text).bold(),
        env!("CARGO_PKG_VERSION"),
    );
    println!(
        "  {}   {} · session {}",
        "▝▜██▛▘".with(t.accent),
        model.with(t.text).bold(),
        session_id_display.as_str().with(t.muted),
    );
    println!("  {}   {}", "  ▘▘  ".with(t.accent), cwd.with(t.muted),);

    println!();
    println!("{}", divider.with(t.muted));

    // Show hint for shortcuts.
    println!("  {}", "? for shortcuts".with(t.muted),);
    println!();

    // Render any pending startup warnings (dangerous flags, missing
    // dependencies, deprecations). No-op when the registry is empty.
    super::tui::render_warnings_banner();

    let mut ctrl_c_pending = false;

    loop {
        let sink = TerminalSink::new(verbose);
        let t = super::theme::current();

        // Inline status divider before prompt (after first turn).
        {
            let state = engine.state();
            let statusline_cfg = &state.config.ui.statusline;
            if state.turn_count > 0 && statusline_cfg.enabled {
                let term_w = crossterm::terminal::size()
                    .map(|(w, _)| w as usize)
                    .unwrap_or(80)
                    .min(100);
                let status = if let Some(template) = statusline_cfg.template.as_deref() {
                    let vars = agent_code_lib::config::StatusLineVars {
                        model: &state.config.api.model,
                        turn: state.turn_count as u64,
                        tokens: state.total_usage.total(),
                        cost_usd: state.total_cost_usd,
                        cwd: &state.cwd,
                        session_id: &state.session_id,
                    };
                    // Wrap in a single space on each side so the
                    // template sits cleanly between the divider
                    // dashes, matching the built-in layout.
                    format!(
                        " {} ",
                        agent_code_lib::config::render_statusline_template(template, &vars)
                    )
                } else {
                    format!(
                        " {} · turn {} · {} tokens · ${:.4} ",
                        state.config.api.model,
                        state.turn_count,
                        state.total_usage.total(),
                        state.total_cost_usd,
                    )
                };
                let pad = term_w.saturating_sub(status.len());
                let left = pad / 2;
                let right = pad - left;
                eprintln!(
                    "{}{}{}",
                    "─".repeat(left).with(t.muted),
                    status.with(t.muted),
                    "─".repeat(right).with(t.muted),
                );
            }
        }

        let prompt = format!("{} ", "❯".with(t.accent).bold());

        match rl.readline(&prompt) {
            Ok(line) => {
                ctrl_c_pending = false;
                let mut input_buf = line.clone();

                // Multi-line input: if line ends with \, keep reading.
                while input_buf.trim_end().ends_with('\\') {
                    input_buf.truncate(input_buf.trim_end().len() - 1);
                    input_buf.push('\n');
                    let cont_prompt = format!("{} ", ".".with(t.muted));
                    match rl.readline(&cont_prompt) {
                        Ok(next) => input_buf.push_str(&next),
                        Err(_) => break,
                    }
                }

                let input = input_buf.trim();
                if input.is_empty() {
                    continue;
                }

                // Toggle shortcuts help panel on "?".
                if input == "?" {
                    render_help_panel(engine.state());
                    continue;
                }

                rl.add_history_entry(input)?;

                // Re-echo user input with styled background, overwriting rustyline's echo.
                if !input.starts_with('/') && input != "?" && !input.starts_with('!') {
                    let t = super::theme::current();
                    let bg = if t.is_dark {
                        "\x1b[48;2;55;55;55m" // dark: subtle grey bg
                    } else {
                        "\x1b[48;2;235;235;240m" // light: subtle grey bg
                    };
                    // Move cursor up to overwrite rustyline's echo line.
                    let line_count = input.lines().count().max(1);
                    eprint!("\x1b[{line_count}A\x1b[2K");
                    let _ = std::io::stderr().flush();
                    // Print styled version.
                    for line in input.lines() {
                        let pad = crossterm::terminal::size()
                            .map(|(w, _)| w as usize)
                            .unwrap_or(80)
                            .saturating_sub(line.len() + 4);
                        println!(
                            "{bg}  {} {}{}\x1b[0m",
                            "❯".with(t.accent),
                            line,
                            " ".repeat(pad),
                        );
                    }
                    println!();
                }

                // @ file path expansion: inline file contents into the prompt.
                let input = if input.contains('@') {
                    expand_file_references(input, &engine.state().cwd)
                } else {
                    input.to_string()
                };
                let input = input.trim();

                // & prefix: run prompt in background (fire-and-forget agent turn).
                if input.starts_with('&') {
                    let bg_input = input.strip_prefix('&').unwrap_or("").trim().to_string();
                    if !bg_input.is_empty() {
                        let t = super::theme::current();
                        eprintln!(
                            "  {} {}",
                            "⟡".with(t.accent),
                            format!("background: {}", &bg_input[..bg_input.len().min(60)])
                                .with(t.muted),
                        );
                        // TODO: spawn actual background agent turn.
                        // For now, just run it as a normal turn.
                        sink.start_indicator();
                        if let Err(e) = engine.run_turn_with_sink(&bg_input, &sink).await {
                            let t = super::theme::current();
                            eprintln!(
                                "{} {e}",
                                super::theme::label(
                                    " ERROR ",
                                    t.error,
                                    crossterm::style::Color::White
                                )
                            );
                        }
                        sink.ensure_newline();
                        println!();
                    }
                    continue;
                }

                // ! prefix: run shell command directly with context injection.
                // Output streams in real-time AND gets injected into conversation
                // history so the agent can reference it in subsequent turns.
                if input.starts_with('!') {
                    let cmd = input.strip_prefix('!').unwrap_or("").trim();
                    if !cmd.is_empty() {
                        use agent_code_lib::services::shell_passthrough;

                        let cwd = std::path::Path::new(&engine.state().cwd);
                        match shell_passthrough::run_and_capture(
                            cmd,
                            cwd,
                            |line| println!("{line}"),
                            |line| eprintln!("{line}"),
                        ) {
                            Ok(output) => {
                                if let Some(msg) =
                                    shell_passthrough::build_context_message(cmd, &output)
                                {
                                    engine.state_mut().push_message(msg);
                                }
                            }
                            Err(e) => eprintln!("{e}"),
                        }
                    }
                    continue;
                }

                // Handle slash commands.
                if input.starts_with('/') {
                    match crate::commands::execute(input, engine) {
                        crate::commands::CommandResult::Handled => continue,
                        crate::commands::CommandResult::Exit => break,
                        crate::commands::CommandResult::Passthrough(text) => {
                            sink.start_indicator();
                            if let Err(e) = engine.run_turn_with_sink(&text, &sink).await {
                                {
                                    let t = super::theme::current();
                                    eprintln!(
                                        "{} {e}",
                                        super::theme::label(
                                            " ERROR ",
                                            t.error,
                                            crossterm::style::Color::White
                                        )
                                    );
                                }
                            }
                            sink.ensure_newline();
                            println!();
                        }
                        crate::commands::CommandResult::Prompt(prompt) => {
                            sink.start_indicator();
                            if let Err(e) = engine.run_turn_with_sink(&prompt, &sink).await {
                                {
                                    let t = super::theme::current();
                                    eprintln!(
                                        "{} {e}",
                                        super::theme::label(
                                            " ERROR ",
                                            t.error,
                                            crossterm::style::Color::White
                                        )
                                    );
                                }
                            }
                            sink.ensure_newline();
                            println!();
                        }
                    }
                    continue;
                }

                // Run the agent turn with Escape key watcher for cancellation.
                let _esc_guard = spawn_escape_watcher(engine);
                sink.start_indicator();
                if let Err(e) = engine.run_turn_with_sink(input, &sink).await {
                    {
                        let t = super::theme::current();
                        raw_eprint(&format!(
                            "{} {e}\n",
                            super::theme::label(" ERROR ", t.error, crossterm::style::Color::White)
                        ));
                    }
                }
                drop(_esc_guard);
                sink.ensure_newline();
                println!();

                // Auto-save session after each turn.
                {
                    let state = engine.state();
                    let _ = agent_code_lib::services::session::save_session_full(
                        &session_id,
                        &state.messages,
                        &state.cwd,
                        &state.config.api.model,
                        state.turn_count,
                        state.total_cost_usd,
                        state.total_usage.input_tokens,
                        state.total_usage.output_tokens,
                        state.plan_mode,
                    );
                }
            }
            Err(ReadlineError::Interrupted) => {
                if engine.state().is_query_active {
                    engine.cancel();
                    eprintln!("{}", "(cancelled)".with(super::theme::current().muted));
                    ctrl_c_pending = false;
                } else if ctrl_c_pending {
                    // Second Ctrl+C at prompt — exit.
                    break;
                } else {
                    // First Ctrl+C at prompt — show hint, continue.
                    eprintln!(
                        "{}",
                        "(Ctrl+C again to exit, or type /exit)".with(super::theme::current().muted)
                    );
                    ctrl_c_pending = true;
                }
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("Input error: {e}");
                break;
            }
        }
    }

    // Save history.
    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }

    // Persist session.
    let state = engine.state();
    if !state.messages.is_empty() {
        match agent_code_lib::services::session::save_session(
            &session_id,
            &state.messages,
            &state.cwd,
            &state.config.api.model,
            state.turn_count,
        ) {
            Ok(_) => {}
            Err(e) => eprintln!(
                "{}",
                format!("Failed to save session: {e}").with(super::theme::current().muted)
            ),
        }
    }

    // Print session summary.
    let divider = "─".repeat(term_width.min(100));
    let t = super::theme::current();
    println!("{}", divider.with(t.muted));
    if state.total_usage.total() > 0 {
        println!(
            "  {} {} turns | {} tokens | ${:.4} | session {}",
            "session".with(t.accent),
            state.turn_count,
            state.total_usage.total(),
            state.total_cost_usd,
            session_id_display.as_str().with(t.muted),
        );
    } else {
        println!(
            "  {} session {}",
            "goodbye".with(t.accent),
            session_id_display.as_str().with(t.muted)
        );
    }

    Ok(())
}

/// Create a short summary of tool input for display.
fn summarize_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    let raw = match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "FileRead" | "FileWrite" | "FileEdit" | "NotebookEdit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Grep" | "Glob" | "WebSearch" => input
            .get("pattern")
            .or_else(|| input.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebFetch" => input
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Agent" => input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            // Compact JSON preview.
            serde_json::to_string(input)
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect()
        }
    };

    // Truncate long summaries.
    if raw.len() > 120 {
        format!("{}...", &raw[..117])
    } else {
        raw
    }
}

/// Render the interactive help panel shown when the user types `?`.
///
/// Pulled out of the REPL loop so it can evolve without cluttering the
/// main read-dispatch body. Three sections:
///
///   * **Current state** — live session info (model, mode, tokens, cost)
///   * **Keyboard shortcuts** — key bindings and input prefixes
///   * **Commands** — all non-hidden slash commands, auto-generated from
///     `COMMANDS`, so new commands show up without touching this panel
fn render_help_panel(state: &agent_code_lib::state::AppState) {
    let t = super::theme::current();
    let divider = "─".repeat(60);

    println!();

    // ---- Current state ----
    println!("  {}", "Current session".with(t.accent).bold());
    println!("  {}", divider.as_str().with(t.muted));

    let tokens = state.total_usage.total();
    let window =
        agent_code_lib::services::tokens::context_window_for_model(&state.config.api.model);
    let ctx_pct = if window > 0 {
        (tokens as f64 / window as f64 * 100.0).round() as u64
    } else {
        0
    };
    let perm_mode = format!("{:?}", state.config.permissions.default_mode).to_lowercase();
    let mode_badges: String = {
        let mut badges: Vec<String> = Vec::new();
        if state.plan_mode {
            badges.push("plan".to_string());
        }
        if state.brief_mode {
            badges.push("brief".to_string());
        }
        if !state.additional_dirs.is_empty() {
            badges.push(format!("+{} dirs", state.additional_dirs.len()));
        }
        if badges.is_empty() {
            String::new()
        } else {
            format!("  [{}]", badges.join(", "))
        }
    };

    println!(
        "  {:<14} {}{}",
        "model".with(t.muted),
        state.config.api.model.as_str().with(t.text),
        mode_badges.with(t.accent),
    );
    println!(
        "  {:<14} {}",
        "permissions".with(t.muted),
        perm_mode.with(t.text),
    );
    println!(
        "  {:<14} {} (turn {})",
        "usage".with(t.muted),
        format!(
            "{tokens} tokens · {ctx_pct}% of {window} · ${:.4}",
            state.total_cost_usd
        )
        .with(t.text),
        state.turn_count,
    );

    // ---- Keyboard shortcuts ----
    println!();
    println!("  {}", "Keyboard & input".with(t.accent).bold());
    println!("  {}", divider.as_str().with(t.muted));
    let rows = [
        ("! command", "run shell command directly"),
        ("@ path/file", "inline file contents in the prompt"),
        ("& prompt", "run prompt in background"),
        ("/name", "slash command — Tab to complete"),
        ("\\ + Enter", "continue on next line"),
        ("Tab", "auto-complete slash commands"),
        ("Ctrl+R", "search prompt history"),
        ("Esc / Ctrl+C", "cancel (Ctrl+C twice to exit)"),
        ("Ctrl+D", "exit REPL"),
    ];
    for (key, desc) in rows {
        println!("  {:<18} {}", key.with(t.text), desc.with(t.muted));
    }

    // ---- Commands ----
    println!();
    println!(
        "  {} {}",
        "Commands".with(t.accent).bold(),
        format!(
            "({})",
            crate::commands::COMMANDS
                .iter()
                .filter(|c| !c.hidden)
                .count()
        )
        .with(t.muted),
    );
    println!("  {}", divider.as_str().with(t.muted));

    // Auto-generated, sorted alphabetically.
    let mut cmds: Vec<_> = crate::commands::COMMANDS
        .iter()
        .filter(|c| !c.hidden)
        .collect();
    cmds.sort_by_key(|c| c.name);

    // Column-align the names.
    let max_name_len = cmds.iter().map(|c| c.name.len()).max().unwrap_or(10);
    let name_width = (max_name_len + 2).min(24);

    for cmd in cmds {
        let alias_suffix = if cmd.aliases.is_empty() {
            String::new()
        } else {
            format!(" (alias: /{})", cmd.aliases.join(", /"))
        };
        println!(
            "  /{:<width$} {}{}",
            cmd.name.with(t.text),
            cmd.description.with(t.muted),
            alias_suffix.with(t.muted),
            width = name_width,
        );
    }

    println!();
}

/// Expand @path references in user input to include file contents.
/// e.g., "explain @src/main.rs" → "explain\n\nContents of src/main.rs:\n```\n...```"
fn expand_file_references(input: &str, cwd: &str) -> String {
    let mut result = String::new();
    let mut last_end = 0;

    // Find @path patterns (@ followed by non-whitespace chars containing / or .).
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@'
            && (i == 0 || chars[i - 1].is_whitespace())
            && i + 1 < chars.len()
            && !chars[i + 1].is_whitespace()
        {
            // Collect the path.
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && !chars[end].is_whitespace() {
                end += 1;
            }
            let path_str: String = chars[start..end].iter().collect();

            // Only expand if it looks like a file path (contains / or .).
            if path_str.contains('/') || path_str.contains('.') {
                let full_path = std::path::Path::new(cwd).join(&path_str);
                if full_path.exists() && full_path.is_file() {
                    // Add text before the @reference.
                    let before: String = chars[last_end..i].iter().collect();
                    result.push_str(&before);

                    // Read and inline the file.
                    match std::fs::read_to_string(&full_path) {
                        Ok(content) => {
                            let ext = full_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                            // Truncate large files.
                            let display = if content.len() > 50_000 {
                                format!(
                                    "{}...\n(truncated, {} bytes total)",
                                    &content[..50_000],
                                    content.len()
                                )
                            } else {
                                content
                            };
                            result.push_str(&format!(
                                "\n\nContents of {path_str}:\n```{ext}\n{display}\n```\n"
                            ));
                        }
                        Err(_) => {
                            result.push('@');
                            result.push_str(&path_str);
                        }
                    }
                    last_end = end;
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }

    // Append remaining text.
    let remaining: String = chars[last_end..].iter().collect();
    result.push_str(&remaining);
    result
}

#[cfg(test)]
mod raw_print_tests {
    //! The translation used by `raw_print` / `raw_eprint` is extracted here as
    //! a pure function so we can test it without touching stdout. The bug
    //! these guard against: during a streaming turn the escape watcher holds
    //! the terminal in raw mode, where a bare `\n` moves the cursor down one
    //! row without returning to column 0. Every newline emitted by the
    //! streaming sink must therefore be `\r\n`.

    fn translate(text: &str) -> String {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    }

    #[test]
    fn bare_lf_becomes_crlf() {
        assert_eq!(translate("a\nb"), "a\r\nb");
    }

    #[test]
    fn existing_crlf_is_preserved_not_doubled() {
        // Must not produce `\r\r\n`.
        assert_eq!(translate("a\r\nb"), "a\r\nb");
    }

    #[test]
    fn multiple_newlines_all_translated() {
        assert_eq!(translate("a\nb\nc\n"), "a\r\nb\r\nc\r\n");
    }

    #[test]
    fn text_without_newlines_is_unchanged() {
        assert_eq!(translate("hello world"), "hello world");
    }

    #[test]
    fn empty_string() {
        assert_eq!(translate(""), "");
    }

    #[test]
    fn mixed_lf_and_crlf() {
        assert_eq!(translate("a\nb\r\nc\n"), "a\r\nb\r\nc\r\n");
    }

    #[test]
    fn lone_cr_is_not_touched() {
        // Activity indicator and other helpers use bare `\r` to rewrite the
        // current line; that must survive translation intact.
        assert_eq!(translate("\rstatus"), "\rstatus");
    }

    #[test]
    fn cr_followed_by_text_then_lf() {
        assert_eq!(translate("\rstatus\n"), "\rstatus\r\n");
    }
}

#[cfg(test)]
mod completer_tests {
    //! Scoring for the fuzzy command completer. The completer is tab-
    //! triggered, so bad scoring means the wrong command floats to the top
    //! of the suggestion list (or worse, a correct match gets filtered out).

    use super::{fuzzy_subsequence, score_command};

    #[test]
    fn empty_partial_matches_everything_at_top_score() {
        assert_eq!(score_command("commit", &[], ""), Some(1000));
        assert_eq!(score_command("output-style", &["style"], ""), Some(1000));
    }

    #[test]
    fn prefix_beats_substring() {
        let prefix = score_command("review", &[], "rev").unwrap();
        let substring = score_command("thinkback-play", &[], "kbk").unwrap();
        assert!(
            prefix > substring,
            "prefix {prefix} should beat substring {substring}"
        );
    }

    #[test]
    fn substring_match_works() {
        // `/install-github-app` when user types `github`
        assert_eq!(
            score_command("install-github-app", &["gh-setup"], "github"),
            Some(500),
        );
    }

    #[test]
    fn alias_match_is_recognised() {
        // User types `/gh-` — the real name is `install-github-app`, the
        // alias is `gh-setup`.
        let score = score_command("install-github-app", &["gh-setup"], "gh-").unwrap();
        assert!(score >= 100, "alias match should score >= 100, got {score}");
    }

    #[test]
    fn fuzzy_subsequence_basic() {
        assert!(fuzzy_subsequence("review", "rvw"));
        assert!(fuzzy_subsequence("output-style", "os"));
        assert!(!fuzzy_subsequence("commit", "xyz"));
        assert!(!fuzzy_subsequence("abc", "abcd"));
    }

    #[test]
    fn case_insensitive_matching() {
        assert!(score_command("Review", &[], "rev").is_some());
        assert!(score_command("review", &[], "REV").is_some());
    }

    #[test]
    fn no_match_returns_none() {
        assert_eq!(score_command("commit", &[], "xyz"), None);
        assert_eq!(score_command("exit", &["quit"], "totally-different"), None);
    }

    #[test]
    fn prefix_scores_higher_than_alias_prefix() {
        // Name prefix (1000) beats alias prefix (150).
        let name_prefix = score_command("style", &[], "sty").unwrap();
        let alias_prefix = score_command("output-style", &["style"], "sty").unwrap();
        assert!(
            name_prefix > alias_prefix,
            "name prefix {name_prefix} should beat alias prefix {alias_prefix}"
        );
    }
}

#[cfg(test)]
mod at_completion_tests {
    //! Tests for the `@`-path tab completer. These are file-system tests
    //! over a tempdir so we don't depend on the repo layout.

    use super::{complete_at_path, find_at_context};

    #[test]
    fn at_context_at_start_of_line() {
        let line = "@src";
        let pos = 4;
        assert_eq!(find_at_context(line, pos), Some((0, "src")));
    }

    #[test]
    fn at_context_after_whitespace() {
        let line = "explain @src/main";
        let pos = line.len();
        assert_eq!(find_at_context(line, pos), Some((8, "src/main")));
    }

    #[test]
    fn at_context_rejected_when_preceded_by_non_whitespace() {
        // email@example.com → not a path context
        assert_eq!(find_at_context("email@example.com", 10), None);
    }

    #[test]
    fn at_context_rejected_when_partial_contains_whitespace() {
        // Once the user types past the token, we should stop completing.
        assert_eq!(find_at_context("@src/main.rs here", 17), None);
    }

    #[test]
    fn at_context_empty_partial_after_at() {
        let line = "@";
        assert_eq!(find_at_context(line, 1), Some((0, "")));
    }

    #[test]
    fn at_context_no_at_sign_returns_none() {
        assert_eq!(find_at_context("just text here", 10), None);
    }

    #[test]
    fn at_context_multiple_at_signs_uses_most_recent() {
        let line = "@src foo @tests";
        let pos = line.len();
        assert_eq!(find_at_context(line, pos), Some((9, "tests")));
    }

    #[test]
    fn complete_paths_lists_tempdir_entries() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("alpha.md"), "").unwrap();
        std::fs::write(tmp.path().join("beta.rs"), "").unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        let cwd = tmp.path().to_str().unwrap();

        let results = complete_at_path(cwd, "");
        let replacements: Vec<String> = results.iter().map(|p| p.replacement.clone()).collect();
        // Directories first, trailing slash.
        assert!(replacements.contains(&"@src/".to_string()));
        assert!(replacements.contains(&"@alpha.md".to_string()));
        assert!(replacements.contains(&"@beta.rs".to_string()));
        // dirs sort before files.
        let src_pos = replacements.iter().position(|r| r == "@src/").unwrap();
        let alpha_pos = replacements.iter().position(|r| r == "@alpha.md").unwrap();
        assert!(src_pos < alpha_pos, "dirs should list before files");
    }

    #[test]
    fn complete_paths_filters_by_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("alpha.md"), "").unwrap();
        std::fs::write(tmp.path().join("beta.rs"), "").unwrap();
        std::fs::write(tmp.path().join("alphabet.txt"), "").unwrap();
        let cwd = tmp.path().to_str().unwrap();

        let results = complete_at_path(cwd, "alp");
        let replacements: Vec<String> = results.iter().map(|p| p.replacement.clone()).collect();
        assert!(replacements.contains(&"@alpha.md".to_string()));
        assert!(replacements.contains(&"@alphabet.txt".to_string()));
        assert!(!replacements.contains(&"@beta.rs".to_string()));
    }

    #[test]
    fn complete_paths_descends_into_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), "").unwrap();
        std::fs::write(tmp.path().join("src/lib.rs"), "").unwrap();
        let cwd = tmp.path().to_str().unwrap();

        let results = complete_at_path(cwd, "src/m");
        let replacements: Vec<String> = results.iter().map(|p| p.replacement.clone()).collect();
        assert_eq!(replacements, vec!["@src/main.rs".to_string()]);
    }

    #[test]
    fn complete_paths_skips_dotfiles_unless_requested() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".hidden"), "").unwrap();
        std::fs::write(tmp.path().join("visible"), "").unwrap();
        let cwd = tmp.path().to_str().unwrap();

        let default = complete_at_path(cwd, "");
        let replacements: Vec<String> = default.iter().map(|p| p.replacement.clone()).collect();
        assert!(replacements.contains(&"@visible".to_string()));
        assert!(!replacements.contains(&"@.hidden".to_string()));

        let with_dot = complete_at_path(cwd, ".");
        let replacements: Vec<String> = with_dot.iter().map(|p| p.replacement.clone()).collect();
        assert!(replacements.contains(&"@.hidden".to_string()));
    }

    #[test]
    fn complete_paths_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        assert!(complete_at_path(cwd, "../foo").is_empty());
        assert!(complete_at_path(cwd, "/etc/passwd").is_empty());
    }

    #[test]
    fn complete_paths_empty_for_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        assert!(complete_at_path(cwd, "does-not-exist/foo").is_empty());
    }
}
