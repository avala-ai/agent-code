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

mod heapdump;
mod uninstall;

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
        name: "skill",
        aliases: &[],
        description: "Manage skills: install, remove, search (try /skill help)",
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
        name: "sandbox",
        aliases: &[],
        description: "Show process-level sandbox status and policy",
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
    Command {
        name: "update",
        aliases: &["upgrade"],
        description: "Check for newer versions",
        hidden: false,
    },
    Command {
        name: "uninstall",
        aliases: &[],
        description: "Remove agent-code binary, config, and data",
        hidden: false,
    },
    Command {
        name: "powerup",
        aliases: &["tutorial", "learn"],
        description: "Interactive tutorials to learn agent-code features",
        hidden: false,
    },
    Command {
        name: "effort",
        aliases: &[],
        description: "Rate the effort required for a task (XS/S/M/L/XL)",
        hidden: false,
    },
    Command {
        name: "btw",
        aliases: &[],
        description: "Append a quick note to user memory (e.g. /btw always prefer X over Y)",
        hidden: false,
    },
    Command {
        name: "break-cache",
        aliases: &[],
        description: "Force the next request to skip the prompt cache",
        hidden: false,
    },
    Command {
        name: "heapdump",
        aliases: &[],
        description: "Write a process memory snapshot to disk for debugging",
        hidden: true,
    },
    Command {
        name: "add-dir",
        aliases: &[],
        description: "Track an additional directory alongside the cwd (or list/remove)",
        hidden: false,
    },
    Command {
        name: "rename",
        aliases: &[],
        description: "Set a human-readable label on the current session (or clear it)",
        hidden: false,
    },
    Command {
        name: "usage",
        aliases: &[],
        description: "Per-turn token usage timeline (input / output / cache)",
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
                    ProviderKind::Cohere => vec![
                        ("command-r-plus", "Command R+ · Most capable"),
                        ("command-r", "Command R · Balanced"),
                        ("command-light", "Command Light · Fast"),
                    ],
                    ProviderKind::Perplexity => vec![
                        ("sonar-pro", "Sonar Pro · Most capable, web search"),
                        ("sonar", "Sonar · Balanced, web search"),
                        ("sonar-deep-research", "Sonar Deep Research · In-depth"),
                    ],
                    ProviderKind::OpenRouter => vec![
                        ("anthropic/claude-sonnet-4", "Claude Sonnet 4 · Balanced"),
                        ("anthropic/claude-opus-4", "Claude Opus 4 · Most capable"),
                        ("openai/gpt-4.1", "GPT-4.1 · Balanced"),
                        ("openai/gpt-4.1-mini", "GPT-4.1 Mini · Fast"),
                        ("google/gemini-2.5-flash", "Gemini 2.5 Flash · Fast"),
                        ("meta-llama/llama-3.3-70b", "Llama 3.3 70B · Open"),
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
                    let label = s
                        .label
                        .as_deref()
                        .map(|l| format!(" [{l}]"))
                        .unwrap_or_default();
                    println!(
                        "  {}{label} — {} ({} turns, {} msgs, {})",
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
        Some("skill") => {
            let sub = args.unwrap_or("").trim().to_string();
            let (subcmd, subarg) = sub
                .split_once(char::is_whitespace)
                .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                .unwrap_or((sub.clone(), String::new()));

            match subcmd.as_str() {
                "install" | "add" if !subarg.is_empty() => {
                    println!("Installing skill '{subarg}'...");
                    let rt = tokio::runtime::Handle::current();
                    let name = subarg.clone();
                    match std::thread::spawn(move || {
                        rt.block_on(agent_code_lib::skills::remote::install_skill(&name, None))
                    })
                    .join()
                    .unwrap_or_else(|_| Err("Thread panicked".to_string()))
                    {
                        Ok(path) => println!("Installed to {}", path.display()),
                        Err(e) => println!("Failed: {e}"),
                    }
                }
                "remove" | "uninstall" if !subarg.is_empty() => {
                    match agent_code_lib::skills::remote::uninstall_skill(&subarg) {
                        Ok(()) => println!("Removed skill '{subarg}'."),
                        Err(e) => println!("Failed: {e}"),
                    }
                }
                "search" | "list-remote" => {
                    println!("Fetching skill index...");
                    let rt = tokio::runtime::Handle::current();
                    let query = subarg.to_lowercase();
                    match std::thread::spawn(move || {
                        rt.block_on(agent_code_lib::skills::remote::fetch_index(None))
                    })
                    .join()
                    .unwrap_or_else(|_| Err("Thread panicked".to_string()))
                    {
                        Ok(skills) => {
                            let filtered: Vec<_> = if query.is_empty() {
                                skills.iter().collect()
                            } else {
                                skills
                                    .iter()
                                    .filter(|s| {
                                        s.name.to_lowercase().contains(&query)
                                            || s.description.to_lowercase().contains(&query)
                                    })
                                    .collect()
                            };
                            let installed = agent_code_lib::skills::remote::list_installed();
                            if filtered.is_empty() {
                                println!("No skills found.");
                            } else {
                                println!("{} skill(s) available:\n", filtered.len());
                                for skill in &filtered {
                                    let tag = if installed.contains(&skill.name) {
                                        " [installed]"
                                    } else {
                                        ""
                                    };
                                    let ver = if skill.version.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" v{}", skill.version)
                                    };
                                    println!(
                                        "  {}{} — {}{}",
                                        skill.name, ver, skill.description, tag
                                    );
                                }
                                println!("\nInstall with: /skill install <name>");
                            }
                        }
                        Err(e) => println!("Failed to fetch index: {e}"),
                    }
                }
                "installed" => {
                    let installed = agent_code_lib::skills::remote::list_installed();
                    if installed.is_empty() {
                        println!("No user-installed skills. Install with: /skill install <name>");
                    } else {
                        println!("{} installed skill(s):\n", installed.len());
                        for name in &installed {
                            println!("  {name}");
                        }
                    }
                }
                "help" | "" => {
                    println!("Skill management commands:\n");
                    println!("  /skill search [query]    Search the remote skill index");
                    println!("  /skill install <name>    Install a skill from the index");
                    println!("  /skill remove <name>     Remove an installed skill");
                    println!("  /skill installed         List user-installed skills");
                    println!("  /skill help              Show this help");
                }
                _ => {
                    println!("Unknown subcommand: {subcmd}. Try /skill help");
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
        Some("sandbox") => {
            let cwd = std::path::PathBuf::from(&engine.state().cwd);
            let cfg = &engine.state().config.sandbox;
            let exec = agent_code_lib::sandbox::SandboxExecutor::from_session_config(
                &engine.state().config,
                &cwd,
            );
            let policy = exec.policy();

            println!("Process-level sandbox:");
            println!(
                "  status      : {}",
                if exec.is_active() {
                    "active"
                } else if cfg.enabled {
                    "requested (no working strategy — running unsandboxed)"
                } else {
                    "disabled"
                }
            );
            println!("  strategy    : {}", exec.strategy_name());
            println!("  project_dir : {}", policy.project_dir.display());
            if !policy.allowed_write_paths.is_empty() {
                println!("  allowed writes:");
                for p in &policy.allowed_write_paths {
                    println!("    - {}", p.display());
                }
            }
            if !policy.forbidden_paths.is_empty() {
                println!("  forbidden reads:");
                for p in &policy.forbidden_paths {
                    println!("    - {}", p.display());
                }
            }
            println!(
                "  network     : {}",
                if policy.allow_network {
                    "allowed"
                } else {
                    "denied"
                }
            );
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
                 Messages: {}\n\
                 Working directory: {}",
                engine.state().messages.len(),
                engine.state().cwd,
            );
            let extra = &engine.state().additional_dirs;
            if !extra.is_empty() {
                println!("Additional dirs:");
                for d in extra {
                    println!("  {d}");
                }
            }
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
        Some("update") => {
            println!("Checking for updates...");
            let rt = tokio::runtime::Handle::current();
            let check = std::thread::spawn(move || rt.block_on(crate::update::check_for_update()))
                .join()
                .ok()
                .flatten();
            match check {
                Some(c) if c.is_newer => {
                    println!("Update available: v{} → v{}", c.current, c.latest);
                    println!("{}", c.release_url);
                    println!("\nTo update:");
                    println!("  cargo install agent-code");
                    println!("  # or download from the release page above");
                }
                Some(c) => {
                    println!("You're on the latest version (v{}).", c.current);
                }
                None => {
                    println!("Could not check for updates. Try again later.");
                }
            }
            CommandResult::Handled
        }
        Some("uninstall") => {
            uninstall::run(args);
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
        Some("powerup") => execute_powerup(args),
        Some("usage") => {
            execute_usage(engine);
            CommandResult::Handled
        }
        Some("effort") => {
            let task = args.unwrap_or("").trim();
            let prompt = if task.is_empty() {
                "Rate the effort required to complete the task we are discussing \
                 in this conversation. Pick one: XS (< 15 min, single file, \
                 trivial), S (< 1 hr, 1-3 files, straightforward), M (half day, \
                 multiple files or a new module, some design), L (1-2 days, \
                 cross-cutting or new subsystem, real design work), XL (3+ days \
                 or architectural, multiple teams/subsystems, real risk). \
                 Respond with: the rating, a one-line justification, and the \
                 top 2 risks or unknowns. Be blunt. Do not hedge."
                    .to_string()
            } else {
                format!(
                    "Rate the effort required for the following task. Pick one: \
                     XS (< 15 min, single file, trivial), S (< 1 hr, 1-3 files, \
                     straightforward), M (half day, multiple files or a new \
                     module, some design), L (1-2 days, cross-cutting or new \
                     subsystem, real design work), XL (3+ days or architectural, \
                     multiple teams/subsystems, real risk). Respond with: the \
                     rating, a one-line justification, and the top 2 risks or \
                     unknowns. Be blunt. Do not hedge.\n\n\
                     Task: {task}"
                )
            };
            CommandResult::Prompt(prompt)
        }
        Some("btw") => {
            execute_btw(args);
            CommandResult::Handled
        }
        Some("break-cache") => {
            if engine.state().break_cache_next {
                println!("Cache bust already armed for the next request.");
            } else {
                engine.state_mut().break_cache_next = true;
                println!(
                    "Next request will skip the prompt cache. \
                     Subsequent requests will cache normally."
                );
            }
            CommandResult::Handled
        }
        Some("heapdump") => {
            heapdump::run();
            CommandResult::Handled
        }
        Some("add-dir") => {
            execute_add_dir(args, engine);
            CommandResult::Handled
        }
        Some("rename") => {
            let session_id = engine.state().session_id.clone();
            let label = args.map(|s| s.trim()).filter(|s| !s.is_empty());
            match agent_code_lib::services::session::set_session_label(
                &session_id,
                label.map(|s| s.to_string()),
            ) {
                Ok(_) => match label {
                    Some(name) => println!("Session labelled: {name}"),
                    None => println!("Session label cleared."),
                },
                Err(e) => {
                    // If the session hasn't been saved yet (first turn not
                    // taken), there's no file to label.
                    eprintln!("Failed to rename session: {e}");
                    eprintln!(
                        "Note: the session file is created after the first turn; \
                         try /rename again once you've sent a message."
                    );
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

// ---------------------------------------------------------------------------
// /powerup — interactive tutorial system
// ---------------------------------------------------------------------------

/// Lesson definition for the /powerup tutorial system.
struct Lesson {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    prompt: &'static str,
}

/// The five built-in lessons that ship with agent-code.
const LESSONS: &[Lesson] = &[
    Lesson {
        id: "01-first-conversation",
        title: "Your First Conversation",
        description: "Talk to the agent, ask questions, get answers",
        prompt: r#"You are running an interactive tutorial lesson for the user.

**Lesson 1: Your First Conversation**

Walk the user through their first interaction with agent-code. Follow these steps:

1. **Explain** (briefly): The agent reads your codebase and can answer questions about it. You just type naturally — no special syntax needed.

2. **Try it**: Ask the user to type a question about their current project, like "what does this project do?" or "what language is this written in?" Then answer that question using the codebase tools (read files, search, etc.).

3. **Verify**: After answering, confirm the user saw how the agent read files and provided context-aware answers. Explain that the agent always grounds its answers in the actual code.

4. **Bonus tips**:
   - Use `@filename` to attach a specific file to your prompt
   - Use `\` + Enter for multi-line input
   - Press `?` to see all keyboard shortcuts

End with: "Lesson complete! Try `/powerup` to pick your next lesson."

Keep your teaching style concise and practical — show, don't lecture."#,
    },
    Lesson {
        id: "02-editing-files",
        title: "Editing Files",
        description: "Let the agent read, edit, and create files for you",
        prompt: r#"You are running an interactive tutorial lesson for the user.

**Lesson 2: Editing Files**

Teach the user how the agent modifies code. Follow these steps:

1. **Explain** (briefly): The agent can read, edit, and create files. It uses dedicated tools (Read, Edit, Write) rather than shell commands, so changes are precise and reviewable.

2. **Try it**: Ask the user to request a small, safe change in their project. Good examples:
   - "Add a comment at the top of [file] explaining what it does"
   - "Rename variable X to something more descriptive in [file]"
   If the user isn't sure, pick a file and suggest adding a doc comment. Make the edit.

3. **Verify**: After making the edit, show the user the change with `git diff`. Explain that every edit is reviewable and reversible with `/rewind` or `git checkout`.

4. **Bonus tips**:
   - The agent reads files before editing to avoid blind changes
   - Use `/diff` to see all pending changes
   - Use `/commit` to commit when you're happy with the changes
   - Permission mode controls whether edits need approval

End with: "Lesson complete! Try `/powerup` to pick your next lesson."

Keep it hands-on. Make a real edit in the user's project."#,
    },
    Lesson {
        id: "03-shell-and-tools",
        title: "Shell Commands & Tools",
        description: "Run commands, search code, and use the 32 built-in tools",
        prompt: r#"You are running an interactive tutorial lesson for the user.

**Lesson 3: Shell Commands & Tools**

Teach the user about the agent's tool system. Follow these steps:

1. **Explain** (briefly): The agent has 32 built-in tools: file ops, code search (grep/glob), shell execution, git, web search, and more. It picks the right tool automatically based on your request. You can also run shell commands directly with the `!` prefix.

2. **Try it**: Walk through three examples:
   a) Ask the user to try `!git status` (direct shell — output lands in the conversation)
   b) Have the user ask "find all TODO comments in this project" (agent uses Grep tool)
   c) Have the user ask "what tests exist in this project?" (agent uses Glob + Read)

3. **Verify**: Point out how the agent chose different tools for each task. Explain the tool output is visible in the conversation.

4. **Bonus tips**:
   - `!command` runs a shell command directly in your terminal
   - `&prompt` runs a prompt in the background
   - The agent parallelizes independent tool calls for speed
   - Use `/permissions` to see which tools need approval

End with: "Lesson complete! Try `/powerup` to pick your next lesson."

Be practical — use the user's actual project for demonstrations."#,
    },
    Lesson {
        id: "04-skills-and-workflows",
        title: "Skills & Workflows",
        description: "Use /commit, /review, /test, and create custom skills",
        prompt: r#"You are running an interactive tutorial lesson for the user.

**Lesson 4: Skills & Workflows**

Teach the user about the skill system. Follow these steps:

1. **Explain** (briefly): Skills are reusable workflows invoked with `/name`. There are 12 bundled skills for common tasks: `/commit`, `/review`, `/test`, `/debug`, `/explain`, `/pr`, `/refactor`, `/init`, `/security-review`, `/advisor`, `/bughunter`, `/plan`. You can also create custom skills.

2. **Try it**: Walk through two examples:
   a) Run `/explain` on a file in the user's project — show how it provides a structured explanation
   b) Show the user how to see all available skills with `/skills`

3. **Explain custom skills**: Tell the user they can create their own:
   - Create a `.agent/skills/` directory in their project
   - Add a markdown file with YAML frontmatter and a prompt template
   - Example: a `deploy-check.md` skill that verifies pre-deploy conditions
   - Use `/skill search` to find community skills, `/skill install <name>` to install them

4. **Bonus tips**:
   - Skills are just prompt templates with `{{arg}}` substitution
   - Project skills override bundled skills with the same name
   - `/skill help` shows all skill management commands

End with: "Lesson complete! Try `/powerup` to pick your next lesson."

Focus on the practical value of each skill."#,
    },
    Lesson {
        id: "05-multi-provider",
        title: "Models & Providers",
        description: "Switch models, compare providers, manage costs",
        prompt: r#"You are running an interactive tutorial lesson for the user.

**Lesson 5: Models & Providers**

Teach the user about model and provider management. Follow these steps:

1. **Explain** (briefly): agent-code works with 15+ LLM providers. You can switch models mid-session, compare outputs, and control costs. The agent normalizes different API formats so all tools work with any provider.

2. **Try it**: Walk through two examples:
   a) Show the current model with `/model` — let the user see the interactive selector
   b) Show session cost so far with `/cost` — explain token breakdown and cache hits

3. **Explain configuration**: Tell the user about:
   - `~/.config/agent-code/config.toml` for default model and provider
   - `--model <name>` flag to start with a specific model
   - `--api-base-url` for any OpenAI-compatible endpoint (local models, proxies)
   - Setting max cost with `max_cost_usd` in config to prevent runaway spending

4. **Bonus tips**:
   - Use `/context` to see how much of the context window is used
   - `/compact` frees context space when running long sessions
   - `/doctor` checks your provider connection health
   - Smaller models (mini/nano) are great for simple tasks and cost less

End with: "All 5 lessons complete! You're ready to use agent-code like a pro. Run `/powerup` anytime to revisit a lesson."

Celebrate the user finishing all lessons."#,
    },
];

/// Load completed lesson IDs from the progress file.
fn load_progress() -> Vec<String> {
    let path = match dirs::data_local_dir() {
        Some(d) => d.join("agent-code/powerup-progress.json"),
        None => return Vec::new(),
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// Save a lesson ID to the progress file.
fn save_progress(lesson_id: &str) {
    let dir = match dirs::data_local_dir() {
        Some(d) => d.join("agent-code"),
        None => return,
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("powerup-progress.json");
    let mut completed = load_progress();
    if !completed.contains(&lesson_id.to_string()) {
        completed.push(lesson_id.to_string());
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&completed).unwrap_or_default(),
    );
}

/// Execute the /powerup command.
fn execute_powerup(args: Option<&str>) -> CommandResult {
    let completed = load_progress();

    // Direct lesson selection: /powerup 1, /powerup 3, etc.
    if let Some(arg) = args {
        let arg = arg.trim();
        // Accept "1"-"5" or "reset".
        if arg == "reset" {
            if let Some(d) = dirs::data_local_dir() {
                let _ = std::fs::remove_file(d.join("agent-code/powerup-progress.json"));
            }
            println!("Tutorial progress reset.");
            return CommandResult::Handled;
        }
        if let Ok(num) = arg.parse::<usize>()
            && num >= 1
            && num <= LESSONS.len()
        {
            let lesson = &LESSONS[num - 1];
            save_progress(lesson.id);
            return CommandResult::Prompt(lesson.prompt.to_string());
        }
        println!("Usage: /powerup [1-5 | reset]");
        return CommandResult::Handled;
    }

    // Show interactive lesson picker.
    let total = LESSONS.len();
    let done = completed.len().min(total);

    println!();
    println!("  ⚡ Interactive Tutorials ({done}/{total} completed)");
    println!();

    let options: Vec<crate::ui::selector::SelectOption> = LESSONS
        .iter()
        .enumerate()
        .map(|(i, lesson)| {
            let check = if completed.contains(&lesson.id.to_string()) {
                " ✔"
            } else {
                ""
            };
            crate::ui::selector::SelectOption {
                label: format!("{}. {}{check}", i + 1, lesson.title),
                description: lesson.description.to_string(),
                value: lesson.id.to_string(),
                preview: None,
            }
        })
        .collect();

    let chosen = crate::ui::selector::select(&options);

    if chosen.is_empty() {
        return CommandResult::Handled;
    }

    // Find the chosen lesson and run it.
    if let Some(lesson) = LESSONS.iter().find(|l| l.id == chosen) {
        save_progress(lesson.id);
        CommandResult::Prompt(lesson.prompt.to_string())
    } else {
        CommandResult::Handled
    }
}

/// Execute the /add-dir command.
///
/// Forms:
///   /add-dir                — list currently tracked extra dirs
///   /add-dir <path>         — add a directory (must exist)
///   /add-dir --remove <p>   — remove a directory
///   /add-dir --clear        — remove all
fn execute_add_dir(args: Option<&str>, engine: &mut QueryEngine) {
    let raw = args.map(|s| s.trim()).unwrap_or("");

    if raw.is_empty() {
        let extras = &engine.state().additional_dirs;
        if extras.is_empty() {
            println!("No additional directories tracked.");
            println!("Usage: /add-dir <path>");
        } else {
            println!("Additional tracked directories:");
            for d in extras {
                println!("  {d}");
            }
        }
        return;
    }

    if raw == "--clear" {
        let n = engine.state().additional_dirs.len();
        engine.state_mut().additional_dirs.clear();
        println!(
            "Cleared {n} tracked director{}.",
            if n == 1 { "y" } else { "ies" }
        );
        return;
    }

    if let Some(rest) = raw.strip_prefix("--remove ") {
        let target = rest.trim();
        let existed = engine.state().additional_dirs.iter().any(|d| d == target);
        engine.state_mut().additional_dirs.retain(|d| d != target);
        if existed {
            println!("Removed: {target}");
        } else {
            println!("Not tracked: {target}");
        }
        return;
    }

    // Add a directory. Accept both absolute and relative paths; store
    // canonical form so the agent sees an unambiguous path.
    let path = std::path::PathBuf::from(raw);
    let canonical = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Cannot add {raw}: {e}");
            return;
        }
    };
    if !canonical.is_dir() {
        eprintln!("Not a directory: {}", canonical.display());
        return;
    }
    let s = canonical.display().to_string();
    if engine.state().additional_dirs.iter().any(|d| d == &s) {
        println!("Already tracked: {s}");
        return;
    }
    engine.state_mut().additional_dirs.push(s.clone());
    println!("Tracking: {s}");
}

/// Execute the /btw command: save a free-form note to user memory.
fn execute_btw(args: Option<&str>) {
    let text = args.map(|s| s.trim()).unwrap_or("");
    if text.is_empty() {
        println!("Usage: /btw <note>");
        println!("Example: /btw prefers short, direct commit messages");
        return;
    }

    let dir = match agent_code_lib::memory::ensure_memory_dir() {
        Some(d) => d,
        None => {
            eprintln!("Could not resolve user memory directory.");
            return;
        }
    };

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let slug = slugify_note(text);
    let filename = if slug.is_empty() {
        format!("btw-{stamp}.md")
    } else {
        format!("btw-{stamp}-{slug}.md")
    };

    // Short description for the index line.
    let description = truncate_to_words(text, 120);
    let name = format!("Note ({stamp})");

    let meta = agent_code_lib::memory::types::MemoryMeta {
        name: name.clone(),
        description,
        memory_type: Some(agent_code_lib::memory::types::MemoryType::User),
    };

    match agent_code_lib::memory::writer::write_memory(&dir, &filename, &meta, text) {
        Ok(path) => println!("Noted. Saved to {}", path.display()),
        Err(e) => eprintln!("Failed to save note: {e}"),
    }
}

/// Slugify a note for use in a filename (ASCII lowercase, hyphen-separated,
/// max 40 chars). Returns an empty string if nothing slugifiable remains.
fn slugify_note(text: &str) -> String {
    let mut out = String::with_capacity(40);
    let mut prev_dash = true;
    for ch in text.chars() {
        if out.len() >= 40 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Truncate a free-form note to approximately `max_chars` characters,
/// ending at a word boundary when possible.
fn truncate_to_words(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let cutoff = text[..max_chars].rfind(' ').unwrap_or(max_chars);
    format!("{}…", text[..cutoff].trim_end())
}

/// Row of per-turn token usage for display.
struct UsageRow {
    turn: usize,
    model: String,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

/// Pull per-turn usage from assistant messages that carry a Usage payload.
/// Messages without `usage` (older history, streaming failures) are skipped.
fn collect_usage_rows(
    messages: &[agent_code_lib::llm::message::Message],
    default_model: &str,
) -> Vec<UsageRow> {
    use agent_code_lib::llm::message::Message;
    let mut rows = Vec::new();
    let mut turn = 0usize;
    for msg in messages {
        if let Message::Assistant(a) = msg
            && let Some(u) = &a.usage
        {
            turn += 1;
            rows.push(UsageRow {
                turn,
                model: a.model.clone().unwrap_or_else(|| default_model.to_string()),
                input: u.input_tokens,
                output: u.output_tokens,
                cache_read: u.cache_read_input_tokens,
                cache_write: u.cache_creation_input_tokens,
            });
        }
    }
    rows
}

/// Execute /usage — print a per-turn token timeline.
fn execute_usage(engine: &QueryEngine) {
    let rows = collect_usage_rows(&engine.state().messages, &engine.state().config.api.model);

    if rows.is_empty() {
        println!("No completed turns with usage data yet.");
        return;
    }

    // Model column width: longest model name, capped at 24.
    let model_w = rows
        .iter()
        .map(|r| r.model.len())
        .max()
        .unwrap_or(5)
        .min(24);

    println!();
    println!(
        "  {:>3}  {:<width$}  {:>8}  {:>8}  {:>10}  {:>10}",
        "#",
        "model",
        "input",
        "output",
        "cache read",
        "cache write",
        width = model_w,
    );
    println!(
        "  {}  {}  {}  {}  {}  {}",
        "-".repeat(3),
        "-".repeat(model_w),
        "-".repeat(8),
        "-".repeat(8),
        "-".repeat(10),
        "-".repeat(10),
    );

    let mut tot_in = 0u64;
    let mut tot_out = 0u64;
    let mut tot_cr = 0u64;
    let mut tot_cw = 0u64;
    for r in &rows {
        let model_display = if r.model.len() > model_w {
            // Keep the tail — model family tokens live at the end.
            let start = r.model.len() - model_w;
            &r.model[start..]
        } else {
            r.model.as_str()
        };
        println!(
            "  {:>3}  {:<width$}  {:>8}  {:>8}  {:>10}  {:>10}",
            r.turn,
            model_display,
            r.input,
            r.output,
            r.cache_read,
            r.cache_write,
            width = model_w,
        );
        tot_in += r.input;
        tot_out += r.output;
        tot_cr += r.cache_read;
        tot_cw += r.cache_write;
    }
    println!(
        "  {}  {}  {}  {}  {}  {}",
        "-".repeat(3),
        "-".repeat(model_w),
        "-".repeat(8),
        "-".repeat(8),
        "-".repeat(10),
        "-".repeat(10),
    );
    println!(
        "  {:>3}  {:<width$}  {:>8}  {:>8}  {:>10}  {:>10}",
        "∑",
        "",
        tot_in,
        tot_out,
        tot_cr,
        tot_cw,
        width = model_w,
    );

    // Cache hit rate hint — a quick read on whether caching is effective.
    let cached_input = tot_cr + tot_cw;
    let total_input = tot_in + cached_input;
    if total_input > 0 {
        let hit_pct = (tot_cr as f64 / total_input as f64 * 100.0).round() as u64;
        println!("\n  Cache hit rate: {hit_pct}%  (use /cost for cost summary)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify_note("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_punctuation_collapses() {
        assert_eq!(slugify_note("foo---bar!!!baz"), "foo-bar-baz");
    }

    #[test]
    fn slugify_trims_leading_trailing_dashes() {
        assert_eq!(slugify_note("!!!hello!!!"), "hello");
    }

    #[test]
    fn slugify_truncates_to_40_chars() {
        let long = "a".repeat(100);
        let slug = slugify_note(&long);
        assert!(slug.len() <= 40);
    }

    #[test]
    fn slugify_empty_for_no_alnum() {
        assert_eq!(slugify_note("---!!!"), "");
    }

    #[test]
    fn truncate_short_passthrough() {
        assert_eq!(truncate_to_words("hello", 100), "hello");
    }

    #[test]
    fn truncate_at_word_boundary() {
        let text = "the quick brown fox jumps over the lazy dog";
        let out = truncate_to_words(text, 20);
        assert!(out.ends_with('…'));
        assert!(!out.contains("quickb")); // Did not split mid-word.
    }

    #[test]
    fn usage_rows_skip_messages_without_usage() {
        use agent_code_lib::llm::message::{
            AssistantMessage, ContentBlock, Message, Usage, user_message,
        };
        use uuid::Uuid;

        let mk_assistant = |usage: Option<Usage>, model: Option<&str>| {
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: "0".to_string(),
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
                model: model.map(String::from),
                usage,
                stop_reason: None,
                request_id: None,
            })
        };

        let messages = vec![
            user_message("first"),
            mk_assistant(
                Some(Usage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
                Some("model-a"),
            ),
            user_message("second"),
            // No usage — should be skipped (counted as 0 assistant turns for
            // the table; streaming failures, etc.).
            mk_assistant(None, Some("model-b")),
            user_message("third"),
            mk_assistant(
                Some(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_input_tokens: 80,
                    cache_read_input_tokens: 20,
                }),
                None, // falls back to default model
            ),
        ];

        let rows = collect_usage_rows(&messages, "default-model");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].turn, 1);
        assert_eq!(rows[0].model, "model-a");
        assert_eq!(rows[0].input, 100);
        assert_eq!(rows[1].turn, 2);
        assert_eq!(rows[1].model, "default-model");
        assert_eq!(rows[1].cache_read, 20);
        assert_eq!(rows[1].cache_write, 80);
    }
}
