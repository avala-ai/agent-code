//! First-run bootstrap and credentials.
//!
//! Interactive launch is a **single provider-choice screen** when no
//! credentials exist — not a silent pin to one vendor, and not a four-step
//! wizard. Env API keys win when present; `--provider` / `AGENT_CODE_PROVIDER`
//! pins the choice for scripts. Then OAuth or an env-key hint finishes
//! sign-in and the modern TUI starts.

use agent_code_lib::config::atomic::atomic_write_secret;
use crossterm::style::Stylize;

use super::selector::{SelectOption, select};

/// Check if a default config.toml should be created.
pub fn needs_setup() -> bool {
    let config_path = agent_code_lib::config::agent_config_dir().map(|d| d.join("config.toml"));
    match config_path {
        Some(path) => !path.exists(),
        None => true,
    }
}

/// Theme + permission defaults shared by every first-run path.
///
/// Endpoint and provider are **not** filled here — silent bootstrap used
/// to pin xAI, which configured users with no key for one vendor and
/// dropped them into its OAuth. Callers must set provider fields after
/// env detection or an explicit choice.
pub fn default_setup_result() -> SetupResult {
    SetupResult {
        api_key: String::new(),
        auth_mode: "api_key".into(),
        provider: String::new(),
        base_url: None,
        model: None,
        theme: "auto".into(),
        permission_mode: "accept_edits".into(),
    }
}

/// Write provider defaults when no config exists **and** an env API key
/// already names the provider. Silent — no UI.
///
/// Without a key we write nothing: the interactive path asks which
/// provider to use instead of inventing one. The key itself is never
/// persisted — the loader re-resolves it from the environment on every
/// launch.
pub fn ensure_default_config() {
    if !needs_setup() || env_selects_undescribable_endpoint() {
        return;
    }
    let Some(mut result) = try_env_credentials() else {
        return;
    };
    result.api_key = String::new();
    write_config_quiet(&result);
}

/// True when the environment already selects an endpoint the silent
/// bootstrap cannot describe: Azure/Bedrock/Vertex URLs are assembled
/// from their own env vars by `ApiConfig::default()`, and subscription
/// OAuth pins its own base_url/model at load time. A pinned provider
/// section would misroute the first session, so write nothing and let
/// env detection take over.
fn env_selects_undescribable_endpoint() -> bool {
    const CLOUD_VARS: &[&str] = &[
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_ENDPOINT",
        "AGENT_CODE_USE_BEDROCK",
        "AGENT_CODE_USE_VERTEX",
    ];
    if CLOUD_VARS
        .iter()
        .any(|var| std::env::var(var).is_ok_and(|v| !v.trim().is_empty()))
    {
        return true;
    }
    std::env::var("AGENT_CODE_AUTH_MODE").is_ok_and(|v| {
        matches!(
            v.as_str(),
            "codex_chatgpt" | "chatgpt" | "xai_oauth" | "grok_oauth" | "xai" | "grok"
        )
    })
}

