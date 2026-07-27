//! Terminal UI layer.
//!
//! Interactive sessions always use the full-screen modern TUI (`modern`).
//! Headless (`-p`), HTTP (`--serve`), and ACP remain separate entry points.

pub mod color_emit;
pub mod keybindings;
pub mod modern;
pub mod onboarding;
pub mod render;
pub mod selector;
pub mod setup;
pub mod terminal_query;
#[path = "theme_runtime.rs"]
pub mod theme;
pub mod tui;
