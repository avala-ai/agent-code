//! AskUserQuestion tool: interactive prompts during execution.
//!
//! Allows the agent to ask the user questions and collect structured
//! responses. Used for gathering preferences, clarifying ambiguity,
//! and making implementation decisions.

use async_trait::async_trait;
use serde_json::json;
use std::io::Write;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &'static str {
        "AskUserQuestion"
    }

    fn description(&self) -> &'static str {
        "Ask the user a question to gather preferences or clarify requirements. \
         Present 2-4 options for the user to choose from."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["questions"],
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "Questions to ask (1-4)",
                    "items": {
                        "type": "object",
                        "required": ["question", "options"],
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The question to ask"
                            },
                            "options": {
                                "type": "array",
                                "description": "Available choices (2-4)",
                                "items": {
                                    "type": "object",
                                    "required": ["label", "description"],
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Short choice label"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "What this choice means"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        false // Requires user interaction.
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let questions = input
            .get("questions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidInput("'questions' array is required".into()))?;

        let mut answers = Vec::new();

        for q in questions {
            let question_text = q
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("?");

            let options = q
                .get("options")
                .and_then(|v| v.as_array())
                .ok_or_else(|| ToolError::InvalidInput("'options' array required".into()))?;

            // Display the question.
            eprintln!("\n{question_text}");
            for (i, opt) in options.iter().enumerate() {
                let label = opt.get("label").and_then(|v| v.as_str()).unwrap_or("?");
                let desc = opt
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let letter = (b'A' + i as u8) as char;
                eprintln!("  {letter}) {label} — {desc}");
            }
            eprint!("Choice: ");
            let _ = std::io::stderr().flush();

            // Read user input from stdin.
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read input: {e}")))?;

            let choice = line.trim().to_uppercase();

            // Map letter to option label.
            let selected = if choice.len() == 1 {
                let idx = choice.as_bytes()[0].wrapping_sub(b'A') as usize;
                if idx < options.len() {
                    options[idx]
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&choice)
                        .to_string()
                } else {
                    choice
                }
            } else {
                choice
            };

            answers.push(format!("{question_text}={selected}"));
        }

        let result = format!(
            "User has answered your questions: {}. You can now continue with the user's answers in mind.",
            answers
                .iter()
                .map(|a| format!("\"{a}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );

        Ok(ToolResult::success(result))
    }
}
