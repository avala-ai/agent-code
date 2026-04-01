//! Interactive REPL (Read-Eval-Print Loop).
//!
//! The main user interaction loop. Reads input via rustyline,
//! passes it to the query engine, and streams output to the terminal.
//! Integrates markdown rendering, activity indicators, and permission
//! prompts.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crossterm::style::Stylize;
use rustyline::error::ReadlineError;

use crate::llm::message::Usage;
use crate::query::{QueryEngine, StreamSink};
use crate::tools::ToolResult;
use crate::ui::activity::ActivityIndicator;

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
            indicator: Arc::new(Mutex::new(Some(ActivityIndicator::thinking()))),
            verbose,
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
        let label = format!(" {tool_name} ");
        let detail = summarize_tool_input(tool_name, input);
        eprintln!(
            "{} {}",
            label.on_dark_cyan().white().bold(),
            detail.dark_grey()
        );
    }

    fn on_tool_result(&self, tool_name: &str, result: &ToolResult) {
        if result.is_error {
            let label = format!(" {tool_name} ERROR ");
            let first_line = result.content.lines().next().unwrap_or("");
            eprintln!("{} {}", label.on_red().white().bold(), first_line.red());
        }
        // Restart indicator — LLM will be called again with tool results.
        self.restart_indicator();
    }

    fn on_thinking(&self, text: &str) {
        self.stop_indicator();
        // Show a brief thinking indicator, not the full content.
        if text.len() > 80 {
            eprint!(
                "\r{}\r",
                format!("  thinking ({} chars)...", text.len()).dark_grey()
            );
        }
    }

    fn on_turn_complete(&self, turn: usize) {
        self.stop_indicator();
        self.ensure_newline();
        if self.verbose {
            eprintln!("{}", format!("  (turn {turn} complete)").dark_grey());
        }
    }

    fn on_error(&self, error: &str) {
        self.stop_indicator();
        self.ensure_newline();
        eprintln!("{} {error}", " ERROR ".on_red().white().bold());
    }

    fn on_usage(&self, usage: &Usage) {
        if self.verbose && usage.total() > 0 {
            let cache_info = if usage.cache_read_input_tokens > 0 {
                format!(
                    ", cache: {}r/{}w",
                    usage.cache_read_input_tokens, usage.cache_creation_input_tokens
                )
            } else {
                String::new()
            };
            eprintln!(
                "{}",
                format!(
                    "  tokens: {}in + {}out{cache_info}",
                    usage.input_tokens, usage.output_tokens
                )
                .dark_grey()
            );
        }
    }

    fn on_compact(&self, freed_tokens: u64) {
        eprintln!(
            "{}",
            format!("  compacted ~{freed_tokens} tokens").dark_grey()
        );
    }

    fn on_warning(&self, msg: &str) {
        eprintln!("{} {msg}", " WARN ".on_yellow().black().bold());
    }
}

/// Run the interactive REPL loop.
pub async fn run_repl(engine: &mut QueryEngine) -> anyhow::Result<()> {
    // Configure editing mode and load custom keybindings.
    let input_mode = super::keymap::InputMode::default();
    let _keybindings = super::keybindings::KeybindingRegistry::load();
    let rl_config = rustyline::Config::builder()
        .edit_mode(input_mode.to_edit_mode())
        .build();
    let mut rl =
        rustyline::Editor::<(), rustyline::history::DefaultHistory>::with_config(rl_config)?;

    // Generate a session ID for persistence. Clone for later use since
    // Stylize methods consume the String.
    let session_id = crate::services::session::new_session_id();
    let session_id_display = session_id.clone();

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

    println!();
    println!(
        "  {}   agent-code v{}",
        "▐▛██▜▌".dark_cyan().bold(),
        env!("CARGO_PKG_VERSION"),
    );
    println!(
        "  {}  {} · session {}",
        "▝▜██▛▘".dark_cyan(),
        model.white().bold(),
        session_id_display.as_str().dark_grey(),
    );
    println!("  {}   {}", "  ▘▘  ".dark_cyan(), cwd.dark_grey(),);
    println!();
    println!("{}", divider.dark_grey());

    loop {
        let sink = TerminalSink::new(verbose);
        let prompt = format!("{} ", "❯".dark_cyan().bold());

        match rl.readline(&prompt) {
            Ok(line) => {
                let mut input_buf = line.clone();

                // Multi-line input: if line ends with \, keep reading.
                while input_buf.trim_end().ends_with('\\') {
                    input_buf.truncate(input_buf.trim_end().len() - 1);
                    input_buf.push('\n');
                    let cont_prompt = format!("{} ", ".".dark_grey());
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
                            if let Err(e) = engine.run_turn_with_sink(&text, &sink).await {
                                eprintln!("{} {e}", " ERROR ".on_red().white().bold());
                            }
                            sink.ensure_newline();
                            println!();
                        }
                        crate::commands::CommandResult::Prompt(prompt) => {
                            if let Err(e) = engine.run_turn_with_sink(&prompt, &sink).await {
                                eprintln!("{} {e}", " ERROR ".on_red().white().bold());
                            }
                            sink.ensure_newline();
                            println!();
                        }
                    }
                    continue;
                }

                // Run the agent turn.
                if let Err(e) = engine.run_turn_with_sink(input, &sink).await {
                    eprintln!("{} {e}", " ERROR ".on_red().white().bold());
                }
                sink.ensure_newline();
                println!();
            }
            Err(ReadlineError::Interrupted) => {
                if engine.state().is_query_active {
                    engine.cancel();
                    eprintln!("{}", "(cancelled)".dark_grey());
                } else {
                    // Not in a turn — Ctrl+C at the prompt means exit.
                    eprintln!("{}", "(Ctrl+C again to exit, or type /exit)".dark_grey());
                    // Wait for next input — if it's another Ctrl+C, break.
                    match rl.readline(&format!("{} ", "❯".dark_cyan().bold())) {
                        Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
                        Ok(line) => {
                            let input = line.trim();
                            if !input.is_empty() {
                                rl.add_history_entry(input).ok();
                                // Process it normally on next loop iteration
                                // by pushing it back... actually just continue.
                            }
                            continue;
                        }
                        Err(_) => break,
                    }
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
            Err(e) => eprintln!("{}", format!("Failed to save session: {e}").dark_grey()),
        }
    }

    // Print session summary.
    let divider = "─".repeat(term_width.min(100));
    println!("{}", divider.dark_grey());
    if state.total_usage.total() > 0 {
        println!(
            "  {} {} turns | {} tokens | ${:.4} | session {}",
            "session".dark_cyan(),
            state.turn_count,
            state.total_usage.total(),
            state.total_cost_usd,
            session_id_display.as_str().dark_grey(),
        );
    } else {
        println!(
            "  {} session {}",
            "goodbye".dark_cyan(),
            session_id_display.as_str().dark_grey()
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
