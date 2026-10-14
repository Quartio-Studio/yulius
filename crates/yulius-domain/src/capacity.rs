//! Capacity monitoring for the subscription pool: burn rate, run-out forecast, hours paused and
//! how many subscriptions to add (SPEC-001 ROT-013, SPEC-002 ADR-17 §2.4 "Forecast").
//!
//! Units: a subscription's weekly window is 100 %; the pool's capacity and its burn are counted
//! in percentage points of one subscription's weekly window ("points"), so a pool of three fresh
//! subscriptions has 3 × [`crate::rotation::THRESHOLD`] points before it pauses. Unknown usage is
//! never treated as zero: members without a known weekly percentage add no headroom and are
//! counted separately.

use crate::time::Timestamp;

const HOUR_MS: f64 = 3_600_000.0;
/// Hours in a week.
pub const WEEK_HOURS: f64 = 168.0;
/// The forecast horizon: this week.
const HORIZON_MS: i64 = 7 * 24 * 3_600_000;
/// One rolling day.
const DAY_MS: i64 = 24 * 3_600_000;
/// An account's samples must span at least this long to give a rate.
const MIN_SPAN_HOURS: f64 = 1.0;

/// One provider reading of an account's weekly window.
#[derive(Debug, Clone, PartialEq)]
pub struct WeeklySample {
    /// Account the reading belongs to (any stable key; removed accounts keep counting as
    /// demand that was served).
    pub account: String,
    /// When it was read.
    pub at: Timestamp,
    /// Weekly utilization in percent.
    pub used_percent: f64,
    /// When that weekly window resets, when known.
    pub resets_at: Option<Timestamp>,
}

/// A healthy subscription of the pool as it stands now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolMember {
    /// Weekly utilization applying now (0 after a reset); `None` when unknown.
    pub weekly_used: Option<f64>,
    /// When its weekly window resets next, when known.
    pub weekly_resets_at: Option<Timestamp>,
}

/// A time interval; `end = None` means "still going".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Start.
    pub start: Timestamp,
    /// End, or `None` while it lasts.
    pub end: Option<Timestamp>,
}

/// Everything [`forecast`] reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastInput<'a> {
    /// Evaluation time.
    pub now: Timestamp,
    /// Pause threshold in percent ([`crate::rotation::THRESHOLD`]).
    pub threshold: f64,
    /// Healthy subscriptions now.
    pub members: &'a [PoolMember],
    /// Weekly readings of the last 7 days (and possibly older ones, ignored).
    pub samples: &'a [WeeklySample],
    /// Agent runs (attempt lifetimes).
    pub runs: &'a [Span],
    /// Pauses for lack of capacity.
    pub pauses: &'a [Span],
    /// The part of `pauses` during which the agent's process stayed alive (held at a turn
    /// boundary or at a limit), which `runs` also covers.
    pub held: &'a [Span],
}

/// The pool's capacity picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Forecast {
    /// Healthy subscriptions.
    pub subscriptions: u32,
    /// Subscriptions whose weekly usage is unknown (they add no headroom here).
    pub unknown_usage: u32,
    /// Points left before the pool pauses: Σ (threshold − weekly used)⁺ over members with
    /// known usage; `None` when no member's usage is known.
    pub weekly_headroom: Option<f64>,
    /// Burn rate over the last 24 h in points per hour; `None` without enough readings.
    pub burn_24h: Option<f64>,
    /// Burn rate over the last 7 days in points per hour; `None` without enough readings.
    pub burn_7d: Option<f64>,
    /// Burn rate of each of the last 7 rolling 24 hours (oldest first; the last ends now).
    pub burn_per_day: Vec<Option<f64>>,
    /// Task-hours paused in each of the last 7 rolling 24 hours (oldest first).
    pub paused_hours_per_day: Vec<f64>,
    /// When the pool's weekly headroom runs out at the 24-hour burn rate, if within 7 days.
    pub runs_out_at: Option<Timestamp>,
    /// Task-hours paused for lack of capacity in the last 7 days.
    pub paused_hours_7d: f64,
    /// Task-hours agents ran (not paused) in the last 7 days.
    pub active_hours_7d: f64,
    /// Subscriptions the observed demand needs (see [`needed_subscriptions`]).
    pub needed: u32,
    /// Subscriptions to add: `needed − subscriptions`, at least 1 with an empty pool and
    /// paused or waiting work.
    pub add: u32,
}