/// (env var, provider, base_url, default model) for every provider the
/// config loader resolves keys for.
///
/// Ordered to mirror `agent_code_lib::config::API_KEY_ENV_VARS` (see
/// the test `env_candidates_match_loader_priority`): when several keys
/// are exported, the provider defaults persisted from here must belong
/// to the same key `Config::load` will pick, or the key is sent to
/// another provider's endpoint.
const ENV_KEY_CANDIDATES: &[(&str, &str, &str, &str)] = &[
    (
        "AGENT_CODE_API_KEY",
        "openai",
        "https://api.openai.com/v1",
        "gpt-5.4",
    ),
    (
        "ANTHROPIC_API_KEY",
        "anthropic",
        "https://api.anthropic.com/v1",
        "claude-sonnet-4-20250514",
    ),
    (
        "OPENAI_API_KEY",
        "openai",
        "https://api.openai.com/v1",
        "gpt-5.4",
    ),
    (
        "XAI_API_KEY",
        "xai",
        "https://api.x.ai/v1",
        "grok-build-0.1",
    ),
    (
        "GOOGLE_API_KEY",
        "google",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "gemini-2.5-flash",
    ),
    (
        "DEEPSEEK_API_KEY",
        "deepseek",
        "https://api.deepseek.com/v1",
        "deepseek-chat",
    ),
    (
        "GROQ_API_KEY",
        "groq",
        "https://api.groq.com/openai/v1",
        "llama-3.3-70b-versatile",
    ),
    (
        "MISTRAL_API_KEY",
        "mistral",
        "https://api.mistral.ai/v1",
        "mistral-large-latest",
    ),
    (
        "ZHIPU_API_KEY",
        "zhipu",
        "https://open.bigmodel.cn/api/paas/v4",
        "glm-4.7",
    ),
    (
        "TOGETHER_API_KEY",
        "together",
        "https://api.together.xyz/v1",
        "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
    ),
    (
        "OPENROUTER_API_KEY",
        "openrouter",
        "https://openrouter.ai/api/v1",
        "openrouter/auto",
    ),
    (
        "COHERE_API_KEY",
        "cohere",
        "https://api.cohere.com/v2",
        "command-r-plus",
    ),
    (
        "PERPLEXITY_API_KEY",
        "perplexity",
        "https://api.perplexity.ai",
        "sonar-pro",
    ),
];

/// Prefer API keys already present in the environment (no interactive UI).
fn try_env_credentials() -> Option<SetupResult> {
    for (env, provider, url, model) in ENV_KEY_CANDIDATES {
        if let Ok(key) = std::env::var(env)
            && !key.trim().is_empty()
        {
            return Some(SetupResult {
                api_key: key,
                auth_mode: "api_key".into(),
                provider: (*provider).into(),
                base_url: Some((*url).into()),
                model: Some((*model).into()),
                theme: "auto".into(),
                permission_mode: "accept_edits".into(),
            });
        }
    }
    None
}

/// First-run when interactive launch has no usable credentials.
///
/// 1. Env API keys (if any) pick the provider and write config.
/// 2. Else `cli_provider` when not `"auto"` pins the provider for scripts.
/// 3. Else one interactive screen asks which provider.
/// 4. Then OAuth or a clear env-key hint for that provider.
///
/// `cli_provider` is the value of `--provider` (and may also be read from
/// `AGENT_CODE_PROVIDER` by clap). Passing `"auto"` means "ask".
pub fn run_setup(cli_provider: &str) -> Option<SetupResult> {
    ensure_default_config();

    if let Some(from_env) = try_env_credentials() {
        write_config(&from_env);
        println!(
            "  {} Using credentials from the environment ({}).",
            "✓".green(),
            from_env.provider
        );
        println!();
        return Some(from_env);
    }

    let choice = resolve_provider_choice(cli_provider)?;
    finish_provider_setup(&choice)
}

/// Map CLI / interactive choice to a concrete provider id.
fn resolve_provider_choice(cli_provider: &str) -> Option<String> {
    let pinned = cli_provider.trim();
    if !pinned.is_empty() && !pinned.eq_ignore_ascii_case("auto") {
        let mapped = map_cli_provider(pinned);
        println!();
        println!(
            "  {} Using provider `{}` from --provider / AGENT_CODE_PROVIDER.",
            "→".dark_cyan().bold(),
            mapped
        );
        println!();
        return Some(mapped);
    }
    interactive_provider_choice()
}

fn map_cli_provider(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => "anthropic".into(),
        "openai" | "gpt" => "openai".into(),
        "xai" | "grok" => "xai".into(),
        "google" | "gemini" => "google".into(),
        "deepseek" => "deepseek".into(),
        "groq" => "groq".into(),
        "mistral" => "mistral".into(),
        "together" => "together".into(),
        "zhipu" | "glm" | "z.ai" => "zhipu".into(),
        "ollama" | "local" => "ollama".into(),
        "codex" | "chatgpt" | "codex_subscription" => "codex_subscription".into(),
        "xai_subscription" | "grok_oauth" | "supergrok" => "xai_subscription".into(),
        other => other.to_string(),
    }
}

