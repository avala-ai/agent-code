//! fake_engine test harness (#406): drive the REAL modern event loop with
//! a scripted provider under paused `tokio::time`.
//!
//! Architecture: instead of faking the UI-facing `EngineEvent` channel, the
//! harness fakes one level lower — the LLM [`Provider`]. A [`ScriptedProvider`]
//! plays a per-turn script of [`StreamEvent`]s (with virtual-time delays), so
//! every real layer in between runs for real: `QueryEngine`, tool execution,
//! permissions (`ModernPrompter` → modal → `PermissionResponse`), the
//! `ChannelSink` adapter, the coalescer, and `run::event_loop` itself.
//! Terminal input is a second script of crossterm [`Event`]s played on a
//! channel; frames render to a ratatui `TestBackend`.
//!
//! Tests run on a small multi-thread runtime with REAL time and short
//! script delays. Paused virtual time is not an option here:
//! `start_paused` implies a current-thread runtime, and the production
//! `ModernPrompter::ask` blocks its thread on a std channel while the UI
//! loop answers the modal — on one thread that deadlocks (the lib review
//! flagged this blocking as "safe only because the runtime is
//! multi-threaded"; the harness inherits that constraint). Cancel-latency
//! bounds are therefore asserted in generous real-time terms: the scripts
//! embed a 60s provider stall that only a working cancel path can skip.
//! Every test is hermetic: no network, no real terminal, no raw mode.
//!
//! To end a test script, drop/close the input channel: the loop treats a
//! closed terminal stream as quit (same as production EOF).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::Stream;
use tokio::sync::mpsc;

use agent_code_lib::config::Config;
use agent_code_lib::llm::message::{ContentBlock, StopReason, Usage};
use agent_code_lib::llm::provider::{Provider, ProviderError, ProviderRequest};
use agent_code_lib::llm::stream::StreamEvent;
use agent_code_lib::output_styles::AgentKind;
use agent_code_lib::permissions::PermissionChecker;
use agent_code_lib::query::{QueryEngine, QueryEngineConfig, Session};
use agent_code_lib::state::AppState;
use agent_code_lib::tools::registry::ToolRegistry;

use super::app::App;
use super::sink::{EngineEvent, ModernPrompter, ModernQuestionAsker};

/// One step of a scripted model turn.
pub(super) enum Step {
    /// Wait before the next emission (models streaming latency; also the
    /// stall that cancel tests race against).
    Sleep(Duration),
    /// Emit a raw provider stream event.
    Emit(StreamEvent),
}

/// Plays one pre-recorded script per `stream()` call (i.e. per model turn).
/// A turn's script task races emission against the request's cancel token,
/// exactly like a real provider's HTTP read loop must.
pub(super) struct ScriptedProvider {
    turns: std::sync::Mutex<VecDeque<Vec<Step>>>,
}

impl ScriptedProvider {
    pub(super) fn new(turns: Vec<Vec<Step>>) -> Self {
        Self {
            turns: std::sync::Mutex::new(turns.into()),
        }
    }

    /// Script for a plain text answer: chunked deltas, then completion.
    pub(super) fn text_turn(chunks: &[&str]) -> Vec<Step> {
        let mut steps = Vec::new();
        for c in chunks {
            steps.push(Step::Sleep(Duration::from_millis(5)));
            steps.push(Step::Emit(StreamEvent::TextDelta((*c).into())));
        }
        steps.push(Step::Emit(StreamEvent::ContentBlockComplete(
            ContentBlock::Text {
                text: chunks.concat(),
            },
        )));
        steps.push(Step::Emit(StreamEvent::Done {
            usage: Usage::default(),
            stop_reason: Some(StopReason::EndTurn),
        }));
        steps
    }

    /// Script for a turn that requests one tool call.
    pub(super) fn tool_turn(name: &str, input: serde_json::Value) -> Vec<Step> {
        vec![
            Step::Sleep(Duration::from_millis(5)),
            Step::Emit(StreamEvent::ContentBlockComplete(ContentBlock::ToolUse {
                id: format!("call-{name}"),
                name: name.into(),
                input,
            })),
            Step::Emit(StreamEvent::Done {
                usage: Usage::default(),
                stop_reason: Some(StopReason::ToolUse),
            }),
        ]
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "fake-engine"
    }

