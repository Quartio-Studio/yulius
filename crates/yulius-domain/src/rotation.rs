//! Subscription pool policy (SPEC-001 ROT-003/004/010, SPEC-002 ADR-17 and §6.3).
//!
//! [`decide`] is a pure function of a [`RotationInput`] snapshot and a [`Trigger`]. Callers
//! persist the input together with the returned [`Decision`] and [`DecisionReason`] for audit
//! (ROT-011).
//!
//! Every connected Claude subscription is one member of a pool; there are no manual priorities
//! (F 2026-10-10). Rotation stays within the task's provider: Yulius runs only Claude Code
//! (F 2026-10-08). A **candidate** is a healthy account of that provider that is below
//! [`THRESHOLD`] in every window binding the task (5-hour, weekly and the weekly cap of the
//! task's model family) and has no usage limit in effect. Candidates are ranked by:
//!
//! 1. [`Tier`]: known headroom of at least [`MIN_ROOM`] points, then unknown usage (never
//!    treated as zero, but not evidence of exhaustion either — and a subscription nobody uses is
//!    never read, F 2026-10-11), then known headroom below [`MIN_ROOM`] (a task placed there would
//!    move again within minutes and pay for a cold prompt cache);
//! 2. [`score`] — use it or lose it: the weekly headroom divided by the hours until that weekly
//!    window resets (the rate needed to spend the capacity before it expires), divided by the
//!    live tasks already on the account plus one, so concurrent tasks spread;
//! 3. ties: the capacity that expires soonest (weekly reset, then 5-hour reset), fewer tasks,
//!    account id.
//!
//! When there is no candidate the task waits ([`Decision::Wait`]) until the earliest known time
//! an account becomes available again (ROT-010).

use crate::harness::Provider;
use crate::ids::AccountId;
use crate::time::Timestamp;
use crate::usage::{AccountUsage, LimitScope, ModelFamily, ReportedLimit, UsageWindow};

/// Proactive switch and pause threshold in percent of any binding window (SPEC-001 ROT-003,
/// F 2026-10-10).
pub const THRESHOLD: f64 = 98.0;

/// Headroom (percentage points below [`THRESHOLD`]) an account needs in every binding window to
/// be preferred for placement; below it a task would move again within minutes.
pub const MIN_ROOM: f64 = 5.0;

/// Length of the weekly window in hours: the time to reset assumed when it is unknown.
const WEEK_HOURS: f64 = 168.0;
/// Shortest time to reset used in the score, so a window resetting now does not divide by zero.
const MIN_HOURS: f64 = 0.25;
const HOUR_MS: f64 = 3_600_000.0;

/// The policy's view of one connected account.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountView {
    /// Account identity.
    pub id: AccountId,
    /// Provider the account belongs to.
    pub provider: Provider,
    /// Credentials work (signed in, not in an error or re-login state).
    pub healthy: bool,
    /// Latest usage snapshot.
    pub usage: AccountUsage,
    /// Live attempts currently using the account (including the deciding task's own attempt).
    pub in_use: u32,
}

/// Everything [`decide`] looks at.
#[derive(Debug, Clone, PartialEq)]
pub struct RotationInput {
    /// The account the task currently runs on (or last ran on).
    pub current: Option<AccountId>,
    /// Provider of the task's harness; only its accounts are candidates.
    pub task_provider: Provider,
    /// Model family of the task: its weekly cap binds too.
    pub family: ModelFamily,
    /// Model family of the model the task falls back to while no account is a candidate for
    /// `family` (SPEC-482); `None` for a task that waits instead.
    pub fallback: Option<ModelFamily>,
    /// Model family of the model the run on `current` was launched with; `None` without a run,
    /// or for one launched with the task's own model (`family`).
    pub running: Option<ModelFamily>,
    /// All connected accounts. For [`Trigger::LimitReached`], callers fold the observed limit
    /// into the current account's usage first so its reset time is known.
    pub accounts: Vec<AccountView>,
    /// Decision time.
    pub now: Timestamp,
    /// Threshold in percent; [`THRESHOLD`] in production.
    pub threshold: f64,
}

/// Why a decision is requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// A new attempt is starting without a live process (new task, Continue, a queued or
    /// paused task starting): placement among all candidates.
    Start,
    /// A turn boundary, or the resume check of a task paused with a live process: stay while
    /// the current account is a candidate, else move to the best other one, else wait.
    Proactive,
    /// The current account hit a usage limit the CLI reported; always move off it.
    LimitReached,
    /// A user asked for a specific account, or to rotate now (ROT-007).
    Manual {
        /// Requested account; `None` rotates to the best other account.
        account: Option<AccountId>,
    },
}

/// What the rotation controller should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Keep the current account.
    Stay,
    /// Run the task on another account of its provider.
    Switch {
        /// Target account.
        account: AccountId,
    },
    /// No account has capacity; pause (ROT-010).
    Wait {
        /// Earliest known time an account becomes available; `None` when unknown.
        until: Option<Timestamp>,
    },
}

/// Which candidate class a selected account came from (better first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    /// Known headroom of at least [`MIN_ROOM`] points in every binding window.
    Roomy,
    /// No binding window has a known percentage. Before [`Tier::Tight`]: unknown is not
    /// exhausted, and a subscription that never gets a task never gets a reading either — the
    /// pool collapsed onto one subscription at 97 % while six others sat idle (F 2026-10-11).
    UnknownUsage,
    /// Known headroom below [`MIN_ROOM`] points in some binding window.
    Tight,
}

/// Why a manual request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// No account has the requested id.
    UnknownAccount,
    /// The requested account is not signed in or broken.
    Unavailable,
    /// The requested account is full (100 % of a binding window) or limited at decision time.
    Exhausted,
    /// The requested account belongs to another provider.
    ProviderMismatch,
    /// No other candidate exists.
    NoCandidate,
}

/// Audit explanation for a [`Decision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionReason {
    /// The current account is still a candidate.
    CurrentUsable,
    /// The current account's usage is unknown; there is no evidence to switch.
    CurrentUsageUnknown,
    /// The best candidate was selected automatically.
    Selected {
        /// Candidate class of the chosen account.
        tier: Tier,
    },
    /// No account has capacity.
    NoCapacity,
    /// The user's explicitly chosen account was accepted.
    ManualChoice,
    /// The user chose the account the task already uses.
    ManualAlreadyCurrent,
    /// The user's request was refused; the task stays on its account.
    ManualRefused(Refusal),
}

