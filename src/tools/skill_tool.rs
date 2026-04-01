//! Skill tool: invoke skills dynamically from the agent.
//!
//! Unlike slash commands which are user-initiated, the SkillTool
//! lets the LLM trigger skills programmatically when it determines
//! one is appropriate for the current task.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct SkillTool;

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "Skill"
    }

    fn description(&self) -> &'static str {
        "Invoke a user-defined skill by name. Skills are reusable \
         workflows loaded from .agent/skills/ or ~/.config/agent-code/skills/."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["skill"],
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Name of the skill to invoke"
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments passed to the skill template"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true // The skill itself may invoke mutation tools, but loading is read-only.
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let skill_name = input
            .get("skill")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'skill' is required".into()))?;

        let args = input.get("args").and_then(|v| v.as_str());

        let registry = crate::skills::SkillRegistry::load_all(Some(ctx.cwd.as_path()));

        match registry.find(skill_name) {
            Some(skill) => {
                let expanded = skill.expand(args);
                Ok(ToolResult::success(format!(
                    "Skill '{}' loaded. Execute the following instructions:\n\n{}",
                    skill_name, expanded
                )))
            }
            None => {
                let available: Vec<&str> = registry.all().iter().map(|s| s.name.as_str()).collect();
                Err(ToolError::InvalidInput(format!(
                    "Skill '{}' not found. Available: {}",
                    skill_name,
                    if available.is_empty() {
                        "none".to_string()
                    } else {
                        available.join(", ")
                    }
                )))
            }
        }
    }
}
