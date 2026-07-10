//! Terminal UI layer.
//!
//! Provides the interactive REPL, markdown rendering, and streaming
//! output display. Built on crossterm and rustyline.
//!
//! The **modern** full-screen TUI (`modern` module) is the alt-screen
//! pager overhaul; opt in with `--tui modern`. Classic REPL remains the default.

pub mod activity;
pub mod color_emit;
pub mod keybindings;
pub mod keymap;
pub mod modern;
pub mod onboarding;
pub mod prompt;
pub mod render;
pub mod repl;
pub mod selector;
pub mod setup;
pub mod terminal_query;
#[path = "theme_runtime.rs"]
pub mod theme;
pub mod tui;
