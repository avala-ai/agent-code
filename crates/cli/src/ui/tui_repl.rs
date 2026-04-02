//! Full-screen TUI REPL using ratatui alternate screen.
//!
//! Drops rustyline entirely. Uses crossterm raw mode for input
//! and ratatui Terminal::draw() for rendering. The footer (status bar
//! + input line) is pinned at the bottom via Layout constraints.
//!
//! During agent turns, we exit alternate screen and raw mode so
//! streaming output prints normally. Re-enter after the turn.

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use agent_code_lib::llm::message::Usage;
use agent_code_lib::query::{QueryEngine, StreamSink};
use agent_code_lib::tools::ToolResult;

// ---- Public entry point ----

/// Run the full-screen TUI REPL.
pub async fn run_tui_repl(engine: &mut QueryEngine) -> anyhow::Result<()> {
    let session_id = agent_code_lib::services::session::new_session_id();
    agent_code_lib::memory::session_notes::init_session_notes(&session_id);
    agent_code_lib::memory::session_notes::cleanup_old_notes();

    // Initialize theme.
    let theme_name = super::theme::resolve_theme(&engine.state().config.ui.theme);
    super::theme::init(&theme_name);

    // Install panic hook to restore terminal on crash.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
        );
        default_panic(info);
    }));

    // Set up ratatui terminal.
    terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        terminal::EnterAlternateScreen,
        crossterm::cursor::Hide,
    )?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut term = Terminal::new(backend)?;

    let mut app = App::new(engine, &session_id);
    app.render(&mut term)?;

    // Main loop.
    loop {
        if event::poll(std::time::Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            match app.on_key(key) {
                Action::None => {}
                Action::Submit => {
                    let input = app.take_input();
                    if !input.is_empty() {
                        // Leave alternate screen for agent turn.
                        crossterm::execute!(
                            io::stdout(),
                            terminal::LeaveAlternateScreen,
                            crossterm::cursor::Show,
                        )?;
                        terminal::disable_raw_mode()?;

                        // Process input in normal terminal.
                        let should_exit = app.run_input(&input, engine, &session_id).await;

                        if should_exit {
                            // Don't re-enter alt screen — just break.
                            break;
                        }

                        // Re-enter alternate screen.
                        terminal::enable_raw_mode()?;
                        crossterm::execute!(
                            io::stdout(),
                            terminal::EnterAlternateScreen,
                            crossterm::cursor::Hide,
                        )?;
                        // Ratatui needs a fresh terminal after re-entering.
                        term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
                        app.sync_stats(engine);
                    }
                }
                Action::Exit => break,
            }
        }
        app.render(&mut term)?;
    }

    // Cleanup.
    crossterm::execute!(
        io::stdout(),
        terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    )?;
    terminal::disable_raw_mode()?;

    // Save session + print summary.
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
    if state.turn_count > 0 {
        println!(
            "Session {} | {} turns | {} tokens | ${:.4}",
            session_id,
            state.turn_count,
            state.total_usage.total(),
            state.total_cost_usd,
        );
    }

    Ok(())
}

// ---- App state ----

enum Action {
    None,
    Submit,
    Exit,
}

struct App {
    input: String,
    cursor: usize,
    history: Vec<String>,
    hist_idx: usize,
    saved_input: String,
    content: Vec<String>,
    scroll: u16,
    model: String,
    turns: usize,
    tokens: u64,
    cost: f64,
    ctrl_c: bool,
}

impl App {
    fn new(engine: &QueryEngine, session_id: &str) -> Self {
        let state = engine.state();
        let content = vec![
            String::new(),
            format!("  Agent Code v{}", env!("CARGO_PKG_VERSION")),
            format!("  {} · session {}", state.config.api.model, session_id),
            format!("  {}", state.cwd),
            String::new(),
        ];
        Self {
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            hist_idx: 0,
            saved_input: String::new(),
            content,
            scroll: 0,
            model: state.config.api.model.clone(),
            turns: 0,
            tokens: 0,
            cost: 0.0,
            ctrl_c: false,
        }
    }

    fn sync_stats(&mut self, engine: &QueryEngine) {
        let state = engine.state();
        self.model = state.config.api.model.clone();
        self.turns = state.turn_count;
        self.tokens = state.total_usage.total();
        self.cost = state.total_cost_usd;
    }

    fn take_input(&mut self) -> String {
        let s = self.input.clone();
        self.history.push(s.clone());
        self.hist_idx = self.history.len();
        self.input.clear();
        self.cursor = 0;
        s
    }