fn interactive_provider_choice() -> Option<String> {
    println!();
    println!(
        "  {} Choose a provider for this machine.",
        "→".dark_cyan().bold()
    );
    println!(
        "  {}",
        "No API key was found in the environment. Pick one — you can change later with /config."
            .dark_grey()
    );
    println!();
    let choice = select(&provider_select_options());
    if choice.is_empty() {
        None
    } else {
        Some(choice)
    }
}

fn provider_select_options() -> Vec<SelectOption> {
    vec![
        SelectOption {
            label: "ChatGPT / Codex subscription".into(),
            description: "Sign in with OpenAI account in browser (no API key)".into(),
            value: "codex_subscription".into(),
            preview: None,
        },
        SelectOption {
            label: "SuperGrok / X Premium subscription".into(),
            description: "xAI Grok OAuth device sign-in (no XAI_API_KEY)".into(),
            value: "xai_subscription".into(),
            preview: None,
        },
        SelectOption {
            label: "OpenAI API key".into(),
            description: "GPT models — export OPENAI_API_KEY".into(),
            value: "openai".into(),
            preview: None,
        },
        SelectOption {
            label: "Anthropic (Claude)".into(),
            description: "Opus, Sonnet, Haiku — export ANTHROPIC_API_KEY".into(),
            value: "anthropic".into(),
            preview: None,
        },
        SelectOption {
            label: "xAI (Grok) API key".into(),
            description: "Grok models — export XAI_API_KEY".into(),
            value: "xai".into(),
            preview: None,
        },
        SelectOption {
            label: "Google (Gemini)".into(),
            description: "Gemini — export GOOGLE_API_KEY".into(),
            value: "google".into(),
            preview: None,
        },
        SelectOption {
            label: "DeepSeek".into(),
            description: "export DEEPSEEK_API_KEY".into(),
            value: "deepseek".into(),
            preview: None,
        },
        SelectOption {
            label: "Groq".into(),
            description: "export GROQ_API_KEY".into(),
            value: "groq".into(),
            preview: None,
        },
        SelectOption {
            label: "Mistral".into(),
            description: "export MISTRAL_API_KEY".into(),
            value: "mistral".into(),
            preview: None,
        },
        SelectOption {
            label: "Together".into(),
            description: "export TOGETHER_API_KEY".into(),
            value: "together".into(),
            preview: None,
        },
        SelectOption {
            label: "Zhipu (z.ai)".into(),
            description: "export ZHIPU_API_KEY".into(),
            value: "zhipu".into(),
            preview: None,
        },
        SelectOption {
            label: "Ollama (local)".into(),
            description: "Local models, no API key".into(),
            value: "ollama".into(),
            preview: None,
        },
        SelectOption {
            label: "Other OpenAI-compatible".into(),
            description: "Set base_url later in config".into(),
            value: "custom".into(),
            preview: None,
        },
    ]
}

fn finish_provider_setup(choice: &str) -> Option<SetupResult> {
    match choice {
        "codex_subscription" => finish_codex_oauth(),
        "xai_subscription" => finish_xai_oauth(),
        "ollama" => {
            let result = SetupResult {
                api_key: String::new(),
                auth_mode: "api_key".into(),
                provider: "ollama".into(),
                base_url: Some("http://127.0.0.1:11434/v1".into()),
                model: Some("llama3.2".into()),
                theme: "auto".into(),
                permission_mode: "accept_edits".into(),
            };
            write_config(&result);
            print_ready_tips(&result);
            Some(result)
        }
        other => {
            let (provider, env_var, url, model) = api_key_provider_defaults(other);
            let result = SetupResult {
                api_key: String::new(),
                auth_mode: "api_key".into(),
                provider: provider.into(),
                base_url: Some(url.into()),
                model: Some(model.into()),
                theme: "auto".into(),
                permission_mode: "accept_edits".into(),
            };
            write_config(&result);
            println!();
            println!(
                "  {} Provider `{}` saved. Export a key, then re-run:",
                "→".dark_cyan().bold(),
                provider
            );
            println!("    {}", format!("export {env_var}=…").white().bold());
            println!(
                "  {}",
                "(Or paste a key into config.toml under [api] — env is preferred.)".dark_grey()
            );
            println!();
            print_ready_tips(&result);
            // Return Some so the caller reloads config; the next has_key
            // check may still fail until the user exports the key.
            Some(result)
        }
    }
}

