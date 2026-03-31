//! Cost and token budget enforcement.
//!
//! Tracks spending against configurable limits and determines
//! whether the agent should continue or stop.

/// Budget check result.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetDecision {
    /// Within budget, continue.
    Continue,
    /// Approaching limit, continue with notification.
    ContinueWithWarning { percent_used: f64, message: String },
    /// Budget exhausted, stop.
    Stop { message: String },
}

/// Configuration for budget limits.
#[derive(Debug, Clone)]
pub struct BudgetConfig {
    /// Maximum USD spend per session (None = unlimited).
    pub max_cost_usd: Option<f64>,
    /// Maximum total tokens per session (None = unlimited).
    pub max_tokens: Option<u64>,
    /// Warning threshold as fraction of budget (e.g., 0.8 = 80%).
    pub warning_threshold: f64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_cost_usd: None,
            max_tokens: None,
            warning_threshold: 0.8,
        }
    }
}

/// Check whether the current spend is within budget.
pub fn check_budget(
    current_cost_usd: f64,
    current_tokens: u64,
    config: &BudgetConfig,
) -> BudgetDecision {
    // Check cost budget.
    if let Some(max_cost) = config.max_cost_usd
        && max_cost > 0.0
    {
        let ratio = current_cost_usd / max_cost;
        if ratio >= 1.0 {
            return BudgetDecision::Stop {
                message: format!(
                    "Cost budget exhausted: ${:.4} / ${:.4}",
                    current_cost_usd, max_cost
                ),
            };
        }
        if ratio >= config.warning_threshold {
            return BudgetDecision::ContinueWithWarning {
                percent_used: ratio * 100.0,
                message: format!("Cost at {:.0}% of ${:.4} budget", ratio * 100.0, max_cost),
            };
        }
    }

    // Check token budget.
    if let Some(max_tokens) = config.max_tokens
        && max_tokens > 0
    {
        let ratio = current_tokens as f64 / max_tokens as f64;
        if ratio >= 1.0 {
            return BudgetDecision::Stop {
                message: format!(
                    "Token budget exhausted: {} / {} tokens",
                    current_tokens, max_tokens
                ),
            };
        }
        if ratio >= config.warning_threshold {
            return BudgetDecision::ContinueWithWarning {
                percent_used: ratio * 100.0,
                message: format!("Tokens at {:.0}% of {} budget", ratio * 100.0, max_tokens),
            };
        }
    }

    BudgetDecision::Continue
}

/// Continuation check for token budget during multi-turn execution.
///
/// After each turn, decide whether to continue based on how many
/// tokens were consumed and whether progress is diminishing.
pub fn should_continue_turn(
    turn_tokens: u64,
    total_budget: Option<u64>,
    consecutive_low_progress_turns: u32,
) -> bool {
    let Some(budget) = total_budget else {
        return true; // No budget = always continue.
    };

    if budget == 0 {
        return false;
    }

    // Stop at 90% of budget.
    if turn_tokens >= (budget as f64 * 0.9) as u64 {
        return false;
    }

    // Stop after 3 turns with minimal progress (< 500 tokens each).
    if consecutive_low_progress_turns >= 3 {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_within_budget() {
        let config = BudgetConfig {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        assert_eq!(check_budget(0.5, 0, &config), BudgetDecision::Continue);
    }

    #[test]
    fn test_budget_warning() {
        let config = BudgetConfig {
            max_cost_usd: Some(1.0),
            warning_threshold: 0.8,
            ..Default::default()
        };
        match check_budget(0.85, 0, &config) {
            BudgetDecision::ContinueWithWarning { .. } => {}
            other => panic!("Expected warning, got: {other:?}"),
        }
    }

    #[test]
    fn test_budget_exhausted() {
        let config = BudgetConfig {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        match check_budget(1.5, 0, &config) {
            BudgetDecision::Stop { .. } => {}
            other => panic!("Expected stop, got: {other:?}"),
        }
    }

    #[test]
    fn test_continuation_logic() {
        assert!(should_continue_turn(1000, Some(10000), 0));
        assert!(!should_continue_turn(9500, Some(10000), 0));
        assert!(!should_continue_turn(1000, Some(10000), 3));
    }
}
