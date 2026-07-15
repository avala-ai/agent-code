//! Full-screen modern TUI (alt-screen ratatui pager).
//!
//! The only interactive surface. See `docs/tui/README.md`.

mod app;
mod colors;
#[cfg(test)]
mod fake_engine;
mod layout;
mod markdown;
mod modal;
mod mode;
mod render;
mod run;
mod scroll;
mod sink;
mod stream_buffer;
mod tasks;
mod terminal_caps;
mod toolcard;

pub use run::run_modern_tui;