    async fn stream(
        &self,
        request: &ProviderRequest,
    ) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        // Scripts are consumed by AGENT-LOOP calls only. The engine also
        // makes background completions (memory extraction, titles) with no
        // tool schemas — those must not steal the next scripted turn, so
        // tool-less requests always get a bare completion.
        let script = if request.tools.is_empty() {
            None
        } else {
            self.turns.lock().unwrap().pop_front()
        };
        if std::env::var("FAKE_ENGINE_TRACE").is_ok() {
            eprintln!(
                "[fake_engine] provider.stream: tools={} msgs={} scripted={}",
                request.tools.len(),
                request.messages.len(),
                script.is_some()
            );
        }
        let script = script.unwrap_or_else(|| {
            // Tool-less/background call, or ran out of scripted turns:
            // end the conversation rather than hanging the test.
            vec![Step::Emit(StreamEvent::Done {
                usage: Usage::default(),
                stop_reason: Some(StopReason::EndTurn),
            })]
        });
        let cancel = request.cancel.clone();
        let trace = std::env::var("FAKE_ENGINE_TRACE").is_ok();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            for step in script {
                match step {
                    Step::Sleep(d) => {
                        tokio::select! {
                            _ = tokio::time::sleep(d) => {}
                            _ = cancel.cancelled() => {
                                if trace {
                                    eprintln!("[fake_engine] player cancelled mid-sleep");
                                }
                                return;
                            }
                        }
                    }
                    Step::Emit(ev) => {
                        if trace {
                            eprintln!("[fake_engine] player emit: {ev:?}");
                        }
                        if tx.send(ev).await.is_err() {
                            if trace {
                                eprintln!("[fake_engine] player: receiver dropped");
                            }
                            return;
                        }
                    }
                }
            }
            if trace {
                eprintln!("[fake_engine] player script complete");
            }
        });
        Ok(rx)
    }
}

/// A scripted terminal: plays `(at, Event)` pairs on a channel the event
/// loop consumes as its input stream. Times are virtual offsets from start.
/// When the last event has been sent the sender drops, the stream yields
/// `None`, and the loop quits — so scripts don't need an explicit quit key.
pub(super) struct ScriptedTerm {
    rx: mpsc::UnboundedReceiver<std::io::Result<Event>>,
}

impl ScriptedTerm {
    pub(super) fn play(script: Vec<(Duration, Event)>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let trace = std::env::var("FAKE_ENGINE_TRACE").is_ok();
            let start = tokio::time::Instant::now();
            for (at, ev) in script {
                tokio::time::sleep_until(start + at).await;
                if trace {
                    eprintln!("[fake_engine] term send at {:?}: {ev:?}", start.elapsed());
                }
                if tx.send(Ok(ev)).is_err() {
                    return;
                }
            }
            if trace {
                eprintln!("[fake_engine] term script done, closing channel");
            }
            // tx drops here → stream ends → event loop quits.
        });
        Self { rx }
    }
}

