//! Prompt cache tracking and break detection.
//!
//! Monitors cache hit/miss patterns across API calls to identify
//! when the prompt cache is breaking and why. Tracks cache creation
//! vs read tokens to compute effective cache utilization.

use crate::llm::message::Usage;

/// Tracks cache performance across multiple API calls.
#[derive(Debug, Default)]
pub struct CacheTracker {
    /// Total cache creation tokens (cache misses that create new entries).
    pub total_cache_writes: u64,
    /// Total cache read tokens (cache hits).
    pub total_cache_reads: u64,
    /// Number of API calls observed.
    pub call_count: u64,
    /// Number of calls that had any cache reads (hits).
    pub hit_count: u64,
    /// Number of calls where cache writes exceeded reads (likely break).
    pub break_count: u64,
    /// Last observed cache state.
    last_write: u64,
    last_read: u64,
    /// Fingerprint of the last request prefix (system prompt hash + tool count).
    /// Used to detect when prompt changes cause cache invalidation.
    last_fingerprint: u64,
}

impl CacheTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the fingerprint of the cacheable request prefix.
    /// Call before each API request. Returns true if the fingerprint changed
    /// (indicating the cache will likely break).
    pub fn update_fingerprint(&mut self, system_prompt: &str, tool_count: usize) -> bool {
        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&system_prompt.len(), &mut hasher);
        // Hash first and last 200 chars for speed (full hash unnecessary).
        let prefix = &system_prompt[..system_prompt.len().min(200)];
        std::hash::Hash::hash(prefix, &mut hasher);
        std::hash::Hash::hash(&tool_count, &mut hasher);
        let fp = std::hash::Hasher::finish(&hasher);

        let changed = self.last_fingerprint != 0 && self.last_fingerprint != fp;
        self.last_fingerprint = fp;
        changed
    }

    /// Record usage from an API call and detect cache breaks.
    pub fn record(&mut self, usage: &Usage) -> CacheEvent {
        self.call_count += 1;
        self.total_cache_writes += usage.cache_creation_input_tokens;
        self.total_cache_reads += usage.cache_read_input_tokens;

        let had_reads = usage.cache_read_input_tokens > 0;
        let had_writes = usage.cache_creation_input_tokens > 0;

        if had_reads {
            self.hit_count += 1;
        }

        let event = if !had_reads && had_writes && self.call_count > 1 {
            // Cache miss on a non-first call — likely a break.
            self.break_count += 1;
            CacheEvent::Break {
                write_tokens: usage.cache_creation_input_tokens,
                reason: if self.last_read > 0 {
                    "Cache invalidated since last call".to_string()
                } else {
                    "No cache hits — content may have changed".to_string()
                },
            }
        } else if had_reads && !had_writes {
            // Pure cache hit — ideal.
            CacheEvent::Hit {
                read_tokens: usage.cache_read_input_tokens,
            }
        } else if had_reads && had_writes {
            // Partial hit — some content cached, some new.
            CacheEvent::Partial {
                read_tokens: usage.cache_read_input_tokens,
                write_tokens: usage.cache_creation_input_tokens,
            }
        } else {
            // First call or no caching configured.
            CacheEvent::Miss
        };

        self.last_write = usage.cache_creation_input_tokens;
        self.last_read = usage.cache_read_input_tokens;

        event
    }

    /// Cache hit rate as a percentage (0-100).
    pub fn hit_rate(&self) -> f64 {
        if self.call_count == 0 {
            return 0.0;
        }
        (self.hit_count as f64 / self.call_count as f64) * 100.0
    }

    /// Estimated cost savings from cache hits.
    /// Cache reads are ~10% the cost of cache writes.
    pub fn estimated_savings(&self) -> f64 {
        // Savings = (cache_reads * 0.9 * cost_per_token)
        // Approximate: saved tokens * 90% discount
        self.total_cache_reads as f64 * 0.9
    }
}

/// Event produced by cache tracking for each API call.
#[derive(Debug)]
pub enum CacheEvent {
    /// Full cache hit — all cached content was reused.
    Hit { read_tokens: u64 },
    /// Cache break — previously cached content was not reused.
    Break { write_tokens: u64, reason: String },
    /// Partial hit — some cached, some new.
    Partial { read_tokens: u64, write_tokens: u64 },
    /// No cache interaction (first call or caching disabled).
    Miss,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tracker() {
        let t = CacheTracker::new();
        assert_eq!(t.call_count, 0);
        assert_eq!(t.hit_rate(), 0.0);
    }

    #[test]
    fn test_first_call_miss() {
        let mut t = CacheTracker::new();
        let event = t.record(&Usage {
            cache_creation_input_tokens: 1000,
            ..Default::default()
        });
        assert!(matches!(event, CacheEvent::Miss));
        assert_eq!(t.call_count, 1);
    }

    #[test]
    fn test_cache_hit() {
        let mut t = CacheTracker::new();
        // First call — miss.
        t.record(&Usage {
            cache_creation_input_tokens: 1000,
            ..Default::default()
        });
        // Second call — hit.
        let event = t.record(&Usage {
            cache_read_input_tokens: 900,
            ..Default::default()
        });
        assert!(matches!(event, CacheEvent::Hit { .. }));
        assert_eq!(t.hit_count, 1);
    }

    #[test]
    fn test_cache_break() {
        let mut t = CacheTracker::new();
        // First call with reads.
        t.record(&Usage {
            cache_read_input_tokens: 500,
            ..Default::default()
        });
        // Second call — no reads, only writes = break.
        let event = t.record(&Usage {
            cache_creation_input_tokens: 1000,
            ..Default::default()
        });
        assert!(matches!(event, CacheEvent::Break { .. }));
        assert_eq!(t.break_count, 1);
    }

    #[test]
    fn test_partial_hit() {
        let mut t = CacheTracker::new();
        t.record(&Usage::default()); // First call.
        let event = t.record(&Usage {
            cache_read_input_tokens: 500,
            cache_creation_input_tokens: 200,
            ..Default::default()
        });
        assert!(matches!(event, CacheEvent::Partial { .. }));
    }

    #[test]
    fn test_hit_rate() {
        let mut t = CacheTracker::new();
        t.record(&Usage::default()); // Miss.
        t.record(&Usage {
            cache_read_input_tokens: 100,
            ..Default::default()
        }); // Hit.
        assert!((t.hit_rate() - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_fingerprint_change_detection() {
        let mut t = CacheTracker::new();
        let changed = t.update_fingerprint("system prompt v1", 10);
        assert!(!changed); // First call, no previous fingerprint.

        let changed = t.update_fingerprint("system prompt v1", 10);
        assert!(!changed); // Same fingerprint.

        let changed = t.update_fingerprint("system prompt v2", 10);
        assert!(changed); // Different prompt.
    }

    #[test]
    fn test_fingerprint_tool_count_change() {
        let mut t = CacheTracker::new();
        t.update_fingerprint("prompt", 10);
        let changed = t.update_fingerprint("prompt", 15);
        assert!(changed); // Different tool count.
    }
}
