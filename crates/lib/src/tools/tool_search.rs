//! ToolSearch tool: discover deferred tools by keyword.
//!
//! Allows the agent to search for tools that aren't loaded by default.
//! Supports direct selection (`select:ToolName`) and keyword search.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct ToolSearchTool;

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &'static str {
        "ToolSearch"
    }

    fn description(&self) -> &'static str {
        "Search for available tools by name or keyword. Use 'select:Name' \
         for direct lookup, or keywords to search descriptions."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Query: 'select:ToolName' for direct lookup, or keywords"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results to return (default: 5)",
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
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'query' is required".into()))?;

        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        // For now, search against the currently registered tools.
        // In a full implementation, this would also search deferred/MCP tools.
        let query_lower = query.to_lowercase();

        // Direct selection: select:Name,Name2
        if let Some(names) = query_lower.strip_prefix("select:") {
            let requested: Vec<&str> = names.split(',').map(|s| s.trim()).collect();
            let mut results = Vec::new();
            for name in &requested {
                results.push(format!("Tool '{}' — use it directly by name", name));
            }
            return Ok(ToolResult::success(format!(
                "Selected {} tool(s):\n{}",
                results.len(),
                results.join("\n")
            )));
        }

        // Keyword search against tool names and descriptions.
        let terms: Vec<&str> = query_lower.split_whitespace().collect();
        let mut matches: Vec<(String, String, usize)> = Vec::new();

        // Search built-in tools (in a full implementation, this would
        // also search deferred tools that aren't currently loaded).
        let tool_info = vec![
            ("Bash", "Execute shell commands"),
            ("FileRead", "Read file contents with line numbers"),
            ("FileWrite", "Write or create files"),
            ("FileEdit", "Search-and-replace editing in files"),
            ("Grep", "Regex content search powered by ripgrep"),
            ("Glob", "Find files matching glob patterns"),
            ("Agent", "Spawn subagents for parallel tasks"),
            ("WebFetch", "Fetch content from URLs"),
            ("AskUserQuestion", "Ask the user interactive questions"),
            ("NotebookEdit", "Edit Jupyter notebook cells"),
            ("ToolSearch", "Search for available tools"),
        ];

        for (name, desc) in &tool_info {
            let name_lower = name.to_lowercase();
            let desc_lower = desc.to_lowercase();
            let mut score = 0usize;

            for term in &terms {
                if name_lower.contains(term) {
                    score += 10;
                }
                if desc_lower.contains(term) {
                    score += 2;
                }
            }

            if score > 0 {
                matches.push((name.to_string(), desc.to_string(), score));
            }
        }

        matches.sort_by(|a, b| b.2.cmp(&a.2));
        matches.truncate(max_results);

        if matches.is_empty() {
            Ok(ToolResult::success(format!(
                "No tools found matching '{query}'. Available tools: {}",
                tool_info
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        } else {
            let results: Vec<String> = matches
                .iter()
                .map(|(name, desc, _)| format!("- {name}: {desc}"))
                .collect();
            Ok(ToolResult::success(format!(
                "Found {} tool(s):\n{}",
                results.len(),
                results.join("\n")
            )))
        }
    }
}
