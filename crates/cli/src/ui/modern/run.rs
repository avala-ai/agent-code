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

use agent_code_lib::config::PermissionMode;
use agent_code_lib::query::{QueryEngine, Session, TurnHandle};
use agent_code_lib::tools::PermissionResponse;

use super::app::App;
use super::render;
use super::sink::{ChannelSink, EngineEvent, ModernPrompter};

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Run the modern full-screen TUI until the user quits.
pub async fn run_modern_tui(mut engine: QueryEngine) -> anyhow::Result<()> {
    let model = engine.state().config.api.model.clone();
    let cwd = engine.state().cwd.clone();
    let session_id = engine.state().session_id.clone();
    let base_permission_mode = engine.state().config.permissions.default_mode;
    let bypass_disabled = engine.state().config.security.disable_bypass_permissions;

    // Apply theme so any shared color helpers still resolve.
    let configured = engine.state().config.ui.theme.clone();
    let inherit_fg = engine.state().config.ui.inherit_fg;
    let theme_name = crate::ui::theme::resolve_theme(&configured);
    crate::ui::theme::init_with_options(&theme_name, &configured, inherit_fg);

    // Session notes / tips (lightweight — same as classic startup).
    agent_code_lib::memory::session_notes::init_session_notes(&session_id);

    // SessionStart already fired in main before we get here.

    // Engine → UI event channel. Created before `Session` wraps the engine
    // so the permission prompter can be installed: without one, the tool
    // executor treats `ask` decisions as auto-allow (non-interactive
    // default), which must never happen in an interactive surface.
    let (eng_tx, eng_rx) = mpsc::unbounded_channel::<EngineEvent>();
    engine.set_permission_prompter(ModernPrompter::new(eng_tx.clone()));

    let session = Session::new(engine);
    let mut app = App::new(model, cwd, session_id);

    // Restore the terminal even if the draw path panics.
    install_panic_restore_hook();

    let mut terminal = setup_terminal()?;
    let result = event_loop(
        &mut terminal,
        &session,
        &mut app,
        eng_tx,
        eng_rx,
        base_permission_mode,
        bypass_disabled,
    )
    .await;
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
    if let Err(e) = execute!(out, EnterAlternateScreen) {
        // Don't leave the shell in raw mode if the alt screen failed.
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    let backend = CrosstermBackend::new(out);
    match Terminal::new(backend) {
        Ok(terminal) => Ok(terminal),
        Err(e) => {
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
            Err(e.into())
        }
    }
}

fn restore_terminal(terminal: &mut Term) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Chain a panic hook that restores the terminal (raw mode off, leave alt
/// screen, cursor visible) before the default hook prints the panic, so a
/// panic in the draw path never leaves the user's shell unusable.
fn install_panic_restore_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
        prev(info);
    }));
}

#[allow(clippy::too_many_arguments)]
async fn event_loop(
    terminal: &mut Term,
    session: &Session,
    app: &mut App,
    eng_tx: mpsc::UnboundedSender<EngineEvent>,
    mut eng_rx: mpsc::UnboundedReceiver<EngineEvent>,
    base_permission_mode: PermissionMode,
    bypass_disabled: bool,
) -> anyhow::Result<()> {
    let mut turn: Option<TurnHandle> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Sync SessionMode with the engine when it changes. Only advance
    // `last_mode` after a successful apply — if the turn task holds the
    // engine mutex, retry on a later loop iteration.
    let mut last_mode = app.mode;

    loop {
        // Apply session mode → engine (best-effort).
        if app.mode != last_mode {
            if app.mode == super::mode::SessionMode::AlwaysApprove && bypass_disabled {
                app.transcript.push(super::app::TranscriptItem::Warning(
                    "always-approve is disabled by security.disable_bypass_permissions".into(),
                ));
                app.mode = super::mode::SessionMode::Normal;
            }
            if app.mode == last_mode
                || apply_mode_to_engine(session, app.mode, base_permission_mode).await
            {
                last_mode = app.mode;
            }
        }
        app.mode_pending = app.mode != last_mode;

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
                    if let Ok(mut eng) = engine_arc.try_lock() {
                        app.cost_usd = eng.state().total_cost_usd;
                        app.tokens_in = eng.state().total_usage.input_tokens;
                        app.tokens_out = eng.state().total_usage.output_tokens;
                        app.turn_count = eng.state().turn_count;
                        app.model = eng.state().config.api.model.clone();

                        // The model can toggle plan mode itself
                        // (EnterPlanMode/ExitPlanMode tools). Sync the badge
                        // — and the permission override — back from the
                        // engine, unless the user has a newer pending switch.
                        if app.mode == last_mode {
                            let engine_plan = eng.state().plan_mode;
                            let ui_plan = app.mode == super::mode::SessionMode::Plan;
                            if engine_plan != ui_plan {
                                app.mode = if engine_plan {
                                    super::mode::SessionMode::Plan
                                } else {
                                    super::mode::SessionMode::Normal
                                };
                                last_mode = app.mode;
                                eng.state_mut().config.permissions.default_mode =
                                    app.mode.permission_hint().unwrap_or(base_permission_mode);
                                app.transcript
                                    .push(super::app::TranscriptItem::System(format!(
                                        "mode synced from engine → {}",
                                        app.mode.label()
                                    )));
                            }
                        }
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

    // Cancel any in-flight turn on exit. Deny a pending permission first:
    // the turn task is blocked inside the prompter until it gets an
    // answer, and `join()` below would deadlock otherwise.
    if let Some(h) = turn.take() {
        app.resolve_permission(PermissionResponse::Deny);
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

    // Permission modal captures all input until answered. Esc/Ctrl+C mean
    // deny (never cancel/quit from inside the modal).
    if app.phase == super::app::Phase::Permission {
        match (key.modifiers, key.code) {
            (_, KeyCode::Char('y')) | (_, KeyCode::Char('1')) => {
                app.resolve_permission(PermissionResponse::AllowOnce);
            }
            (_, KeyCode::Char('a')) | (_, KeyCode::Char('2')) => {
                app.resolve_permission(PermissionResponse::AllowSession);
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c'))
            | (_, KeyCode::Esc)
            | (_, KeyCode::Char('n'))
            | (_, KeyCode::Char('3')) => {
                app.resolve_permission(PermissionResponse::Deny);
            }
            _ => {}
        }
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
        // Only plain / shifted characters type into the prompt; Ctrl/Alt
        // chords must not fall through as literal input.
        (m, KeyCode::Char(c))
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            app.insert_char(c);
        }
        _ => {}
    }
}

/// Apply UI session mode to the engine. Returns `true` if the lock was
/// acquired and the state was updated; `false` if the engine is busy
/// (caller should retry without updating its "last applied" tracker).
///
/// Mirrors the `--permission-mode` CLI plumbing: `plan_mode` is the
/// read-only safety switch, and `permissions.default_mode` carries the
/// AcceptEdits / AlwaysApprove semantics (Normal restores the mode the
/// session started with, so user config survives a round-trip).
async fn apply_mode_to_engine(
    session: &Session,
    mode: super::mode::SessionMode,
    base_permission_mode: PermissionMode,
) -> bool {
    let engine_arc = session.engine();
    let Ok(mut eng) = engine_arc.try_lock() else {
        return false;
    };
    let state = eng.state_mut();
    state.plan_mode = matches!(mode, super::mode::SessionMode::Plan);
    state.config.permissions.default_mode = mode.permission_hint().unwrap_or(base_permission_mode);
    true
}
