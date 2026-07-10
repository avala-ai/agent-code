//! Live event loop for the modern TUI.
//!
//! Owns the terminal (alt-screen + raw mode), drives [`App`], and runs
//! turns through [`Session::spawn_turn`] so drawing never blocks on the
//! engine lock.

use std::io::{Stdout, stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use agent_code_lib::query::{QueryEngine, Session, TurnHandle};

use super::app::App;
use super::render;
use super::sink::{ChannelSink, EngineEvent};

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Run the modern full-screen TUI until the user quits.
pub async fn run_modern_tui(engine: QueryEngine) -> anyhow::Result<()> {
    let model = engine.state().config.api.model.clone();
    let cwd = engine.state().cwd.clone();
    let session_id = engine.state().session_id.clone();

    // Apply theme so any shared color helpers still resolve.
    let configured = engine.state().config.ui.theme.clone();
    let inherit_fg = engine.state().config.ui.inherit_fg;
    let theme_name = crate::ui::theme::resolve_theme(&configured);
    crate::ui::theme::init_with_options(&theme_name, &configured, inherit_fg);

    // Session notes / tips (lightweight — same as classic startup).
    agent_code_lib::memory::session_notes::init_session_notes(&session_id);

    // SessionStart already fired in main before we get here.

    let session = Session::new(engine);
    let mut app = App::new(model, cwd, session_id);

    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, &session, &mut app).await;
    restore_terminal(&mut terminal)?;

    // SessionStop on clean exit (engine is behind the Session mutex).
    {
        let engine_arc = session.engine();
        let eng = engine_arc.lock().await;
        let _ = eng.fire_session_stop_hooks().await;
    }

    result
}

fn setup_terminal() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Term) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

async fn event_loop(terminal: &mut Term, session: &Session, app: &mut App) -> anyhow::Result<()> {
    let (eng_tx, mut eng_rx) = mpsc::unbounded_channel::<EngineEvent>();
    let mut turn: Option<TurnHandle> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Sync plan_mode with SessionMode on the engine when it changes.
    let mut last_mode = app.mode;

    loop {
        // Apply session mode → plan_mode on the engine (best-effort).
        if app.mode != last_mode {
            last_mode = app.mode;
            apply_mode_to_engine(session, app.mode).await;
        }

        // Start a pending turn if idle.
        if turn.is_none()
            && let Some(prompt) = app.pending_submit.take()
        {
            let sink = ChannelSink::new(eng_tx.clone());
            let handle = session.spawn_turn(prompt, sink).await;
            turn = Some(handle);
            app.phase = super::app::Phase::Streaming;
        }

        // Cancel if requested.
        if app.cancel_requested {
            if let Some(ref h) = turn {
                h.cancel();
            }
            app.cancel_requested = false;
        }

        // Drain engine events without blocking.
        while let Ok(ev) = eng_rx.try_recv() {
            app.apply_engine(ev);
        }

        // Reap finished turn.
        if let Some(ref h) = turn {
            use agent_code_lib::query::TurnStatus;
            if matches!(
                h.status(),
                TurnStatus::Completed | TurnStatus::Aborted | TurnStatus::Errored(_)
            ) {
                // Drain remaining events
                while let Ok(ev) = eng_rx.try_recv() {
                    app.apply_engine(ev);
                }
                if let Some(handle) = turn.take()
                    && let Err(e) = handle.join().await
                {
                    app.apply_engine(EngineEvent::Error(e.to_string()));
                }
                // Refresh cost/tokens from engine state.
                {
                    let engine_arc = session.engine();
                    if let Ok(eng) = engine_arc.try_lock() {
                        app.cost_usd = eng.state().total_cost_usd;
                        app.tokens_in = eng.state().total_usage.input_tokens;
                        app.tokens_out = eng.state().total_usage.output_tokens;
                        app.turn_count = eng.state().turn_count;
                        app.model = eng.state().config.api.model.clone();
                    }
                }
                app.mark_turn_idle();
            }
        }

        terminal.draw(|f| render::draw(f, app))?;

        if app.should_quit {
            break;
        }

        tokio::select! {
            _ = tick.tick() => {
                if app.phase == super::app::Phase::Streaming {
                    app.tick();
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                // Poll crossterm without blocking the runtime for long.
                while event::poll(Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()? {
                        handle_key(app, key);
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Cancel any in-flight turn on exit.
    if let Some(h) = turn.take() {
        h.cancel();
        let _ = h.join().await;
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // Ignore key release / repeat on platforms that emit them.
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return;
    }

    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if app.phase == super::app::Phase::Streaming {
                app.request_cancel();
            } else {
                app.should_quit = true;
            }
        }
        (KeyModifiers::CONTROL, KeyCode::Char('d')) if app.input.is_empty() => {
            app.should_quit = true;
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) | (KeyModifiers::SHIFT, KeyCode::Tab) => {
            app.cycle_mode_forward();
        }
        (_, KeyCode::Esc) => {
            app.request_cancel();
        }
        (_, KeyCode::Enter) => {
            app.submit();
        }
        (_, KeyCode::Backspace) => app.backspace(),
        (_, KeyCode::Left) => app.move_left(),
        (_, KeyCode::Right) => app.move_right(),
        (_, KeyCode::Up) => {
            app.scroll_offset = app.scroll_offset.saturating_add(1);
        }
        (_, KeyCode::Down) => {
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
        }
        (_, KeyCode::PageUp) => {
            app.scroll_offset = app.scroll_offset.saturating_add(10);
        }
        (_, KeyCode::PageDown) => {
            app.scroll_offset = app.scroll_offset.saturating_sub(10);
        }
        (_, KeyCode::Char(c)) => {
            app.insert_char(c);
        }
        _ => {}
    }
}

async fn apply_mode_to_engine(session: &Session, mode: super::mode::SessionMode) {
    let engine_arc = session.engine();
    let Ok(mut eng) = engine_arc.try_lock() else {
        return;
    };
    eng.state_mut().plan_mode = matches!(mode, super::mode::SessionMode::Plan);
    // AcceptEdits / AlwaysApprove: full permission overlay wiring lands in a
    // follow-up; plan mode is the load-bearing safety switch for v1.
}
