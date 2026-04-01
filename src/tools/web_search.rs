//! WebSearch tool: search the web for information.

use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "WebSearch"
    }

    fn description(&self) -> &'static str {
        "Search the web for information using a search query."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 5)",
                    "default": 5
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
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'query' is required".into()))?;

        // Use a search API if configured, otherwise fall back to a
        // simple web fetch of a search engine results page.
        let encoded = urlencoded(query);
        let search_url = format!("https://html.duckduckgo.com/html/?q={encoded}");

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("agent-code/0.2")
            .build()
            .map_err(|e| ToolError::ExecutionFailed(format!("HTTP client error: {e}")))?;

        let response = tokio::select! {
            r = client.get(&search_url).send() => {
                r.map_err(|e| ToolError::ExecutionFailed(format!("Search failed: {e}")))?
            }
            _ = ctx.cancel.cancelled() => {
                return Err(ToolError::Cancelled);
            }
        };

        let body = response
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Read failed: {e}")))?;

        // Extract search results from HTML (simplified parsing).
        let results = extract_search_results(&body, 5);

        if results.is_empty() {
            Ok(ToolResult::success(format!(
                "No results found for: {query}"
            )))
        } else {
            let formatted: Vec<String> = results
                .iter()
                .enumerate()
                .map(|(i, r)| format!("{}. {}\n   {}", i + 1, r.title, r.snippet))
                .collect();
            Ok(ToolResult::success(format!(
                "Search results for: {query}\n\n{}",
                formatted.join("\n\n")
            )))
        }
    }
}

struct SearchResult {
    title: String,
    snippet: String,
}

fn extract_search_results(html: &str, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Simple extraction: look for result link patterns in DuckDuckGo HTML.
    for segment in html.split("class=\"result__a\"").skip(1).take(max) {
        let title = segment
            .split('>')
            .nth(1)
            .and_then(|s| s.split('<').next())
            .unwrap_or("")
            .trim()
            .to_string();

        let snippet = segment
            .split("class=\"result__snippet\"")
            .nth(1)
            .and_then(|s| s.split('>').nth(1))
            .and_then(|s| s.split('<').next())
            .unwrap_or("")
            .trim()
            .to_string();

        if !title.is_empty() {
            results.push(SearchResult { title, snippet });
        }
    }

    results
}

fn urlencoded(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => '+'.to_string(),
            c if c.is_alphanumeric() || "-_.~".contains(c) => c.to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
