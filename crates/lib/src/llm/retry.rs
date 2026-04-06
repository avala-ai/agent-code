//! Retry logic and streaming fallback handling.
//!
//! When streaming fails mid-response, the retry handler can:
//! - Discard partial tool executions with synthetic error blocks
//! - Fall back to a smaller model on repeated overload errors
//! - Apply exponential backoff with jitter

use std::time::Duration;

/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum retry attempts for transient errors.
    pub max_retries: u32,
    /// Initial backoff duration.
    pub initial_backoff: Duration,
    /// Maximum backoff duration.
    pub max_backoff: Duration,
    /// Backoff multiplier (exponential).
    pub multiplier: f64,
    /// Maximum 529 (overloaded) retries before falling back.
    pub max_overload_retries: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1000),
            max_backoff: Duration::from_secs(60),
            multiplier: 2.0,
            max_overload_retries: 3,
        }
    }
}

/// State tracker for retry logic across multiple attempts.
#[derive(Debug, Default)]
pub struct RetryState {
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
    /// Number of 429 (rate limit) retries.
    pub rate_limit_retries: u32,
    /// Number of 529 (overload) retries.
    pub overload_retries: u32,
    /// Whether we've fallen back to the smaller model.
    pub using_fallback: bool,
}

impl RetryState {
    /// Determine the next action after a failure.
    pub fn next_action(&mut self, error: &RetryableError, config: &RetryConfig) -> RetryAction {
        self.consecutive_failures += 1;

        match error {
            RetryableError::RateLimited { retry_after } => {
                self.rate_limit_retries += 1;
                if self.rate_limit_retries > config.max_retries {
                    return RetryAction::Abort("Rate limit retries exhausted".into());
                }
                RetryAction::Retry {
                    after: Duration::from_millis(*retry_after),
                }
            }
            RetryableError::Overloaded => {
                self.overload_retries += 1;
                if self.overload_retries > config.max_overload_retries {
                    if !self.using_fallback {
                        self.using_fallback = true;
                        self.overload_retries = 0;
                        return RetryAction::FallbackModel;
                    }
                    return RetryAction::Abort("Overload retries exhausted on fallback".into());
                }
                let backoff = calculate_backoff(
                    self.overload_retries,
                    config.initial_backoff,
                    config.max_backoff,
                    config.multiplier,
                );
                RetryAction::Retry { after: backoff }
            }
            RetryableError::StreamInterrupted => {
                if self.consecutive_failures > config.max_retries {
                    return RetryAction::Abort("Stream retry limit reached".into());
                }
                let backoff = calculate_backoff(
                    self.consecutive_failures,
                    config.initial_backoff,
                    config.max_backoff,
                    config.multiplier,
                );
                RetryAction::Retry { after: backoff }
            }
            RetryableError::NonRetryable(msg) => RetryAction::Abort(msg.clone()),
        }
    }

    /// Reset counters after a successful call.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.rate_limit_retries = 0;
        // Don't reset overload_retries or using_fallback — those persist.
    }
}

/// Categorized error for retry logic.
pub enum RetryableError {
    RateLimited { retry_after: u64 },
    Overloaded,
    StreamInterrupted,
    NonRetryable(String),
}

/// Action the caller should take after a failure.
#[derive(Debug)]
pub enum RetryAction {
    /// Wait and retry with the same model.
    Retry { after: Duration },
    /// Switch to the fallback model and retry.
    FallbackModel,
    /// Give up — unrecoverable.
    Abort(String),
}

/// Calculate exponential backoff with jitter.
fn calculate_backoff(attempt: u32, initial: Duration, max: Duration, multiplier: f64) -> Duration {
    let base = initial.as_millis() as f64 * multiplier.powi(attempt as i32 - 1);
    let capped = base.min(max.as_millis() as f64);
    // Add 10% jitter.
    let jitter = capped * 0.1 * rand_f64();
    Duration::from_millis((capped + jitter) as u64)
}

