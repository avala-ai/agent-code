//! agent-code: An AI-powered coding agent for the terminal.
//!
//! Entry point for the `agent` binary. Handles CLI argument parsing,
//! configuration loading, and launches the interactive REPL or
//! one-shot execution mode.

// Many types exist for the public API surface but aren't used internally yet.
#![allow(dead_code)]

mod commands;
mod config;
mod error;
mod hooks;
mod llm;
mod memory;
mod permissions;
mod query;
mod services;
mod skills;
mod state;
mod tools;
mod ui;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use std::sync::Arc;

use crate::config::Config;
use crate::llm::provider::{ProviderKind, detect_provider};
use crate::permissions::PermissionChecker;
use crate::query::QueryEngine;
use crate::state::AppState;
use crate::tools::registry::ToolRegistry;

/// AI-powered coding agent for the terminal.
#[derive(Parser, Debug)]
#[command(name = "agent", version, about)]
struct Cli {
    /// Execute a single prompt and exit (non-interactive mode).
    #[arg(short, long)]
    prompt: Option<String>,

    /// API base URL override.
    #[arg(long, env = "AGENT_CODE_API_BASE_URL")]
    api_base_url: Option<String>,

    /// Model to use.
    #[arg(long, short, env = "AGENT_CODE_MODEL")]
    model: Option<String>,

    /// API key.
    #[arg(long, env = "AGENT_CODE_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// Enable verbose output.
    #[arg(short, long)]
    verbose: bool,

    /// Working directory (defaults to current directory).
    #[arg(short = 'C', long)]
    cwd: Option<String>,

    /// Permission mode: ask, allow, deny, plan, accept_edits.
    #[arg(long, default_value = "ask")]
    permission_mode: String,

    /// Skip all permission checks. Equivalent to --permission-mode allow.
    /// Use only in trusted environments (CI, scripting).
    #[arg(long)]
    dangerously_skip_permissions: bool,

    /// LLM provider: anthropic, openai, xai (grok), or auto (default).
    #[arg(long, default_value = "auto")]
    provider: String,

    /// Print system prompt and exit.
    #[arg(long)]
    dump_system_prompt: bool,

    /// Maximum number of agent turns before stopping.
    #[arg(long)]
    max_turns: Option<usize>,
}

