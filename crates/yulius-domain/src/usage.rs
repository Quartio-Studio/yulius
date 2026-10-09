//! Provider usage snapshots and limit signals (SPEC-001 ROT-003/004/010/011, SPEC-002 §7.2).
//!
//! Unknown values stay `None`: unknown usage is never zero usage (ROT-011, EVT-001). A window or
//! limit whose reset time has passed no longer counts against the account.

use crate::time::Timestamp;

/// One provider rate-limit window as last reported (for example Claude's five-hour window).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UsageWindow {
    /// Percentage of the window already used (0–100, may exceed 100); `None` when unknown.
    pub used_percent: Option<f64>,
    /// Window length in minutes; `None` when unknown.
    pub window_minutes: Option<u32>,
    /// When the window resets; `None` when unknown.
    pub resets_at: Option<Timestamp>,
}

impl UsageWindow {
    /// `true` when the reset time is known and not after `now`.
    #[must_use]
    pub fn is_reset(&self, now: Timestamp) -> bool {
        self.resets_at.is_some_and(|at| at <= now)
    }

    /// The reported percentage when it is a real number.
    #[must_use]
    pub fn known_percent(&self) -> Option<f64> {
        self.used_percent.filter(|p| p.is_finite())
    }

    /// Usage that still applies at `now`: `Some(0.0)` after a known reset (a fresh window),
    /// otherwise the reported percentage; `None` when unknown.
    #[must_use]
    pub fn current_percent(&self, now: Timestamp) -> Option<f64> {
        let reported = self.known_percent()?;
        Some(if self.is_reset(now) { 0.0 } else { reported })
    }
}

/// An account's latest usage snapshot. `Default` is "nothing known".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AccountUsage {
    /// Short window (for example the 5-hour window); `None` when not reported.
    pub primary: Option<UsageWindow>,
    /// Long window (for example the weekly window); `None` when not reported.
    pub secondary: Option<UsageWindow>,
    /// The provider reported that a usage limit was reached.
    pub limit_reached: bool,
    /// When the reached limit resets; `None` when unknown.
    pub limit_resets_at: Option<Timestamp>,
    /// When this snapshot was observed; `None` when unknown.
    pub observed_at: Option<Timestamp>,
}

impl AccountUsage {
    fn windows(&self) -> impl Iterator<Item = &UsageWindow> {
        self.primary.iter().chain(self.secondary.iter())
    }

    /// Highest reported window percentage, ignoring reset times; `None` when no window
    /// percentage is known.
    #[must_use]
    pub fn max_used_percent(&self) -> Option<f64> {
        self.windows()
            .filter_map(UsageWindow::known_percent)
            .reduce(f64::max)
    }

    /// Highest window percentage still applying at `now` (reset windows count as 0);
    /// `None` when no window percentage is known.
    #[must_use]
    pub fn current_used_percent(&self, now: Timestamp) -> Option<f64> {
        self.windows()
            .filter_map(|w| w.current_percent(now))
            .reduce(f64::max)
    }

    /// `true` while a reported limit is still in effect (its reset is unknown or in the future).
    #[must_use]
    pub fn limit_in_effect(&self, now: Timestamp) -> bool {
        self.limit_reached && self.limit_resets_at.is_none_or(|at| at > now)
    }

    /// `true` when the account cannot be used at `now`: a limit is in effect or a window that
    /// has not reset is at or above `threshold` percent.
    #[must_use]
    pub fn exhausted(&self, now: Timestamp, threshold: f64) -> bool {
        self.limit_in_effect(now)
            || self
                .current_used_percent(now)
                .is_some_and(|p| p >= threshold)
    }

    /// `true` when the snapshot contains a reached limit or a window whose known reset time has
    /// passed, so the snapshot overstates current usage.
    #[must_use]
    pub fn window_reset(&self, now: Timestamp) -> bool {
        (self.limit_reached && self.limit_resets_at.is_some_and(|at| at <= now))
            || self.windows().any(|w| w.is_reset(now))
    }

    /// `true` when the snapshot showed the account exhausted (ignoring reset times) but every
    /// cause of that exhaustion has reset by `now`.
    #[must_use]
    pub fn recovered_by_reset(&self, now: Timestamp, threshold: f64) -> bool {
        let raw_exhausted =
            self.limit_reached || self.max_used_percent().is_some_and(|p| p >= threshold);
        raw_exhausted && !self.exhausted(now, threshold) && self.window_reset(now)
    }

    /// When the account stops being exhausted: the latest reset among all causes still in
    /// effect at `now`. `None` when it is not exhausted or a blocking cause has no known reset.
    #[must_use]
    pub fn available_at(&self, now: Timestamp, threshold: f64) -> Option<Timestamp> {
        if !self.exhausted(now, threshold) {
            return None;
        }
        let mut latest: Option<Timestamp> = None;
        if self.limit_in_effect(now) {
            latest = Some(self.limit_resets_at?);
        }
        for window in self.windows() {
            if window.current_percent(now).is_some_and(|p| p >= threshold) {
                let at = window.resets_at?;
                latest = Some(latest.map_or(at, |l| l.max(at)));
            }
        }
        latest
    }
}

/// Token counts for a message, turn or attempt. Every field is `None` when unknown; unknown is
/// not zero.
///
/// `input` counts input tokens not served from cache, `cache_read` cached input tokens,
/// `cache_write` tokens written to the prompt cache and `output` generated tokens (including
/// reasoning tokens where the provider counts them as output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct TokenUsage {
    /// Uncached input tokens.
    pub input: Option<u64>,
    /// Output tokens.
    pub output: Option<u64>,
    /// Input tokens read from the prompt cache.
    pub cache_read: Option<u64>,
    /// Input tokens written to the prompt cache.
    pub cache_write: Option<u64>,
}

