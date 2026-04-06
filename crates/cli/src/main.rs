//! agent-code: An AI-powered coding agent for the terminal.
//!
//! Entry point for the `agent` binary. Handles CLI argument parsing,
//! configuration loading, and launches the interactive REPL or
//! one-shot execution mode.

// Many types exist for the public API surface but aren't used internally yet.
#![allow(dead_code)]

mod acp;
mod attach;
mod commands;
mod daemon;
mod serve;
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

    /// Start as a headless HTTP API server instead of interactive REPL.
    #[arg(long)]
    serve: bool,

    /// Port for the HTTP server (default: 4096). Only used with --serve.
    #[arg(long, default_value = "4096")]
    port: u16,

    /// Attach to a running serve instance. Optionally pass a session ID
    /// prefix to connect to a specific instance. Discovers via bridge lock
    /// files or connects to the specified --port.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    attach: Option<String>,

    /// Start ACP (Agent Client Protocol) stdio server for IDE integrations.
    /// IDEs spawn `agent acp` and communicate via stdin/stdout JSON-RPC 2.0.
    #[arg(long)]
    acp: bool,

    /// Subcommand (schedule, daemon).
    #[command(subcommand)]
    command: Option<SubCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum SubCommand {
    /// Manage scheduled agent runs.
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
    /// Start the schedule daemon (cron loop + optional webhook server).
    Daemon {
        /// Port for the webhook trigger server.
        #[arg(long)]
        webhook_port: Option<u16>,
    },
}

#[derive(clap::Subcommand, Debug)]
enum ScheduleAction {
    /// Add a new schedule.
    Add {
        /// Cron expression (5-field, e.g. "0 9 * * *").
        cron: String,
        /// Prompt to execute.
        #[arg(long)]
        prompt: String,
        /// Schedule name.
        #[arg(long)]
        name: String,
        /// Model override.
        #[arg(long)]
        model: Option<String>,
        /// Max cost per run (USD).
        #[arg(long)]
        max_cost: Option<f64>,
        /// Max agent turns per run.
        #[arg(long)]
        max_turns: Option<usize>,
        /// Generate a webhook secret for HTTP triggers.
        #[arg(long)]
        webhook: bool,
    },
    /// List all schedules.
    #[command(name = "list", alias = "ls")]
    List,
    /// Remove a schedule.
    #[command(alias = "rm")]
    Remove {
        /// Schedule name.
        name: String,
    },
    /// Run a schedule immediately.
    Run {
        /// Schedule name.
        name: String,
    },
    /// Enable a disabled schedule.
    Enable {
        /// Schedule name.
        name: String,
    },
    /// Disable a schedule without removing it.
    Disable {
        /// Schedule name.
        name: String,
    },
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

    // Handle schedule subcommands that don't need an LLM provider.
    if let Some(SubCommand::Schedule { ref action }) = cli.command {
        match action {
            ScheduleAction::List => {
                return handle_schedule_list();
            }
            ScheduleAction::Remove { name } => {
                return handle_schedule_remove(name);
            }
            ScheduleAction::Enable { name } => {
                return handle_schedule_toggle(name, true);
            }
            ScheduleAction::Disable { name } => {
                return handle_schedule_toggle(name, false);
            }
            ScheduleAction::Add {
                cron,
                prompt,
                name,
                model,
                max_cost,
                max_turns,
                webhook,
            } => {
                return handle_schedule_add(
                    name,
                    cron,
                    prompt,
                    model.as_deref(),
                    *max_cost,
                    *max_turns,
                    *webhook,
                );
            }
            // Run and Daemon need the LLM — handled after provider setup.
            _ => {}
        }
    }

    // Attach mode: connect to a running serve instance (no local API key needed).
    if let Some(ref session_filter) = cli.attach {
        return attach::run_attach(cli.port, session_filter).await;
    }

