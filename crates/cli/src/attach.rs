//! Attach to a running agent-code serve instance.
//!
//! Discovers running instances via bridge lock files, connects via HTTP,
//! and provides an interactive REPL that sends prompts to the remote
//! agent and displays responses.

use std::io::Write;

/// Attach to a running serve instance and enter an interactive loop.
pub async fn run_attach(port: u16) -> anyhow::Result<()> {
    // Discover or use specified port.
    let target_port = if port != 4096 {
        // User specified a port explicitly.
        port
    } else {
        // Try to discover a running instance.
        let bridges = agent_code_lib::services::bridge::discover_bridges();
        if bridges.is_empty() {
            eprintln!("No running agent-code instances found.");
            eprintln!("Start one with: agent --serve");
            return Ok(());
        }
        if bridges.len() == 1 {
            let b = &bridges[0];
            eprintln!(
                "Attaching to instance on port {} (pid {}, {})",
                b.port, b.pid, b.cwd
            );
            b.port
        } else {
            eprintln!("Multiple instances found:\n");
            for (i, b) in bridges.iter().enumerate() {
                eprintln!("  [{}] port {} — pid {} — {}", i + 1, b.port, b.pid, b.cwd);
            }
            eprintln!("\nUse --port <N> to connect to a specific one.");
            return Ok(());
        }
    };

    let base_url = format!("http://127.0.0.1:{target_port}");

    // Verify the instance is reachable.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    match client.get(format!("{base_url}/health")).send().await {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            eprintln!("Instance returned HTTP {}", resp.status());
            return Ok(());
        }
        Err(e) => {
            eprintln!("Cannot connect to {base_url}: {e}");
            eprintln!("Is the server running? Start with: agent --serve --port {target_port}");
            return Ok(());
        }
    }

    // Get initial status.
    if let Ok(resp) = client.get(format!("{base_url}/status")).send().await
        && let Ok(status) = resp.json::<serde_json::Value>().await
    {
        let model = status["model"].as_str().unwrap_or("unknown");
        let turns = status["turn_count"].as_u64().unwrap_or(0);
        let cost = status["cost_usd"].as_f64().unwrap_or(0.0);
        eprintln!("Connected. Model: {model}, turns: {turns}, cost: ${cost:.4}\n");
    }

    eprintln!("Type a message and press Enter. Ctrl+D to detach.\n");

    // Simple line-based REPL.
    let stdin = std::io::stdin();
    let mut input = String::new();

    loop {
        print!("> ");
        std::io::stdout().flush()?;

        input.clear();
        if stdin.read_line(&mut input)? == 0 {
            // EOF (Ctrl+D).
            eprintln!("\nDetached.");
            break;
        }

        let prompt = input.trim();
        if prompt.is_empty() {
            continue;
        }

        // Send prompt to the server.
        let body = serde_json::json!({"content": prompt});
        match client
            .post(format!("{base_url}/message"))
            .json(&body)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(result) = resp.json::<serde_json::Value>().await {
                    let response = result["response"].as_str().unwrap_or("");
                    let tools: Vec<&str> = result["tools_used"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();
                    let cost = result["cost_usd"].as_f64().unwrap_or(0.0);

                    println!("{response}");
                    if !tools.is_empty() {
                        eprintln!("[tools: {} | cost: ${cost:.4}]", tools.join(", "));
                    }
                    println!();
                } else {
                    eprintln!("[Error: could not parse response]");
                }
            }
            Err(e) => {
                eprintln!("[Error: {e}]");
            }
        }
    }

    Ok(())
}