    /// Convert character index to byte index in self.input.
    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    fn on_key(&mut self, key: crossterm::event::KeyEvent) -> Action {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if self.ctrl_c {
                    return Action::Exit;
                }
                self.ctrl_c = true;
                self.content.push("  (Ctrl+C again to exit)".into());
                Action::None
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) if self.input.is_empty() => Action::Exit,
            (KeyCode::Enter, _) if !self.input.trim().is_empty() => {
                self.ctrl_c = false;
                Action::Submit
            }
            (KeyCode::Backspace, _) => {
                self.ctrl_c = false;
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let byte_idx = self.char_to_byte(self.cursor);
                    self.input.remove(byte_idx);
                }
                Action::None
            }
            (KeyCode::Delete, _) => {
                if self.cursor < self.input.chars().count() {
                    let byte_idx = self.char_to_byte(self.cursor);
                    self.input.remove(byte_idx);
                }
                Action::None
            }
            (KeyCode::Left, _) => {
                self.cursor = self.cursor.saturating_sub(1);
                Action::None
            }
            (KeyCode::Right, _) => {
                if self.cursor < self.input.chars().count() {
                    self.cursor += 1;
                }
                Action::None
            }
            (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.cursor = 0;
                Action::None
            }
            (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                self.cursor = self.input.chars().count();
                Action::None
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.input.clear();
                self.cursor = 0;
                Action::None
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                let byte_idx = self.char_to_byte(self.cursor);
                self.input.truncate(byte_idx);
                Action::None
            }
            (KeyCode::Up, _) => {
                if !self.history.is_empty() && self.hist_idx > 0 {
                    if self.hist_idx == self.history.len() {
                        self.saved_input = self.input.clone();
                    }
                    self.hist_idx -= 1;
                    self.input = self.history[self.hist_idx].clone();
                    self.cursor = self.input.chars().count();
                }
                Action::None
            }
            (KeyCode::Down, _) => {
                if self.hist_idx < self.history.len() {
                    self.hist_idx += 1;
                    self.input = if self.hist_idx == self.history.len() {
                        self.saved_input.clone()
                    } else {
                        self.history[self.hist_idx].clone()
                    };
                    self.cursor = self.input.chars().count();
                }
                Action::None
            }
            (KeyCode::PageUp, _) => {
                self.scroll = self.scroll.saturating_add(10);
                Action::None
            }
            (KeyCode::PageDown, _) => {
                self.scroll = self.scroll.saturating_sub(10);
                Action::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.ctrl_c = false;
                let byte_idx = self.char_to_byte(self.cursor);
                self.input.insert(byte_idx, c);
                self.cursor += 1;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Process input in normal terminal mode (alt screen OFF).
    /// Returns true if exit was requested.
    async fn run_input(&mut self, input: &str, engine: &mut QueryEngine, session_id: &str) -> bool {
        let input = input.trim();

        // Echo user input.
        println!(
            "\x1b[48;2;55;55;55m  \x1b[1;38;2;164;34;225m❯\x1b[0;48;2;55;55;55m {input}\x1b[0m"
        );
        println!();

        // ? shortcuts.
        if input == "?" {
            println!("  Keyboard Shortcuts");
            println!("  ────────────────────────────────");
            println!("  ! command    run shell command");
            println!("  @ path       attach file to prompt");
            println!("  / command    slash commands");
            println!("  Ctrl+C       cancel (twice to exit)");
            println!("  PgUp/PgDn    scroll content");
            println!();
            self.content.push("  (shortcuts shown)".into());
            return false;
        }

        // ! bash.
        if let Some(cmd) = input.strip_prefix('!') {
            let cmd = cmd.trim();
            if !cmd.is_empty() {
                let output = std::process::Command::new("bash")
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(&engine.state().cwd)
                    .output();
                match output {
                    Ok(out) => {
                        let s = String::from_utf8_lossy(&out.stdout);
                        let e = String::from_utf8_lossy(&out.stderr);
                        if !s.is_empty() {
                            print!("{s}");
                        }
                        if !e.is_empty() {
                            eprint!("{e}");
                        }
                    }
                    Err(e) => eprintln!("bash error: {e}"),
                }
            }
            self.content
                .push(format!("  ! {}", input.strip_prefix('!').unwrap_or("")));
            return false;
        }

        // / commands.
        if input.starts_with('/') {
            match crate::commands::execute(input, engine) {
                crate::commands::CommandResult::Handled => {
                    self.content.push(format!("  {}", input));
                }
                crate::commands::CommandResult::Exit => {
                    return true;
                }
                crate::commands::CommandResult::Passthrough(text)
                | crate::commands::CommandResult::Prompt(text) => {
                    self.run_turn(&text, engine, session_id).await;
                }
            }
            return false;
        }

        // @ file expansion.
        let expanded = if input.contains('@') {
            super::repl::expand_file_references(input, &engine.state().cwd)
        } else {
            input.to_string()
        };

        self.run_turn(&expanded, engine, session_id).await;
        false
    }

    async fn run_turn(&mut self, input: &str, engine: &mut QueryEngine, session_id: &str) {
        let sink = PrintSink {
            cleared: std::sync::atomic::AtomicBool::new(false),
        };
        print!("\x1b[90mworking...\x1b[0m");
        let _ = io::Write::flush(&mut io::stdout());

        if let Err(e) = engine.run_turn_with_sink(input, &sink).await {
            println!("\r\x1b[2K\x1b[31m  ERROR\x1b[0m {e}");
        } else {
            print!("\r\x1b[2K"); // Clear "working..."
        }
        let _ = io::Write::flush(&mut io::stdout());
        println!();

        // Record in content for alt-screen history.
        self.content.push(format!(
            "  ❯ {}",
            input.chars().take(60).collect::<String>()
        ));

        // Auto-save.
        let state = engine.state();
        let _ = agent_code_lib::services::session::save_session_full(
            session_id,
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

    /// Render the full screen using ratatui.
    fn render(&self, term: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        term.draw(|f| {
            let area = f.area();

            // 3 zones: content (fills), status (1 row), input (1 row).
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(area);

            // Content area.
            let content_h = chunks[0].height as usize;
            let total = self.content.len();
            let max_scroll = total.saturating_sub(content_h);
            let scroll = (self.scroll as usize).min(max_scroll);
            let start = total.saturating_sub(content_h + scroll);
            let end = (start + content_h).min(total);

            let lines: Vec<Line> = self.content[start..end]
                .iter()
                .map(|s| Line::raw(s.as_str()))
                .collect();

            f.render_widget(Paragraph::new(lines), chunks[0]);

            // Status bar.
            let status = if self.turns > 0 {
                format!(
                    " {} · turn {} · {} tokens · ${:.4} · ? for shortcuts",
                    self.model, self.turns, self.tokens, self.cost
                )
            } else {
                format!(" {} · ? for shortcuts", self.model)
            };
            let w = chunks[1].width as usize;
            let pad = w.saturating_sub(status.len());
            let status_text = format!("{status}{}", "─".repeat(pad));
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    status_text,
                    Style::default().fg(Color::DarkGray),
                ))),
                chunks[1],
            );

            // Input line.
            let prompt_style = Style::default()
                .fg(Color::Rgb(164, 34, 225))
                .add_modifier(Modifier::BOLD);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("❯ ", prompt_style),
                    Span::raw(&self.input),
                ])),
                chunks[2],
            );

            // Cursor.
            f.set_cursor_position((chunks[2].x + 2 + self.cursor as u16, chunks[2].y));
        })?;
        Ok(())
    }
}

