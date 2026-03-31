//! Configuration system.
//!
//! Configuration is loaded from multiple sources with the following
//! priority (highest to lowest):
//!
//! 1. CLI flags and environment variables
//! 2. Project-local settings (`.rc/settings.toml`)
//! 3. User settings (`~/.config/rust-code/config.toml`)
//!
//! Each layer is merged into the final `Config` struct.

mod schema;

pub use schema::*;

use crate::error::ConfigError;
use std::path::{Path, PathBuf};

impl Config {
    /// Load configuration from all sources, merging by priority.
    pub fn load() -> Result<Config, ConfigError> {
        let mut config = Config::default();

        // Layer 1: User-level config.
        if let Some(path) = user_config_path() {
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| ConfigError::FileError(format!("{path:?}: {e}")))?;
                let user_config: Config = toml::from_str(&content)?;
                config.merge(user_config);
            }
        }

        // Layer 2: Project-level config (walk up from cwd).
        if let Some(path) = find_project_config() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| ConfigError::FileError(format!("{path:?}: {e}")))?;
            let project_config: Config = toml::from_str(&content)?;
            config.merge(project_config);
        }

        // Layer 3: Environment variables (applied in CLI parsing).

        Ok(config)
    }

    /// Merge another config into this one. Non-default values from `other`
    /// overwrite values in `self`.
    fn merge(&mut self, other: Config) {
        if !other.api.base_url.is_empty() {
            self.api.base_url = other.api.base_url;
        }
        if !other.api.model.is_empty() {
            self.api.model = other.api.model;
        }
        if other.api.api_key.is_some() {
            self.api.api_key = other.api.api_key;
        }
        if other.api.max_output_tokens.is_some() {
            self.api.max_output_tokens = other.api.max_output_tokens;
        }
        if other.permissions.default_mode != PermissionMode::Ask {
            self.permissions.default_mode = other.permissions.default_mode;
        }
        if !other.permissions.rules.is_empty() {
            self.permissions.rules.extend(other.permissions.rules);
        }
    }
}

/// Returns the user-level config file path.
fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("rust-code").join("config.toml"))
}

/// Walk up from the current directory to find `.rc/settings.toml`.
fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_config_in_ancestors(&cwd)
}

fn find_config_in_ancestors(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".rc").join("settings.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}
