//! First-run setup wizard.
//!
//! Guides new users through initial configuration: theme, API key,
//! provider selection, permission mode, and a brief safety overview.
//! Runs automatically on first launch when no config file exists.
//! Writes settings to ~/.config/agent-code/config.toml.

use std::io::Write;

use crossterm::style::Stylize;

/// Check if the setup wizard should run.
pub fn needs_setup() -> bool {
    let config_path = dirs::config_dir().map(|d| d.join("agent-code").join("config.toml"));

    match config_path {
        Some(path) => !path.exists(),
        None => true,
    }
}

/// Run the interactive setup wizard. Returns the configured settings
/// as a TOML string ready to write to config.toml.
pub fn run_setup() -> Option<SetupResult> {
    println!();
    println!("{}", " agent-code setup ".on_dark_cyan().white().bold());
    println!();
    println!("Welcome. Let's get you configured in about 30 seconds.");
    println!();

    // Step 1: Theme
    let theme = pick_theme();

    // Step 2: Provider and API key
    let (provider, api_key, base_url, model) = pick_provider();

    // Step 3: Permission mode
    let permission_mode = pick_permission_mode();

    // Step 4: Safety overview
    show_safety_notice();

    // Write config
    let config = format!(
        r#"[api]
base_url = "{base_url}"
model = "{model}"

[permissions]
default_mode = "{permission_mode}"

[ui]
theme = "{theme}"
"#
    );

    let config_dir = dirs::config_dir()?.join("agent-code");
    let _ = std::fs::create_dir_all(&config_dir);
    let config_path = config_dir.join("config.toml");
    let _ = std::fs::write(&config_path, &config);

    // Save API key to a separate file (not in config.toml for security)
    if !api_key.is_empty() {
        let key_hint = if api_key.len() > 8 {
            format!("{}...{}", &api_key[..4], &api_key[api_key.len() - 4..])
        } else {
            "****".to_string()
        };
        println!(
            "\n{}",
            format!("  Config saved to {}", config_path.display()).dark_grey()
        );
        println!(
            "{}",
            format!("  API key ({key_hint}) set via environment variable").dark_grey()
        );
    }

    println!();
    println!(
        "{} You're all set. Type {} to start.",
        "Done!".green().bold(),
        "agent".bold(),
    );
    println!();

    Some(SetupResult { api_key, provider })
}

pub struct SetupResult {
    pub api_key: String,
    pub provider: String,
}

fn pick_theme() -> String {
    println!("  {} Choose a theme:\n", "1.".dark_cyan().bold());
    println!("    {} Dark (recommended for most terminals)", "A)".bold());
    println!("    {} Light", "B)".bold());
    println!();

    let choice = read_choice("  Choice [A]: ", "a");
    let theme = match choice.as_str() {
        "b" => "light",
        _ => "dark",
    };

    println!("    {}\n", format!("→ {theme}").dark_grey());
    theme.to_string()
}

