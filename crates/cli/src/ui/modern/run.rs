//! Live event loop for the modern TUI.
//!
//! Owns the terminal (alt-screen + raw mode), drives [`App`], and runs
//! turns through [`Session::spawn_turn`] so drawing never blocks on the
//! engine lock.

use std::io::{Stdout, stdout};
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

/// Second Ctrl+C within this window (on an empty prompt) quits.
const QUIT_ARM_WINDOW: Duration = Duration::from_millis(1500);

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

    // Don't silently lose prompts queued but never sent (plan §M5).
    if !app.queue.is_empty() {
        println!("\nUnsent queued prompts:");
        for (i, p) in app.queue.iter().enumerate() {
            println!("  {}. {p}", i + 1);
        }
    }

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
    let mut term_events = EventStream::new();

    // Spinner animation (~12 fps) and coalescer flush deadline (~10 fps).
    // Both are only *polled* while a turn is live / text is buffered, so an
    // idle session never wakes on them.
    let mut anim_tick = tokio::time::interval(Duration::from_millis(80));
    anim_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut flush_tick = tokio::time::interval(super::stream_buffer::FLUSH_INTERVAL);
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Sync SessionMode with the engine when it changes. Only advance
    // `last_mode` after a successful apply — if the turn task holds the
    // engine mutex, retry on a later loop iteration.
    let mut last_mode = app.mode;
    let mut quit_armed_at: Option<Instant> = None;

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
            app.dirty = true;
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
            app.dirty = true;
        }

        // Cancel if requested.
        if app.cancel_requested {
            if let Some(ref h) = turn {
                h.cancel();
            }
            app.cancel_requested = false;
        }

        // Reap finished turn.
        if let Some(ref h) = turn {
            use agent_code_lib::query::TurnStatus;
            let status = h.status();
            if matches!(
                status,
                TurnStatus::Completed | TurnStatus::Aborted | TurnStatus::Errored(_)
            ) {
                let completed_ok = matches!(status, TurnStatus::Completed);
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

                // Queue handling (plan §M5): auto-send the head on a clean
                // finish; on abort/error keep the queue and tell the user.
                if completed_ok {
                    app.dispatch_queue_head();
                } else if !app.queue.is_empty() {
                    app.transcript.push(super::app::TranscriptItem::System(
                        "queued prompts kept — press Enter to send".into(),
                    ));
                }
            }
        }

        // Draw only when something changed. An idle session with no events
        // and no pending deltas never repaints (plan §2.2 rule 1).
        if app.dirty {
            terminal.draw(|f| render::draw(f, app))?;
            app.dirty = false;
        }

        if app.should_quit {
            break;
        }

        // A turn is "live" while its handle exists or text is still buffered.
        // The flush + spinner timers are only polled while live, so an idle
        // session parks on the two channel branches with zero wakeups.
        let live = turn.is_some() || app.stream_buf.has_pending();

        tokio::select! {
            // Terminal input.
            maybe_ev = term_events.next() => {
                match maybe_ev {
                    Some(Ok(Event::Key(key))) => {
                        // Disarm a stale quit before routing the key so a late
                        // second Ctrl+C re-arms instead of quitting.
                        if app.quit_armed
                            && quit_armed_at.map(|t| t.elapsed() > QUIT_ARM_WINDOW).unwrap_or(true)
                        {
                            app.quit_armed = false;
                            quit_armed_at = None;
                        }
                        let was_armed = app.quit_armed;
                        handle_key(app, key);
                        app.dirty = true;
                        // Track when the quit arm was raised so it can expire.
                        if app.quit_armed && !was_armed {
                            quit_armed_at = Some(Instant::now());
                        } else if !app.quit_armed {
                            quit_armed_at = None;
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => { app.dirty = true; }
                    Some(Ok(_)) => {}
                    // Stream closed or errored: stop the UI cleanly.
                    Some(Err(_)) | None => { app.should_quit = true; }
                }
            }
            // Engine → UI events.
            Some(ev) = eng_rx.recv() => {
                app.apply_engine(ev);
            }
            // Coalescer flush deadline (only while text is buffered).
            _ = flush_tick.tick(), if app.stream_buf.has_pending() => {
                app.flush_stream();
            }
            // Spinner animation (only while a turn is live).
            _ = anim_tick.tick(), if live => {
                if app.phase == super::app::Phase::Streaming {
                    app.tick();
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

    // Any keypress other than the arming Ctrl+C disarms quit; capture the
    // prior arm state so a second Ctrl+C can act on it (§5 Ctrl+C machine).
    let was_armed = app.quit_armed;
    app.quit_armed = false;

    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if app.phase == super::app::Phase::Streaming {
                // Ctrl+C is the ONLY cancel (Esc never cancels a turn).
                app.request_cancel();
            } else if !app.input.is_empty() {
                app.clear_prompt();
            } else if was_armed {
                app.should_quit = true;
            } else {
                app.quit_armed = true;
                app.status_message = "press Ctrl+C again to quit".into();
            }
        }
        (KeyModifiers::CONTROL, KeyCode::Char('d')) if app.input.is_empty() => {
            app.should_quit = true;
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) | (KeyModifiers::SHIFT, KeyCode::Tab) => {
            app.cycle_mode_forward();
        }
        // Queue editing (plan §M5): Alt+↑ pops the newest queued prompt back
        // into the editor; Alt+- deletes it.
        (KeyModifiers::ALT, KeyCode::Up) => app.pop_newest_queued_to_editor(),
        (KeyModifiers::ALT, KeyCode::Char('-')) => app.delete_newest_queued(),
        (_, KeyCode::Esc) => {
            // Navigation only: clear the prompt; NEVER cancel a running turn.
            app.clear_prompt();
        }
        (_, KeyCode::Enter) => {
            app.submit();
        }
        (_, KeyCode::Backspace) => app.backspace(),
        (_, KeyCode::Left) => app.move_left(),
        (_, KeyCode::Right) => app.move_right(),
        // Transcript scrolling. Up/wheel enters Free; End/Home jump.
        (_, KeyCode::Up) => app.scroll_up(1),
        (_, KeyCode::Down) => app.scroll_down(1),
        (_, KeyCode::PageUp) => app.scroll_up(app.viewport_h.max(1)),
        (_, KeyCode::PageDown) => app.scroll_down(app.viewport_h.max(1)),
        (KeyModifiers::CONTROL, KeyCode::Char('u')) => app.scroll_up(app.viewport_h / 2),
        (_, KeyCode::Home) => app.scroll_to_top(),
        (_, KeyCode::End) => app.scroll_to_bottom(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::modern::app::Phase;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_c_while_streaming_cancels_not_quits() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        handle_key(&mut app, ctrl('c'));
        assert!(app.cancel_requested);
        assert!(!app.should_quit);
    }

    #[test]
    fn ctrl_c_with_text_clears_prompt_not_quit() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "hello".into();
        app.cursor = 5;
        handle_key(&mut app, ctrl('c'));
        assert!(app.input.is_empty());
        assert!(!app.should_quit);
        assert!(!app.quit_armed);
    }

    #[test]
    fn ctrl_c_double_press_arms_then_quits() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('c'));
        assert!(app.quit_armed, "first Ctrl+C arms");
        assert!(!app.should_quit);
        handle_key(&mut app, ctrl('c'));
        assert!(app.should_quit, "second Ctrl+C quits");
    }

    #[test]
    fn any_key_disarms_quit() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('c'));
        assert!(app.quit_armed);
        // A non-inserting key (leaves the prompt empty) still disarms.
        handle_key(&mut app, key(KeyCode::Left));
        assert!(!app.quit_armed);
        // A subsequent lone Ctrl+C should only re-arm, not quit.
        handle_key(&mut app, ctrl('c'));
        assert!(app.quit_armed);
        assert!(!app.should_quit);
    }

    #[test]
    fn esc_clears_prompt_never_cancels_turn() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.input = "typed while running".into();
        app.cursor = app.input.len();
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.input.is_empty(), "Esc clears the prompt");
        assert!(!app.cancel_requested, "Esc must NEVER cancel a turn");
        assert!(!app.should_quit);
    }

    #[test]
    fn permission_modal_esc_denies_without_quitting() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, rx) = std::sync::mpsc::channel();
        app.pending_permission = Some(crate::ui::modern::app::PendingPermission {
            name: "Bash".into(),
            description: "d".into(),
            input_preview: None,
            respond,
        });
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::Deny)));
        assert!(!app.should_quit);
        assert!(app.pending_permission.is_none());
    }
}
