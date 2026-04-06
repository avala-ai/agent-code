//! Schedule execution engine.
//!
//! Runs the daemon loop that checks schedules every 30 seconds and
//! executes matching jobs. Also provides one-shot execution for
//! `agent schedule run <name>`.

use std::sync::Arc;

use chrono::{Timelike, Utc};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::llm::provider::Provider;
use crate::permissions::PermissionChecker;
use crate::query::{QueryEngine, QueryEngineConfig, StreamSink};
use crate::state::AppState;
use crate::tools::registry::ToolRegistry;

use super::cron::CronExpr;
use super::storage::{RunResult, Schedule, ScheduleStore};

/// Outcome of a single scheduled run.
#[derive(Debug)]
pub struct JobOutcome {
    pub schedule_name: String,
    pub success: bool,
    pub turns: usize,
    pub cost_usd: f64,
    pub response_summary: String,
    pub session_id: String,
}

/// Executes scheduled agent jobs.
pub struct ScheduleExecutor {
    llm: Arc<dyn Provider>,
    config: Config,
}

impl ScheduleExecutor {
    pub fn new(llm: Arc<dyn Provider>, config: Config) -> Self {
        Self { llm, config }
    }

    /// Run a single schedule by name (for `agent schedule run`).
    pub async fn run_once(
        &self,
        schedule: &Schedule,
        sink: &dyn StreamSink,
    ) -> Result<JobOutcome, String> {
        info!("Running schedule '{}': {}", schedule.name, schedule.prompt);

        let mut config = self.config.clone();
        if let Some(ref model) = schedule.model {
            config.api.model = model.clone();
        }
        if let Some(ref perm) = schedule.permission_mode {
            config.permissions.default_mode = match perm.as_str() {
                "allow" => crate::config::PermissionMode::Allow,
                "deny" => crate::config::PermissionMode::Deny,
                "plan" => crate::config::PermissionMode::Plan,
                _ => crate::config::PermissionMode::Allow, // schedules default to allow
            };
        } else {
            // Schedules run non-interactively — default to allow.
            config.permissions.default_mode = crate::config::PermissionMode::Allow;
        }
        if let Some(max_cost) = schedule.max_cost_usd {
            config.api.max_cost_usd = Some(max_cost);
        }

        // Set cwd for the session.
        let prev_dir = std::env::current_dir().ok();
        if std::path::Path::new(&schedule.cwd).is_dir() {
            let _ = std::env::set_current_dir(&schedule.cwd);
        }

        let tool_registry = ToolRegistry::default_tools();
        let permission_checker = PermissionChecker::from_config(&config.permissions);
        let app_state = AppState::new(config.clone());
        let session_id = app_state.session_id.clone();

        let mut engine = QueryEngine::new(
            self.llm.clone(),
            tool_registry,
            permission_checker,
            app_state,
            QueryEngineConfig {
                max_turns: schedule.max_turns.or(Some(25)),
                verbose: false,
                unattended: true,
            },
        );

        engine.load_hooks(&config.hooks);

        let result = engine.run_turn_with_sink(&schedule.prompt, sink).await;

        // Restore cwd.
        if let Some(prev) = prev_dir {
            let _ = std::env::set_current_dir(prev);
        }

        let state = engine.state();
        let success = result.is_ok();
        let response = if let Err(ref e) = result {
            format!("Error: {e}")
        } else {
            // Extract last assistant text from messages.
            state
                .messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    crate::llm::message::Message::Assistant(a) => {
                        let text: String = a
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let crate::llm::message::ContentBlock::Text { text } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("");
                        if text.is_empty() { None } else { Some(text) }
                    }
                    _ => None,
                })
                .unwrap_or_default()
        };

        // Save session for later /resume.
        let _ = crate::services::session::save_session_full(
            &session_id,
            &state.messages,
            &state.cwd,
            &state.config.api.model,
            state.turn_count,
            state.total_cost_usd,
            state.total_usage.input_tokens,
            state.total_usage.output_tokens,
            false,
        );

        // Truncate summary.
        let summary = if response.len() > 500 {
            format!("{}...", &response[..497])
        } else {
            response
        };

        Ok(JobOutcome {
            schedule_name: schedule.name.clone(),
            success,
            turns: state.turn_count,
            cost_usd: state.total_cost_usd,
            response_summary: summary,
            session_id,
        })
    }

    /// Check all schedules and run any that are due.
    pub async fn check_and_run(&self, store: &ScheduleStore) {
        let now = Utc::now().naive_utc();
        let schedules = store.list();

        for schedule in schedules {
            if !schedule.enabled {
                continue;
            }

            let cron = match CronExpr::parse(&schedule.cron) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        "Schedule '{}': invalid cron '{}': {e}",
                        schedule.name, schedule.cron
                    );
                    continue;
                }
            };

            // Skip if not matching current minute.
            if !cron.matches(&now) {
                continue;
            }

            // Skip if already ran this minute.
            if let Some(ref last) = schedule.last_run_at {
                let last_naive = last.naive_utc();
                if last_naive.date() == now.date()
                    && last_naive.hour() == now.hour()
                    && last_naive.minute() == now.minute()
                {
                    continue;
                }
            }

            info!("Schedule '{}' triggered at {}", schedule.name, now);

            let outcome = self.run_once(&schedule, &crate::query::NullSink).await;

            // Update last_run state.
            let mut updated = schedule.clone();
            updated.last_run_at = Some(Utc::now());
            match outcome {
                Ok(ref o) => {
                    updated.last_result = Some(RunResult {
                        started_at: Utc::now() - chrono::Duration::seconds(1),
                        finished_at: Utc::now(),
                        success: o.success,
                        turns: o.turns,
                        cost_usd: o.cost_usd,
                        summary: o.response_summary.clone(),
                        session_id: o.session_id.clone(),
                    });
                    info!(
                        "Schedule '{}' completed: success={}, turns={}, cost=${:.4}",
                        updated.name, o.success, o.turns, o.cost_usd
                    );
                }
                Err(ref e) => {
                    updated.last_result = Some(RunResult {
                        started_at: Utc::now(),
                        finished_at: Utc::now(),
                        success: false,
                        turns: 0,
                        cost_usd: 0.0,
                        summary: e.clone(),
                        session_id: String::new(),
                    });
                    error!("Schedule '{}' failed: {e}", updated.name);
                }
            }

            if let Err(e) = store.save(&updated) {
                error!("Failed to save schedule state for '{}': {e}", updated.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_outcome_fields() {
        let outcome = JobOutcome {
            schedule_name: "test".to_string(),
            success: true,
            turns: 3,
            cost_usd: 0.05,
            response_summary: "done".to_string(),
            session_id: "abc".to_string(),
        };
        assert!(outcome.success);
        assert_eq!(outcome.turns, 3);
    }
}
