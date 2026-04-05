//! Attach to a running agent-code serve instance.
//!
//! Discovers running instances via bridge lock files, connects via HTTP,
//! and provides an interactive REPL that sends prompts to the remote
//! agent and displays responses.
//!
//! Supports connecting by:
//! - Auto-discovery (single instance)
//! - Session ID prefix (`--attach abc123`)
//! - Explicit port (`--attach --port 8080`)
//! - Interactive selection (multiple instances)

use std::io::Write;

/// Attach to a running serve instance and enter an interactive loop.
///
/// `session_filter` is an optional session ID prefix. When non-empty,
/// the attach command queries each discovered instance's `/status`
/// endpoint and connects to the one whose session ID starts with the
/// given prefix.
pub async fn run_attach(port: u16, session_filter: &str) -> anyhow::Result<()> {
    let target_port = if port != 4096 {
        // User specified a port explicitly.
        port
    } else if !session_filter.is_empty() {
        // Find the instance matching the session ID prefix.
        match find_by_session_id(session_filter).await {
            Some(p) => p,
            None => {
                eprintln!("No running instance matches session '{session_filter}'.");
                eprintln!("Run /status in a serve session to see its session ID.");
                return Ok(());
            }
        }
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
            // Interactive selection.
            eprintln!("Multiple instances found:\n");
            for (i, b) in bridges.iter().enumerate() {
                // Fetch session ID for display.
                let session = fetch_session_id(b.port).await.unwrap_or_default();
                let session_display = if session.is_empty() {
                    String::new()
                } else {
                    format!(" — session {session}")
                };
                eprintln!(
                    "  [{}] port {} — pid {} — {}{}",
                    i + 1,
                    b.port,
                    b.pid,
                    b.cwd,
                    session_display
                );
            }
            eprintln!();

            // Read selection from stdin.
            eprint!("Select [1-{}]: ", bridges.len());
            std::io::stderr().flush()?;
            let mut choice = String::new();
            std::io::stdin().read_line(&mut choice)?;
            let idx: usize = choice.trim().parse().unwrap_or(0);
            if idx < 1 || idx > bridges.len() {
                eprintln!("Invalid selection.");
                return Ok(());
            }
            let b = &bridges[idx - 1];
            eprintln!("Attaching to instance on port {} (pid {})", b.port, b.pid);
            b.port
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
        let session = status["session_id"].as_str().unwrap_or("?");
        let turns = status["turn_count"].as_u64().unwrap_or(0);
        let cost = status["cost_usd"].as_f64().unwrap_or(0.0);
        eprintln!(
            "Connected. Session: {session}, model: {model}, turns: {turns}, cost: ${cost:.4}\n"
        );
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

/// Find a running instance by session ID prefix.
async fn find_by_session_id(prefix: &str) -> Option<u16> {
    let bridges = agent_code_lib::services::bridge::discover_bridges();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;

    for b in &bridges {
        if let Some(session_id) = fetch_session_id_with_client(&client, b.port).await
            && session_id.starts_with(prefix)
        {
            eprintln!(
                "Found session {session_id} on port {} (pid {}, {})",
                b.port, b.pid, b.cwd
            );
            return Some(b.port);
        }
    }
    None
}

/// Fetch the session ID from a running instance.
async fn fetch_session_id(port: u16) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()?;
    fetch_session_id_with_client(&client, port).await
}

async fn fetch_session_id_with_client(client: &reqwest::Client, port: u16) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}/status");
    let resp = client.get(&url).send().await.ok()?;
    let status: serde_json::Value = resp.json().await.ok()?;
    status["session_id"].as_str().map(|s| s.to_string())
}
