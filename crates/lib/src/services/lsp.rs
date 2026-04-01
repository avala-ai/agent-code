//! Language Server Protocol integration.
//!
//! Connects to LSP servers to provide diagnostics (errors, warnings),
//! symbol information, and code intelligence. Communicates via JSON-RPC
//! over stdio, similar to the MCP transport.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::debug;

/// An LSP client connection to a language server.
pub struct LspClient {
    name: String,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    child: Mutex<tokio::process::Child>,
    next_id: Mutex<u64>,
    root_uri: String,
}

/// A diagnostic reported by the language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl LspClient {
    /// Spawn and initialize an LSP server.
    pub async fn start(
        name: &str,
        command: &str,
        args: &[String],
        root_path: &Path,
    ) -> Result<Self, String> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start LSP server '{name}': {e}"))?;

        let stdin = child.stdin.take().ok_or("No stdin")?;
        let stdout = child.stdout.take().ok_or("No stdout")?;

        let root_uri = format!("file://{}", root_path.display());

        let client = Self {
            name: name.to_string(),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            child: Mutex::new(child),
            next_id: Mutex::new(1),
            root_uri: root_uri.clone(),
        };

        // Send initialize request.
        let init_result = client
            .request(
                "initialize",
                serde_json::json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "capabilities": {
                        "textDocument": {
                            "publishDiagnostics": {
                                "relatedInformation": true
                            }
                        }
                    }
                }),
            )
            .await?;

        debug!("LSP '{name}' initialized: {:?}", init_result);

        // Send initialized notification.
        client.notify("initialized", serde_json::json!({})).await?;

        Ok(client)
    }

    /// Send a JSON-RPC request and read the response.
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let body_str = serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?;

        let message = format!("Content-Length: {}\r\n\r\n{}", body_str.len(), body_str);

        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(message.as_bytes())
                .await
                .map_err(|e| format!("Write error: {e}"))?;
            stdin
                .flush()
                .await
                .map_err(|e| format!("Flush error: {e}"))?;
        }

        // Read response (Content-Length header + body).
        let mut stdout = self.stdout.lock().await;
        let content_length = read_content_length(&mut stdout).await?;

        let mut buf = vec![0u8; content_length];
        tokio::io::AsyncReadExt::read_exact(&mut *stdout, &mut buf)
            .await
            .map_err(|e| format!("Read error: {e}"))?;

        let response: serde_json::Value =
            serde_json::from_slice(&buf).map_err(|e| format!("Parse error: {e}"))?;

        if let Some(error) = response.get("error") {
            return Err(format!("LSP error: {error}"));
        }

        Ok(response.get("result").cloned().unwrap_or_default())
    }

    /// Send a notification (no response expected).
    async fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let body_str = serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?;

        let message = format!("Content-Length: {}\r\n\r\n{}", body_str.len(), body_str);

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(message.as_bytes())
            .await
            .map_err(|e| format!("Write error: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("Flush error: {e}"))?;

        Ok(())
    }

    /// Get diagnostics for a file by opening it and waiting for the server.
    pub async fn get_diagnostics(&self, file_path: &PathBuf) -> Result<Vec<Diagnostic>, String> {
        let uri = format!("file://{}", file_path.display());
        let content = std::fs::read_to_string(file_path).map_err(|e| format!("Read error: {e}"))?;

        // Notify the server that we opened the file.
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": detect_language(file_path),
                    "version": 1,
                    "text": content,
                }
            }),
        )
        .await?;

        // Give the server a moment to analyze.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Diagnostics come via notifications; for now return empty.
        // Full implementation would read from a notification stream.
        Ok(Vec::new())
    }

    /// Shut down the LSP server.
    pub async fn shutdown(&self) {
        let _ = self.request("shutdown", serde_json::json!(null)).await;
        let _ = self.notify("exit", serde_json::json!(null)).await;
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }
}

/// Read the Content-Length header from an LSP message.
async fn read_content_length(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> Result<usize, String> {
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Header read error: {e}"))?;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err("Empty header line without Content-Length".to_string());
        }

        if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
            let length: usize = len_str
                .parse()
                .map_err(|e| format!("Invalid Content-Length: {e}"))?;

            // Read the empty line after headers.
            let mut empty = String::new();
            reader
                .read_line(&mut empty)
                .await
                .map_err(|e| format!("Header separator read error: {e}"))?;

            return Ok(length);
        }
    }
}

/// Detect language ID from file extension.
fn detect_language(path: &Path) -> &str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("js") => "javascript",
        Some("ts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("jsx") => "javascriptreact",
        Some("go") => "go",
        Some("java") => "java",
        Some("rb") => "ruby",
        Some("c" | "h") => "c",
        Some("cpp" | "cc" | "cxx" | "hpp") => "cpp",
        Some("cs") => "csharp",
        Some("swift") => "swift",
        Some("kt") => "kotlin",
        Some("json") => "json",
        Some("yaml" | "yml") => "yaml",
        Some("toml") => "toml",
        Some("md") => "markdown",
        Some("html") => "html",
        Some("css") => "css",
        Some("sh" | "bash") => "shellscript",
        _ => "plaintext",
    }
}
