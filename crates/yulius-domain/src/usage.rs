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

/// The five-hour session window and the weekly window, in minutes.
pub const SESSION_WINDOW_MINUTES: u32 = 5 * 60;
/// See [`SESSION_WINDOW_MINUTES`].
pub const WEEKLY_WINDOW_MINUTES: u32 = 7 * 24 * 60;

/// Which model family a task runs, for the weekly windows of its own: the per-model caps
/// (`seven_day_opus`, `seven_day_sonnet`) and the weekly allowance of usage credits the Fable
/// models run on (`seven_day_overage_included`, Claude's "Fable limit"). A task of another model
/// is bound only by the account-wide windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ModelFamily {
    /// No per-model window applies (Haiku, unknown names).
    #[default]
    Any,
    /// An Opus model: the weekly Opus cap binds too.
    Opus,
    /// A Sonnet model: the weekly Sonnet cap binds too.
    Sonnet,
    /// A Fable model: its weekly allowance of usage credits binds too (SPEC-464).
    Fable,
}

impl ModelFamily {
    /// The families that have a weekly window of their own.
    pub const WITH_WINDOW: [Self; 3] = [Self::Opus, Self::Sonnet, Self::Fable];

    /// The family of a task's model setting (`None` = unknown; callers pass the default model
    /// of a task that names none).
    #[must_use]
    pub fn of(model: Option<&str>) -> Self {
        let Some(model) = model else {
            return Self::Any;
        };
        let lower = model.to_ascii_lowercase();
        if lower.contains("opus") {
            Self::Opus
        } else if lower.contains("sonnet") {
            Self::Sonnet
        } else if lower.contains("fable") {
            Self::Fable
        } else {
            Self::Any
        }
    }
}

/// What a usage limit Claude reported binds (SPEC-464).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LimitScope {
    /// The subscription: its 5-hour or weekly window, or a limit whose kind the report does not
    /// say.
    #[default]
    Account,
    /// The models of one family only: their weekly cap or their allowance of usage credits.
    /// `None` when the report does not name the family: that of the model the session runs.
    Model(Option<ModelFamily>),
}

/// A usage limit as Claude reported it to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReportedLimit {
    /// What it binds.
    pub scope: LimitScope,
    /// Length of the limited window in minutes, when the report said which
    /// ([`SESSION_WINDOW_MINUTES`] or [`WEEKLY_WINDOW_MINUTES`]).
    pub window_minutes: Option<u32>,
    /// When it resets; `None` when the report did not say.
    pub resets_at: Option<Timestamp>,
}

/// An account's latest usage snapshot. `Default` is "nothing known".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AccountUsage {
    /// Short window (Claude's 5-hour window, started by the first request); `None` when not
    /// reported.
    pub primary: Option<UsageWindow>,
    /// Long window (Claude's weekly window, a fixed reset per account); `None` when not
    /// reported.
    pub secondary: Option<UsageWindow>,
    /// Weekly cap of Opus models (`seven_day_opus`); binds only tasks that run Opus.
    pub weekly_opus: Option<UsageWindow>,
    /// Weekly cap of Sonnet models (`seven_day_sonnet`); binds only tasks that run Sonnet.
    pub weekly_sonnet: Option<UsageWindow>,
    /// Weekly allowance of usage credits the Fable models run on (`seven_day_overage_included`);
    /// binds only tasks that run Fable. No usage reading carries it: it is known from the limit
    /// Claude reports once it is used up (SPEC-464).
    pub weekly_fable: Option<UsageWindow>,
    /// The provider reported that a usage limit was reached.
    pub limit_reached: bool,
    /// When the reached limit resets; `None` when unknown.
    pub limit_resets_at: Option<Timestamp>,
    /// When this snapshot was observed; `None` when unknown.
    pub observed_at: Option<Timestamp>,
}

impl AccountUsage {
    /// The weekly window of `family`'s own (its cap, or its allowance of usage credits); `None`
    /// for a family without one and while it is not known.
    #[must_use]
    pub fn model_window(&self, family: ModelFamily) -> Option<&UsageWindow> {
        match family {
            ModelFamily::Any => None,
            ModelFamily::Opus => self.weekly_opus.as_ref(),
            ModelFamily::Sonnet => self.weekly_sonnet.as_ref(),
            ModelFamily::Fable => self.weekly_fable.as_ref(),
        }
    }