/// How an account ranks for a task right now; `None` from [`assess`] means "not a candidate".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assessment {
    /// Candidate class.
    pub tier: Tier,
    /// Use-it-or-lose-it score (higher first); see [`score`].
    pub score: f64,
    /// Smallest headroom in points below the threshold over the binding windows with a known
    /// percentage; `None` when none is known.
    pub room: Option<f64>,
    /// When the binding weekly window with the least headroom resets, if in the future.
    pub weekly_reset: Option<Timestamp>,
    /// When the running 5-hour window resets, if in the future.
    pub five_hour_reset: Option<Timestamp>,
}

fn hours_until(window: &UsageWindow, now: Timestamp) -> f64 {
    match window.resets_at {
        // A weekly window whose reset passed restarted: a whole week lies ahead.
        Some(at) if at > now => {
            (at.millis_since(now) as f64 / HOUR_MS).clamp(MIN_HOURS, WEEK_HOURS)
        }
        _ => WEEK_HOURS,
    }
}

/// The use-it-or-lose-it score of an account with the given weekly headroom (points below the
/// threshold), hours until that weekly window resets, and live tasks on it:
/// `headroom / hours / (in_use + 1)`. Higher is better.
#[must_use]
pub fn score(weekly_headroom: f64, hours_to_reset: f64, in_use: u32) -> f64 {
    let hours = hours_to_reset.clamp(MIN_HOURS, WEEK_HOURS);
    weekly_headroom.max(0.0) / hours / (f64::from(in_use) + 1.0)
}

/// How `account` ranks for a task of `family` at `now`; `None` when it is not a candidate
/// (unhealthy, limited, or at the threshold in a binding window).
#[must_use]
pub fn assess(
    account: &AccountView,
    now: Timestamp,
    threshold: f64,
    family: ModelFamily,
) -> Option<Assessment> {
    let usage = &account.usage;
    if !account.healthy || usage.exhausted_for(now, threshold, family) {
        return None;
    }
    let room = usage
        .binding_windows(family)
        .filter_map(|w| w.current_percent(now))
        .map(|p| threshold - p)
        .reduce(f64::min);
    let five_hour_reset = usage
        .primary
        .and_then(|w| w.resets_at)
        .filter(|at| *at > now);
    let Some(room) = room else {
        return Some(Assessment {
            tier: Tier::UnknownUsage,
            score: 0.0,
            room: None,
            weekly_reset: None,
            five_hour_reset,
        });
    };
    let weekly = usage
        .weekly_windows(family)
        .filter_map(|w| w.current_percent(now).map(|p| (w, threshold - p)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let (score, weekly_reset) = match weekly {
        Some((window, headroom)) => (
            score(headroom, hours_until(window, now), account.in_use),
            window.resets_at.filter(|at| *at > now),
        ),
        // Only the 5-hour window is known: no weekly urgency.
        None => (0.0, None),
    };
    Some(Assessment {
        tier: if room >= MIN_ROOM {
            Tier::Roomy
        } else {
            Tier::Tight
        },
        score,
        room: Some(room),
        weekly_reset,
        five_hour_reset,
    })
}

/// Scores are compared at micro-unit precision so float noise never decides between equals.
/// Scores are at most 100 / 0.25 = 400, so the cast cannot overflow.
fn quantized(score: f64) -> i64 {
    (score * 1_000_000.0).round() as i64
}

/// The model family a run placed now is launched with (SPEC-482 FBK-002): the task's own while
/// an account is a candidate for it, else its fallback's while an account is a candidate for
/// that, else the task's own (it waits).
#[must_use]
pub fn placing_family(input: &RotationInput) -> ModelFamily {
    let runs = |family| {
        input.accounts.iter().any(|a| {
            a.provider == input.task_provider
                && assess(a, input.now, input.threshold, family).is_some()
        })
    };
    match input.fallback {
        Some(fallback) if !runs(input.family) && runs(fallback) => fallback,
        _ => input.family,
    }
}

/// Candidates for a run of the input's provider placed now ([`placing_family`]), best first,
/// excluding `exclude`.
#[must_use]
pub fn ranked<'a>(
    input: &'a RotationInput,
    exclude: Option<&AccountId>,
) -> Vec<(&'a AccountView, Assessment)> {
    ranked_for(input, placing_family(input), exclude)
}

fn ranked_for<'a>(
    input: &'a RotationInput,
    family: ModelFamily,
    exclude: Option<&AccountId>,
) -> Vec<(&'a AccountView, Assessment)> {
    let mut candidates: Vec<(&AccountView, Assessment)> = input
        .accounts
        .iter()
        .filter(|a| a.provider == input.task_provider && Some(&a.id) != exclude)
        .filter_map(|a| {
            assess(a, input.now, input.threshold, family).map(|assessment| (a, assessment))
        })
        .collect();
    candidates.sort_by(|(a, x), (b, y)| {
        let never = Timestamp(i64::MAX);
        x.tier
            .cmp(&y.tier)
            .then_with(|| quantized(y.score).cmp(&quantized(x.score)))
            .then_with(|| {
                x.weekly_reset
                    .unwrap_or(never)
                    .cmp(&y.weekly_reset.unwrap_or(never))
            })
            .then_with(|| {
                x.five_hour_reset
                    .unwrap_or(never)
                    .cmp(&y.five_hour_reset.unwrap_or(never))
            })
            .then_with(|| a.in_use.cmp(&b.in_use))
            .then_with(|| a.id.cmp(&b.id))
    });
    candidates
}

/// Decides whether and where a task's run should go (SPEC-001 §7.4).
///
/// A run is placed with [`placing_family`]: the task's fallback model while no account can run
/// its own (SPEC-482). Never panics; an unknown current account is treated as unusable.
#[must_use]
pub fn decide(input: &RotationInput, trigger: Trigger) -> (Decision, DecisionReason) {
    let placing = Placing::of(input);
    match trigger {
        Trigger::Start => placing.select_or_wait(None),
        Trigger::Proactive => proactive(&placing),
        Trigger::LimitReached => placing.select_or_wait(placing.left),
        Trigger::Manual { account } => manual(&placing, account),
    }
}

