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
        name: "brief",
        aliases: &[],
        description: "Toggle brief mode (terse responses, ≤3 sentences)",
        hidden: false,
    },
    Command {
        name: "ctxviz",
        aliases: &["context-viz"],
        description: "Per-category token breakdown of the current context",
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
        name: "copy",
        aliases: &[],
        description: "Copy the last assistant message to the system clipboard",
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
    Command {
        name: "thinkback",
        aliases: &[],
        description: "Show the model's thinking blocks from a recent turn",
        hidden: false,
    },
    Command {
        name: "pr-comments",
        aliases: &[],
        description: "Fetch and review open review comments on the current or specified PR",
        hidden: false,
    },
    Command {
        name: "perf-issue",
        aliases: &[],
        description: "Audit recent changes for performance regressions",
        hidden: false,
    },
    Command {
        name: "autofix-pr",
        aliases: &[],
        description: "Check out a PR, run lint + tests, fix failures, push back",
        hidden: false,
    },
    Command {
        name: "env",
        aliases: &[],
        description: "Show agent-code-relevant environment variables (API keys masked)",
        hidden: false,
    },
    Command {
        name: "issue",
        aliases: &[],
        description: "Open a GitHub issue prefilled with session context (title optional)",
        hidden: false,
    },
    Command {
        name: "profile",
        aliases: &[],
        description: "Save/load/list/delete named config profiles (try /profile help)",
        hidden: false,
    },
    Command {
        name: "tokens",
        aliases: &[],
        description: "Estimate the token count of arbitrary text (e.g. /tokens hello world)",
        hidden: false,
    },
    Command {
        name: "thinkback-play",
        aliases: &[],
        description: "Replay every turn's thinking blocks in order with a short pause between",
        hidden: false,
    },
    Command {
        name: "keybindings",
        aliases: &["keys"],
        description: "List keyboard shortcuts and the override file path",
        hidden: false,
    },
    Command {
        name: "tag",
        aliases: &[],
        description: "Tag the current session for filtering (list / add <tag> / --remove <tag>)",
        hidden: false,
    },
    Command {
        name: "rules",
        aliases: &[],
        description: "List / enable / disable project rules (.agent/rules/*.md)",
        hidden: false,
    },
    Command {
        name: "install-github-app",
        aliases: &["gh-setup"],
        description: "Walk through `gh` CLI setup and verify scopes for PR commands",
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
            // Optional filter: /sessions --tag <tag>
            let filter_tag = args
                .and_then(|a| a.strip_prefix("--tag "))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty());

            let mut sessions = agent_code_lib::services::session::list_sessions(100);

            if let Some(tag) = filter_tag {
                let normalized = agent_code_lib::services::session::normalize_tag(tag);
                match normalized {
                    Ok(t) => sessions.retain(|s| s.tags.iter().any(|x| x == &t)),
                    Err(e) => {
                        eprintln!("Invalid tag: {e}");
                        return CommandResult::Handled;
                    }
                }
            }

            // Display cap after filtering.
            sessions.truncate(10);

            if sessions.is_empty() {
                if filter_tag.is_some() {
                    println!("No sessions match that tag.");
                } else {
                    println!("No saved sessions.");
                }
            } else {
                println!("Recent sessions:\n");
                for s in &sessions {
                    let label = s
                        .label
                        .as_deref()
                        .map(|l| format!(" [{l}]"))
                        .unwrap_or_default();
                    let tags = if s.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" #{}", s.tags.join(" #"))
                    };
                    println!(
                        "  {}{label}{tags} — {} ({} turns, {} msgs, {})",
                        s.id, s.cwd, s.turn_count, s.message_count, s.updated_at,
                    );
                }
                println!("\nUse /resume <id> to restore, or /sessions --tag <tag> to filter.");
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
                "validate" | "lint" if !subarg.is_empty() => {
                    execute_skill_validate(&subarg);
                }
                "validate" | "lint" => {
                    println!("Usage: /skill validate <path-or-name>");
                    println!(
                        "  Path can be a skill file (my-skill.md) or a directory \
                         (the tool walks .md files inside)."
                    );
                }
                "help" | "" => {
                    println!("Skill management commands:\n");
                    println!("  /skill search [query]    Search the remote skill index");
                    println!("  /skill install <name>    Install a skill from the index");
                    println!("  /skill remove <name>     Remove an installed skill");
                    println!("  /skill installed         List user-installed skills");
                    println!("  /skill validate <path>   Lint a skill file or directory");
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
        Some("brief") => {
            let brief = &mut engine.state_mut().brief_mode;
            *brief = !*brief;
            if *brief {
                println!("Brief mode enabled. Responses will be kept terse (≤3 sentences).");
            } else {
                println!("Brief mode disabled. Response style restored.");
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
        Some("copy") => {
            execute_copy(engine);
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
        Some("ctxviz") | Some("context-viz") => {
            execute_ctxviz(engine);
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
        Some("thinkback") => {
            execute_thinkback(args, engine);
            CommandResult::Handled
        }
        Some("thinkback-play") => {
            execute_thinkback_play(engine);
            CommandResult::Handled
        }
        Some("pr-comments") => {
            let target = args.map(|s| s.trim()).unwrap_or("");
            let selector = if target.is_empty() {
                "the current branch's PR".to_string()
            } else {
                format!("PR {target}")
            };
            let prompt = format!(
                "Fetch review comments on {selector} and help me triage them. Steps:\n\n\
                 1. Run `gh pr view` (or `gh pr view {target}` if a number was given) to \
                 confirm the PR exists and is open. Abort with a clear message if it isn't.\n\
                 2. Fetch review comments with `gh api repos/{{owner}}/{{repo}}/pulls/\
                 <pr>/comments --paginate` — these are inline/line-level comments.\n\
                 3. Fetch issue-style comments with `gh pr view <pr> --json comments`.\n\
                 4. Group the results: (a) unresolved inline threads, (b) action-requested \
                 items in issue comments (questions, change requests), (c) resolved threads \
                 (skip — don't re-open the discussion).\n\
                 5. For each unresolved item, print: file:line, author, short quote of the \
                 comment, and a one-line suggested response OR concrete code fix. Do not \
                 implement anything yet — just present the triage list.\n\
                 6. End with a numbered action list ordered by importance (blocking review \
                 first, then nits). Ask the user which items to address.\n\n\
                 Never respond to reviewers without the user's go-ahead. Never mark threads \
                 resolved — that's the reviewer's call."
            );
            CommandResult::Prompt(prompt)
        }
        Some("perf-issue") => {
            let scope = args.map(|s| s.trim()).filter(|s| !s.is_empty());
            let target = match scope {
                Some(s) => format!("the following target: {s}"),
                None => "the current git diff".to_string(),
            };
            let prompt = format!(
                "Audit {target} for performance regressions. Do NOT rewrite code — \
                 produce a report. For each finding, cite file:line, describe the hot \
                 path it affects, and propose the minimal fix.\n\n\
                 Look specifically for:\n\
                 - N+1 queries: loops that issue a query per iteration (SQL, HTTP, RPC) \
                 where a batch/IN clause / prefetch would collapse the fan-out\n\
                 - Missing DB indexes: new WHERE / ORDER BY / JOIN columns without an \
                 index; full-table scans on growing tables\n\
                 - Synchronous I/O on hot paths: blocking reads/writes inside request \
                 handlers, tight loops, or render paths\n\
                 - Allocation hotspots: per-iteration allocations that could be pooled, \
                 cloned buffers that could be borrowed, repeated string concatenation in \
                 loops (Vec<u8>/String builder instead)\n\
                 - Quadratic algorithms hidden behind nested iteration over user data; \
                 .contains() inside .iter() on large inputs\n\
                 - Cache invalidation bugs: writes that don't bust caches, reads that \
                 hit stale entries\n\
                 - Unbounded growth: unbounded channels, Vec pushes without a ceiling, \
                 in-memory state that grows per request\n\
                 - Synchronous operations in async contexts: `std::thread::sleep` in \
                 async fn, blocking file I/O instead of `tokio::fs`, CPU-bound work \
                 that should be `spawn_blocking`\n\n\
                 Format the report as: severity (critical / high / medium / low), \
                 file:line, one-sentence impact, proposed fix. Sort critical first. \
                 If the diff is clean, say so plainly — do not invent findings to \
                 justify the run."
            );
            CommandResult::Prompt(prompt)
        }
        Some("autofix-pr") => {
            let target = args.map(|s| s.trim()).unwrap_or("");
            let selector = if target.is_empty() {
                "the current branch's PR".to_string()
            } else {
                format!("PR {target}")
            };
            let prompt = format!(
                "Autofix {selector}. Work inside a git worktree so the current working \
                 tree stays clean. Steps, in order — do NOT skip the verification steps:\n\n\
                 1. Confirm the PR exists and is open: `gh pr view {target}` (or for \
                 the current branch). Abort with a clear message if merged or closed.\n\
                 2. Create an isolated worktree via the worktree tool, checkout the PR's \
                 head branch in it. Run all further commands from that worktree.\n\
                 3. Detect the project toolchain from manifest files (Cargo.toml, \
                 package.json, pyproject.toml, go.mod). Run the project's lint and test \
                 commands — check AGENTS.md / CONTRIBUTING.md for the canonical commands \
                 first; fall back to `cargo check && cargo clippy --all-targets -D warnings \
                 && cargo test` etc.\n\
                 4. Capture every failure. Classify: formatter/linter (safe to fix), \
                 unit-test failures (read source, root-cause, minimal fix), type errors \
                 (honor the types, don't cast to bypass).\n\
                 5. Apply minimal fixes for each failure. Run the gate again after EACH \
                 fix to confirm it's real — do not batch speculative edits.\n\
                 6. When all checks pass, commit with a conventional message describing \
                 what was fixed (e.g. \"fix(lint): satisfy clippy, address test flake\"). \
                 One commit per logical fix if they're orthogonal; one combined commit \
                 if they're the same class of fix.\n\
                 7. Push to the PR's head branch. Never force-push. Never skip hooks.\n\
                 8. Report back: commit SHAs pushed, what was fixed, anything left broken \
                 that needs the author's judgment (e.g. a failing test that asserts wrong \
                 behavior — flag, don't delete).\n\n\
                 Never touch workflow files (.github/workflows/**) as part of an autofix. \
                 Never modify tests to make them pass — fix the code they test, or flag \
                 that the test is wrong."
            );
            CommandResult::Prompt(prompt)
        }
        Some("env") => {
            execute_env();
            CommandResult::Handled
        }
        Some("issue") => {
            let title_hint = args.map(|s| s.trim()).filter(|s| !s.is_empty());
            let title_clause = match title_hint {
                Some(t) => format!(
                    "The user suggested a title: \"{t}\". Refine it to be \
                                    specific and action-oriented (under 70 chars)."
                ),
                None => "Derive the title yourself from the top user-reported symptom in \
                         this session. Keep it specific and action-oriented (under 70 \
                         chars)."
                    .to_string(),
            };
            let prompt = format!(
                "Open a GitHub issue with context from this session. Steps:\n\n\
                 1. Title: {title_clause}\n\
                 2. Body (markdown), with these sections:\n   \
                 **What happened** — one paragraph describing the symptom or ask, from \
                 the user's perspective. Do not dump transcript; summarize.\n   \
                 **Reproduction** — the minimal steps or command that triggers it. If \
                 it's environmental, note the OS / agent-code version / model.\n   \
                 **Expected vs actual** — one line each.\n   \
                 **Context** — anything load-bearing the agent discovered while \
                 investigating (relevant file:line references, error messages, commit \
                 SHAs). Use fenced code blocks for logs or stack traces.\n   \
                 **Environment** — agent-code version (from env!(\"CARGO_PKG_VERSION\") \
                 equivalent via /version), OS, model, relevant env vars (mask secrets).\n\
                 3. Show the draft to the user and wait for approval before opening.\n\
                 4. On approval, run `gh issue create --title <title> --body-file <file>` \
                 to open it in the current repository. Print the issue URL.\n\n\
                 Never include API keys, tokens, passwords, or session transcripts with \
                 personal data. If the session contains credentials, strip them before \
                 including any log excerpt."
            );
            CommandResult::Prompt(prompt)
        }
        Some("profile") => {
            execute_profile(args, engine);
            CommandResult::Handled
        }
        Some("keybindings") => {
            let registry = crate::ui::keybindings::KeybindingRegistry::load();
            let bindings = registry.all();
            if bindings.is_empty() {
                println!("No keybindings loaded.");
            } else {
                println!();
                println!("  Key             Action");
                println!("  ---             ------");
                for b in &bindings {
                    use crate::ui::keybindings::KeyAction;
                    let action = match &b.action {
                        KeyAction::Command { command } => format!("/{command}"),
                        KeyAction::Prompt { prompt } => {
                            let short = prompt.chars().take(40).collect::<String>();
                            let ellipsis = if prompt.chars().count() > 40 {
                                "…"
                            } else {
                                ""
                            };
                            format!("prompt: \"{short}{ellipsis}\"")
                        }
                        KeyAction::Toggle { setting } => format!("toggle: {setting}"),
                    };
                    let desc = b
                        .description
                        .as_deref()
                        .map(|d| format!(" — {d}"))
                        .unwrap_or_default();
                    println!("  {:<14}  {action}{desc}", b.key);
                }
                println!();
                if let Some(d) = dirs::config_dir() {
                    let path = d.join("agent-code").join("keybindings.json");
                    if path.exists() {
                        println!("  Overrides file: {}", path.display());
                    } else {
                        println!(
                            "  Add custom bindings at: {} (JSON, see docs/reference/cli-flags.mdx)",
                            path.display()
                        );
                    }
                }
            }
            CommandResult::Handled
        }
        Some("rules") => {
            execute_rules(args, engine);
            CommandResult::Handled
        }
        Some("tokens") => {
            let text = args.unwrap_or("").trim();
            if text.is_empty() {
                println!("Usage: /tokens <text>");
                println!("Example: /tokens the quick brown fox");
                return CommandResult::Handled;
            }
            let n = agent_code_lib::services::tokens::estimate_tokens(text);
            let bytes = text.len();
            let chars = text.chars().count();
            println!("  Tokens:     ~{n}");
            println!("  Characters: {chars}");
            println!("  Bytes:      {bytes}");
            if n > 0 {
                let ratio = chars as f64 / n as f64;
                println!("  Chars/token: {ratio:.2}");
            }
            CommandResult::Handled
        }
        Some("tag") => {
            execute_tag(args, engine);
            CommandResult::Handled
        }
        Some("install-github-app") => {
            let prompt = "Walk the user through setting up the `gh` CLI so the PR-related \
                 slash commands (/pr-comments, /autofix-pr, /issue) have what \
                 they need. Steps:\n\n\
                 1. Check whether `gh` is installed: run `gh --version`. If not, \
                 point to the install instructions for the user's OS and stop — \
                 don't try to install it silently.\n\
                 2. Check auth: run `gh auth status`. If not logged in, instruct \
                 the user to run `gh auth login` themselves (interactive login \
                 needs a TTY you don't own); list the scopes we need: `repo`, \
                 `read:org`, `workflow`.\n\
                 3. If logged in but missing scopes: instruct the user to run \
                 `gh auth refresh -s repo,workflow` and re-verify.\n\
                 4. Confirm the current directory has a GitHub remote: \
                 `gh repo view --json nameWithOwner`. If it doesn't (no remote, \
                 or not on GitHub), explain which PR commands still work (none) \
                 and which need the remote.\n\
                 5. Print a one-line summary: ready / needs install / needs \
                 login / needs scope refresh, plus the next action the user \
                 must take.\n\n\
                 Never store tokens in this process. Never exfiltrate the \
                 token — `gh auth token` output must stay in the user's \
                 terminal. If the user asks you to write their token to a \
                 file, refuse."
                .to_string();
            CommandResult::Prompt(prompt)
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

/// Walk the conversation and collect the thinking blocks attached to
/// each assistant message. Returned Vec is in chronological order, so
/// `last()` is the most recent turn's thinking.
fn collect_thinking_turns(messages: &[agent_code_lib::llm::message::Message]) -> Vec<Vec<String>> {
    use agent_code_lib::llm::message::{ContentBlock, Message};
    let mut turns: Vec<Vec<String>> = Vec::new();
    for msg in messages {
        if let Message::Assistant(a) = msg {
            let blocks: Vec<String> = a
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Thinking { thinking, .. } => Some(thinking.clone()),
                    _ => None,
                })
                .collect();
            if !blocks.is_empty() {
                turns.push(blocks);
            }
        }
    }
    turns
}

/// Execute `/thinkback [n]`. With no arg, shows the most recent turn's
/// thinking blocks. With `n`, shows the nth most recent (1 = latest).
/// Execute `/thinkback-play` — replay every turn's thinking blocks in
/// chronological order with a short pause between turns so the reader
/// can follow. Ctrl-C interrupts (the repl signal handler handles
/// that; this function just blocks in `thread::sleep`).
fn execute_thinkback_play(engine: &QueryEngine) {
    let turns = collect_thinking_turns(&engine.state().messages);
    if turns.is_empty() {
        println!("No thinking blocks in this session yet.");
        return;
    }

    println!();
    println!(
        "  Replaying {} turn(s) of thinking. Ctrl-C to stop.",
        turns.len()
    );
    println!();

    for (i, blocks) in turns.iter().enumerate() {
        let turn_num = i + 1;
        println!("  ─── turn {turn_num} / {} ───", turns.len());
        for (j, block) in blocks.iter().enumerate() {
            if blocks.len() > 1 {
                println!("  · block {} ·", j + 1);
            }
            println!("{block}");
            // Pause proportional to content length: ~15ms per char,
            // clamped to 0.3s–4s per block so tiny turns don't feel
            // instant and long turns don't wait forever.
            let chars = block.chars().count() as u64;
            let ms = (chars * 15).clamp(300, 4000);
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        println!();
    }
    println!("  Done.");
}

fn execute_thinkback(args: Option<&str>, engine: &QueryEngine) {
    let turns = collect_thinking_turns(&engine.state().messages);
    if turns.is_empty() {
        println!("No thinking blocks in this session yet.");
        return;
    }

    let n: usize = args
        .and_then(|s| s.trim().parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(1);

    if n > turns.len() {
        println!(
            "Only {} turn(s) with thinking blocks in this session; asked for #{n}.",
            turns.len()
        );
        return;
    }

    // Index from the end so 1 = latest.
    let idx = turns.len() - n;
    let blocks = &turns[idx];
    println!(
        "\nThinking blocks from turn {} of {} (most recent is #1):\n",
        n,
        turns.len()
    );
    for (i, block) in blocks.iter().enumerate() {
        if blocks.len() > 1 {
            println!("--- block {} ---", i + 1);
        }
        println!("{block}\n");
    }
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

/// Variables agent-code actually reads. Kept in a single const so it's
/// obvious which ones are wired up — stale entries are a lie to the user.
const ENV_VARS: &[&str] = &[
    // Config overrides (plaintext).
    "AGENT_CODE_API_BASE_URL",
    "AGENT_CODE_MODEL",
    "AGENT_CODE_CONFIG",
    // API keys (masked).
    "AGENT_CODE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "XAI_API_KEY",
    "GOOGLE_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "ZHIPU_API_KEY",
    "TOGETHER_API_KEY",
    "OPENROUTER_API_KEY",
    "COHERE_API_KEY",
    "PERPLEXITY_API_KEY",
    // Runtime / logging.
    "RUST_LOG",
    "RUST_BACKTRACE",
    "NO_COLOR",
    "CLICOLOR_FORCE",
    // Shell and PATH context.
    "SHELL",
    "TERM",
    "EDITOR",
];

/// Returns true if the variable name indicates a secret that must be
/// masked before printing. Error-side-safe: unknown variables are
/// treated as non-secret — callers only pass names from ENV_VARS.
fn is_secret_var(name: &str) -> bool {
    name.ends_with("_API_KEY") || name.ends_with("_TOKEN") || name.ends_with("_SECRET")
}

/// Mask a secret value so it's useful for "is it set?" checks without
/// leaking the secret itself. Shows length and last 4 chars.
fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return "(empty)".to_string();
    }
    let len = value.len();
    if len <= 4 {
        return format!("({len} chars, masked)");
    }
    let tail = &value[len - 4..];
    format!("({len} chars, ends in …{tail})")
}

/// Execute `/env` — print the environment vars agent-code actually reads,
/// with secrets masked. Omits any variable that isn't set.
fn execute_env() {
    println!();
    println!("  agent-code environment:");
    println!();

    let mut shown = 0;
    for name in ENV_VARS {
        let Ok(value) = std::env::var(name) else {
            continue;
        };
        let display = if is_secret_var(name) {
            mask_secret(&value)
        } else {
            value
        };
        println!("  {name}={display}");
        shown += 1;
    }

    if shown == 0 {
        println!("  (none of the tracked variables are set)");
    }

    println!();
    println!(
        "  {} tracked variables total; set but not listed variables are not read by agent-code.",
        ENV_VARS.len()
    );
}

/// Execute `/profile` with its sub-commands.
///
///   /profile                    list (same as `/profile list`)
///   /profile list               list saved profiles
///   /profile save <name>        save current config as <name>
///   /profile load <name>        replace runtime config with <name>
///   /profile delete <name>      delete <name>
///   /profile help               show usage
fn execute_profile(args: Option<&str>, engine: &mut QueryEngine) {
    let raw = args.map(|s| s.trim()).unwrap_or("");
    let (subcmd, rest) = raw
        .split_once(char::is_whitespace)
        .map(|(a, b)| (a.trim(), b.trim()))
        .unwrap_or((raw, ""));

    match subcmd {
        "" | "list" => {
            let profiles = agent_code_lib::services::profiles::list_profiles();
            if profiles.is_empty() {
                println!("No saved profiles.");
                println!("Usage: /profile save <name>");
                return;
            }
            println!("Saved profiles:");
            for p in &profiles {
                println!("  {}  — model={}", p.name, p.model);
            }
        }
        "save" => {
            if rest.is_empty() {
                println!("Usage: /profile save <name>");
                return;
            }
            match agent_code_lib::services::profiles::save_profile(rest, &engine.state().config) {
                Ok(path) => println!("Saved profile '{rest}' to {}", path.display()),
                Err(e) => eprintln!("Failed to save profile: {e}"),
            }
        }
        "load" => {
            if rest.is_empty() {
                println!("Usage: /profile load <name>");
                return;
            }
            match agent_code_lib::services::profiles::load_profile(rest) {
                Ok(new_config) => {
                    engine.state_mut().config = new_config;
                    println!("Loaded profile '{rest}'. Runtime config replaced.");
                    println!("Note: env var overrides (AGENT_CODE_MODEL etc.) are NOT re-applied.");
                }
                Err(e) => eprintln!("Failed to load profile: {e}"),
            }
        }
        "delete" | "rm" => {
            if rest.is_empty() {
                println!("Usage: /profile delete <name>");
                return;
            }
            match agent_code_lib::services::profiles::delete_profile(rest) {
                Ok(true) => println!("Deleted profile '{rest}'."),
                Ok(false) => println!("No profile named '{rest}'."),
                Err(e) => eprintln!("Failed to delete profile: {e}"),
            }
        }
        "help" => {
            println!("Usage:");
            println!("  /profile                   list saved profiles");
            println!("  /profile list              (same as above)");
            println!("  /profile save <name>       save current config as a new profile");
            println!("  /profile load <name>       replace runtime config with <name>");
            println!("  /profile delete <name>     remove <name>");
            println!();
            println!(
                "Profiles are full config snapshots stored under \
                 <config_dir>/agent-code/profiles/<name>.toml. Loading one \
                 replaces the runtime config wholesale; merging is intentionally \
                 not supported."
            );
        }
        other => {
            eprintln!("Unknown subcommand: {other}");
            println!("Try /profile help");
        }
    }
}

/// Execute `/skill validate <path>`. Lints the skill file (or every
/// `.md` file in the directory) and prints findings grouped by file.
fn execute_skill_validate(target: &str) {
    let path = std::path::PathBuf::from(target);
    if !path.exists() {
        eprintln!("Not found: {target}");
        return;
    }

    let files: Vec<std::path::PathBuf> = if path.is_dir() {
        std::fs::read_dir(&path)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "md"))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![path]
    };

    if files.is_empty() {
        println!("No .md files to validate at {target}.");
        return;
    }

    let mut any_findings = false;
    let mut total_errors = 0usize;
    let mut total_warns = 0usize;

    for file in &files {
        let findings = agent_code_lib::skills::validate_skill_file(file);
        if findings.is_empty() {
            println!("  ✓ {}", file.display());
            continue;
        }
        any_findings = true;
        println!("  ✗ {}", file.display());
        for f in &findings {
            match f.level {
                agent_code_lib::skills::ValidationLevel::Error => total_errors += 1,
                agent_code_lib::skills::ValidationLevel::Warning => total_warns += 1,
                agent_code_lib::skills::ValidationLevel::Info => {}
            }
            println!("     [{}] {}", f.level.label(), f.message);
        }
    }

    println!();
    if any_findings {
        println!(
            "  Summary: {total_errors} error(s), {total_warns} warning(s) across {} file(s).",
            files.len()
        );
    } else {
        println!("  All {} skill file(s) clean.", files.len());
    }
}

/// Execute `/copy` — collect the text from the most recent assistant
/// message and pipe it into the platform clipboard via a subprocess.
fn execute_copy(engine: &QueryEngine) {
    let Some(text) = last_assistant_text(engine) else {
        println!("No assistant message to copy.");
        return;
    };

    if text.is_empty() {
        println!("Last assistant message has no text content to copy.");
        return;
    }

    match copy_to_clipboard(&text) {
        Ok(cmd) => println!("Copied {} byte(s) to clipboard (via {cmd}).", text.len()),
        Err(e) => {
            eprintln!("Failed to copy to clipboard: {e}");
            eprintln!(
                "Install one of: pbcopy (macOS), xclip or xsel (Linux X11), \
                 wl-copy (Wayland), or clip (Windows)."
            );
        }
    }
}

/// Extract the concatenated text content of the most recent
/// assistant message. Returns `None` if the history contains no
/// assistant messages yet.
fn last_assistant_text(engine: &QueryEngine) -> Option<String> {
    for msg in engine.state().messages.iter().rev() {
        if let agent_code_lib::llm::message::Message::Assistant(a) = msg {
            let text: String = a
                .content
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join("\n");
            return Some(text);
        }
    }
    None
}

/// Pipe `text` into the first working platform clipboard command and
/// return its name. Probes in order of least-surprise for the current
/// platform. Returns `Err` if none succeeded.
fn copy_to_clipboard(text: &str) -> Result<&'static str, String> {
    let candidates: &[(&'static str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip", &[])]
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    } else {
        &[
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
            ("wl-copy", &[]),
        ]
    };

    let mut last_err: Option<String> = None;
    for (cmd, args) in candidates {
        match std::process::Command::new(cmd)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                use std::io::Write;
                if let Some(mut stdin) = child.stdin.take() {
                    if let Err(e) = stdin.write_all(text.as_bytes()) {
                        last_err = Some(format!("{cmd}: write error: {e}"));
                        let _ = child.wait();
                        continue;
                    }
                    drop(stdin);
                }
                match child.wait() {
                    Ok(status) if status.success() => return Ok(cmd),
                    Ok(status) => {
                        last_err = Some(format!("{cmd} exited with {status}"));
                        continue;
                    }
                    Err(e) => {
                        last_err = Some(format!("{cmd}: wait error: {e}"));
                        continue;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                last_err = Some(format!("{cmd}: {e}"));
                continue;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| "no clipboard command available on this platform".into()))
}

/// Per-category context breakdown. The fields are independent
/// token estimates summed to produce `total`. Exposed so tests can
/// exercise the accounting without driving a full QueryEngine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextBreakdown {
    pub system_prompt: u64,
    pub user_text: u64,
    pub assistant_text: u64,
    pub tool_use: u64,
    pub tool_result: u64,
    pub thinking: u64,
    pub system_messages: u64,
    pub tool_schemas: u64,
    pub total: u64,
    pub window: u64,
    pub compact_threshold: u64,
    pub message_count: usize,
}

/// Compute a per-category token breakdown for the given state and
/// tool registry. Uses the same estimation path as runtime planning.
fn compute_context_breakdown(
    state: &agent_code_lib::state::AppState,
    tools: &agent_code_lib::tools::registry::ToolRegistry,
) -> ContextBreakdown {
    use agent_code_lib::llm::message::{ContentBlock, Message};
    use agent_code_lib::services::tokens;

    // System prompt — rebuild from scratch here so the breakdown is
    // accurate even if the engine has a stale cache.
    let system_prompt = agent_code_lib::query::build_system_prompt(tools, state);
    let system_prompt_tokens = tokens::estimate_tokens(&system_prompt);

    let mut user_text = 0u64;
    let mut assistant_text = 0u64;
    let mut tool_use = 0u64;
    let mut tool_result = 0u64;
    let mut thinking = 0u64;
    let mut system_messages = 0u64;

    for msg in &state.messages {
        match msg {
            Message::User(u) => {
                for block in &u.content {
                    match block {
                        ContentBlock::Text { .. } => {
                            user_text =
                                user_text.saturating_add(tokens::estimate_block_tokens(block));
                        }
                        ContentBlock::ToolResult { .. } => {
                            tool_result =
                                tool_result.saturating_add(tokens::estimate_block_tokens(block));
                        }
                        _ => {
                            user_text =
                                user_text.saturating_add(tokens::estimate_block_tokens(block));
                        }
                    }
                }
            }
            Message::Assistant(a) => {
                for block in &a.content {
                    match block {
                        ContentBlock::Text { .. } => {
                            assistant_text =
                                assistant_text.saturating_add(tokens::estimate_block_tokens(block));
                        }
                        ContentBlock::ToolUse { .. } => {
                            tool_use =
                                tool_use.saturating_add(tokens::estimate_block_tokens(block));
                        }
                        ContentBlock::Thinking { .. } => {
                            thinking =
                                thinking.saturating_add(tokens::estimate_block_tokens(block));
                        }
                        _ => {
                            assistant_text =
                                assistant_text.saturating_add(tokens::estimate_block_tokens(block));
                        }
                    }
                }
            }
            Message::System(_) => {
                system_messages =
                    system_messages.saturating_add(tokens::estimate_message_tokens(msg));
            }
        }
    }

    // Tool schemas — each enabled tool contributes name + description
    // + JSON schema as sent to the model.
    let mut tool_schemas = 0u64;
    for tool in tools.all() {
        if !tool.is_enabled() {
            continue;
        }
        tool_schemas = tool_schemas.saturating_add(tokens::estimate_tokens(tool.name()));
        tool_schemas = tool_schemas.saturating_add(tokens::estimate_tokens(tool.description()));
        let schema_str = serde_json::to_string(&tool.input_schema()).unwrap_or_default();
        tool_schemas = tool_schemas.saturating_add(tokens::estimate_tokens(&schema_str));
    }

    let total = system_prompt_tokens
        + user_text
        + assistant_text
        + tool_use
        + tool_result
        + thinking
        + system_messages
        + tool_schemas;

    let model = &state.config.api.model;
    let window = tokens::context_window_for_model(model);
    let compact_threshold = agent_code_lib::services::compact::auto_compact_threshold(model);

    ContextBreakdown {
        system_prompt: system_prompt_tokens,
        user_text,
        assistant_text,
        tool_use,
        tool_result,
        thinking,
        system_messages,
        tool_schemas,
        total,
        window,
        compact_threshold,
        message_count: state.messages.len(),
    }
}

/// Execute `/ctxviz` — compute the breakdown and render a table.
fn execute_ctxviz(engine: &QueryEngine) {
    let b = compute_context_breakdown(engine.state(), engine_tools(engine));
    let pct = |n: u64| {
        if b.total == 0 {
            0
        } else {
            (n as f64 / b.total as f64 * 100.0).round() as u64
        }
    };
    let window_pct = if b.window > 0 {
        (b.total as f64 / b.window as f64 * 100.0).round() as u64
    } else {
        0
    };
    println!(
        "Context breakdown (~{} tokens, {}% of {} window):\n",
        b.total, window_pct, b.window
    );
    println!(
        "  System prompt       {:>8}  {:>3}%",
        b.system_prompt,
        pct(b.system_prompt)
    );
    println!(
        "  Tool schemas        {:>8}  {:>3}%",
        b.tool_schemas,
        pct(b.tool_schemas)
    );
    println!(
        "  User text           {:>8}  {:>3}%",
        b.user_text,
        pct(b.user_text)
    );
    println!(
        "  Assistant text      {:>8}  {:>3}%",
        b.assistant_text,
        pct(b.assistant_text)
    );
    println!(
        "  Tool use            {:>8}  {:>3}%",
        b.tool_use,
        pct(b.tool_use)
    );
    println!(
        "  Tool result         {:>8}  {:>3}%",
        b.tool_result,
        pct(b.tool_result)
    );
    println!(
        "  Thinking            {:>8}  {:>3}%",
        b.thinking,
        pct(b.thinking)
    );
    println!(
        "  System msgs         {:>8}  {:>3}%",
        b.system_messages,
        pct(b.system_messages)
    );
    println!();
    println!(
        "  {} messages · auto-compact at {} tokens",
        b.message_count, b.compact_threshold
    );
    if b.total >= b.compact_threshold {
        println!("  ⚠ Over compact threshold — next turn will auto-compact.");
    }
}

/// Accessor shim — the engine doesn't currently expose its registry
/// publicly, but it does hand out `&AppState`. We lazily build the
/// default tools registry once per process so `/ctxviz` can include
/// schema token counts in its breakdown without requiring access to
/// the engine's own registry. This is an approximation — if the
/// engine has registered additional tools (e.g. MCP), those won't be
/// counted. Good enough for a debugging aid.
fn engine_tools(_engine: &QueryEngine) -> &agent_code_lib::tools::registry::ToolRegistry {
    use std::sync::OnceLock;
    static REG: OnceLock<agent_code_lib::tools::registry::ToolRegistry> = OnceLock::new();
    REG.get_or_init(agent_code_lib::tools::registry::ToolRegistry::default_tools)
}

/// Execute the /tag command.
///
///   /tag                     list tags on the current session
///   /tag <tag>               add a tag
///   /tag --remove <tag>      remove a tag
///   /tag --clear             remove all tags
///
/// Tags are lowercase, alphanumeric + `-`/`_`, max 32 chars.
fn execute_tag(args: Option<&str>, engine: &QueryEngine) {
    let session_id = engine.state().session_id.clone();
    let raw = args.map(|s| s.trim()).unwrap_or("");

    if raw.is_empty() {
        // List current tags by reading the saved session.
        match agent_code_lib::services::session::load_session(&session_id) {
            Ok(data) => {
                if data.tags.is_empty() {
                    println!("No tags on the current session.");
                    println!("Usage: /tag <tag>");
                } else {
                    println!("Tags: #{}", data.tags.join(" #"));
                }
            }
            Err(_) => {
                println!(
                    "Current session has no saved file yet — send a turn first, \
                     then /tag <tag>."
                );
            }
        }
        return;
    }

    if raw == "--clear" {
        match agent_code_lib::services::session::load_session(&session_id) {
            Ok(mut data) => {
                let n = data.tags.len();
                data.tags.clear();
                let dir = dirs::config_dir()
                    .map(|d| d.join("agent-code").join("sessions"))
                    .expect("config dir");
                let path = dir.join(format!("{}.json", data.id));
                match serde_json::to_string_pretty(&data) {
                    Ok(json) => {
                        let masked = agent_code_lib::services::secret_masker::mask(&json);
                        match std::fs::write(&path, masked) {
                            Ok(()) => println!("Cleared {n} tag{}.", if n == 1 { "" } else { "s" }),
                            Err(e) => eprintln!("Failed to clear tags: {e}"),
                        }
                    }
                    Err(e) => eprintln!("Failed to serialize session: {e}"),
                }
            }
            Err(e) => eprintln!("Failed to load session: {e}"),
        }
        return;
    }

    if let Some(rest) = raw.strip_prefix("--remove ") {
        let target = rest.trim();
        match agent_code_lib::services::session::remove_session_tag(&session_id, target) {
            Ok(true) => println!("Removed tag: {target}"),
            Ok(false) => println!("Not tagged: {target}"),
            Err(e) => eprintln!("Failed to remove tag: {e}"),
        }
        return;
    }

    match agent_code_lib::services::session::add_session_tag(&session_id, raw) {
        Ok(true) => println!("Added tag: {raw}"),
        Ok(false) => println!("Already tagged: {raw}"),
        Err(e) => eprintln!("Failed to add tag: {e}"),
    }
}

/// Execute `/rules` — list, enable, or disable project rules loaded
/// from `.agent/rules/*.md` and injected into the system prompt.
fn execute_rules(args: Option<&str>, _engine: &mut QueryEngine) {
    let raw = args.map(|s| s.trim()).unwrap_or("");
    let (subcmd, rest) = raw
        .split_once(char::is_whitespace)
        .map(|(a, b)| (a.trim(), b.trim()))
        .unwrap_or((raw, ""));

    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to resolve current directory: {e}");
            return;
        }
    };

    match subcmd {
        "" | "list" => {
            let rules = agent_code_lib::services::rules::load_project_rules(&cwd);
            if rules.is_empty() {
                println!("No project rules found under .agent/rules/.");
                println!("Create .agent/rules/<name>.md to add one.");
                return;
            }
            println!("Project rules (.agent/rules/):");
            for r in &rules {
                let status = if r.enabled { "on " } else { "off" };
                println!("  [{status}] {:<24} p{:<3} {}", r.name, r.priority, r.title);
            }
        }
        "enable" => {
            if rest.is_empty() {
                println!("Usage: /rules enable <name>");
                return;
            }
            match agent_code_lib::services::rules::set_rule_enabled(&cwd, rest, true) {
                Ok(true) => println!("Enabled rule '{rest}'."),
                Ok(false) => println!("Rule '{rest}' was already enabled."),
                Err(e) => eprintln!("Failed to enable rule: {e}"),
            }
        }
        "disable" => {
            if rest.is_empty() {
                println!("Usage: /rules disable <name>");
                return;
            }
            match agent_code_lib::services::rules::set_rule_enabled(&cwd, rest, false) {
                Ok(true) => println!("Disabled rule '{rest}'."),
                Ok(false) => println!("Rule '{rest}' was already disabled."),
                Err(e) => eprintln!("Failed to disable rule: {e}"),
            }
        }
        "help" => {
            println!("Usage:");
            println!("  /rules                     list project rules with on/off status");
            println!("  /rules list                (same as above)");
            println!("  /rules enable <name>       turn rule <name> on");
            println!("  /rules disable <name>      turn rule <name> off");
            println!();
            println!(
                "Rules are plain markdown files at .agent/rules/<name>.md. \
                 Optional YAML frontmatter: title, priority (lower = earlier), \
                 enabled (bool). Enabled rules are injected into the system \
                 prompt every turn."
            );
        }
        other => {
            eprintln!("Unknown subcommand: {other}");
            println!("Try /rules help");
        }
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
    fn thinking_walker_skips_user_messages_and_non_thinking_blocks() {
        use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message, user_message};
        use uuid::Uuid;

        let mk_assistant = |content| {
            Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: "0".to_string(),
                content,
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            })
        };

        let assistant_with_thinking = mk_assistant(vec![
            ContentBlock::Thinking {
                thinking: "first thought".to_string(),
                signature: None,
            },
            ContentBlock::Text {
                text: "user-facing reply".to_string(),
            },
        ]);
        let assistant_without_thinking = mk_assistant(vec![ContentBlock::Text {
            text: "plain reply".to_string(),
        }]);
        let messages = vec![
            user_message("hi"),
            assistant_with_thinking,
            user_message("next"),
            assistant_without_thinking,
        ];

        let turns = collect_thinking_turns(&messages);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0], vec!["first thought".to_string()]);
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

    #[test]
    fn is_secret_var_classifies_api_keys_and_tokens() {
        assert!(is_secret_var("ANTHROPIC_API_KEY"));
        assert!(is_secret_var("OPENAI_API_KEY"));
        assert!(is_secret_var("GITHUB_TOKEN"));
        assert!(is_secret_var("STRIPE_SECRET"));
        assert!(!is_secret_var("RUST_LOG"));
        assert!(!is_secret_var("AGENT_CODE_MODEL"));
        assert!(!is_secret_var("AGENT_CODE_API_BASE_URL"));
    }

    #[test]
    fn mask_secret_preserves_length_and_tail_for_long_values() {
        let masked = mask_secret("sk-ant-api03-abcdef1234567890");
        assert!(masked.contains("29 chars"));
        assert!(masked.contains("ends in …7890"));
        // The secret itself is NOT in the masked output (sanity-check the guarantee).
        assert!(!masked.contains("abcdef"));
    }

    #[test]
    fn mask_secret_handles_short_values_without_leaking_tail() {
        assert_eq!(mask_secret("x"), "(1 chars, masked)");
        assert_eq!(mask_secret("abc"), "(3 chars, masked)");
        assert_eq!(mask_secret(""), "(empty)");
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn copy_to_clipboard_errors_with_empty_path() {
        // Force `copy_to_clipboard` to have no candidates on PATH by
        // temporarily emptying PATH. This verifies the error path
        // returns a helpful string instead of silently succeeding.
        //
        // Windows-only: `clip.exe` lives in `%SYSTEMROOT%\System32` and
        // Windows' `CreateProcess` searches the system directory even
        // with an empty PATH, so clearing PATH doesn't actually hide
        // it. The test is an assertion about the fallback code path on
        // *nix; the Windows probe (`clip` only) has a simpler path.
        //
        // SAFETY: single-threaded test, restored before exit.
        let prev = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", "");
        }
        let result = copy_to_clipboard("hello");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(result.is_err(), "expected error on empty PATH");
    }

    // ---- /ctxviz breakdown accounting ----

    #[test]
    fn ctxviz_breakdown_empty_state_has_only_system_prompt() {
        let state = agent_code_lib::state::AppState::new(agent_code_lib::config::Config::default());
        let tools = agent_code_lib::tools::registry::ToolRegistry::new();
        let b = compute_context_breakdown(&state, &tools);
        assert_eq!(b.message_count, 0);
        assert_eq!(b.user_text, 0);
        assert_eq!(b.assistant_text, 0);
        assert_eq!(b.tool_use, 0);
        assert_eq!(b.tool_result, 0);
        assert_eq!(b.tool_schemas, 0, "empty registry → no schema tokens");
        assert!(b.system_prompt > 0, "system prompt always has content");
        assert!(b.total >= b.system_prompt);
    }

    #[test]
    fn ctxviz_breakdown_user_text_counted_separately_from_assistant() {
        use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message, user_message};
        fn mk_assistant(text: &str) -> Message {
            Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: "2026-04-22T00:00:00Z".into(),
                content: vec![ContentBlock::Text { text: text.into() }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            })
        }
        let mut state =
            agent_code_lib::state::AppState::new(agent_code_lib::config::Config::default());
        state.push_message(user_message("hello there"));
        state.push_message(mk_assistant("general kenobi"));
        let tools = agent_code_lib::tools::registry::ToolRegistry::new();
        let b = compute_context_breakdown(&state, &tools);
        assert_eq!(b.message_count, 2);
        assert!(b.user_text > 0, "user text must accumulate");
        assert!(b.assistant_text > 0, "assistant text must accumulate");
        assert_eq!(b.tool_use, 0);
        assert_eq!(b.tool_result, 0);
    }

    #[test]
    fn ctxviz_breakdown_total_equals_sum_of_parts() {
        use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message, user_message};
        fn mk_assistant(text: &str) -> Message {
            Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: "2026-04-22T00:00:00Z".into(),
                content: vec![ContentBlock::Text { text: text.into() }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            })
        }
        let mut state =
            agent_code_lib::state::AppState::new(agent_code_lib::config::Config::default());
        state.push_message(user_message("hi"));
        state.push_message(mk_assistant("ok"));
        let tools = agent_code_lib::tools::registry::ToolRegistry::new();
        let b = compute_context_breakdown(&state, &tools);
        let sum = b.system_prompt
            + b.user_text
            + b.assistant_text
            + b.tool_use
            + b.tool_result
            + b.thinking
            + b.system_messages
            + b.tool_schemas;
        assert_eq!(b.total, sum);
    }
}