    fn model_slot(&mut self, family: ModelFamily) -> Option<&mut Option<UsageWindow>> {
        match family {
            ModelFamily::Any => None,
            ModelFamily::Opus => Some(&mut self.weekly_opus),
            ModelFamily::Sonnet => Some(&mut self.weekly_sonnet),
            ModelFamily::Fable => Some(&mut self.weekly_fable),
        }
    }

    /// The windows that bind a task of `family`: the 5-hour and weekly windows, plus the
    /// family's own weekly window.
    pub fn binding_windows(&self, family: ModelFamily) -> impl Iterator<Item = &UsageWindow> {
        self.primary
            .iter()
            .chain(self.secondary.iter())
            .chain(self.model_window(family))
    }

    /// The weekly windows that bind a task of `family` (account-wide and the family's own).
    pub fn weekly_windows(&self, family: ModelFamily) -> impl Iterator<Item = &UsageWindow> {
        self.secondary.iter().chain(self.model_window(family))
    }

    /// Records a usage limit Claude reported at `now` to a session that runs a model of
    /// `family`. What the CLI was told is the account's state (ROT-009), also when no usage
    /// request answers:
    ///
    /// - a limit of the subscription fills its 5-hour or weekly window until the reset and is a
    ///   limit in effect ([`Self::limit_in_effect`]);
    /// - a limit of one model family, with its reset, fills that family's weekly window: the
    ///   subscription's own windows keep their readings and other models still run on it
    ///   (SPEC-464);
    /// - a model's limit without a reset, or of a family without a window, is a limit in effect
    ///   with no reset, as before: the next usage reading ends it.
    pub fn record_limit(&mut self, limit: ReportedLimit, family: ModelFamily, now: Timestamp) {
        self.observed_at = Some(now);
        let full = |minutes: u32| UsageWindow {
            used_percent: Some(100.0),
            window_minutes: Some(minutes),
            resets_at: limit.resets_at,
        };
        if let LimitScope::Model(named) = limit.scope {
            if limit.resets_at.is_some()
                && let Some(slot) = self.model_slot(named.unwrap_or(family))
            {
                *slot = Some(full(WEEKLY_WINDOW_MINUTES));
                return;
            }
            self.limit_reached = true;
            self.limit_resets_at = limit.resets_at;
            return;
        }
        self.limit_reached = true;
        self.limit_resets_at = limit.resets_at;
        match limit.window_minutes {
            Some(minutes) if minutes <= SESSION_WINDOW_MINUTES => {
                self.primary = Some(full(minutes));
            }
            Some(minutes) => self.secondary = Some(full(minutes)),
            None => {}
        }
    }

    /// This reading with what no reading shows kept from `stored`: the weekly window of a model
    /// family that the reading does not carry stays while its reset is ahead. It is a limit
    /// Claude reported for that model, or the provider's last word on its cap; a reading of the
    /// 5-hour and weekly windows says nothing about either (SPEC-464).
    #[must_use]
    pub fn keeping_unread(mut self, stored: &Self, now: Timestamp) -> Self {
        for family in ModelFamily::WITH_WINDOW {
            let kept = stored
                .model_window(family)
                .copied()
                .filter(|w| w.resets_at.is_some_and(|at| at > now));
            if let Some(slot) = self.model_slot(family)
                && slot.is_none()
            {
                *slot = kept;
            }
        }
        self
    }

    /// Without the per-model windows whose reset has passed: a new window began, and nothing is
    /// known about it yet.
    #[must_use]
    pub fn without_reset_model_windows(mut self, now: Timestamp) -> Self {
        for family in ModelFamily::WITH_WINDOW {
            if let Some(slot) = self.model_slot(family)
                && slot.is_some_and(|w| w.is_reset(now))
            {
                *slot = None;
            }
        }
        self
    }

    fn windows(&self) -> impl Iterator<Item = &UsageWindow> {
        self.binding_windows(ModelFamily::Any)
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
        self.current_used_percent_for(now, ModelFamily::Any)
    }

    /// [`Self::current_used_percent`] over the windows binding `family`.
    #[must_use]
    pub fn current_used_percent_for(&self, now: Timestamp, family: ModelFamily) -> Option<f64> {
        self.binding_windows(family)
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
        self.exhausted_for(now, threshold, ModelFamily::Any)
    }

    /// [`Self::exhausted`] for a task of `family` (its model's weekly cap binds too).
    #[must_use]
    pub fn exhausted_for(&self, now: Timestamp, threshold: f64, family: ModelFamily) -> bool {
        self.limit_in_effect(now)
            || self
                .current_used_percent_for(now, family)
                .is_some_and(|p| p >= threshold)
    }

