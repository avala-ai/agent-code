//! Customizable keyboard shortcuts.
//!
//! Keybindings are loaded from `~/.config/agent-code/keybindings.json`.
//! Each binding maps a key chord to an action (command, prompt, or
//! built-in function).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A single keybinding definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keybinding {
    /// Key sequence (e.g., "ctrl+k", "alt+r", "ctrl+shift+p").
    pub key: String,
    /// Action to perform.
    pub action: KeyAction,
    /// Optional description for help display.
    pub description: Option<String>,
}

/// Action triggered by a keybinding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum KeyAction {
    /// Execute a slash command.
    #[serde(rename = "command")]
    Command { command: String },
    /// Inject a prompt to the agent.
    #[serde(rename = "prompt")]
    Prompt { prompt: String },
    /// Toggle a setting.
    #[serde(rename = "toggle")]
    Toggle { setting: String },
}

/// Loaded keybindings registry.
#[derive(Debug)]
pub struct KeybindingRegistry {
    bindings: HashMap<String, Keybinding>,
    /// Chords that came from the user's file rather than the built-in
    /// defaults. The TUI dispatches only these — see
    /// [`KeybindingRegistry::is_user_defined`].
    user_defined: std::collections::HashSet<String>,
}

impl KeybindingRegistry {
    /// Load keybindings from the config file.
    pub fn load() -> Self {
        let mut registry = Self {
            bindings: HashMap::new(),
            user_defined: std::collections::HashSet::new(),
        };

        // Add built-in defaults.
        registry.add_default(
            "ctrl+c",
            KeyAction::Command {
                command: "cancel".into(),
            },
            "Cancel current operation",
        );
        registry.add_default(
            "ctrl+d",
            KeyAction::Command {
                command: "exit".into(),
            },
            "Exit",
        );
        registry.add_default(
            "ctrl+l",
            KeyAction::Command {
                command: "clear".into(),
            },
            "Clear conversation",
        );

        // Load user overrides.
        if let Some(path) = keybindings_path()
            && path.exists()
        {
            match load_keybindings_file(&path) {
                Ok(user_bindings) => {
                    for binding in user_bindings {
                        let key = binding.key.trim().to_lowercase();
                        registry.user_defined.insert(key.clone());
                        registry.bindings.insert(key, binding);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load keybindings: {e}");
                }
            }
        }

        registry
    }

    fn add_default(&mut self, key: &str, action: KeyAction, desc: &str) {
        self.bindings.insert(
            key.to_string(),
            Keybinding {
                key: key.to_string(),
                action,
                description: Some(desc.to_string()),
            },
        );
    }

    /// Look up the action for a key sequence.
    pub fn lookup(&self, key: &str) -> Option<&KeyAction> {
        self.bindings.get(key).map(|b| &b.action)
    }

    /// Get all bindings for display.
    pub fn all(&self) -> Vec<&Keybinding> {
        let mut bindings: Vec<_> = self.bindings.values().collect();
        bindings.sort_by_key(|b| &b.key);
        bindings
    }
}

fn keybindings_path() -> Option<PathBuf> {
    agent_code_lib::config::agent_config_dir().map(|d| d.join("keybindings.json"))
}

fn load_keybindings_file(path: &PathBuf) -> Result<Vec<Keybinding>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("Read error: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Parse error: {e}"))
}

/// Chords the user may not rebind.
///
/// `ctrl+c` and `esc` are how you get out of a stuck state — including
/// out of a binding that turned out to be a mistake — so they stay
/// fixed. Everything else is fair game.
pub const RESERVED_CHORDS: &[&str] = &["ctrl+c", "esc"];

/// Render a key event as the chord string used in `keybindings.json`
/// (`ctrl+k`, `alt+shift+r`, `f5`, `enter`).
///
/// Returns `None` for events that are not meaningfully bindable — a bare
/// printable character is ordinary typing, not a shortcut.
pub fn chord_string(
    code: crossterm::event::KeyCode,
    mods: crossterm::event::KeyModifiers,
) -> Option<String> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let name = match code {
        KeyCode::Char(c) => {
            // Shift is already encoded in the character the terminal
            // reports, so it is not repeated as a modifier for letters.
            let lower = c.to_ascii_lowercase().to_string();
            if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                lower
            } else {
                // Plain text: not a shortcut.
                return None;
            }
        }
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "shift+tab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Esc => "esc".into(),
        _ => return None,
    };

    let mut out = String::new();
    if mods.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl+");
    }
    if mods.contains(KeyModifiers::ALT) {
        out.push_str("alt+");
    }
    // Shift is only reported separately for non-character keys.
    if mods.contains(KeyModifiers::SHIFT) && !matches!(code, KeyCode::Char(_) | KeyCode::BackTab) {
        out.push_str("shift+");
    }
    out.push_str(&name);
    Some(out)
}