impl Forecast {
    /// The plain recommendation shown to administrators.
    #[must_use]
    pub fn recommendation(&self) -> String {
        match self.add {
            0 => "Capacity is sufficient".to_owned(),
            1 if self.subscriptions == 0 => "Add a subscription to start working".to_owned(),
            1 => "Add about 1 subscription to stop pausing".to_owned(),
            n => format!("Add about {n} subscriptions to stop pausing"),
        }
    }
}

fn hours(ms: i64) -> f64 {
    ms as f64 / HOUR_MS
}

/// Burn rate in points per hour over `[now − window_ms, now]`: for every account, the weekly
/// percentage consumed between consecutive readings (the rise, or the reading itself when the
/// window reset in between; a fall without a reset counts 0) divided by the hours its readings
/// span, summed over accounts. Accounts whose readings span less than an hour are skipped;
/// `None` when no account qualifies.
#[must_use]
pub fn burn_rate(samples: &[WeeklySample], now: Timestamp, window_ms: i64) -> Option<f64> {
    let from = now.plus_millis(-window_ms);
    let mut inside: Vec<&WeeklySample> = samples
        .iter()
        .filter(|s| s.at >= from && s.at <= now && s.used_percent.is_finite())
        .collect();
    inside.sort_by(|a, b| a.account.cmp(&b.account).then(a.at.cmp(&b.at)));
    let mut total: Option<f64> = None;
    for group in inside.chunk_by(|a, b| a.account == b.account) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let span = hours(last.at.millis_since(first.at));
        if span < MIN_SPAN_HOURS {
            continue;
        }
        let consumed: f64 = group
            .windows(2)
            .map(|pair| {
                let (before, after) = (pair[0], pair[1]);
                if before.resets_at.is_some_and(|r| r <= after.at) {
                    after.used_percent.max(0.0)
                } else {
                    (after.used_percent - before.used_percent).max(0.0)
                }
            })
            .sum();
        total = Some(total.unwrap_or(0.0) + consumed / span);
    }
    total
}

/// When the pool's weekly headroom runs out at `burn` points per hour, walking forward from
/// `now`: the headroom falls linearly, and at each member's weekly reset it gets back what that
/// member had used (capped at threshold × members with known usage). `None` when the headroom
/// lasts the next 7 days, the burn is not positive, or no member's usage is known.
#[must_use]
pub fn run_out(
    members: &[PoolMember],
    threshold: f64,
    burn: Option<f64>,
    now: Timestamp,
) -> Option<Timestamp> {
    let burn = burn.filter(|b| *b > 0.0)?;
    let known: Vec<(f64, Option<Timestamp>)> = members
        .iter()
        .filter_map(|m| {
            m.weekly_used
                .filter(|u| u.is_finite())
                .map(|u| (u, m.weekly_resets_at))
        })
        .collect();
    if known.is_empty() {
        return None;
    }
    let cap = threshold * known.len() as f64;
    let mut headroom: f64 = known.iter().map(|(u, _)| (threshold - u).max(0.0)).sum();
    let end = now.plus_millis(HORIZON_MS);
    let mut refills: Vec<(Timestamp, f64)> = known
        .iter()
        .filter_map(|(u, at)| {
            at.filter(|at| *at > now && *at <= end)
                .map(|at| (at, u.clamp(0.0, threshold)))
        })
        .collect();
    refills.sort_by_key(|(at, _)| *at);
    let mut t = now;
    for (at, refill) in refills.into_iter().chain(std::iter::once((end, 0.0))) {
        let need = burn * hours(at.millis_since(t));
        if need >= headroom {
            let ms = (headroom / burn * HOUR_MS).round() as i64;
            return Some(t.plus_millis(ms));
        }
        headroom = (headroom - need + refill).min(cap);
        t = at;
    }
    None
}

