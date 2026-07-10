//! Live event loop for the modern TUI.
//!
//! Owns the terminal (alt-screen + raw mode), drives [`App`], and runs
//! turns through [`Session::spawn_turn`] so drawing never blocks on the
//! engine lock.

use std::io::{Stdout, stdout};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;

use super::terminal_caps::TerminalCaps;
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
use super::sink::{ChannelSink, EngineEvent, ModernPrompter, ModernQuestionAsker};

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Run the modern full-screen TUI until the user quits.
pub async fn run_modern_tui(mut engine: QueryEngine) -> anyhow::Result<()> {
    let model = engine.state().config.api.model.clone();
    let cwd = engine.state().cwd.clone();
    let session_id = engine.state().session_id.clone();
    let base_permission_mode = engine.state().config.permissions.default_mode;
    let disable_skill_shell = engine.state().config.security.disable_skill_shell_execution;

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
    // Route AskUserQuestion through a UI modal instead of stdin (which would
    // hang under the alt-screen raw mode).
    engine.set_question_asker(ModernQuestionAsker::new(eng_tx.clone()));

    let session = Session::new(engine);
    let mut app = App::new_with_security(model, cwd, session_id, disable_skill_shell);

    // Restore the terminal even if the draw path panics.
    install_panic_restore_hook();
    let caps = probe_caps();

    let mut terminal = setup_terminal()?;
    let result = event_loop(
        &mut terminal,
        &session,
        &mut app,
        eng_tx,
        eng_rx,
        base_permission_mode,
        caps,
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

fn probe_caps() -> TerminalCaps {
    let enhancement = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    TerminalCaps::detect(|k| std::env::var(k).ok(), enhancement)
}

fn setup_terminal() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut out = stdout();
    // Enter alt screen and enable focus + bracketed paste + mouse capture.
    // All are consumed by the loop and disabled on exit so no `^[[I`/`^[[O`
    // (focus), paste brackets, or mouse tracking leak into the shell
    // (plan §M7/§M9).
    if let Err(e) = execute!(
        out,
        EnterAlternateScreen,
        EnableFocusChange,
        EnableBracketedPaste,
        EnableMouseCapture,
    ) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    let backend = CrosstermBackend::new(out);
    match Terminal::new(backend) {
        Ok(terminal) => Ok(terminal),
        Err(e) => {
            restore_stdout_modes();
            Err(e.into())
        }
    }
}

/// Undo every terminal mode we enabled, in reverse order. Idempotent and
/// used by both the normal restore and the panic hook.
fn restore_stdout_modes() {
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = disable_raw_mode();
}

fn restore_terminal(terminal: &mut Term) -> anyhow::Result<()> {
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
    )?;
    terminal.show_cursor()?;
    disable_raw_mode()?;
    Ok(())
}

/// Chain a panic hook that restores the terminal (raw mode off, focus/paste
/// reporting off, leave alt screen, cursor visible) before the default hook
/// prints the panic, so a panic never leaves the user's shell unusable or
/// leaking focus escape sequences.
fn install_panic_restore_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_stdout_modes();
        prev(info);
    }));
}

