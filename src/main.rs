//! rust-code: An AI-powered coding agent for the terminal.
//!
//! Entry point for the `rc` binary. Handles CLI argument parsing,
//! configuration loading, and launches the interactive REPL or
//! one-shot execution mode.

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

use crate::config::Config;
use crate::llm::client::LlmClient;
use crate::permissions::PermissionChecker;
use crate::query::QueryEngine;
use crate::state::AppState;
use crate::tools::registry::ToolRegistry;

/// AI-powered coding agent for the terminal.
#[derive(Parser, Debug)]
#[command(name = "rc", version, about)]
struct Cli {
    /// Execute a single prompt and exit (non-interactive mode).
    #[arg(short, long)]
    prompt: Option<String>,

    /// API base URL override.
    #[arg(long, env = "RC_API_BASE_URL")]
    api_base_url: Option<String>,

    /// Model to use.
    #[arg(long, short, env = "RC_MODEL")]
    model: Option<String>,

    /// API key.
    #[arg(long, env = "RC_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// Enable verbose output.
    #[arg(short, long)]
    verbose: bool,

    /// Working directory (defaults to current directory).
    #[arg(short = 'C', long)]
    cwd: Option<String>,

    /// Permission mode: ask, allow, deny.
    #[arg(long, default_value = "ask")]
    permission_mode: String,

    /// Print system prompt and exit.
    #[arg(long)]
    dump_system_prompt: bool,

    /// Maximum number of agent turns before stopping.
    #[arg(long)]
    max_turns: Option<usize>,
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

    let api_key = config
        .api
        .api_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!(
            "API key required. Set RC_API_KEY or pass --api-key."
        ))?;

    // Initialize core subsystems.
    let llm_client = LlmClient::new(
        &config.api.base_url,
        api_key,
        &config.api.model,
    );

    let tool_registry = ToolRegistry::default_tools();
    let permission_checker = PermissionChecker::from_config(&config.permissions);
    let app_state = AppState::new(config.clone());

    // Connect configured MCP servers.
    if !config.mcp_servers.is_empty() {
        for (name, entry) in &config.mcp_servers {
            let transport = if let Some(ref cmd) = entry.command {
                services::mcp::McpTransport::Stdio {
                    command: cmd.clone(),
                    args: entry.args.clone(),
                }
            } else if let Some(ref url) = entry.url {
                services::mcp::McpTransport::Sse { url: url.clone() }
            } else {
                tracing::warn!("MCP server '{name}' has no command or url, skipping");
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
                    let tool_count = client.tools().len();
                    tracing::info!("MCP server '{name}' connected ({tool_count} tools)");
                }
                Err(e) => {
                    tracing::warn!("MCP server '{name}' failed to connect: {e}");
                }
            }
            // TODO: register MCP tools into the tool registry
        }
    }

    if cli.dump_system_prompt {
        let prompt = query::build_system_prompt(&tool_registry, &app_state);
        println!("{prompt}");
        return Ok(());
    }

    // Build the query engine (agent loop).
    let mut engine = QueryEngine::new(
        llm_client,
        tool_registry,
        permission_checker,
        app_state,
        query::QueryEngineConfig {
            max_turns: cli.max_turns,
            verbose: cli.verbose,
        },
    );

    // One-shot or interactive mode.
    match cli.prompt {
        Some(prompt) => {
            engine.run_turn(&prompt).await?;
        }
        None => {
            ui::repl::run_repl(&mut engine).await?;
        }
    }

    Ok(())
}