fn api_key_provider_defaults(
    choice: &str,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match choice {
        "anthropic" => (
            "anthropic",
            "ANTHROPIC_API_KEY",
            "https://api.anthropic.com/v1",
            "claude-sonnet-4-20250514",
        ),
        "xai" => (
            "xai",
            "XAI_API_KEY",
            "https://api.x.ai/v1",
            "grok-build-0.1",
        ),
        "google" => (
            "google",
            "GOOGLE_API_KEY",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-2.5-flash",
        ),
        "deepseek" => (
            "deepseek",
            "DEEPSEEK_API_KEY",
            "https://api.deepseek.com/v1",
            "deepseek-chat",
        ),
        "groq" => (
            "groq",
            "GROQ_API_KEY",
            "https://api.groq.com/openai/v1",
            "llama-3.3-70b-versatile",
        ),
        "mistral" => (
            "mistral",
            "MISTRAL_API_KEY",
            "https://api.mistral.ai/v1",
            "mistral-large-latest",
        ),
        "together" => (
            "together",
            "TOGETHER_API_KEY",
            "https://api.together.xyz/v1",
            "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
        ),
        "zhipu" => (
            "zhipu",
            "ZHIPU_API_KEY",
            "https://open.bigmodel.cn/api/paas/v4",
            "glm-4.7",
        ),
        "custom" => (
            "openai",
            "OPENAI_API_KEY",
            "https://api.openai.com/v1",
            "gpt-5.4",
        ),
        _ => (
            "openai",
            "OPENAI_API_KEY",
            "https://api.openai.com/v1",
            "gpt-5.4",
        ),
    }
}

fn finish_codex_oauth() -> Option<SetupResult> {
    println!();
    println!(
        "  {} Opening browser for ChatGPT / Codex sign-in…",
        "→".dark_cyan().bold()
    );
    match run_codex_browser_login() {
        Ok(path) => {
            println!(
                "  {} Signed in. Session saved to {}",
                "✓".green(),
                path.display()
            );
            let result = SetupResult {
                api_key: String::new(),
                auth_mode: "codex_chatgpt".into(),
                provider: "openai".into(),
                base_url: Some("https://chatgpt.com/backend-api/codex".into()),
                model: Some("gpt-5.5".into()),
                theme: "auto".into(),
                permission_mode: "accept_edits".into(),
            };
            write_config(&result);
            print_ready_tips(&result);
            Some(result)
        }
        Err(e) => {
            println!("  {} Sign-in failed: {e}", "✗".red());
            println!("  {}", "Retry later with: agent login codex".yellow());
            None
        }
    }
}

fn finish_xai_oauth() -> Option<SetupResult> {
    println!();
    println!(
        "  {} SuperGrok / X Premium device sign-in…",
        "→".dark_cyan().bold()
    );
    match run_xai_device_login() {
        Ok(path) => {
            println!(
                "  {} Signed in. Session saved to {}",
                "✓".green(),
                path.display()
            );
            let result = SetupResult {
                api_key: String::new(),
                auth_mode: "xai_oauth".into(),
                provider: "xai".into(),
                base_url: Some("https://api.x.ai/v1".into()),
                model: Some("grok-build-0.1".into()),
                theme: "auto".into(),
                permission_mode: "accept_edits".into(),
            };
            write_config(&result);
            print_ready_tips(&result);
            Some(result)
        }
        Err(e) => {
            println!("  {} Sign-in failed: {e}", "✗".red());
            println!("  {}", "Retry later with: agent login xai".yellow());
            None
        }
    }
}

