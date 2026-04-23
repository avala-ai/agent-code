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

impl Completer for CommandCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Only complete at the start of input for / commands.
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
            if state.turn_count > 0 {
                let term_w = crossterm::terminal::size()
                    .map(|(w, _)| w as usize)
                    .unwrap_or(80)
                    .min(100);
                let status = format!(
                    " {} · turn {} · {} tokens · ${:.4} ",
                    state.config.api.model,
                    state.turn_count,
                    state.total_usage.total(),
                    state.total_cost_usd,
                );
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
                    let t = super::theme::current();
                    println!();
                    println!("  {}", "Keyboard Shortcuts".with(t.accent).bold());
                    println!("  {}", "─".repeat(50).with(t.muted));
                    println!(
                        "  {}  {}",
                        "! command".with(t.text),
                        "run shell command directly".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "@ path/file".with(t.text),
                        "attach file contents to prompt".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "& prompt".with(t.text),
                        "run prompt in background".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "/ + command".with(t.text),
                        "slash commands (/help to list all)".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "Tab".with(t.text),
                        "auto-complete /commands".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "\\ + Enter".with(t.text),
                        "multi-line input".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "Esc / Ctrl+C".with(t.text),
                        "cancel (Ctrl+C twice to exit)".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "Ctrl+R".with(t.text),
                        "search history".with(t.muted),
                    );
                    println!("  {}  {}", "Ctrl+D".with(t.text), "exit".with(t.muted),);
                    println!("  {}", "─".repeat(50).with(t.muted));
                    println!(
                        "  {}  {}",
                        "/model <name>".with(t.text),
                        "switch model".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "/scroll".with(t.text),
                        "scrollable conversation view".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "/snip <N-M>".with(t.text),
                        "remove messages from history".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "/fork".with(t.text),
                        "branch conversation".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "/rewind".with(t.text),
                        "undo last turn".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "/features".with(t.text),
                        "show feature flags".with(t.muted),
                    );
                    println!(
                        "  {}  {}",
                        "/doctor".with(t.text),
                        "check environment health".with(t.muted),
                    );
                    println!();
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