impl Stream for ScriptedTerm {
    type Item = std::io::Result<Event>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

pub(super) fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

pub(super) fn shift_backtab() -> Event {
    Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
}

pub(super) fn type_str(s: &str, from: Duration) -> Vec<(Duration, Event)> {
    s.chars()
        .map(|c| (from, key(KeyCode::Char(c))))
        .collect::<Vec<_>>()
}

/// Everything a fake_engine test needs to drive the real loop.
pub(super) struct Harness {
    pub(super) session: Session,
    pub(super) app: App,
    pub(super) eng_tx: mpsc::UnboundedSender<EngineEvent>,
    pub(super) eng_rx: mpsc::UnboundedReceiver<EngineEvent>,
    pub(super) base_mode: agent_code_lib::config::PermissionMode,
    /// Lock-free view of the engine's live plan flag (mid-turn assertions).
    pub(super) live_plan: Arc<std::sync::atomic::AtomicBool>,
    /// Shared permission checker (mid-turn default-mode assertions).
    pub(super) permissions: Arc<PermissionChecker>,
}

/// Build a real engine + session around a scripted provider, wired exactly
/// like `run_modern_tui` does it (prompter + question asker installed, so
/// `ask` decisions become UI modals instead of auto-allow).
pub(super) fn harness(provider: ScriptedProvider, cwd: &std::path::Path) -> Harness {
    let mut config = Config::default();
    // Keep the engine from touching the user's real data dir in tests.
    config.api.model = "fake-model".into();
    let permissions = PermissionChecker::from_config(&config.permissions);
    let mut state = AppState::new(config);
    state.cwd = cwd.display().to_string();

    let mut engine = QueryEngine::new(
        Arc::new(provider),
        ToolRegistry::default_tools(),
        permissions,
        state,
        QueryEngineConfig {
            max_turns: Some(4),
            verbose: false,
            unattended: false,
            agent_kind: AgentKind::Main,
        },
    );

    let (eng_tx, eng_rx) = mpsc::unbounded_channel::<EngineEvent>();
    engine.set_permission_prompter(ModernPrompter::new(eng_tx.clone()));
    engine.set_question_asker(ModernQuestionAsker::new(eng_tx.clone()));
    let live_plan = engine.live_plan_mode_handle();
    let permissions = engine.permissions_handle();
    let base_mode = engine.state().config.permissions.default_mode;

    let session = Session::new(engine);
    let app = App::new("fake-model", cwd.display().to_string(), "fake-session");

    Harness {
        session,
        app,
        eng_tx,
        eng_rx,
        base_mode,
        live_plan,
        permissions,
    }
}

/// Run the real `event_loop` against a scripted terminal, rendering frames
/// to a `TestBackend`. Returns the final `App` for assertions plus the
/// number of frames drawn.
pub(super) async fn run_script(
    mut h: Harness,
    term_script: Vec<(Duration, Event)>,
) -> (App, usize) {
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).expect("test backend");
    let mut frames = 0usize;
    let mut draw = |app: &mut App| -> anyhow::Result<()> {
        terminal.draw(|f| super::render::draw(f, app))?;
        frames += 1;
        if std::env::var("FAKE_ENGINE_TRACE").is_ok() {
            eprintln!(
                "[fake_engine] frame={} phase={:?} modals={} transcript={}",
                frames,
                app.phase,
                app.pending_modal_count() + usize::from(app.front_modal().is_some()),
                app.transcript.len()
            );
        }
        Ok(())
    };
    let mut term_events = ScriptedTerm::play(term_script);
    super::run::event_loop(
        &h.session,
        &mut h.app,
        h.eng_tx.clone(),
        h.eng_rx,
        h.base_mode,
        &mut term_events,
        &mut draw,
    )
    .await
    .expect("event loop");
    (h.app, frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::modern::app::TranscriptItem;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Full path: type → submit → deltas coalesce → turn completes → the
    /// transcript holds the assistant text and the loop went idle.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn end_to_end_prompt_streams_and_completes() {
        let provider = ScriptedProvider::new(vec![ScriptedProvider::text_turn(&["hel", "lo"])]);
        let tmp = tempfile::tempdir().unwrap();
        let h = harness(provider, tmp.path());

        let mut script = type_str("hi", ms(1));
        script.push((ms(2), key(KeyCode::Enter)));
        // Leave the loop plenty of time to finish the turn before the
        // input channel closes (which quits the loop).
        script.push((ms(2000), key(KeyCode::Char(' '))));

        let (app, frames) = run_script(h, script).await;

        assert!(frames > 0, "at least one frame drawn");
        assert!(
            app.transcript
                .iter()
                .any(|t| matches!(t, TranscriptItem::Assistant(s) if s.contains("hello"))),
            "assistant text reached the transcript: {:?}",
            app.transcript
        );
        assert_eq!(app.phase, crate::ui::modern::app::Phase::Idle);
        assert!(app.queue.is_empty());
    }

    /// Ctrl+C mid-turn skips a 60s provider stall: only a cancel that
    /// actually reaches the provider stream lets this test finish in
    /// seconds instead of a minute (#400 analogue at the UI layer).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ctrl_c_cancels_streaming_turn_quickly() {
        let provider = ScriptedProvider::new(vec![vec![
            Step::Emit(StreamEvent::TextDelta("thinking…".into())),
            Step::Sleep(Duration::from_secs(60)),
            Step::Emit(StreamEvent::Done {
                usage: Usage::default(),
                stop_reason: Some(StopReason::EndTurn),
            }),
        ]]);
        let tmp = tempfile::tempdir().unwrap();
        let h = harness(provider, tmp.path());

        let start = tokio::time::Instant::now();
        let mut script = type_str("go", ms(1));
        script.push((ms(2), key(KeyCode::Enter)));
        script.push((
            ms(100),
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ));
        // Bounded window: if cancel failed to reach the stream, the
        // provider task would hold the turn for the full 60s stall and
        // the reap (join) would blow the bound below.
        script.push((ms(2000), key(KeyCode::Char(' '))));

        let (app, _frames) = run_script(h, script).await;
        let elapsed = start.elapsed();

