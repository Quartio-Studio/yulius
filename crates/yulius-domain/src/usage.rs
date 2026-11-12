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

/// A window at this many percent is used up.
pub const FULL_PERCENT: f64 = 100.0;

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

/// The furthest ahead a reset a session reports is believed: a week and a day. No window lasts
/// longer, and both a limit's reset (from the session's transcript, which is the agent's own
/// file) and a session's usage reading (SPEC-398 VSH-005) come from the agent's side.
pub const LIMIT_RESET_MAX_MS: i64 = 8 * 24 * 3_600_000;

/// Claude's name of the weekly allowance of usage credits included for a model that otherwise
/// runs on them (`rateLimitType` in Claude Code's limit record, the claim of a refused
/// response): a bucket per model, which Claude Code 2.1.322 calls the "Fable limit" after the
/// one model that has it today.
pub const CREDITS_ALLOWANCE_CLAIM: &str = "seven_day_overage_included";

impl ReportedLimit {
    /// The limit a refusal names (SPEC-464, SPEC-473). `claim` is Claude's name of the limited
    /// window (`five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`,
    /// [`CREDITS_ALLOWANCE_CLAIM`]), `claim_reset` the reset given for it, and `credits` says
    /// that the model was refused for its usage credits.
    ///
    /// The 5-hour and weekly windows are the subscription's, and so is a claim this does not
    /// know. A per-model cap is that family's. The allowance of usage credits is the bucket of
    /// the model the request ran, whichever that is. A refusal for usage credits is the model's
    /// whichever window the response claimed, and then only the allowance's own reset says
    /// when the model runs again: another claim's window and reset are dropped.
    #[must_use]
    pub fn of_refusal(claim: Option<&str>, credits: bool, claim_reset: Option<Timestamp>) -> Self {
        let named = match claim {
            Some("seven_day_opus") => LimitScope::Model(Some(ModelFamily::Opus)),
            Some("seven_day_sonnet") => LimitScope::Model(Some(ModelFamily::Sonnet)),
            Some(CREDITS_ALLOWANCE_CLAIM) => LimitScope::Model(None),
            _ => LimitScope::Account,
        };
        let window_minutes = match claim {
            Some("five_hour") => Some(SESSION_WINDOW_MINUTES),
            Some(kind) if kind.starts_with("seven_day") => Some(WEEKLY_WINDOW_MINUTES),
            _ => None,
        };
        let own = !credits || claim == Some(CREDITS_ALLOWANCE_CLAIM);
        Self {
            scope: if credits && named == LimitScope::Account {
                LimitScope::Model(None)
            } else {
                named
            },
            window_minutes: window_minutes.filter(|_| own),
            resets_at: claim_reset.filter(|_| own),
        }
    }

    /// When the limit resets, if the report says so believably: at most
    /// [`LIMIT_RESET_MAX_MS`] ahead of `now`.
    #[must_use]
    pub fn believed_reset(&self, now: Timestamp) -> Option<Timestamp> {
        self.resets_at
            .filter(|at| at.millis_since(now) <= LIMIT_RESET_MAX_MS)
    }

    /// Whether this is a model's limit nothing says the end of: the report names no reset that
    /// is believed, and no usage reading shows a model's allowance of usage credits. A limit of
    /// the subscription is not one: a provider's reading shows its windows (SPEC-464 UCL-010).
    #[must_use]
    pub fn without_an_end(&self, now: Timestamp) -> bool {
        matches!(self.scope, LimitScope::Model(_)) && self.believed_reset(now).is_none()
    }
}

