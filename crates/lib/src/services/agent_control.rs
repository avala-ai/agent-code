//! Subagent execution limiter.
//!
//! Background subagents are spawned as separate processes (see
//! [`crate::tools::agent`]). Without a cap, a lead agent (or a runaway
//! loop) can fan out an unbounded number of them and exhaust the
//! machine. [`AgentExecutionLimiter`] is a small semaphore that bounds
//! the number of subagents running *concurrently*; spawns past the cap
//! queue and start as running ones finish.
//!
//! It is intentionally separate from [`crate::services::policy_limits`],
//! which governs LLM-provider request/token admission — a different
//! concern keyed by provider, not OS-process fan-out.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Default cap on concurrently-running background subagents.
pub const DEFAULT_MAX_SUBAGENTS: usize = 4;

/// Bounds the number of concurrently-running subagents.
#[derive(Clone)]
pub struct AgentExecutionLimiter {
    sem: Arc<Semaphore>,
    max: usize,
}

impl AgentExecutionLimiter {
    /// Create a limiter allowing at most `max` concurrent subagents.
    /// `max` is clamped to at least 1.
    pub fn new(max: usize) -> Self {
        let max = max.max(1);
        Self {
            sem: Arc::new(Semaphore::new(max)),
            max,
        }
    }

    /// The configured maximum.
    pub fn max(&self) -> usize {
        self.max
    }

    /// Currently-available slots (subagents that could start right now).
    pub fn available(&self) -> usize {
        self.sem.available_permits()
    }

    /// Acquire a slot, waiting if all are in use. The returned guard
    /// releases the slot when dropped. Returns `None` only if the
    /// semaphore was closed (never, in normal use).
    pub async fn acquire(&self) -> Option<AgentExecutionGuard> {
        self.sem
            .clone()
            .acquire_owned()
            .await
            .ok()
            .map(|permit| AgentExecutionGuard { _permit: permit })
    }

    /// Try to acquire a slot without waiting. Returns `None` if at cap.
    pub fn try_acquire(&self) -> Option<AgentExecutionGuard> {
        self.sem
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| AgentExecutionGuard { _permit: permit })
    }
}

/// RAII slot held while a subagent runs; releases the slot on drop.
#[must_use = "the guard must be held for the subagent's lifetime; dropping it frees the slot"]
pub struct AgentExecutionGuard {
    _permit: OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clamps_zero_to_one() {
        assert_eq!(AgentExecutionLimiter::new(0).max(), 1);
        assert_eq!(AgentExecutionLimiter::new(3).max(), 3);
    }

    #[test]
    fn try_acquire_enforces_the_cap_and_releases_on_drop() {
        let lim = AgentExecutionLimiter::new(2);
        assert_eq!(lim.available(), 2);

        let g1 = lim.try_acquire().expect("slot 1");
        let g2 = lim.try_acquire().expect("slot 2");
        assert_eq!(lim.available(), 0);
        assert!(lim.try_acquire().is_none(), "cap exceeded");

        drop(g1);
        assert_eq!(lim.available(), 1);
        let _g3 = lim.try_acquire().expect("slot freed after drop");
        assert!(lim.try_acquire().is_none());
        drop(g2);
    }

    #[tokio::test]
    async fn acquire_waits_for_a_freed_slot() {
        let lim = AgentExecutionLimiter::new(1);
        let g1 = lim.try_acquire().expect("slot 1");

        // A second acquire can't complete until g1 is released.
        let early =
            tokio::time::timeout(std::time::Duration::from_millis(100), lim.acquire()).await;
        assert!(early.is_err(), "acquire should block while at cap");

        drop(g1);
        let g2 = tokio::time::timeout(std::time::Duration::from_millis(500), lim.acquire())
            .await
            .expect("acquire resolves after release");
        assert!(g2.is_some());
    }
}
