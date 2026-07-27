//! CronDelete tool: remove a stored routine by id.

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::permissions::{PermissionChecker, PermissionDecision};

use super::cron_support::open_store;

pub struct CronDeleteTool;

#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &'static str {
        "CronDelete"
    }

    fn description(&self) -> &'static str {
        "Delete a stored cron routine by id. Returns {deleted: true} on success."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Routine id (the value returned by CronCreate or shown in CronList)."
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        true
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        checker: &PermissionChecker,
    ) -> PermissionDecision {
        checker.check(self.name(), input)
    }

    /// Pinned to the routine this id names right now — see
    /// [`super::cron_support::routine_grant_binding`].
    fn grant_binding(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Option<super::GrantBinding> {
        super::cron_support::routine_grant_binding(input)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        let store = open_store().map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to open schedule store: {e}"))
        })?;

        match store.remove(id) {
            Ok(()) => {
                let body = serde_json::to_string_pretty(&json!({
                    "id": id,
                    "deleted": true,
                }))
                .unwrap_or_else(|_| format!("Deleted routine '{id}'"));
                Ok(ToolResult::success(body))
            }
            Err(e) => {
                // Treat "not found" as a structured non-error result so
                // the model can react idempotently.
                if e.contains("not found") {
                    let body = serde_json::to_string_pretty(&json!({
                        "id": id,
                        "deleted": false,
                        "reason": "not_found",
                    }))
                    .unwrap_or_else(|_| format!("Routine '{id}' not found"));
                    Ok(ToolResult::success(body))
                } else {
                    Err(ToolError::ExecutionFailed(format!(
                        "Failed to delete routine '{id}': {e}"
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::Schedule;
    use crate::tools::cron_support::{test_ctx, with_test_store};
    use chrono::Utc;

    fn fixture(name: &str) -> Schedule {
        Schedule {
            name: name.to_string(),
            cron: "0 9 * * *".to_string(),
            prompt: "x".to_string(),
            cwd: ".".to_string(),
            enabled: true,
            model: None,
            permission_mode: None,
            max_cost_usd: None,
            max_turns: None,
            created_at: Utc::now(),
            last_run_at: None,
            last_result: None,
            webhook_secret: None,
        }
    }

    #[tokio::test]
    async fn delete_removes_existing_routine() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        store.save(&fixture("doomed")).unwrap();

        let res = CronDeleteTool
            .call(json!({"id": "doomed"}), &test_ctx())
            .await
            .unwrap();
        assert!(!res.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(parsed["deleted"].as_bool(), Some(true));
        assert!(open_store().unwrap().load("doomed").is_err());
    }

    #[tokio::test]
    async fn delete_missing_routine_returns_not_found() {
        let _guard = with_test_store();
        let res = CronDeleteTool
            .call(json!({"id": "ghost"}), &test_ctx())
            .await
            .unwrap();
        assert!(!res.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(parsed["deleted"].as_bool(), Some(false));
        assert_eq!(parsed["reason"].as_str(), Some("not_found"));
    }

    #[tokio::test]
    async fn delete_requires_id() {
        let _guard = with_test_store();
        let err = CronDeleteTool
            .call(json!({}), &test_ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
