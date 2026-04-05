//! agent-code: An AI-powered coding agent for the terminal.
//!
//! Entry point for the `agent` binary. Handles CLI argument parsing,
//! configuration loading, and launches the interactive REPL or
//! one-shot execution mode.

// Many types exist for the public API surface but aren't used internally yet.
#![allow(dead_code)]

mod commands;
mod ui;
mod update;

/// Estimate cost for a single model's usage (used by /cost command).
fn estimate_model_cost(usage: &agent_code_lib::llm::message::Usage, model: &str) -> f64 {
    agent_code_lib::services::pricing::calculate_cost(
        model,
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens,
    )
}

use clap::Parser;
use tracing_subscriber::EnvFilter;

use std::sync::Arc;

use agent_code_lib::config::Config;
use agent_code_lib::llm::provider::{ProviderKind, WireFormat, detect_provider};
use agent_code_lib::permissions::PermissionChecker;
use agent_code_lib::query::QueryEngine;
use agent_code_lib::state::AppState;
use agent_code_lib::tools::registry::ToolRegistry;

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
    let session_env = agent_code_lib::services::session_env::SessionEnvironment::detect().await;
    tracing::debug!(
        "Environment: {} on {}, git={}, shell={}",
        session_env.project_root.display(),
        session_env.platform,
        session_env.is_git_repo,
        session_env.shell,
    );

    // Memory consolidation check is deferred until after provider setup.

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
        config.permissions.default_mode = agent_code_lib::config::PermissionMode::Allow;
        tracing::warn!("All permission checks disabled (--dangerously-skip-permissions)");
    } else {
        config.permissions.default_mode = match cli.permission_mode.as_str() {
            "allow" => agent_code_lib::config::PermissionMode::Allow,
            "deny" => agent_code_lib::config::PermissionMode::Deny,
            "plan" => agent_code_lib::config::PermissionMode::Plan,
            "accept_edits" => agent_code_lib::config::PermissionMode::AcceptEdits,
            _ => agent_code_lib::config::PermissionMode::Ask,
        };
    }

    // Determine the effective API key: CLI flag > env var (in config) > config file.
    // If nothing found and interactive, run the setup wizard.
    let has_key = cli.api_key.is_some() || config.api.api_key.is_some();

    if !has_key && cli.prompt.is_none() && !cli.dump_system_prompt {
        eprintln!("No API key found. Starting setup...\n");
        run_setup_wizard();
        config = Config::load()?;
    }

    // CLI --api-key overrides everything.
    if let Some(ref key) = cli.api_key {
        config.api.api_key = Some(key.clone());
    }

    let api_key = config.api.api_key.as_deref().ok_or_else(|| {
        anyhow::anyhow!("API key required. Set AGENT_CODE_API_KEY or pass --api-key.")
    })?;

    // Initialize LLM provider. If --model or --provider implies a different
    // provider than what's in the config, override the base URL to match.
    let provider_kind = match cli.provider.as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "openai" => ProviderKind::OpenAi,
        "bedrock" | "aws" => ProviderKind::Bedrock,
        "vertex" | "gcp" => ProviderKind::Vertex,
        "xai" | "grok" => ProviderKind::Xai,
        "google" | "gemini" => ProviderKind::Google,
        "deepseek" => ProviderKind::DeepSeek,
        "groq" => ProviderKind::Groq,
        "mistral" => ProviderKind::Mistral,
        "together" => ProviderKind::Together,
        "zhipu" | "glm" | "z.ai" => ProviderKind::Zhipu,
        _ => detect_provider(&config.api.model, &config.api.base_url),
    };

    // Override base URL if the detected provider has a known default.
    if cli.api_base_url.is_none()
        && let Some(default_url) = provider_kind.default_base_url()
    {
        config.api.base_url = default_url.to_string();
    }
    let mut llm: Arc<dyn agent_code_lib::llm::provider::Provider> = match provider_kind
        .wire_format()
    {
        WireFormat::Anthropic => Arc::new(agent_code_lib::llm::anthropic::AnthropicProvider::new(
            &config.api.base_url,
            api_key,
        )),
        WireFormat::OpenAiCompatible => Arc::new(agent_code_lib::llm::openai::OpenAiProvider::new(
            &config.api.base_url,
            api_key,
        )),
    };
    tracing::info!(
        "Using {:?} provider at {}",
        provider_kind,
        config.api.base_url
    );

    // Validate API key with a quick curl check (skip for local/Ollama).
    if !config.api.base_url.contains("localhost")
        && !config.api.base_url.contains("127.0.0.1")
        && cli.prompt.is_none()
        && !cli.dump_system_prompt
    {
        let check_url = format!("{}/models", config.api.base_url);
        let key_invalid = std::process::Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "--max-time",
                "5",
                "-H",
                &format!("Authorization: Bearer {api_key}"),
                "-H",
                &format!("x-api-key: {api_key}"),
                &check_url,
            ])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .is_some_and(|code| code.trim() == "401" || code.trim() == "403");

        if key_invalid {
            eprintln!(
                "\nAPI key rejected by {}. Let's update it.\n",
                config.api.base_url
            );
            run_setup_wizard();
            config = Config::load()?;
            let api_key_new = config
                .api
                .api_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("API key required after setup."))?;
            llm = match provider_kind.wire_format() {
                WireFormat::Anthropic => {
                    Arc::new(agent_code_lib::llm::anthropic::AnthropicProvider::new(
                        &config.api.base_url,
                        api_key_new,
                    ))
                }
                WireFormat::OpenAiCompatible => {
                    Arc::new(agent_code_lib::llm::openai::OpenAiProvider::new(
                        &config.api.base_url,
                        api_key_new,
                    ))
                }
            };
        }
    }

    let mut tool_registry = ToolRegistry::default_tools();
    let permission_checker = PermissionChecker::from_config(&config.permissions);
    let app_state = AppState::new(config.clone());

    // Connect configured MCP servers and register their tools.
    for (name, entry) in &config.mcp_servers {
        let transport = if let Some(ref cmd) = entry.command {
            agent_code_lib::services::mcp::McpTransport::Stdio {
                command: cmd.clone(),
                args: entry.args.clone(),
            }
        } else if let Some(ref url) = entry.url {
            agent_code_lib::services::mcp::McpTransport::Sse { url: url.clone() }
        } else {
            tracing::warn!("MCP server '{name}': no command or url configured, skipping");
            continue;
        };

        let mcp_config = agent_code_lib::services::mcp::McpServerConfig {
            transport,
            name: name.clone(),
            env: entry.env.clone(),
        };

        let mut client = agent_code_lib::services::mcp::McpClient::new(mcp_config);
        match client.connect().await {
            Ok(()) => {
                let discovered = client.tools().to_vec();
                let client_arc = std::sync::Arc::new(tokio::sync::Mutex::new(client));
                let proxies = agent_code_lib::tools::mcp_proxy::create_proxy_tools(
                    name,
                    &discovered,
                    client_arc,
                );
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
        let prompt = agent_code_lib::query::build_system_prompt(&tool_registry, &app_state);
        println!("{prompt}");
        return Ok(());
    }

    // Build the query engine (agent loop).
    let llm_for_consolidation = llm.clone();
    let mut engine = QueryEngine::new(
        llm,
        tool_registry,
        permission_checker,
        app_state,
        agent_code_lib::query::QueryEngineConfig {
            max_turns: cli.max_turns,
            verbose: cli.verbose,
            unattended: cli.prompt.is_some(),
        },
    );

    // Load hooks from config.
    engine.load_hooks(&config.hooks);

    // Run memory consolidation in the background if due and feature enabled.
    if config.features.extract_memories
        && let Some(memory_dir) = agent_code_lib::memory::ensure_memory_dir()
        && agent_code_lib::memory::consolidation::should_consolidate(&memory_dir)
        && let Some(lock_path) =
            agent_code_lib::memory::consolidation::try_acquire_lock(&memory_dir)
    {
        let consolidation_llm = llm_for_consolidation;
        let consolidation_model = config.api.model.clone();
        tokio::spawn(async move {
            tracing::info!("Memory consolidation starting (background)");
            agent_code_lib::memory::consolidation::run_consolidation(
                &memory_dir,
                &lock_path,
                consolidation_llm,
                &consolidation_model,
            )
            .await;
        });
    }

    // Install Ctrl+C handler for graceful cancellation.
    engine.install_signal_handler();

    // One-shot or interactive mode.
    match cli.prompt {
        Some(prompt) => {
            // One-shot mode: use a simple sink that prints to stdout.
            struct StdoutSink;
            impl agent_code_lib::query::StreamSink for StdoutSink {
                fn on_text(&self, text: &str) {
                    print!("{text}");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                fn on_tool_start(&self, name: &str, _: &serde_json::Value) {
                    eprintln!("[{name}]");
                }
                fn on_tool_result(&self, name: &str, r: &agent_code_lib::tools::ToolResult) {
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
            // Check for updates in the background (non-blocking).
            let update_handle = tokio::spawn(update::check_for_update());

            ui::repl::run_repl(&mut engine).await?;

            // Show update notification after session ends.
            if let Ok(Some(check)) = update_handle.await {
                update::print_update_hint(&check);
            }
        }
    }

    Ok(())
}