/// Hours of `spans` inside `[from, to]` (an open span ends at `to`), summed.
#[must_use]
pub fn overlap_hours(spans: &[Span], from: Timestamp, to: Timestamp) -> f64 {
    spans
        .iter()
        .map(|s| {
            let start = s.start.max(from);
            let end = s.end.unwrap_or(to).min(to);
            hours(end.millis_since(start).max(0))
        })
        .sum()
}

/// Subscriptions the demand needs (ADR-17 §2.4):
/// `ceil(max(burn_24h × 168 / threshold, N × (active + paused) / active))`, the second term
/// only when tasks were paused (`N + 1` when they were paused without ever running).
#[must_use]
pub fn needed_subscriptions(
    subscriptions: u32,
    threshold: f64,
    burn_24h: Option<f64>,
    active_hours: f64,
    paused_hours: f64,
) -> u32 {
    let n = f64::from(subscriptions);
    let by_rate = burn_24h.map_or(0.0, |b| b.max(0.0) * WEEK_HOURS / threshold);
    let by_pauses = if paused_hours <= 0.0 {
        0.0
    } else if active_hours > 0.0 {
        n * (active_hours + paused_hours) / active_hours
    } else {
        n + 1.0
    };
    // A hair of tolerance so float noise (2.000000001) does not round up.
    let needed = (by_rate.max(by_pauses) - 1e-9).ceil().clamp(0.0, 10_000.0);
    needed as u32
}

/// The ends of the last 7 rolling 24-hour periods, oldest first; the last is `now`.
fn day_ends(now: Timestamp) -> impl Iterator<Item = Timestamp> {
    (0..7).rev().map(move |i| now.plus_millis(-i * DAY_MS))
}

/// The whole capacity picture (ADR-17 §2.4).
#[must_use]
pub fn forecast(input: &ForecastInput<'_>) -> Forecast {
    let now = input.now;
    let week_ago = now.plus_millis(-HORIZON_MS);
    let subscriptions = u32::try_from(input.members.len()).unwrap_or(u32::MAX);
    let known: Vec<f64> = input
        .members
        .iter()
        .filter_map(|m| m.weekly_used.filter(|u| u.is_finite()))
        .collect();
    let unknown_usage = subscriptions - u32::try_from(known.len()).unwrap_or(subscriptions);
    let weekly_headroom = (!known.is_empty()).then(|| {
        known
            .iter()
            .map(|u| (input.threshold - u).max(0.0))
            .sum::<f64>()
    });
    let burn_24h = burn_rate(input.samples, now, 24 * 3_600_000);
    let burn_7d = burn_rate(input.samples, now, HORIZON_MS);
    let paused_hours_7d = overlap_hours(input.pauses, week_ago, now);
    let active_hours_7d = (overlap_hours(input.runs, week_ago, now)
        - overlap_hours(input.held, week_ago, now))
    .max(0.0);
    let needed = needed_subscriptions(
        subscriptions,
        input.threshold,
        burn_24h,
        active_hours_7d,
        paused_hours_7d,
    );
    let mut add = needed.saturating_sub(subscriptions);
    if subscriptions == 0 && (paused_hours_7d > 0.0 || !input.pauses.is_empty()) {
        add = add.max(1);
    }
    Forecast {
        subscriptions,
        unknown_usage,
        weekly_headroom,
        burn_24h,
        burn_7d,
        burn_per_day: day_ends(now)
            .map(|end| burn_rate(input.samples, end, DAY_MS))
            .collect(),
        paused_hours_per_day: day_ends(now)
            .map(|end| overlap_hours(input.pauses, end.plus_millis(-DAY_MS), end))
            .collect(),
        runs_out_at: run_out(input.members, input.threshold, burn_24h, now),
        paused_hours_7d,
        active_hours_7d,
        needed,
        add,
    }
}