    // Run setup wizard on first launch (no config file). Skip for non-interactive modes.
    if cli.prompt.is_none()
        && !cli.dump_system_prompt
        && !cli.serve
        && !cli.acp
        && cli.command.is_none()
        && ui::setup::needs_setup()
    {
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

    if !has_key && cli.prompt.is_none() && !cli.dump_system_prompt && !cli.serve && !cli.acp {
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
        "azure" | "azure-openai" => ProviderKind::AzureOpenAi,
        _ => detect_provider(&config.api.model, &config.api.base_url),
    };

    // Override base URL if the detected provider has a known default.
    if cli.api_base_url.is_none()
        && let Some(default_url) = provider_kind.default_base_url()
    {
        config.api.base_url = default_url.to_string();
    }
    let llm: Arc<dyn agent_code_lib::llm::provider::Provider> = match provider_kind {
        ProviderKind::AzureOpenAi => {
            Arc::new(agent_code_lib::llm::azure_openai::AzureOpenAiProvider::new(
                &config.api.base_url,
                api_key,
            ))
        }
        _ => match provider_kind.wire_format() {
            WireFormat::Anthropic => {
                Arc::new(agent_code_lib::llm::anthropic::AnthropicProvider::new(
                    &config.api.base_url,
                    api_key,
                ))
            }
            WireFormat::OpenAiCompatible => Arc::new(
                agent_code_lib::llm::openai::OpenAiProvider::new(&config.api.base_url, api_key),
            ),
        },
    };
    tracing::info!(
        "Using {:?} provider at {}",
        provider_kind,
        config.api.base_url
    );

    // Validate API key in background (non-blocking).
    // The old approach used a synchronous curl subprocess with 5s timeout,
    // blocking startup. Now we spawn it as a background task and only
    // interrupt if the key is actually invalid.
    let api_key_check_handle = if !config.api.base_url.contains("localhost")
        && !config.api.base_url.contains("127.0.0.1")
        && cli.prompt.is_none()
        && !cli.dump_system_prompt
        && !cli.serve
        && !cli.acp
    {
        let check_url = format!("{}/models", config.api.base_url);
        let check_key = api_key.to_string();
        Some(tokio::spawn(async move {
            tokio::process::Command::new("curl")
                .args([
                    "-s",
                    "-o",
                    "/dev/null",
                    "-w",
                    "%{http_code}",
                    "--max-time",
                    "3",
                    "-H",
                    &format!("Authorization: Bearer {check_key}"),
                    "-H",
                    &format!("x-api-key: {check_key}"),
                    &check_url,
                ])
                .output()
                .await
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .is_some_and(|code| code.trim() == "401" || code.trim() == "403")
        }))
    } else {
        None
    };

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
    let llm_for_schedule = llm.clone();
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

    // Check background API key validation result before entering interactive mode.
    if let Some(handle) = api_key_check_handle
        && let Ok(Ok(true)) =
            tokio::time::timeout(std::time::Duration::from_millis(500), handle).await
    {
        eprintln!(
            "\nWarning: API key may be invalid (rejected by {}). \
             Run setup with `agent --api-key <key>` to update.\n",
            config.api.base_url
        );
    }

    // Install Ctrl+C handler for graceful cancellation.
    engine.install_signal_handler();

    // Handle schedule/daemon subcommands that need the LLM.
    if let Some(SubCommand::Schedule {
        action: ScheduleAction::Run { name },
    }) = &cli.command
    {
        return handle_schedule_run(name, &llm_for_schedule, &config).await;
    }
    if let Some(SubCommand::Daemon { webhook_port }) = &cli.command {
        return daemon::run_daemon(llm_for_schedule, config, *webhook_port).await;
    }

    // Serve mode: start HTTP API server.
    if cli.serve {
        return serve::run_server(engine, cli.port).await;
    }

    // ACP mode: start stdio JSON-RPC server for IDE integrations.
    if cli.acp {
        return acp::run_acp(engine).await;
    }

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

// ---------------------------------------------------------------------------
// Schedule subcommand handlers
// ---------------------------------------------------------------------------

fn handle_schedule_list() -> anyhow::Result<()> {
    let store =
        agent_code_lib::schedule::ScheduleStore::open().map_err(|e| anyhow::anyhow!("{e}"))?;
    let schedules = store.list();

    if schedules.is_empty() {
        println!("No schedules configured.");
        println!("\nAdd one with:");
        println!("  agent schedule add \"0 9 * * *\" --prompt \"run tests\" --name daily-tests");
        return Ok(());
    }

    println!(
        "{:<20} {:<7} {:<20} {:<16} PROMPT",
        "NAME", "STATUS", "CRON", "LAST RUN"
    );
    println!("{}", "-".repeat(90));
    for s in &schedules {
        let status = if s.enabled { "active" } else { "paused" };
        let last = s
            .last_run_at
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "never".into());
        let prompt = if s.prompt.len() > 30 {
            format!("{}...", &s.prompt[..27])
        } else {
            s.prompt.clone()
        };
        println!(
            "{:<20} {:<7} {:<20} {:<16} {}",
            s.name, status, s.cron, last, prompt
        );
    }
    println!("\n{} schedule(s)", schedules.len());
    Ok(())
}

fn handle_schedule_remove(name: &str) -> anyhow::Result<()> {
    let store =
        agent_code_lib::schedule::ScheduleStore::open().map_err(|e| anyhow::anyhow!("{e}"))?;
    store.remove(name).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("Removed schedule '{name}'");
    Ok(())
}

fn handle_schedule_toggle(name: &str, enabled: bool) -> anyhow::Result<()> {
    let store =
        agent_code_lib::schedule::ScheduleStore::open().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut sched = store.load(name).map_err(|e| anyhow::anyhow!("{e}"))?;
    sched.enabled = enabled;
    store.save(&sched).map_err(|e| anyhow::anyhow!("{e}"))?;
    let verb = if enabled { "enabled" } else { "disabled" };
    println!("Schedule '{name}' {verb}");
    Ok(())
}

fn handle_schedule_add(
    name: &str,
    cron: &str,
    prompt: &str,
    model: Option<&str>,
    max_cost: Option<f64>,
    max_turns: Option<usize>,
    webhook: bool,
) -> anyhow::Result<()> {
    // Validate cron expression.
    agent_code_lib::schedule::CronExpr::parse(cron)
        .map_err(|e| anyhow::anyhow!("Invalid cron expression: {e}"))?;

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());