/// Simple pseudo-random f64 in [0, 1) using timestamp.
fn rand_f64() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1000) as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let c = RetryConfig::default();
        assert_eq!(c.max_retries, 3);
        assert!(c.multiplier > 1.0);
    }

    #[test]
    fn test_retry_on_rate_limit() {
        let mut state = RetryState::default();
        let config = RetryConfig::default();
        let err = RetryableError::RateLimited { retry_after: 500 };
        match state.next_action(&err, &config) {
            RetryAction::Retry { after } => assert!(after.as_millis() >= 500),
            other => panic!("Expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn test_retry_exhaustion() {
        let mut state = RetryState::default();
        let config = RetryConfig {
            max_retries: 1,
            ..Default::default()
        };
        let err = RetryableError::RateLimited { retry_after: 100 };
        let _ = state.next_action(&err, &config); // First retry.
        match state.next_action(&err, &config) {
            RetryAction::Abort(_) => {}
            other => panic!("Expected Abort, got {other:?}"),
        }
    }

    #[test]
    fn test_non_retryable_aborts() {
        let mut state = RetryState::default();
        let config = RetryConfig::default();
        let err = RetryableError::NonRetryable("bad request".into());
        match state.next_action(&err, &config) {
            RetryAction::Abort(msg) => assert!(msg.contains("bad request")),
            other => panic!("Expected Abort, got {other:?}"),
        }
    }

    #[test]
    fn test_overload_escalates_to_fallback() {
        let mut state = RetryState::default();
        let config = RetryConfig {
            max_overload_retries: 2,
            ..Default::default()
        };
        let err = RetryableError::Overloaded;
        let _ = state.next_action(&err, &config);
        let _ = state.next_action(&err, &config);
        match state.next_action(&err, &config) {
            RetryAction::FallbackModel => {}
            other => panic!("Expected FallbackModel, got {other:?}"),
        }
    }

    #[test]
    fn test_reset_preserves_fallback() {
        let mut state = RetryState {
            using_fallback: true,
            consecutive_failures: 5,
            ..Default::default()
        };
        state.reset();
        assert_eq!(state.consecutive_failures, 0);
        assert!(state.using_fallback); // Preserved.
    }

    #[test]
    fn test_backoff_increases_with_attempt() {
        let initial = Duration::from_millis(1000);
        let max = Duration::from_secs(60);
        let multiplier = 2.0;

        let _b1 = calculate_backoff(1, initial, max, multiplier);
        let b2 = calculate_backoff(2, initial, max, multiplier);
        let b3 = calculate_backoff(3, initial, max, multiplier);

        // Each attempt should generally produce a larger backoff (before jitter caps).
        // With multiplier 2.0: attempt 1 ~1s, attempt 2 ~2s, attempt 3 ~4s.
        assert!(b2.as_millis() >= 1500, "b2 should be >= 1.5s, got {:?}", b2);
        assert!(b3.as_millis() >= 3000, "b3 should be >= 3s, got {:?}", b3);
    }

    #[test]
    fn test_reset_clears_rate_limit_retries() {
        let mut state = RetryState {
            consecutive_failures: 3,
            rate_limit_retries: 5,
            overload_retries: 2,
            using_fallback: false,
        };
        state.reset();
        assert_eq!(state.rate_limit_retries, 0);
        assert_eq!(state.consecutive_failures, 0);
        // overload_retries and using_fallback persist.
        assert_eq!(state.overload_retries, 2);
    }

    #[test]
    fn test_overloads_then_fallback_then_abort() {
        let mut state = RetryState::default();
        let config = RetryConfig {
            max_overload_retries: 1,
            ..Default::default()
        };
        let err = RetryableError::Overloaded;

        // First overload: retry with backoff.
        match state.next_action(&err, &config) {
            RetryAction::Retry { .. } => {}
            other => panic!("Expected Retry, got {other:?}"),
        }

        // Second overload: exceeds max_overload_retries, triggers fallback.
        match state.next_action(&err, &config) {
            RetryAction::FallbackModel => {}
            other => panic!("Expected FallbackModel, got {other:?}"),
        }
        assert!(state.using_fallback);

        // Now on fallback model, overload again: retry.
        match state.next_action(&err, &config) {
            RetryAction::Retry { .. } => {}
            other => panic!("Expected Retry on fallback, got {other:?}"),
        }

        // Exceed overloads on fallback: abort.
        match state.next_action(&err, &config) {
            RetryAction::Abort(msg) => assert!(msg.contains("fallback")),
            other => panic!("Expected Abort, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_interrupted_retries_then_aborts() {
        let mut state = RetryState::default();
        let config = RetryConfig {
            max_retries: 2,
            ..Default::default()
        };
        let err = RetryableError::StreamInterrupted;

        // First two interruptions should retry.
        match state.next_action(&err, &config) {
            RetryAction::Retry { .. } => {}
            other => panic!("Expected Retry, got {other:?}"),
        }
        match state.next_action(&err, &config) {
            RetryAction::Retry { .. } => {}
            other => panic!("Expected Retry, got {other:?}"),
        }

        // Third interruption exceeds max_retries => abort.
        match state.next_action(&err, &config) {
            RetryAction::Abort(msg) => assert!(msg.contains("Stream")),
            other => panic!("Expected Abort, got {other:?}"),
        }
    }

    #[test]
    fn test_retry_state_default_values() {
        let state = RetryState::default();
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.rate_limit_retries, 0);
        assert_eq!(state.overload_retries, 0);
        assert!(!state.using_fallback);
    }
}