/// One agent run's tokens for the pool's token use (SPEC-110 TUS-003).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenRun {
    /// Its lifetime; `end = None` while it runs.
    pub span: Span,
    /// The model it ran, when known.
    pub model: Option<String>,
    /// Input tokens, when reported.
    pub input: Option<u64>,
    /// Output tokens, when reported.
    pub output: Option<u64>,
    /// Cache-read tokens, when reported.
    pub cache_read: Option<u64>,
    /// Cache-write tokens, when reported.
    pub cache_write: Option<u64>,
}

/// Tokens spent in one period and what they would cost at API list prices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenSpend {
    /// Input tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Cache-read tokens.
    pub cache_read: f64,
    /// Cache-write tokens.
    pub cache_write: f64,
    /// Estimated cost in USD of the runs with a price.
    pub cost_usd: f64,
    /// Some run with a price spent tokens here.
    pub priced: bool,
    /// Some run without a price spent tokens here.
    pub unpriced: bool,
}

impl TokenSpend {
    /// The estimated cost, or `None` when every run here lacks a price.
    #[must_use]
    pub fn cost(&self) -> Option<f64> {
        (self.priced || !self.unpriced).then_some(self.cost_usd)
    }

    fn add(&mut self, run: &TokenRun, share: f64, cost: Option<f64>) {
        let part = |v: Option<u64>| v.map_or(0.0, |v| v as f64 * share);
        self.input += part(run.input);
        self.output += part(run.output);
        self.cache_read += part(run.cache_read);
        self.cache_write += part(run.cache_write);
        match cost {
            Some(c) => {
                self.cost_usd += c * share;
                self.priced = true;
            }
            None => self.unpriced = true,
        }
    }
}

/// The pool's token use over the last `days` rolling 24-hour periods.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoolTokens {
    /// One entry per 24 hours, oldest first; the last ends at `now`.
    pub per_day: Vec<TokenSpend>,
    /// Runs in the window that reported no usage.
    pub unknown_usage_runs: u32,
    /// Runs in the window whose model has no price.
    pub unpriced_runs: u32,
}

