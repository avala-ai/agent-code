//! Terminal UI layer.
//!
//! Interactive sessions always use the full-screen modern TUI (`modern`).
//! Headless (`-p`), HTTP (`--serve`), and ACP remain separate entry points.

pub mod color_emit;
pub mod fuzzy;
pub mod keybindings;
pub mod modern;
pub mod onboarding;
pub mod render;
pub mod selector;
pub mod setup;
pub mod terminal_query;
pub mod text_safety;
#[path = "theme_runtime.rs"]
pub mod theme;
pub mod tui;

/// Locks shared by tests that touch process-global state.
///
/// Env vars are the process's, not a module's: the onboarding tests point
/// `XDG_CONFIG_HOME` and `HOME` at a tempdir and delete it afterwards,
/// and anything reading the user config layer meanwhile sees a directory
/// that has just vanished. A lock private to one module cannot express
/// that — the tests that *mutate* and the tests that *depend on* the same
/// variable have to serialize against each other.
#[cfg(test)]
pub(crate) mod test_locks {
    use std::sync::{Mutex, MutexGuard};

    static ENV: Mutex<()> = Mutex::new(());

    /// Held by any test that sets process env, and by any test whose
    /// result depends on it — config loading above all, since it reads
    /// the user layer through `XDG_CONFIG_HOME`.
    ///
    /// Poison is ignored: a panicking test tells us nothing about whether
    /// the environment is usable, and refusing the lock would turn one
    /// failure into every later test failing.
    pub(crate) fn env() -> MutexGuard<'static, ()> {
        ENV.lock().unwrap_or_else(|e| e.into_inner())
    }
}
