//! Live event loop for the modern TUI.
//!
//! Owns the terminal (alt-screen + raw mode), drives [`App`], and runs
//! turns through [`Session::spawn_turn`] so drawing never blocks on the
//! engine lock.

use std::io::{Stdout, Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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

/// How many recent sessions `/resume` offers.
const SESSION_PICKER_LIMIT: usize = 50;

/// A session read from disk plus the transcript rebuilt from it: the id
/// that was asked for, the restored data, and the display items. Produced
/// on a blocking thread, applied by the event loop.
type LoadedSession = (
    String,
    agent_code_lib::services::session::SessionData,
    Vec<super::app::TranscriptItem>,
);

use agent_code_lib::config::PermissionMode;
use agent_code_lib::query::{QueryEngine, Session, TurnHandle};
use agent_code_lib::services::notifier::{NotificationKind, NotifierService};
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
    let show_thinking_blocks = engine.state().config.ui.show_thinking_blocks;
    let initial_effort = engine.state().config.api.effort.clone();
    let notifier_config = engine.state().config.notifier.clone();

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
    // Constructors install defaults only; the user's file is read once
    // here so tests building Apps stay independent of machine config.
    app.keybindings = std::sync::Arc::new(crate::ui::keybindings::KeybindingRegistry::load());
    // The picker previews by mutating the global theme, so the App has to
    // remember what the user actually configured in order to revert.
    app.theme_name = configured.clone();
    app.inherit_fg = inherit_fg;
    app.show_thinking_blocks = show_thinking_blocks;
    app.effort = initial_effort;
    app.notifier_enabled = notifier_config.enabled;
    // Construction performs no I/O; the first `notify` is what spawns.
    let notifier = NotifierService::new(notifier_config);

    // Restore the terminal even if the draw path panics.
    install_panic_restore_hook();
    let caps = probe_caps();
    app.caps = caps;
    // Only now does the probe result actually reach the terminal: without
    // this the Ctrl+Enter / Shift+Enter disambiguation the composer
    // advertises was detected and then never requested.
    KEYBOARD_ENHANCEMENT_WANTED.store(caps.kitty_keyboard_safe, Ordering::Relaxed);

    let mut terminal = setup_terminal()?;
    let mut term_events = EventStream::new();
    let mut draw = |app: &mut App| draw_frame(&mut terminal, app, caps);
    let result = event_loop(
        &session,
        &mut app,
        eng_tx,
        eng_rx,
        base_permission_mode,
        &notifier,
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

/// The only flag we request. `DISAMBIGUATE_ESCAPE_CODES` is what makes
/// Ctrl+Enter and Shift+Enter distinguishable from a plain Enter — the
/// disambiguation the composer already advertises. The richer report modes
/// change how ordinary printable keys arrive and buy nothing here, so they
/// stay off.
const KEYBOARD_ENHANCEMENT_FLAGS: KeyboardEnhancementFlags =
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;

/// Set once from the capability probe: may we push the flags at all?
static KEYBOARD_ENHANCEMENT_WANTED: AtomicBool = AtomicBool::new(false);
/// Whether the flags are currently on the terminal's stack. Every teardown
/// path — normal exit, the `with_main_screen` detour, and the panic hook —
/// consults this so we pop exactly as many times as we pushed. A leaked
/// keyboard mode makes the user's shell unusable.
static KEYBOARD_ENHANCEMENT_PUSHED: AtomicBool = AtomicBool::new(false);

fn push_keyboard_enhancement(out: &mut impl Write) {
    if !KEYBOARD_ENHANCEMENT_WANTED.load(Ordering::Relaxed)
        || KEYBOARD_ENHANCEMENT_PUSHED.load(Ordering::Relaxed)
    {
        return;
    }
    if execute!(
        out,
        PushKeyboardEnhancementFlags(KEYBOARD_ENHANCEMENT_FLAGS)
    )
    .is_ok()
    {
        KEYBOARD_ENHANCEMENT_PUSHED.store(true, Ordering::Relaxed);
    }
}

fn pop_keyboard_enhancement(out: &mut impl Write) {
    if !KEYBOARD_ENHANCEMENT_PUSHED.swap(false, Ordering::Relaxed) {
        return;
    }
    let _ = execute!(out, PopKeyboardEnhancementFlags);
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
    // Pushed last, so it is popped first on every restore path.
    push_keyboard_enhancement(&mut out);
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
    let mut out = stdout();
    // Reverse order: the keyboard flags went on last, so they come off first.
    pop_keyboard_enhancement(&mut out);
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = disable_raw_mode();
}

fn restore_terminal(terminal: &mut Term) -> anyhow::Result<()> {
    pop_keyboard_enhancement(terminal.backend_mut());
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

/// Temporarily leave alt-screen / raw mode so an interactive slash command
/// (picker, scrollback viewer, `$EDITOR`) can own the real terminal, then
/// re-enter the modern TUI modes.
fn with_main_screen<R>(f: impl FnOnce() -> R) -> R {
    restore_stdout_modes();
    let result = f();
    // Best-effort re-enter; draw path will recover on the next frame.
    let _ = enable_raw_mode();
    let mut out = stdout();
    let _ = execute!(
        out,
        EnterAlternateScreen,
        EnableFocusChange,
        EnableBracketedPaste,
        EnableMouseCapture,
        crossterm::cursor::Hide,
    );
    // `restore_stdout_modes` popped the flags for the interactive command;
    // put them back last so the teardown order stays the same.
    push_keyboard_enhancement(&mut out);
    result
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
    if app.force_full_redraw {
        let _ = terminal.clear();
        app.force_full_redraw = false;
    }
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
    notifier: &NotifierService,
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
    // Background tasks live in the shared `TaskManager`, which emits no
    // events, so the pane has to ask. Polled only while a turn is live or
    // rows already exist, so an idle session still never wakes — and the
    // sync only marks the frame dirty when something actually changed.
    let mut tasks_tick = tokio::time::interval(Duration::from_millis(750));
    tasks_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let task_manager = {
        let engine_arc = session.engine();
        let eng = engine_arc.lock().await;
        eng.state().task_manager.clone()
    };
    // Drill-in output reads run detached (see the select arm) and land
    // back here, so a slow filesystem never blocks the event loop.
    let (task_out_tx, mut task_out_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, Result<String, String>)>();
    // `/resume` session discovery, same shape: the scan runs on a blocking
    // thread and the rows come back here (see the select arms).
    let (session_list_tx, mut session_list_rx) = tokio::sync::mpsc::unbounded_channel::<
        Vec<agent_code_lib::services::session::SessionSummary>,
    >();
    // Loading the *selected* session is the heavier half — a whole
    // transcript read and deserialized — so it is detached too. The
    // payload is boxed because a full conversation is far too big to pass
    // around by value in a select arm.
    #[allow(clippy::type_complexity)]
    let (resume_tx, mut resume_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, Result<Box<LoadedSession>, String>)>();
    // Set while a load is in flight so the arm does not respawn it every
    // pass; `app.pending_resume` stays set until the restore is applied,
    // which is what suppresses queue auto-dispatch in the meantime.
    let mut resume_loading = false;
    let mut pending_restore: Option<Box<LoadedSession>> = None;
    // Seed the pane once so tasks adopted from a previous process show
    // before the first turn arms the periodic poll.
    app.sync_background_tasks(manager_rows(&task_manager).await);

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
        // Not while a resume is outstanding: this would push the new mode
        // onto the conversation being replaced. The choice is not lost —
        // the restore replays it on top of the restored session, since a
        // toggle made after picking is the user's newest instruction.
        if app.mode != last_mode && app.pending_resume.is_none() {
            apply_mode_to_engine(session, app.mode, base_permission_mode);
            last_mode = app.mode;
            app.dirty = true;
        }

        // Apply deferred `/model` (list or set). try_lock so a mid-turn
        // switch does not block the UI; if the turn holds the mutex we
        // retry on the next loop iteration.
        // Everything below that mutates *session* state is held while a
        // resume is outstanding. The load is asynchronous, so without this
        // a `/clear`, `/model`, bridged slash command or `!cmd` submitted
        // during the load would run against the conversation being
        // replaced — reporting success, and then having its effect (and
        // the transcript recording it) wiped by the restore. Each of these
        // handlers is already "apply when possible", so they simply run
        // once the restored session is in place. `/theme` is exempt: it is
        // global UI state, not session state.
        if app.pending_resume.is_none()
            && let Some(action) = app.pending_model.take()
        {
            let engine_arc = session.engine();
            match engine_arc.try_lock() {
                Ok(mut eng) => {
                    let current = eng.state().config.api.model.clone();
                    let base_url = eng.state().config.api.base_url.clone();
                    let mut next_model: Option<String> = None;
                    let mut next_effort: Option<Option<String>> = None;
                    app.apply_model_action(
                        action,
                        &current,
                        &base_url,
                        |name| next_model = Some(name),
                        |effort| next_effort = Some(effort),
                    );
                    if let Some(name) = next_model {
                        eng.state_mut().config.api.model = name;
                    }
                    if let Some(effort) = next_effort {
                        eng.state_mut().config.api.effort = effort;
                    }
                }
                Err(_) => {
                    app.pending_model = Some(action);
                }
            }
        }

        // Mirror a `/theme` selection into the engine config and persist
        // it, so `/config` agrees with the screen and the choice survives
        // a restart. The global theme was already switched by the picker.
        if let Some(theme_id) = app.pending_theme.take() {
            match session.engine().try_lock() {
                Ok(mut eng) => {
                    eng.state_mut().config.ui.theme = theme_id.clone();
                    // Not fatal: the theme is already live either way.
                    if let Err(e) = crate::ui::onboarding::persist_theme(&theme_id) {
                        app.status_message = format!("theme applied, not saved: {e}");
                    }
                }
                Err(_) => app.pending_theme = Some(theme_id),
            }
        }

        // Open a picker whose rows landed while a HITL modal was up.
        app.retry_session_picker();

        // Apply a session loaded off-thread (see the resume select arms):
        // engine state, the visible transcript, *and* every App mirror, so
        // the header, the mode badge and `/cost` describe what was
        // actually restored.
        //
        // Gated on `turn.is_none()`, not just on the engine mutex being
        // free. A finishing turn releases the mutex before the reaper
        // below takes its handle, drains its trailing events and flushes
        // its buffered text — all of which would land in the transcript
        // we just replaced, against the conversation we just discarded.
        // Waiting for the reap is the only ordering that cannot interleave.
        if turn.is_none()
            && let Some(loaded) = pending_restore.take()
        {
            let (id, data, items) = *loaded;
            // Superseded by a newer selection made while this one was
            // still loading: drop it, the load arm fetches the new one.
            if app.pending_resume.as_deref() == Some(id.as_str()) {
                match session.engine().try_lock() {
                    Ok(mut eng) => {
                        // A mode toggled after the session was picked but
                        // before it loaded is the user's newest explicit
                        // instruction, so it is replayed on top of the
                        // restored mode rather than silently reverted.
                        let user_mode = (app.mode != last_mode).then_some(app.mode);
                        let turns = data.turn_count;
                        {
                            let st = eng.state_mut();
                            // Identity moves with the conversation. Left alone,
                            // the engine keeps saving this restored transcript
                            // over the *previous* session's file (and reports
                            // the old id to hooks and telemetry) while the
                            // header claims the restored one.
                            st.session_id = id.clone();
                            st.messages = data.messages;
                            st.turn_count = data.turn_count;
                            st.total_cost_usd = data.total_cost_usd;
                            // Replace the whole Usage, not two fields of
                            // it: the cache-token counters are not saved
                            // in a session, so assigning piecemeal left
                            // the discarded conversation's cache figures
                            // adding into the restored session's `/cost`.
                            st.total_usage = agent_code_lib::llm::message::Usage {
                                input_tokens: data.total_input_tokens,
                                output_tokens: data.total_output_tokens,
                                ..Default::default()
                            };
                            if !data.model.is_empty() {
                                st.config.api.model = data.model.clone();
                            }
                        }
                        // "Allow for this session" grants belong to the
                        // conversation they were given in. The engine is
                        // reused across the swap, so without this the
                        // resumed session inherits approvals the user
                        // never gave it and the executor skips the ask.
                        // Permission grants, `/add-dir` paths, the prompt
                        // cache, the memory-extraction cursor, the denial
                        // tracker and the rest of the conversation-scoped
                        // state the session file does not carry. One call,
                        // so a swap cannot forget a field.
                        eng.reset_for_session_swap().await;
                        // Built after the reset so `effort` is the value
                        // the engine actually ends up with, not the one
                        // the discarded conversation had chosen.
                        let restored = super::session_picker::RestoredState {
                            id: id.clone(),
                            model: data.model.clone(),
                            turn_count: data.turn_count,
                            tokens_in: data.total_input_tokens,
                            tokens_out: data.total_output_tokens,
                            cost_usd: data.total_cost_usd,
                            plan_mode: data.plan_mode,
                            effort: eng.state().config.api.effort.clone(),
                        };
                        app.restore_transcript(items, &id, turns);
                        let stored_mode = app.adopt_restored_session(&restored);
                        let mode = user_mode.unwrap_or(stored_mode);
                        app.mode = mode;
                        // Keep the loop's mode tracker in step, or the
                        // `app.mode != last_mode` gate at the top would
                        // re-apply (or, worse, silently skip) the mode we
                        // are about to install.
                        last_mode = mode;
                        let plan = mode == super::mode::SessionMode::Plan;
                        eng.state_mut().plan_mode = plan;
                        // Live handles, not just the config copy: the
                        // permission-checker default has to move with the
                        // restored plan flag, or a resumed plan session
                        // answers permission checks as the old one did.
                        let hint = mode.permission_hint().unwrap_or(base_permission_mode);
                        session.apply_live_mode(plan, hint);
                        eng.state_mut().config.permissions.default_mode = hint;
                        app.pending_resume = None;
                        // Restart the loop so the handlers *above* this one
                        // get their turn now that the resume gate is open.
                        // They have already been passed this iteration, and
                        // `select!` below has no readiness condition for a
                        // pending `/model`, so it would sit unapplied until
                        // some unrelated key or engine event arrived.
                        continue;
                    }
                    // A turn holds the mutex; retry next iteration rather
                    // than half-applying the resume.
                    Err(_) => pending_restore = Some(Box::new((id, data, items))),
                }
            }
        }

        // Apply a deferred `/clear` to the engine conversation (classic
        // parity). try_lock like `/model`: if a turn holds the mutex we
        // retry next iteration (the live atomic state is unaffected).
        if app.pending_clear
            && app.pending_resume.is_none()
            && let Ok(mut eng) = session.engine().try_lock()
        {
            eng.state_mut().messages.clear();
            app.pending_clear = false;
            // `submit` already cleared the view, but a `/clear` held back
            // for a resume lands *after* the restore repainted the
            // screen. Without this the restored history stays on display
            // in front of a conversation the engine just emptied.
            app.clear_transcript_view();
            app.status_message = "context cleared".into();
            app.dirty = true;
        }

        // Full classic slash-command bridge (stdout captured → transcript).
        // Run off the async worker via `block_in_place`: many slash arms call
        // `Handle::block_on` / spawn+join, which panic if invoked directly on
        // a Tokio worker without parking it first.
        if app.pending_resume.is_none()
            && let Some(slash) = app.pending_slash.take()
        {
            match session.engine().try_lock() {
                Ok(mut eng) => {
                    let interactive = crate::commands::is_interactive_slash(&slash);
                    let real_tty = crate::commands::needs_real_tty_stdout(&slash);
                    // Always park the worker: slash arms may call
                    // `Handle::block_on` (e.g. cwd hooks) even after leaving
                    // the alt-screen.
                    let (result, captured) = tokio::task::block_in_place(|| {
                        if interactive {
                            // Leave alt-screen/raw mode so pickers, scrollback
                            // viewer, $EDITOR, and y/N prompts own the real
                            // terminal.
                            with_main_screen(|| {
                                if real_tty {
                                    // `$EDITOR` needs isatty; theme/model/
                                    // session pickers bail unless
                                    // stdout().is_terminal(). Never redirect
                                    // fd 1 to a pipe for these.
                                    let r = crate::commands::execute(&slash, &mut eng);
                                    (r, String::new())
                                } else {
                                    // y/N prompts: tee so the Proceed?/Cancel
                                    // lines still reach the modern transcript.
                                    crate::stdout_capture::capture_stdout_tee(|| {
                                        crate::commands::execute(&slash, &mut eng)
                                    })
                                }
                            })
                        } else {
                            crate::stdout_capture::capture_stdout(|| {
                                crate::commands::execute(&slash, &mut eng)
                            })
                        }
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
                    // Keep TUI header/path and `!` shell in sync with engine
                    // after `/cd` (and any other cwd-changing command).
                    app.cwd = eng.state().cwd.clone();
                    // Same for `/color`: the arm re-inits the palette and
                    // updates the config, but only the app knows what the
                    // theme picker should treat as current.
                    let configured = eng.state().config.ui.theme.clone();
                    app.sync_theme_from_config(&configured);
                    if interactive {
                        app.force_full_redraw = true;
                    }
                    app.dirty = true;
                    drop(eng);
                    // The command may have mutated the TaskManager
                    // (`/tasks kill`, `/tasks clear`): refresh the pane
                    // now, because the gated poll can be parked when
                    // every listed task is already terminal.
                    let rows = manager_rows(&task_manager).await;
                    app.sync_background_tasks(rows);
                }
                Err(_) => {
                    // Turn holds the lock — retry next loop.
                    app.pending_slash = Some(slash);
                }
            }
        }

        // `!cmd` shell passthrough (classic parity).
        if app.pending_resume.is_none()
            && let Some(cmd) = app.pending_shell.take()
        {
            match session.engine().try_lock() {
                Ok(mut eng) => {
                    use agent_code_lib::services::shell_passthrough;
                    use std::sync::{Arc, Mutex};
                    // Prefer engine cwd (source of truth after `/cd`).
                    app.cwd = eng.state().cwd.clone();
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
        //
        // Not while a resume is outstanding. The composer stays live
        // while the selected session loads, so a prompt submitted in that
        // window would otherwise start a turn — tool side effects and all
        // — against the conversation being thrown away, and the restore
        // would then wipe the transcript that recorded it. The prompt is
        // handed back to the composer when the restore lands.
        if turn.is_none()
            && app.pending_resume.is_none()
            && let Some(prompt) = app.pending_submit.take()
        {
            let sink = ChannelSink::new(eng_tx.clone());
            match session.spawn_turn(prompt.clone(), sink).await {
                Ok(handle) => {
                    turn = Some(handle);
                    app.mark_turn_started();
                }
                Err(e) => {
                    // Should be rare: TUI serializes turns. Put the prompt
                    // back so the next idle loop can retry.
                    app.pending_submit = Some(prompt);
                    app.status_message = format!("turn busy: {e}");
                    app.dirty = true;
                }
            }
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
                // Final manager sync. A task registered moments before
                // the turn ended (or a cancelled turn) could otherwise
                // leave `app.tasks` empty with `live` false — and the
                // guarded tick below would never poll again, hiding the
                // task until some later turn.
                let rows = manager_rows(&task_manager).await;
                app.sync_background_tasks(rows);
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
                // Start a pending turn NOW (auto-queue head or interject),
                // or apply a resume that was waiting for this turn to be
                // reaped. Both checks live at the top of the loop; falling
                // through to `select!` would park until an unrelated event
                // — leaving the user staring at "resuming …" until they
                // pressed a key.
                if app.pending_submit.is_some() || app.pending_resume.is_some() {
                    continue;
                }
            }
        }

        // Hand any queued desktop notifications to the service. `notify` is
        // fire-and-forget, but its platform backends probe for the tool
        // (`which`, `where.exe`) and the macOS focus check runs
        // `osascript` — both are synchronous fork+exec. Off the loop
        // thread they go, so a slow or missing notifier can never add a
        // frame of latency to the UI.
        for req in app.take_notifications() {
            let svc = notifier.clone();
            tokio::task::spawn_blocking(move || match req.duration_secs {
                Some(secs) if req.kind == NotificationKind::TaskComplete => {
                    svc.notify_task_complete(&req.title, &req.body, secs);
                }
                _ => svc.notify(req.kind, &req.title, &req.body),
            });
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
            // Keep OSC window title in sync with phase on every paint so
            // idle after a turn/HITL resets the spinner / "action required"
            // title (anim_tick alone stops once needs_anim_tick is false).
            update_terminal_title(app);
            // Arm HITL answer keys only after the user has seen a paint of
            // the modal (+ grace). Stops mid-type keystrokes from auto-allow.
            if app.phase == super::app::Phase::Permission {
                app.note_hitl_drawn();
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
                    Some(Ok(Event::FocusGained)) => {
                        app.terminal_focused = true;
                        app.dirty = true;
                    }
                    Some(Ok(Event::FocusLost)) => {
                        app.terminal_focused = false;
                        app.dirty = true;
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        app.dirty = true;
                    }
                    // Stream closed or errored: stop the UI cleanly.
                    Some(Err(_)) | None => {
                        app.should_quit = true;
                    }
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
            // Drill-in: kick the read off to its own task so a large or
            // slow output file never stalls input handling; the result
            // comes back through the channel arm below.
            _ = std::future::ready(()), if app.pending_task_output.is_some() => {
                if let Some(id) = app.pending_task_output.take() {
                    let tm = task_manager.clone();
                    let tx = task_out_tx.clone();
                    tokio::spawn(async move {
                        // Bounded: the card shows a tail anyway, so never
                        // materialize an arbitrarily large output file.
                        let out = tm.read_output_tail(&id, 256 * 1024).await;
                        let _ = tx.send((id, out));
                    });
                }
            }
            Some((id, out)) = task_out_rx.recv() => {
                app.show_task_output(&id, out);
            }
            // `/resume`: enumerating sessions stats and parses every file
            // in the sessions directory. Done inline in the key handler it
            // froze input and repaint for as long as that took, so it goes
            // to a blocking thread and returns through the arm below.
            _ = std::future::ready(()), if app.pending_session_list => {
                app.pending_session_list = false;
                let tx = session_list_tx.clone();
                tokio::task::spawn_blocking(move || {
                    // Summary-only listing: it is index-cached and skips
                    // deserializing every transcript, which is the entire
                    // cost of the full read for data the picker never shows.
                    let rows = agent_code_lib::services::session::list_session_summaries(
                        SESSION_PICKER_LIMIT,
                    );
                    let _ = tx.send(rows);
                });
            }
            Some(rows) = session_list_rx.recv() => {
                app.show_session_picker(rows);
            }
            // Read and rebuild the selected session off-thread: a long
            // conversation is megabytes of JSON, and deserializing it on
            // this thread froze input and repaint for its duration. Only
            // the (cheap) apply stays on the loop, where it can be
            // ordered against turn teardown.
            _ = std::future::ready(()), if app.pending_resume.is_some()
                && !resume_loading
                && pending_restore.is_none()
                && turn.is_none() => {
                if let Some(id) = app.pending_resume.clone() {
                    resume_loading = true;
                    let tx = resume_tx.clone();
                    // Read the display setting here: the blocking task
                    // cannot touch `app`.
                    let show_thinking = app.show_thinking_blocks;
                    tokio::task::spawn_blocking(move || {
                        let loaded = agent_code_lib::services::session::load_session(&id)
                            .map(|data| {
                                let items = super::session_picker::transcript_from_messages(
                                    &data.messages,
                                    show_thinking,
                                );
                                Box::new((id.clone(), data, items))
                            });
                        let _ = tx.send((id, loaded));
                    });
                }
            }
            Some((id, loaded)) = resume_rx.recv() => {
                resume_loading = false;
                // A second `/resume` while this one was loading wins; drop
                // the stale result rather than restoring a session the
                // user has already moved on from.
                if app.pending_resume.as_deref() == Some(id.as_str()) {
                    match loaded {
                        Ok(l) => pending_restore = Some(l),
                        Err(e) => {
                            app.status_message.clear();
                            app.transcript.push(super::app::TranscriptItem::Error(
                                format!("could not resume {id}: {e}"),
                            ));
                            // Cancel *before* clearing `pending_resume`:
                            // the work was deferred for a session that
                            // never arrived, and releasing it would run it
                            // against the conversation the user was trying
                            // to leave.
                            app.cancel_deferred_resume_work();
                            app.pending_resume = None;
                            app.dirty = true;
                        }
                    }
                }
            }
            // Background-task rows (`&` shell jobs, workflows, monitors).
            // Gated on work that can still change: polling while any
            // rows exist at all would tick forever once a subagent row
            // (which persists for the session) appears.
            _ = tasks_tick.tick(), if live || app.has_live_manager_tasks() => {
                let rows = manager_rows(&task_manager).await;
                app.sync_background_tasks(rows);
            }
            // Micro-animations: spinner, action-required blink, toast decay.
            _ = anim_tick.tick(), if live || app.needs_anim_tick() => {
                if app.needs_anim_tick() {
                    app.tick();
                    update_terminal_title(app);
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

/// Snapshot the shared `TaskManager` as tasks-pane rows.
async fn manager_rows(
    tm: &std::sync::Arc<agent_code_lib::services::background::TaskManager>,
) -> Vec<super::tasks::ManagerRow> {
    use agent_code_lib::services::background::{TaskKind, TaskPayload, TaskStatus};
    let mut rows: Vec<super::tasks::ManagerRow> = tm
        .list()
        .await
        .into_iter()
        .map(|t| {
            let state = match &t.status {
                TaskStatus::Running => "working",
                TaskStatus::Completed => "done",
                TaskStatus::Failed(_) | TaskStatus::Killed => "failed",
            };
            // LocalAgent runs reconcile with their event-driven pane row
            // via the payload's subagent id. A legacy record without one
            // still lists under the agents group, keyed by its task id.
            let subagent_id = (t.kind == TaskKind::LocalAgent).then(|| match &t.payload {
                Some(TaskPayload::LocalAgent {
                    subagent_id: Some(sid),
                    ..
                }) => sid.clone(),
                _ => t.id.to_string(),
            });
            super::tasks::ManagerRow {
                id: t.id.to_string(),
                state: state.to_string(),
                headline: t.description.clone(),
                subagent_id,
            }
        })
        .collect();
    // The manager's map iterates in arbitrary order; sort by the id's
    // numeric sequence so reconciliation (which record folds into an
    // event row) is stable and chronological. The sequence is unpadded
    // ("a9", "a10"), so a lexical sort would misorder digit boundaries.
    fn task_seq(id: &str) -> u64 {
        id.trim_start_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .unwrap_or(u64::MAX)
    }
    rows.sort_by(|a, b| {
        task_seq(&a.id)
            .cmp(&task_seq(&b.id))
            .then_with(|| a.id.cmp(&b.id))
    });
    rows
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

/// True for Ctrl+C / Cmd+C (Super), or raw ETX — but **not** Ctrl+Shift+C.
///
/// Ctrl+Shift+C is reserved for "copy selection / last reply". Bare Ctrl+C
/// still cancels. Some terminals set extra modifiers; we still treat
/// CONTROL without SHIFT as cancel (SHIFT alone no longer forces cancel).
///
/// **Not Esc.** Esc is navigate / dismiss / clear only — never cancels a
/// turn (world-class agent-screen contract; see ACCEPTANCE + KEYBINDINGS).
fn is_cancel_chord(key: &KeyEvent) -> bool {
    match key.code {
        // Raw ETX (0x03) — some paths deliver the byte without CONTROL.
        KeyCode::Char('\u{3}') => true,
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER);
            // Leave Ctrl+Shift+C for the copy shortcut.
            ctrl && !key.modifiers.contains(KeyModifiers::SHIFT)
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

    // HITL always wins: dismiss help + palette so y/a/n reach the modal.
    if app.phase == super::app::Phase::Permission {
        if app.show_shortcuts {
            app.show_shortcuts = false;
            app.dirty = true;
        }
        app.close_command_palette();
        app.close_model_picker();
        // Reverts the live preview rather than leaving a browsed theme on.
        app.theme_picker_cancel();
        // Restores the reader's place; a hidden search bar must not swallow
        // the modal's answer keys.
        app.cancel_search();
    }

    // Shortcuts overlay steals keys only when no HITL modal is up.
    // Ctrl+C / Cmd+C always fall through so a live turn can still be cancelled
    // while help is open (the overlay itself documents that chord).
    if app.show_shortcuts && app.phase != super::app::Phase::Permission && !is_cancel_chord(&key) {
        if is_esc(&key)
            || matches!(
                key.code,
                KeyCode::Char('.') | KeyCode::Char('x') | KeyCode::Char('X')
            ) && key.modifiers.contains(KeyModifiers::CONTROL)
            || matches!(key.code, KeyCode::Char('?'))
        {
            app.show_shortcuts = false;
            app.dirty = true;
        }
        return;
    }

    // Search captures input when open (and no HITL modal is up).
    if app.search_open() {
        handle_search_key(app, key);
        return;
    }

    // Session picker captures input when open.
    if app.session_picker_open() {
        handle_session_picker_key(app, key);
        return;
    }

    // Model picker captures input when open (and no HITL modal is up).
    if app.model_picker_open() {
        handle_model_picker_key(app, key);
        return;
    }

    // Theme picker captures input when open (and no HITL modal is up).
    if app.theme_picker_open() {
        handle_theme_picker_key(app, key);
        return;
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
        // Scrolling the permission input view is read-only, so it works
        // during the answer-grace window too — the whole point is to
        // inspect the full input BEFORE answering.
        if let Some(super::app::Modal::Permission(p)) = app.front_modal() {
            let total = p
                .input_preview
                .as_deref()
                .map(|s| s.lines().count())
                .unwrap_or(0);
            let step = match key.code {
                KeyCode::Up => Some(-1isize),
                KeyCode::Down => Some(1),
                KeyCode::PageUp => Some(-10),
                KeyCode::PageDown => Some(10),
                _ => None,
            };
            if let Some(step) = step {
                // Loose upper clamp (renderer clamps exactly against its
                // viewport); keeps the offset from running away.
                let max = total.saturating_sub(1);
                app.perm_scroll = app.perm_scroll.saturating_add_signed(step).min(max);
                app.dirty = true;
                return;
            }
        }

        // Answer keys only after first paint + grace (#431). Esc/Ctrl+C above
        // stay live so the user can always dismiss or cancel.
        if !app.hitl_answers_ready() {
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
        if app.selection.is_some() {
            app.clear_selection();
            return;
        }
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

    // A user chord from `keybindings.json` wins over the built-in
    // dispatch below, which is the point of customization. Reserved
    // chords (Ctrl+C, Esc) are filtered out inside `action_for`, and
    // they are handled above this line anyway.
    if apply_user_keybinding(app, &key) {
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
        // Force full redraw (Ctrl+L) — documented in docs/tui/KEYBINDINGS.md.
        (m, KeyCode::Char('l') | KeyCode::Char('L')) if m.contains(KeyModifiers::CONTROL) => {
            app.request_full_redraw();
        }
        // Command palette (Ctrl+P / ?).
        // Ctrl+F opens in-transcript search.
        (m, KeyCode::Char('f') | KeyCode::Char('F')) if m.contains(KeyModifiers::CONTROL) => {
            app.open_search();
        }
        (m, KeyCode::Char('p') | KeyCode::Char('P')) if m.contains(KeyModifiers::CONTROL) => {
            app.open_command_palette();
        }
        (_, KeyCode::Char('?')) if app.input.is_empty() => {
            app.open_command_palette();
        }
        // Keyboard shortcuts help (Ctrl+. / Ctrl+X).
        (m, KeyCode::Char('.') | KeyCode::Char('x') | KeyCode::Char('X'))
            if m.contains(KeyModifiers::CONTROL) =>
        {
            app.toggle_shortcuts();
        }
        // Copy selection or last assistant (Ctrl+Shift+C).
        (m, KeyCode::Char('c') | KeyCode::Char('C'))
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
            app.copy_selection_or_last();
        }
        // Queue pane toggle (Ctrl+; / Ctrl+').
        (m, KeyCode::Char(';') | KeyCode::Char('\'')) if m.contains(KeyModifiers::CONTROL) => {
            app.toggle_queue_pane();
        }
        // When the tasks pane is open (and the queue pane is not, which
        // owns the same keys), plain arrows move the selection and plain
        // Enter opens the selected task's output. Modified presses fall
        // through so global bindings (Ctrl+Enter interject, Alt+Enter
        // newline) stay reachable while the pane is open.
        (m, KeyCode::Up)
            if m.is_empty()
                && app.tasks_visible()
                && !app.show_queue_pane
                && app.input.is_empty() =>
        {
            app.tasks_select(-1);
        }
        (m, KeyCode::Down)
            if m.is_empty()
                && app.tasks_visible()
                && !app.show_queue_pane
                && app.input.is_empty() =>
        {
            app.tasks_select(1);
        }
        // Retained prompts win the empty Enter: after an aborted turn the
        // UI promises "press Enter to send", so drill-in only claims the
        // key when no queued prompt is waiting for dispatch.
        (m, KeyCode::Enter)
            if m.is_empty()
                && app.tasks_visible()
                && !app.show_queue_pane
                && app.queue.is_empty()
                && app.input.is_empty() =>
        {
            app.drill_into_selected_task();
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
        // Block copy: y = body (or selection), Y = metadata (only when empty composer).
        (_, KeyCode::Char('y')) if app.input.is_empty() => {
            if app.selection.is_some() {
                app.copy_selection_or_last();
            } else if app.selected_item.is_some() {
                app.copy_selected_content();
            }
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
        // Ctrl+M: Grok-style — model picker when composer empty / block
        // selected (scrollback); multiline toggle when drafting.
        (m, KeyCode::Char('m') | KeyCode::Char('M')) if m.contains(KeyModifiers::CONTROL) => {
            if app.input.is_empty() || app.selected_item.is_some() {
                app.request_model_picker();
            } else {
                app.toggle_multiline_mode();
            }
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
        // Tab completes `@path` mentions and slash commands.
        (_, KeyCode::Tab) if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.complete_tab();
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
    // The search bar owns typed input while open, so it owns pasted input
    // too — otherwise the paste silently edits the composer behind it.
    if app.search_open() {
        app.search_insert_str(text);
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

fn handle_search_key(app: &mut App, key: KeyEvent) {
    // Esc restores the reader's position; Ctrl+C is the cancel chord
    // everywhere else, so it behaves like Esc here.
    if is_esc(&key) || is_cancel_chord(&key) {
        app.cancel_search();
        return;
    }
    match key.code {
        // Enter accepts: close the bar and stay on the match, so the
        // composer is usable again at the spot that was searched for.
        KeyCode::Enter => app.close_search(),
        KeyCode::Down => app.search_next(),
        KeyCode::Up => app.search_prev(),
        // Ctrl+N / Ctrl+P mirror ↓/↑. Bare letters stay ordinary
        // characters — `N` must be typable in a smart-case query
        // like `API_NAME`.
        KeyCode::Char('n') | KeyCode::Char('N')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.search_next();
        }
        KeyCode::Char('p') | KeyCode::Char('P')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.search_prev();
        }
        KeyCode::Backspace => app.search_backspace(),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.search_insert_char(c);
        }
        _ => {}
    }
}

fn handle_theme_picker_key(app: &mut App, key: KeyEvent) {
    // Esc / Ctrl+C revert to the theme that was active on open — a
    // browse must never be able to leave a theme behind.
    if is_esc(&key) || is_cancel_chord(&key) {
        app.theme_picker_cancel();
        return;
    }
    match key.code {
        KeyCode::Up => app.theme_picker_move(-1),
        KeyCode::Down => app.theme_picker_move(1),
        KeyCode::Enter => app.theme_picker_accept(),
        KeyCode::Backspace => app.theme_picker_backspace(),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.theme_picker_insert_char(c);
        }
        _ => {}
    }
}

/// Dispatch a user-defined keybinding. Returns true when one fired.
///
/// Only chords the user actually wrote are dispatched: the built-in
/// defaults in the registry describe chords the hardcoded handler
/// already owns, so routing those through here would run them twice.
fn apply_user_keybinding(app: &mut App, key: &KeyEvent) -> bool {
    use crate::ui::keybindings::{KeyAction, chord_string};

    // A held key emits Repeat events; dispatching a binding on each one
    // would queue a duplicate turn per repeat from a single hold. Only
    // the initial press fires — built-in handlers below keep their own
    // repeat behavior (held Ctrl+C must still count).
    if key.kind != KeyEventKind::Press {
        return false;
    }
    let Some(chord) = chord_string(key.code, key.modifiers) else {
        return false;
    };
    if !app.keybindings.is_user_defined(&chord) {
        return false;
    }
    let registry = app.keybindings.clone();
    let Some(action) = registry.action_for(key.code, key.modifiers) else {
        return false;
    };
    // Bound actions submit their own text, never the composer's draft —
    // stash the draft so opening the tasks pane (or any binding) does not
    // silently discard what the user was writing.
    let draft = std::mem::take(&mut app.input);
    let draft_cursor = app.cursor;
    app.cursor = 0;
    match action {
        // Both go through the normal submit path so slash dispatch,
        // queueing and mid-turn behaviour are identical to typing it.
        KeyAction::Command { command } => {
            app.input = format!("/{}", command.trim_start_matches('/'));
            app.cursor = app.input.len();
            app.submit();
        }
        KeyAction::Prompt { prompt } => {
            app.input = prompt.clone();
            app.cursor = app.input.len();
            app.submit();
        }
        KeyAction::Toggle { setting } => {
            // Toggles map onto the slash commands that own each setting
            // rather than reaching into state directly.
            let cmd = match setting.as_str() {
                "tasks" => Some("/tasks"),
                "queue" => Some("/queue"),
                "minimal" => Some("/minimal"),
                "fullscreen" => Some("/fullscreen"),
                _ => None,
            };
            match cmd {
                Some(c) => {
                    app.input = c.to_string();
                    app.cursor = app.input.len();
                    app.submit();
                }
                None => {
                    app.status_message = format!("keybinding: unknown toggle `{setting}`");
                    app.dirty = true;
                }
            }
        }
    }
    app.input = draft;
    app.cursor = draft_cursor;
    app.dirty = true;
    true
}

fn handle_session_picker_key(app: &mut App, key: KeyEvent) {
    if is_esc(&key) || is_cancel_chord(&key) {
        app.close_session_picker();
        return;
    }
    match key.code {
        KeyCode::Up => app.session_picker_move(-1),
        KeyCode::Down => app.session_picker_move(1),
        KeyCode::Enter => app.session_picker_accept(),
        KeyCode::Backspace => app.session_picker_backspace(),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.session_picker_insert_char(c);
        }
        _ => {}
    }
}

fn handle_model_picker_key(app: &mut App, key: KeyEvent) {
    // Ctrl+M toggles closed; Esc / Ctrl+C dismiss.
    if matches!(key.code, KeyCode::Char('m') | KeyCode::Char('M'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        app.close_model_picker();
        return;
    }
    if is_esc(&key) || is_cancel_chord(&key) {
        app.close_model_picker();
        return;
    }
    match key.code {
        KeyCode::Up => app.model_picker_move(-1),
        KeyCode::Down => app.model_picker_move(1),
        KeyCode::Enter => app.model_picker_accept(),
        KeyCode::Tab => app.model_picker_enter_effort(),
        KeyCode::Backspace => app.model_picker_backspace(),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.model_picker_insert_char(c);
        }
        _ => {}
    }
}

/// Shift/Alt-modified clicks leave native terminal selection alone when
/// we cannot map the row into the transcript.
fn handle_mouse(app: &mut App, m: MouseEvent) {
    // Focus events may arrive as special kinds on some backends; ignore here.
    match m.kind {
        MouseEventKind::ScrollUp => {
            app.scroll_up(3);
            app.dirty = true;
        }
        MouseEventKind::ScrollDown => {
            app.scroll_down(3);
            app.dirty = true;
        }
        // Jump pill: bottom transcript row → Follow.
        MouseEventKind::Down(MouseButton::Left)
            if !app.scroll.is_following()
                && app.transcript_bottom_row != 0
                && m.row == app.transcript_bottom_row =>
        {
            app.scroll_to_bottom();
            app.clear_selection();
        }
        MouseEventKind::Down(MouseButton::Left) if m.modifiers.is_empty() => {
            if let Some(abs) = mouse_abs_line(app, m.row) {
                app.selection = Some(super::app::TextSelection {
                    start_line: abs,
                    end_line: abs,
                });
                app.dirty = true;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(abs) = mouse_abs_line(app, m.row) {
                if let Some(sel) = app.selection.as_mut() {
                    sel.end_line = abs;
                } else {
                    app.selection = Some(super::app::TextSelection {
                        start_line: abs,
                        end_line: abs,
                    });
                }
                app.dirty = true;
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // Keep selection for y / Ctrl+Shift+C; toast if non-empty.
            if let Some(sel) = app.selection
                && sel.start_line != sel.end_line
            {
                app.push_toast("selection ready · Ctrl+Shift+C or y to copy");
            }
        }
        // Middle-click: no OS PRIMARY read without extra deps; hint paste path.
        MouseEventKind::Down(MouseButton::Middle) if app.phase != super::app::Phase::Permission => {
            app.push_toast("use Ctrl+Shift+V / terminal paste for clipboard");
        }
        _ => {}
    }
}

/// Map a screen row to an absolute layout line in the transcript viewport.
fn mouse_abs_line(app: &App, row: u16) -> Option<usize> {
    if app.viewport_h == 0 || app.transcript_bottom_row == 0 {
        return None;
    }
    let bottom = app.transcript_bottom_row;
    let top_row = bottom.saturating_sub(app.viewport_h as u16 - 1);
    if row < top_row || row > bottom {
        return None;
    }
    let row_in_view = (row - top_row) as usize;
    let total = app.layout.total_lines();
    let top = app.scroll.top(total, app.viewport_h);
    app.layout.abs_line_at(top, row_in_view)
}

/// Strip C0/C1 control bytes (and DEL) so untrusted title components
/// (e.g. `[api].model` from project config) cannot break out of OSC 0
/// via embedded BEL/ESC sequences.
fn sanitize_osc_title(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let u = *c as u32;
            // Keep printable ASCII + all non-control Unicode; drop C0, DEL, C1.
            !matches!(u, 0x00..=0x1F | 0x7F | 0x80..=0x9F)
        })
        .collect()
}

/// OSC 0 window title: braille spinner + model when live; calm idle title.
fn update_terminal_title(app: &App) {
    use std::io::Write;
    let model = sanitize_osc_title(&app.model);
    let title = match app.phase {
        super::app::Phase::Streaming => {
            format!(
                "{} agent · {} · {}",
                super::anim::spinner_glyph(app.tick),
                model,
                app.waiting_on.label_with_elapsed(app.thinking_started_at)
            )
        }
        super::app::Phase::Permission => {
            if super::anim::blink_visible(app.tick, app.terminal_focused) {
                format!("⚠ action required · {model}")
            } else {
                format!("agent · {model}")
            }
        }
        _ => format!("agent · {model}"),
    };
    let title = sanitize_osc_title(&title);
    // OSC 0 ; title BEL
    let seq = format!("\x1b]0;{title}\x07");
    let _ = std::io::stdout().write_all(seq.as_bytes());
    let _ = std::io::stdout().flush();
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

    /// The tasks-pane keys claim only unmodified presses: Ctrl+Enter must
    /// still reach the global interject binding, not open task output.
    #[test]
    fn modified_enter_is_not_captured_by_the_tasks_pane() {
        let mut app = App::new("m", "/tmp", "s");
        crate::ui::modern::tasks::upsert(&mut app.tasks, "a1", "working", "explore");
        app.show_tasks = true;
        assert!(app.tasks_visible());

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );
        assert!(
            app.pending_task_output.is_none() && !app.status_message.contains("output"),
            "Ctrl+Enter was swallowed by the pane"
        );

        // Plain Enter still drives the pane (subagent row → explanation).
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(app.status_message.contains("no separate output"));
    }

    /// After an aborted turn the UI says "queued prompts kept — press
    /// Enter to send"; a visible tasks pane must not swallow that Enter.
    #[test]
    fn queued_prompts_take_enter_before_task_drill_in() {
        let mut app = App::new("m", "/tmp", "s");
        crate::ui::modern::tasks::upsert(&mut app.tasks, "a1", "working", "explore");
        app.show_tasks = true;
        app.queue.push_back("retained prompt".into());

        handle_key(&mut app, key(KeyCode::Enter));
        assert!(
            app.pending_task_output.is_none() && !app.status_message.contains("output"),
            "drill-in stole the queue-dispatch Enter"
        );
        assert!(
            app.queue.is_empty() && app.pending_submit.is_some(),
            "queued prompt was not dispatched"
        );
    }

    /// LocalAgent manager records must carry their stream subagent id so
    /// the pane folds them into the event-driven row; every other kind
    /// maps to a plain background row.
    #[tokio::test]
    async fn manager_rows_link_local_agent_records_to_their_subagent_id() {
        use agent_code_lib::services::background::{TaskKind, TaskManager, TaskPayload};
        let tm = std::sync::Arc::new(TaskManager::new());
        tm.register(
            "scan crates",
            TaskKind::LocalAgent,
            TaskPayload::LocalAgent {
                subagent_kind: Some("explore".into()),
                prompt: "scan".into(),
                parent_session: None,
                subagent_id: Some("explore-1".into()),
            },
        )
        .await;
        tm.register(
            "cargo build",
            TaskKind::LocalShell,
            TaskPayload::LocalShell {
                command: "cargo build".into(),
                cwd: std::path::PathBuf::from("/tmp"),
            },
        )
        .await;

        let rows = manager_rows(&tm).await;
        assert_eq!(rows.len(), 2);
        let agent = rows.iter().find(|r| r.headline == "scan crates").unwrap();
        assert_eq!(agent.subagent_id.as_deref(), Some("explore-1"));
        let shell = rows.iter().find(|r| r.headline == "cargo build").unwrap();
        assert_eq!(shell.subagent_id, None);
        assert_eq!(shell.state, "working");
    }

    #[test]
    fn sanitize_osc_title_strips_control_breakouts() {
        // Malicious model name trying to terminate OSC early and inject CSI.
        let dirty = "evil\x07\x1b[31mred\x1b[0m";
        let clean = sanitize_osc_title(dirty);
        assert!(!clean.contains('\x07'));
        assert!(!clean.contains('\x1b'));
        assert_eq!(clean, "evil[31mred[0m");
        // Normal model names and unicode remain.
        assert_eq!(sanitize_osc_title("grok-4 · β"), "grok-4 · β");
    }

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

    /// Build an App whose registry has one user binding.
    fn app_with_binding(chord: &str, action: crate::ui::keybindings::KeyAction) -> App {
        use crate::ui::keybindings::{Keybinding, KeybindingRegistry};
        let mut app = App::new("m", "/tmp", "s");
        app.keybindings =
            std::sync::Arc::new(KeybindingRegistry::from_user_bindings(vec![Keybinding {
                key: chord.to_string(),
                action,
                description: None,
            }]));
        app
    }

    /// The registry was loaded and listed by `/keybindings`, but nothing
    /// ever consulted it on a keypress — a user could see their binding
    /// listed and have it do nothing.
    #[test]
    fn a_user_bound_chord_runs_its_command() {
        use crate::ui::keybindings::KeyAction;
        let mut app = app_with_binding(
            "ctrl+k",
            KeyAction::Command {
                command: "tasks".into(),
            },
        );
        // Assert on the effect of `/tasks`, not on the composer being
        // empty — the composer is empty whether or not the binding fires,
        // which would make this test pass for the wrong reason.
        let before = app.show_tasks;
        handle_key(&mut app, ctrl('k'));
        assert_ne!(
            app.show_tasks, before,
            "the bound command did not reach slash dispatch"
        );
        assert!(app.input.is_empty(), "the chord left text in the composer");
    }

    #[test]
    fn a_user_bound_chord_submits_a_prompt() {
        use crate::ui::keybindings::KeyAction;
        let mut app = app_with_binding(
            "alt+r",
            KeyAction::Prompt {
                prompt: "run the tests".into(),
            },
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
        );
        assert_eq!(
            app.pending_submit.as_deref(),
            Some("run the tests"),
            "the bound prompt was not submitted"
        );
    }

    /// A held chord emits Repeat events; each must not re-fire the
    /// binding — one hold of a prompt binding would otherwise queue a
    /// duplicate turn (and a duplicate model call) per repeat.
    #[test]
    fn a_held_chord_repeat_does_not_refire_the_binding() {
        use crate::ui::keybindings::KeyAction;
        let mut app = app_with_binding(
            "alt+r",
            KeyAction::Prompt {
                prompt: "run the tests".into(),
            },
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
        );
        assert_eq!(
            app.pending_submit.as_deref(),
            Some("run the tests"),
            "the initial press did not fire"
        );
        handle_key(
            &mut app,
            KeyEvent::new_with_kind(KeyCode::Char('r'), KeyModifiers::ALT, KeyEventKind::Repeat),
        );
        assert!(
            app.queue.is_empty(),
            "a key repeat queued a duplicate prompt"
        );
    }

    /// A binding fired mid-composition must not eat the draft: the bound
    /// action submits its own text, the user's half-written prompt stays.
    #[test]
    fn a_bound_command_keeps_the_composer_draft() {
        use crate::ui::keybindings::KeyAction;
        let mut app = app_with_binding(
            "ctrl+k",
            KeyAction::Command {
                command: "tasks".into(),
            },
        );
        app.input = "half a thought".to_string();
        app.cursor = 4;
        let before = app.show_tasks;
        handle_key(&mut app, ctrl('k'));
        assert_ne!(app.show_tasks, before, "the bound command did not run");
        assert_eq!(app.input, "half a thought", "the binding ate the draft");
        assert_eq!(app.cursor, 4, "the binding moved the cursor");
    }

    #[test]
    fn a_bound_prompt_keeps_the_composer_draft() {
        use crate::ui::keybindings::KeyAction;
        let mut app = app_with_binding(
            "alt+r",
            KeyAction::Prompt {
                prompt: "run the tests".into(),
            },
        );
        app.input = "half a thought".to_string();
        app.cursor = 3;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
        );
        assert_eq!(
            app.pending_submit.as_deref(),
            Some("run the tests"),
            "the bound prompt was not submitted"
        );
        assert_eq!(app.input, "half a thought", "the binding ate the draft");
        assert_eq!(app.cursor, 3, "the binding moved the cursor");
    }

    /// An unbound chord must still reach the built-in handler.
    #[test]
    fn an_unbound_chord_falls_through_to_the_built_in() {
        use crate::ui::keybindings::KeyAction;
        let mut app = app_with_binding(
            "ctrl+k",
            KeyAction::Command {
                command: "tasks".into(),
            },
        );
        handle_key(&mut app, ctrl('p'));
        assert!(
            app.command_palette_open(),
            "Ctrl+P stopped opening the palette"
        );
    }

    /// Rebinding Ctrl+C must not take away the way out.
    #[test]
    fn a_binding_cannot_steal_ctrl_c() {
        use crate::ui::keybindings::KeyAction;
        let mut app = app_with_binding(
            "ctrl+c",
            KeyAction::Prompt {
                prompt: "hijacked".into(),
            },
        );
        app.phase = Phase::Streaming;
        handle_key(&mut app, ctrl('c'));
        assert!(app.cancel_requested, "Ctrl+C no longer cancels");
        assert!(app.pending_submit.is_none(), "Ctrl+C submitted a prompt");
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
    fn ctrl_c_cancels_through_shortcuts_overlay() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.show_shortcuts = true;
        handle_key(&mut app, ctrl('c'));
        assert!(
            app.cancel_requested,
            "Ctrl+C must cancel even while help overlay is open"
        );
    }

    #[test]
    fn ctrl_c_uppercase_still_cancels() {
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        handle_key(&mut app, ctrl('C'));
        assert!(app.cancel_requested, "Ctrl+C (uppercase) must cancel");
    }

    #[test]
    fn ctrl_shift_c_copies_not_cancels() {
        // Ctrl+Shift+C is copy selection / last reply — not cancel.
        let mut app = App::new("m", "/tmp", "s");
        app.phase = Phase::Streaming;
        app.transcript
            .push(super::super::app::TranscriptItem::Assistant("hi".into()));
        handle_key(&mut app, ctrl_shift('c'));
        assert!(!app.cancel_requested, "Ctrl+Shift+C must not cancel a turn");
        // Copy path leaves a toast (or status) rather than arming quit.
        assert!(!app.quit_armed);
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
    fn ctrl_shift_c_idle_does_not_quit() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl_shift('c'));
        assert!(!app.should_quit);
        assert!(!app.quit_armed);
        handle_key(&mut app, ctrl_shift('C'));
        assert!(!app.should_quit);
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
    fn permission_preview_scroll_keys_work_even_during_grace() {
        let mut app = App::new("m", "/tmp", "s");
        let (tx, _rx) = std::sync::mpsc::channel();
        app.modals.push_back(super::super::app::Modal::Permission(
            super::super::app::PendingPermission {
                name: "Bash".into(),
                description: "run".into(),
                origin: None,
                input_preview: Some(
                    (1..=50)
                        .map(|i| format!("line{i}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                respond: tx,
            },
        ));
        app.phase = Phase::Permission;
        // Deliberately NOT force_hitl_answers_ready(): scrolling is
        // read-only and must work while answers are still gated.
        handle_key(&mut app, key(KeyCode::Down));
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.perm_scroll, 2);
        handle_key(&mut app, key(KeyCode::PageDown));
        assert_eq!(app.perm_scroll, 12);
        handle_key(&mut app, key(KeyCode::PageUp));
        assert_eq!(app.perm_scroll, 2);
        handle_key(&mut app, key(KeyCode::Up));
        handle_key(&mut app, key(KeyCode::Up));
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(app.perm_scroll, 0, "saturates at the top");
        // Upper clamp: cannot scroll past the last line.
        for _ in 0..20 {
            handle_key(&mut app, key(KeyCode::PageDown));
        }
        assert_eq!(app.perm_scroll, 49);
        // Resolving the modal resets the offset for the next one.
        app.force_hitl_answers_ready();
        handle_key(&mut app, key(KeyCode::Char('y')));
        assert_eq!(app.perm_scroll, 0);
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
        app.force_hitl_answers_ready();
        // y must reach the modal, not the palette filter.
        handle_key(&mut app, key(KeyCode::Char('y')));
        assert!(!app.command_palette_open());
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::AllowOnce)));
    }

    #[test]
    fn permission_phase_closes_search_and_takes_keys() {
        use agent_code_lib::tools::PermissionResponse;
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('f'));
        assert!(app.search_open());
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
        app.force_hitl_answers_ready();
        // y must reach the modal, not the search query — the bar is not
        // even drawn during Permission, so a swallowed key looks dead.
        handle_key(&mut app, key(KeyCode::Char('y')));
        assert!(!app.search_open());
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::AllowOnce)));
    }

    /// Bare letters are query characters, full stop — `N` must be typable
    /// in a smart-case query like `API_NAME`.
    #[test]
    fn search_uppercase_n_edits_the_query_instead_of_navigating() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('f'));
        for c in "API_N".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        assert_eq!(app.search.as_ref().unwrap().query, "API_N");
    }

    /// Enter is the exit that stays at the found match; Esc is the one
    /// that goes back. Without the former the bar had to stay open to
    /// keep the position it found.
    #[test]
    fn search_enter_closes_the_bar_and_stays_put() {
        let mut app = App::new("m", "/tmp", "s");
        app.viewport_h = 10;
        for t in ["auth one", "filler", "auth two"] {
            app.transcript
                .push(super::super::app::TranscriptItem::System(t.into()));
        }
        let expanded = app.expanded.clone();
        app.layout.sync(&app.transcript, 80, &expanded, None);
        app.scroll_to_top();
        handle_key(&mut app, ctrl('f'));
        for c in "auth".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        let at_match = app.scroll;
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(!app.search_open(), "Enter must close the bar");
        assert_eq!(app.scroll, at_match, "Enter must keep the match position");
        assert!(app.pending_submit.is_none(), "Enter must not submit a turn");
    }

    #[test]
    fn search_ctrl_n_and_ctrl_p_step_matches() {
        let mut app = App::new("m", "/tmp", "s");
        app.viewport_h = 10;
        for t in ["auth one", "filler", "auth two"] {
            app.transcript
                .push(super::super::app::TranscriptItem::System(t.into()));
        }
        let expanded = app.expanded.clone();
        app.layout.sync(&app.transcript, 80, &expanded, None);
        handle_key(&mut app, ctrl('f'));
        for c in "auth".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        assert_eq!(app.search.as_ref().unwrap().position(), (1, 2));
        handle_key(&mut app, ctrl('n'));
        assert_eq!(app.search.as_ref().unwrap().position(), (2, 2));
        handle_key(&mut app, ctrl('p'));
        assert_eq!(app.search.as_ref().unwrap().position(), (1, 2));
    }

    #[test]
    fn paste_lands_in_the_open_search_query_not_the_composer() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('f'));
        handle_paste(&mut app, "auth token");
        assert_eq!(app.search.as_ref().unwrap().query, "auth token");
        assert!(
            app.input.is_empty(),
            "paste must not leak into the composer behind the bar"
        );
    }

    #[test]
    fn permission_answer_keys_blocked_before_draw_and_grace() {
        use agent_code_lib::tools::PermissionResponse;
        let mut app = App::new("m", "/tmp", "s");
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
        app.reset_hitl_answer_arm();
        // In-flight typing the instant the modal appears must not allow-session.
        handle_key(&mut app, key(KeyCode::Char('a')));
        handle_key(&mut app, key(KeyCode::Char('y')));
        assert!(
            rx.try_recv().is_err(),
            "answer keys must wait for paint + grace"
        );
        assert!(app.front_modal().is_some());
        // After draw arm + forced ready, answers work.
        app.note_hitl_drawn();
        assert!(!app.hitl_answers_ready(), "still in grace window");
        app.force_hitl_answers_ready();
        handle_key(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::AllowOnce)));
    }

    #[test]
    fn permission_esc_works_during_grace() {
        use agent_code_lib::tools::PermissionResponse;
        let mut app = App::new("m", "/tmp", "s");
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
        app.reset_hitl_answer_arm();
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(matches!(rx.try_recv(), Ok(PermissionResponse::Deny)));
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
    fn ctrl_m_empty_requests_model_picker() {
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('m'));
        assert_eq!(
            app.pending_model,
            Some(super::super::app::PendingModelAction::Show)
        );
        assert!(!app.multiline_mode);
    }

    #[test]
    fn ctrl_m_with_draft_toggles_multiline() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "draft".into();
        app.cursor = 5;
        handle_key(&mut app, ctrl('m'));
        assert!(app.multiline_mode);
        assert!(app.pending_model.is_none());
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
    fn keyboard_enhancement_push_requests_disambiguation() {
        // CSI > 1 u — push flags with DISAMBIGUATE_ESCAPE_CODES (bit 1).
        let s = ansi_of(PushKeyboardEnhancementFlags(KEYBOARD_ENHANCEMENT_FLAGS));
        assert_eq!(s, "\x1b[>1u", "unexpected push sequence: {s:?}");
    }

    #[test]
    fn keyboard_enhancement_pop_is_emitted_on_teardown() {
        // CSI < 1 u — pop one entry off the terminal's keyboard-flag stack.
        // Leaking this makes the user's shell unusable, so the teardown
        // paths must all emit it.
        let s = ansi_of(PopKeyboardEnhancementFlags);
        assert_eq!(s, "\x1b[<1u", "unexpected pop sequence: {s:?}");
    }

    #[test]
    fn keyboard_enhancement_pop_only_fires_when_pushed() {
        // The pop is stack-balanced: popping without a push would eat an
        // outer application's flags.
        KEYBOARD_ENHANCEMENT_WANTED.store(false, Ordering::Relaxed);
        KEYBOARD_ENHANCEMENT_PUSHED.store(false, Ordering::Relaxed);
        let mut buf: Vec<u8> = Vec::new();
        push_keyboard_enhancement(&mut buf);
        assert!(buf.is_empty(), "must not push when unsupported/denylisted");
        pop_keyboard_enhancement(&mut buf);
        assert!(buf.is_empty(), "must not pop what we never pushed");

        // With the capability granted the push is attempted. Whether the
        // terminal layer accepts it is platform-dependent: the sequence is a
        // Unix terminal concept, and the Windows console path rejects the
        // command outright. So assert the *invariant* rather than success —
        // we record a push exactly when bytes went out, and we only ever pop
        // what we pushed. An unbalanced pop would eat an outer
        // application's keyboard flags.
        KEYBOARD_ENHANCEMENT_WANTED.store(true, Ordering::Relaxed);
        buf.clear();
        push_keyboard_enhancement(&mut buf);
        let pushed = KEYBOARD_ENHANCEMENT_PUSHED.load(Ordering::Relaxed);
        assert_eq!(
            pushed,
            !buf.is_empty(),
            "the pushed flag must track whether bytes were actually emitted"
        );

        if pushed {
            push_keyboard_enhancement(&mut buf);
            assert_eq!(
                String::from_utf8_lossy(&buf).matches("\x1b[>").count(),
                1,
                "push must be idempotent"
            );
            buf.clear();
            pop_keyboard_enhancement(&mut buf);
            assert_eq!(String::from_utf8_lossy(&buf), "\x1b[<1u");
            assert!(!KEYBOARD_ENHANCEMENT_PUSHED.load(Ordering::Relaxed));
            buf.clear();
            pop_keyboard_enhancement(&mut buf);
            assert!(buf.is_empty(), "double pop must be a no-op");
        } else {
            // The platform refused the push; we must not claim it happened,
            // and must not emit a pop we never earned.
            buf.clear();
            pop_keyboard_enhancement(&mut buf);
            assert!(
                buf.is_empty(),
                "must not pop when the push was rejected by the platform"
            );
        }
        KEYBOARD_ENHANCEMENT_WANTED.store(false, Ordering::Relaxed);
    }

    #[test]
    fn ctrl_l_forces_a_full_redraw_and_drops_the_layout_cache() {
        let mut app = App::new("m", "/tmp", "s");
        app.transcript
            .push(super::super::app::TranscriptItem::User("hello".into()));
        app.layout
            .sync(&app.transcript, 80, &std::collections::HashSet::new(), None);
        assert!(app.layout.total_lines() > 0, "cache populated");
        app.dirty = false;
        app.force_full_redraw = false;

        handle_key(&mut app, ctrl('l'));

        assert!(app.force_full_redraw, "Ctrl+L must clear the terminal");
        assert!(app.dirty, "Ctrl+L must schedule a repaint");
        assert_eq!(
            app.layout.total_lines(),
            0,
            "Ctrl+L must drop the layout cache so every block re-renders"
        );
        assert!(
            app.input.is_empty(),
            "Ctrl+L must not type a literal into the composer"
        );
        // Uppercase variant (some terminals report Ctrl+Shift+L as 'L').
        let mut app = App::new("m", "/tmp", "s");
        handle_key(&mut app, ctrl('L'));
        assert!(app.force_full_redraw);
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
