//! First-run setup wizard.
//!
//! Guides new users through initial configuration with arrow-key
//! navigable menus: theme, API provider, permission mode, and
//! a brief safety overview. Runs automatically on first launch
//! or when no API key is configured.

use std::io::Write;

use agent_code_lib::config::atomic::atomic_write_secret;
use crossterm::style::Stylize;

use super::selector::{SelectOption, select};

/// Check if the setup wizard should run.
pub fn needs_setup() -> bool {
    let config_path = agent_code_lib::config::agent_config_dir().map(|d| d.join("config.toml"));
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

/// Render the `config.toml` body for a setup result.
///
/// Built through the `toml` serializer rather than string formatting so
/// every value — most importantly the API key, which can legitimately
/// contain `\`, `"`, or other TOML metacharacters on OpenAI-compatible
/// "Other" endpoints — is escaped correctly. Hand-formatting silently
/// corrupted such keys (`\b` became a backspace, a `"` made the file
/// unparseable), so the key was effectively forgotten on the next
/// launch even though setup appeared to succeed (see issue #288).
fn render_config_toml(result: &SetupResult) -> String {
    let base_url = result
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com/v1");
    let model = result.model.as_deref().unwrap_or("gpt-5.4");

    let mut api = toml::value::Table::new();
    api.insert("base_url".into(), base_url.into());
    api.insert("model".into(), model.into());
    // Include the API key only when it's a real, persistable secret.
    // Ollama needs no key, and an empty key must not be written.
    if !result.api_key.is_empty() && result.api_key != "ollama" {
        api.insert("api_key".into(), result.api_key.clone().into());
    }

    let mut permissions = toml::value::Table::new();
    permissions.insert("default_mode".into(), result.permission_mode.clone().into());

    let mut ui = toml::value::Table::new();
    ui.insert("theme".into(), result.theme.clone().into());

    let mut root = toml::value::Table::new();
    root.insert("api".into(), toml::Value::Table(api));
    root.insert("permissions".into(), toml::Value::Table(permissions));
    root.insert("ui".into(), toml::Value::Table(ui));

    // Serializing a table of tables can't produce a value-after-table
    // error, so this only fails on a serializer bug — fall back to an
    // empty document so `write_config` still surfaces a save error
    // rather than panicking inside the wizard.
    toml::to_string_pretty(&toml::Value::Table(root)).unwrap_or_default()
}

/// Write config file from setup result.
///
/// Persists atomically with owner-only (`0600`) permissions — the file
/// holds the API key — and surfaces failures to the user instead of
/// swallowing them. A silent write failure used to masquerade as a
/// successful setup: the in-process key kept the current session
/// working while nothing reached disk, so the next launch re-ran the
/// wizard (issue #288).
pub fn write_config(result: &SetupResult) {
    let Some(config_dir) = agent_code_lib::config::agent_config_dir() else {
        println!(
            "  {}",
            "Could not determine a config directory — setup was not saved. \
             Set AGENT_CODE_API_KEY in your environment to use the agent."
                .yellow()
        );
        println!();
        return;
    };

    let config_path = config_dir.join("config.toml");
    let body = render_config_toml(result);

    match atomic_write_secret(&config_path, body.as_bytes()) {
        Ok(()) => {
            println!(
                "{}",
                format!("  Config saved to {}", config_path.display()).dark_grey()
            );
        }
        Err(e) => {
            println!(
                "  {}",
                format!(
                    "Could not save config to {} ({e}). \
                     Set AGENT_CODE_API_KEY in your environment to use the agent.",
                    config_path.display()
                )
                .yellow()
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn result_with_key(api_key: &str) -> SetupResult {
        SetupResult {
            api_key: api_key.to_string(),
            provider: "custom".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            model: Some("gpt-5.4".to_string()),
            theme: "midnight".to_string(),
            permission_mode: "ask".to_string(),
        }
    }

    /// Read the `api.api_key` field back out of a rendered config the
    /// same way `Config::load` would: parse the TOML, then index in.
    fn loaded_api_key(result: &SetupResult) -> Option<String> {
        let doc: toml::Value =
            toml::from_str(&render_config_toml(result)).expect("rendered config must parse");
        doc.get("api")
            .and_then(|a| a.get("api_key"))
            .and_then(|k| k.as_str())
            .map(str::to_string)
    }

    #[test]
    fn plain_key_round_trips() {
        let result = result_with_key("sk-normaltoken-123");
        assert_eq!(
            loaded_api_key(&result).as_deref(),
            Some("sk-normaltoken-123")
        );
    }

    /// Issue #288: a key with a backslash used to be mangled by the
    /// hand-rolled `format!` writer (`\b` decoded to a backspace), so
    /// the persisted key silently differed from what the user pasted.
    #[test]
    fn key_with_backslash_survives() {
        let result = result_with_key(r"sk-with\backslash");
        assert_eq!(
            loaded_api_key(&result).as_deref(),
            Some(r"sk-with\backslash")
        );
    }

    /// Issue #288: a key with a double quote used to make config.toml
    /// unparseable, so the next launch failed to load any key at all.
    #[test]
    fn key_with_quote_survives() {
        let result = result_with_key(r#"sk-with"quote"#);
        assert_eq!(loaded_api_key(&result).as_deref(), Some(r#"sk-with"quote"#));
    }

    #[test]
    fn other_sections_are_preserved() {
        let result = result_with_key("sk-abc");
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert_eq!(
            doc["api"]["base_url"].as_str(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(doc["api"]["model"].as_str(), Some("gpt-5.4"));
        assert_eq!(doc["permissions"]["default_mode"].as_str(), Some("ask"));
        assert_eq!(doc["ui"]["theme"].as_str(), Some("midnight"));
    }

    #[test]
    fn empty_key_is_omitted() {
        let result = result_with_key("");
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert!(
            doc["api"].get("api_key").is_none(),
            "an empty key must not be written"
        );
    }

    #[test]
    fn ollama_sentinel_key_is_omitted() {
        let result = result_with_key("ollama");
        let doc: toml::Value = toml::from_str(&render_config_toml(&result)).unwrap();
        assert!(
            doc["api"].get("api_key").is_none(),
            "the ollama sentinel must not be persisted as a key"
        );
    }
}
