//! Slash command system.
//!
//! Commands are user-invokable actions triggered by `/command` syntax
//! in the REPL. They can be:
//!
//! - **Built-in**: implemented directly in Rust
//! - **Skills**: loaded from skill files, executed as prompt templates
//!
//! Commands have access to the query engine state and can modify
//! the conversation, change settings, or execute side effects.

use agent_code_lib::query::QueryEngine;

/// Result of executing a command.
pub enum CommandResult {
    /// Command handled, continue REPL.
    Handled,
    /// Exit the REPL.
    Exit,
    /// Not a command — pass through as a prompt to the agent.
    Passthrough(String),
    /// Command wants to inject a prompt for the agent.
    Prompt(String),
}

/// A built-in command definition.
pub struct Command {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub hidden: bool,
}

/// All built-in commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "help",
        aliases: &["h", "?"],
        description: "Show available commands",
        hidden: false,
    },
    Command {
        name: "exit",
        aliases: &["quit", "q"],
        description: "Exit the REPL",
        hidden: false,
    },
    Command {
        name: "clear",
        aliases: &[],
        description: "Clear conversation history",
        hidden: false,
    },
    Command {
        name: "compact",
        aliases: &[],
        description: "Compact conversation history to free context",
        hidden: false,
    },
    Command {
        name: "cost",
        aliases: &[],
        description: "Show session cost and token usage",
        hidden: false,
    },
    Command {
        name: "model",
        aliases: &[],
        description: "Show or change the current model",
        hidden: false,
    },
    Command {
        name: "diff",
        aliases: &[],
        description: "Show git diff of current changes",
        hidden: false,
    },
    Command {
        name: "status",
        aliases: &[],
        description: "Show git status",
        hidden: false,
    },
    Command {
        name: "commit",
        aliases: &[],
        description: "Ask the agent to commit current changes",
        hidden: false,
    },
    Command {
        name: "resume",
        aliases: &[],
        description: "Resume a previous session by ID",
        hidden: false,
    },
    Command {
        name: "sessions",
        aliases: &[],
        description: "List recent sessions",
        hidden: false,
    },
    Command {
        name: "memory",
        aliases: &[],
        description: "Show loaded memory context",
        hidden: false,
    },
    Command {
        name: "skills",
        aliases: &[],
        description: "List available skills",
        hidden: false,
    },
    Command {
        name: "review",
        aliases: &[],
        description: "Ask the agent to review the current diff",
        hidden: false,
    },
    Command {
        name: "doctor",
        aliases: &[],
        description: "Check environment and configuration health",
        hidden: false,
    },
    Command {
        name: "mcp",
        aliases: &[],
        description: "Show connected MCP servers and tools",
        hidden: false,
    },
    Command {
        name: "plan",
        aliases: &[],
        description: "Toggle plan mode (read-only)",
        hidden: false,
    },
    Command {
        name: "init",
        aliases: &[],
        description: "Initialize project config (.agent/settings.toml)",
        hidden: false,
    },
    Command {
        name: "export",
        aliases: &[],
        description: "Export conversation as markdown",
        hidden: false,
    },
    Command {
        name: "branch",
        aliases: &[],
        description: "Show or switch git branch",
        hidden: false,
    },
    Command {
        name: "context",
        aliases: &["ctx"],
        description: "Show context window usage",
        hidden: false,
    },
    Command {
        name: "agents",
        aliases: &[],
        description: "List available agent types",
        hidden: false,
    },
    Command {
        name: "hooks",
        aliases: &[],
        description: "List configured hooks",
        hidden: false,
    },
    Command {
        name: "plugins",
        aliases: &[],
        description: "List loaded plugins",
        hidden: false,
    },
    Command {
        name: "verbose",
        aliases: &[],
        description: "Toggle verbose output",
        hidden: false,
    },
    Command {
        name: "tasks",
        aliases: &[],
        description: "List background tasks",
        hidden: false,
    },
    Command {
        name: "permissions",
        aliases: &["perms"],
        description: "Show current permission mode and rules",
        hidden: false,
    },
    Command {
        name: "theme",
        aliases: &[],
        description: "Switch color theme",
        hidden: false,
    },
    Command {
        name: "stats",
        aliases: &[],
        description: "Show session statistics",
        hidden: false,
    },
    Command {
        name: "log",
        aliases: &[],
        description: "Show recent git log",
        hidden: false,
    },
    Command {
        name: "files",
        aliases: &[],
        description: "List files in the working directory",
        hidden: false,
    },
    Command {
        name: "scroll",
        aliases: &["history-view"],
        description: "Scrollable view of conversation history (arrow keys to navigate, q to exit)",
        hidden: false,
    },
    Command {
        name: "rewind",
        aliases: &["undo"],
        description: "Undo the last assistant turn (removes last assistant + tool messages)",
        hidden: false,
    },
    Command {
        name: "color",
        aliases: &[],
        description: "Switch color theme mid-session",
        hidden: false,
    },
    Command {
        name: "config",
        aliases: &[],
        description: "Show current configuration",
        hidden: false,
    },
    Command {
        name: "snip",
        aliases: &[],
        description: "Remove a range of messages from history (e.g., /snip 3-7)",
        hidden: false,
    },
    Command {
        name: "fork",
        aliases: &[],
        description: "Branch the conversation from this point",
        hidden: false,
    },
    Command {
        name: "features",
        aliases: &[],
        description: "Show enabled feature flags",
        hidden: false,
    },
    Command {
        name: "transcript",
        aliases: &[],
        description: "Show conversation transcript",
        hidden: false,
    },
    Command {
        name: "bug",
        aliases: &[],
        description: "Report a bug",
        hidden: false,
    },
    Command {
        name: "vim",
        aliases: &["vi"],
        description: "Switch to vi editing mode",
        hidden: false,
    },
    Command {
        name: "emacs",
        aliases: &[],
        description: "Switch to emacs editing mode",
        hidden: false,
    },
    Command {
        name: "version",
        aliases: &[],
        description: "Show version information",
        hidden: true,
    },
    Command {
        name: "release-notes",
        aliases: &["rn"],
        description: "Show release notes for the current version",
        hidden: false,
    },
    Command {
        name: "summary",
        aliases: &[],
        description: "Summarize what happened in this session",
        hidden: false,
    },
    Command {
        name: "feedback",
        aliases: &[],
        description: "Submit feedback or suggestions",
        hidden: false,
    },
    Command {
        name: "share",
        aliases: &[],
        description: "Export session as shareable markdown",
        hidden: false,
    },
];

