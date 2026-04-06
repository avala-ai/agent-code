use serde::{Deserialize, Serialize};

/// Eval pass/fail policy tier.
///
/// | Tier           | Pass Requirement     | CI Behavior          |
/// |----------------|---------------------|----------------------|
/// | AlwaysPasses   | 100% (all retries)  | Blocks merge         |
/// | UsuallyPasses  | 50%+ (best-of-N)    | Monitored, no block  |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvalPolicy {
    /// Must pass every retry. Gates CI.
    AlwaysPasses,
    /// Must pass 50%+ of retries. Monitored nightly.
    UsuallyPasses,
}

impl EvalPolicy {
    /// Check if the eval passed given the number of passes and total runs.
    pub fn passed(&self, passes: usize, total: usize) -> bool {
        match self {
            EvalPolicy::AlwaysPasses => passes == total,
            EvalPolicy::UsuallyPasses => passes * 2 >= total, // 50%+
        }
    }

    /// Default number of retries for this policy.
    pub fn default_retries(&self) -> usize {
        match self {
            EvalPolicy::AlwaysPasses => 4,
            EvalPolicy::UsuallyPasses => 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_passes_requires_all() {
        assert!(EvalPolicy::AlwaysPasses.passed(4, 4));
        assert!(!EvalPolicy::AlwaysPasses.passed(3, 4));
        assert!(!EvalPolicy::AlwaysPasses.passed(0, 4));
    }

    #[test]
    fn usually_passes_requires_half() {
        assert!(EvalPolicy::UsuallyPasses.passed(4, 4));
        assert!(EvalPolicy::UsuallyPasses.passed(3, 4));
        assert!(EvalPolicy::UsuallyPasses.passed(2, 4));
        assert!(!EvalPolicy::UsuallyPasses.passed(1, 4));
        assert!(!EvalPolicy::UsuallyPasses.passed(0, 4));
    }
}
