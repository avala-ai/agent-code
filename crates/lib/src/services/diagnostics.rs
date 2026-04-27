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
    if config.api.auth_mode == crate::config::ApiAuthMode::CodexChatgpt {
        checks.push(Check::pass("config:auth", "Codex ChatGPT auth configured"));
    } else if config.api.api_key.is_some() {
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
    //
    // User-level config is always a fixed path. Project configs
    // are resolved via the same walk-up logic the loader uses, so
    // a sub-crate shelled into a monorepo sees the repo-root
    // settings.toml (not just files sitting in the cwd).
    let user_config = crate::config::user_config_path();
    if let Some(ref path) = user_config {
        if path.exists() {
            match toml::from_str::<toml::Value>(&std::fs::read_to_string(path).unwrap_or_default())
            {
                Ok(value) => {
                    let unknown = collect_unknown_config_keys(&value);
                    if unknown.is_empty() {
                        checks.push(Check::pass(
                            "config:user_file",
                            &format!("User config: {}", path.display()),
                        ));
                    } else {
                        checks.push(Check::warn(
                            "config:user_file",
                            &format!(
                                "User config {} has unknown key(s): {}",
                                path.display(),
                                unknown.join(", ")
                            ),
                        ));
                    }
                }
                Err(e) => {
                    checks.push(Check::fail(
                        "config:user_file",
                        &format!("User config {} failed to parse: {e}", path.display()),
                    ));
                }
            }
        } else {
            checks.push(Check::warn(
                "config:user_file",
                &format!("No user config at {}", path.display()),
            ));
        }
    }

    if let Some(project_config) = crate::config::find_project_config() {
        match toml::from_str::<toml::Value>(
            &std::fs::read_to_string(&project_config).unwrap_or_default(),
        ) {
            Ok(value) => {
                let unknown = collect_unknown_config_keys(&value);
                if unknown.is_empty() {
                    checks.push(Check::pass(
                        "config:project_file",
                        &format!("Project config: {}", project_config.display()),
                    ));
                } else {
                    checks.push(Check::warn(
                        "config:project_file",
                        &format!(
                            "Project config {} has unknown key(s): {}",
                            project_config.display(),
                            unknown.join(", ")
                        ),
                    ));
                }
            }
            Err(e) => {
                checks.push(Check::fail(
                    "config:project_file",
                    &format!(
                        "Project config {} failed to parse: {e}",
                        project_config.display()
                    ),
                ));
            }
        }
    }

    if let Some(local_config) = crate::config::find_project_local_config() {
        match toml::from_str::<toml::Value>(
            &std::fs::read_to_string(&local_config).unwrap_or_default(),
        ) {
            Ok(value) => {
                let unknown = collect_unknown_config_keys(&value);
                if unknown.is_empty() {
                    checks.push(Check::pass(
                        "config:project_local_file",
                        &format!("Project local overrides: {}", local_config.display()),
                    ));
                } else {
                    checks.push(Check::warn(
                        "config:project_local_file",
                        &format!(
                            "Project local overrides {} has unknown key(s): {}",
                            local_config.display(),
                            unknown.join(", ")
                        ),
                    ));
                }
            }
            Err(e) => {
                checks.push(Check::fail(
                    "config:project_local_file",
                    &format!(
                        "Project local overrides {} failed to parse: {e}",
                        local_config.display()
                    ),
                ));
            }
        }
    }

    // Warn when multiple provider API keys are set — the resolver
    // picks the highest-priority one deterministically but the user
    // might not realize the others are being ignored.
    let present_keys = [
        "AGENT_CODE_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "XAI_API_KEY",
        "GOOGLE_API_KEY",
        "DEEPSEEK_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "TOGETHER_API_KEY",
        "OPENROUTER_API_KEY",
        "COHERE_API_KEY",
        "PERPLEXITY_API_KEY",
    ]
    .into_iter()
    .filter(|k| std::env::var(k).is_ok())
    .collect::<Vec<_>>();
    if present_keys.len() > 1 {
        checks.push(Check::warn(
            "env:api_key_precedence",
            &format!(
                "Multiple API-key env vars set: {}. Resolver uses the first — others ignored.",
                present_keys.join(", ")
            ),
        ));
    }

    // Session cleanup policy. Surface it so operators can tell at a
    // glance whether old sessions are being pruned on startup.
    match config.session.cleanup_period_days {
        Some(days) if days > 0 => {
            checks.push(Check::pass(
                "session:cleanup_period_days",
                &format!("Session cleanup: prune files older than {days} days"),
            ));
        }
        _ => {
            checks.push(Check::warn(
                "session:cleanup_period_days",
                "Session cleanup disabled — set [session] cleanup_period_days to prune old sessions",
            ));
        }
    }

    // 6. MCP servers.
    let mcp_count = config.mcp_servers.len();
    if mcp_count > 0 {
        checks.push(Check::pass(
            "mcp:servers",
            &format!("{mcp_count} MCP server(s) configured"),
        ));
    }

    // 7. Provider detection and health check.
    let provider_kind =
        crate::llm::provider::detect_provider(&config.api.model, &config.api.base_url);
    checks.push(Check::pass(
        "provider:detected",
        &format!("Provider: {provider_kind:?}"),
    ));

    if let Some(expected_env) = match provider_kind {
        crate::llm::provider::ProviderKind::AzureOpenAi => Some("AZURE_OPENAI_API_KEY"),
        crate::llm::provider::ProviderKind::Bedrock => Some("AWS_REGION"),
        crate::llm::provider::ProviderKind::Vertex => Some("GOOGLE_CLOUD_PROJECT"),
        _ => None,
    } {
        if std::env::var(expected_env).is_ok() {
            checks.push(Check::pass(
                "provider:env",
                &format!("{expected_env} is set"),
            ));
        } else {
            checks.push(Check::warn(
                "provider:env",
                &format!("{expected_env} not set (may be needed for {provider_kind:?})"),
            ));
        }
    }

    // 8. API connectivity test (provider-aware auth).
    if config.api.auth_mode == crate::config::ApiAuthMode::CodexChatgpt {
        checks.push(Check::warn(
            "api:connectivity",
            "Skipping /models connectivity check for Codex ChatGPT auth",
        ));
    } else if config.api.api_key.is_some() {
        let api_key = config.api.api_key.as_deref().unwrap_or("");
        let url = format!("{}/models", config.api.base_url);

        let client = reqwest::Client::new();
        let mut request = client.get(&url).timeout(std::time::Duration::from_secs(5));

        // Use provider-specific auth headers.
        match provider_kind {
            crate::llm::provider::ProviderKind::AzureOpenAi => {
                request = request.header("api-key", api_key);
            }
            crate::llm::provider::ProviderKind::Anthropic
            | crate::llm::provider::ProviderKind::Bedrock
            | crate::llm::provider::ProviderKind::Vertex => {
                request = request
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01");
            }
            _ => {
                request = request.header("Authorization", format!("Bearer {api_key}"));
            }
        }

        match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() || status.as_u16() == 200 {
                    checks.push(Check::pass(
                        "api:connectivity",
                        &format!(
                            "API reachable ({:?} at {})",
                            provider_kind, config.api.base_url
                        ),
                    ));
                } else if status.as_u16() == 401 || status.as_u16() == 403 {
                    checks.push(Check::fail(
                        "api:connectivity",
                        &format!(
                            "API key rejected by {:?} (HTTP {})",
                            provider_kind,
                            status.as_u16()
                        ),
                    ));
                } else {
                    checks.push(Check::warn(
                        "api:connectivity",
                        &format!(
                            "{:?} responded with HTTP {}",
                            provider_kind,
                            status.as_u16()
                        ),
                    ));
                }
            }
            Err(e) => {
                let msg = if e.is_timeout() {
                    format!("{:?} unreachable (timeout after 5s)", provider_kind)
                } else if e.is_connect() {
                    format!(
                        "Cannot connect to {:?} at {}",
                        provider_kind, config.api.base_url
                    )
                } else {
                    format!("{:?} error: {e}", provider_kind)
                };
                checks.push(Check::fail("api:connectivity", &msg));
            }
        }
    }

    // 9. MCP server health check.
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

    // 10. Disk space (warn if < 1GB free).
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

    // Hook configuration validation.
    //
    // Catches broken hooks early — a malformed URL or empty command
    // would otherwise silently fire and do nothing (shell hooks fail
    // their subprocess; HTTP hooks fail their request), leaving the
    // user to discover it the hard way when the expected side-effect
    // doesn't happen. Emits one pass line per N valid hooks, plus one
    // fail per broken entry.
    if !config.hooks.is_empty() {
        let mut broken = 0usize;
        for (i, hook) in config.hooks.iter().enumerate() {
            match &hook.action {
                crate::config::HookAction::Shell { command } => {
                    if command.trim().is_empty() {
                        checks.push(Check::fail(
                            &format!("hooks:entry:{i}"),
                            &format!("Hook #{i} ({:?}): shell command is empty", hook.event),
                        ));
                        broken += 1;
                    }
                }
                crate::config::HookAction::Http { url, method } => {
                    if let Err(e) = reqwest::Url::parse(url) {
                        checks.push(Check::fail(
                            &format!("hooks:entry:{i}"),
                            &format!(
                                "Hook #{i} ({:?}): url {url:?} is malformed: {e}",
                                hook.event
                            ),
                        ));
                        broken += 1;
                    } else if let Some(m) = method
                        && !is_valid_http_method(m)
                    {
                        checks.push(Check::warn(
                            &format!("hooks:entry:{i}"),
                            &format!(
                                "Hook #{i} ({:?}): method {m:?} is not a recognized HTTP verb",
                                hook.event
                            ),
                        ));
                    }
                }
            }
        }
        let total = config.hooks.len();
        let ok = total - broken;
        if broken == 0 {
            checks.push(Check::pass(
                "hooks:count",
                &format!("{total} hook(s) configured, all valid"),
            ));
        } else {
            checks.push(Check::fail(
                "hooks:count",
                &format!("{ok}/{total} hook(s) valid ({broken} broken)"),
            ));
        }
    }

    checks
}