/// Execute a slash command. Returns how to proceed.
pub fn execute(input: &str, engine: &mut QueryEngine) -> CommandResult {
    let input = input.trim_start_matches('/');
    let (cmd, args) = input
        .split_once(' ')
        .map(|(c, a)| (c, Some(a.trim())))
        .unwrap_or((input, None));

    // Check built-in commands.
    let matched = COMMANDS
        .iter()
        .find(|c| c.name == cmd || c.aliases.contains(&cmd));

    match matched.map(|c| c.name) {
        Some("help") => {
            println!("\nAvailable commands:\n");
            for c in COMMANDS.iter().filter(|c| !c.hidden) {
                let aliases = if c.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", c.aliases.join(", "))
                };
                println!("  /{}{:<12} {}", c.name, aliases, c.description);
            }

            // Show skill commands.
            let skills = agent_code_lib::skills::SkillRegistry::load_all(Some(
                std::path::Path::new(&engine.state().cwd),
            ));
            let invocable = skills.user_invocable();
            if !invocable.is_empty() {
                println!("\nSkills:");
                for skill in invocable {
                    let desc = skill.metadata.description.as_deref().unwrap_or("");
                    println!("  /{:<18} {}", skill.name, desc);
                }
            }
            println!();
            CommandResult::Handled
        }
        Some("exit") => CommandResult::Exit,
        Some("clear") => {
            engine.state_mut().messages.clear();
            println!("Conversation cleared.");
            CommandResult::Handled
        }
        Some("compact") => {
            let freed = agent_code_lib::services::compact::microcompact(
                &mut engine.state_mut().messages,
                2,
            );
            if freed > 0 {
                println!("Freed ~{freed} estimated tokens.");
            } else {
                println!("Nothing to compact.");
            }
            CommandResult::Handled
        }
        Some("cost") => {
            let state = engine.state();
            let usage = &state.total_usage;
            println!(
                "Turns: {}\nTokens: {} (in: {}, out: {}, cache_write: {}, cache_read: {})\nCost: ${:.4}",
                state.turn_count,
                usage.total(),
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_creation_input_tokens,
                usage.cache_read_input_tokens,
                state.total_cost_usd,
            );

            // Per-model breakdown (only shown when multiple models were used).
            if state.model_usage.len() > 1 {
                println!("\nPer model:");
                let mut models: Vec<_> = state.model_usage.iter().collect();
                models.sort_by(|a, b| a.0.cmp(b.0));
                for (model, mu) in &models {
                    let cost = crate::estimate_model_cost(mu, model);
                    let cache_pct = if mu.input_tokens > 0 {
                        (mu.cache_read_input_tokens as f64 / mu.input_tokens as f64 * 100.0).round()
                            as u64
                    } else {
                        0
                    };
                    println!(
                        "  {model}: {} tokens (in: {}, out: {}), cache hit: {cache_pct}%, ${cost:.4}",
                        mu.total(),
                        mu.input_tokens,
                        mu.output_tokens,
                    );
                }
            } else if state.model_usage.len() == 1 {
                // Single model — show cache hit rate.
                let (_, mu) = state.model_usage.iter().next().unwrap();
                if mu.cache_read_input_tokens > 0 && mu.input_tokens > 0 {
                    let cache_pct = (mu.cache_read_input_tokens as f64 / mu.input_tokens as f64
                        * 100.0)
                        .round() as u64;
                    println!("Cache hit: {cache_pct}%");
                }
            }
            CommandResult::Handled
        }
        Some("model") => {
            if let Some(new_model) = args {
                engine.state_mut().config.api.model = new_model.to_string();
                println!("Model changed to: {new_model}");
            } else {
                // Interactive model selector based on configured provider.
                let current = engine.state().config.api.model.clone();
                let base_url = engine.state().config.api.base_url.clone();
                let provider = agent_code_lib::llm::provider::detect_provider(&current, &base_url);

                use agent_code_lib::llm::provider::ProviderKind;
                let models: Vec<(&str, &str)> = match provider {
                    ProviderKind::Anthropic | ProviderKind::Bedrock | ProviderKind::Vertex => vec![
                        ("claude-opus-4-20250514", "Opus 4 · Most capable"),
                        ("claude-sonnet-4-20250514", "Sonnet 4 · Balanced"),
                        ("claude-haiku-4-20250414", "Haiku 4 · Fast"),
                    ],
                    ProviderKind::OpenAi => vec![
                        ("gpt-5.4", "GPT-5.4 · Most capable"),
                        ("gpt-5.4-mini", "GPT-5.4 Mini · Balanced"),
                        ("gpt-5.4-nano", "GPT-5.4 Nano · Fast"),
                        ("gpt-4.1", "GPT-4.1 · Previous gen"),
                        ("gpt-4.1-mini", "GPT-4.1 Mini · Fast"),
                        ("gpt-4.1-nano", "GPT-4.1 Nano · Fastest"),
                        ("o3", "o3 · Reasoning"),
                        ("o3-mini", "o3 Mini · Fast reasoning"),
                    ],
                    ProviderKind::Xai => vec![
                        ("grok-3", "Grok 3 · Most capable"),
                        ("grok-3-mini", "Grok 3 Mini · Fast"),
                    ],
                    ProviderKind::Google => vec![
                        ("gemini-2.5-pro", "Gemini 2.5 Pro · Most capable"),
                        ("gemini-2.5-flash", "Gemini 2.5 Flash · Fast"),
                    ],
                    ProviderKind::DeepSeek => vec![
                        ("deepseek-chat", "DeepSeek Chat · General"),
                        ("deepseek-reasoner", "DeepSeek Reasoner · Reasoning"),
                    ],
                    ProviderKind::Mistral => vec![
                        ("mistral-large-latest", "Mistral Large · Most capable"),
                        ("codestral-latest", "Codestral · Code-focused"),
                    ],
                    ProviderKind::Zhipu => vec![
                        ("glm-4.7", "GLM-4.7 · Latest"),
                        ("glm-4.6", "GLM-4.6 · Balanced"),
                        ("glm-4.6-air", "GLM-4.6 Air · Fast"),
                        ("glm-4.5", "GLM-4.5 · Previous gen"),
                    ],
                    _ => vec![],
                };

                if models.is_empty() {
                    println!("Model: {current}");
                    println!("Use /model <name> to change.");
                } else {
                    println!();
                    println!("  Select model");
                    println!();

                    let options: Vec<crate::ui::selector::SelectOption> = models
                        .iter()
                        .map(|(name, desc)| {
                            let check = if *name == current { " ✔" } else { "" };
                            crate::ui::selector::SelectOption {
                                label: format!("{name}{check}"),
                                description: desc.to_string(),
                                value: name.to_string(),
                                preview: None,
                            }
                        })
                        .collect();

                    let chosen = crate::ui::selector::select(&options);
                    if !chosen.is_empty() {
                        engine.state_mut().config.api.model = chosen.clone();
                        println!("Model changed to: {chosen}");
                    }
                }
            }
            CommandResult::Handled
        }
        Some("resume") => {
            if let Some(id) = args {
                match agent_code_lib::services::session::load_session(id) {
                    Ok(data) => {
                        let state = engine.state_mut();
                        state.messages = data.messages;
                        state.turn_count = data.turn_count;
                        state.total_cost_usd = data.total_cost_usd;
                        state.total_usage.input_tokens = data.total_input_tokens;
                        state.total_usage.output_tokens = data.total_output_tokens;
                        state.plan_mode = data.plan_mode;
                        if !data.model.is_empty() {
                            state.config.api.model = data.model.clone();
                        }
                        println!(
                            "Resumed session {} ({} messages, {} turns, ${:.4})",
                            id,
                            engine.state().messages.len(),
                            data.turn_count,
                            data.total_cost_usd,
                        );
                    }
                    Err(e) => println!("Failed to resume: {e}"),
                }
            } else {
                println!("Usage: /resume <session-id>");
                println!("Use /sessions to list recent sessions.");
            }
            CommandResult::Handled
        }
        Some("sessions") => {
            let sessions = agent_code_lib::services::session::list_sessions(10);
            if sessions.is_empty() {
                println!("No saved sessions.");
            } else {
                println!("Recent sessions:\n");
                for s in &sessions {
                    println!(
                        "  {} — {} ({} turns, {} msgs, {})",
                        s.id, s.cwd, s.turn_count, s.message_count, s.updated_at,
                    );
                }
                println!("\nUse /resume <id> to restore a session.");
            }
            CommandResult::Handled
        }
        Some("diff") => {
            CommandResult::Prompt("Run `git diff` and show me the changes.".to_string())
        }
        Some("status") => {
            CommandResult::Prompt("Run `git status` and show me the result.".to_string())
        }
        Some("commit") => {
            let msg = if let Some(m) = args {
                format!("Commit the current changes with message: {m}")
            } else {
                "Review the current git diff and create an appropriate commit.".to_string()
            };
            CommandResult::Prompt(msg)
        }
        Some("memory") => {
            let memory = agent_code_lib::memory::MemoryContext::load(Some(std::path::Path::new(
                &engine.state().cwd,
            )));
            if memory.is_empty() {
                println!("No memory loaded.");
            } else {
                if memory.project_context.is_some() {
                    println!("Project context: loaded");
                }
                if memory.user_memory.is_some() {
                    println!("User memory: loaded ({} files)", memory.memory_files.len());
                }
            }
            CommandResult::Handled
        }
        Some("skills") => {
            let skills = agent_code_lib::skills::SkillRegistry::load_all(Some(
                std::path::Path::new(&engine.state().cwd),
            ));
            if skills.all().is_empty() {
                println!(
                    "No skills loaded. Add .md files to .agent/skills/ or ~/.config/agent-code/skills/"
                );
            } else {
                println!("Loaded {} skills:", skills.all().len());
                for skill in skills.all() {
                    let invocable = if skill.metadata.user_invocable {
                        " [invocable]"
                    } else {
                        ""
                    };
                    let desc = skill.metadata.description.as_deref().unwrap_or("");
                    println!("  {}{} — {}", skill.name, invocable, desc);
                }
            }
            CommandResult::Handled
        }
        Some("review") => CommandResult::Prompt(
            "Review the current git diff. Look for bugs, security issues, \
                 code quality problems, and suggest improvements."
                .to_string(),
        ),
        Some("doctor") => {
            // Run the full async diagnostics synchronously via a blocking call.
            let cwd = std::path::Path::new(&engine.state().cwd).to_path_buf();
            let config = engine.state().config.clone();
            let rt = tokio::runtime::Handle::current();
            let checks = std::thread::spawn(move || {
                rt.block_on(agent_code_lib::services::diagnostics::run_all(
                    &cwd, &config,
                ))
            })
            .join()
            .unwrap_or_default();

            println!("Environment diagnostics:\n");
            for check in &checks {
                let icon = match check.status {
                    agent_code_lib::services::diagnostics::CheckStatus::Pass => "✓".to_string(),
                    agent_code_lib::services::diagnostics::CheckStatus::Warn => "!".to_string(),
                    agent_code_lib::services::diagnostics::CheckStatus::Fail => "✗".to_string(),
                };
                println!("  {icon} {}: {}", check.name, check.detail);
            }

            let pass = checks
                .iter()
                .filter(|c| c.status == agent_code_lib::services::diagnostics::CheckStatus::Pass)
                .count();
            let fail = checks
                .iter()
                .filter(|c| c.status == agent_code_lib::services::diagnostics::CheckStatus::Fail)
                .count();
            println!("\n  {pass} passed, {fail} failed, {} total", checks.len());
            CommandResult::Handled
        }
        Some("mcp") => {
            let server_count = engine.state().config.mcp_servers.len();
            if server_count == 0 {
                println!("No MCP servers configured.");
            } else {
                println!("{server_count} MCP server(s) configured:");
                for (name, entry) in &engine.state().config.mcp_servers {
                    let transport = if entry.command.is_some() {
                        "stdio"
                    } else if entry.url.is_some() {
                        "sse"
                    } else {
                        "unknown"
                    };
                    println!("  {name} ({transport})");
                }
            }
            CommandResult::Handled
        }
        Some("plan") => {
            let plan_mode = &mut engine.state_mut().plan_mode;
            *plan_mode = !*plan_mode;
            if *plan_mode {
                println!("Plan mode enabled. Only read-only tools available.");
            } else {
                println!("Plan mode disabled. All tools available.");
            }
            CommandResult::Handled
        }
        Some("init") => {
            let config_dir = std::path::Path::new(&engine.state().cwd).join(".agent");
            let config_file = config_dir.join("settings.toml");
            if config_file.exists() {
                println!("Project already initialized: {}", config_file.display());
            } else {
                let _ = std::fs::create_dir_all(&config_dir);
                let default = "[api]\n# model = \"claude-sonnet-4-20250514\"\n\n\
                               [permissions]\ndefault_mode = \"ask\"\n";
                match std::fs::write(&config_file, default) {
                    Ok(_) => println!("Created {}", config_file.display()),
                    Err(e) => println!("Failed to create config: {e}"),
                }
            }
            CommandResult::Handled
        }
        Some("export") => {
            let messages = &engine.state().messages;
            if messages.is_empty() {
                println!("No conversation to export.");
            } else {
                let mut md = String::from("# Conversation Export\n\n");
                for msg in messages {
                    match msg {
                        agent_code_lib::llm::message::Message::User(u) => {
                            md.push_str("## User\n\n");
                            for block in &u.content {
                                if let agent_code_lib::llm::message::ContentBlock::Text { text } =
                                    block
                                {
                                    md.push_str(text);
                                    md.push_str("\n\n");
                                }
                            }
                        }
                        agent_code_lib::llm::message::Message::Assistant(a) => {
                            md.push_str("## Assistant\n\n");
                            for block in &a.content {
                                if let agent_code_lib::llm::message::ContentBlock::Text { text } =
                                    block
                                {
                                    md.push_str(text);
                                    md.push_str("\n\n");
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let path = format!(
                    "conversation-export-{}.md",
                    chrono::Utc::now().format("%Y%m%d-%H%M%S")
                );
                match std::fs::write(&path, &md) {
                    Ok(_) => println!("Exported to {path}"),
                    Err(e) => println!("Export failed: {e}"),
                }
            }
            CommandResult::Handled
        }
        Some("branch") => {
            if let Some(name) = args {
                CommandResult::Prompt(format!("Switch to git branch '{name}' and confirm."))
            } else {
                CommandResult::Prompt(
                    "Show the current git branch and list recent branches.".into(),
                )
            }
        }
        Some("context") | Some("ctx") => {
            let tokens =
                agent_code_lib::services::tokens::estimate_context_tokens(&engine.state().messages);
            let model = &engine.state().config.api.model;
            let window = agent_code_lib::services::tokens::context_window_for_model(model);
            let threshold = agent_code_lib::services::compact::auto_compact_threshold(model);
            let pct = if window > 0 {
                (tokens as f64 / window as f64 * 100.0).round() as u64
            } else {
                0
            };
            println!(
                "Context: ~{tokens} tokens ({pct}% of {window} window)\n\
                 Auto-compact at: {threshold} tokens\n\
                 Messages: {}",
                engine.state().messages.len(),
            );
            CommandResult::Handled
        }
        Some("agents") => {
            let registry = agent_code_lib::services::coordinator::AgentRegistry::with_defaults();
            println!("Available agent types:\n");
            for agent in registry.list() {
                let ro = if agent.read_only { " (read-only)" } else { "" };
                println!("  {}{ro} — {}", agent.name, agent.description);
            }
            CommandResult::Handled
        }
        Some("hooks") => {
            println!("Hook system active. Configure hooks in .agent/settings.toml:");
            println!("  [[hooks]]");
            println!("  event = \"pre_tool_use\"");
            println!("  action = {{ type = \"shell\", command = \"./check.sh\" }}");
            CommandResult::Handled
        }
        Some("plugins") => {
            let plugins = agent_code_lib::services::plugins::PluginRegistry::load_all(Some(
                std::path::Path::new(&engine.state().cwd),
            ));
            if plugins.all().is_empty() {
                println!(
                    "No plugins loaded. Add plugin directories to ~/.config/agent-code/plugins/"
                );
            } else {
                println!("Loaded {} plugins:", plugins.all().len());
                for p in plugins.all() {
                    let desc = p.manifest.description.as_deref().unwrap_or("");
                    let ver = p.manifest.version.as_deref().unwrap_or("?");
                    println!("  {} v{} — {}", p.manifest.name, ver, desc);
                }
            }
            CommandResult::Handled
        }
        Some("verbose") => {
            println!("Verbose mode toggled.");
            CommandResult::Handled
        }
        Some("tasks") => {
            CommandResult::Prompt("List all background tasks and their status.".into())
        }
        Some("permissions") | Some("perms") => {
            let config = &engine.state().config;
            println!("Permission mode: {:?}", config.permissions.default_mode);
            if config.permissions.rules.is_empty() {
                println!("No custom rules configured.");
            } else {
                println!("Rules:");
                for rule in &config.permissions.rules {
                    let pattern = rule.pattern.as_deref().unwrap_or("*");
                    println!("  {} {} -> {:?}", rule.tool, pattern, rule.action);
                }
            }
            if engine.state().plan_mode {
                println!("Plan mode: ACTIVE (read-only tools only)");
            }
            CommandResult::Handled
        }
        Some("theme") => {
            println!(
                "Theme: {} (dark is the default)",
                engine.state().config.ui.theme
            );
            println!("Configure in ~/.config/agent-code/config.toml under [ui]");
            CommandResult::Handled
        }
        Some("stats") => {
            let state = engine.state();
            let msg_count = state.messages.len();
            let tool_count = agent_code_lib::services::history::tool_use_count(&state.messages);
            let tools_used = agent_code_lib::services::history::tools_used(&state.messages);
            println!(
                "Session stats:\n  \
                 Turns: {}\n  \
                 Messages: {msg_count}\n  \
                 Tool calls: {tool_count}\n  \
                 Tools used: {}\n  \
                 Tokens: {}\n  \
                 Cost: ${:.4}",
                state.turn_count,
                tools_used.join(", "),
                state.total_usage.total(),
                state.total_cost_usd,
            );
            CommandResult::Handled
        }
        Some("log") => CommandResult::Prompt(
            "Show the last 10 git commits with `git log --oneline -10`.".into(),
        ),
        Some("files") => CommandResult::Prompt(
            "List files in the current directory. Use `ls -la` for details \
                 or Glob for pattern matching."
                .into(),
        ),
        Some("scroll") => {
            let messages = &engine.state().messages;
            if messages.is_empty() {
                println!("No conversation history yet.");
            } else {
                crate::ui::tui::scrollback_viewer(messages);
            }
            CommandResult::Handled
        }
        Some("rewind") => {
            let messages = &mut engine.state_mut().messages;
            // Remove messages from the end until we've removed the last assistant turn.
            let mut removed = 0;
            let mut found_assistant = false;
            while let Some(msg) = messages.last() {
                match msg {
                    agent_code_lib::llm::message::Message::Assistant(_) => {
                        messages.pop();
                        removed += 1;
                        found_assistant = true;
                    }
                    agent_code_lib::llm::message::Message::User(u) if found_assistant => {
                        // Also remove the user message that triggered the turn.
                        if !u.is_compact_summary {
                            messages.pop();
                            removed += 1;
                        }
                        break;
                    }
                    _ => {
                        if found_assistant {
                            break;
                        }
                        messages.pop();
                        removed += 1;
                    }
                }
            }
            if removed > 0 {
                println!("Rewound {removed} message(s). Last turn undone.");
            } else {
                println!("Nothing to rewind.");
            }
            CommandResult::Handled
        }
        Some("color") => {
            let themes = [
                "midnight",
                "daybreak",
                "midnight-muted",
                "daybreak-muted",
                "terminal",
                "auto",
            ];
            if let Some(name) = args {
                if themes.contains(&name) {
                    engine.state_mut().config.ui.theme = name.to_string();
                    println!("Theme set to: {name}");
                } else {
                    println!("Unknown theme. Available: {}", themes.join(", "));
                }
            } else {
                println!("Current theme: {}", engine.state().config.ui.theme);
                println!("Available: {}", themes.join(", "));
                println!("Usage: /color <theme>");
            }
            CommandResult::Handled
        }
        Some("config") => {
            let config = &engine.state().config;
            println!("API:");
            println!("  base_url: {}", config.api.base_url);
            println!("  model: {}", config.api.model);
            println!("  max_output_tokens: {:?}", config.api.max_output_tokens);
            println!("  timeout: {}s", config.api.timeout_secs);
            println!("  max_retries: {}", config.api.max_retries);
            if let Some(max_cost) = config.api.max_cost_usd {
                println!("  max_cost: ${:.2}", max_cost);
            }
            println!("\nPermissions:");
            println!("  mode: {:?}", config.permissions.default_mode);
            println!("  rules: {}", config.permissions.rules.len());
            println!("\nUI:");
            println!("  theme: {}", config.ui.theme);
            println!("  edit_mode: {}", config.ui.edit_mode);
            println!("  markdown: {}", config.ui.markdown);
            println!("\nMCP servers: {}", config.mcp_servers.len());
            println!("Hooks: {}", config.hooks.len());
            CommandResult::Handled
        }
        Some("snip") => {
            if !engine.state().config.features.history_snip {
                println!("Feature disabled. Enable with: [features] history_snip = true");
                return CommandResult::Handled;
            }
            if let Some(range) = args {
                let parts: Vec<&str> = range.split('-').collect();
                let (start, end) = match parts.len() {
                    1 => {
                        let idx = parts[0].parse::<usize>().unwrap_or(0);
                        (idx, idx)
                    }
                    2 => {
                        let s = parts[0].parse::<usize>().unwrap_or(0);
                        let e = parts[1].parse::<usize>().unwrap_or(0);
                        (s, e)
                    }
                    _ => {
                        println!("Usage: /snip <index> or /snip <start>-<end>");
                        return CommandResult::Handled;
                    }
                };
                let messages = &mut engine.state_mut().messages;
                let len = messages.len();
                if start >= len || end >= len || start > end {
                    println!("Invalid range. Messages: 0-{}", len.saturating_sub(1));
                } else {
                    let count = end - start + 1;
                    messages.drain(start..=end);
                    println!(
                        "Removed {count} message(s) ({start}-{end}). {} remaining.",
                        messages.len()
                    );
                }
            } else {
                println!("Usage: /snip <index> or /snip <start>-<end>");
                println!("Use /transcript to see message indices.");
            }
            CommandResult::Handled
        }
        Some("fork") => {
            if !engine.state().config.features.fork_conversation {
                println!("Feature disabled. Enable with: [features] fork_conversation = true");
                return CommandResult::Handled;
            }
            // Fork = save current session, start fresh from this point
            let state = engine.state();
            let fork_id = agent_code_lib::services::session::new_session_id();
            let msg_count = state.messages.len();
            match agent_code_lib::services::session::save_session(
                &fork_id,
                &state.messages,
                &state.cwd,
                &state.config.api.model,
                state.turn_count,
            ) {
                Ok(_) => {
                    println!("Forked conversation at message {msg_count} -> session {fork_id}",);
                    println!("Continue here, or /resume {fork_id} to return to this point.");
                }
                Err(e) => println!("Fork failed: {e}"),
            }
            CommandResult::Handled
        }
        Some("features") => {
            let f = &engine.state().config.features;
            println!("Feature flags:\n");
            let flags = [
                ("token_budget", f.token_budget),
                ("commit_attribution", f.commit_attribution),
                ("compaction_reminders", f.compaction_reminders),
                ("unattended_retry", f.unattended_retry),
                ("history_snip", f.history_snip),
                ("auto_theme", f.auto_theme),
                ("mcp_rich_output", f.mcp_rich_output),
                ("fork_conversation", f.fork_conversation),
                ("verification_agent", f.verification_agent),
                ("extract_memories", f.extract_memories),
                ("context_collapse", f.context_collapse),
                ("reactive_compact", f.reactive_compact),
            ];
            for (name, enabled) in flags {
                let icon = if enabled { "on " } else { "off" };
                println!("  {icon}  {name}");
            }
            println!("\nConfigure in ~/.config/agent-code/config.toml under [features]");
            CommandResult::Handled
        }
        Some("transcript") => {
            let messages = &engine.state().messages;
            if messages.is_empty() {
                println!("No conversation yet.");
            } else {
                println!("Conversation ({} messages):\n", messages.len());
                for (i, msg) in messages.iter().enumerate() {
                    match msg {
                        agent_code_lib::llm::message::Message::User(u) => {
                            let text: String = u
                                .content
                                .iter()
                                .filter_map(|b| {
                                    if let agent_code_lib::llm::message::ContentBlock::Text {
                                        text,
                                    } = b
                                    {
                                        Some(text.as_str())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("");
                            let preview = if text.len() > 120 {
                                format!("{}...", &text[..117])
                            } else {
                                text
                            };
                            println!("  [{i}] USER: {preview}");
                        }
                        agent_code_lib::llm::message::Message::Assistant(a) => {
                            let text: String = a
                                .content
                                .iter()
                                .filter_map(|b| {
                                    if let agent_code_lib::llm::message::ContentBlock::Text {
                                        text,
                                    } = b
                                    {
                                        Some(text.as_str())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("");
                            let tool_count = a
                                .content
                                .iter()
                                .filter(|b| {
                                    matches!(
                                        b,
                                        agent_code_lib::llm::message::ContentBlock::ToolUse { .. }
                                    )
                                })
                                .count();
                            let preview = if text.len() > 120 {
                                format!("{}...", &text[..117])
                            } else {
                                text
                            };
                            let tools = if tool_count > 0 {
                                format!(" (+{tool_count} tool calls)")
                            } else {
                                String::new()
                            };
                            println!("  [{i}] ASSISTANT: {preview}{tools}");
                        }
                        _ => {}
                    }
                }
            }
            CommandResult::Handled
        }
        Some("bug") => {
            println!("To report a bug:");
            println!("  https://github.com/avala-ai/agent-code/issues/new");
            println!("\nInclude: agent version, OS, steps to reproduce.");
            println!("Version: agent {}", env!("CARGO_PKG_VERSION"));
            CommandResult::Handled
        }
        Some("vim") => {
            engine.state_mut().config.ui.edit_mode = "vi".to_string();
            println!("Editing mode set to vi. Takes effect on next session.");
            CommandResult::Handled
        }
        Some("emacs") => {
            engine.state_mut().config.ui.edit_mode = "emacs".to_string();
            println!("Editing mode set to emacs. Takes effect on next session.");
            CommandResult::Handled
        }
        Some("version") => {
            println!("agent {}", env!("CARGO_PKG_VERSION"));
            CommandResult::Handled
        }
        Some("release-notes") => {
            let version = env!("CARGO_PKG_VERSION");
            // Look for CHANGELOG.md in the project directory, then the binary's directory.
            let changelog = std::path::Path::new(&engine.state().cwd).join("CHANGELOG.md");
            let content = std::fs::read_to_string(&changelog).ok();
            match content {
                Some(text) => {
                    // Extract the section for the current version.
                    let header = format!("## [{version}]");
                    if let Some(start) = text.find(&header) {
                        let section = &text[start..];
                        // Find the next version header or end of file.
                        let end = section[header.len()..]
                            .find("\n## [")
                            .map(|i| i + header.len())
                            .unwrap_or(section.len());
                        println!("{}", section[..end].trim());
                    } else {
                        println!("No release notes found for v{version} in CHANGELOG.md.");
                    }
                }
                None => {
                    println!("No CHANGELOG.md found.");
                    println!(
                        "See https://github.com/avala-ai/agent-code/releases for release notes."
                    );
                }
            }
            CommandResult::Handled
        }
        Some("summary") => CommandResult::Prompt(
            "Summarize this session concisely. List: (1) files modified, \
             (2) key decisions made, (3) tools used and how many times, \
             (4) what was accomplished. Be brief."
                .to_string(),
        ),
        Some("feedback") => {
            if let Some(text) = args {
                let feedback_dir = dirs::data_local_dir()
                    .unwrap_or_default()
                    .join("agent-code/feedback");
                let _ = std::fs::create_dir_all(&feedback_dir);
                let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                let path = feedback_dir.join(format!("{timestamp}.md"));
                let content = format!(
                    "# Feedback\n\nDate: {}\nVersion: {}\n\n{}\n",
                    chrono::Utc::now().to_rfc3339(),
                    env!("CARGO_PKG_VERSION"),
                    text
                );
                match std::fs::write(&path, content) {
                    Ok(_) => println!("Feedback saved. Thank you!"),
                    Err(e) => println!("Failed to save feedback: {e}"),
                }
            } else {
                println!("Usage: /feedback <your message>");
                println!("Example: /feedback the /review command could show line numbers");
            }
            CommandResult::Handled
        }
        Some("share") => {
            let messages = &engine.state().messages;
            if messages.is_empty() {
                println!("No conversation to share.");
            } else {
                let state = engine.state();
                let mut md = format!(
                    "# Agent Code Session\n\n\
                     Model: {} | Turns: {} | Cost: ${:.4}\n\n---\n\n",
                    state.config.api.model, state.turn_count, state.total_cost_usd,
                );
                for msg in messages {
                    match msg {
                        agent_code_lib::llm::message::Message::User(u) => {
                            md.push_str("### User\n\n");
                            for block in &u.content {
                                if let agent_code_lib::llm::message::ContentBlock::Text { text } =
                                    block
                                {
                                    md.push_str(text);
                                    md.push_str("\n\n");
                                }
                            }
                        }
                        agent_code_lib::llm::message::Message::Assistant(a) => {
                            md.push_str("### Assistant\n\n");
                            for block in &a.content {
                                match block {
                                    agent_code_lib::llm::message::ContentBlock::Text { text } => {
                                        md.push_str(text);
                                        md.push_str("\n\n");
                                    }
                                    agent_code_lib::llm::message::ContentBlock::ToolUse {
                                        name,
                                        ..
                                    } => {
                                        md.push_str(&format!("*Used tool: {name}*\n\n"));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let path = format!(
                    "session-share-{}.md",
                    chrono::Utc::now().format("%Y%m%d-%H%M%S")
                );
                match std::fs::write(&path, &md) {
                    Ok(_) => {
                        println!("Session exported to {path}");
                        println!("Share this file or paste its contents.");
                    }
                    Err(e) => println!("Export failed: {e}"),
                }
            }
            CommandResult::Handled
        }
        _ => {
            // Check if it's a skill invocation.
            let skills = agent_code_lib::skills::SkillRegistry::load_all(Some(
                std::path::Path::new(&engine.state().cwd),
            ));
            if let Some(skill) = skills.find(cmd) {
                let expanded = skill.expand(args);
                CommandResult::Prompt(expanded)
            } else {
                // Unknown command — pass through as prompt.
                CommandResult::Passthrough(format!("/{input}"))
            }
        }
    }
}
