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

    let project_config = cwd.join(".rc").join("settings.toml");
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

    // 7. Disk space (warn if < 1GB free).
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
