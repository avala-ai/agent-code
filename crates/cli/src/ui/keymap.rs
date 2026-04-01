//! Keyboard configuration for the REPL.
//!
//! Supports emacs (default) and vi editing modes via rustyline's
//! built-in modal editing. The active mode is determined by config.

use rustyline::config::EditMode;

/// Editing mode for the REPL input line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Standard emacs keybindings (Ctrl-A, Ctrl-E, etc.).
    Emacs,
    /// Vi-style modal editing (normal/insert modes).
    Vi,
}

impl InputMode {
    /// Parse from a config string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "vi" | "vim" => Self::Vi,
            _ => Self::Emacs,
        }
    }

    /// Convert to rustyline's EditMode.
    pub fn to_edit_mode(self) -> EditMode {
        match self {
            Self::Emacs => EditMode::Emacs,
            Self::Vi => EditMode::Vi,
        }
    }
}

impl Default for InputMode {
    fn default() -> Self {
        // Check EDITOR env var for vim preference.
        if let Ok(editor) = std::env::var("EDITOR")
            && editor.contains("vi")
        {
            return Self::Vi;
        }
        Self::Emacs
    }
}
