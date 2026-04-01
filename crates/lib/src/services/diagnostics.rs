//! Environment diagnostics.
//!
//! Comprehensive checks for the agent's runtime environment:
//! tools, configuration, connectivity, and permissions.

use std::path::Path;

/// Result of a single diagnostic check.
#[derive(Debug)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl Check {
    fn pass(name: &str, detail: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Pass,
            detail: detail.to_string(),
        }
    }
    fn warn(name: &str, detail: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Warn,
            detail: detail.to_string(),
        }
    }
    fn fail(name: &str, detail: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Fail,
            detail: detail.to_string(),
        }
    }

    pub fn symbol(&self) -> &str {
        match self.status {
            CheckStatus::Pass => "ok",
            CheckStatus::Warn => "!?",
            CheckStatus::Fail => "xx",
        }
    }
}

/// Run all diagnostic checks and return results.
pub async fn run_all(cwd: &Path, config: &crate::config::Config) -> Vec<Check> {
    let mut checks = Vec::new();

    // 1. Required CLI tools.
    for (tool, purpose) in &[
        ("git", "version control"),
        ("rg", "content search (ripgrep)"),
        ("bash", "shell execution"),
    ] {
        let available = tokio::process::Command::new("which")
            .arg(tool)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if available {
            checks.push(Check::pass(
                &format!("tool:{tool}"),
                &format!("{tool} found ({purpose})"),
            ));
        } else {
            checks.push(Check::fail(
                &format!("tool:{tool}"),
                &format!("{tool} not found — needed for {purpose}"),
            ));
        }
    }

    // 2. Optional tools.
    for (tool, purpose) in &[
        ("node", "JavaScript execution"),
        ("python3", "Python execution"),
        ("cargo", "Rust toolchain"),
    ] {
        let available = tokio::process::Command::new("which")
            .arg(tool)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if available {
            checks.push(Check::pass(
                &format!("tool:{tool}"),
                &format!("{tool} available ({purpose})"),
            ));
        } else {
            checks.push(Check::warn(
                &format!("tool:{tool}"),
                &format!("{tool} not found — optional, for {purpose}"),
            ));
        }
    }

    // 3. API configuration.
    if config.api.api_key.is_some() {
        checks.push(Check::pass("config:api_key", "API key configured"));
    } else {
        checks.push(Check::fail(
            "config:api_key",
            "No API key set (AGENT_CODE_API_KEY or --api-key)",
        ));
    }

    checks.push(Check::pass(
        "config:model",
        &format!("Model: {}", config.api.model),
    ));

    checks.push(Check::pass(
        "config:base_url",
        &format!("API endpoint: {}", config.api.base_url),
    ));

    // 4. Git repository.
    if crate::services::git::is_git_repo(cwd).await {
        let branch = crate::services::git::current_branch(cwd)
            .await
            .unwrap_or_else(|| "(detached HEAD)".to_string());
        checks.push(Check::pass(
            "git:repo",
            &format!("Git repository on branch '{branch}'"),
        ));
    } else {
        checks.push(Check::warn("git:repo", "Not inside a git repository"));
    }

    // 5. Config file locations.
    let user_config = dirs::config_dir().map(|d| d.join("agent-code").join("config.toml"));
    if let Some(ref path) = user_config {
        if path.exists() {
            checks.push(Check::pass(
                "config:user_file",
                &format!("User config: {}", path.display()),
            ));
        } else {
            checks.push(Check::warn(
                "config:user_file",
                &format!("No user config at {}", path.display()),
            ));
        }
    }

    let project_config = cwd.join(".agent").join("settings.toml");
    if project_config.exists() {
        checks.push(Check::pass(
            "config:project_file",
            &format!("Project config: {}", project_config.display()),
        ));
    }

    // 6. MCP servers.
    let mcp_count = config.mcp_servers.len();
    if mcp_count > 0 {
        checks.push(Check::pass(
            "mcp:servers",
            &format!("{mcp_count} MCP server(s) configured"),
        ));
    }

    // 7. API connectivity test.
    if config.api.api_key.is_some() {
        let url = format!("{}/models", config.api.base_url);
        match reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .header(
                "Authorization",
                format!("Bearer {}", config.api.api_key.as_deref().unwrap_or("")),
            )
            .header("x-api-key", config.api.api_key.as_deref().unwrap_or(""))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() || status.as_u16() == 200 {
                    checks.push(Check::pass(
                        "api:connectivity",
                        &format!("API reachable ({})", config.api.base_url),
                    ));
                } else if status.as_u16() == 401 || status.as_u16() == 403 {
                    checks.push(Check::fail(
                        "api:connectivity",
                        &format!("API key rejected (HTTP {})", status.as_u16()),
                    ));
                } else {
                    checks.push(Check::warn(
                        "api:connectivity",
                        &format!("API responded with HTTP {}", status.as_u16()),
                    ));
                }
            }
            Err(e) => {
                let msg = if e.is_timeout() {
                    "API unreachable (timeout after 5s)".to_string()
                } else if e.is_connect() {
                    format!("Cannot connect to {}", config.api.base_url)
                } else {
                    format!("API error: {e}")
                };
                checks.push(Check::fail("api:connectivity", &msg));
            }
        }
    }

    // 8. MCP server health check.
    for (name, entry) in &config.mcp_servers {
        if let Some(ref cmd) = entry.command {
            // Check if the command binary exists.
            let binary = cmd.split_whitespace().next().unwrap_or(cmd);
            if let Ok(output) = tokio::process::Command::new("which")
                .arg(binary)
                .output()
                .await
            {
                if output.status.success() {
                    checks.push(Check::pass(
                        &format!("mcp:{name}"),
                        &format!("MCP server '{name}' binary found: {binary}"),
                    ));
                } else {
                    checks.push(Check::fail(
                        &format!("mcp:{name}"),
                        &format!("MCP server '{name}' binary not found: {binary}"),
                    ));
                }
            }
        } else if let Some(ref url) = entry.url {
            // Check if the SSE endpoint is reachable.
            match reqwest::Client::new()
                .get(url)
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await
            {
                Ok(_) => {
                    checks.push(Check::pass(
                        &format!("mcp:{name}"),
                        &format!("MCP server '{name}' reachable at {url}"),
                    ));
                }
                Err(_) => {
                    checks.push(Check::fail(
                        &format!("mcp:{name}"),
                        &format!("MCP server '{name}' unreachable at {url}"),
                    ));
                }
            }
        }
    }

    // 9. Disk space (warn if < 1GB free).
    // Simple check via df.
    if let Ok(output) = tokio::process::Command::new("df")
        .args(["-BG", "."])
        .current_dir(cwd)
        .output()
        .await
    {
        let text = String::from_utf8_lossy(&output.stdout);
        // Parse the "Available" column from df output.
        if let Some(line) = text.lines().nth(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(avail) = parts.get(3) {
                let gb: f64 = avail.trim_end_matches('G').parse().unwrap_or(999.0);
                if gb < 1.0 {
                    checks.push(Check::warn(
                        "disk:space",
                        &format!("Low disk space: {avail} available"),
                    ));
                }
            }
        }
    }

    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_constructors() {
        let p = Check::pass("test", "ok");
        assert_eq!(p.status, CheckStatus::Pass);
        assert_eq!(p.symbol(), "ok");

        let w = Check::warn("test", "warning");
        assert_eq!(w.status, CheckStatus::Warn);
        assert_eq!(w.symbol(), "!?");

        let f = Check::fail("test", "failed");
        assert_eq!(f.status, CheckStatus::Fail);
        assert_eq!(f.symbol(), "xx");
    }

    #[test]
    fn test_check_fields() {
        let c = Check::pass("git:repo", "Git repository on branch 'main'");
        assert_eq!(c.name, "git:repo");
        assert!(c.detail.contains("main"));
    }

    #[tokio::test]
    async fn test_run_all_returns_checks() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let checks = run_all(dir.path(), &config).await;

        // Should always return at least a few checks.
        assert!(checks.len() >= 3);

        // Should always check for git.
        assert!(checks.iter().any(|c| c.name.starts_with("tool:")));
    }

    #[tokio::test]
    async fn test_run_all_in_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        tokio::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let config = crate::config::Config::default();
        let checks = run_all(dir.path(), &config).await;

        let git_check = checks.iter().find(|c| c.name == "git:repo");
        assert!(git_check.is_some());
        assert_eq!(git_check.unwrap().status, CheckStatus::Pass);
    }

    #[tokio::test]
    async fn test_run_all_no_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.api.api_key = None;

        let checks = run_all(dir.path(), &config).await;

        let api_check = checks.iter().find(|c| c.name == "config:api_key");
        assert!(api_check.is_some());
        assert_eq!(api_check.unwrap().status, CheckStatus::Fail);
    }

    #[tokio::test]
    async fn test_run_all_with_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.api.api_key = Some("test-key".to_string());

        let checks = run_all(dir.path(), &config).await;

        let api_check = checks.iter().find(|c| c.name == "config:api_key");
        assert!(api_check.is_some());
        assert_eq!(api_check.unwrap().status, CheckStatus::Pass);
    }

    #[tokio::test]
    async fn test_run_all_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.mcp_servers.insert(
            "test-server".to_string(),
            crate::config::McpServerEntry {
                command: Some("nonexistent-binary-xyz".to_string()),
                args: vec![],
                url: None,
                env: std::collections::HashMap::new(),
            },
        );

        let checks = run_all(dir.path(), &config).await;

        // Should have a check for the MCP server.
        let mcp_check = checks.iter().find(|c| c.name == "mcp:test-server");
        assert!(mcp_check.is_some());
        // The binary won't exist, so it should fail.
        assert_eq!(mcp_check.unwrap().status, CheckStatus::Fail);
    }
}
