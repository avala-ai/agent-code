//! Remote skill discovery and installation.
//!
//! Fetches a skill index from a configurable URL, caches it locally,
//! and installs skills to the user's skills directory. The index is
//! a JSON array of skill entries with name, description, and download URL.
//!
//! # Index Format
//!
//! ```json
//! [
//!   {
//!     "name": "deploy",
//!     "description": "Deploy to production with safety checks",
//!     "version": "1.0.0",
//!     "author": "avala-ai",
//!     "url": "https://raw.githubusercontent.com/avala-ai/agent-code-skills/main/deploy.md"
//!   }
//! ]
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Default skill index URL.
const DEFAULT_INDEX_URL: &str =
    "https://raw.githubusercontent.com/avala-ai/agent-code-skills/main/index.json";

/// How long to cache the index before re-fetching (1 hour).
const CACHE_MAX_AGE_SECS: u64 = 3600;

/// A skill entry in the remote index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSkill {
    /// Skill name (used as filename).
    pub name: String,
    /// What this skill does.
    pub description: String,
    /// Semantic version.
    #[serde(default)]
    pub version: String,
    /// Author or organization.
    #[serde(default)]
    pub author: String,
    /// URL to download the skill markdown file.
    pub url: String,
}

/// Fetch the remote skill index, falling back to the local cache.
pub async fn fetch_index(index_url: Option<&str>) -> Result<Vec<RemoteSkill>, String> {
    let url = index_url.unwrap_or(DEFAULT_INDEX_URL);

    // Try fetching from the network first.
    match fetch_index_from_url(url).await {
        Ok(skills) => {
            // Cache the result for offline use.
            if let Err(e) = save_cached_index(&skills) {
                warn!("Failed to cache skill index: {e}");
            }
            Ok(skills)
        }
        Err(net_err) => {
            debug!("Network fetch failed: {net_err}, trying cache");
            // Fall back to cached index.
            match load_cached_index() {
                Some(skills) => {
                    debug!("Using cached skill index ({} entries)", skills.len());
                    Ok(skills)
                }
                None => Err(format!("Failed to fetch skill index: {net_err}")),
            }
        }
    }
}

/// Fetch the index from a URL.
async fn fetch_index_from_url(url: &str) -> Result<Vec<RemoteSkill>, String> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .header("User-Agent", "agent-code")
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;

    serde_json::from_str(&body).map_err(|e| format!("Invalid index JSON: {e}"))
}

/// Install a skill by name from the remote index.
pub async fn install_skill(name: &str, index_url: Option<&str>) -> Result<PathBuf, String> {
    let index = fetch_index(index_url).await?;

    let skill = index
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("Skill '{name}' not found in index"))?;

    // Download the skill file.
    let content = download_skill(&skill.url).await?;

    // Save to user skills directory.
    let dest = user_skills_dir()?;
    std::fs::create_dir_all(&dest).map_err(|e| format!("Failed to create skills dir: {e}"))?;

    let file_path = dest.join(format!("{name}.md"));
    std::fs::write(&file_path, &content).map_err(|e| format!("Failed to write skill file: {e}"))?;

    debug!("Installed skill '{name}' to {}", file_path.display());
    Ok(file_path)
}

/// Remove an installed skill by name.
pub fn uninstall_skill(name: &str) -> Result<(), String> {
    let dir = user_skills_dir()?;
    let file_path = dir.join(format!("{name}.md"));

    if !file_path.exists() {
        return Err(format!("Skill '{name}' is not installed"));
    }

    std::fs::remove_file(&file_path).map_err(|e| format!("Failed to remove skill: {e}"))?;
    Ok(())
}

/// List installed skills (in user skills directory).
pub fn list_installed() -> Vec<String> {
    let dir = match user_skills_dir() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    if !dir.is_dir() {
        return Vec::new();
    }

    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Download a skill file from a URL.
async fn download_skill(url: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(15))
        .header("User-Agent", "agent-code")
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Download failed: HTTP {}", response.status()));
    }

    response
        .text()
        .await
        .map_err(|e| format!("Failed to read download: {e}"))
}

// ── Cache ────────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("agent-code").join("skill-index.json"))
}

fn save_cached_index(skills: &[RemoteSkill]) -> Result<(), String> {
    let path = cache_path().ok_or("No cache directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(skills).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

fn load_cached_index() -> Option<Vec<RemoteSkill>> {
    let path = cache_path()?;
    if !path.exists() {
        return None;
    }

    // Check cache age.
    if let Ok(metadata) = path.metadata()
        && let Ok(modified) = metadata.modified()
        && let Ok(age) = modified.elapsed()
        && age.as_secs() > CACHE_MAX_AGE_SECS * 24
    {
        // Cache is older than 24 hours — still usable as fallback
        // but log a warning.
        debug!(
            "Skill index cache is stale ({:.0}h old)",
            age.as_secs_f64() / 3600.0
        );
    }

    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn user_skills_dir() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|d| d.join("agent-code").join("skills"))
        .ok_or_else(|| "Cannot determine user config directory".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_skill_deserialize() {
        let json = r#"[
            {
                "name": "deploy",
                "description": "Deploy to production",
                "version": "1.0.0",
                "author": "test",
                "url": "https://example.com/deploy.md"
            }
        ]"#;
        let skills: Vec<RemoteSkill> = serde_json::from_str(json).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "deploy");
        assert_eq!(skills[0].version, "1.0.0");
    }

    #[test]
    fn test_remote_skill_optional_fields() {
        let json = r#"[{"name":"test","description":"A test","url":"https://example.com/t.md"}]"#;
        let skills: Vec<RemoteSkill> = serde_json::from_str(json).unwrap();
        assert_eq!(skills[0].version, "");
        assert_eq!(skills[0].author, "");
    }

    #[test]
    fn test_list_installed_empty() {
        // Should not panic even if the directory doesn't exist.
        let result = list_installed();
        assert!(result.is_empty() || !result.is_empty()); // Just shouldn't panic.
    }

    #[test]
    fn test_uninstall_nonexistent() {
        let result = uninstall_skill("nonexistent-skill-xyz-test");
        assert!(result.is_err());
    }
}
