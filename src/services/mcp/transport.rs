//! MCP transport layer.
//!
//! Handles the low-level communication with MCP servers over
//! stdio (subprocess) or SSE (HTTP).

use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::types::*;

/// A transport connection to an MCP server.
pub struct McpTransportConnection {
    inner: TransportInner,
    next_id: Mutex<u64>,
}

enum TransportInner {
    Stdio {
        child: Mutex<Child>,
        stdin: Mutex<tokio::process::ChildStdin>,
        stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    },
    // SSE transport can be added later.
}

impl McpTransportConnection {
    /// Connect to an MCP server via stdio subprocess.
    pub async fn connect_stdio(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server '{command}': {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to capture stdin".to_string())?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture stdout".to_string())?;

        Ok(Self {
            inner: TransportInner::Stdio {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                stdout: Mutex::new(BufReader::new(stdout)),
            },
            next_id: Mutex::new(1),
        })
    }

    /// Send a JSON-RPC request and wait for the response.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };

        let request = JsonRpcRequest::new(id, method, params);
        let request_json = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize request: {e}"))?;

        debug!("MCP request: {method} (id={id})");

        match &self.inner {
            TransportInner::Stdio { stdin, stdout, .. } => {
                // Write the request.
                {
                    let mut stdin = stdin.lock().await;
                    stdin
                        .write_all(request_json.as_bytes())
                        .await
                        .map_err(|e| format!("Failed to write to MCP server: {e}"))?;
                    stdin
                        .write_all(b"\n")
                        .await
                        .map_err(|e| format!("Failed to write newline: {e}"))?;
                    stdin
                        .flush()
                        .await
                        .map_err(|e| format!("Failed to flush: {e}"))?;
                }

                // Read the response.
                let mut line = String::new();
                {
                    let mut stdout = stdout.lock().await;
                    stdout
                        .read_line(&mut line)
                        .await
                        .map_err(|e| format!("Failed to read from MCP server: {e}"))?;
                }

                if line.is_empty() {
                    return Err("MCP server closed connection".to_string());
                }

                let response: JsonRpcResponse = serde_json::from_str(&line)
                    .map_err(|e| format!("Invalid JSON-RPC response: {e}"))?;

                if let Some(error) = response.error {
                    return Err(format!(
                        "MCP error ({}): {}",
                        error.code, error.message
                    ));
                }

                response
                    .result
                    .ok_or_else(|| "MCP response missing 'result'".to_string())
            }
        }
    }

    /// Send a notification (no response expected).
    pub async fn notify(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let json = serde_json::to_string(&notification)
            .map_err(|e| format!("Failed to serialize notification: {e}"))?;

        match &self.inner {
            TransportInner::Stdio { stdin, .. } => {
                let mut stdin = stdin.lock().await;
                stdin
                    .write_all(json.as_bytes())
                    .await
                    .map_err(|e| format!("Failed to write notification: {e}"))?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|e| format!("Failed to write newline: {e}"))?;
                stdin.flush().await.map_err(|e| format!("Flush failed: {e}"))?;
            }
        }

        Ok(())
    }

    /// Shut down the transport connection.
    pub async fn shutdown(&self) {
        match &self.inner {
            TransportInner::Stdio { child, .. } => {
                let mut child = child.lock().await;
                let _ = child.kill().await;
            }
        }
    }
}
