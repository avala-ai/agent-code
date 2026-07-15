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
    app.caps = caps;

    let mut terminal = setup_terminal()?;
    let mut term_events = EventStream::new();
    let mut draw = |app: &mut App| draw_frame(&mut terminal, app, caps);
    let result = event_loop(
        &session,
        &mut app,
        eng_tx,
        eng_rx,
        base_permission_mode,
        &mut term_events,
        &mut draw,
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

/// Core select! loop, decoupled from the real terminal so the fake_engine
/// harness (#406) can drive it: `term_events` is any stream of crossterm
/// events (the real `EventStream` in production, a scripted channel in
/// tests) and `draw` renders a frame (real alt-screen terminal in
/// production, `TestBackend` in tests). Engine/session wiring is always
/// real — tests fake the *provider*, not the loop.
#[allow(clippy::too_many_arguments)]
pub(super) async fn event_loop(
    session: &Session,
    app: &mut App,
    eng_tx: mpsc::UnboundedSender<EngineEvent>,
    mut eng_rx: mpsc::UnboundedReceiver<EngineEvent>,
    base_permission_mode: PermissionMode,
    term_events: &mut (impl futures::Stream<Item = std::io::Result<Event>> + Unpin),
    draw: &mut dyn FnMut(&mut App) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut turn: Option<TurnHandle> = None;
    let mut loop_err: Option<anyhow::Error> = None;

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

        // Apply a deferred `/clear` to the engine conversation (classic
        // parity). try_lock like `/model`: if a turn holds the mutex we
        // retry next iteration (the live atomic state is unaffected).
        if app.pending_clear
            && let Ok(mut eng) = session.engine().try_lock()
        {
            eng.state_mut().messages.clear();
            app.pending_clear = false;
            app.status_message = "context cleared".into();
            app.dirty = true;
        }

        // Full classic slash-command bridge (stdout captured → transcript).
        // Run off the async worker via `block_in_place`: many slash arms call
        // `Handle::block_on` / spawn+join, which panic if invoked directly on
        // a Tokio worker without parking it first.
        if let Some(slash) = app.pending_slash.take() {
            match session.engine().try_lock() {
                Ok(mut eng) => {
                    let (result, captured) = tokio::task::block_in_place(|| {
                        crate::stdout_capture::capture_stdout(|| {
                            crate::commands::execute(&slash, &mut eng)
                        })
                    });
                    match result {
                        crate::commands::CommandResult::Exit => {
                            app.should_quit = true;
                        }
                        crate::commands::CommandResult::Prompt(p) => {
                            app.enqueue_turn_from_command(p);
                        }
                        crate::commands::CommandResult::Passthrough(p) => {
                            app.enqueue_turn_from_command(p);
                        }
                        crate::commands::CommandResult::Handled => {
                            let text = captured.trim();
                            if !text.is_empty() {
                                for line in text.lines() {
                                    // Strip ANSI for the transcript view.
                                    let plain = strip_ansi_simple(line);
                                    if !plain.is_empty() {
                                        app.transcript
                                            .push(super::app::TranscriptItem::System(plain));
                                    }
                                }
                            }
                            app.status_message = format!("ran {slash}");
                        }
                    }
                    app.dirty = true;
                }
                Err(_) => {
                    // Turn holds the lock — retry next loop.
                    app.pending_slash = Some(slash);
                }
            }
        }

        // `!cmd` shell passthrough (classic parity).
        if let Some(cmd) = app.pending_shell.take() {
            match session.engine().try_lock() {
                Ok(mut eng) => {
                    use agent_code_lib::services::shell_passthrough;
                    use std::sync::{Arc, Mutex};
                    let cwd = std::path::PathBuf::from(&app.cwd);
                    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
                    let out_l = lines.clone();
                    let err_l = lines.clone();
                    match shell_passthrough::run_and_capture(
                        &cmd,
                        &cwd,
                        move |line| {
                            if let Ok(mut g) = out_l.lock() {
                                g.push(line.to_string());
                            }
                        },
                        move |line| {
                            if let Ok(mut g) = err_l.lock() {
                                g.push(format!("[stderr] {line}"));
                            }
                        },
                    ) {
                        Ok(output) => {
                            if let Ok(g) = lines.lock() {
                                for line in g.iter() {
                                    app.transcript
                                        .push(super::app::TranscriptItem::System(line.clone()));
                                }
                            }
                            // Also show truncated capture if streaming missed.
                            if !output.text.is_empty() {
                                let preview: String = output.text.chars().take(500).collect();
                                if lines.lock().map(|g| g.is_empty()).unwrap_or(true) {
                                    app.transcript
                                        .push(super::app::TranscriptItem::System(preview));
                                }
                            }
                            if let Some(msg) =
                                shell_passthrough::build_context_message(&cmd, &output)
                            {
                                eng.state_mut().push_message(msg);
                            }
                            app.status_message = format!("! done · exit {:?}", output.exit_code);
                        }
                        Err(e) => {
                            app.transcript.push(super::app::TranscriptItem::Error(e));
                        }
                    }
                    app.dirty = true;
                }
                Err(_) => {
                    app.pending_shell = Some(cmd);
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
            app.turn_live = true;
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
                                // Apply to the LIVE handles (plan atomic +
                                // checker default), not just the config copy:
                                // after a model-initiated ExitPlanMode the
                                // checker default stayed Plan, so every
                                // subsequent edit was denied while the badge
                                // said NORMAL.
                                let hint =
                                    app.mode.permission_hint().unwrap_or(base_permission_mode);
                                session.apply_live_mode(engine_plan, hint);
                                eng.state_mut().config.permissions.default_mode = hint;
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
                // Interject leaves `pending_submit` set so we start it even
                // after Aborted (send-now cancel-and-send).
                if completed_ok {
                    app.dispatch_queue_head();
                } else if app.pending_submit.is_none() && !app.queue.is_empty() {
                    app.transcript.push(super::app::TranscriptItem::System(
                        "queued prompts kept — press Enter to send".into(),
                    ));
                }
                // Start a pending turn NOW (auto-queue head or interject).
                // The spawn check lives at the top of the loop; falling
                // through to `select!` would park until an unrelated event.
                if app.pending_submit.is_some() {
                    continue;
                }
            }
        }

        // Draw only when something changed. An idle session with no events
        // and no pending deltas never repaints (plan §2.2 rule 1). The draw
        // is wrapped in a synchronized update when the terminal supports it.
        if app.dirty {
            if let Err(e) = draw(app) {
                // Do NOT early-return: the teardown below must still deny
                // pending modals and join the turn. Returning here left a
                // turn task blocked in the prompter holding the engine
                // mutex, and run_modern_tui's SessionStop lock then hung
                // the process forever after the terminal was restored.
                loop_err = Some(e);
                break;
            }
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

    match loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Strip common CSI/OSC ANSI sequences for transcript display.
fn strip_ansi_simple(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for x in chars.by_ref() {
                        if x.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC … BEL or ST
                    chars.next();
                    for x in chars.by_ref() {
                        if x == '\u{7}' {
                            break;
                        }
                        if x == '\u{1b}' {
                            let _ = chars.next(); // skip \
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// True for Ctrl+C / Ctrl+Shift+C / Cmd+C (Super), or raw ETX.
///
/// **Not Esc.** Esc is navigate / dismiss / clear only — never cancels a
/// turn (world-class agent-screen contract; see ACCEPTANCE + KEYBINDINGS).
/// Real terminals often set extra modifier bits (e.g. SHIFT with Ctrl+C)
/// so exact `KeyModifiers::CONTROL` equality silently drops the key —
/// use `.contains(CONTROL)` instead.
fn is_cancel_chord(key: &KeyEvent) -> bool {
    match key.code {
        // Raw ETX (0x03) — some paths deliver the byte without CONTROL.
        KeyCode::Char('\u{3}') => true,
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') => {
            key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER)
        }
        _ => false,
    }
}

fn is_esc(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc)
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // Ignore key release on platforms that emit them. Accept Repeat so a
    // held Ctrl+C still counts (double-tap quit / cancel).
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return;
    }

    // HITL modals always win over the command palette. A permission ask
    // can arrive while the palette is open (streaming turn); dismiss the
    // palette so y/a/n, Esc, and Ctrl+C reach the modal.
    if app.phase == super::app::Phase::Permission {
        app.close_command_palette();
    }

    // Command palette captures input when open (and no HITL modal is up).
    if app.command_palette_open() {
        handle_palette_key(app, key);
        return;
    }

    // Permission modal captures all input until answered.
    // Esc = dismiss only. Ctrl+C = dismiss + cancel turn ("get me out").
    if app.phase == super::app::Phase::Permission {
        use super::app::Modal;
        if is_cancel_chord(&key) {
            match app.front_modal() {
                Some(Modal::Permission(_)) => {
                    app.resolve_permission(PermissionResponse::Deny);
                    app.request_cancel();
                }
                Some(Modal::Plan(_)) => {
                    app.resolve_plan(false, false);
                }
                Some(Modal::Question(_)) => {
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
        if is_esc(&key) {
            match app.front_modal() {
                Some(Modal::Permission(_)) => {
                    app.resolve_permission(PermissionResponse::Deny);
                }
                Some(Modal::Plan(_)) => {
                    app.resolve_plan(false, false);
                }
                Some(Modal::Question(_)) => {
                    // Drop respond channel (ask fails closed) without
                    // cancelling the turn — Esc is dismiss, not interrupt.
                    app.deny_all_modals();
                    if app.turn_live {
                        app.phase = super::app::Phase::Streaming;
                    } else {
                        app.phase = super::app::Phase::Idle;
                    }
                    app.dirty = true;
                }
                None => {}
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
                (_, KeyCode::Char('n')) | (_, KeyCode::Char('3')) => {
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
                _ => {}
            },
            Some(Modal::Question(_)) => match (key.modifiers, key.code) {
                (_, KeyCode::Up) => app.question_move(-1),
                (_, KeyCode::Down) => app.question_move(1),
                (_, KeyCode::Enter) => app.question_select(None),
                (_, KeyCode::Char(c)) if c.is_ascii_digit() && c != '0' => {
                    // Out-of-range digits are ignored by question_select.
                    app.question_select(Some(c as usize - '1' as usize));
                }
                _ => {}
            },
            None => {}
        }
        return;
    }

    // Any keypress other than the arming chord disarms quit; capture the
    // prior arm state so a second Ctrl+C / Esc can act on it.
    let was_armed = app.quit_armed;
    app.quit_armed = false;

    // Esc: never cancel a turn. Clear draft, or double-press quit when idle.
    if is_esc(&key) {
        if !app.input.is_empty() {
            app.clear_prompt();
        } else if app.phase == super::app::Phase::Streaming {
            // Mid-turn empty Esc is a no-op (use Ctrl+C to cancel).
            app.status_message = "Ctrl+C to cancel turn".into();
            app.dirty = true;
        } else if was_armed {
            app.should_quit = true;
        } else {
            app.quit_armed = true;
            app.status_message = "press Esc/Ctrl+C again to quit".into();
            app.transcript.push(super::app::TranscriptItem::System(
                "Esc or Ctrl+C again to quit · or type /exit · or Ctrl+D".into(),
            ));
        }
        return;
    }

    // Ctrl+C (and Super+C / ETX): cancel turn, clear draft, or double-quit.
    if is_cancel_chord(&key) {
        if app.phase == super::app::Phase::Streaming {
            // With a non-empty draft mid-turn: clear draft first, keep turn.
            if !app.input.is_empty() {
                app.clear_prompt();
            } else {
                app.request_cancel();
                app.transcript.push(super::app::TranscriptItem::System(
                    "interrupted — cancelling turn…".into(),
                ));
            }
        } else if !app.input.is_empty() {
            app.clear_prompt();
        } else if was_armed {
            app.should_quit = true;
        } else {
            app.quit_armed = true;
            app.status_message = "press Esc/Ctrl+C again to quit".into();
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
        // Command palette (Ctrl+P).
        (m, KeyCode::Char('p') | KeyCode::Char('P')) if m.contains(KeyModifiers::CONTROL) => {
            app.open_command_palette();
        }
        // Queue pane toggle (Ctrl+; / Ctrl+').
        (m, KeyCode::Char(';') | KeyCode::Char('\'')) if m.contains(KeyModifiers::CONTROL) => {
            app.toggle_queue_pane();
        }
        // When the queue pane is open, arrows and Enter drive it.
        (_, KeyCode::Up) if app.show_queue_pane && !app.queue.is_empty() => {
            app.queue_select_prev();
        }
        (_, KeyCode::Down) if app.show_queue_pane && !app.queue.is_empty() => {
            app.queue_select_next();
        }
        (_, KeyCode::Enter)
            if app.show_queue_pane && !app.queue.is_empty() && app.input.is_empty() =>
        {
            app.queue_send_selected();
        }
        (_, KeyCode::Backspace | KeyCode::Delete)
            if app.show_queue_pane && !app.queue.is_empty() && app.input.is_empty() =>
        {
            app.queue_delete_selected();
        }
        // Block copy: y = body, Y = metadata (only when a block is selected).
        (_, KeyCode::Char('y')) if app.input.is_empty() && app.selected_item.is_some() => {
            app.copy_selected_content();
        }
        (_, KeyCode::Char('Y')) if app.input.is_empty() && app.selected_item.is_some() => {
            app.copy_selected_meta();
        }
        // Interject / send-now: Ctrl+Enter (kitty keyboard) or Ctrl+I alt.
        (m, KeyCode::Enter) if m.contains(KeyModifiers::CONTROL) => {
            app.interject();
        }
        (m, KeyCode::Char('i') | KeyCode::Char('I')) if m.contains(KeyModifiers::CONTROL) => {
            // Alt chord when the terminal does not distinguish Ctrl+Enter.
            app.interject();
        }
        // Multiline compose: Ctrl+M toggles Enter vs Alt/Shift+Enter semantics.
        (m, KeyCode::Char('m') | KeyCode::Char('M')) if m.contains(KeyModifiers::CONTROL) => {
            app.toggle_multiline_mode();
        }
        // Alt+Enter / Shift+Enter: newline in normal mode, submit in multiline mode.
        (m, KeyCode::Enter) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::SHIFT) => {
            if app.multiline_mode {
                app.submit();
            } else {
                app.insert_newline();
            }
        }
        (_, KeyCode::Enter) => {
            if app.multiline_mode {
                app.insert_newline();
            } else {
                app.submit();
            }
        }
        (_, KeyCode::Backspace) => app.backspace(),
        // Tab completes slash commands when drafting `/…`.
        (_, KeyCode::Tab) if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.complete_slash_tab();
        }
        // Turn navigation (Shift+Left/Right) — before bare arrows.
        (m, KeyCode::Left) if m.contains(KeyModifiers::SHIFT) => {
            app.jump_prev_user_turn();
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::SHIFT) => {
            app.jump_next_user_turn();
        }
        // Fold / expand selected block (`e`) and all thinking (Ctrl+E).
        (m, KeyCode::Char('e') | KeyCode::Char('E')) if m.contains(KeyModifiers::CONTROL) => {
            app.toggle_expand_all_thinking();
        }
        // Only steal `e` when a block is already selected — otherwise it types.
        (_, KeyCode::Char('e')) if app.input.is_empty() && app.selected_item.is_some() => {
            app.toggle_expand_selected();
        }
        // Select prev/next block when composer is empty (scrollback focus lite).
        (_, KeyCode::Left) if app.input.is_empty() => {
            app.select_prev_item();
        }
        (_, KeyCode::Right) if app.input.is_empty() => {
            app.select_next_item();
        }
        (_, KeyCode::Left) => app.move_left(),
        (_, KeyCode::Right) => app.move_right(),
        // In a multi-line draft, ↑/↓ move within the composer; empty ↑ is
        // prompt history; otherwise scroll transcript.
        (_, KeyCode::Up) => {
            if app.input.is_empty() || app.history_browse.is_some() {
                app.history_older();
            } else if app.input_is_multiline() || app.multiline_mode {
                let (line, _) = app.cursor_line_col();
                if line > 0 {
                    app.move_up_line();
                } else {
                    app.scroll_up(1);
                }
            } else {
                app.scroll_up(1);
            }
        }
        (_, KeyCode::Down) => {
            if app.history_browse.is_some() {
                app.history_newer();
            } else if app.input_is_multiline() || app.multiline_mode {
                let (line, _) = app.cursor_line_col();
                if line + 1 < app.input_line_count() {
                    app.move_down_line();
                } else {
                    app.scroll_down(1);
                }
            } else {
                app.scroll_down(1);
            }
        }
        (_, KeyCode::PageUp) => app.scroll_up(app.viewport_h.max(1)),
        (_, KeyCode::PageDown) => app.scroll_down(app.viewport_h.max(1)),
        (m, KeyCode::Char('u') | KeyCode::Char('U')) if m.contains(KeyModifiers::CONTROL) => {
            app.scroll_up(app.viewport_h / 2)
        }
        // Home/End: line bounds when composing; transcript jump when empty.
        (_, KeyCode::Home) => {
            if app.input.is_empty() {
                app.scroll_to_top();
            } else {
                app.move_line_start();
            }
        }
        (_, KeyCode::End) => {
            if app.input.is_empty() {
                app.scroll_to_bottom();
            } else {
                app.move_line_end();
            }
        }
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
fn handle_palette_key(app: &mut App, key: KeyEvent) {
    // Ctrl+P toggles closed; Esc / Ctrl+C dismiss.
    if matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        app.close_command_palette();
        return;
    }
    if is_esc(&key) || is_cancel_chord(&key) {
        app.close_command_palette();
        return;
    }
    match key.code {
        KeyCode::Up => app.palette_move(-1),
        KeyCode::Down => app.palette_move(1),
        KeyCode::Enter | KeyCode::Tab => app.palette_accept(),
        KeyCode::Backspace => app.palette_backspace(),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.palette_insert_char(c);
        }
        _ => {}
    }
}

/// Shift/Alt-modified drags are left to the terminal's native selection.
fn handle_mouse(app: &mut App, m: MouseEvent) {
    match m.kind {
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        // Clicking near the bottom of the transcript jumps to the live tail
        // (the jump pill target). Cheap heuristic without full hit-testing:
        // bottom row of the transcript region.
        MouseEventKind::Down(MouseButton::Left)
            if !app.scroll.is_following()
                && app.transcript_bottom_row != 0
                && m.row == app.transcript_bottom_row =>
        {
            // Exactly the transcript's bottom screen row (recorded at last
            // draw) — comparing against viewport_h (a HEIGHT) made any click
            // on the lower half of the screen, including the input box,
            // silently snap the transcript to bottom.
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
    fn ctrl_p_opens_and_accepts_command_palette() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('p'));
        assert!(app.command_palette_open());
        for c in "hel".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(!app.command_palette_open());
        assert!(app.input.starts_with("/help"));
    }

    #[test]
    fn permission_phase_closes_palette_and_takes_keys() {
        use agent_code_lib::tools::PermissionResponse;
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('p'));
        assert!(app.command_palette_open());
        let (tx, rx) = std::sync::mpsc::channel();
        app.modals.push_back(super::super::app::Modal::Permission(
            super::super::app::PendingPermission {
                name: "Bash".into(),
                description: "run".into(),
                origin: None,
                input_preview: None,
                respond: tx,
            },
        ));
        app.phase = Phase::Permission;
        // y must reach the modal, not the palette filter.
        handle_key(&mut app, key(KeyCode::Char('y')));
        assert!(!app.command_palette_open());
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::AllowOnce)));
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
    fn alt_enter_inserts_newline_in_normal_mode() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "hi".into();
        app.cursor = 2;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(app.input, "hi\n");
        assert!(app.pending_submit.is_none());
    }

    #[test]
    fn shift_enter_inserts_newline_in_normal_mode() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "x".into();
        app.cursor = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(app.input, "x\n");
    }

    #[test]
    fn multiline_mode_enter_inserts_newline_shift_enter_sends() {
        let mut app = App::new("m", "/tmp", "s");
        app.multiline_mode = true;
        app.input = "line".into();
        app.cursor = 4;
        handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.input, "line\n");
        assert!(app.pending_submit.is_none());
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(app.pending_submit.as_deref(), Some("line"));
    }

    #[test]
    fn ctrl_m_toggles_multiline() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('m'));
        assert!(app.multiline_mode);
        handle_key(&mut app, ctrl('m'));
        assert!(!app.multiline_mode);
    }

    #[test]
    fn ctrl_enter_interjects() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.turn_live = true;
        app.input = "now".into();
        app.cursor = 3;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );
        assert!(app.cancel_requested);
        assert_eq!(app.pending_submit.as_deref(), Some("now"));
    }

    #[test]
    fn ctrl_i_interjects_as_alt_chord() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.turn_live = true;
        app.input = "alt".into();
        app.cursor = 3;
        handle_key(&mut app, ctrl('i'));
        assert!(app.cancel_requested);
        assert_eq!(app.pending_submit.as_deref(), Some("alt"));
    }

    #[test]
    fn esc_while_streaming_clears_draft_does_not_cancel() {
        // World-class contract: Esc never cancels. Ctrl+C cancels.
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.input = "typed while running".into();
        app.cursor = app.input.len();
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(!app.cancel_requested, "Esc must not cancel a running turn");
        assert!(app.input.is_empty(), "Esc clears mid-turn draft");
        assert!(!app.should_quit);
    }

    #[test]
    fn esc_while_streaming_empty_is_noop() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(!app.cancel_requested);
        assert!(!app.should_quit);
        assert!(!app.quit_armed, "mid-turn empty Esc must not arm quit");
    }

    #[test]
    fn ctrl_c_mid_turn_with_draft_clears_before_cancel() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.input = "note".into();
        app.cursor = 4;
        handle_key(&mut app, ctrl('c'));
        assert!(app.input.is_empty());
        assert!(
            !app.cancel_requested,
            "first Ctrl+C with draft clears draft only"
        );
        handle_key(&mut app, ctrl('c'));
        assert!(app.cancel_requested, "second Ctrl+C on empty cancels");
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
        app.layout
            .sync(&app.transcript, 80, &std::collections::HashSet::new(), None);
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
        app.layout
            .sync(&app.transcript, 80, &std::collections::HashSet::new(), None);
        app.viewport_h = 20;
        app.transcript_bottom_row = 22;
        app.scroll_up(30);
        assert!(!app.scroll.is_following());
        // A click anywhere BELOW the transcript (status bar, input box) must
        // NOT snap the viewport — that lost the user's reading position.
        handle_mouse(&mut app, mouse(MouseEventKind::Down(MouseButton::Left), 25));
        assert!(
            !app.scroll.is_following(),
            "click on input box must not jump"
        );
        // Exactly the transcript's bottom row (the jump-pill target) follows.
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
        assert!(
            !app.cancel_requested,
            "Esc on permission denies without cancelling the turn"
        );
        assert!(app.front_permission().is_none());
    }
}