        assert_eq!(app.phase, crate::ui::modern::app::Phase::Idle);
        assert!(
            elapsed < Duration::from_secs(30),
            "cancel did not reach the stream: took {elapsed:?}"
        );
    }

    /// A write tool triggers the permission modal; 'y' allows it once and
    /// the tool actually runs (file exists). The prompter, modal FIFO and
    /// respond channel are all the production wiring.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permission_modal_allow_once_runs_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let provider = ScriptedProvider::new(vec![
            ScriptedProvider::tool_turn(
                "FileWrite",
                serde_json::json!({
                    "file_path": target.display().to_string(),
                    "content": "written-by-fake-engine"
                }),
            ),
            ScriptedProvider::text_turn(&["done"]),
        ]);
        let h = harness(provider, tmp.path());

        let mut script = type_str("write it", ms(1));
        script.push((ms(2), key(KeyCode::Enter)));
        // The modal pops once the tool's Ask decision reaches the prompter;
        // answer it with 'y' (allow once) a bit later in virtual time.
        script.push((ms(300), key(KeyCode::Char('y'))));
        script.push((ms(3000), key(KeyCode::Char(' '))));

        let (app, _frames) = run_script(h, script).await;

        assert!(
            target.exists(),
            "allowed tool actually executed and wrote the file"
        );
        assert_eq!(app.phase, crate::ui::modern::app::Phase::Idle);
        assert!(
            app.front_modal().is_none(),
            "no modal left pending after answer"
        );
    }

    /// Denying the modal blocks the tool: the file must not exist.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permission_modal_deny_blocks_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("blocked.txt");
        let provider = ScriptedProvider::new(vec![
            ScriptedProvider::tool_turn(
                "FileWrite",
                serde_json::json!({
                    "file_path": target.display().to_string(),
                    "content": "must-not-exist"
                }),
            ),
            ScriptedProvider::text_turn(&["ok"]),
        ]);
        let h = harness(provider, tmp.path());

        let mut script = type_str("try", ms(1));
        script.push((ms(2), key(KeyCode::Enter)));
        script.push((ms(300), key(KeyCode::Char('n'))));
        script.push((ms(3000), key(KeyCode::Char(' '))));

        let (app, _frames) = run_script(h, script).await;

        assert!(!target.exists(), "denied tool must not run");
        assert_eq!(app.phase, crate::ui::modern::app::Phase::Idle);
    }

    /// Shift+Tab mid-turn reaches the engine immediately: cycling
    /// Normal → AcceptEdits → Plan flips the live plan atomic and the
    /// checker's default mode while the turn still holds the engine lock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shift_tab_mid_turn_applies_live_mode() {
        let provider = ScriptedProvider::new(vec![vec![
            Step::Emit(StreamEvent::TextDelta("long…".into())),
            Step::Sleep(Duration::from_secs(5)),
            Step::Emit(StreamEvent::Done {
                usage: Usage::default(),
                stop_reason: Some(StopReason::EndTurn),
            }),
        ]]);
        let tmp = tempfile::tempdir().unwrap();
        let h = harness(provider, tmp.path());
        let live_plan = h.live_plan.clone();
        let checker = h.permissions.clone();

        let mut script = type_str("go", ms(1));
        script.push((ms(2), key(KeyCode::Enter)));
        // Mid-turn (turn runs 5 virtual seconds): two BackTabs from the
        // default Normal → AcceptEdits → Plan.
        script.push((ms(100), shift_backtab()));
        script.push((ms(110), shift_backtab()));
        // Probe point: give the loop a beat to apply the mode, then quit
        // while the turn is STILL streaming (mid-turn is the whole point).
        script.push((ms(200), key(KeyCode::Char(' '))));

        // Run the loop and the mid-turn probe concurrently on this task
        // (join!, not spawn — the loop future is deliberately !Send).
        let probe = async {
            // Poll the lock-free handles until the deadline; they must flip
            // while the provider stream is still sleeping (turn unfinished).
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if live_plan.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "live plan mode never applied mid-turn"
                );
                tokio::time::sleep(ms(10)).await;
            }
            assert_eq!(
                checker.default_mode(),
                agent_code_lib::config::PermissionMode::Plan,
                "checker default follows the UI mode mid-turn"
            );
        };
        let ((app, _frames), ()) = tokio::join!(run_script(h, script), probe);
        assert_eq!(app.mode, crate::ui::modern::mode::SessionMode::Plan);
    }

    /// Prompts submitted while a turn runs queue up and auto-send when the
    /// turn completes cleanly (plan §M5).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_prompt_auto_sends_after_turn() {
        let provider = ScriptedProvider::new(vec![
            vec![
                Step::Emit(StreamEvent::TextDelta("first".into())),
                Step::Sleep(ms(500)),
                Step::Emit(StreamEvent::ContentBlockComplete(ContentBlock::Text {
                    text: "first".into(),
                })),
                Step::Emit(StreamEvent::Done {
                    usage: Usage::default(),
                    stop_reason: Some(StopReason::EndTurn),
                }),
            ],
            ScriptedProvider::text_turn(&["second"]),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let h = harness(provider, tmp.path());

        let mut script = type_str("one", ms(1));
        script.push((ms(2), key(KeyCode::Enter)));
        // While turn 1 streams (500ms), type and submit a second prompt —
        // it must queue, then auto-send on completion.
        let mut second = type_str("two", ms(100));
        script.append(&mut second);
        script.push((ms(110), key(KeyCode::Enter)));
        script.push((ms(4000), key(KeyCode::Char(' '))));

        let (app, _frames) = run_script(h, script).await;

        assert!(app.queue.is_empty(), "queued prompt was auto-sent");
        assert!(
            app.transcript
                .iter()
                .any(|t| matches!(t, TranscriptItem::Assistant(s) if s.contains("second"))),
            "second turn ran: {:?}",
            app.transcript
        );
    }
}
