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

use crate::llm::message::Usage;
use crate::query::{QueryEngine, StreamSink};
use crate::tools::ToolResult;
use crate::ui::activity::ActivityIndicator;

/// Tab-completion helper for slash commands.
struct CommandCompleter;

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
        let matches: Vec<Pair> = crate::commands::COMMANDS
            .iter()
            .filter(|c| !c.hidden)
            .filter(|c| c.name.starts_with(partial))
            .map(|c| Pair {
                display: format!("/{} — {}", c.name, c.description),
                replacement: format!("/{}", c.name),
            })
            .collect();

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
}

impl TerminalSink {
    fn new(verbose: bool) -> Self {
        Self {
            mid_line: Arc::new(Mutex::new(false)),
            response_buffer: Arc::new(Mutex::new(String::new())),
            // Don't start indicator here — it would overwrite the input prompt.
            // It gets started when the API call begins (via start_indicator).
            indicator: Arc::new(Mutex::new(None)),
            verbose,
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
            println!();
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

        print!("{text}");
        let _ = std::io::stdout().flush();
        *self.mid_line.lock().unwrap() = !text.ends_with('\n');

        // Buffer for potential post-processing (markdown render of full blocks).
        self.response_buffer.lock().unwrap().push_str(text);
    }

    fn on_tool_start(&self, tool_name: &str, input: &serde_json::Value) {
        self.stop_indicator();
        self.ensure_newline();
        let detail = summarize_tool_input(tool_name, input);
        super::tui::render_tool_block(tool_name, &detail, None, false);
    }

    fn on_tool_result(&self, _tool_name: &str, result: &ToolResult) {
        // Re-render the tool block with result (overwrites the start block).
        // For now, just append the result line.
        let t = super::theme::current();
        if result.is_error {
            let first_line = result.content.lines().next().unwrap_or("");
            eprintln!("  {} {}", "✗".with(t.error), first_line.with(t.error),);
        } else {
            let preview = result
                .content
                .lines()
                .next()
                .unwrap_or("(ok)")
                .chars()
                .take(80)
                .collect::<String>();
            let line_count = result.content.lines().count();
            let suffix = if line_count > 1 {
                format!(" (+{} lines)", line_count - 1)
                    .with(t.muted)
                    .to_string()
            } else {
                String::new()
            };
            eprintln!(
                "  {} {}{}",
                "✓".with(t.success),
                preview.with(t.muted),
                suffix,
            );
        }
        // Restart indicator — LLM will be called again with tool results.
        self.restart_indicator();
    }

    fn on_thinking(&self, text: &str) {
        self.stop_indicator();
        super::tui::render_thinking_block(text);
    }

    fn on_turn_complete(&self, turn: usize) {
        self.stop_indicator();
        self.ensure_newline();
        if self.verbose {
            let t = super::theme::current();
            eprintln!(
                "  {} {}",
                "·".with(t.muted),
                format!("turn {turn} complete").with(t.muted),
            );
        }
    }

    fn on_error(&self, error: &str) {
        self.stop_indicator();
        self.ensure_newline();
        let t = super::theme::current();
        eprintln!(
            "{} {error}",
            super::theme::label(" ERROR ", t.error, crossterm::style::Color::White)
        );
    }

    fn on_usage(&self, usage: &Usage) {
        if self.verbose && usage.total() > 0 {
            let t = super::theme::current();
            let mut parts = vec![format!("{}in", usage.input_tokens)];
            parts.push(format!("{}out", usage.output_tokens));
            if usage.cache_read_input_tokens > 0 {
                parts.push(format!("{}cache↓", usage.cache_read_input_tokens));
            }
            if usage.cache_creation_input_tokens > 0 {
                parts.push(format!("{}cache↑", usage.cache_creation_input_tokens));
            }
            eprintln!(
                "  {} {}",
                "⟡".with(t.muted),
                parts.join(" · ").with(t.muted),
            );
        }
    }

    fn on_compact(&self, freed_tokens: u64) {
        let t = super::theme::current();
        eprintln!(
            "  {} {}",
            "↻".with(t.accent),
            format!("compacted ~{freed_tokens} tokens").with(t.muted),
        );
    }

    fn on_warning(&self, msg: &str) {
        let t = super::theme::current();
        eprintln!(
            "{} {msg}",
            super::theme::label(" WARN ", t.warning, crossterm::style::Color::Black)
        );
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
        .build();
    let mut rl =
        rustyline::Editor::<CommandCompleter, rustyline::history::DefaultHistory>::with_config(
            rl_config,
        )?;
    rl.set_helper(Some(CommandCompleter));

    // Generate a session ID for persistence. Clone for later use since
    // Stylize methods consume the String.
    let session_id = crate::services::session::new_session_id();
    let session_id_display = session_id.clone();

    // Initialize session notes and clean up old ones.
    crate::memory::session_notes::init_session_notes(&session_id);
    crate::memory::session_notes::cleanup_old_notes();

    // Load history.
    let history_path = dirs::data_dir().map(|d| d.join("agent-code").join("history.txt"));
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
    println!(
        "  {}   agent-code v{}",
        "▐▛██▜▌".with(t.accent).bold(),
        env!("CARGO_PKG_VERSION"),
    );
    println!(
        "  {}  {} · session {}",
        "▝▜██▛▘".with(t.accent),
        model.with(t.text).bold(),
        session_id_display.as_str().with(t.muted),
    );
    println!("  {}   {}", "  ▘▘  ".with(t.accent), cwd.with(t.muted),);
    println!();
    println!("{}", divider.with(t.muted));

    let mut ctrl_c_pending = false;

    loop {
        let sink = TerminalSink::new(verbose);
        let t = super::theme::current();
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

                rl.add_history_entry(input)?;

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

                // Run the agent turn. Start the indicator now (after input is captured).
                sink.start_indicator();
                if let Err(e) = engine.run_turn_with_sink(input, &sink).await {
                    {
                        let t = super::theme::current();
                        eprintln!(
                            "{} {e}",
                            super::theme::label(" ERROR ", t.error, crossterm::style::Color::White)
                        );
                    }
                }
                sink.ensure_newline();
                println!();
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
        match crate::services::session::save_session(
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
