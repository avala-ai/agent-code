//! Schedule persistence.
//!
//! Each schedule is stored as a JSON file in
//! `~/.config/agent-code/schedules/<name>.json`. The store handles
//! CRUD operations and persists execution history.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// A persisted schedule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique schedule name (used as filename).
    pub name: String,
    /// Cron expression (5-field).
    pub cron: String,
    /// Prompt to send to the agent on each run.
    pub prompt: String,
    /// Working directory for the agent session.
    pub cwd: String,
    /// Whether this schedule is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional permission mode override.
    pub permission_mode: Option<String>,
    /// Maximum cost (USD) per run.
    pub max_cost_usd: Option<f64>,
    /// Maximum turns per run.
    pub max_turns: Option<usize>,
    /// When this schedule was created.
    pub created_at: DateTime<Utc>,
    /// Last execution time (if any).
    pub last_run_at: Option<DateTime<Utc>>,
    /// Last execution result.
    pub last_result: Option<RunResult>,
    /// Webhook secret for HTTP trigger (if set).
    pub webhook_secret: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Result of one execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub success: bool,
    pub turns: usize,
    pub cost_usd: f64,
    /// First 500 chars of the response.
    pub summary: String,
    /// Session ID for `/resume`.
    pub session_id: String,
}

/// CRUD operations for schedules.
pub struct ScheduleStore {
    dir: PathBuf,
}

impl ScheduleStore {
    /// Open or create the schedule store.
    pub fn open() -> Result<Self, String> {
        let dir = schedules_dir().ok_or("Could not determine config directory")?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create schedules dir: {e}"))?;
        Ok(Self { dir })
    }

    /// Open a store at a specific directory (for testing).
    pub fn open_at(dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create schedules dir: {e}"))?;
        Ok(Self { dir })
    }

    /// Save a schedule (creates or updates).
    pub fn save(&self, schedule: &Schedule) -> Result<(), String> {
        let path = self.path_for(&schedule.name);
        let json = serde_json::to_string_pretty(schedule)
            .map_err(|e| format!("Serialization error: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("Write error: {e}"))?;
        debug!("Schedule saved: {}", path.display());
        Ok(())
    }

    /// Load a schedule by name.
    pub fn load(&self, name: &str) -> Result<Schedule, String> {
        let path = self.path_for(name);
        if !path.exists() {
            return Err(format!("Schedule '{name}' not found"));
        }
        let content = std::fs::read_to_string(&path).map_err(|e| format!("Read error: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("Parse error: {e}"))
    }

    /// List all schedules, sorted by name.
    pub fn list(&self) -> Vec<Schedule> {
        let mut schedules: Vec<Schedule> = std::fs::read_dir(&self.dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| {
                let content = std::fs::read_to_string(entry.path()).ok()?;
                serde_json::from_str(&content).ok()
            })
            .collect();
        schedules.sort_by(|a, b| a.name.cmp(&b.name));
        schedules
    }

    /// Remove a schedule by name.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let path = self.path_for(name);
        if !path.exists() {
            return Err(format!("Schedule '{name}' not found"));
        }
        std::fs::remove_file(&path).map_err(|e| format!("Delete error: {e}"))?;
        debug!("Schedule removed: {name}");
        Ok(())
    }

    /// Find a schedule by webhook secret.
    pub fn find_by_secret(&self, secret: &str) -> Option<Schedule> {
        self.list()
            .into_iter()
            .find(|s| s.webhook_secret.as_deref() == Some(secret))
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }
}

fn schedules_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("schedules"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_schedule(name: &str) -> Schedule {
        Schedule {
            name: name.to_string(),
            cron: "0 9 * * *".to_string(),
            prompt: "run tests".to_string(),
            cwd: "/tmp/project".to_string(),
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

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        let sched = test_schedule("daily-tests");
        store.save(&sched).unwrap();

        let loaded = store.load("daily-tests").unwrap();
        assert_eq!(loaded.name, "daily-tests");
        assert_eq!(loaded.cron, "0 9 * * *");
        assert_eq!(loaded.prompt, "run tests");
        assert!(loaded.enabled);
    }

    #[test]
    fn test_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        store.save(&test_schedule("beta")).unwrap();
        store.save(&test_schedule("alpha")).unwrap();

        let list = store.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alpha"); // sorted
        assert_eq!(list[1].name, "beta");
    }

    #[test]
    fn test_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        store.save(&test_schedule("temp")).unwrap();
        assert!(store.load("temp").is_ok());

        store.remove("temp").unwrap();
        assert!(store.load("temp").is_err());
    }

    #[test]
    fn test_remove_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        assert!(store.remove("nope").is_err());
    }

    #[test]
    fn test_find_by_secret() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open_at(dir.path().to_path_buf()).unwrap();
        let mut sched = test_schedule("webhook-job");
        sched.webhook_secret = Some("s3cret".to_string());
        store.save(&sched).unwrap();

        let found = store.find_by_secret("s3cret").unwrap();
        assert_eq!(found.name, "webhook-job");
        assert!(store.find_by_secret("wrong").is_none());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut sched = test_schedule("roundtrip");
        sched.model = Some("gpt-5.4".to_string());
        sched.max_cost_usd = Some(1.0);
        sched.max_turns = Some(10);
        sched.last_result = Some(RunResult {
            started_at: Utc::now(),
            finished_at: Utc::now(),
            success: true,
            turns: 3,
            cost_usd: 0.05,
            summary: "All tests passed".to_string(),
            session_id: "abc12345".to_string(),
        });

        let json = serde_json::to_string(&sched).unwrap();
        let loaded: Schedule = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.model.as_deref(), Some("gpt-5.4"));
        assert!(loaded.last_result.unwrap().success);
    }
}