fn run_setup_wizard() {
    if let Some(result) = ui::setup::run_setup()
        && !result.api_key.is_empty()
    {
        // SAFETY: single-threaded, before async runtime work.
        unsafe { std::env::set_var("AGENT_CODE_API_KEY", &result.api_key) };
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing/logging.
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Set working directory if specified.
    if let Some(ref cwd) = cli.cwd {
        std::env::set_current_dir(cwd)?;
    }

    // Run setup wizard on first launch (no config file). Skip for non-interactive modes.
    if cli.prompt.is_none() && !cli.dump_system_prompt && ui::setup::needs_setup() {
        run_setup_wizard();
    }

    // Detect session environment.
    let session_env = services::session_env::SessionEnvironment::detect().await;
    tracing::debug!(
        "Environment: {} on {}, git={}, shell={}",
        session_env.project_root.display(),
        session_env.platform,
        session_env.is_git_repo,
        session_env.shell,
    );

    // Load configuration (files + env + CLI overrides).
    let mut config = Config::load()?;
    if let Some(ref url) = cli.api_base_url {
        config.api.base_url = url.clone();
    }
    if let Some(ref model) = cli.model {
        config.api.model = model.clone();
    }
    if let Some(ref key) = cli.api_key {
        config.api.api_key = Some(key.clone());
    }

    // Apply permission mode from CLI.
    if cli.dangerously_skip_permissions {
        config.permissions.default_mode = crate::config::PermissionMode::Allow;
        tracing::warn!("All permission checks disabled (--dangerously-skip-permissions)");
    } else {
        config.permissions.default_mode = match cli.permission_mode.as_str() {
            "allow" => crate::config::PermissionMode::Allow,
            "deny" => crate::config::PermissionMode::Deny,
            "plan" => crate::config::PermissionMode::Plan,
            "accept_edits" => crate::config::PermissionMode::AcceptEdits,
            _ => crate::config::PermissionMode::Ask,
        };
    }

    // If no API key found and interactive, offer setup or hint the right env var.
    if config.api.api_key.is_none()
        && cli.prompt.is_none()
        && cli.api_key.is_none()
        && !cli.dump_system_prompt
    {
        if ui::setup::needs_setup() {
            // First time — run full wizard.
            run_setup_wizard();
            config = Config::load()?;
        } else {
            // Config exists but no key in env. Tell user which var to set.
            let hint = match config.api.base_url.as_str() {
                u if u.contains("anthropic") => "ANTHROPIC_API_KEY",
                u if u.contains("openai") => "OPENAI_API_KEY",
                u if u.contains("x.ai") => "XAI_API_KEY",
                u if u.contains("googleapis") => "GOOGLE_API_KEY",
                u if u.contains("deepseek") => "DEEPSEEK_API_KEY",
                u if u.contains("groq") => "GROQ_API_KEY",
                u if u.contains("mistral") => "MISTRAL_API_KEY",
                u if u.contains("together") => "TOGETHER_API_KEY",
                _ => "AGENT_CODE_API_KEY",
            };
            eprintln!(
                "No API key found. Set it with:\n\n  export {hint}=\"your-key\"\n\n\
                 Or run: agent --api-key your-key\n\
                 Or delete ~/.config/agent-code/config.toml to re-run setup.\n"
            );
        }
    }

    let api_key = config.api.api_key.as_deref().ok_or_else(|| {
        anyhow::anyhow!("API key required. Set AGENT_CODE_API_KEY or pass --api-key.")
    })?;

    // Initialize LLM provider. If --model or --provider implies a different
    // provider than what's in the config, override the base URL to match.
    let provider_kind = match cli.provider.as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "openai" => ProviderKind::OpenAi,
        "xai" | "grok" => ProviderKind::Xai,
        "google" | "gemini" => ProviderKind::Google,
        "deepseek" => ProviderKind::DeepSeek,
        "groq" => ProviderKind::Groq,
        "mistral" => ProviderKind::Mistral,
        "together" => ProviderKind::Together,
        _ => detect_provider(&config.api.model, &config.api.base_url),
    };

    // Override base URL if the detected provider doesn't match the configured URL.
    if cli.api_base_url.is_none() {
        let default_url = match provider_kind {
            ProviderKind::Anthropic => "https://api.anthropic.com/v1",
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            ProviderKind::Xai => "https://api.x.ai/v1",
            ProviderKind::Google => "https://generativelanguage.googleapis.com/v1beta/openai",
            ProviderKind::DeepSeek => "https://api.deepseek.com/v1",
            ProviderKind::Groq => "https://api.groq.com/openai/v1",
            ProviderKind::Mistral => "https://api.mistral.ai/v1",
            ProviderKind::Together => "https://api.together.xyz/v1",
            ProviderKind::OpenAiCompatible => &config.api.base_url,
        };
        config.api.base_url = default_url.to_string();
    }
    let llm: Arc<dyn crate::llm::provider::Provider> = match provider_kind {
        ProviderKind::Anthropic => Arc::new(crate::llm::anthropic::AnthropicProvider::new(
            &config.api.base_url,
            api_key,
        )),
        // All other providers use the OpenAI-compatible wire format.
        ProviderKind::OpenAi
        | ProviderKind::Xai
        | ProviderKind::Google
        | ProviderKind::DeepSeek
        | ProviderKind::Groq
        | ProviderKind::Mistral
        | ProviderKind::Together
        | ProviderKind::OpenAiCompatible => Arc::new(crate::llm::openai::OpenAiProvider::new(
            &config.api.base_url,
            api_key,
        )),
    };
    tracing::info!(
        "Using {:?} provider at {}",
        provider_kind,
        config.api.base_url
    );

    let mut tool_registry = ToolRegistry::default_tools();
    let permission_checker = PermissionChecker::from_config(&config.permissions);
    let app_state = AppState::new(config.clone());

    // Connect configured MCP servers and register their tools.
    for (name, entry) in &config.mcp_servers {
        let transport = if let Some(ref cmd) = entry.command {
            services::mcp::McpTransport::Stdio {
                command: cmd.clone(),
                args: entry.args.clone(),
            }
        } else if let Some(ref url) = entry.url {
            services::mcp::McpTransport::Sse { url: url.clone() }
        } else {
            tracing::warn!("MCP server '{name}': no command or url configured, skipping");
            continue;
        };

        let mcp_config = services::mcp::McpServerConfig {
            transport,
            name: name.clone(),
            env: entry.env.clone(),
        };

        let mut client = services::mcp::McpClient::new(mcp_config);
        match client.connect().await {
            Ok(()) => {
                let discovered = client.tools().to_vec();
                let client_arc = std::sync::Arc::new(tokio::sync::Mutex::new(client));
                let proxies = tools::mcp_proxy::create_proxy_tools(name, &discovered, client_arc);
                let count = proxies.len();
                for proxy in proxies {
                    tool_registry.register(proxy);
                }
                tracing::info!("MCP '{name}': registered {count} tools");
            }
            Err(e) => {
                tracing::warn!("MCP '{name}': connection failed: {e}");
            }
        }
    }

    if cli.dump_system_prompt {
        let prompt = query::build_system_prompt(&tool_registry, &app_state);
        println!("{prompt}");
        return Ok(());
    }

    // Build the query engine (agent loop).
    let mut engine = QueryEngine::new(
        llm,
        tool_registry,
        permission_checker,
        app_state,
        query::QueryEngineConfig {
            max_turns: cli.max_turns,
            verbose: cli.verbose,
        },
    );

    // Load hooks from config.
    engine.load_hooks(&config.hooks);

    // Install Ctrl+C handler for graceful cancellation.
    engine.install_signal_handler();

    // One-shot or interactive mode.
    match cli.prompt {
        Some(prompt) => {
            // One-shot mode: use a simple sink that prints to stdout.
            struct StdoutSink;
            impl query::StreamSink for StdoutSink {
                fn on_text(&self, text: &str) {
                    print!("{text}");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                fn on_tool_start(&self, name: &str, _: &serde_json::Value) {
                    eprintln!("[{name}]");
                }
                fn on_tool_result(&self, name: &str, r: &tools::ToolResult) {
                    if r.is_error {
                        eprintln!("[{name} error: {}]", r.content.lines().next().unwrap_or(""));
                    }
                }
                fn on_error(&self, e: &str) {
                    eprintln!("Error: {e}");
                }
            }
            engine.run_turn_with_sink(&prompt, &StdoutSink).await?;
            println!();
        }
        None => {
            ui::repl::run_repl(&mut engine).await?;
        }
    }

    Ok(())
}