/// [`decide`] for a task the pool keeps off `left_out`: the subscription that refused it at a
/// limit nothing says the end of, again right after its retry (SPEC-464 UCL-010). While a reset
/// is known to wait for, `left_out` counts as out of the task's model with no known reset,
/// whatever a reading said since: the task waits for that reset and is not started on
/// `left_out` at every reading. With no reset known, nothing would ever take the task on, so
/// `left_out` is a candidate like any other.
///
/// Leaving out is about the task's own model (SPEC-482 FBK-007). A pool that places the task
/// with its fallback anyway leaves nothing out: the fallback is another model than the one
/// `left_out` refused. And a placement with the fallback that only leaving out makes is not
/// taken: the launch asks the policy without it and would place the task's own model.
#[must_use]
pub fn decide_leaving_out(
    input: &RotationInput,
    trigger: Trigger,
    left_out: &AccountId,
) -> (Decision, DecisionReason) {
    if placing_family(input) != input.family {
        return decide(input, trigger);
    }
    let mut without = input.clone();
    for account in &mut without.accounts {
        if &account.id == left_out {
            // The model's own window, full with no reset; the subscription's limit for a model
            // that has no window.
            let refused = ReportedLimit {
                scope: LimitScope::Model(None),
                window_minutes: None,
                resets_at: None,
            };
            account.usage.record_limit(refused, input.family, input.now);
        }
    }
    match decide(&without, trigger.clone()) {
        (Decision::Wait { until: None }, _) => decide(input, trigger),
        (Decision::Switch { .. }, _) if placing_family(&without) != input.family => {
            decide(input, trigger)
        }
        decided => decided,
    }
}

fn find<'a>(input: &'a RotationInput, id: &AccountId) -> Option<&'a AccountView> {
    input.accounts.iter().find(|a| &a.id == id)
}

/// One decision's view of where a run would be placed.
struct Placing<'a> {
    input: &'a RotationInput,
    /// The family a run placed now is launched with ([`placing_family`]).
    family: ModelFamily,
    /// The account a placement after a limit, or off an account the run cannot stay on, leaves
    /// out: the current one, while the run placed would be of the model the current run was
    /// launched with. A run that changes its model may stay on its subscription (SPEC-482
    /// FBK-003): what it hit there binds the other model.
    left: Option<&'a AccountId>,
}

impl<'a> Placing<'a> {
    fn of(input: &'a RotationInput) -> Self {
        let family = placing_family(input);
        Self {
            input,
            family,
            left: input
                .current
                .as_ref()
                .filter(|_| input.running.unwrap_or(input.family) == family),
        }
    }

    fn select_or_wait(&self, exclude: Option<&AccountId>) -> (Decision, DecisionReason) {
        match ranked_for(self.input, self.family, exclude).first() {
            Some((account, assessment)) => switch(account, assessment.tier),
            None => (
                Decision::Wait {
                    until: available_again(self.input),
                },
                DecisionReason::NoCapacity,
            ),
        }
    }
}

fn proactive(placing: &Placing<'_>) -> (Decision, DecisionReason) {
    let input = placing.input;
    let current = input
        .current
        .as_ref()
        .and_then(|id| find(input, id))
        .filter(|a| a.provider == input.task_provider);
    // A run stays by the model it was launched with: one on the fallback is not restarted
    // because the task's own model runs again (SPEC-482 FBK-004).
    let running = input.running.unwrap_or(input.family);
    if let Some(current) = current
        && let Some(assessment) = assess(current, input.now, input.threshold, running)
    {
        let reason = if assessment.tier == Tier::UnknownUsage {
            DecisionReason::CurrentUsageUnknown
        } else {
            DecisionReason::CurrentUsable
        };
        return (Decision::Stay, reason);
    }
    placing.select_or_wait(placing.left)
}

fn manual(placing: &Placing<'_>, account: Option<AccountId>) -> (Decision, DecisionReason) {
    let input = placing.input;
    let refuse = |refusal| (Decision::Stay, DecisionReason::ManualRefused(refusal));
    let Some(id) = account else {
        // "Rotate now": the best other account.
        return match ranked_for(input, placing.family, input.current.as_ref()).first() {
            Some((target, assessment)) => switch(target, assessment.tier),
            None => refuse(Refusal::NoCandidate),
        };
    };
    let Some(target) = find(input, &id) else {
        return refuse(Refusal::UnknownAccount);
    };
    if target.provider != input.task_provider {
        return refuse(Refusal::ProviderMismatch);
    }
    if input.current.as_ref() == Some(&target.id) {
        return (Decision::Stay, DecisionReason::ManualAlreadyCurrent);
    }
    if !target.healthy {
        return refuse(Refusal::Unavailable);
    }
    // A person may use what the pool keeps in reserve above its threshold, never a full window
    // or a limit in effect (SPEC-447 MOV-002), judged for the model the run is placed with.
    if target.usage.exhausted_for(input.now, 100.0, placing.family) {
        return refuse(Refusal::Exhausted);
    }
    (
        Decision::Switch {
            account: target.id.clone(),
        },
        DecisionReason::ManualChoice,
    )
}

fn switch(account: &AccountView, tier: Tier) -> (Decision, DecisionReason) {
    (
        Decision::Switch {
            account: account.id.clone(),
        },
        DecisionReason::Selected { tier },
    )
}

