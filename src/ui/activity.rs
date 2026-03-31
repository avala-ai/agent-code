//! Activity indicator for long-running operations.
//!
//! Shows an animated status line while the agent is thinking or
//! executing tools. Runs on a background thread and clears itself
//! when the operation completes.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::style::Stylize;

/// Phrases cycled through during LLM response wait.
const THINKING_PHRASES: &[&str] = &[
    "reasoning",
    "considering",
    "analyzing",
    "evaluating",
    "processing",
    "formulating",
];

/// Frames for the dot animation.
const DOT_FRAMES: &[&str] = &["   ", ".  ", ".. ", "..."];

/// An animated activity indicator that runs until dropped or stopped.
pub struct ActivityIndicator {
    active: Arc<AtomicBool>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl ActivityIndicator {
    /// Start a new indicator with a label.
    pub fn start(label: &str) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let active_clone = active.clone();
        let label = label.to_string();

        let handle = tokio::spawn(async move {
            let mut frame = 0usize;
            let mut phrase_idx = 0usize;

            while active_clone.load(Ordering::Relaxed) {
                let dots = DOT_FRAMES[frame % DOT_FRAMES.len()];
                let phrase = THINKING_PHRASES[phrase_idx % THINKING_PHRASES.len()];

                let status = if label.is_empty() {
                    format!("{phrase}{dots}")
                } else {
                    format!("{label}{dots}")
                };

                eprint!("\r{}", status.dark_grey());
                let _ = std::io::stderr().flush();

                tokio::time::sleep(Duration::from_millis(400)).await;
                frame += 1;
                if frame % (DOT_FRAMES.len() * 2) == 0 {
                    phrase_idx += 1;
                }
            }

            // Clear the line.
            eprint!("\r{}\r", " ".repeat(40));
            let _ = std::io::stderr().flush();
        });

        Self {
            active,
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

    /// Stop the indicator.
    pub fn stop(&self) {
        self.active.store(false, Ordering::Relaxed);
    }
}

impl Drop for ActivityIndicator {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Relaxed);
        // Don't block on the handle — just let it clean up.
    }
}