impl KeybindingRegistry {
    /// Build a registry from user bindings only, for tests.
    #[cfg(test)]
    pub fn from_user_bindings(bindings: Vec<Keybinding>) -> Self {
        let mut registry = Self::load();
        for b in bindings {
            let key = b.key.trim().to_lowercase();
            registry.user_defined.insert(key.clone());
            registry.bindings.insert(key, b);
        }
        registry
    }

    /// Look up a user-bound action for a key event.
    ///
    /// Reserved chords always return `None` so a binding cannot take away
    /// the means of escape.
    pub fn action_for(
        &self,
        code: crossterm::event::KeyCode,
        mods: crossterm::event::KeyModifiers,
    ) -> Option<&KeyAction> {
        let chord = chord_string(code, mods)?;
        if RESERVED_CHORDS.contains(&chord.as_str()) {
            return None;
        }
        self.lookup(&chord)
    }

    /// True when the user's file supplied this chord (as opposed to a
    /// built-in default). Only these are dispatched by the TUI: the
    /// defaults describe chords the hardcoded handler already owns, and
    /// routing them through here would double-handle them.
    pub fn is_user_defined(&self, chord: &str) -> bool {
        self.user_defined.contains(chord)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn chords_render_the_way_the_config_file_spells_them() {
        for (code, mods, expected) in [
            (KeyCode::Char('k'), KeyModifiers::CONTROL, Some("ctrl+k")),
            (KeyCode::Char('K'), KeyModifiers::CONTROL, Some("ctrl+k")),
            (KeyCode::Char('r'), KeyModifiers::ALT, Some("alt+r")),
            (
                KeyCode::Char('p'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
                Some("ctrl+alt+p"),
            ),
            (KeyCode::F(5), KeyModifiers::NONE, Some("f5")),
            (KeyCode::Up, KeyModifiers::NONE, Some("up")),
            (KeyCode::Up, KeyModifiers::SHIFT, Some("shift+up")),
            (KeyCode::Esc, KeyModifiers::NONE, Some("esc")),
        ] {
            assert_eq!(
                chord_string(code, mods).as_deref(),
                expected,
                "{code:?} + {mods:?}"
            );
        }
    }

    /// A bare printable character is typing, not a shortcut. If it were
    /// bindable, a binding on `a` would make the composer unusable.
    #[test]
    fn a_plain_character_is_not_a_chord() {
        assert_eq!(chord_string(KeyCode::Char('a'), KeyModifiers::NONE), None);
        assert_eq!(chord_string(KeyCode::Char('A'), KeyModifiers::SHIFT), None);
    }

    /// Ctrl+C and Esc are how you escape a stuck state, including a
    /// binding that turned out to be a mistake. They stay fixed.
    #[test]
    fn reserved_chords_cannot_be_rebound() {
        let mut registry = KeybindingRegistry {
            bindings: HashMap::new(),
            user_defined: std::collections::HashSet::new(),
        };
        for chord in RESERVED_CHORDS {
            registry.user_defined.insert((*chord).to_string());
            registry.bindings.insert(
                (*chord).to_string(),
                Keybinding {
                    key: (*chord).to_string(),
                    action: KeyAction::Prompt {
                        prompt: "hijacked".into(),
                    },
                    description: None,
                },
            );
        }
        assert!(
            registry
                .action_for(KeyCode::Char('c'), KeyModifiers::CONTROL)
                .is_none(),
            "ctrl+c was rebindable"
        );
        assert!(
            registry
                .action_for(KeyCode::Esc, KeyModifiers::NONE)
                .is_none(),
            "esc was rebindable"
        );
    }

    /// Built-in defaults describe chords the hardcoded handler already
    /// owns. Dispatching them from the registry too would run them twice.
    #[test]
    fn built_in_defaults_are_not_user_defined() {
        let registry = KeybindingRegistry::load();
        for chord in ["ctrl+c", "ctrl+d", "ctrl+l"] {
            assert!(
                registry.lookup(chord).is_some(),
                "default {chord} missing from the registry"
            );
            assert!(
                !registry.is_user_defined(chord),
                "default {chord} would be dispatched as if the user wrote it"
            );
        }
    }

    #[test]
    fn a_user_binding_is_found_by_its_chord() {
        let mut registry = KeybindingRegistry {
            bindings: HashMap::new(),
            user_defined: std::collections::HashSet::new(),
        };
        registry.user_defined.insert("ctrl+k".to_string());
        registry.bindings.insert(
            "ctrl+k".to_string(),
            Keybinding {
                key: "ctrl+k".to_string(),
                action: KeyAction::Command {
                    command: "tasks".into(),
                },
                description: None,
            },
        );
        assert!(registry.is_user_defined("ctrl+k"));
        assert!(matches!(
            registry.action_for(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Some(KeyAction::Command { command }) if command == "tasks"
        ));
    }
}
