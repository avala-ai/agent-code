//! BATCH stage: bucket signals into bounded shards.
//!
//! Deterministic and pure. `max_signals_per_shard` is the primary cost
//! lever: it sets how many MAP workers fan out, which multiplies both
//! wall-clock and token spend. Signals for the same file are always kept
//! together so a worker can reason locally, and a file whose signal count
//! exceeds the cap becomes its own shard rather than being split.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::types::{Batch, Signal};

/// Batching parameters.
#[derive(Debug, Clone, Copy)]
pub struct BatchConfig {
    /// Target upper bound on signals per shard. The real cost knob.
    pub max_signals_per_shard: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_signals_per_shard: 40,
        }
    }
}

/// Group signals by file and pack files into bounded shards.
///
/// Files are visited in sorted order, so the same signal set always yields
/// the same shards with the same ids.
pub fn batch_signals(signals: Vec<Signal>, cfg: &BatchConfig) -> Vec<Batch> {
    let cap = cfg.max_signals_per_shard.max(1);

    let mut by_file: BTreeMap<PathBuf, Vec<Signal>> = BTreeMap::new();
    for s in signals {
        by_file.entry(s.file.clone()).or_default().push(s);
    }

    let mut shards: Vec<Vec<Signal>> = Vec::new();
    let mut current: Vec<Signal> = Vec::new();
    for (_file, sigs) in by_file {
        if !current.is_empty() && current.len() + sigs.len() > cap {
            shards.push(std::mem::take(&mut current));
        }
        current.extend(sigs);
        if current.len() >= cap {
            shards.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        shards.push(current);
    }

    shards
        .into_iter()
        .enumerate()
        .map(|(i, signals)| Batch {
            id: format!("shard-{i:04}"),
            signals,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(file: &str, n: usize) -> Signal {
        Signal {
            file: PathBuf::from(file),
            line: Some(n),
            byte_range: Some((n, n + 1)),
            selector_id: "s".into(),
            evidence: String::new(),
        }
    }

    #[test]
    fn keeps_a_files_signals_in_one_shard() {
        let signals = vec![sig("a.py", 1), sig("a.py", 2), sig("b.py", 1)];
        let batches = batch_signals(
            signals,
            &BatchConfig {
                max_signals_per_shard: 40,
            },
        );
        // Small input fits a single shard.
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].id, "shard-0000");
        assert_eq!(batches[0].signals.len(), 3);
    }

    #[test]
    fn respects_the_cap_across_files() {
        let signals = vec![
            sig("a.py", 1),
            sig("a.py", 2),
            sig("b.py", 1),
            sig("b.py", 2),
            sig("c.py", 1),
        ];
        let batches = batch_signals(
            signals,
            &BatchConfig {
                max_signals_per_shard: 2,
            },
        );
        // a.py (2) fills one shard, b.py (2) another, c.py (1) a third.
        assert_eq!(batches.len(), 3);
        assert!(batches.iter().all(|b| b.signals.len() <= 2));
    }

    #[test]
    fn a_single_oversized_file_becomes_its_own_shard() {
        let signals = vec![sig("big.py", 1), sig("big.py", 2), sig("big.py", 3)];
        let batches = batch_signals(
            signals,
            &BatchConfig {
                max_signals_per_shard: 2,
            },
        );
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].signals.len(), 3);
        assert_eq!(batches[0].files(), vec![PathBuf::from("big.py")]);
    }

    #[test]
    fn batching_is_deterministic() {
        let signals = vec![sig("z.py", 1), sig("a.py", 1), sig("m.py", 1)];
        let cfg = BatchConfig {
            max_signals_per_shard: 1,
        };
        let a = batch_signals(signals.clone(), &cfg);
        let b = batch_signals(signals, &cfg);
        assert_eq!(a, b);
        // Files visited in sorted order → a.py lands in shard-0000.
        assert_eq!(a[0].signals[0].file, PathBuf::from("a.py"));
    }

    #[test]
    fn empty_input_yields_no_shards() {
        assert!(batch_signals(vec![], &BatchConfig::default()).is_empty());
    }
}
