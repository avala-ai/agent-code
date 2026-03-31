//! WebFetch tool: fetch content from URLs.
//!
//! Makes HTTP GET requests to URLs and returns the content.
//! Converts HTML to plain text for readability.

use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Maximum content size to return (100KB).
const MAX_CONTENT_SIZE: usize = 100_000;

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "WebFetch"
    }

    fn description(&self) -> &'static str {
        "Fetches content from a URL. Returns the page content as text."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt to apply to the fetched content"
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

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let url = input
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'url' is required".into()))?;

        // Validate URL.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::InvalidInput(
                "URL must start with http:// or https://".into(),
            ));
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("rc/0.1")
            .build()
            .map_err(|e| ToolError::ExecutionFailed(format!("HTTP client error: {e}")))?;

        let start = std::time::Instant::now();

        let response = tokio::select! {
            r = client.get(url).send() => {
                r.map_err(|e| ToolError::ExecutionFailed(format!("Fetch failed: {e}")))?
            }
            _ = ctx.cancel.cancelled() => {
                return Err(ToolError::Cancelled);
            }
        };

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = response
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read body: {e}")))?;

        let duration_ms = start.elapsed().as_millis();

        // Convert HTML to plain text (simple tag stripping).
        let text = if content_type.contains("html") {
            strip_html_tags(&body)
        } else {
            body.clone()
        };

        // Truncate if needed.
        let truncated = text.len() > MAX_CONTENT_SIZE;
        let content = if truncated {
            format!(
                "{}\n\n(Content truncated from {} to {} chars)",
                &text[..MAX_CONTENT_SIZE],
                text.len(),
                MAX_CONTENT_SIZE
            )
        } else {
            text
        };

        let result = format!(
            "URL: {url}\nStatus: {status}\nContent-Type: {content_type}\n\
             Size: {} bytes\nFetch time: {duration_ms}ms\n\n{content}",
            body.len()
        );

        Ok(ToolResult {
            content: result,
            is_error: !status.is_success(),
        })
    }
}

/// Simple HTML tag stripping. Removes tags and decodes common entities.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        if !in_tag && chars[i] == '<' {
            // Check for script/style start.
            let remaining: String = lower_chars[i..].iter().take(20).collect();
            if remaining.starts_with("<script") {
                in_script = true;
            } else if remaining.starts_with("<style") {
                in_style = true;
            } else if remaining.starts_with("</script") {
                in_script = false;
            } else if remaining.starts_with("</style") {
                in_style = false;
            }
            in_tag = true;
            i += 1;
            continue;
        }

        if in_tag && chars[i] == '>' {
            in_tag = false;
            i += 1;
            // Add newline after block elements.
            continue;
        }

        if !in_tag && !in_script && !in_style {
            // Decode common HTML entities.
            if chars[i] == '&' {
                let entity: String = chars[i..].iter().take(10).collect();
                if entity.starts_with("&amp;") {
                    result.push('&');
                    i += 5;
                    continue;
                } else if entity.starts_with("&lt;") {
                    result.push('<');
                    i += 4;
                    continue;
                } else if entity.starts_with("&gt;") {
                    result.push('>');
                    i += 4;
                    continue;
                } else if entity.starts_with("&quot;") {
                    result.push('"');
                    i += 6;
                    continue;
                } else if entity.starts_with("&nbsp;") {
                    result.push(' ');
                    i += 6;
                    continue;
                }
            }
            result.push(chars[i]);
        }

        i += 1;
    }

    // Collapse multiple whitespace/newlines.
    let mut collapsed = String::with_capacity(result.len());
    let mut last_was_newline = false;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !last_was_newline {
                collapsed.push('\n');
                last_was_newline = true;
            }
        } else {
            collapsed.push_str(trimmed);
            collapsed.push('\n');
            last_was_newline = false;
        }
    }

    collapsed.trim().to_string()
}