/// What Claude answered a request of one model on a subscription, asked to learn whether a limit
/// stored for that model still holds (SPEC-473).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModelAnswer {
    /// The usage limit that refused the request; `None` when the model ran.
    pub refused: Option<ReportedLimit>,
    /// The 5-hour window as the answer read it, when it carried one.
    pub five_hour: Option<UsageWindow>,
    /// The weekly window as the answer read it, when it carried one.
    pub seven_day: Option<UsageWindow>,
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
    /// When the subscription's own windows were last read: by the provider, by a session
    /// whose reading carried both, or with an answer about a model that carried both
    /// (SPEC-473 RFR-009). A limit Claude reports and a reading of one window change what is
    /// stored without being a reading of the usage. `None` when it was never read.
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
    /// - a limit of one model family fills that family's weekly window until its reset: the
    ///   subscription's own windows keep their readings and other models still run on it
    ///   (SPEC-464). Without a reset the window is full until a provider's reading ends it
    ///   ([`Self::keeping_unread`]);
    /// - a model's limit of a family without a window is a limit of the subscription, its
    ///   windows untouched.
    ///
    /// The reset is the one the report says believably ([`ReportedLimit::believed_reset`]).
    /// A reported limit is not a reading of the usage: [`Self::observed_at`] stays.
    pub fn record_limit(&mut self, limit: ReportedLimit, family: ModelFamily, now: Timestamp) {
        let resets_at = limit.believed_reset(now);
        let full = |minutes: u32| UsageWindow {
            used_percent: Some(100.0),
            window_minutes: Some(minutes),
            resets_at,
        };
        if let LimitScope::Model(named) = limit.scope {
            if let Some(slot) = self.model_slot(named.unwrap_or(family)) {
                *slot = Some(full(WEEKLY_WINDOW_MINUTES));
            } else {
                self.limit_reached = true;
                self.limit_resets_at = resets_at;
            }
            return;
        }
        self.limit_reached = true;
        self.limit_resets_at = resets_at;
        match limit.window_minutes {
            Some(minutes) if minutes <= SESSION_WINDOW_MINUTES => {
                self.primary = Some(full(minutes));
            }
            Some(minutes) => self.secondary = Some(full(minutes)),
            None => {}
        }
    }

    /// Records Claude's answer at `now` to a request of a model of `family`, asked to learn
    /// whether a limit stored for that model still holds (SPEC-473). The 5-hour and weekly
    /// windows the answer carried replace the stored ones, and an answer that carried both is
    /// a reading of the usage ([`Self::observed_at`]).
    ///
    /// - The model ran: a limit of the subscription ends, and so does the family's own
    ///   window. Nothing is stored in its place, whatever the answer says of the model's
    ///   allowance: a model that runs with its allowance used up runs on usage credits a
    ///   person bought, and no reading would replace a window stored below its limit.
    /// - It was refused: recorded like a limit a session reports ([`Self::record_limit`]), so
    ///   a window stored without a reset gets the one the answer names. An answer that names
    ///   no believable reset keeps the one stored for the window it fills, while that is
    ///   ahead.
    pub fn record_model_answer(
        &mut self,
        answer: ModelAnswer,
        family: ModelFamily,
        now: Timestamp,
    ) {
        if answer.five_hour.is_some() && answer.seven_day.is_some() {
            self.observed_at = Some(now);
        }
        self.primary = answer.five_hour.or(self.primary);
        self.secondary = answer.seven_day.or(self.secondary);
        let Some(limit) = answer.refused else {
            self.limit_reached = false;
            self.limit_resets_at = None;
            if let Some(slot) = self.model_slot(family) {
                *slot = None;
            }
            return;
        };
        let stored = match limit.scope {
            LimitScope::Model(named) => self
                .model_window(named.unwrap_or(family))
                .filter(|w| !w.is_reset(now))
                .and_then(|w| w.resets_at),
            LimitScope::Account => None,
        };
        let resets_at = limit.believed_reset(now).or(stored);
        self.record_limit(ReportedLimit { resets_at, ..limit }, family, now);
    }

    /// The model families Claude is asked about to learn whether a stored limit still holds at
    /// `now` (SPEC-473): each family whose own weekly window is used up, and `default` (the
    /// default model's family) when a limit of the whole subscription is in effect. The
    /// subscription's 5-hour and weekly windows are not asked about: a usage reading shows
    /// them.
    #[must_use]
    pub fn families_at_a_limit(&self, now: Timestamp, default: ModelFamily) -> Vec<ModelFamily> {
        let mut families: Vec<ModelFamily> = ModelFamily::WITH_WINDOW
            .into_iter()
            .filter(|family| {
                self.model_window(*family)
                    .and_then(|w| w.current_percent(now))
                    .is_some_and(|p| p >= FULL_PERCENT)
            })
            .collect();
        if self.limit_in_effect(now) && !families.contains(&default) {
            families.push(default);
        }
        families
    }

    /// This reading with what it does not show kept from `stored`: the weekly window of a model
    /// family that the reading does not carry stays while its reset is ahead, and no further
    /// than a window lasts ([`LIMIT_RESET_MAX_MS`]). It is a limit Claude reported for that
    /// model, or the provider's last word on its cap; a reading of the 5-hour and weekly
    /// windows says nothing about either (SPEC-464). A window without a reset is not kept: a
    /// provider's reading is what ends a model's limit nothing says the end of.
    #[must_use]
    pub fn keeping_unread(mut self, stored: &Self, now: Timestamp) -> Self {
        for family in ModelFamily::WITH_WINDOW {
            let kept = stored.model_window(family).copied().filter(|w| {
                w.resets_at
                    .is_some_and(|at| at > now && at.millis_since(now) <= LIMIT_RESET_MAX_MS)
            });
            if let Some(slot) = self.model_slot(family)
                && slot.is_none()
            {
                *slot = kept;
            }
        }
        self
    }

    /// [`Self::keeping_unread`] for a reading after which Claude is asked about each limit that
    /// is stored (SPEC-473): a per-model window stored used up without a reset is kept too.
    /// There the answer ends such a limit or gives it its reset, not the reading.
    #[must_use]
    pub fn keeping_asked(self, stored: &Self, now: Timestamp) -> Self {
        let mut kept = self.keeping_unread(stored, now);
        for family in ModelFamily::WITH_WINDOW {
            let blind = stored.model_window(family).copied().filter(|w| {
                w.resets_at.is_none() && w.known_percent().is_some_and(|p| p >= FULL_PERCENT)
            });
            if let Some(slot) = kept.model_slot(family)
                && slot.is_none()
            {
                *slot = blind;
            }
        }
        kept
    }

    /// What is stored about the limits an answer for a model of `family` speaks of: the
    /// family's own window, and the limit of the whole subscription with its reset. Equal
    /// before and after a question when no limit was recorded meanwhile (SPEC-473 RFR-005).
    #[must_use]
    pub fn stored_limit(
        &self,
        family: ModelFamily,
    ) -> (Option<UsageWindow>, bool, Option<Timestamp>) {
        (
            self.model_window(family).copied(),
            self.limit_reached,
            self.limit_resets_at,
        )
    }

    /// Without the per-model windows: they are what Claude told one sign-in, and a new sign-in
    /// of the subscription knows nothing of them (SPEC-464).
    #[must_use]
    pub fn without_model_windows(mut self) -> Self {
        for family in ModelFamily::WITH_WINDOW {
            if let Some(slot) = self.model_slot(family) {
                *slot = None;
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
        let read = Some(Timestamp(900_000));
        let mut usage = AccountUsage {
            secondary: Some(week),
            observed_at: read,
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
        assert_eq!(
            usage.observed_at, read,
            "a reported limit is not a reading of the usage (SPEC-473 RFR-009)"
        );
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

        // No reset time: still the model's own window, full until a provider's reading ends it
        // (it keeps no window without a reset), so other models run on the subscription.
        let mut no_reset = AccountUsage::default();
        no_reset.record_limit(
            model_limit(Some(ModelFamily::Fable), None),
            ModelFamily::Fable,
            NOW,
        );
        assert_eq!(
            no_reset.weekly_fable,
            Some(UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(WEEKLY_WINDOW_MINUTES),
                resets_at: None,
            })
        );
        assert!(!no_reset.limit_reached, "not the whole subscription's");
        assert_eq!(no_reset.secondary, None, "never the week");
        assert!(no_reset.exhausted_for(Timestamp(i64::MAX), 98.0, ModelFamily::Fable));
        assert!(!no_reset.exhausted_for(NOW, 98.0, ModelFamily::Opus));
        assert_eq!(
            no_reset.available_at_for(NOW, 98.0, ModelFamily::Fable),
            None
        );
        let read = AccountUsage::default().keeping_unread(&no_reset, NOW);
        assert_eq!(read.weekly_fable, None, "a provider's reading ends it");
        // A family without a window of its own: a limit of the subscription, its windows
        // untouched.
        let mut no_window = AccountUsage::default();
        no_window.record_limit(model_limit(None, Some(5_000_000)), ModelFamily::Any, NOW);
        assert!(no_window.limit_in_effect(NOW) && !no_window.limit_in_effect(Timestamp(5_000_000)));
        assert_eq!(no_window.secondary, None);
    }

    /// SPEC-464 (review round 1): a limit's reset comes from the session's transcript, which is
    /// the agent's own file. A reset further ahead than any window lasts is not believed (a week
    /// and a day, as for a session's reading, SPEC-398 VSH-005): the limit is recorded without a
    /// reset, and a provider's reading ends it. A forged reset in 2286 kept a model off a
    /// subscription for good, across every reading.
    #[test]
    fn a_limits_reset_is_believed_only_as_far_as_a_window_lasts() {
        let far = Timestamp(NOW.0 + LIMIT_RESET_MAX_MS + 1);
        let edge = Timestamp(NOW.0 + LIMIT_RESET_MAX_MS);
        let full = |resets_at: Option<Timestamp>| UsageWindow {
            used_percent: Some(100.0),
            window_minutes: Some(WEEKLY_WINDOW_MINUTES),
            resets_at,
        };
        let reading = AccountUsage {
            secondary: Some(window(Some(61.0), Some(9_000_000))),
            ..AccountUsage::default()
        };
        let fable = ModelFamily::Fable;

        let mut forged = AccountUsage::default();
        forged.record_limit(model_limit(Some(fable), Some(far.0)), fable, NOW);
        assert_eq!(forged.weekly_fable, Some(full(None)), "no reset is kept");
        assert_eq!(reading.keeping_unread(&forged, NOW).weekly_fable, None);
        assert_eq!(
            model_limit(Some(fable), Some(far.0)).believed_reset(NOW),
            None
        );

        let mut real = AccountUsage::default();
        real.record_limit(model_limit(Some(fable), Some(edge.0)), fable, NOW);
        assert_eq!(real.weekly_fable, Some(full(Some(edge))));
        assert_eq!(
            reading.keeping_unread(&real, NOW).weekly_fable,
            Some(full(Some(edge)))
        );
        // A reset that passed is believed: the window it ends counts no longer.
        assert_eq!(
            model_limit(Some(fable), Some(NOW.0 - 1)).believed_reset(NOW),
            Some(Timestamp(NOW.0 - 1))
        );

        // UCL-010: a model's limit nothing says the end of is one with no believed reset. The
        // subscription's own limit is none, whatever its reset: a reading shows its windows.
        assert!(model_limit(Some(fable), None).without_an_end(NOW));
        assert!(model_limit(None, None).without_an_end(NOW));
        assert!(model_limit(Some(fable), Some(far.0)).without_an_end(NOW));
        assert!(!model_limit(Some(fable), Some(edge.0)).without_an_end(NOW));
        for resets_at in [None, Some(far), Some(edge)] {
            let own = ReportedLimit {
                scope: LimitScope::Account,
                window_minutes: Some(SESSION_WINDOW_MINUTES),
                resets_at,
            };
            assert!(!own.without_an_end(NOW), "{resets_at:?}");
        }

        // A window stored with such a reset (before this bound) is not kept by a reading.
        let stored = AccountUsage {
            weekly_fable: Some(full(Some(far))),
            ..AccountUsage::default()
        };
        assert_eq!(reading.keeping_unread(&stored, NOW).weekly_fable, None);

        // The subscription's own limit: its window and the limit carry no reset either.
        let mut account = AccountUsage::default();
        account.record_limit(
            ReportedLimit {
                scope: LimitScope::Account,
                window_minutes: Some(WEEKLY_WINDOW_MINUTES),
                resets_at: Some(far),
            },
            ModelFamily::Any,
            NOW,
        );
        assert!(account.limit_reached);
        assert_eq!(account.limit_resets_at, None);
        assert_eq!(account.secondary, Some(full(None)));
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
        // A new sign-in knows none of them, whatever their reset; the rest stays.
        let fresh = stored.without_model_windows();
        assert_eq!(
            (fresh.weekly_fable, fresh.weekly_opus, fresh.weekly_sonnet),
            (None, None, None)
        );
        assert_eq!(fresh.secondary, stored.secondary);
        assert!(fresh.limit_reached);
        let later = stored.without_reset_model_windows(Timestamp(5_000_000));
        assert_eq!((later.weekly_fable, later.weekly_opus), (None, None));
        assert_eq!(
            later.weekly_sonnet, stored.weekly_sonnet,
            "an unknown reset is not a passed one"
        );
        assert_eq!(later.secondary, stored.secondary);
    }

    /// SPEC-473 RFR-001: a claim says what the limit binds and how long its window is, for the
    /// harness (Claude Code's `rateLimitType`) and the probe (the response's claim) alike. A
    /// refusal for usage credits is the model's whatever window the response claimed, and that
    /// window's reset is not when the model runs again.
    #[test]
    fn a_claim_says_what_a_refusal_binds() {
        let reset = Some(Timestamp(5_000_000));
        let of = |claim: Option<&str>, credits: bool| {
            let limit = ReportedLimit::of_refusal(claim, credits, reset);
            (limit.scope, limit.window_minutes, limit.resets_at)
        };
        let week = Some(WEEKLY_WINDOW_MINUTES);
        assert_eq!(
            of(Some("five_hour"), false),
            (LimitScope::Account, Some(SESSION_WINDOW_MINUTES), reset)
        );
        assert_eq!(
            of(Some("seven_day"), false),
            (LimitScope::Account, week, reset)
        );
        assert_eq!(
            of(Some("seven_day_opus"), false),
            (LimitScope::Model(Some(ModelFamily::Opus)), week, reset)
        );
        assert_eq!(
            of(Some("seven_day_sonnet"), false),
            (LimitScope::Model(Some(ModelFamily::Sonnet)), week, reset)
        );
        assert_eq!(
            of(Some(CREDITS_ALLOWANCE_CLAIM), false),
            (LimitScope::Model(None), week, reset)
        );
        assert_eq!(
            of(Some(CREDITS_ALLOWANCE_CLAIM), true),
            (LimitScope::Model(None), week, reset)
        );
        // A claim nothing knows, and none: the subscription's, with the reset the response gave.
        assert_eq!(
            of(Some("monthly"), false),
            (LimitScope::Account, None, reset)
        );
        assert_eq!(of(None, false), (LimitScope::Account, None, reset));
        // Credits refused while the response claims another window, or none.
        assert_eq!(
            of(Some("five_hour"), true),
            (LimitScope::Model(None), None, None)
        );
        assert_eq!(of(None, true), (LimitScope::Model(None), None, None));
        // A per-model cap stays that family's, with its own reset, also on a credits refusal.
        assert_eq!(
            of(Some("seven_day_opus"), true),
            (LimitScope::Model(Some(ModelFamily::Opus)), None, None)
        );
    }

    fn full_week(resets_at: Option<i64>) -> UsageWindow {
        UsageWindow {
            used_percent: Some(100.0),
            window_minutes: Some(WEEKLY_WINDOW_MINUTES),
            resets_at: resets_at.map(Timestamp),
        }
    }

    /// SPEC-473 RFR-002: Claude's answer to a request of a model ends the limit stored for that
    /// model when the model ran. Nothing is stored in its place: the model may run on credits
    /// a person bought with its allowance used up, and a window stored just below its limit
    /// would keep the pool away with no reading to replace it.
    #[test]
    fn a_model_that_ran_ends_the_limit_stored_for_it() {
        let fable = ModelFamily::Fable;
        let week = window(Some(61.0), Some(9_000_000));
        let read = Some(Timestamp(900_000));
        let stored = AccountUsage {
            secondary: Some(week),
            weekly_fable: Some(full_week(Some(5_000_000))),
            weekly_opus: Some(full_week(Some(6_000_000))),
            limit_reached: true,
            limit_resets_at: Some(Timestamp(7_000_000)),
            observed_at: read,
            ..AccountUsage::default()
        };

        let mut usage = stored;
        usage.record_model_answer(ModelAnswer::default(), fable, NOW);
        assert_eq!(usage.weekly_fable, None, "the model's limit ends");
        assert!(!usage.limit_reached, "and a limit of the subscription");
        assert_eq!(usage.limit_resets_at, None);
        assert_eq!(usage.secondary, Some(week), "what it did not read stays");
        assert_eq!(
            usage.weekly_opus, stored.weekly_opus,
            "another model's limit is not this answer's"
        );
        assert!(!usage.exhausted_for(NOW, 98.0, fable));
        assert_eq!(
            usage.observed_at, read,
            "an answer without the windows is no reading of the usage"
        );

        // The windows of the subscription the answer carried replace the stored ones, and an
        // answer that carried both is a reading of the usage; one window alone is not.
        let five = window(Some(12.0), Some(2_000_000));
        let seven = window(Some(70.0), Some(9_500_000));
        let mut usage = stored;
        usage.record_model_answer(
            ModelAnswer {
                five_hour: Some(five),
                seven_day: Some(seven),
                ..ModelAnswer::default()
            },
            fable,
            NOW,
        );
        assert_eq!((usage.primary, usage.secondary), (Some(five), Some(seven)));
        assert_eq!(usage.observed_at, Some(NOW));
        for one in [
            ModelAnswer {
                five_hour: Some(five),
                ..ModelAnswer::default()
            },
            ModelAnswer {
                seven_day: Some(seven),
                ..ModelAnswer::default()
            },
        ] {
            let mut usage = stored;
            usage.record_model_answer(one, fable, NOW);
            assert_eq!(usage.observed_at, read, "{one:?}");
            assert_eq!(usage.primary, one.five_hour);
            assert_eq!(usage.secondary, one.seven_day.or(stored.secondary));
        }

        // A family without a window of its own: only the subscription's limit ends.
        let mut usage = stored;
        usage.record_model_answer(ModelAnswer::default(), ModelFamily::Any, NOW);
        assert!(!usage.limit_reached);
        assert_eq!(usage.weekly_fable, stored.weekly_fable);
    }

    /// SPEC-473 RFR-002: a refusal is recorded like a limit a session reports, so a window
    /// stored without a reset gets the one the answer names; an answer that names none keeps
    /// the reset that is stored.
    #[test]
    fn a_refused_model_dates_the_limit_stored_for_it() {
        let fable = ModelFamily::Fable;
        let refused = |scope, minutes, resets_at: Option<i64>| ModelAnswer {
            refused: Some(ReportedLimit {
                scope,
                window_minutes: minutes,
                resets_at: resets_at.map(Timestamp),
            }),
            ..ModelAnswer::default()
        };
        let week = Some(WEEKLY_WINDOW_MINUTES);

        let mut blind = AccountUsage {
            weekly_fable: Some(full_week(None)),
            ..AccountUsage::default()
        };
        blind.record_model_answer(
            refused(LimitScope::Model(None), week, Some(5_000_000)),
            fable,
            NOW,
        );
        assert_eq!(blind.weekly_fable, Some(full_week(Some(5_000_000))));
        assert!(!blind.limit_reached, "still only that model's");
        assert_eq!(
            blind.observed_at, None,
            "a refusal is no reading of the usage"
        );

        // No reset in the answer: the stored one stays, and a newer one replaces it.
        let mut dated = AccountUsage {
            weekly_fable: Some(full_week(Some(5_000_000))),
            ..AccountUsage::default()
        };
        dated.record_model_answer(refused(LimitScope::Model(None), week, None), fable, NOW);
        assert_eq!(dated.weekly_fable, Some(full_week(Some(5_000_000))));
        dated.record_model_answer(
            refused(LimitScope::Model(None), week, Some(6_000_000)),
            fable,
            NOW,
        );
        assert_eq!(dated.weekly_fable, Some(full_week(Some(6_000_000))));
        // A reset that passed is not a limit's: nothing is carried over from it.
        let mut passed = AccountUsage {
            weekly_fable: Some(full_week(Some(900_000))),
            ..AccountUsage::default()
        };
        passed.record_model_answer(refused(LimitScope::Model(None), week, None), fable, NOW);
        assert_eq!(passed.weekly_fable, Some(full_week(None)));
        // A reset no window could have is not believed, and does not replace the stored one.
        let mut far = AccountUsage {
            weekly_fable: Some(full_week(Some(5_000_000))),
            ..AccountUsage::default()
        };
        far.record_model_answer(
            refused(
                LimitScope::Model(None),
                week,
                Some(NOW.0 + LIMIT_RESET_MAX_MS + 1),
            ),
            fable,
            NOW,
        );
        assert_eq!(far.weekly_fable, Some(full_week(Some(5_000_000))));

        // The family the answer names, not the one asked with.
        let mut named = AccountUsage::default();
        named.record_model_answer(
            refused(
                LimitScope::Model(Some(ModelFamily::Opus)),
                week,
                Some(5_000_000),
            ),
            fable,
            NOW,
        );
        assert_eq!(named.weekly_opus, Some(full_week(Some(5_000_000))));
        assert_eq!(named.weekly_fable, None);

        // The subscription's own window: full until the reset, a limit in effect, and the
        // reading the answer carried beside it.
        let seven = window(Some(70.0), Some(9_500_000));
        let mut own = AccountUsage {
            weekly_fable: Some(full_week(Some(5_000_000))),
            ..AccountUsage::default()
        };
        own.record_model_answer(
            ModelAnswer {
                seven_day: Some(seven),
                ..refused(
                    LimitScope::Account,
                    Some(SESSION_WINDOW_MINUTES),
                    Some(2_000_000),
                )
            },
            fable,
            NOW,
        );
        assert_eq!(own.primary.and_then(|w| w.used_percent), Some(100.0));
        assert_eq!(
            own.primary.and_then(|w| w.resets_at),
            Some(Timestamp(2_000_000))
        );
        assert!(own.limit_in_effect(NOW));
        assert_eq!(own.secondary, Some(seven));
        assert_eq!(
            own.weekly_fable,
            Some(full_week(Some(5_000_000))),
            "the model's limit is not answered by the subscription's"
        );
    }

    /// SPEC-473 RFR-003: a reading that is followed by questions about the stored limits keeps
    /// a model's window stored used up without a reset, which a reading alone ends; everything
    /// else is kept or replaced as by any reading.
    #[test]
    fn a_reading_before_claude_is_asked_keeps_a_limit_without_a_reset() {
        let stored = AccountUsage {
            secondary: Some(full_week(Some(9_000_000))),
            weekly_fable: Some(full_week(None)),
            weekly_opus: Some(full_week(Some(5_000_000))),
            // Not used up, and without a reset: nothing to ask about.
            weekly_sonnet: Some(window(Some(80.0), None)),
            limit_reached: true,
            ..AccountUsage::default()
        };
        let reading = AccountUsage {
            secondary: Some(window(Some(61.0), Some(9_000_000))),
            observed_at: Some(NOW),
            ..AccountUsage::default()
        };
        assert_eq!(reading.keeping_unread(&stored, NOW).weekly_fable, None);
        let kept = reading.keeping_asked(&stored, NOW);
        assert_eq!(kept.weekly_fable, stored.weekly_fable);
        assert_eq!(kept.weekly_opus, stored.weekly_opus, "as by any reading");
        assert_eq!(kept.weekly_sonnet, None);
        // A window past its reset is no limit to ask about: it goes, as by any reading.
        let passed = AccountUsage {
            weekly_fable: Some(full_week(Some(900_000))),
            ..stored
        };
        assert_eq!(reading.keeping_asked(&passed, NOW).weekly_fable, None);
        assert_eq!(kept.secondary, reading.secondary);
        assert!(!kept.limit_reached);
        // A reading that carries the window is the newer word.
        let carried = AccountUsage {
            weekly_fable: Some(window(Some(20.0), Some(8_000_000))),
            ..reading
        };
        assert_eq!(
            carried.keeping_asked(&stored, NOW).weekly_fable,
            carried.weekly_fable
        );

        // What an answer for a family speaks of: its window and the subscription's limit.
        assert_eq!(
            stored.stored_limit(ModelFamily::Fable),
            (stored.weekly_fable, true, None)
        );
        assert_eq!(
            stored.stored_limit(ModelFamily::Opus),
            (stored.weekly_opus, true, None)
        );
        assert_eq!(
            AccountUsage {
                limit_resets_at: Some(Timestamp(7)),
                ..AccountUsage::default()
            }
            .stored_limit(ModelFamily::Any),
            (None, false, Some(Timestamp(7)))
        );
    }

    /// SPEC-473 RFR-003: Claude is asked about each limit that is stored and in effect: with
    /// the family whose own window is used up, and with the default model's family for a limit
    /// of the whole subscription.
    #[test]
    fn the_stored_limits_say_which_models_to_ask_about() {
        let fable = ModelFamily::Fable;
        assert_eq!(AccountUsage::default().families_at_a_limit(NOW, fable), []);
        let windows = AccountUsage {
            // The subscription's own windows are read, not asked about.
            primary: Some(window(Some(100.0), Some(2_000_000))),
            secondary: Some(window(Some(100.0), Some(9_000_000))),
            weekly_opus: Some(full_week(None)),
            // Not used up, and one past its reset.
            weekly_sonnet: Some(window(Some(99.9), Some(9_000_000))),
            weekly_fable: Some(full_week(Some(900_000))),
            ..AccountUsage::default()
        };
        assert_eq!(windows.families_at_a_limit(NOW, fable), [ModelFamily::Opus]);
        let both = AccountUsage {
            weekly_fable: Some(full_week(Some(5_000_000))),
            weekly_sonnet: Some(full_week(Some(5_000_000))),
            ..windows
        };
        assert_eq!(
            both.families_at_a_limit(NOW, fable),
            [ModelFamily::Opus, ModelFamily::Sonnet, fable]
        );
        // A limit of the subscription: the default model's family, once.
        let reported = AccountUsage {
            limit_reached: true,
            ..AccountUsage::default()
        };
        assert_eq!(reported.families_at_a_limit(NOW, fable), [fable]);
        assert_eq!(
            reported.families_at_a_limit(NOW, ModelFamily::Any),
            [ModelFamily::Any]
        );
        let ended = AccountUsage {
            limit_resets_at: Some(Timestamp(900_000)),
            ..reported
        };
        assert_eq!(ended.families_at_a_limit(NOW, fable), []);
        let with_window = AccountUsage {
            limit_reached: true,
            ..both
        };
        assert_eq!(
            with_window.families_at_a_limit(NOW, fable),
            [ModelFamily::Opus, ModelFamily::Sonnet, fable]
        );
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
