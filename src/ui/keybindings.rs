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
pub struct KeybindingRegistry {
    bindings: HashMap<String, Keybinding>,
}

impl KeybindingRegistry {
    /// Load keybindings from the config file.
    pub fn load() -> Self {
        let mut registry = Self {
            bindings: HashMap::new(),
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
                        registry.bindings.insert(binding.key.clone(), binding);
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
    dirs::config_dir().map(|d| d.join("agent-code").join("keybindings.json"))
}

fn load_keybindings_file(path: &PathBuf) -> Result<Vec<Keybinding>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("Read error: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Parse error: {e}"))
}