impl TokenUsage {
    /// Usage with every field unknown.
    pub const UNKNOWN: Self = Self {
        input: None,
        output: None,
        cache_read: None,
        cache_write: None,
    };

    /// `true` when no field is known.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        *self == Self::UNKNOWN
    }

    /// Field-wise sum. A field is known only when it is known on both sides and the sum does
    /// not overflow; otherwise it becomes unknown (never silently saturated or zeroed).
    #[must_use]
    pub fn checked_add(self, other: Self) -> Self {
        fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            a?.checked_add(b?)
        }
        Self {
            input: add(self.input, other.input),
            output: add(self.output, other.output),
            cache_read: add(self.cache_read, other.cache_read),
            cache_write: add(self.cache_write, other.cache_write),
        }
    }

    /// Sum of the known fields, or `None` when every field is unknown or the sum overflows.
    #[must_use]
    pub fn known_total(&self) -> Option<u64> {
        let mut total: Option<u64> = None;
        for value in [self.input, self.output, self.cache_read, self.cache_write]
            .into_iter()
            .flatten()
        {
            total = Some(total.unwrap_or(0).checked_add(value)?);
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp(1_000_000);

    fn window(percent: Option<f64>, resets_at: Option<i64>) -> UsageWindow {
        UsageWindow {
            used_percent: percent,
            window_minutes: Some(300),
            resets_at: resets_at.map(Timestamp),
        }
    }

    #[test]
    fn unknown_usage_is_neither_exhausted_nor_zero() {
        let usage = AccountUsage::default();
        assert_eq!(usage.max_used_percent(), None);
        assert_eq!(usage.current_used_percent(NOW), None);
        assert!(!usage.exhausted(NOW, 99.0));
        assert!(!usage.window_reset(NOW));
        assert_eq!(usage.available_at(NOW, 99.0), None);
    }

    #[test]
    fn threshold_and_reset_semantics() {
        let usage = AccountUsage {
            primary: Some(window(Some(99.0), Some(2_000_000))),
            secondary: Some(window(Some(40.0), Some(9_000_000))),
            ..AccountUsage::default()
        };
        assert_eq!(usage.max_used_percent(), Some(99.0));
        assert!(usage.exhausted(NOW, 99.0));
        assert!(!usage.exhausted(NOW, 99.5));
        assert_eq!(usage.available_at(NOW, 99.0), Some(Timestamp(2_000_000)));
        // After the primary window resets it no longer counts.
        let later = Timestamp(2_000_000);
        assert!(!usage.exhausted(later, 99.0));
        assert!(usage.window_reset(later));
        assert!(usage.recovered_by_reset(later, 99.0));
        assert_eq!(usage.current_used_percent(later), Some(40.0));
    }

    #[test]
    fn reached_limit_counts_until_its_reset() {
        let usage = AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(Timestamp(1_500_000)),
            ..AccountUsage::default()
        };
        assert!(usage.limit_in_effect(NOW));
        assert!(usage.exhausted(NOW, 99.0));
        assert_eq!(usage.available_at(NOW, 99.0), Some(Timestamp(1_500_000)));
        assert!(!usage.exhausted(Timestamp(1_500_000), 99.0));
        assert!(usage.recovered_by_reset(Timestamp(1_500_000), 99.0));

        let unknown_reset = AccountUsage {
            limit_reached: true,
            ..AccountUsage::default()
        };
        assert!(unknown_reset.exhausted(Timestamp(i64::MAX), 99.0));
        assert_eq!(unknown_reset.available_at(NOW, 99.0), None);
    }

    #[test]
    fn available_at_is_latest_blocking_reset_and_unknown_if_any_is_unknown() {
        let both = AccountUsage {
            primary: Some(window(Some(100.0), Some(2_000_000))),
            secondary: Some(window(Some(100.0), Some(5_000_000))),
            ..AccountUsage::default()
        };
        assert_eq!(both.available_at(NOW, 99.0), Some(Timestamp(5_000_000)));
        let one_unknown = AccountUsage {
            primary: Some(window(Some(100.0), Some(2_000_000))),
            secondary: Some(window(Some(100.0), None)),
            ..AccountUsage::default()
        };
        assert_eq!(one_unknown.available_at(NOW, 99.0), None);
    }

    #[test]
    fn non_finite_percent_is_unknown() {
        let usage = AccountUsage {
            primary: Some(window(Some(f64::NAN), None)),
            ..AccountUsage::default()
        };
        assert_eq!(usage.max_used_percent(), None);
        assert!(!usage.exhausted(NOW, 99.0));
    }

    #[test]
    fn token_usage_unknown_is_not_zero() {
        let known = TokenUsage {
            input: Some(10),
            output: Some(5),
            cache_read: Some(100),
            cache_write: None,
        };
        let sum = known.checked_add(known);
        assert_eq!(sum.input, Some(20));
        assert_eq!(sum.cache_read, Some(200));
        assert_eq!(sum.cache_write, None);
        assert_eq!(known.checked_add(TokenUsage::UNKNOWN), TokenUsage::UNKNOWN);
        assert!(TokenUsage::default().is_unknown());
        assert_eq!(TokenUsage::UNKNOWN.known_total(), None);
        assert_eq!(known.known_total(), Some(115));
    }

    #[test]
    fn token_usage_overflow_becomes_unknown() {
        let big = TokenUsage {
            input: Some(u64::MAX),
            output: Some(1),
            cache_read: None,
            cache_write: None,
        };
        let sum = big.checked_add(big);
        assert_eq!(sum.input, None);
        assert_eq!(sum.output, Some(2));
        assert_eq!(big.known_total(), None);
    }
}
