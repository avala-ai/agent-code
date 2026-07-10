//! Terminal UI layer.
//!
//! The interactive surface is the full-screen **modern** TUI
//! (`modern` module — alt-screen ratatui pager). Classic rustyline
//! REPL was removed. Supporting modules here cover theming, setup,
//! and shared helpers used by modern and slash-command paths.

pub mod activity;
pub mod color_emit;
pub mod keybindings;
pub mod keymap;
pub mod modern;
pub mod onboarding;
pub mod prompt;
pub mod render;
pub mod selector;
pub mod setup;
pub mod terminal_query;
#[path = "theme_runtime.rs"]
pub mod theme;
pub mod tui;