    /// `true` when the snapshot contains a reached limit or a window whose known reset time has
    /// passed, so the snapshot overstates current usage.
    #[must_use]
    pub fn window_reset(&self, now: Timestamp) -> bool {
        (self.limit_reached && self.limit_resets_at.is_some_and(|at| at <= now))
            || self.windows().any(|w| w.is_reset(now))
    }

    /// When the account stops being exhausted: the latest reset among all causes still in
    /// effect at `now`. `None` when it is not exhausted or a blocking cause has no known reset.
    #[must_use]
    pub fn available_at(&self, now: Timestamp, threshold: f64) -> Option<Timestamp> {
        self.available_at_for(now, threshold, ModelFamily::Any)
    }

    /// [`Self::available_at`] for a task of `family`.
    #[must_use]
    pub fn available_at_for(
        &self,
        now: Timestamp,
        threshold: f64,
        family: ModelFamily,
    ) -> Option<Timestamp> {
        if !self.exhausted_for(now, threshold, family) {
            return None;
        }
        let mut latest: Option<Timestamp> = None;
        if self.limit_in_effect(now) {
            latest = Some(self.limit_resets_at?);
        }
        for window in self.binding_windows(family) {
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

        let unknown_reset = AccountUsage {
            limit_reached: true,
            ..AccountUsage::default()
        };
        assert!(unknown_reset.exhausted(Timestamp(i64::MAX), 99.0));
        assert_eq!(unknown_reset.available_at(NOW, 99.0), None);
    }

    fn model_limit(family: Option<ModelFamily>, resets_at: Option<i64>) -> ReportedLimit {
        ReportedLimit {
            scope: LimitScope::Model(family),
            window_minutes: Some(WEEKLY_WINDOW_MINUTES),
            resets_at: resets_at.map(Timestamp),
        }
    }

    /// SPEC-464 UCL-003: a model's limit fills that model's weekly window until the reset Claude
    /// named. The subscription's own windows keep their readings, other models still run on it,
    /// and it is usable for that model again the moment the window resets.
    #[test]
    fn a_models_limit_is_its_own_window() {
        let week = window(Some(61.0), Some(9_000_000));
        let mut usage = AccountUsage {
            secondary: Some(week),
            ..AccountUsage::default()
        };
        // The family the report names wins over the one the session runs.
        usage.record_limit(
            model_limit(Some(ModelFamily::Fable), Some(5_000_000)),
            ModelFamily::Opus,
            NOW,
        );
        assert_eq!(
            usage.weekly_fable,
            Some(UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(WEEKLY_WINDOW_MINUTES),
                resets_at: Some(Timestamp(5_000_000)),
            })
        );
        assert_eq!(usage.secondary, Some(week), "the week keeps its reading");
        assert_eq!(usage.weekly_opus, None);
        assert!(!usage.limit_reached && !usage.limit_in_effect(NOW));
        assert_eq!(usage.observed_at, Some(NOW));
        assert!(usage.exhausted_for(NOW, 98.0, ModelFamily::Fable));
        assert!(!usage.exhausted_for(NOW, 98.0, ModelFamily::Opus));
        assert!(!usage.exhausted(NOW, 98.0));
        assert_eq!(
            usage.available_at_for(NOW, 98.0, ModelFamily::Fable),
            Some(Timestamp(5_000_000))
        );
        assert!(!usage.exhausted_for(Timestamp(5_000_000), 98.0, ModelFamily::Fable));

        // A report that names no family: the model the session runs.
        let mut unnamed = AccountUsage::default();
        unnamed.record_limit(model_limit(None, Some(5_000_000)), ModelFamily::Opus, NOW);
        assert!(unnamed.weekly_opus.is_some() && unnamed.weekly_fable.is_none());
        assert!(!unnamed.limit_reached);

        // No reset time, or a family without a window of its own: a limit of the subscription
        // with what is known, which the next reading ends.
        let mut no_reset = AccountUsage::default();
        no_reset.record_limit(
            model_limit(Some(ModelFamily::Fable), None),
            ModelFamily::Fable,
            NOW,
        );
        assert_eq!(no_reset.weekly_fable, None);
        assert!(no_reset.limit_in_effect(Timestamp(i64::MAX)));
        assert_eq!(no_reset.secondary, None, "never the week");
        let mut no_window = AccountUsage::default();
        no_window.record_limit(model_limit(None, Some(5_000_000)), ModelFamily::Any, NOW);
        assert!(no_window.limit_in_effect(NOW) && !no_window.limit_in_effect(Timestamp(5_000_000)));
        assert_eq!(no_window.secondary, None);
    }

