//! Spawnable turns.
//!
//! [`QueryEngine::run_turn_with_sink`] takes `&mut self`, so a turn can
//! only run while its caller holds the engine exclusively — it cannot be
//! moved onto a `tokio::spawn` task, observed, or cancelled from
//! elsewhere. [`Session`] wraps the engine in an `Arc<tokio::Mutex<…>>`
//! and runs turns through it, returning a [`TurnHandle`] that can be
//! awaited, polled for [`TurnStatus`], or cancelled — even while the
//! turn owns the engine lock.
//!
//! This is the foundation the promotion (foreground→background) and
//! steering work builds on. The engine's turn internals are untouched;
//! `Session` only owns the turn's *lifecycle*.

use std::sync::Arc;

use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

use super::{QueryEngine, StreamSink, TurnStatus};
use crate::error::{Error, Result};

/// Owns a [`QueryEngine`] behind a shared async mutex so turns can run
/// on detached tasks.
#[derive(Clone)]
pub struct Session {
    engine: Arc<Mutex<QueryEngine>>,
}

impl Session {
    /// Wrap an engine for spawnable-turn execution.
    pub fn new(engine: QueryEngine) -> Self {
        Self {
            engine: Arc::new(Mutex::new(engine)),
        }
    }

    /// Access the shared engine (e.g. to read state between turns).
    pub fn engine(&self) -> Arc<Mutex<QueryEngine>> {
        self.engine.clone()
    }

    /// Subscribe to the engine's turn-status stream.
    pub async fn turn_status(&self) -> watch::Receiver<TurnStatus> {
        self.engine.lock().await.turn_status()
    }

    /// Run a turn to completion, holding the engine lock for its
    /// duration (the foreground path). Equivalent to calling the engine
    /// directly, but routed through the session so status is published.
    pub async fn run_turn(&self, input: &str, sink: &dyn StreamSink) -> Result<()> {
        self.engine
            .lock()
            .await
            .run_turn_with_sink(input, sink)
            .await
    }

    /// Spawn a turn on a detached task and return a handle to it.
    ///
    /// The task acquires the engine lock and runs the turn; the returned
    /// [`TurnHandle`] carries a status receiver and a cancel handle that
    /// work *without* the engine lock, so the turn can be observed and
    /// cancelled while it runs.
    pub async fn spawn_turn(&self, input: String, sink: Arc<dyn StreamSink>) -> TurnHandle {
        // Start the turn's lifecycle up front (install this turn's
        // cancel token + publish `Running`) *before* grabbing the
        // handles, so the returned handle binds to this turn: its cancel
        // targets this turn's token, and its status baseline is this
        // turn's `Running` rather than a stale terminal value left in the
        // channel by a previous turn.
        let (status, cancel, steer) = {
            let mut engine = self.engine.lock().await;
            engine.begin_turn();
            (
                engine.turn_status(),
                engine.cancel_handle(),
                engine.steer_sender(),
            )
        };

        let engine = self.engine.clone();
        let join = tokio::spawn(async move {
            let mut engine = engine.lock().await;
            engine.run_turn_spawned(&input, sink.as_ref()).await
        });

        TurnHandle {
            join,
            status,
            cancel,
            steer,
        }
    }
}

/// Handle to a turn running on a detached task.
pub struct TurnHandle {
    join: tokio::task::JoinHandle<Result<()>>,
    status: watch::Receiver<TurnStatus>,
    cancel: Arc<std::sync::Mutex<CancellationToken>>,
    steer: tokio::sync::mpsc::UnboundedSender<String>,
}

impl TurnHandle {
    /// The latest observed [`TurnStatus`].
    pub fn status(&self) -> TurnStatus {
        self.status.borrow().clone()
    }

    /// Request cancellation of the running turn. Cancels the engine's
    /// current turn token without needing the engine lock, so it works
    /// even while the turn task holds it.
    pub fn cancel(&self) {
        self.cancel.lock().unwrap().cancel();
    }

    /// Steer the running turn: inject `text` as a user message at the
    /// turn's next agent-loop iteration boundary. Returns `false` if the
    /// turn's engine has gone away. Works without the engine lock.
    pub fn steer(&self, text: impl Into<String>) -> bool {
        self.steer.send(text.into()).is_ok()
    }

    /// Wait until the turn reaches a terminal status and return it.
    /// Does not consume the handle (the task may still be finishing its
    /// teardown after publishing the terminal status).
    pub async fn wait_status(&mut self) -> TurnStatus {
        loop {
            {
                let s = self.status.borrow();
                if s.is_final() {
                    return s.clone();
                }
            }
            if self.status.changed().await.is_err() {
                // Sender dropped — return whatever we last saw.
                return self.status.borrow().clone();
            }
        }
    }

    /// Await the turn task and return its result. A panic in the task
    /// surfaces as an error rather than propagating.
    pub async fn join(self) -> Result<()> {
        match self.join.await {
            Ok(r) => r,
            Err(e) => Err(Error::Other(format!("turn task failed to join: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_status_is_final_only_for_terminal_states() {
        assert!(!TurnStatus::Idle.is_final());
        assert!(!TurnStatus::Running.is_final());
        assert!(TurnStatus::Completed.is_final());
        assert!(TurnStatus::Aborted.is_final());
        assert!(TurnStatus::Errored("x".into()).is_final());
    }
}
