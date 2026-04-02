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
            label: "Groq".into(),
            description: "Llama, Mixtral (fast inference)".into(),
            value: "groq".into(),
            preview: None,
        },
        SelectOption {
            label: "Mistral".into(),
            description: "Mistral Large, Codestral".into(),
            value: "mistral".into(),
            preview: None,
        },
        SelectOption {
            label: "Together".into(),
            description: "Llama, Qwen, 100+ open models".into(),
            value: "together".into(),
            preview: None,
        },
        SelectOption {
            label: "Zhipu (z.ai)".into(),
            description: "GLM-4.7, GLM-4.6, GLM-4.5".into(),
            value: "zhipu".into(),
            preview: None,
        },
        SelectOption {
            label: "Ollama (local)".into(),
            description: "Run models locally, no API key needed".into(),
            value: "ollama".into(),
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
        "groq" => (
            "GROQ_API_KEY",
            "https://api.groq.com/openai/v1",
            "llama-3.3-70b-versatile",
        ),
        "mistral" => (
            "MISTRAL_API_KEY",
            "https://api.mistral.ai/v1",
            "mistral-large-latest",
        ),
        "together" => (
            "TOGETHER_API_KEY",
            "https://api.together.xyz/v1",
            "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
        ),
        "zhipu" => (
            "ZHIPU_API_KEY",
            "https://open.bigmodel.cn/api/paas/v4",
            "glm-4.7",
        ),
        "ollama" => ("", "http://localhost:11434/v1", "qwen3:latest"),
        "custom" => ("AGENT_CODE_API_KEY", "", ""),
        _ => ("OPENAI_API_KEY", "https://api.openai.com/v1", "gpt-5.4"),
    };
    println!();

    // Handle API key based on provider.
    let api_key = if provider_choice == "ollama" {
        // Ollama: no key needed, check if running.
        println!();
        println!("    {} No API key needed for local Ollama.", "✓".green());
        // Check if Ollama is running.
        match std::process::Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "http://localhost:11434/api/tags",
            ])
            .output()
        {
            Ok(out) if String::from_utf8_lossy(&out.stdout).trim() == "200" => {
                println!("    {} Ollama is running at localhost:11434", "✓".green());
            }
            _ => {
                println!(
                    "    {} Ollama not detected. Start it with: {}",
                    "!".yellow(),
                    "ollama serve".bold()
                );
            }
        }

        // Let user pick a model.
        println!();
        println!("  {} Ollama model:\n", "  ".dark_cyan().bold());
        let ollama_model = select(&[
            SelectOption {
                label: "qwen3:latest".into(),
                description: "8B, tool use, recommended".into(),
                value: "qwen3:latest".into(),
                preview: None,
            },
            SelectOption {
                label: "mistral:latest".into(),
                description: "7B, tool use".into(),
                value: "mistral:latest".into(),
                preview: None,
            },
            SelectOption {
                label: "mistral-nemo:latest".into(),
                description: "12B, tool use".into(),
                value: "mistral-nemo:latest".into(),
                preview: None,
            },
            SelectOption {
                label: "llama4:latest".into(),
                description: "109B, tool use".into(),
                value: "llama4:latest".into(),
                preview: None,
            },
            SelectOption {
                label: "Other".into(),
                description: "(type model name)".into(),
                value: "_other_".into(),
                preview: None,
            },
        ]);

        // Override model if user picked from list.
        let ollama_model_name = if ollama_model != "_other_" {
            ollama_model
        } else {
            eprint!("  Model name (e.g. qwen3:latest): ");
            let _ = std::io::stderr().flush();
            let mut m = String::new();
            let _ = std::io::stdin().read_line(&mut m);
            let m = m.trim().to_string();
            if m.is_empty() {
                "qwen3:latest".to_string()
            } else {
                m
            }
        };

        println!();
        println!("  {} Permission mode:\n", "3.".dark_cyan().bold());
        let pm = select(&[
            SelectOption {
                label: "Ask before changes".into(),
                description: "(recommended)".into(),
                value: "ask".into(),
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

        let result = SetupResult {
            api_key: "ollama".to_string(),
            provider: "ollama".to_string(),
            base_url: Some(default_url.to_string()),
            model: Some(ollama_model_name),
            theme: theme.clone(),
            permission_mode: pm,
        };
        write_config(&result);
        return Some(result);
    } else {
        // Cloud provider: check for existing key.
        let existing_key = std::env::var(env_var)
            .ok()
            .or_else(|| std::env::var("AGENT_CODE_API_KEY").ok());

        if let Some(ref key) = existing_key {
            let masked = if key.len() > 8 {
                format!("{}...{}", &key[..4], &key[key.len() - 4..])
            } else {
                "****".to_string()
            };
            println!("    {} found in env ({masked})", env_var.green());
        }

        // Always ask for API key — user can press Enter to keep the env key,
        // or paste a new one to override.
        let hint = if existing_key.is_some() {
            "  Paste API key (Enter to keep existing, or paste new): ".to_string()
        } else {
            format!("  Paste your {env_var}: ")
        };
        eprint!("{hint}");
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        let pasted = input.trim().to_string();

        let key = if pasted.is_empty() {
            if let Some(env_key) = existing_key {
                env_key
            } else {
                println!(
                    "    {}",
                    format!("Set {env_var} before running agent.").yellow()
                );
                String::new()
            }
        } else {
            pasted
        };
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

    let result = SetupResult {
        api_key,
        provider: provider_choice,
        base_url: Some(base_url),
        model: Some(model),
        theme,
        permission_mode,
    };
    write_config(&result);

    println!(
        "  {} Type {} to start.",
        "Ready!".green().bold(),
        "agent".bold(),
    );
    println!();

    Some(result)
}

/// Write config file from setup result.
pub fn write_config(result: &SetupResult) {
    let base_url = result
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com/v1");
    let model = result.model.as_deref().unwrap_or("gpt-5.4");
    // Include API key in config if provided (so it persists across sessions).
    let api_key_line = if !result.api_key.is_empty() && result.api_key != "ollama" {
        format!("api_key = \"{}\"\n", result.api_key)
    } else {
        String::new()
    };

    let config = format!(
        r#"[api]
base_url = "{base_url}"
model = "{model}"
{api_key_line}
[permissions]
default_mode = "{}"

[ui]
theme = "{}"
"#,
        result.permission_mode, result.theme,
    );

    if let Some(config_dir) = dirs::config_dir().map(|d| d.join("agent-code")) {
        let _ = std::fs::create_dir_all(&config_dir);
        let config_path = config_dir.join("config.toml");
        let _ = std::fs::write(&config_path, &config);
        println!(
            "{}",
            format!("  Config saved to {}", config_path.display()).dark_grey()
        );
    }
    println!();
}

pub struct SetupResult {
    pub api_key: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub theme: String,
    pub permission_mode: String,
}