    /// A limit of the subscription fills its 5-hour or weekly window, as it always did.
    #[test]
    fn a_subscriptions_limit_fills_its_window() {
        let account_limit = |minutes: Option<u32>| ReportedLimit {
            scope: LimitScope::Account,
            window_minutes: minutes,
            resets_at: Some(Timestamp(2_000_000)),
        };
        let mut five = AccountUsage::default();
        five.record_limit(
            account_limit(Some(SESSION_WINDOW_MINUTES)),
            ModelFamily::Fable,
            NOW,
        );
        assert_eq!(five.primary.and_then(|w| w.used_percent), Some(100.0));
        assert_eq!(
            five.primary.and_then(|w| w.resets_at),
            Some(Timestamp(2_000_000))
        );
        assert_eq!(five.secondary, None);
        assert_eq!(five.weekly_fable, None);
        assert!(five.limit_in_effect(NOW));
        assert_eq!(five.limit_resets_at, Some(Timestamp(2_000_000)));
        let mut week = AccountUsage::default();
        week.record_limit(
            account_limit(Some(WEEKLY_WINDOW_MINUTES)),
            ModelFamily::Any,
            NOW,
        );
        assert_eq!(
            week.secondary.and_then(|w| w.window_minutes),
            Some(WEEKLY_WINDOW_MINUTES)
        );
        assert_eq!(week.primary, None);
        let mut unknown = AccountUsage::default();
        unknown.record_limit(account_limit(None), ModelFamily::Any, NOW);
        assert!(unknown.limit_in_effect(NOW));
        assert_eq!((unknown.primary, unknown.secondary), (None, None));
    }

    /// SPEC-464 UCL-004: a usage reading replaces the windows it carries and keeps the per-model
    /// windows it cannot see while their reset is ahead (the model's limit was wiped by the next
    /// reading, and a task was placed back onto the exhausted subscription).
    #[test]
    fn a_reading_keeps_the_model_windows_it_cannot_see() {
        let full = |resets_at: Option<i64>| UsageWindow {
            used_percent: Some(100.0),
            window_minutes: Some(WEEKLY_WINDOW_MINUTES),
            resets_at: resets_at.map(Timestamp),
        };
        let stored = AccountUsage {
            secondary: Some(full(Some(9_000_000))),
            weekly_fable: Some(full(Some(5_000_000))),
            // Its reset passed, and one whose reset is unknown: nothing says they still hold.
            weekly_opus: Some(full(Some(900_000))),
            weekly_sonnet: Some(full(None)),
            limit_reached: true,
            ..AccountUsage::default()
        };
        let reading = AccountUsage {
            secondary: Some(window(Some(61.0), Some(9_000_000))),
            observed_at: Some(NOW),
            ..AccountUsage::default()
        };
        let merged = reading.keeping_unread(&stored, NOW);
        assert_eq!(merged.weekly_fable, stored.weekly_fable);
        assert_eq!(
            merged.secondary, reading.secondary,
            "what it reads is replaced"
        );
        assert_eq!((merged.weekly_opus, merged.weekly_sonnet), (None, None));
        assert!(
            !merged.limit_reached,
            "a reading still ends a limit of the subscription"
        );
        assert!(merged.exhausted_for(NOW, 98.0, ModelFamily::Fable));
        // A reading that carries the window is the newer word.
        let carried = AccountUsage {
            weekly_fable: Some(window(Some(20.0), Some(8_000_000))),
            ..reading
        };
        assert_eq!(
            carried.keeping_unread(&stored, NOW).weekly_fable,
            carried.weekly_fable
        );
        // At its reset the kept window is gone: nothing is known about the new one.
        assert_eq!(
            reading
                .keeping_unread(&stored, Timestamp(5_000_000))
                .weekly_fable,
            None
        );
        assert_eq!(
            stored.without_reset_model_windows(NOW).weekly_fable,
            stored.weekly_fable
        );
        let later = stored.without_reset_model_windows(Timestamp(5_000_000));
        assert_eq!((later.weekly_fable, later.weekly_opus), (None, None));
        assert_eq!(
            later.weekly_sonnet, stored.weekly_sonnet,
            "an unknown reset is not a passed one"
        );
        assert_eq!(later.secondary, stored.secondary);
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
