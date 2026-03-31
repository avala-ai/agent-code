//! Plugin system.
//!
//! Plugins bundle skills, commands, and configuration together as
//! installable packages. A plugin is a directory containing:
//!
//! - `plugin.toml` — metadata and configuration
//! - `skills/` — skill files to register
//! - `hooks/` — hook definitions
//!
//! Plugins are loaded from `~/.config/agent-code/plugins/` and
//! project-level `.rc/plugins/`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Plugin metadata from plugin.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    /// Skills provided by this plugin.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Hook definitions.
    #[serde(default)]
    pub hooks: Vec<PluginHook>,
    /// Configuration keys this plugin adds.
    #[serde(default)]
    pub config: std::collections::HashMap<String, serde_json::Value>,
}

/// A hook defined by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHook {
    pub event: String,
    pub command: String,
    pub tool_name: Option<String>,
}

/// A loaded plugin with its manifest and path.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: PluginManifest,
    pub path: PathBuf,
}

/// Registry of loaded plugins.
pub struct PluginRegistry {
    plugins: Vec<Plugin>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Load plugins from all configured directories.
    pub fn load_all(project_root: Option<&Path>) -> Self {
        let mut registry = Self::new();

        // User-level plugins.
        if let Some(dir) = user_plugin_dir() {
            registry.load_from_dir(&dir);
        }

        // Project-level plugins.
        if let Some(root) = project_root {
            registry.load_from_dir(&root.join(".rc").join("plugins"));
        }

        debug!("Loaded {} plugins", registry.plugins.len());
        registry
    }

    fn load_from_dir(&mut self, dir: &Path) {
        if !dir.is_dir() {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("plugin.toml");
            if !manifest_path.exists() {
                continue;
            }

            match load_plugin(&path) {
                Ok(plugin) => {
                    debug!(
                        "Loaded plugin '{}' from {}",
                        plugin.manifest.name,
                        path.display()
                    );
                    self.plugins.push(plugin);
                }
                Err(e) => {
                    warn!("Failed to load plugin at {}: {e}", path.display());
                }
            }
        }
    }

    /// Get all loaded plugins.
    pub fn all(&self) -> &[Plugin] {
        &self.plugins
    }

    /// Find a plugin by name.
    pub fn find(&self, name: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.manifest.name == name)
    }

    /// Get all skill directories from loaded plugins.
    pub fn skill_dirs(&self) -> Vec<PathBuf> {
        self.plugins
            .iter()
            .map(|p| p.path.join("skills"))
            .filter(|d| d.is_dir())
            .collect()
    }

    /// Get all hook definitions from loaded plugins.
    pub fn hooks(&self) -> Vec<&PluginHook> {
        self.plugins
            .iter()
            .flat_map(|p| &p.manifest.hooks)
            .collect()
    }
}

fn load_plugin(path: &Path) -> Result<Plugin, String> {
    let manifest_path = path.join("plugin.toml");
    let content =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("Read error: {e}"))?;

    let manifest: PluginManifest =
        toml::from_str(&content).map_err(|e| format!("Parse error: {e}"))?;

    Ok(Plugin {
        manifest,
        path: path.to_path_buf(),
    })
}

fn user_plugin_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("plugins"))
}