fn print_ready_tips(result: &SetupResult) {
    println!();
    println!(
        "  {}",
        "╭──────────────────────────────────────────────────────────╮".green()
    );
    println!(
        "  {}  {}{}",
        "│".green(),
        "You're ready.".green().bold(),
        "                                         │".green()
    );
    println!(
        "  {}",
        "╰──────────────────────────────────────────────────────────╯".green()
    );
    println!();
    println!("  {} Modern TUI (default)", "▸".dark_cyan());
    println!(
        "    {}  send          {}  newline",
        "Enter".white().bold(),
        "Alt+Enter".white().bold()
    );
    println!(
        "    {}  cycle modes   {}  cancel turn",
        "Shift+Tab".white().bold(),
        "Ctrl+C".white().bold()
    );
    println!(
        "    {}  never cancels {}  interject / send-now",
        "Esc".white().bold(),
        "Ctrl+Enter".white().bold()
    );
    println!(
        "    {}  queue pane    {}  tasks pane",
        "Ctrl+;".white().bold(),
        "Ctrl+T".white().bold()
    );
    println!();
    println!("  {} Sign-in", "▸".dark_cyan());
    match result.auth_mode.as_str() {
        "codex_chatgpt" => println!("    ChatGPT / Codex subscription (browser)"),
        "xai_oauth" => println!("    SuperGrok / X Premium (device code)"),
        _ => {
            if !result.api_key.is_empty() && result.api_key != "ollama" {
                println!("    API key stored in config (owner-only perms)");
            } else if result.provider == "ollama" {
                println!("    Ollama local — no API key");
            } else {
                println!("    Provider: {}", result.provider);
            }
        }
    }
    println!(
        "    model={} · permissions={}",
        result.model.as_deref().unwrap_or("default"),
        result.permission_mode
    );
    println!();
    println!(
        "  {}  {}{}{}",
        "Tip".dark_grey(),
        "docs/tui/KEYBINDINGS.md".dark_grey(),
        " · ".dark_grey(),
        "/help inside the TUI".dark_grey()
    );
    println!();
    println!(
        "  {} Launch:  {}",
        "→".green().bold(),
        "agent".white().bold()
    );
    println!();
}

/// Legacy multi-step interactive wizard implementation.
fn render_config_toml(result: &SetupResult) -> String {
    let base_url = result
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com/v1");
    let model = result.model.as_deref().unwrap_or("gpt-5.4");

    let mut api = toml::value::Table::new();
    api.insert("base_url".into(), base_url.into());
    api.insert("model".into(), model.into());
    if result.auth_mode != "api_key" && !result.auth_mode.is_empty() {
        api.insert("auth_mode".into(), result.auth_mode.clone().into());
    }
    // Include the API key only when it's a real, persistable secret.
    // Ollama needs no key, and an empty key must not be written.
    // Subscription (codex_chatgpt) auth has no API key field.
    if result.auth_mode == "api_key" && !result.api_key.is_empty() && result.api_key != "ollama" {
        api.insert("api_key".into(), result.api_key.clone().into());
    }

    let mut permissions = toml::value::Table::new();
    permissions.insert("default_mode".into(), result.permission_mode.clone().into());

    let mut ui = toml::value::Table::new();
    ui.insert("theme".into(), result.theme.clone().into());

    let mut root = toml::value::Table::new();
    root.insert("api".into(), toml::Value::Table(api));
    root.insert("permissions".into(), toml::Value::Table(permissions));
    root.insert("ui".into(), toml::Value::Table(ui));

    // Serializing a table of tables can't produce a value-after-table
    // error, so this only fails on a serializer bug — fall back to an
    // empty document so `write_config` still surfaces a save error
    // rather than panicking inside the wizard.
    toml::to_string_pretty(&toml::Value::Table(root)).unwrap_or_default()
}

/// Write config file from setup result (prints path on success).
///
/// Persists atomically with owner-only (`0600`) permissions — the file
/// holds the API key — and surfaces failures to the user instead of
/// swallowing them. A silent write failure used to masquerade as a
/// successful setup: the in-process key kept the current session
/// working while nothing reached disk, so the next launch re-ran the
/// wizard (issue #288).
pub fn write_config(result: &SetupResult) {
    write_config_impl(result, true);
}

