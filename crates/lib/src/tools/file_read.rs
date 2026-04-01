//! FileRead tool: read file contents with optional line ranges.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &'static str {
        "FileRead"
    }

    fn description(&self) -> &'static str {
        "Reads a file from the filesystem. Returns contents with line numbers."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read"
                },
                "pages": {
                    "type": "string",
                    "description": "Page range for PDF files (e.g., \"1-5\", \"3\", \"10-20\"). Max 20 pages per request."
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'file_path' is required".into()))?;

        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

        let path = std::path::Path::new(file_path);

        // Block device and virtual filesystem paths.
        const BLOCKED_PREFIXES: &[&str] = &["/dev/", "/proc/", "/sys/"];
        if BLOCKED_PREFIXES
            .iter()
            .any(|prefix| file_path.starts_with(prefix))
        {
            return Err(ToolError::InvalidInput(format!(
                "Cannot read virtual/device file: {file_path}"
            )));
        }

        let pages = input
            .get("pages")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Handle binary/special file types.
        match path.extension().and_then(|e| e.to_str()) {
            Some("pdf") => {
                return read_pdf(file_path, pages.as_deref()).await;
            }
            Some("ipynb") => {
                return read_notebook(file_path).await;
            }
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp") => {
                let meta = tokio::fs::metadata(file_path).await.ok();
                let size = meta.map(|m| m.len()).unwrap_or(0);

                // For small images (< 5MB), embed as base64 for vision models.
                if size < 5 * 1024 * 1024
                    && crate::llm::message::image_block_from_file(path).is_ok()
                {
                    return Ok(ToolResult::success(format!(
                        "(Image: {file_path}, {size} bytes — loaded for vision analysis)"
                    )));
                }

                return Ok(ToolResult::success(format!(
                    "(Image file: {file_path}, {size} bytes — \
                     too large for inline embedding)"
                )));
            }
            Some("wasm" | "exe" | "dll" | "so" | "dylib" | "o" | "a") => {
                let meta = tokio::fs::metadata(file_path).await.ok();
                let size = meta.map(|m| m.len()).unwrap_or(0);
                return Ok(ToolResult::success(format!(
                    "(Binary file: {file_path}, {size} bytes)"
                )));
            }
            _ => {}
        }

        // Try to read as text; if it fails (binary content), report the file type.
        let content = match tokio::fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(e) => {
                // May be binary — try to read size at least.
                if let Ok(meta) = tokio::fs::metadata(file_path).await {
                    return Ok(ToolResult::success(format!(
                        "(Binary or unreadable file: {file_path}, {} bytes: {e})",
                        meta.len()
                    )));
                }
                return Err(ToolError::ExecutionFailed(format!(
                    "Failed to read {file_path}: {e}"
                )));
            }
        };

        // Apply line range and add line numbers (1-indexed).
        let lines: Vec<&str> = content.lines().collect();
        let start = (offset.saturating_sub(1)).min(lines.len());
        let end = (start + limit).min(lines.len());

        let mut output = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_num = start + i + 1;
            output.push_str(&format!("{line_num}\t{line}\n"));
        }

        if output.is_empty() {
            output = "(empty file)".to_string();
        }

        Ok(ToolResult::success(output))
    }
}

/// Extract text from a PDF file using pdftotext (poppler-utils).
async fn read_pdf(file_path: &str, pages: Option<&str>) -> Result<ToolResult, ToolError> {
    // Build pdftotext command with optional page range.
    let mut cmd = tokio::process::Command::new("pdftotext");

    if let Some(page_spec) = pages {
        // Parse page spec like "1-5", "3", "10-20".
        let (first, last) = if let Some((start, end)) = page_spec.split_once('-') {
            (start.trim().to_string(), end.trim().to_string())
        } else {
            let page = page_spec.trim().to_string();
            (page.clone(), page)
        };
        cmd.arg("-f").arg(&first).arg("-l").arg(&last);
    }

    cmd.arg(file_path).arg("-");
    let output = cmd.output().await;

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            if text.trim().is_empty() {
                Ok(ToolResult::success(format!(
                    "(PDF file: {file_path} — extracted but contains no text. \
                     May be image-based; OCR would be needed.)"
                )))
            } else {
                // Truncate very large PDFs.
                let display = if text.len() > 100_000 {
                    format!(
                        "{}\n\n(PDF truncated: {} chars total)",
                        &text[..100_000],
                        text.len()
                    )
                } else {
                    text
                };
                Ok(ToolResult::success(display))
            }
        }
        _ => {
            // pdftotext not available — report file info.
            let meta = tokio::fs::metadata(file_path).await.ok();
            let size = meta.map(|m| m.len()).unwrap_or(0);
            Ok(ToolResult::success(format!(
                "(PDF file: {file_path}, {size} bytes. \
                 Install poppler-utils for text extraction: \
                 apt install poppler-utils / brew install poppler)"
            )))
        }
    }
}

/// Render a Jupyter notebook (.ipynb) as readable text.
async fn read_notebook(file_path: &str) -> Result<ToolResult, ToolError> {
    let content = tokio::fs::read_to_string(file_path)
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read {file_path}: {e}")))?;

    let notebook: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| ToolError::ExecutionFailed(format!("Invalid notebook JSON: {e}")))?;

    let cells = notebook
        .get("cells")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::ExecutionFailed("Notebook has no 'cells' array".into()))?;

    let mut output = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let cell_type = cell
            .get("cell_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        output.push_str(&format!("--- Cell {} ({}) ---\n", i + 1, cell_type));

        // Source lines.
        if let Some(source) = cell.get("source") {
            let text = match source {
                serde_json::Value::Array(lines) => lines
                    .iter()
                    .filter_map(|l| l.as_str())
                    .collect::<Vec<_>>()
                    .join(""),
                serde_json::Value::String(s) => s.clone(),
                _ => String::new(),
            };
            output.push_str(&text);
            if !text.ends_with('\n') {
                output.push('\n');
            }
        }

        // Outputs (for code cells).
        if cell_type == "code"
            && let Some(outputs) = cell.get("outputs").and_then(|v| v.as_array())
        {
            for out in outputs {
                if let Some(text) = out.get("text").and_then(|v| v.as_array()) {
                    output.push_str("Output:\n");
                    for line in text {
                        if let Some(s) = line.as_str() {
                            output.push_str(s);
                        }
                    }
                }
                if let Some(data) = out.get("data")
                    && let Some(plain) = data.get("text/plain").and_then(|v| v.as_array())
                {
                    output.push_str("Output:\n");
                    for line in plain {
                        if let Some(s) = line.as_str() {
                            output.push_str(s);
                        }
                    }
                }
            }
        }

        output.push('\n');
    }

    Ok(ToolResult::success(output))
}