/// Draw one frame, wrapped in a DEC 2026 synchronized update when the
/// terminal supports it — the tmux/VS Code flicker fix (plan §M7). The
/// begin/end are best-effort; a terminal that ignores them just renders
/// normally.
fn draw_frame(terminal: &mut Term, app: &mut App, caps: TerminalCaps) -> anyhow::Result<()> {
    if caps.sync_output {
        let _ = execute!(terminal.backend_mut(), BeginSynchronizedUpdate);
    }
    // Map away the returned CompletedFrame so it doesn't hold a borrow of
    // `terminal` across the End-sync `execute!` below.
    let res = terminal.draw(|f| render::draw(f, app)).map(|_| ());
    if caps.sync_output {
        let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    }
    res?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn event_loop(
    terminal: &mut Term,
    session: &Session,
    app: &mut App,
    eng_tx: mpsc::UnboundedSender<EngineEvent>,
    mut eng_rx: mpsc::UnboundedReceiver<EngineEvent>,
    base_permission_mode: PermissionMode,
    caps: TerminalCaps,
) -> anyhow::Result<()> {
    app.caps = caps;
    let mut turn: Option<TurnHandle> = None;
    let mut term_events = EventStream::new();

    // Spinner animation (~12 fps) and coalescer flush deadline (~10 fps).
    // Both are only *polled* while a turn is live / text is buffered, so an
    // idle session never wakes on them.
    let mut anim_tick = tokio::time::interval(Duration::from_millis(80));
    anim_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut flush_tick = tokio::time::interval(super::stream_buffer::FLUSH_INTERVAL);
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Sync SessionMode with the engine when it changes.
    let mut last_mode = app.mode;
    let mut quit_armed_at: Option<Instant> = None;

    loop {
        // Apply a mode change to the engine. `apply_live_mode` updates the
        // lock-free live plan flag + PermissionChecker default, so the change
        // takes effect at the executor's next decision point *even mid-turn*
        // while the turn task holds the engine mutex (the exact bug the API
        // was built to close). The AppState sync is best-effort for observers.
        // The mode→permission policy lives entirely in `SessionMode`
        // (`permission_hint`); the loop just applies it — no per-mode
        // special-cases here.
        if app.mode != last_mode {
            apply_mode_to_engine(session, app.mode, base_permission_mode);
            last_mode = app.mode;
            app.dirty = true;
        }

        // Apply deferred `/model` (list or set). try_lock so a mid-turn
        // switch does not block the UI; if the turn holds the mutex we
        // retry on the next loop iteration.
        if let Some(action) = app.pending_model.take() {
            let engine_arc = session.engine();
            match engine_arc.try_lock() {
                Ok(mut eng) => {
                    let current = eng.state().config.api.model.clone();
                    let base_url = eng.state().config.api.base_url.clone();
                    app.apply_model_action(action, &current, &base_url, |name| {
                        eng.state_mut().config.api.model = name;
                    });
                }
                Err(_) => {
                    app.pending_model = Some(action);
                }
            }
        }

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
        // and no pending deltas never repaints (plan §2.2 rule 1). The draw
        // is wrapped in a synchronized update when the terminal supports it.
        if app.dirty {
            draw_frame(terminal, app, caps)?;
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
                    // Bracketed paste is enabled at setup; without this arm
                    // pastes are silently dropped (terminals stop emitting
                    // per-key events for the paste block).
                    Some(Ok(Event::Paste(text))) => {
                        app.quit_armed = false;
                        quit_armed_at = None;
                        handle_paste(app, &text);
                        app.dirty = true;
                    }
                    Some(Ok(Event::Mouse(m))) => handle_mouse(app, m),
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

    // Cancel any in-flight turn on exit. Deny every pending permission
    // first: turn tasks blocked inside the prompter would otherwise
    // deadlock the `join()` below.
    if let Some(h) = turn.take() {
        app.deny_all_modals();
        h.cancel();
        let _ = h.join().await;
    }

    Ok(())
}

/// True for Esc, Ctrl+C / Ctrl+Shift+C / Cmd+C (Super), or raw ETX.
///
/// Classic REPL: "Esc / Ctrl+C cancel". Real terminals often set extra
/// modifier bits (e.g. SHIFT with Ctrl+C) so exact `KeyModifiers::CONTROL`
/// equality silently drops the key — use `.contains(CONTROL)` instead.
fn is_interrupt_chord(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => true,
        // Raw ETX (0x03) — some paths deliver the byte without CONTROL.
        KeyCode::Char('\u{3}') => true,
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') => {
            key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER)
        }
        _ => false,
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // Ignore key release on platforms that emit them. Accept Repeat so a
    // held Ctrl+C still counts (double-tap quit / cancel).
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return;
    }

    // Permission modal captures all input until answered. Interrupt/Esc mean
    // deny (and interrupt also cancels the in-flight turn).
    if app.phase == super::app::Phase::Permission {
        use super::app::Modal;
        if is_interrupt_chord(&key) {
            match app.front_modal() {
                Some(Modal::Permission(_)) => {
                    app.resolve_permission(PermissionResponse::Deny);
                    // Also stop the turn — "get me out" not just "deny this tool".
                    app.request_cancel();
                }
                Some(Modal::Plan(_)) => {
                    app.resolve_plan(false, false);
                }
                Some(Modal::Question(_)) => {
                    // Drop respond channel (ask fails closed) and cancel turn.
                    app.deny_all_modals();
                    app.phase = super::app::Phase::Streaming;
                    app.request_cancel();
                }
                None => {
                    app.request_cancel();
                }
            }
            return;
        }
        match app.front_modal() {
            Some(Modal::Permission(_)) => match (key.modifiers, key.code) {
                (_, KeyCode::Char('y')) | (_, KeyCode::Char('1')) => {
                    app.resolve_permission(PermissionResponse::AllowOnce);
                }
                (_, KeyCode::Char('a')) | (_, KeyCode::Char('2')) => {
                    app.resolve_permission(PermissionResponse::AllowSession);
                }
                (_, KeyCode::Esc) | (_, KeyCode::Char('n')) | (_, KeyCode::Char('3')) => {
                    app.resolve_permission(PermissionResponse::Deny);
                }
                _ => {}
            },
            Some(Modal::Plan(_)) => match (key.modifiers, key.code) {
                (_, KeyCode::Char('a')) => {
                    app.resolve_plan(true, false);
                }
                (_, KeyCode::Char('k')) => {
                    app.resolve_plan(false, true);
                }
                (_, KeyCode::Esc) => {
                    app.resolve_plan(false, false);
                }
                _ => {}
            },
            Some(Modal::Question(_)) => match (key.modifiers, key.code) {
                (_, KeyCode::Up) => app.question_move(-1),
                (_, KeyCode::Down) => app.question_move(1),
                (_, KeyCode::Enter) => app.question_select(None),
                (_, KeyCode::Esc) => {
                    app.deny_all_modals();
                    app.phase = super::app::Phase::Streaming;
                    app.request_cancel();
                }
                (_, KeyCode::Char(c)) if c.is_ascii_digit() && c != '0' => {
                    app.question_select(Some((c as usize - '1' as usize).min(8)));
                }
                _ => {}
            },
            None => {}
        }
        return;
    }

    // Any keypress other than the arming interrupt disarms quit; capture the
    // prior arm state so a second Ctrl+C can act on it (§5 Ctrl+C machine).
    let was_armed = app.quit_armed;
    app.quit_armed = false;

    if is_interrupt_chord(&key) {
        if app.phase == super::app::Phase::Streaming {
            // Esc / Ctrl+C cancel the running turn (classic REPL parity).
            app.request_cancel();
            app.transcript.push(super::app::TranscriptItem::System(
                "interrupted — cancelling turn…".into(),
            ));
        } else if !app.input.is_empty() {
            app.clear_prompt();
        } else if was_armed {
            app.should_quit = true;
        } else {
            app.quit_armed = true;
            app.status_message = "press Esc/Ctrl+C again to quit".into();
            // Status bar alone is easy to miss — also pin a transcript line.
            app.transcript.push(super::app::TranscriptItem::System(
                "Esc or Ctrl+C again to quit · or type /exit · or Ctrl+D".into(),
            ));
        }
        return;
    }

    match (key.modifiers, key.code) {
        (m, KeyCode::Char('d') | KeyCode::Char('D'))
            if m.contains(KeyModifiers::CONTROL) && app.input.is_empty() =>
        {
            app.should_quit = true;
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) | (KeyModifiers::SHIFT, KeyCode::Tab) => {
            app.cycle_mode_forward();
        }
        // Queue editing (plan §M5): Alt+↑ pops the newest queued prompt back
        // into the editor; Alt+- deletes it.
        (KeyModifiers::ALT, KeyCode::Up) => app.pop_newest_queued_to_editor(),
        (KeyModifiers::ALT, KeyCode::Char('-')) => app.delete_newest_queued(),
        // Toggle the tasks/agents pane (plan §M8).
        (m, KeyCode::Char('t') | KeyCode::Char('T')) if m.contains(KeyModifiers::CONTROL) => {
            app.toggle_tasks()
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
        (m, KeyCode::Char('u') | KeyCode::Char('U')) if m.contains(KeyModifiers::CONTROL) => {
            app.scroll_up(app.viewport_h / 2)
        }
        (_, KeyCode::Home) => app.scroll_to_top(),
        (_, KeyCode::End) => app.scroll_to_bottom(),
        // Only plain / shifted characters type into the prompt; Ctrl/Alt/Super
        // chords must not fall through as literal input.
        (m, KeyCode::Char(c))
            if !m.contains(KeyModifiers::CONTROL)
                && !m.contains(KeyModifiers::ALT)
                && !m.contains(KeyModifiers::SUPER) =>
        {
            app.insert_char(c);
        }
        _ => {}
    }
}

/// Insert bracketed-paste text into the prompt (or ignore during modals).
fn handle_paste(app: &mut App, text: &str) {
    // Modals own the keyboard; don't dump clipboard into the prompt behind them.
    if app.phase == super::app::Phase::Permission {
        return;
    }
    app.insert_str(text);
}

/// Route a mouse event (plan §M9). Wheel scrolls the transcript; a left
/// click on the bottom row (where the jump pill sits) returns to Follow.
/// Shift/Alt-modified drags are left to the terminal's native selection.
fn handle_mouse(app: &mut App, m: MouseEvent) {
    match m.kind {
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        // Clicking near the bottom of the transcript jumps to the live tail
        // (the jump pill target). Cheap heuristic without full hit-testing:
        // bottom row of the transcript region.
        MouseEventKind::Down(MouseButton::Left)
            if !app.scroll.is_following() && m.row as usize >= app.viewport_h =>
        {
            app.scroll_to_bottom();
        }
        _ => {}
    }
}

/// Apply a UI session mode to the engine so it takes effect immediately —
/// even mid-turn while the turn task holds the engine mutex.
///
/// `Session::apply_live_mode` updates the lock-free live plan flag and the
/// `PermissionChecker` default, which the executor reads at its next
/// decision point (§3.4.2 / AUDIT.md §5) — so Shift+Tab into Plan stops the
/// next write without waiting for the turn to finish. The `try_lock`
/// AppState write is only a best-effort sync for observers (the badge and
/// engine-initiated `EnterPlanMode`); it never gates whether the mode
/// applied. `Normal` restores the mode the session started with.
fn apply_mode_to_engine(
    session: &Session,
    mode: super::mode::SessionMode,
    base_permission_mode: PermissionMode,
) {
    let plan = matches!(mode, super::mode::SessionMode::Plan);
    let perm = mode.permission_hint().unwrap_or(base_permission_mode);
    // Lock-free — always applies, mid-turn.
    session.apply_live_mode(plan, perm);
    // Best-effort AppState sync (do not block the UI loop on the turn's lock).
    if let Ok(mut eng) = session.engine().try_lock() {
        let state = eng.state_mut();
        state.plan_mode = plan;
        state.config.permissions.default_mode = perm;
    }
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

    fn ctrl_shift(c: char) -> KeyEvent {
        KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )
    }

    fn super_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SUPER)
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
    fn ctrl_shift_c_and_uppercase_still_cancel() {
        // Terminals often set SHIFT with Ctrl+C; exact-modifier match used to drop these.
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        handle_key(&mut app, ctrl_shift('c'));
        assert!(app.cancel_requested, "Ctrl+Shift+c must cancel");

        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        handle_key(&mut app, ctrl('C'));
        assert!(app.cancel_requested, "Ctrl+C (uppercase) must cancel");
    }

    #[test]
    fn super_c_interrupts_like_ctrl_c() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        handle_key(&mut app, super_key('c'));
        assert!(app.cancel_requested, "Cmd/Super+C must cancel");
    }

    #[test]
    fn raw_etx_cancels_turn() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        handle_key(&mut app, key(KeyCode::Char('\u{3}')));
        assert!(app.cancel_requested);
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
    fn ctrl_shift_c_double_press_quits() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl_shift('c'));
        assert!(app.quit_armed);
        handle_key(&mut app, ctrl_shift('C'));
        assert!(app.should_quit);
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
    fn request_cancel_works_even_when_phase_idle() {
        // Phase desync must not swallow interrupt.
        let mut app = App::new("m", "/tmp", "s");
        assert_eq!(app.phase, Phase::Idle);
        app.request_cancel();
        assert!(app.cancel_requested);
    }

    #[test]
    fn esc_while_streaming_cancels_turn() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.input = "typed while running".into();
        app.cursor = app.input.len();
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(
            app.cancel_requested,
            "Esc cancels a running turn (classic parity)"
        );
        assert!(!app.should_quit);
    }

    #[test]
    fn esc_with_text_clears_prompt() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "hello".into();
        app.cursor = 5;
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.input.is_empty());
        assert!(!app.should_quit);
        assert!(!app.cancel_requested);
    }

    #[test]
    fn esc_double_press_arms_then_quits() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.quit_armed, "first Esc arms");
        assert!(!app.should_quit);
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.should_quit, "second Esc quits");
    }

    #[test]
    fn esc_then_ctrl_c_still_quits() {
        // Mix of interrupt keys shares the same quit arm.
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.quit_armed);
        handle_key(&mut app, ctrl('c'));
        assert!(app.should_quit);
    }

    #[test]
    fn paste_inserts_into_prompt() {
        let mut app = App::new("m", "/tmp", "s");
        handle_paste(&mut app, "hello\nworld");
        assert_eq!(app.input, "hello\nworld");
        assert_eq!(app.cursor, "hello\nworld".len());
    }

    #[test]
    fn paste_ignored_during_permission() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        handle_paste(&mut app, "secret");
        assert!(app.input.is_empty());
    }

    fn mouse(kind: MouseEventKind, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn wheel_up_enters_free_wheel_down_follows() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.clear();
        for i in 0..100 {
            app.transcript
                .push(crate::ui::modern::app::TranscriptItem::System(format!(
                    "l {i}"
                )));
        }
        app.layout.sync(&app.transcript, 80);
        app.viewport_h = 20;
        handle_mouse(&mut app, mouse(MouseEventKind::ScrollUp, 5));
        assert!(!app.scroll.is_following(), "wheel up enters Free");
        // Wheel down enough to reach the bottom re-enters Follow.
        for _ in 0..10 {
            handle_mouse(&mut app, mouse(MouseEventKind::ScrollDown, 5));
        }
        assert!(app.scroll.is_following(), "wheel down returns to Follow");
    }

    #[test]
    fn click_bottom_row_jumps_to_follow() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript.clear();
        for i in 0..100 {
            app.transcript
                .push(crate::ui::modern::app::TranscriptItem::System(format!(
                    "l {i}"
                )));
        }
        app.layout.sync(&app.transcript, 80);
        app.viewport_h = 20;
        app.scroll_up(30);
        assert!(!app.scroll.is_following());
        // Click on the bottom row (row >= viewport_h) → follow.
        handle_mouse(&mut app, mouse(MouseEventKind::Down(MouseButton::Left), 22));
        assert!(app.scroll.is_following(), "click at bottom follows");
    }

    // Assert a command's ANSI byte sequence via `Command::write_ansi`, which
    // is cross-platform — unlike `execute!`, which on Windows takes the
    // console (winapi) path and fails without a real console under test.
    fn ansi_of(cmd: impl crossterm::Command) -> String {
        let mut s = String::new();
        cmd.write_ansi(&mut s).unwrap();
        s
    }

    #[test]
    fn restore_sequence_disables_mouse_capture() {
        // Mouse tracking off = CSI ?1000l (and friends); assert the base one.
        let s = ansi_of(DisableMouseCapture);
        assert!(
            s.contains("\x1b[?1000l"),
            "mouse capture not disabled: {s:?}"
        );
    }

    #[test]
    fn restore_sequence_disables_focus_and_paste_reporting() {
        // The bytes we emit on restore must turn OFF focus reporting and
        // bracketed paste so no `^[[I`/`^[[O` or paste brackets leak into the
        // shell after exit (plan §M7).
        let s = format!(
            "{}{}",
            ansi_of(DisableFocusChange),
            ansi_of(DisableBracketedPaste)
        );
        // Focus reporting off = CSI ?1004l; bracketed paste off = CSI ?2004l.
        assert!(
            s.contains("\x1b[?1004l"),
            "focus reporting not disabled: {s:?}"
        );
        assert!(
            s.contains("\x1b[?2004l"),
            "bracketed paste not disabled: {s:?}"
        );
    }

    #[test]
    fn permission_modal_esc_denies_without_quitting() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Permission;
        let (respond, rx) = std::sync::mpsc::channel();
        app.modals
            .push_back(crate::ui::modern::app::Modal::Permission(
                crate::ui::modern::app::PendingPermission {
                    name: "Bash".into(),
                    description: "d".into(),
                    origin: None,
                    input_preview: None,
                    respond,
                },
            ));
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::Deny)));
        assert!(!app.should_quit);
        assert!(app.front_permission().is_none());
    }
}
