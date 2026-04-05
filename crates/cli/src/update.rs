//! Self-update mechanism.
//!
//! Checks GitHub releases for newer versions and optionally downloads
//! the update. Update checks are throttled to once per 24 hours.

use std::path::PathBuf;

use tracing::debug;

const REPO: &str = "avala-ai/agent-code";
const CHECK_INTERVAL_SECS: u64 = 86_400; // 24 hours

/// Result of an update check.
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub is_newer: bool,
    pub release_url: String,
}

/// Check if a newer version is available on GitHub.
///
/// Returns `None` if the check was skipped (too recent) or failed.
pub async fn check_for_update() -> Option<UpdateCheck> {
    // Throttle: only check once per 24 hours.
    if !should_check() {
        debug!("Update check skipped (checked recently)");
        return None;
    }

    let current = env!("CARGO_PKG_VERSION").to_string();
    let latest = fetch_latest_version().await?;

    record_check();

    let is_newer = version_is_newer(&latest, &current);
    let release_url = format!("https://github.com/{REPO}/releases/tag/v{latest}");

    Some(UpdateCheck {
        current,
        latest,
        is_newer,
        release_url,
    })
}

/// Print a notification if a newer version is available.
pub fn print_update_hint(check: &UpdateCheck) {
    if check.is_newer {
        eprintln!(
            "\n  Update available: v{} → v{}\n  {}\n",
            check.current, check.latest, check.release_url,
        );
    }
}

/// Fetch the latest release version from GitHub API.
async fn fetch_latest_version() -> Option<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");

    let client = reqwest::Client::builder()
        .user_agent("agent-code-update-check")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    let response = client.get(&url).send().await.ok()?;

    if !response.status().is_success() {
        debug!("GitHub API returned {}", response.status());
        return None;
    }

    let json: serde_json::Value = response.json().await.ok()?;
    let tag = json.get("tag_name")?.as_str()?;

    // Strip leading 'v' from tag.
    Some(tag.trim_start_matches('v').to_string())
}

/// Compare two semver strings. Returns true if `latest` > `current`.
fn version_is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = v.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };

    parse(latest) > parse(current)
}

/// Path to the timestamp file that tracks when we last checked.
fn check_timestamp_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("agent-code").join("last-update-check"))
}

/// Whether enough time has passed since the last check.
fn should_check() -> bool {
    let Some(path) = check_timestamp_path() else {
        return true;
    };

    let Ok(content) = std::fs::read_to_string(&path) else {
        return true;
    };

    let Ok(timestamp) = content.trim().parse::<u64>() else {
        return true;
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    now.saturating_sub(timestamp) >= CHECK_INTERVAL_SECS
}

/// Record that we just checked.
fn record_check() {
    let Some(path) = check_timestamp_path() else {
        return;
    };

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default();

    let _ = std::fs::write(&path, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_is_newer() {
        assert!(version_is_newer("0.11.0", "0.10.0"));
        assert!(version_is_newer("1.0.0", "0.10.0"));
        assert!(version_is_newer("0.10.1", "0.10.0"));
        assert!(!version_is_newer("0.10.0", "0.10.0"));
        assert!(!version_is_newer("0.9.7", "0.10.0"));
    }

    #[test]
    fn test_version_is_newer_major() {
        assert!(version_is_newer("2.0.0", "1.99.99"));
    }
}