/// Spreads every run's tokens over `days` rolling 24-hour periods ending at `now`, in proportion
/// to the part of its lifetime (start to end, or `now`) inside each; a run without duration counts
/// where it started. Runs outside the window count nowhere.
#[must_use]
pub fn pool_tokens(runs: &[TokenRun], now: Timestamp, days: usize) -> PoolTokens {
    let n = i64::try_from(days).unwrap_or(0);
    let from = now.plus_millis(-n * DAY_MS);
    let mut out = PoolTokens {
        per_day: vec![TokenSpend::default(); days],
        ..PoolTokens::default()
    };
    for run in runs {
        let start = run.span.start;
        let end = run.span.end.unwrap_or(now).min(now).max(start);
        let inside = if end > start {
            end > from && start < now
        } else {
            start >= from && start <= now
        };
        if !inside {
            continue;
        }
        if run.input.is_none()
            && run.output.is_none()
            && run.cache_read.is_none()
            && run.cache_write.is_none()
        {
            out.unknown_usage_runs = out.unknown_usage_runs.saturating_add(1);
            continue;
        }
        let cost = run
            .model
            .as_deref()
            .and_then(crate::pricing::price_of)
            .map(|p| {
                crate::pricing::cost_usd(p, run.input, run.output, run.cache_read, run.cache_write)
            });
        if cost.is_none() {
            out.unpriced_runs = out.unpriced_runs.saturating_add(1);
        }
        let length = end.millis_since(start);
        for (i, bucket) in out.per_day.iter_mut().enumerate() {
            let lo = from.plus_millis(i64::try_from(i).unwrap_or(0) * DAY_MS);
            let hi = lo.plus_millis(DAY_MS);
            let share = if length > 0 {
                end.min(hi).millis_since(start.max(lo)).max(0) as f64 / length as f64
            } else if start >= lo && (start < hi || (hi == now && start == now)) {
                1.0
            } else {
                0.0
            };
            if share > 0.0 {
                bucket.add(run, share, cost);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp(1_000 * 3_600_000);
    const HOUR: i64 = 3_600_000;

    fn at(hours_from_now: i64) -> Timestamp {
        NOW.plus_millis(hours_from_now * HOUR)
    }

    fn sample(account: &str, h: i64, used: f64, resets: Option<i64>) -> WeeklySample {
        WeeklySample {
            account: account.into(),
            at: at(h),
            used_percent: used,
            resets_at: resets.map(at),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn burn_rate_sums_rises_and_counts_resets() {
        let samples = vec![
            // a: 10 → 20 → 30 over 10 h: 2 points/h.
            sample("a", -10, 10.0, Some(100)),
            sample("a", -5, 20.0, Some(100)),
            sample("a", 0, 30.0, Some(100)),
            // b: 90 → (reset at −3 h) → 6 over 4 h: 6 points / 4 h = 1.5 points/h.
            sample("b", -4, 90.0, Some(-3)),
            sample("b", 0, 6.0, Some(165)),
            // c: one reading only: no span, skipped.
            sample("c", -1, 50.0, None),
            // d: a fall without a reset (provider correction) counts 0 over 2 h.
            sample("d", -2, 40.0, None),
            sample("d", 0, 35.0, None),
            // Outside the 24-hour window: ignored.
            sample("e", -30, 0.0, None),
            sample("e", -25, 80.0, None),
        ];
        let rate = burn_rate(&samples, NOW, 24 * HOUR).unwrap_or(f64::NAN);
        assert!(close(rate, 3.5), "{rate}");
        assert_eq!(burn_rate(&samples[5..6], NOW, 24 * HOUR), None);
        assert_eq!(burn_rate(&[], NOW, 24 * HOUR), None);
    }

    #[test]
    fn run_out_walks_through_resets() {
        // Two members: 78 % (resets in 10 h) and 38 % (resets in 100 h): headroom 20 + 60 = 80.
        let members = [
            PoolMember {
                weekly_used: Some(78.0),
                weekly_resets_at: Some(at(10)),
            },
            PoolMember {
                weekly_used: Some(38.0),
                weekly_resets_at: Some(at(100)),
            },
        ];
        // 10 points/h: runs out after 8 h, before the first reset.
        assert_eq!(run_out(&members, 98.0, Some(10.0), NOW), Some(at(8)));
        // 4 points/h: 40 used by the 10 h reset, which refills 78 → 118; then 29.5 h more.
        assert_eq!(
            run_out(&members, 98.0, Some(4.0), NOW),
            Some(NOW.plus_millis(39 * HOUR + HOUR / 2))
        );
        // 0.5 points/h lasts the week; no burn never runs out; unknown usage gives no answer.
        assert_eq!(run_out(&members, 98.0, Some(0.5), NOW), None);
        assert_eq!(run_out(&members, 98.0, None, NOW), None);
        let unknown = [PoolMember {
            weekly_used: None,
            weekly_resets_at: None,
        }];
        assert_eq!(run_out(&unknown, 98.0, Some(10.0), NOW), None);
    }

    #[test]
    fn overlap_counts_task_hours_in_the_window() {
        let spans = [
            Span {
                start: at(-200),
                end: Some(at(-166)),
            }, // 2 h inside the last 168 h
            Span {
                start: at(-10),
                end: Some(at(-7)),
            }, // 3 h
            Span {
                start: at(-1),
                end: None,
            }, // 1 h, still going
            Span {
                start: at(5),
                end: None,
            }, // in the future: 0
        ];
        assert!(close(overlap_hours(&spans, at(-168), NOW), 6.0));
    }

    #[test]
    fn needed_subscriptions_follow_the_formula() {
        // 3 subscriptions, 100 active and 25 paused task-hours: 3 × 125 / 100 = 3.75 → 4.
        assert_eq!(needed_subscriptions(3, 98.0, Some(1.0), 100.0, 25.0), 4);
        // Burn 1.75 points/h needs 1.75 × 168 / 98 = 3.0 subscriptions exactly → 3.
        assert_eq!(needed_subscriptions(3, 98.0, Some(1.75), 100.0, 0.0), 3);
        // Burn 2 points/h needs 3.43 → 4 even without pauses.
        assert_eq!(needed_subscriptions(3, 98.0, Some(2.0), 100.0, 0.0), 4);
        // Paused but never ran: one more than now.
        assert_eq!(needed_subscriptions(0, 98.0, None, 0.0, 2.0), 1);
        assert_eq!(needed_subscriptions(2, 98.0, None, 0.0, 0.0), 0);
    }

    /// AC-ROT-12: the forecast numbers for a known history.
    #[test]
    fn the_forecast_numbers_are_correct() {
        let members = [
            PoolMember {
                weekly_used: Some(60.0),
                weekly_resets_at: Some(at(48)),
            },
            PoolMember {
                weekly_used: Some(90.0),
                weekly_resets_at: Some(at(5)),
            },
            PoolMember {
                weekly_used: None,
                weekly_resets_at: None,
            },
        ];
        let samples = vec![
            sample("a", -24, 36.0, Some(48)),
            sample("a", 0, 60.0, Some(48)),
            sample("b", -24, 66.0, Some(5)),
            sample("b", 0, 90.0, Some(5)),
        ];
        let runs = [Span {
            start: at(-40),
            end: None,
        }];
        let pauses = [Span {
            start: at(-10),
            end: Some(at(-2)),
        }];
        let forecast = forecast(&ForecastInput {
            now: NOW,
            threshold: 98.0,
            members: &members,
            samples: &samples,
            runs: &runs,
            pauses: &pauses,
            held: &pauses,
        });
        assert_eq!(forecast.subscriptions, 3);
        assert_eq!(forecast.unknown_usage, 1);
        assert_eq!(forecast.weekly_headroom, Some(38.0 + 8.0));
        // 24 points over 24 h on each of two accounts: 2 points/h.
        assert_eq!(forecast.burn_24h, Some(2.0));
        assert_eq!(forecast.burn_7d, Some(2.0));
        // 46 points at 2/h: 10 are used by b's reset at 5 h (+90 → 126), 86 more by a's reset
        // at 48 h (+60 → 100), and those last 50 h more: 98 h.
        assert_eq!(forecast.runs_out_at, Some(at(98)));
        assert!(close(forecast.paused_hours_7d, 8.0));
        assert!(close(forecast.active_hours_7d, 32.0));
        // TUS-005: per rolling 24 hours, oldest first; the readings span only the last 24 h.
        assert_eq!(
            forecast.burn_per_day,
            vec![None, None, None, None, None, None, Some(2.0)]
        );
        assert_eq!(
            forecast.paused_hours_per_day,
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 8.0]
        );
        // max(2 × 168 / 98 = 3.43, 3 × 40 / 32 = 3.75) → 4: add 1.
        assert_eq!(forecast.needed, 4);
        assert_eq!(forecast.add, 1);
        assert_eq!(
            forecast.recommendation(),
            "Add about 1 subscription to stop pausing"
        );
    }

    #[test]
    fn an_idle_pool_is_sufficient_and_an_empty_one_is_not() {
        let members = [PoolMember {
            weekly_used: Some(10.0),
            weekly_resets_at: Some(at(50)),
        }];
        let quiet = forecast(&ForecastInput {
            now: NOW,
            threshold: 98.0,
            members: &members,
            samples: &[],
            runs: &[],
            pauses: &[],
            held: &[],
        });
        assert_eq!((quiet.needed, quiet.add), (0, 0));
        assert_eq!(quiet.burn_24h, None);
        assert_eq!(quiet.runs_out_at, None);
        assert_eq!(quiet.recommendation(), "Capacity is sufficient");
        let waiting = [Span {
            start: at(-1),
            end: None,
        }];
        let empty = forecast(&ForecastInput {
            now: NOW,
            threshold: 98.0,
            members: &[],
            samples: &[],
            runs: &[],
            pauses: &waiting,
            held: &[],
        });
        assert_eq!(empty.add, 1);
        assert_eq!(empty.weekly_headroom, None);
        assert_eq!(
            empty.recommendation(),
            "Add a subscription to start working"
        );
    }

    fn token_run(from: i64, to: Option<i64>, model: &str, tokens: u64) -> TokenRun {
        TokenRun {
            span: Span {
                start: at(from),
                end: to.map(at),
            },
            model: Some(model.into()),
            input: Some(tokens),
            output: Some(tokens),
            cache_read: Some(tokens),
            cache_write: None,
        }
    }

    /// TUS-003: a run counts in each 24-hour period by the share of its lifetime inside it.
    #[test]
    fn pool_tokens_spread_runs_over_rolling_days() {
        let runs = vec![
            // 40 h, still running: 16 h in the day before yesterday's window, 24 h in the last.
            token_run(-40, None, "claude-opus-5-5", 1_000_000),
            // Ended 12 h ago after 2 h: all in the last 24 h.
            token_run(-14, Some(-12), "claude-sonnet-5-5", 100),
            // Ended before the window: nowhere.
            token_run(-200, Some(-170), "claude-opus-5-5", 9_999),
            // Half of its 4 h lifetime falls before the 7 days.
            token_run(-170, Some(-166), "claude-opus-5-5", 1_000),
        ];
        let pool = pool_tokens(&runs, NOW, 7);
        assert_eq!(pool.per_day.len(), 7);
        assert!(close(pool.per_day[6].input, 600_000.0 + 100.0));
        assert!(close(pool.per_day[5].input, 400_000.0));
        assert!(close(pool.per_day[0].input, 500.0));
        assert!(close(pool.per_day[3].input, 0.0));
        // Opus 5.5: $4 in + $20 out + $0.20 cache read per million.
        assert!(close(pool.per_day[5].cost_usd, 0.4 * 24.2));
        assert_eq!(pool.per_day[3].cost(), Some(0.0));
        assert_eq!((pool.unknown_usage_runs, pool.unpriced_runs), (0, 0));
    }

    #[test]
    fn pool_tokens_count_unknown_usage_and_models_without_price() {
        let mut silent = token_run(-3, Some(-2), "claude-opus-5-5", 0);
        silent.input = None;
        silent.output = None;
        silent.cache_read = None;
        let runs = vec![
            silent,
            token_run(-5, Some(-4), "mystery-model", 1_000),
            token_run(-6, Some(-6), "claude-opus-5-5", 0),
        ];
        let pool = pool_tokens(&runs, NOW, 7);
        assert_eq!(pool.unknown_usage_runs, 1);
        assert_eq!(pool.unpriced_runs, 1);
        let last = &pool.per_day[6];
        assert!(
            close(last.input, 1_000.0),
            "the unpriced run's tokens count"
        );
        assert_eq!(last.cost(), Some(0.0), "the priced zero-length run counts");
        let only_unpriced = pool_tokens(&runs[1..2], NOW, 7);
        assert_eq!(only_unpriced.per_day[6].cost(), None);
        assert_eq!(only_unpriced.per_day[5].cost(), Some(0.0), "nothing ran");
    }
}