/// Persist defaults without chatty "Config saved" lines (Grok-style launch).
fn write_config_quiet(result: &SetupResult) {
    write_config_impl(result, false);
}

fn write_config_impl(result: &SetupResult, announce: bool) {
    let Some(config_dir) = agent_code_lib::config::agent_config_dir() else {
        if announce {
            println!(
                "  {}",
                "Could not determine a config directory — setup was not saved. \
                 Set AGENT_CODE_API_KEY in your environment to use the agent."
                    .yellow()
            );
            println!();
        }
        return;
    };

    let config_path = config_dir.join("config.toml");
    let body = render_config_toml(result);

    match atomic_write_secret(&config_path, body.as_bytes()) {
        Ok(()) => {
            if announce {
                println!(
                    "{}",
                    format!("  Config saved to {}", config_path.display()).dark_grey()
                );
                println!();
            }
            // Skip the separate first-run theme picker on the next launch.
            super::onboarding::mark_onboarded();
        }
        Err(e) => {
            if announce {
                println!(
                    "  {}",
                    format!(
                        "Could not save config to {} ({e}). \
                         Set AGENT_CODE_API_KEY in your environment to use the agent.",
                        config_path.display()
                    )
                    .yellow()
                );
                println!();
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SetupResult {
    pub api_key: String,
    /// `"api_key"` (default) or `"codex_chatgpt"` for ChatGPT subscription.
    pub auth_mode: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub theme: String,
    pub permission_mode: String,
}

fn run_codex_browser_login() -> Result<std::path::PathBuf, String> {
    run_async_login(|| async {
        agent_code_lib::llm::codex_auth::browser_login(None)
            .await
            .map_err(|e| e.to_string())
    })
}

fn run_xai_device_login() -> Result<std::path::PathBuf, String> {
    run_async_login(|| async {
        agent_code_lib::llm::xai_auth::device_code_login(true)
            .await
            .map_err(|e| e.to_string())
    })
}

fn run_async_login<F, Fut>(f: F) -> Result<std::path::PathBuf, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<std::path::PathBuf, String>> + Send,
{
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;
        rt.block_on(f())
    })
    .join()
    .unwrap_or_else(|_| Err("login thread panicked".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_with_key(api_key: &str) -> SetupResult {
        SetupResult {
            api_key: api_key.to_string(),
            auth_mode: "api_key".into(),
            provider: "custom".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            model: Some("gpt-5.4".to_string()),
            theme: "nord".to_string(),
            permission_mode: "ask".to_string(),
        }
    }

    /// The bootstrap's env-credential table must resolve keys in the
    /// same priority order as `Config::load`, or the provider defaults
    /// it persists can belong to a different provider than the key the
    /// loader picks (which sends that key to the wrong endpoint).
    #[test]
    fn env_candidates_match_loader_priority() {
        let candidate_vars: Vec<&str> = ENV_KEY_CANDIDATES.iter().map(|(var, ..)| *var).collect();
        assert_eq!(
            candidate_vars.as_slice(),
            agent_code_lib::config::API_KEY_ENV_VARS,
            "keep ENV_KEY_CANDIDATES in the same order as API_KEY_ENV_VARS"
        );
    }

    #[test]
    fn default_setup_does_not_pin_a_vendor() {
        let d = default_setup_result();
        assert!(
            d.provider.is_empty() && d.base_url.is_none() && d.model.is_none(),
            "silent defaults must not invent a provider: {d:?}"
        );
    }

    #[test]
    fn map_cli_provider_aliases() {
        assert_eq!(map_cli_provider("claude"), "anthropic");
        assert_eq!(map_cli_provider("grok"), "xai");
        assert_eq!(map_cli_provider("codex"), "codex_subscription");
        assert_eq!(map_cli_provider("openai"), "openai");
    }

    #[test]
    fn api_key_defaults_cover_main_providers() {
        let (_, env, url, _) = api_key_provider_defaults("anthropic");
        assert_eq!(env, "ANTHROPIC_API_KEY");
        assert!(url.contains("anthropic"));
        let (p, env, _, model) = api_key_provider_defaults("xai");
        assert_eq!(p, "xai");
        assert_eq!(env, "XAI_API_KEY");
        assert!(model.contains("grok"));
    }

    #[test]
    fn codex_subscription_writes_auth_mode_not_api_key() {
        let result = SetupResult {
            api_key: String::new(),
            auth_mode: "codex_chatgpt".into(),
            provider: "openai".into(),
            base_url: Some("https://chatgpt.com/backend-api/codex".into()),
            model: Some("gpt-5.5".into()),
            theme: "auto".into(),
            permission_mode: "ask".into(),
        };
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert_eq!(doc["api"]["auth_mode"].as_str(), Some("codex_chatgpt"));
        assert_eq!(doc["api"]["model"].as_str(), Some("gpt-5.5"));
        assert!(doc["api"].get("api_key").is_none());
    }

    #[test]
    fn xai_subscription_writes_auth_mode_and_grok_build_model() {
        let result = SetupResult {
            api_key: String::new(),
            auth_mode: "xai_oauth".into(),
            provider: "xai".into(),
            base_url: Some("https://api.x.ai/v1".into()),
            model: Some("grok-build-0.1".into()),
            theme: "auto".into(),
            permission_mode: "ask".into(),
        };
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert_eq!(doc["api"]["auth_mode"].as_str(), Some("xai_oauth"));
        assert_eq!(doc["api"]["model"].as_str(), Some("grok-build-0.1"));
        assert_eq!(doc["api"]["base_url"].as_str(), Some("https://api.x.ai/v1"));
        assert!(doc["api"].get("api_key").is_none());
    }

    /// Read the `api.api_key` field back out of a rendered config the
    /// same way `Config::load` would: parse the TOML, then index in.
    fn loaded_api_key(result: &SetupResult) -> Option<String> {
        let doc: toml::Value =
            toml::from_str(&render_config_toml(result)).expect("rendered config must parse");
        doc.get("api")
            .and_then(|a| a.get("api_key"))
            .and_then(|k| k.as_str())
            .map(str::to_string)
    }

    #[test]
    fn plain_key_round_trips() {
        let result = result_with_key("sk-normaltoken-123");
        assert_eq!(
            loaded_api_key(&result).as_deref(),
            Some("sk-normaltoken-123")
        );
    }

    /// Issue #288: a key with a backslash used to be mangled by the
    /// hand-rolled `format!` writer (`\b` decoded to a backspace), so
    /// the persisted key silently differed from what the user pasted.
    #[test]
    fn key_with_backslash_survives() {
        let result = result_with_key(r"sk-with\backslash");
        assert_eq!(
            loaded_api_key(&result).as_deref(),
            Some(r"sk-with\backslash")
        );
    }

    /// Issue #288: a key with a double quote used to make config.toml
    /// unparseable, so the next launch failed to load any key at all.
    #[test]
    fn key_with_quote_survives() {
        let result = result_with_key(r#"sk-with"quote"#);
        assert_eq!(loaded_api_key(&result).as_deref(), Some(r#"sk-with"quote"#));
    }

    #[test]
    fn other_sections_are_preserved() {
        let result = result_with_key("sk-abc");
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert_eq!(
            doc["api"]["base_url"].as_str(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(doc["api"]["model"].as_str(), Some("gpt-5.4"));
        assert_eq!(doc["permissions"]["default_mode"].as_str(), Some("ask"));
        assert_eq!(doc["ui"]["theme"].as_str(), Some("nord"));
        assert!(
            doc["ui"].get("tui").is_none(),
            "tui kind is no longer configurable — modern is the only interactive surface"
        );
    }

    #[test]
    fn empty_key_is_omitted() {
        let result = result_with_key("");
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert!(
            doc["api"].get("api_key").is_none(),
            "an empty key must not be written"
        );
    }

    #[test]
    fn ollama_sentinel_key_is_omitted() {
        let result = result_with_key("ollama");
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert!(
            doc["api"].get("api_key").is_none(),
            "the ollama sentinel must not be persisted as a key"
        );
    }
}
