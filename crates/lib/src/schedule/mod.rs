//! Scheduled agent execution.
//!
//! Enables cron-based and webhook-triggered agent runs. Schedules are
//! persisted as JSON in `~/.config/agent-code/schedules/` and executed
//! by a background daemon loop.
//!
//! # Usage
//!
//! ```bash
//! agent schedule add "0 9 * * *" --prompt "run tests" --name daily-tests
//! agent schedule list
//! agent schedule remove daily-tests
//! agent schedule run daily-tests
//! agent daemon                     # start the scheduler loop
//! ```

pub mod cron;
pub mod executor;
pub mod storage;

pub use cron::CronExpr;
pub use executor::{JobOutcome, ScheduleExecutor};
pub use storage::{Schedule, ScheduleStore};
