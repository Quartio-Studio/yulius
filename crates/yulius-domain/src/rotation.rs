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
//! 1. [`Tier`]: known headroom of at least [`MIN_ROOM`] points, then known headroom below it (a
//!    task placed there would move again within minutes and pay for a cold prompt cache), then
//!    unknown usage (never treated as zero, but not evidence of exhaustion either);
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
use crate::usage::{AccountUsage, ModelFamily, UsageWindow};

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
    /// Known headroom below [`MIN_ROOM`] points in some binding window.
    Tight,
    /// No binding window has a known percentage.
    UnknownUsage,
}

/// Why a manual request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// No account has the requested id.
    UnknownAccount,
    /// The requested account is not signed in or broken.
    Unavailable,
    /// The requested account is at the threshold or limited at decision time.
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

/// Candidates for a task of the input's provider and family, best first, excluding `exclude`.
#[must_use]
pub fn ranked<'a>(
    input: &'a RotationInput,
    exclude: Option<&AccountId>,
) -> Vec<(&'a AccountView, Assessment)> {
    let mut candidates: Vec<(&AccountView, Assessment)> = input
        .accounts
        .iter()
        .filter(|a| a.provider == input.task_provider && Some(&a.id) != exclude)
        .filter_map(|a| {
            assess(a, input.now, input.threshold, input.family).map(|assessment| (a, assessment))
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
/// Never panics; an unknown current account is treated as unusable.
#[must_use]
pub fn decide(input: &RotationInput, trigger: Trigger) -> (Decision, DecisionReason) {
    match trigger {
        Trigger::Start => select_or_wait(input, None),
        Trigger::Proactive => proactive(input),
        Trigger::LimitReached => select_or_wait(input, input.current.as_ref()),
        Trigger::Manual { account } => manual(input, account),
    }
}

fn find<'a>(input: &'a RotationInput, id: &AccountId) -> Option<&'a AccountView> {
    input.accounts.iter().find(|a| &a.id == id)
}

fn proactive(input: &RotationInput) -> (Decision, DecisionReason) {
    let current = input
        .current
        .as_ref()
        .and_then(|id| find(input, id))
        .filter(|a| a.provider == input.task_provider);
    if let Some(current) = current
        && let Some(assessment) = assess(current, input.now, input.threshold, input.family)
    {
        let reason = if assessment.tier == Tier::UnknownUsage {
            DecisionReason::CurrentUsageUnknown
        } else {
            DecisionReason::CurrentUsable
        };
        return (Decision::Stay, reason);
    }
    select_or_wait(input, input.current.as_ref())
}

fn manual(input: &RotationInput, account: Option<AccountId>) -> (Decision, DecisionReason) {
    let refuse = |refusal| (Decision::Stay, DecisionReason::ManualRefused(refusal));
    let Some(id) = account else {
        // "Rotate now": the best other account.
        return match ranked(input, input.current.as_ref()).first() {
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
    if assess(target, input.now, input.threshold, input.family).is_none() {
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

fn select_or_wait(
    input: &RotationInput,
    exclude: Option<&AccountId>,
) -> (Decision, DecisionReason) {
    match ranked(input, exclude).first() {
        Some((account, assessment)) => switch(account, assessment.tier),
        None => (
            Decision::Wait {
                until: available_again(input),
            },
            DecisionReason::NoCapacity,
        ),
    }
}

/// The earliest known time a healthy account of the task's provider becomes a candidate again
/// (ROT-010); `None` when no such time is known (for example no account at all, or limits
/// without a reset time).
#[must_use]
pub fn available_again(input: &RotationInput) -> Option<Timestamp> {
    input
        .accounts
        .iter()
        .filter(|a| a.healthy && a.provider == input.task_provider)
        .filter_map(|a| {
            a.usage
                .available_at_for(input.now, input.threshold, input.family)
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
    fn roomy_beats_tight_beats_unknown() -> Result<(), IdError> {
        // "tight" expires much sooner (high score) but has 2 points of 5-hour room left.
        let tight = account("tight", usage(96.0, 50.0, Some(2)))?;
        let roomy = account("roomy", usage(10.0, 10.0, Some(160)))?;
        let unknown = account("unknown", AccountUsage::default())?;
        assert_eq!(
            start(vec![tight.clone(), roomy.clone(), unknown.clone()])?.as_deref(),
            Some("roomy")
        );
        assert_eq!(
            start(vec![tight.clone(), unknown.clone()])?.as_deref(),
            Some("tight")
        );
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
