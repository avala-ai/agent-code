//! Activity indicator for long-running operations.
//!
//! Shows an animated status line while the agent is thinking or
//! executing tools. Runs on a background thread and clears itself
//! when the operation completes.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::style::{Stylize, style};

/// Status labels displayed while waiting for a response.
const WAIT_LABELS: &[&str] = &[
    "working",
    "running",
    "figuring",
    "searching",
    "assembling",
    "parsing",
    "resolving",
    "mapping",
    "tracing",
    "scanning",
];

/// Frames for the dot animation.
const DOT_FRAMES: &[&str] = &["   ", ".  ", ".. ", "..."];

/// An animated activity indicator that runs until dropped or stopped.
pub struct ActivityIndicator {
    active: Arc<AtomicBool>,
    /// Serializes spinner frame prints against the clear performed by
    /// `stop()`, so the line is guaranteed empty before any streamed text
    /// is written where the spinner used to be.
    print_lock: Arc<Mutex<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl ActivityIndicator {
    /// Start a new indicator with a label.
    pub fn start(label: &str) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let active_clone = active.clone();
        let print_lock = Arc::new(Mutex::new(()));
        let print_lock_clone = print_lock.clone();
        let label = label.to_string();

        let handle = tokio::spawn(async move {
            let mut frame = 0usize;
            let mut phrase_idx = 0usize;

            loop {
                // Re-check the flag under the lock and paint atomically, so a
                // concurrent stop() either clears before this frame prints or
                // waits for it to finish — never leaves a frame on screen
                // after the clear.
                {
                    let _guard = print_lock_clone.lock().unwrap();
                    if !active_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    let dots = DOT_FRAMES[frame % DOT_FRAMES.len()];
                    let phrase = WAIT_LABELS[phrase_idx % WAIT_LABELS.len()];

                    let status = if label.is_empty() {
                        format!("{phrase}{dots}")
                    } else {
                        format!("{label}{dots}")
                    };

                    let color = super::theme::current().muted;
                    print!("\r{}", style(status).with(color));
                    let _ = std::io::stdout().flush();
                }

                tokio::time::sleep(Duration::from_millis(400)).await;
                frame += 1;
                if frame.is_multiple_of(DOT_FRAMES.len() * 2) {
                    phrase_idx += 1;
                }
            }
        });

        Self {
            active,
            print_lock,
            handle: Some(handle),
        }
    }

    /// Start an indicator for LLM thinking.
    pub fn thinking() -> Self {
        Self::start("")
    }

    /// Start an indicator for tool execution.
    pub fn tool(tool_name: &str) -> Self {
        Self::start(&format!("running {tool_name}"))
    }

    /// Stop the indicator and synchronously clear its line.
    ///
    /// Taking `print_lock` waits out any in-flight frame print; once held, no
    /// further frame can run (the task re-checks `active` under the same lock),
    /// so the erase below is the last thing written to the row. Callers can
    /// then print streamed text at column 0 without it being clobbered.
    pub fn stop(&self) {
        self.active.store(false, Ordering::Relaxed);
        let _guard = self.print_lock.lock().unwrap();
        // Erase the whole line (not a fixed 60 spaces) and park the cursor at
        // column 0.
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
    }
}

impl Drop for ActivityIndicator {
    fn drop(&mut self) {
        self.stop();
        // Detach the task; it exits on its next wakeup when it sees `active`
        // is false. No need to join.
        self.handle.take();
    }
}