/// The earliest known time a healthy account of the task's provider becomes a candidate again
/// (ROT-010), for the task's own model or its fallback (SPEC-482); `None` when no such time is
/// known (for example no account at all, or limits without a reset time).
#[must_use]
pub fn available_again(input: &RotationInput) -> Option<Timestamp> {
    input
        .accounts
        .iter()
        .filter(|a| a.healthy && a.provider == input.task_provider)
        .flat_map(|a| {
            std::iter::once(input.family)
                .chain(input.fallback)
                .filter_map(|family| a.usage.available_at_for(input.now, input.threshold, family))
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::IdError;

    const NOW: Timestamp = Timestamp(10_000_000_000);
    const HOUR: i64 = 3_600_000;
    const A: Provider = Provider::Anthropic;

    fn id(text: &str) -> Result<AccountId, IdError> {
        AccountId::parse(text)
    }

    fn window(percent: f64, resets_in_hours: Option<i64>) -> UsageWindow {
        UsageWindow {
            used_percent: Some(percent),
            window_minutes: None,
            resets_at: resets_in_hours.map(|h| NOW.plus_millis(h * HOUR)),
        }
    }

    /// 5-hour window at `five` percent (reset in 2 h) and weekly window at `week` percent.
    fn usage(five: f64, week: f64, week_resets_in_hours: Option<i64>) -> AccountUsage {
        AccountUsage {
            primary: Some(window(five, Some(2))),
            secondary: Some(window(week, week_resets_in_hours)),
            ..AccountUsage::default()
        }
    }

    fn account(name: &str, usage: AccountUsage) -> Result<AccountView, IdError> {
        Ok(AccountView {
            id: id(name)?,
            provider: A,
            healthy: true,
            usage,
            in_use: 0,
        })
    }

    fn input(current: Option<&str>, accounts: Vec<AccountView>) -> Result<RotationInput, IdError> {
        Ok(RotationInput {
            current: current.map(id).transpose()?,
            task_provider: A,
            family: ModelFamily::Any,
            fallback: None,
            running: None,
            accounts,
            now: NOW,
            threshold: THRESHOLD,
        })
    }

    fn switched_to(decision: &Decision) -> Option<&str> {
        match decision {
            Decision::Switch { account } => Some(account.as_str()),
            _ => None,
        }
    }

    fn start(accounts: Vec<AccountView>) -> Result<Option<String>, IdError> {
        let rotation = input(None, accounts)?;
        Ok(switched_to(&decide(&rotation, Trigger::Start).0).map(str::to_owned))
    }

    /// AC-ROT-9: capacity that expires soon is spent first (use it or lose it).
    #[test]
    fn placement_spends_the_capacity_that_expires_first() -> Result<(), IdError> {
        // "soon": 50 % weekly left with 10 h to its reset needs 4.8 points/h to be used;
        // "far": 88 % left over 150 h needs 0.59 points/h.
        let soon = account("soon", usage(10.0, 48.0, Some(10)))?;
        let far = account("far", usage(0.0, 10.0, Some(150)))?;
        assert_eq!(
            start(vec![far.clone(), soon.clone()])?.as_deref(),
            Some("soon")
        );
        // An account far from its reset is not drained while expiring capacity exists, even
        // though it has more headroom.
        let assessed = assess(&far, NOW, THRESHOLD, ModelFamily::Any).map(|a| a.score);
        let urgent = assess(&soon, NOW, THRESHOLD, ModelFamily::Any).map(|a| a.score);
        assert!(urgent > assessed, "{urgent:?} > {assessed:?}");
        Ok(())
    }

    /// AC-ROT-9: concurrent tasks spread over accounts in proportion to their urgency.
    #[test]
    fn concurrent_tasks_spread_over_the_pool() -> Result<(), IdError> {
        // a: 2 points/h needed, b: 1.5 points/h needed.
        let mut a = account("a", usage(0.0, 58.0, Some(20)))?;
        let mut b = account("b", usage(0.0, 68.0, Some(20)))?;
        let mut placed = Vec::new();
        for _ in 0..4 {
            let chosen = start(vec![a.clone(), b.clone()])?.unwrap_or_default();
            if chosen == "a" {
                a.in_use += 1;
            } else {
                b.in_use += 1;
            }
            placed.push(chosen);
        }
        assert_eq!(placed, ["a", "b", "a", "b"]);
        Ok(())
    }

    /// AC-ROT-9: equal scores go to the capacity that expires soonest.
    #[test]
    fn ties_go_to_the_capacity_that_expires_soonest() -> Result<(), IdError> {
        let fresh = |name: &str, five_resets: Option<i64>| {
            account(
                name,
                AccountUsage {
                    primary: Some(window(0.0, five_resets)),
                    secondary: Some(window(20.0, None)),
                    ..AccountUsage::default()
                },
            )
        };
        // Same weekly picture; "running" has a 5-hour window that ends in an hour.
        assert_eq!(
            start(vec![fresh("idle", None)?, fresh("running", Some(1))?])?.as_deref(),
            Some("running")
        );
        // Same everything: the account id decides, deterministically.
        assert_eq!(
            start(vec![fresh("z", None)?, fresh("m", None)?])?.as_deref(),
            Some("m")
        );
        Ok(())
    }

    #[test]
    fn roomy_beats_unknown_beats_tight() -> Result<(), IdError> {
        // "tight" expires much sooner (high score) but has 2 points of 5-hour room left.
        let tight = account("tight", usage(96.0, 50.0, Some(2)))?;
        let roomy = account("roomy", usage(10.0, 10.0, Some(160)))?;
        let unknown = account("unknown", AccountUsage::default())?;
        assert_eq!(
            start(vec![tight.clone(), roomy.clone(), unknown.clone()])?.as_deref(),
            Some("roomy")
        );
        // F 2026-10-11: every task went to a subscription at 97 % weekly while six with unknown
        // usage sat idle (and so were never read).
        assert_eq!(
            start(vec![tight.clone(), unknown.clone()])?.as_deref(),
            Some("unknown")
        );
        assert_eq!(start(vec![tight])?.as_deref(), Some("tight"));
        assert_eq!(start(vec![unknown])?.as_deref(), Some("unknown"));
        Ok(())
    }

    /// AC-ROT-2: at 98 % of either window the task moves at the boundary; below it stays.
    #[test]
    fn proactive_switch_at_98_percent_of_either_window() -> Result<(), IdError> {
        let other = account("b", usage(0.0, 0.0, Some(100)))?;
        for current in [usage(98.0, 10.0, Some(100)), usage(10.0, 98.0, Some(100))] {
            let rotation = input(Some("a"), vec![account("a", current)?, other.clone()])?;
            assert_eq!(
                decide(&rotation, Trigger::Proactive),
                (
                    Decision::Switch { account: id("b")? },
                    DecisionReason::Selected { tier: Tier::Roomy }
                )
            );
        }
        let below = input(
            Some("a"),
            vec![account("a", usage(97.9, 97.9, Some(100)))?, other.clone()],
        )?;
        assert_eq!(
            decide(&below, Trigger::Proactive),
            (Decision::Stay, DecisionReason::CurrentUsable)
        );
        let unknown = input(
            Some("a"),
            vec![account("a", AccountUsage::default())?, other],
        )?;
        assert_eq!(
            decide(&unknown, Trigger::Proactive),
            (Decision::Stay, DecisionReason::CurrentUsageUnknown)
        );
        Ok(())
    }

    /// AC-ROT-10: every account at 98 % or limited → wait until the earliest reset.
    #[test]
    fn an_exhausted_pool_waits_until_the_earliest_reset() -> Result<(), IdError> {
        let limited = AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(NOW.plus_millis(3 * HOUR)),
            ..AccountUsage::default()
        };
        let rotation = input(
            Some("a"),
            vec![
                account("a", usage(98.5, 40.0, Some(100)))?,
                account("b", usage(10.0, 99.0, Some(30)))?,
                account("c", limited)?,
            ],
        )?;
        // a's 5-hour window resets in 2 h (the earliest), b's week in 30 h, c's limit in 3 h.
        let expected = (
            Decision::Wait {
                until: Some(NOW.plus_millis(2 * HOUR)),
            },
            DecisionReason::NoCapacity,
        );
        assert_eq!(decide(&rotation, Trigger::Proactive), expected);
        assert_eq!(decide(&rotation, Trigger::Start), expected);
        assert_eq!(decide(&rotation, Trigger::LimitReached), expected);
        // Two hours later a's window has reset: the paused task stays on it.
        let later = RotationInput {
            now: NOW.plus_millis(2 * HOUR),
            ..rotation
        };
        assert_eq!(
            decide(&later, Trigger::Proactive),
            (Decision::Stay, DecisionReason::CurrentUsable)
        );
        Ok(())
    }

    /// AC-ROT-14: a single subscription works — chosen, paused at 98 %, chosen after its reset.
    #[test]
    fn one_account_alone_still_works() -> Result<(), IdError> {
        let only = account("only", usage(30.0, 30.0, Some(80)))?;
        assert_eq!(start(vec![only])?.as_deref(), Some("only"));
        let full = input(
            Some("only"),
            vec![account("only", usage(98.0, 30.0, Some(80)))?],
        )?;
        assert_eq!(
            decide(&full, Trigger::Proactive).0,
            Decision::Wait {
                until: Some(NOW.plus_millis(2 * HOUR))
            }
        );
        let after = RotationInput {
            now: NOW.plus_millis(2 * HOUR),
            ..full
        };
        assert_eq!(decide(&after, Trigger::Proactive).0, Decision::Stay);
        assert_eq!(switched_to(&decide(&after, Trigger::Start).0), Some("only"));
        let none = input(None, Vec::new())?;
        assert_eq!(
            decide(&none, Trigger::Start),
            (Decision::Wait { until: None }, DecisionReason::NoCapacity)
        );
        Ok(())
    }

    #[test]
    fn a_model_cap_binds_only_tasks_of_that_model() -> Result<(), IdError> {
        let capped = AccountUsage {
            weekly_opus: Some(window(99.0, Some(40))),
            ..usage(10.0, 10.0, Some(100))
        };
        let other = account("b", usage(50.0, 60.0, Some(100)))?;
        let mut rotation = input(Some("a"), vec![account("a", capped)?, other])?;
        assert_eq!(decide(&rotation, Trigger::Proactive).0, Decision::Stay);
        rotation.family = ModelFamily::Opus;
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Proactive).0),
            Some("b")
        );
        assert_eq!(ModelFamily::of(Some("claude-opus-4-5")), ModelFamily::Opus);
        assert_eq!(ModelFamily::of(Some("sonnet")), ModelFamily::Sonnet);
        assert_eq!(ModelFamily::of(Some("haiku")), ModelFamily::Any);
        assert_eq!(ModelFamily::of(None), ModelFamily::Any);
        Ok(())
    }

    /// SPEC-464 UCL-010: a subscription that refused a task again at a limit nothing says the end
    /// of is left out for that task while another subscription's reset is known to wait for,
    /// whatever its own reading says since. With no reset known it is a candidate again.
    #[test]
    fn a_subscription_left_out_is_not_chosen_while_a_reset_is_known() -> Result<(), IdError> {
        let refusing = id("refusing")?;
        let mut later = usage(10.0, 100.0, Some(40));
        later.limit_reached = true;
        later.limit_resets_at = Some(NOW.plus_millis(40 * HOUR));
        // Its reading shows room again; the other one is out until its reset in 40 hours.
        let known = input(
            None,
            vec![
                account("refusing", usage(0.0, 10.0, Some(100)))?,
                account("later", later)?,
            ],
        )?;
        assert_eq!(
            switched_to(&decide(&known, Trigger::Start).0),
            Some("refusing")
        );
        let (decision, _) = decide_leaving_out(&known, Trigger::Start, &refusing);
        assert_eq!(
            decision,
            Decision::Wait {
                until: Some(NOW.plus_millis(40 * HOUR))
            }
        );
        // A paused task that still has its process there is not released in place either.
        let mut held = known.clone();
        held.current = Some(refusing.clone());
        let (decision, _) = decide_leaving_out(&held, Trigger::Proactive, &refusing);
        assert!(
            matches!(decision, Decision::Wait { until: Some(_) }),
            "{decision:?}"
        );
        // What is stored of an earlier limit of the subscription left out says nothing: one that
        // ended does not make it a candidate, and one still ahead is no reset to wait for.
        for stale_hours in [-1, 30] {
            let mut stale = known.clone();
            stale.accounts[0].usage.limit_reached = true;
            stale.accounts[0].usage.limit_resets_at = Some(NOW.plus_millis(stale_hours * HOUR));
            let (decision, _) = decide_leaving_out(&stale, Trigger::Start, &refusing);
            assert_eq!(
                decision,
                Decision::Wait {
                    until: Some(NOW.plus_millis(40 * HOUR))
                },
                "{stale_hours} h"
            );
        }
        // A subscription with room takes the task, left out or not.
        let mut room = known.clone();
        room.accounts
            .push(account("third", usage(20.0, 20.0, Some(100)))?);
        let (decision, _) = decide_leaving_out(&room, Trigger::Start, &refusing);
        assert_eq!(switched_to(&decision), Some("third"));
        // Nothing known to wait for: it is tried again rather than never.
        let mut unknown = usage(10.0, 100.0, None);
        unknown.limit_reached = true;
        let blind = input(
            None,
            vec![
                account("refusing", usage(0.0, 10.0, Some(100)))?,
                account("later", unknown)?,
            ],
        )?;
        let (decision, _) = decide_leaving_out(&blind, Trigger::Start, &refusing);
        assert_eq!(switched_to(&decision), Some("refusing"));
        Ok(())
    }

    /// SPEC-464 (production 2026-11-11): a subscription whose Fable allowance is used up takes no
    /// Fable task until the allowance resets, though its 5-hour and weekly windows have the most
    /// room, and still takes a task of another model. When every subscription is out, the task
    /// waits for the first allowance to reset.
    #[test]
    fn a_fable_allowance_binds_only_fable_tasks() -> Result<(), IdError> {
        let out = |resets_in_hours: i64| -> AccountUsage {
            let mut usage = usage(0.0, 61.0, Some(100));
            usage.record_limit(
                // As the harness reads the refusal: the limit of the model the session runs.
                crate::usage::ReportedLimit {
                    scope: crate::usage::LimitScope::Model(None),
                    window_minutes: Some(crate::usage::WEEKLY_WINDOW_MINUTES),
                    resets_at: Some(NOW.plus_millis(resets_in_hours * HOUR)),
                },
                ModelFamily::Fable,
                NOW,
            );
            usage
        };
        let busy = account("busy", usage(41.0, 80.0, Some(100)))?;
        let mut rotation = input(None, vec![account("out", out(40))?, busy.clone()])?;
        // Another model's task goes where the windows have the most room.
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Start).0),
            Some("out")
        );
        rotation.family = ModelFamily::of(Some("claude-fable-5-2"));
        assert_eq!(rotation.family, ModelFamily::Fable);
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Start).0),
            Some("busy")
        );
        // A Fable task on it moves at its turn boundary, as at 98 % of any window.
        rotation.current = Some(id("out")?);
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Proactive).0),
            Some("busy")
        );
        // Every subscription out: the wait ends at the first reset, and the subscription is
        // usable from that moment.
        let mut all_out = input(
            Some("out"),
            vec![account("out", out(40))?, account("later", out(90))?],
        )?;
        all_out.family = ModelFamily::Fable;
        assert_eq!(
            decide(&all_out, Trigger::LimitReached),
            (
                Decision::Wait {
                    until: Some(NOW.plus_millis(40 * HOUR))
                },
                DecisionReason::NoCapacity
            )
        );
        all_out.now = NOW.plus_millis(40 * HOUR);
        assert_eq!(
            decide(&all_out, Trigger::Proactive),
            (Decision::Stay, DecisionReason::CurrentUsable)
        );
        Ok(())
    }

    #[test]
    fn limit_reached_always_moves_off_current() -> Result<(), IdError> {
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(5.0, 5.0, None))?,
                account("a2", AccountUsage::default())?,
            ],
        )?;
        assert_eq!(
            decide(&rotation, Trigger::LimitReached),
            (
                Decision::Switch { account: id("a2")? },
                DecisionReason::Selected {
                    tier: Tier::UnknownUsage
                }
            )
        );
        Ok(())
    }

    #[test]
    fn unhealthy_accounts_are_never_chosen() -> Result<(), IdError> {
        let mut broken = account("a2", usage(0.0, 0.0, None))?;
        broken.healthy = false;
        let rotation = input(
            Some("a1"),
            vec![account("a1", usage(100.0, 0.0, None))?, broken],
        )?;
        assert_eq!(
            decide(&rotation, Trigger::LimitReached),
            (
                Decision::Wait {
                    until: Some(NOW.plus_millis(2 * HOUR))
                },
                DecisionReason::NoCapacity
            )
        );
        Ok(())
    }

    #[test]
    fn a_reset_window_counts_as_fresh() -> Result<(), IdError> {
        // a2's snapshot showed 100 % but its 5-hour reset has passed.
        let reset = AccountUsage {
            primary: Some(window(100.0, Some(-1))),
            secondary: Some(window(30.0, Some(50))),
            ..AccountUsage::default()
        };
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(100.0, 0.0, Some(60)))?,
                account("a2", reset)?,
            ],
        )?;
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::LimitReached).0),
            Some("a2")
        );
        Ok(())
    }

    #[test]
    fn manual_account_choice_rules() -> Result<(), IdError> {
        let mut broken = account("a3", usage(0.0, 0.0, None))?;
        broken.healthy = false;
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(10.0, 10.0, None))?,
                account("a2", usage(100.0, 0.0, Some(10)))?,
                broken,
                account("a4", AccountUsage::default())?,
                account("a5", usage(99.0, 10.0, None))?,
            ],
        )?;
        let manual = |name: &str| -> Result<Trigger, IdError> {
            Ok(Trigger::Manual {
                account: Some(id(name)?),
            })
        };
        assert_eq!(
            decide(&rotation, manual("a2")?).1,
            DecisionReason::ManualRefused(Refusal::Exhausted)
        );
        assert_eq!(
            decide(&rotation, manual("a3")?).1,
            DecisionReason::ManualRefused(Refusal::Unavailable)
        );
        assert_eq!(
            decide(&rotation, manual("nope")?).1,
            DecisionReason::ManualRefused(Refusal::UnknownAccount)
        );
        assert_eq!(
            decide(&rotation, manual("a1")?),
            (Decision::Stay, DecisionReason::ManualAlreadyCurrent)
        );
        assert_eq!(
            decide(&rotation, manual("a4")?),
            (
                Decision::Switch { account: id("a4")? },
                DecisionReason::ManualChoice
            )
        );
        // A person's choice above the pool's threshold but below the limit is honoured (SPEC-447
        // MOV-002): a task waiting for capacity could not be moved by hand otherwise.
        assert_eq!(
            decide(&rotation, manual("a5")?),
            (
                Decision::Switch { account: id("a5")? },
                DecisionReason::ManualChoice
            )
        );
        let now = Trigger::Manual { account: None };
        assert_eq!(switched_to(&decide(&rotation, now.clone()).0), Some("a4"));
        let alone = input(Some("a1"), vec![account("a1", usage(10.0, 10.0, None))?])?;
        assert_eq!(
            decide(&alone, now),
            (
                Decision::Stay,
                DecisionReason::ManualRefused(Refusal::NoCandidate)
            )
        );
        Ok(())
    }

    #[test]
    fn unknown_current_account_is_treated_as_unusable() -> Result<(), IdError> {
        let rotation = input(Some("gone"), vec![account("a2", usage(1.0, 1.0, None))?])?;
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Proactive).0),
            Some("a2")
        );
        Ok(())
    }

    /// A subscription whose Fable allowance is used up until `resets_in_hours` from now, with
    /// room in its 5-hour window (`five` %) and its week.
    fn fable_out(five: f64, resets_in_hours: Option<i64>) -> AccountUsage {
        let mut usage = usage(five, 61.0, Some(100));
        usage.record_limit(
            crate::usage::ReportedLimit {
                scope: crate::usage::LimitScope::Model(None),
                window_minutes: Some(crate::usage::WEEKLY_WINDOW_MINUTES),
                resets_at: resets_in_hours.map(|h| NOW.plus_millis(h * HOUR)),
            },
            ModelFamily::Fable,
            NOW,
        );
        usage
    }

    /// A Fable task that falls back to Opus, on `current`.
    fn fable_task(
        current: Option<&str>,
        accounts: Vec<AccountView>,
    ) -> Result<RotationInput, IdError> {
        let mut rotation = input(current, accounts)?;
        rotation.family = ModelFamily::Fable;
        rotation.fallback = Some(ModelFamily::Opus);
        Ok(rotation)
    }

    /// SPEC-482 FBK-002: with Fable used up on every subscription and room for Opus, a run is
    /// placed with the fallback's family; with a subscription that still runs Fable, or without
    /// a fallback, nothing changes.
    #[test]
    fn a_fable_task_is_placed_with_its_fallback_when_no_subscription_runs_fable()
    -> Result<(), IdError> {
        let all_out = fable_task(
            None,
            vec![
                account("first", fable_out(0.0, Some(40)))?,
                account("second", fable_out(30.0, Some(90)))?,
            ],
        )?;
        assert_eq!(placing_family(&all_out), ModelFamily::Opus);
        assert_eq!(
            decide(&all_out, Trigger::Start),
            (
                Decision::Switch {
                    account: id("first")?
                },
                DecisionReason::Selected { tier: Tier::Roomy }
            )
        );
        // A task that has no fallback waits for the first allowance, as before (SPEC-464).
        let mut waits = all_out.clone();
        waits.fallback = None;
        assert_eq!(placing_family(&waits), ModelFamily::Fable);
        assert_eq!(
            decide(&waits, Trigger::Start).0,
            Decision::Wait {
                until: Some(NOW.plus_millis(40 * HOUR))
            }
        );
        // One subscription still runs Fable: the task goes there with its own model, though the
        // other has more room.
        let one_left = fable_task(
            None,
            vec![
                account("out", fable_out(0.0, Some(40)))?,
                account("busy", usage(41.0, 80.0, Some(100)))?,
            ],
        )?;
        assert_eq!(placing_family(&one_left), ModelFamily::Fable);
        assert_eq!(
            switched_to(&decide(&one_left, Trigger::Start).0),
            Some("busy")
        );
        // The fallback's own cap binds a run placed with it.
        let mut capped = fable_out(0.0, Some(40));
        capped.weekly_opus = Some(window(99.0, Some(60)));
        let opus_capped = fable_task(
            None,
            vec![
                account("capped", capped)?,
                account("later", fable_out(30.0, Some(90)))?,
            ],
        )?;
        assert_eq!(
            switched_to(&decide(&opus_capped, Trigger::Start).0),
            Some("later")
        );
        // An unhealthy subscription, or one of another provider, runs neither model.
        let mut broken = account("broken", fable_out(0.0, Some(40)))?;
        broken.healthy = false;
        let none = fable_task(None, vec![broken])?;
        assert_eq!(placing_family(&none), ModelFamily::Fable);
        assert_eq!(
            decide(&none, Trigger::Start).0,
            Decision::Wait { until: None }
        );
        Ok(())
    }

    /// SPEC-482 FBK-003: a run that changes its model may stay on its subscription, so one
    /// subscription is enough; a run that would keep its model is moved off it, as at any limit.
    #[test]
    fn a_run_that_changes_its_model_may_stay_on_its_subscription() -> Result<(), IdError> {
        let alone = fable_task(Some("only"), vec![account("only", fable_out(10.0, None))?])?;
        for trigger in [Trigger::LimitReached, Trigger::Proactive] {
            assert_eq!(
                decide(&alone, trigger.clone()),
                (
                    Decision::Switch {
                        account: id("only")?
                    },
                    DecisionReason::Selected { tier: Tier::Roomy }
                ),
                "{trigger:?}"
            );
        }
        // The run is on the fallback already and hit a limit there: its subscription is left
        // out, and with nothing else the task waits.
        let mut on_fallback = alone.clone();
        on_fallback.running = Some(ModelFamily::Opus);
        on_fallback.accounts[0].usage.weekly_opus = Some(window(100.0, Some(30)));
        assert_eq!(
            decide(&on_fallback, Trigger::LimitReached).0,
            Decision::Wait {
                until: Some(NOW.plus_millis(30 * HOUR))
            }
        );
        // A limit reported on the fallback that shows in no window still moves the run off.
        let mut unseen = alone.clone();
        unseen.running = Some(ModelFamily::Opus);
        unseen
            .accounts
            .push(account("other", fable_out(50.0, Some(90)))?);
        assert_eq!(
            switched_to(&decide(&unseen, Trigger::LimitReached).0),
            Some("other")
        );
        // "Rotate now" still asks for another subscription.
        assert_eq!(
            decide(&alone, Trigger::Manual { account: None }),
            (
                Decision::Stay,
                DecisionReason::ManualRefused(Refusal::NoCandidate)
            )
        );
        Ok(())
    }

    /// SPEC-482 FBK-004: a run stays while its subscription can run the model it was launched
    /// with. One on the fallback is not restarted because Fable runs again; its next placement
    /// takes the task's own model.
    #[test]
    fn a_run_stays_by_the_model_it_was_launched_with() -> Result<(), IdError> {
        let mut back = fable_task(
            Some("a"),
            vec![
                account("a", fable_out(10.0, Some(40)))?,
                account("b", usage(20.0, 20.0, Some(100)))?,
            ],
        )?;
        back.running = Some(ModelFamily::Opus);
        assert_eq!(placing_family(&back), ModelFamily::Fable);
        assert_eq!(
            decide(&back, Trigger::Proactive),
            (Decision::Stay, DecisionReason::CurrentUsable)
        );
        // A run of the task's own model on that subscription cannot stay: it moves to Fable.
        back.running = None;
        assert_eq!(switched_to(&decide(&back, Trigger::Proactive).0), Some("b"));
        // The run on the fallback ends anyway (its 5-hour window): placed anew, with Fable.
        back.running = Some(ModelFamily::Opus);
        back.accounts[0].usage.primary = Some(window(98.0, Some(2)));
        assert_eq!(switched_to(&decide(&back, Trigger::Proactive).0), Some("b"));
        assert_eq!(switched_to(&decide(&back, Trigger::Start).0), Some("b"));
        Ok(())
    }

    /// SPEC-482 FBK-002: a person's choice is judged for the model the run would be placed with,
    /// and with no subscription for either model the task waits for the first that can run one.
    #[test]
    fn a_choice_and_a_wait_follow_the_placing_model() -> Result<(), IdError> {
        let choose = |name: &str| -> Result<Trigger, IdError> {
            Ok(Trigger::Manual {
                account: Some(id(name)?),
            })
        };
        let all_out = fable_task(
            Some("a"),
            vec![
                account("a", fable_out(10.0, Some(40)))?,
                account("b", fable_out(20.0, Some(90)))?,
            ],
        )?;
        assert_eq!(
            decide(&all_out, choose("b")?),
            (
                Decision::Switch { account: id("b")? },
                DecisionReason::ManualChoice
            )
        );
        // While another subscription runs Fable, one that cannot is refused, as before.
        let mut one_left = all_out.clone();
        one_left
            .accounts
            .push(account("c", usage(20.0, 20.0, Some(100)))?);
        assert_eq!(
            decide(&one_left, choose("b")?).1,
            DecisionReason::ManualRefused(Refusal::Exhausted)
        );
        // Every 5-hour window full too: the wait ends at the first 5-hour reset (in 2 h), when
        // the fallback can run, not at the allowance's in 40 h.
        let mut full = all_out.clone();
        for account in &mut full.accounts {
            account.usage.primary = Some(window(100.0, Some(2)));
        }
        assert_eq!(
            decide(&full, Trigger::LimitReached),
            (
                Decision::Wait {
                    until: Some(NOW.plus_millis(2 * HOUR))
                },
                DecisionReason::NoCapacity
            )
        );
        full.fallback = None;
        assert_eq!(
            decide(&full, Trigger::LimitReached).0,
            Decision::Wait {
                until: Some(NOW.plus_millis(40 * HOUR))
            }
        );
        Ok(())
    }

    /// SPEC-482 FBK-007: a subscription is left out (SPEC-464 UCL-010) for the task's own model.
    /// The fallback's reset is one to wait for; a pool in fallback leaves nothing out; and a
    /// fallback that only leaving out would make is not taken (the launch would not agree).
    #[test]
    fn a_subscription_left_out_and_the_fallback() -> Result<(), IdError> {
        let refusing = id("refusing")?;
        // Its reading shows Fable again, and Opus is used up on it for 30 more hours: the task
        // waits for Opus, and is not started on it at every reading.
        let mut alone = fable_task(
            None,
            vec![account("refusing", usage(0.0, 10.0, Some(100)))?],
        )?;
        alone.accounts[0].usage.weekly_opus = Some(window(100.0, Some(30)));
        assert_eq!(
            switched_to(&decide(&alone, Trigger::Start).0),
            Some("refusing")
        );
        assert_eq!(
            decide_leaving_out(&alone, Trigger::Start, &refusing).0,
            Decision::Wait {
                until: Some(NOW.plus_millis(30 * HOUR))
            }
        );
        // The same for a session still held there.
        let mut held = alone.clone();
        held.current = Some(refusing.clone());
        assert_eq!(
            decide_leaving_out(&held, Trigger::Proactive, &refusing).0,
            Decision::Wait {
                until: Some(NOW.plus_millis(30 * HOUR))
            }
        );
        // Another subscription could run the fallback now, but only leaving out says the task
        // needs it: the launch would place Fable, so the task is tried where the reading shows
        // Fable. Refused there again, its limit is stored and the pool falls back by itself.
        let known = fable_task(
            None,
            vec![
                account("refusing", usage(0.0, 10.0, Some(100)))?,
                account("later", fable_out(10.0, Some(40)))?,
            ],
        )?;
        assert_eq!(placing_family(&known), ModelFamily::Fable);
        assert_eq!(
            switched_to(&decide_leaving_out(&known, Trigger::Start, &refusing).0),
            Some("refusing")
        );
        // A third subscription that runs Fable takes the task, as before.
        let mut third = known.clone();
        third
            .accounts
            .push(account("third", usage(20.0, 20.0, Some(100)))?);
        assert_eq!(
            switched_to(&decide_leaving_out(&third, Trigger::Start, &refusing).0),
            Some("third")
        );
        // The pool is in fallback anyway: the subscription is a candidate for the other model.
        let mut all_out = known.clone();
        all_out.accounts[0].usage = fable_out(0.0, None);
        all_out.accounts[1].in_use = 1;
        assert_eq!(
            switched_to(&decide_leaving_out(&all_out, Trigger::Start, &refusing).0),
            Some("refusing")
        );
        Ok(())
    }

    #[test]
    fn score_is_headroom_per_hour_shared_by_the_tasks() {
        assert!((score(48.0, 10.0, 0) - 4.8).abs() < 1e-12);
        assert!((score(48.0, 10.0, 1) - 2.4).abs() < 1e-12);
        // Bounds: a reset "now" counts as a quarter hour, an unknown one as a week.
        assert!((score(10.0, 0.0, 0) - 40.0).abs() < 1e-12);
        assert!((score(84.0, 1_000.0, 0) - 0.5).abs() < 1e-12);
        assert!(score(-3.0, 5.0, 0).abs() < 1e-12);
    }
}