// ---- Simple print sink (used during normal-screen agent turns) ----

struct PrintSink {
    cleared: std::sync::atomic::AtomicBool,
}

impl StreamSink for PrintSink {
    fn on_text(&self, text: &str) {
        if !self
            .cleared
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            print!("\r\x1b[2K"); // Clear "working..." only once.
        }
        print!("{text}");
        let _ = io::Write::flush(&mut io::stdout());
    }
    fn on_tool_start(&self, name: &str, input: &serde_json::Value) {
        if !self
            .cleared
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            print!("\r\x1b[2K");
        }
        let detail = super::repl::summarize_tool_input(name, input);
        println!("  \x1b[36m{name}\x1b[0m  \x1b[90m{detail}\x1b[0m");
    }
    fn on_tool_result(&self, _name: &str, result: &ToolResult) {
        let (icon, color) = if result.is_error {
            ("✗", "31")
        } else {
            ("✓", "32")
        };
        let preview: String = result
            .content
            .lines()
            .next()
            .unwrap_or("(ok)")
            .chars()
            .take(80)
            .collect();
        println!("  \x1b[{color}m{icon}\x1b[0m \x1b[90m{preview}\x1b[0m");
    }
    fn on_error(&self, error: &str) {
        println!("  \x1b[31mERROR\x1b[0m {error}");
    }
    fn on_thinking(&self, _: &str) {}
    fn on_turn_complete(&self, _: usize) {}
    fn on_usage(&self, _: &Usage) {}
    fn on_compact(&self, freed: u64) {
        println!("  \x1b[90m↻ compacted ~{freed} tokens\x1b[0m");
    }
    fn on_warning(&self, msg: &str) {
        println!("  \x1b[33mWARN\x1b[0m {msg}");
    }
}
