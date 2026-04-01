//! First-run setup wizard.
//!
//! Guides new users through initial configuration with arrow-key
//! navigable menus: theme, API provider, permission mode, and
//! a brief safety overview. Runs automatically on first launch
//! or when no API key is configured.

use std::io::Write;

use crossterm::style::Stylize;

use super::selector::{SelectOption, select};

/// Check if the setup wizard should run.
pub fn needs_setup() -> bool {
    let config_path = dirs::config_dir().map(|d| d.join("agent-code").join("config.toml"));
    match config_path {
        Some(path) => !path.exists(),
        None => true,
    }
}

/// Run the interactive setup wizard.
pub fn run_setup() -> Option<SetupResult> {
    println!();
    println!("{}", " agent-code setup ".on_dark_cyan().white().bold());
    println!();
    println!("Use arrow keys to navigate, Enter to select.\n");

    // Step 1: Theme.
    println!("  {} Appearance:\n", "1.".dark_cyan().bold());
    let theme = select(&[
        SelectOption {
            label: "Midnight".into(),
            description: "(dark, recommended)".into(),
            value: "midnight".into(),
            preview: Some(
                "\x1b[48;2;24;24;36m\x1b[38;2;86;182;194m  fn \x1b[38;2;198;160;246mmain\x1b[38;2;204;204;204m() {\x1b[0m\n\
                 \x1b[48;2;24;24;36m\x1b[38;2;204;204;204m      \x1b[38;2;86;182;194mlet\x1b[38;2;204;204;204m msg = \x1b[38;2;152;195;121m\"hello world\"\x1b[38;2;204;204;204m;\x1b[0m\n\
                 \x1b[48;2;24;24;36m\x1b[38;2;204;204;204m      println!(\x1b[38;2;152;195;121m\"{}\"\x1b[38;2;204;204;204m, msg);\x1b[0m\n\
                 \x1b[48;2;24;24;36m\x1b[38;2;204;204;204m  }\x1b[0m\n\
                 \x1b[48;2;24;24;36m\x1b[38;2;86;182;194m  // \x1b[38;2;106;115;125mfast and minimal\x1b[0m".to_string(),
            ),
        },
        SelectOption {
            label: "Daybreak".into(),
            description: "(light)".into(),
            value: "daybreak".into(),
            preview: Some(
                "\x1b[48;2;253;246;227m\x1b[38;2;38;139;210m  fn \x1b[38;2;108;113;196mmain\x1b[38;2;55;65;81m() {\x1b[0m\n\
                 \x1b[48;2;253;246;227m\x1b[38;2;55;65;81m      \x1b[38;2;38;139;210mlet\x1b[38;2;55;65;81m msg = \x1b[38;2;133;153;0m\"hello world\"\x1b[38;2;55;65;81m;\x1b[0m\n\
                 \x1b[48;2;253;246;227m\x1b[38;2;55;65;81m      println!(\x1b[38;2;133;153;0m\"{}\"\x1b[38;2;55;65;81m, msg);\x1b[0m\n\
                 \x1b[48;2;253;246;227m\x1b[38;2;55;65;81m  }\x1b[0m\n\
                 \x1b[48;2;253;246;227m\x1b[38;2;38;139;210m  // \x1b[38;2;147;161;161mclean and bright\x1b[0m".to_string(),
            ),
        },
        SelectOption {
            label: "Midnight Muted".into(),
            description: "(dark, softer contrast)".into(),
            value: "midnight-muted".into(),
            preview: Some(
                "\x1b[48;2;40;44;52m\x1b[38;2;97;175;239m  fn \x1b[38;2;198;120;221mmain\x1b[38;2;171;178;191m() {\x1b[0m\n\
                 \x1b[48;2;40;44;52m\x1b[38;2;171;178;191m      \x1b[38;2;97;175;239mlet\x1b[38;2;171;178;191m msg = \x1b[38;2;152;195;121m\"hello world\"\x1b[38;2;171;178;191m;\x1b[0m\n\
                 \x1b[48;2;40;44;52m\x1b[38;2;171;178;191m      println!(\x1b[38;2;152;195;121m\"{}\"\x1b[38;2;171;178;191m, msg);\x1b[0m\n\
                 \x1b[48;2;40;44;52m\x1b[38;2;171;178;191m  }\x1b[0m\n\
                 \x1b[48;2;40;44;52m\x1b[38;2;97;175;239m  // \x1b[38;2;92;99;112measy on the eyes\x1b[0m".to_string(),
            ),
        },
        SelectOption {
            label: "Daybreak Muted".into(),
            description: "(light, softer contrast)".into(),
            value: "daybreak-muted".into(),
            preview: Some(
                "\x1b[48;2;250;244;235m\x1b[38;2;66;133;244m  fn \x1b[38;2;140;100;200mmain\x1b[38;2;80;90;100m() {\x1b[0m\n\
                 \x1b[48;2;250;244;235m\x1b[38;2;80;90;100m      \x1b[38;2;66;133;244mlet\x1b[38;2;80;90;100m msg = \x1b[38;2;80;160;80m\"hello world\"\x1b[38;2;80;90;100m;\x1b[0m\n\
                 \x1b[48;2;250;244;235m\x1b[38;2;80;90;100m      println!(\x1b[38;2;80;160;80m\"{}\"\x1b[38;2;80;90;100m, msg);\x1b[0m\n\
                 \x1b[48;2;250;244;235m\x1b[38;2;80;90;100m  }\x1b[0m\n\
                 \x1b[48;2;250;244;235m\x1b[38;2;66;133;244m  // \x1b[38;2;160;170;180mgentle warmth\x1b[0m".to_string(),
            ),
        },
        SelectOption {
            label: "Terminal Native".into(),
            description: "(uses your terminal colors)".into(),
            value: "terminal".into(),
            preview: Some(
                "\x1b[36m  fn \x1b[35mmain\x1b[0m() {\n\
                 \x1b[0m      \x1b[36mlet\x1b[0m msg = \x1b[32m\"hello world\"\x1b[0m;\n\
                 \x1b[0m      println!(\x1b[32m\"{}\"\x1b[0m, msg);\n\
                 \x1b[0m  }\n\
                 \x1b[36m  // \x1b[90myour colors, your way\x1b[0m".to_string(),
            ),
        },
        SelectOption {
            label: "Auto".into(),
            description: "(follows system dark/light mode)".into(),
            value: "auto".into(),
            preview: Some(
                "\x1b[90m  Detects your system preference\n\
                 \x1b[90m  and switches between Midnight\n\
                 \x1b[90m  and Daybreak automatically.\n\
                 \x1b[0m\n\
                 ".to_string(),
            ),
        },
    ]);
    println!();

    // Step 2: Provider.
    println!("  {} AI provider:\n", "2.".dark_cyan().bold());
    let provider_choice = select(&[
        SelectOption {
            label: "OpenAI (GPT)".into(),
            description: "GPT-5.4, GPT-4.1".into(),
            value: "openai".into(),
            preview: None,
        },
        SelectOption {
            label: "Anthropic (Claude)".into(),
            description: "Opus, Sonnet, Haiku".into(),
            value: "anthropic".into(),
            preview: None,
        },
        SelectOption {
            label: "xAI (Grok)".into(),
            description: "Grok-3, Grok-2".into(),
            value: "xai".into(),
            preview: None,
        },
        SelectOption {
            label: "Google (Gemini)".into(),
            description: "Gemini 2.5 Flash/Pro".into(),
            value: "google".into(),
            preview: None,
        },
        SelectOption {
            label: "DeepSeek".into(),
            description: "DeepSeek-V3".into(),
            value: "deepseek".into(),
            preview: None,
        },
        SelectOption {
            label: "Other".into(),
            description: "(OpenAI-compatible endpoint)".into(),
            value: "custom".into(),
            preview: None,
        },
    ]);

    let (env_var, default_url, default_model) = match provider_choice.as_str() {
        "anthropic" => (
            "ANTHROPIC_API_KEY",
            "https://api.anthropic.com/v1",
            "claude-sonnet-4-20250514",
        ),
        "xai" => ("XAI_API_KEY", "https://api.x.ai/v1", "grok-3"),
        "google" => (
            "GOOGLE_API_KEY",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-2.5-flash",
        ),
        "deepseek" => (
            "DEEPSEEK_API_KEY",
            "https://api.deepseek.com/v1",
            "deepseek-chat",
        ),
        "custom" => ("AGENT_CODE_API_KEY", "", ""),
        _ => ("OPENAI_API_KEY", "https://api.openai.com/v1", "gpt-5.4"),
    };
    println!();

    // Check for existing key in environment.
    let existing_key = std::env::var(env_var)
        .ok()
        .or_else(|| std::env::var("AGENT_CODE_API_KEY").ok());

    let api_key = if let Some(key) = existing_key {
        let masked = if key.len() > 8 {
            format!("{}...{}", &key[..4], &key[key.len() - 4..])
        } else {
            "****".to_string()
        };
        println!("    {} found ({masked})\n", env_var.green());
        key
    } else {
        eprint!("  Paste your API key (or Enter to set {env_var} later): ");
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        let key = input.trim().to_string();
        if key.is_empty() {
            println!(
                "    {}",
                format!("Set {env_var} before running agent.").yellow()
            );
        }
        println!();
        key
    };

    // Custom provider: ask for URL and model.
    let (base_url, model) = if provider_choice == "custom" {
        eprint!("  Base URL: ");
        let _ = std::io::stderr().flush();
        let mut url = String::new();
        let _ = std::io::stdin().read_line(&mut url);
        let url = url.trim().to_string();

        eprint!("  Model name: ");
        let _ = std::io::stderr().flush();
        let mut m = String::new();
        let _ = std::io::stdin().read_line(&mut m);
        let m = m.trim().to_string();
        println!();
        (
            if url.is_empty() {
                "https://api.openai.com/v1".to_string()
            } else {
                url
            },
            if m.is_empty() {
                "gpt-5.4".to_string()
            } else {
                m
            },
        )
    } else {
        (default_url.to_string(), default_model.to_string())
    };

    // Step 3: Permission mode.
    println!("  {} Permission mode:\n", "3.".dark_cyan().bold());
    let permission_mode = select(&[
        SelectOption {
            label: "Ask before changes".into(),
            description: "(recommended) confirms before edits and commands".into(),
            value: "ask".into(),
            preview: None,
        },
        SelectOption {
            label: "Auto-approve edits".into(),
            description: "file changes automatic, commands still ask".into(),
            value: "accept_edits".into(),
            preview: None,
        },
        SelectOption {
            label: "Trust fully".into(),
            description: "everything runs without asking".into(),
            value: "allow".into(),
            preview: None,
        },
    ]);
    println!();

    // Step 4: Safety notes.
    println!("  {} Quick safety notes:\n", "4.".dark_cyan().bold());
    println!(
        "    {} The agent can read, write, and delete files",
        "•".dark_grey()
    );
    println!(
        "    {} It can run shell commands on your machine",
        "•".dark_grey()
    );
    println!(
        "    {} Destructive commands trigger warnings",
        "•".dark_grey()
    );
    println!(
        "    {} Use /plan mode for read-only exploration",
        "•".dark_grey()
    );
    println!("    {} No telemetry is collected", "•".dark_grey());
    println!();

    // Write config.
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

    println!(
        "{}",
        format!("  Config saved to {}", config_path.display()).dark_grey()
    );
    println!();
    println!(
        "  {} Type {} to start.",
        "Ready!".green().bold(),
        "agent".bold(),
    );
    println!();

    Some(SetupResult {
        api_key,
        provider: provider_choice,
    })
}

pub struct SetupResult {
    pub api_key: String,
    pub provider: String,
}
