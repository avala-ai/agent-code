//! Daemon mode for scheduled agent execution.
//!
//! Starts a background loop that checks schedules every 30 seconds
//! and executes matching jobs. Optionally starts a webhook HTTP server
//! for external triggers.
//!
//! # Usage
//!
//! ```bash
//! agent daemon                     # Run scheduler only
//! agent daemon --webhook-port 8090 # Also listen for webhooks
//! ```

use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use agent_code_lib::config::Config;
use agent_code_lib::llm::provider::Provider;
use agent_code_lib::query::NullSink;
use agent_code_lib::schedule::{ScheduleExecutor, ScheduleStore};

/// State shared by webhook handlers.
struct WebhookState {
    executor: ScheduleExecutor,
}

/// POST /trigger?secret=<webhook_secret>
#[derive(Debug, Deserialize)]
struct TriggerParams {
    secret: String,
}

/// Response from the trigger endpoint.
#[derive(Debug, Serialize)]
struct TriggerResponse {
    schedule: String,
    success: bool,
    turns: usize,
    cost_usd: f64,
    summary: String,
    session_id: String,
}

/// Start the daemon (scheduler loop + optional webhook server).
pub async fn run_daemon(
    llm: Arc<dyn Provider>,
    config: Config,
    webhook_port: Option<u16>,
) -> anyhow::Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Start the cron loop in a background task.
    let cron_executor = ScheduleExecutor::new(llm.clone(), config.clone());
    let mut cron_rx = shutdown_rx.clone();
    let cron_handle = tokio::spawn(async move {
        let store = match ScheduleStore::open() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to open schedule store: {e}");
                return;
            }
        };
        tracing::info!("Schedule daemon started — checking every 30s");
        loop {
            tokio::select! {
                _ = cron_rx.changed() => {
                    tracing::info!("Schedule daemon shutting down");
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    cron_executor.check_and_run(&store).await;
                }
            }
        }
    });

    // Optionally start the webhook server.
    if let Some(port) = webhook_port {
        let executor = ScheduleExecutor::new(llm, config);
        let state = Arc::new(WebhookState { executor });

        let app = Router::new()
            .route("/trigger", post(handle_trigger))
            .with_state(state);

        let addr = format!("127.0.0.1:{port}");
        eprintln!("Webhook server listening on http://{addr}/trigger");

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        let mut server_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = server_rx.changed().await;
                })
                .await
                .ok();
        });
    }

    eprintln!("Schedule daemon running. Press Ctrl+C to stop.");

    // Wait for Ctrl+C.
    tokio::signal::ctrl_c().await?;
    let _ = shutdown_tx.send(true);
    cron_handle.await?;

    eprintln!("Daemon stopped.");
    Ok(())
}

/// Handle webhook trigger.
async fn handle_trigger(
    State(state): State<Arc<WebhookState>>,
    Query(params): Query<TriggerParams>,
) -> Result<Json<TriggerResponse>, (StatusCode, String)> {
    let store = ScheduleStore::open().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let schedule = store.find_by_secret(&params.secret).ok_or((
        StatusCode::NOT_FOUND,
        "No schedule matches this secret".into(),
    ))?;

    if !schedule.enabled {
        return Err((StatusCode::CONFLICT, "Schedule is disabled".into()));
    }

    let outcome = state
        .executor
        .run_once(&schedule, &NullSink)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Persist result.
    let mut updated = schedule;
    updated.last_run_at = Some(chrono::Utc::now());
    updated.last_result = Some(agent_code_lib::schedule::storage::RunResult {
        started_at: chrono::Utc::now(),
        finished_at: chrono::Utc::now(),
        success: outcome.success,
        turns: outcome.turns,
        cost_usd: outcome.cost_usd,
        summary: outcome.response_summary.clone(),
        session_id: outcome.session_id.clone(),
    });
    let _ = store.save(&updated);

    Ok(Json(TriggerResponse {
        schedule: outcome.schedule_name,
        success: outcome.success,
        turns: outcome.turns,
        cost_usd: outcome.cost_usd,
        summary: outcome.response_summary,
        session_id: outcome.session_id,
    }))
}
