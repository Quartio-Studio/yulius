//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629, ADR-0461):
//! the rebalance threshold of each harness. [`crate::rotation`] places runs off a subscription
//! past it while another has room.

use crate::harness::Provider;
use crate::rotation::REBALANCE;

/// Lowest rebalance threshold a harness may be given, in percent.
pub const MIN_PERCENT: u8 = 50;
/// Highest rebalance threshold a harness may be given, in percent: the pool's own threshold
/// ([`crate::rotation::THRESHOLD`]), at which it changes nothing.
pub const MAX_PERCENT: u8 = 98;

/// The rebalance threshold of each harness in percent of a window binding the task's model
/// (SPEC-629 RBL-001).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Claude Code's (Claude subscriptions).
    pub claude_code: f64,
    /// Codex's (OpenAI subscriptions).
    pub codex: f64,
    /// Antigravity's (Google subscriptions).
    pub antigravity: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            claude_code: REBALANCE,
            codex: REBALANCE,
            antigravity: REBALANCE,
        }
    }
}

impl Thresholds {
    /// The threshold of the harness whose subscriptions are `provider`'s.
    #[must_use]
    pub fn of(&self, provider: Provider) -> f64 {
        match provider {
            Provider::Anthropic => self.claude_code,
            Provider::OpenAi => self.codex,
            Provider::Google => self.antigravity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_harness_has_its_own_threshold() {
        let thresholds = Thresholds {
            claude_code: 90.0,
            codex: 80.0,
            antigravity: 70.0,
        };
        assert_eq!(thresholds.of(Provider::Anthropic), 90.0);
        assert_eq!(thresholds.of(Provider::OpenAi), 80.0);
        assert_eq!(thresholds.of(Provider::Google), 70.0);
        assert_eq!(Thresholds::default().of(Provider::OpenAi), 95.0);
    }
}