    let webhook_secret = if webhook {
        Some(uuid::Uuid::new_v4().to_string().replace('-', ""))
    } else {
        None
    };

    let schedule = agent_code_lib::schedule::Schedule {
        name: name.to_string(),
        cron: cron.to_string(),
        prompt: prompt.to_string(),
        cwd,
        enabled: true,
        model: model.map(String::from),
        permission_mode: None,
        max_cost_usd: max_cost,
        max_turns,
        created_at: chrono::Utc::now(),
        last_run_at: None,
        last_result: None,
        webhook_secret: webhook_secret.clone(),
    };

    let store =
        agent_code_lib::schedule::ScheduleStore::open().map_err(|e| anyhow::anyhow!("{e}"))?;
    store.save(&schedule).map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("Created schedule '{name}'");
    println!("  Cron: {cron}");
    println!("  Prompt: {prompt}");
    if let Some(ref secret) = webhook_secret {
        // Intentionally shown once at creation — this is the only time the
        // user sees the full secret, similar to API key provisioning flows.
        println!("  Webhook: POST /trigger?secret={secret}"); // codeql[cleartext-logging]: intentional one-time display
    }
    println!("\nStart the daemon to begin executing:");
    println!("  agent daemon");
    Ok(())
}

async fn handle_schedule_run(
    name: &str,
    llm: &Arc<dyn agent_code_lib::llm::provider::Provider>,
    config: &Config,
) -> anyhow::Result<()> {
    let store =
        agent_code_lib::schedule::ScheduleStore::open().map_err(|e| anyhow::anyhow!("{e}"))?;
    let schedule = store.load(name).map_err(|e| anyhow::anyhow!("{e}"))?;

    eprintln!("Running schedule '{name}'...\n");

    // Use a stdout sink so the user sees streaming output.
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

    let executor = agent_code_lib::schedule::ScheduleExecutor::new(llm.clone(), config.clone());
    let outcome = executor
        .run_once(&schedule, &StdoutSink)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Update last_run.
    let mut updated = schedule;
    updated.last_run_at = Some(chrono::Utc::now());
    updated.last_result = Some(agent_code_lib::schedule::storage::RunResult {
        started_at: chrono::Utc::now(),
        finished_at: chrono::Utc::now(),
        success: outcome.success,
        turns: outcome.turns,
        cost_usd: outcome.cost_usd,
        summary: outcome.response_summary.clone(),
        session_id: outcome.session_id.clone(),
    });
    let _ = store.save(&updated);

    println!();
    // Session ID is a non-secret UUID prefix shown for /resume.
    eprintln!(
        "\nDone: {} turns, ${:.4}, session {}",
        outcome.turns, outcome.cost_usd, outcome.session_id // codeql[cleartext-logging]: non-secret session ID for /resume
    );
    Ok(())
}