/// True when the given string is one of the HTTP verbs the hook
/// dispatcher actually recognizes. Anything else gets treated as POST
/// at runtime, which is almost certainly wrong — warn early.
fn is_valid_http_method(m: &str) -> bool {
    matches!(
        m.to_ascii_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
    )
}

/// Top-level TOML table names that the config loader actually reads.
///
/// The loader's struct uses `#[serde(default)]` which silently ignores
/// unknown fields. That's the right choice at runtime (forwards-compat
/// with newer tools writing a shared file), but it turns typos into
/// silent no-ops. `/doctor` uses this allow-list to call them out.
const KNOWN_CONFIG_SECTIONS: &[&str] = &[
    "api",
    "ui",
    "permissions",
    "features",
    "security",
    "sandbox",
    "mcp_servers",
    "hooks",
    "subagents",
    "allowed_project_dirs",
];

fn collect_unknown_config_keys(value: &toml::Value) -> Vec<String> {
    let toml::Value::Table(root) = value else {
        return Vec::new();
    };
    let mut unknown: Vec<String> = root
        .keys()
        .filter(|k| !KNOWN_CONFIG_SECTIONS.contains(&k.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    unknown
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
    async fn test_run_all_includes_provider_check() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.api.base_url = "https://api.openai.com/v1".to_string();
        config.api.model = "gpt-5.4".to_string();

        let checks = run_all(dir.path(), &config).await;

        let provider_check = checks.iter().find(|c| c.name == "provider:detected");
        assert!(provider_check.is_some());
        assert_eq!(provider_check.unwrap().status, CheckStatus::Pass);
        assert!(provider_check.unwrap().detail.contains("OpenAi"));
    }

    #[tokio::test]
    async fn test_run_all_azure_provider_env_check() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.api.base_url =
            "https://myresource.openai.azure.com/openai/deployments/gpt-4".to_string();

        let checks = run_all(dir.path(), &config).await;

        let provider_check = checks.iter().find(|c| c.name == "provider:detected");
        assert!(provider_check.is_some());
        assert!(provider_check.unwrap().detail.contains("AzureOpenAi"));

        // Should have a provider:env check for AZURE_OPENAI_API_KEY.
        let env_check = checks.iter().find(|c| c.name == "provider:env");
        assert!(env_check.is_some());
    }

    #[test]
    fn is_valid_http_method_accepts_common_verbs() {
        assert!(is_valid_http_method("GET"));
        assert!(is_valid_http_method("POST"));
        assert!(is_valid_http_method("PUT"));
        assert!(is_valid_http_method("PATCH"));
        assert!(is_valid_http_method("DELETE"));
        assert!(is_valid_http_method("get")); // case-insensitive
    }

    #[test]
    fn is_valid_http_method_rejects_typos() {
        assert!(!is_valid_http_method("GETS"));
        assert!(!is_valid_http_method("HEAD")); // not in our dispatcher
        assert!(!is_valid_http_method(""));
        assert!(!is_valid_http_method("POSTT"));
    }

    #[tokio::test]
    async fn test_hook_validation_skipped_when_no_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let checks = run_all(dir.path(), &config).await;
        assert!(
            !checks.iter().any(|c| c.name.starts_with("hooks:")),
            "no hook checks should fire when config.hooks is empty"
        );
    }

    #[tokio::test]
    async fn test_hook_validation_passes_all_valid_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.hooks.push(crate::config::HookDefinition {
            event: crate::config::HookEvent::SessionStart,
            tool_name: None,
            action: crate::config::HookAction::Shell {
                command: "echo ok".into(),
            },
        });
        config.hooks.push(crate::config::HookDefinition {
            event: crate::config::HookEvent::PostToolUse,
            tool_name: None,
            action: crate::config::HookAction::Http {
                url: "https://example.com/audit".into(),
                method: Some("POST".into()),
            },
        });
        let checks = run_all(dir.path(), &config).await;
        let count = checks.iter().find(|c| c.name == "hooks:count").unwrap();
        assert_eq!(count.status, CheckStatus::Pass);
        assert!(count.detail.contains("2 hook(s)"));
    }

    #[tokio::test]
    async fn test_hook_validation_flags_empty_shell_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.hooks.push(crate::config::HookDefinition {
            event: crate::config::HookEvent::SessionStart,
            tool_name: None,
            action: crate::config::HookAction::Shell {
                command: "   ".into(),
            },
        });
        let checks = run_all(dir.path(), &config).await;
        let entry = checks
            .iter()
            .find(|c| c.name == "hooks:entry:0")
            .expect("broken hook should have an entry check");
        assert_eq!(entry.status, CheckStatus::Fail);
        assert!(entry.detail.contains("empty"));

        let count = checks.iter().find(|c| c.name == "hooks:count").unwrap();
        assert_eq!(count.status, CheckStatus::Fail);
    }

    #[tokio::test]
    async fn test_hook_validation_flags_malformed_url() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.hooks.push(crate::config::HookDefinition {
            event: crate::config::HookEvent::PostToolUse,
            tool_name: None,
            action: crate::config::HookAction::Http {
                url: "not a url".into(),
                method: None,
            },
        });
        let checks = run_all(dir.path(), &config).await;
        let entry = checks
            .iter()
            .find(|c| c.name == "hooks:entry:0")
            .expect("broken hook should have an entry check");
        assert_eq!(entry.status, CheckStatus::Fail);
        assert!(entry.detail.contains("malformed"));
    }

    #[tokio::test]
    async fn test_hook_validation_warns_on_unknown_http_method() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.hooks.push(crate::config::HookDefinition {
            event: crate::config::HookEvent::PostToolUse,
            tool_name: None,
            action: crate::config::HookAction::Http {
                url: "https://example.com/hook".into(),
                method: Some("FETCH".into()), // invalid verb
            },
        });
        let checks = run_all(dir.path(), &config).await;
        let entry = checks
            .iter()
            .find(|c| c.name == "hooks:entry:0")
            .expect("bad method should surface as a warn entry");
        assert_eq!(entry.status, CheckStatus::Warn);
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

    // ---- collect_unknown_config_keys ----

    fn parse(body: &str) -> toml::Value {
        toml::from_str(body).unwrap()
    }

    #[test]
    fn unknown_config_keys_empty_when_all_sections_known() {
        let v = parse(
            r#"
[api]
model = "x"
[ui]
theme = "dark"
[permissions]
default_mode = "ask"
"#,
        );
        assert!(collect_unknown_config_keys(&v).is_empty());
    }

    #[test]
    fn unknown_config_keys_flags_typo_section() {
        let v = parse(
            r#"
[api]
model = "x"
[permisions]    # typo: should be `permissions`
default_mode = "ask"
"#,
        );
        let unknown = collect_unknown_config_keys(&v);
        assert_eq!(unknown, vec!["permisions".to_string()]);
    }

    #[test]
    fn unknown_config_keys_are_sorted_and_complete() {
        let v = parse(
            r#"
[api]
model = "x"
[zzz_nope]
a = 1
[aaa_nope]
b = 2
"#,
        );
        let unknown = collect_unknown_config_keys(&v);
        assert_eq!(unknown, vec!["aaa_nope", "zzz_nope"]);
    }

    #[test]
    fn unknown_config_keys_accepts_all_sections_the_loader_reads() {
        // Build a TOML body that declares every section in the
        // known-sections list so this test will fail the moment the
        // loader grows a new section and we forget to teach /doctor
        // about it.
        let body = KNOWN_CONFIG_SECTIONS
            .iter()
            .map(|s| format!("[{s}]"))
            .collect::<Vec<_>>()
            .join("\n");
        let v = parse(&body);
        assert!(
            collect_unknown_config_keys(&v).is_empty(),
            "a section in the allow-list is being reported as unknown"
        );
    }

    #[test]
    fn unknown_config_keys_noop_on_non_table_root() {
        let v: toml::Value = "42".parse().unwrap();
        assert!(collect_unknown_config_keys(&v).is_empty());
    }
}
