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

use crate::query::QueryEngine;

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
        description: "Initialize project config (.rc/settings.toml)",
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
        name: "version",
        aliases: &[],
        description: "Show version information",
        hidden: true,
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
            let skills = crate::skills::SkillRegistry::load_all(Some(std::path::Path::new(
                &engine.state().cwd,
            )));
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
            let freed = crate::services::compact::microcompact(&mut engine.state_mut().messages, 2);
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
            CommandResult::Handled
        }
        Some("model") => {
            if let Some(new_model) = args {
                engine.state_mut().config.api.model = new_model.to_string();
                println!("Model changed to: {new_model}");
            } else {
                println!("Model: {}", engine.state().config.api.model);
            }
            CommandResult::Handled
        }
        Some("resume") => {
            if let Some(id) = args {
                match crate::services::session::load_session(id) {
                    Ok(data) => {
                        engine.state_mut().messages = data.messages;
                        engine.state_mut().turn_count = data.turn_count;
                        println!(
                            "Resumed session {} ({} messages, {} turns)",
                            id,
                            engine.state().messages.len(),
                            data.turn_count,
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
            let sessions = crate::services::session::list_sessions(10);
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
            let memory =
                crate::memory::MemoryContext::load(Some(std::path::Path::new(&engine.state().cwd)));
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
            let skills = crate::skills::SkillRegistry::load_all(Some(std::path::Path::new(
                &engine.state().cwd,
            )));
            if skills.all().is_empty() {
                println!(
                    "No skills loaded. Add .md files to .rc/skills/ or ~/.config/agent-code/skills/"
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
            // Quick environment check.
            let mut checks = Vec::new();

            // Check for required tools.
            for tool in &["git", "rg", "bash"] {
                let available = std::process::Command::new("which")
                    .arg(tool)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                checks.push(format!("  {} {}", if available { "✓" } else { "✗" }, tool,));
            }

            // Config status.
            let config = &engine.state().config;
            checks.push(format!("  Model: {}", config.api.model));
            checks.push(format!(
                "  API key: {}",
                if config.api.api_key.is_some() {
                    "set"
                } else {
                    "missing"
                }
            ));

            println!("Environment check:\n{}", checks.join("\n"));
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
            let config_dir = std::path::Path::new(&engine.state().cwd).join(".rc");
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
                        crate::llm::message::Message::User(u) => {
                            md.push_str("## User\n\n");
                            for block in &u.content {
                                if let crate::llm::message::ContentBlock::Text { text } = block {
                                    md.push_str(text);
                                    md.push_str("\n\n");
                                }
                            }
                        }
                        crate::llm::message::Message::Assistant(a) => {
                            md.push_str("## Assistant\n\n");
                            for block in &a.content {
                                if let crate::llm::message::ContentBlock::Text { text } = block {
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
            let tokens = crate::services::tokens::estimate_context_tokens(&engine.state().messages);
            let model = &engine.state().config.api.model;
            let window = crate::services::tokens::context_window_for_model(model);
            let threshold = crate::services::compact::auto_compact_threshold(model);
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
            let registry = crate::services::coordinator::AgentRegistry::with_defaults();
            println!("Available agent types:\n");
            for agent in registry.list() {
                let ro = if agent.read_only { " (read-only)" } else { "" };
                println!("  {}{ro} — {}", agent.name, agent.description);
            }
            CommandResult::Handled
        }
        Some("hooks") => {
            println!("Hook system active. Configure hooks in .rc/settings.toml:");
            println!("  [[hooks]]");
            println!("  event = \"pre_tool_use\"");
            println!("  action = {{ type = \"shell\", command = \"./check.sh\" }}");
            CommandResult::Handled
        }
        Some("plugins") => {
            let plugins = crate::services::plugins::PluginRegistry::load_all(Some(
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
            let tool_count = crate::services::history::tool_use_count(&state.messages);
            let tools_used = crate::services::history::tools_used(&state.messages);
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
        Some("version") => {
            println!("rc {}", env!("CARGO_PKG_VERSION"));
            CommandResult::Handled
        }
        _ => {
            // Check if it's a skill invocation.
            let skills = crate::skills::SkillRegistry::load_all(Some(std::path::Path::new(
                &engine.state().cwd,
            )));
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