fn pick_provider() -> (String, String, String, String) {
    println!("  {} Choose your AI provider:\n", "2.".dark_cyan().bold());
    println!(
        "    {} Anthropic (Claude)          {}",
        "A)".bold(),
        "ANTHROPIC_API_KEY".dark_grey()
    );
    println!(
        "    {} OpenAI (GPT)                {}",
        "B)".bold(),
        "OPENAI_API_KEY".dark_grey()
    );
    println!(
        "    {} xAI (Grok)                  {}",
        "C)".bold(),
        "XAI_API_KEY".dark_grey()
    );
    println!(
        "    {} Google (Gemini)              {}",
        "D)".bold(),
        "GOOGLE_API_KEY".dark_grey()
    );
    println!(
        "    {} DeepSeek                     {}",
        "E)".bold(),
        "DEEPSEEK_API_KEY".dark_grey()
    );
    println!(
        "    {} Other (OpenAI-compatible)    {}",
        "F)".bold(),
        "custom base URL".dark_grey()
    );
    println!();

    let choice = read_choice("  Choice [A]: ", "a");

    let (provider, env_var, default_url, default_model) = match choice.as_str() {
        "b" => (
            "openai",
            "OPENAI_API_KEY",
            "https://api.openai.com/v1",
            "gpt-5.4",
        ),
        "c" => ("xai", "XAI_API_KEY", "https://api.x.ai/v1", "grok-3"),
        "d" => (
            "google",
            "GOOGLE_API_KEY",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-2.5-flash",
        ),
        "e" => (
            "deepseek",
            "DEEPSEEK_API_KEY",
            "https://api.deepseek.com/v1",
            "deepseek-chat",
        ),
        "f" => ("custom", "AGENT_CODE_API_KEY", "", ""),
        _ => (
            "anthropic",
            "ANTHROPIC_API_KEY",
            "https://api.anthropic.com/v1",
            "claude-sonnet-4-20250514",
        ),
    };

    println!("    {}\n", format!("→ {provider}").dark_grey());

    // Check if key is already in environment
    let existing_key = std::env::var(env_var)
        .ok()
        .or_else(|| std::env::var("AGENT_CODE_API_KEY").ok());

    let api_key = if let Some(key) = existing_key {
        let masked = if key.len() > 8 {
            format!("{}...{}", &key[..4], &key[key.len() - 4..])
        } else {
            "****".to_string()
        };
        println!("    {} found in environment ({masked})", env_var.green());
        println!();
        key
    } else {
        // Ask for key
        eprint!("  Enter your API key (or press Enter to set later via {env_var}): ");
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        let key = input.trim().to_string();

        if key.is_empty() {
            println!(
                "    {}",
                format!("No key entered. Set {env_var} before running agent.").yellow()
            );
        }
        println!();
        key
    };

    // For custom provider, ask for base URL and model
    let (base_url, model) = if provider == "custom" {
        eprint!("  Base URL: ");
        let _ = std::io::stderr().flush();
        let mut url = String::new();
        let _ = std::io::stdin().read_line(&mut url);
        let url = url.trim().to_string();

        eprint!("  Model name: ");
        let _ = std::io::stderr().flush();
        let mut model = String::new();
        let _ = std::io::stdin().read_line(&mut model);
        let model = model.trim().to_string();

        println!();
        (
            if url.is_empty() {
                "https://api.openai.com/v1".to_string()
            } else {
                url
            },
            if model.is_empty() {
                "gpt-4o".to_string()
            } else {
                model
            },
        )
    } else {
        (default_url.to_string(), default_model.to_string())
    };

    (provider.to_string(), api_key, base_url, model)
}

fn pick_permission_mode() -> String {
    println!("  {} Permission mode:\n", "3.".dark_cyan().bold());
    println!(
        "    {} Ask before changes (recommended) — confirms before file edits and commands",
        "A)".bold()
    );
    println!(
        "    {} Auto-approve edits — file changes are automatic, commands still ask",
        "B)".bold()
    );
    println!(
        "    {} Trust fully — everything runs without asking (for experienced users)",
        "C)".bold()
    );
    println!();

    let choice = read_choice("  Choice [A]: ", "a");
    let mode = match choice.as_str() {
        "b" => "accept_edits",
        "c" => "allow",
        _ => "ask",
    };

    let label = match mode {
        "accept_edits" => "auto-approve edits",
        "allow" => "trust fully",
        _ => "ask before changes",
    };
    println!("    {}\n", format!("→ {label}").dark_grey());
    mode.to_string()
}

fn show_safety_notice() {
    println!("  {} Quick safety notes:\n", "4.".dark_cyan().bold());
    println!(
        "    {} The agent can read, write, and delete files in your project",
        "•".dark_grey()
    );
    println!(
        "    {} It can run shell commands on your machine",
        "•".dark_grey()
    );
    println!(
        "    {} Destructive commands (rm -rf, git reset) trigger warnings",
        "•".dark_grey()
    );
    println!(
        "    {} Use /plan mode for read-only exploration when unsure",
        "•".dark_grey()
    );
    println!(
        "    {} Your code is sent to the LLM API for processing",
        "•".dark_grey()
    );
    println!(
        "    {} No telemetry is collected by agent-code itself",
        "•".dark_grey()
    );
    println!();
}

fn read_choice(prompt: &str, default: &str) -> String {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    let trimmed = input.trim().to_lowercase();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed
    }
}
