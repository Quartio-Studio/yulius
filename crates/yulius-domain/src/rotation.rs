//! Account rotation policy (SPEC-001 ROT-003..011 and §7.4, SPEC-002 §6.3).
//!
//! [`decide`] is a pure function of a [`RotationInput`] snapshot and a [`Trigger`]. Callers
//! persist the input together with the returned [`Decision`] and [`DecisionReason`] for audit
//! (ROT-011).
//!
//! Rotation stays within the task's provider: Yulius runs only Claude Code (F 2026-10-08), so
//! a task never moves to another harness. Candidate order among the provider's accounts:
//!
//! 1. enabled, healthy accounts that are not exhausted: known headroom before unknown usage,
//!    then lower `priority` value, then fewer `in_use`, then account id;
//! 2. accounts whose snapshot showed them exhausted but whose window or limit has reset (same
//!    tie-breakers);
//! 3. otherwise wait until the earliest known reset (ROT-010), or indefinitely when none is
//!    known.
//!
//! Accounts that report a manual reset entitlement are never chosen while exhausted, because
//! no provider API to trigger such a reset is verified (SPEC-001 U-03); they are surfaced in
//! [`DecisionReason::NoCapacity`] for an administrator instead.

use crate::harness::Provider;
use crate::ids::AccountId;
use crate::time::Timestamp;
use crate::usage::AccountUsage;

/// Default proactive switch threshold in percent (SPEC-001 ROT-003).
pub const DEFAULT_THRESHOLD: f64 = 99.0;

/// Usage at or above this percentage means the window is hard-exhausted.
const HARD_LIMIT_PERCENT: f64 = 100.0;

/// The policy's view of one connected account.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountView {
    /// Account identity.
    pub id: AccountId,
    /// Provider the account belongs to.
    pub provider: Provider,
    /// Administrator enabled the account.
    pub enabled: bool,
    /// Credentials work (not in an error or re-login state).
    pub healthy: bool,
    /// Administrator rank: a lower value is tried first.
    pub priority: i32,
    /// Latest usage snapshot.
    pub usage: AccountUsage,
    /// Manual reset entitlements reported by the provider; `None` when unknown.
    pub manual_resets_available: Option<u32>,
    /// Live attempts currently using the account (including the deciding task's own attempt).
    pub in_use: u32,
}

impl AccountView {
    fn usable(&self) -> bool {
        self.enabled && self.healthy
    }
}

/// Everything [`decide`] looks at.
#[derive(Debug, Clone, PartialEq)]
pub struct RotationInput {
    /// The account the task currently runs on (or last ran on).
    pub current: Option<AccountId>,
    /// Provider of the task's harness; only its accounts are candidates.
    pub task_provider: Provider,
    /// All connected accounts. For [`Trigger::LimitReached`], callers fold the observed limit
    /// into the current account's usage first so its reset time is known.
    pub accounts: Vec<AccountView>,
    /// Decision time.
    pub now: Timestamp,
    /// Proactive switch threshold in percent; usually [`DEFAULT_THRESHOLD`].
    pub threshold: f64,
}

/// Why a decision is requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// Turn boundary check: switch only when the current account is at or above the threshold.
    Proactive,
    /// The current account hit a usage limit; always move off it.
    LimitReached,
    /// A user asked for a specific account, or to rotate now (ROT-007).
    Manual {
        /// Requested account; `None` rotates to the best other account.
        account: Option<AccountId>,
    },
    /// A new attempt is starting (new task or Continue).
    Start,
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
    /// No account has capacity; wait (ROT-010).
    Wait {
        /// Earliest known time an account becomes available; `None` when unknown.
        until: Option<Timestamp>,
    },
}

/// Which candidate class a selected account came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    /// Usage is known and below the threshold.
    Headroom,
    /// Usage is unknown (never treated as zero, but not evidence of exhaustion either).
    UnknownUsage,
    /// The snapshot showed exhaustion, but the window or limit has since reset.
    WindowReset,
}

/// Why a manual request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// No account has the requested id.
    UnknownAccount,
    /// The requested account is disabled or unhealthy.
    Unavailable,
    /// The requested account is exhausted at decision time.
    Exhausted,
    /// The requested account belongs to another provider.
    ProviderMismatch,
    /// No usable account matches the request.
    NoCandidate,
}

/// Audit explanation for a [`Decision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionReason {
    /// The current account has known headroom.
    CurrentUsable,
    /// The current account's usage is unknown; there is no evidence to switch.
    CurrentUsageUnknown,
    /// The current account is at or above the threshold but below its hard limit and no
    /// alternative exists; keep using it until a limit is reached.
    NoAlternativeBelowLimit,
    /// The best candidate was selected automatically.
    Selected {
        /// Candidate class of the chosen account.
        tier: Tier,
    },
    /// No account has capacity.
    NoCapacity {
        /// Exhausted, usable accounts reporting manual reset entitlements, for administrators.
        manual_reset_candidates: Vec<AccountId>,
    },
    /// The user's explicitly chosen account was accepted.
    ManualChoice,
    /// The user chose the account the task already uses.
    ManualAlreadyCurrent,
    /// The user's request was refused; the task stays on its account.
    ManualRefused(Refusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Exhausted,
    Candidate(Tier),
}

fn classify(account: &AccountView, now: Timestamp, threshold: f64) -> Class {
    let usage = &account.usage;
    if usage.exhausted(now, threshold) {
        Class::Exhausted
    } else if usage.recovered_by_reset(now, threshold) {
        Class::Candidate(Tier::WindowReset)
    } else if usage.current_used_percent(now).is_some() {
        Class::Candidate(Tier::Headroom)
    } else {
        Class::Candidate(Tier::UnknownUsage)
    }
}

fn hard_exhausted(account: &AccountView, now: Timestamp) -> bool {
    let usage = &account.usage;
    usage.limit_in_effect(now)
        || usage
            .current_used_percent(now)
            .is_some_and(|p| p >= HARD_LIMIT_PERCENT)
}

/// Decides whether and where a task's run should move (SPEC-001 §7.4).
///
/// Never panics; an unknown current account is treated as unusable.
#[must_use]
pub fn decide(input: &RotationInput, trigger: Trigger) -> (Decision, DecisionReason) {
    match trigger {
        Trigger::Start => start(input),
        Trigger::Proactive => proactive(input),
        Trigger::LimitReached => select_or_wait(input, input.current.as_ref()),
        Trigger::Manual { account } => manual(input, account),
    }
}

fn find<'a>(input: &'a RotationInput, id: &AccountId) -> Option<&'a AccountView> {
    input.accounts.iter().find(|a| &a.id == id)
}

/// The current account when it is usable for the task's provider.
fn current_fit(input: &RotationInput) -> Option<&AccountView> {
    let current = find(input, input.current.as_ref()?)?;
    (current.usable() && current.provider == input.task_provider).then_some(current)
}

fn start(input: &RotationInput) -> (Decision, DecisionReason) {
    if let Some(current) = current_fit(input) {
        match classify(current, input.now, input.threshold) {
            Class::Candidate(Tier::UnknownUsage) => {
                return (Decision::Stay, DecisionReason::CurrentUsageUnknown);
            }
            Class::Candidate(_) => return (Decision::Stay, DecisionReason::CurrentUsable),
            Class::Exhausted => {}
        }
    }
    // No exclusion: an exhausted or unusable current account is filtered out anyway.
    select_or_wait(input, None)
}

fn proactive(input: &RotationInput) -> (Decision, DecisionReason) {
    let Some(current) = current_fit(input) else {
        return select_or_wait(input, input.current.as_ref());
    };
    match classify(current, input.now, input.threshold) {
        Class::Candidate(Tier::UnknownUsage) => {
            (Decision::Stay, DecisionReason::CurrentUsageUnknown)
        }
        Class::Candidate(_) => (Decision::Stay, DecisionReason::CurrentUsable),
        Class::Exhausted if hard_exhausted(current, input.now) => {
            select_or_wait(input, Some(&current.id))
        }
        Class::Exhausted => {
            // Below the hard limit: keep the current account while no other one has room.
            match best(input, Some(&current.id)) {
                Some((account, tier)) => switch(account, tier),
                None => (Decision::Stay, DecisionReason::NoAlternativeBelowLimit),
            }
        }
    }
}

fn manual(input: &RotationInput, account: Option<AccountId>) -> (Decision, DecisionReason) {
    let refuse = |refusal| (Decision::Stay, DecisionReason::ManualRefused(refusal));
    let Some(id) = account else {
        // "Rotate now": the best other account.
        return match best(input, input.current.as_ref()) {
            Some((target, tier)) => switch(target, tier),
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
    if !target.usable() {
        return refuse(Refusal::Unavailable);
    }
    if classify(target, input.now, input.threshold) == Class::Exhausted {
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

/// Best non-exhausted, usable account of the task's provider.
fn best<'a>(
    input: &'a RotationInput,
    exclude: Option<&AccountId>,
) -> Option<(&'a AccountView, Tier)> {
    input
        .accounts
        .iter()
        .filter(|a| a.usable() && a.provider == input.task_provider && Some(&a.id) != exclude)
        .filter_map(|a| match classify(a, input.now, input.threshold) {
            Class::Candidate(tier) => Some((a, tier)),
            Class::Exhausted => None,
        })
        .min_by(|(a, ta), (b, tb)| {
            (ta, a.priority, a.in_use, &a.id).cmp(&(tb, b.priority, b.in_use, &b.id))
        })
}

fn select_or_wait(
    input: &RotationInput,
    exclude: Option<&AccountId>,
) -> (Decision, DecisionReason) {
    match best(input, exclude) {
        Some((account, tier)) => switch(account, tier),
        None => wait(input),
    }
}

fn wait(input: &RotationInput) -> (Decision, DecisionReason) {
    let considered = input.accounts.iter().filter(|a| {
        a.usable()
            && a.provider == input.task_provider
            && a.usage.exhausted(input.now, input.threshold)
    });
    let mut until: Option<Timestamp> = None;
    let mut manual_reset_candidates = Vec::new();
    for account in considered {
        if let Some(at) = account.usage.available_at(input.now, input.threshold) {
            until = Some(until.map_or(at, |u| u.min(at)));
        }
        if account.manual_resets_available.is_some_and(|n| n > 0) {
            manual_reset_candidates.push(account.id.clone());
        }
    }
    manual_reset_candidates.sort();
    (
        Decision::Wait { until },
        DecisionReason::NoCapacity {
            manual_reset_candidates,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::IdError;
    use crate::usage::UsageWindow;

    const NOW: Timestamp = Timestamp(10_000_000);
    const A: Provider = Provider::Anthropic;

    fn id(text: &str) -> Result<AccountId, IdError> {
        AccountId::parse(text)
    }

    fn usage(percent: Option<f64>, resets_at: Option<i64>) -> AccountUsage {
        AccountUsage {
            primary: percent.map(|p| UsageWindow {
                used_percent: Some(p),
                window_minutes: Some(300),
                resets_at: resets_at.map(Timestamp),
            }),
            ..AccountUsage::default()
        }
    }

    fn account(name: &str, usage: AccountUsage) -> Result<AccountView, IdError> {
        Ok(AccountView {
            id: id(name)?,
            provider: A,
            enabled: true,
            healthy: true,
            priority: 0,
            usage,
            manual_resets_available: None,
            in_use: 0,
        })
    }

    fn input(current: Option<&str>, accounts: Vec<AccountView>) -> Result<RotationInput, IdError> {
        Ok(RotationInput {
            current: current.map(id).transpose()?,
            task_provider: A,
            accounts,
            now: NOW,
            threshold: DEFAULT_THRESHOLD,
        })
    }

    fn switched_to(decision: &Decision) -> Option<&str> {
        match decision {
            Decision::Switch { account } => Some(account.as_str()),
            _ => None,
        }
    }

    fn manual(account: &str) -> Result<Trigger, IdError> {
        Ok(Trigger::Manual {
            account: Some(id(account)?),
        })
    }

    #[test]
    fn ac_rot_2_proactive_switch_at_threshold_to_the_next_account() -> Result<(), IdError> {
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(99.0), Some(NOW.0 + 1_000)))?,
                account("a2", usage(Some(10.0), None))?,
            ],
        )?;
        assert_eq!(
            decide(&rotation, Trigger::Proactive),
            (
                Decision::Switch { account: id("a2")? },
                DecisionReason::Selected {
                    tier: Tier::Headroom
                }
            )
        );
        Ok(())
    }

    #[test]
    fn proactive_stays_below_threshold_and_with_unknown_usage() -> Result<(), IdError> {
        let others = account("a2", usage(Some(0.0), None))?;
        let below = input(
            Some("a1"),
            vec![account("a1", usage(Some(98.9), None))?, others.clone()],
        )?;
        assert_eq!(
            decide(&below, Trigger::Proactive),
            (Decision::Stay, DecisionReason::CurrentUsable)
        );
        let unknown = input(
            Some("a1"),
            vec![account("a1", AccountUsage::default())?, others],
        )?;
        assert_eq!(
            decide(&unknown, Trigger::Proactive),
            (Decision::Stay, DecisionReason::CurrentUsageUnknown)
        );
        Ok(())
    }

    #[test]
    fn proactive_keeps_a_working_account_without_alternative() -> Result<(), IdError> {
        let soft = input(Some("a1"), vec![account("a1", usage(Some(99.5), None))?])?;
        assert_eq!(
            decide(&soft, Trigger::Proactive),
            (Decision::Stay, DecisionReason::NoAlternativeBelowLimit)
        );
        let hard = input(
            Some("a1"),
            vec![account("a1", usage(Some(100.0), Some(NOW.0 + 5_000)))?],
        )?;
        assert_eq!(
            decide(&hard, Trigger::Proactive).0,
            Decision::Wait {
                until: Some(Timestamp(NOW.0 + 5_000))
            }
        );
        Ok(())
    }

    #[test]
    fn limit_reached_always_moves_off_current_even_with_stale_headroom() -> Result<(), IdError> {
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(5.0), None))?,
                account("a2", AccountUsage::default())?,
            ],
        )?;
        let (decision, reason) = decide(&rotation, Trigger::LimitReached);
        assert_eq!(switched_to(&decision), Some("a2"));
        assert_eq!(
            reason,
            DecisionReason::Selected {
                tier: Tier::UnknownUsage
            }
        );
        Ok(())
    }

    #[test]
    fn known_headroom_beats_unknown_then_priority_in_use_and_id() -> Result<(), IdError> {
        let mut unknown_preferred = account("a-unknown", AccountUsage::default())?;
        unknown_preferred.priority = -10;
        let mut known_low_priority = account("a-known", usage(Some(80.0), None))?;
        known_low_priority.priority = 5;
        let rotation = input(None, vec![unknown_preferred, known_low_priority])?;
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Start).0),
            Some("a-known")
        );

        let mut high = account("b", usage(Some(50.0), None))?;
        high.priority = 1;
        let mut busy = account("c", usage(Some(50.0), None))?;
        busy.in_use = 2;
        let idle_z = account("z", usage(Some(50.0), None))?;
        let idle_d = account("d", usage(Some(50.0), None))?;
        let rotation = input(
            None,
            vec![high.clone(), busy.clone(), idle_z.clone(), idle_d],
        )?;
        assert_eq!(switched_to(&decide(&rotation, Trigger::Start).0), Some("d"));
        let rotation = input(None, vec![high.clone(), busy.clone(), idle_z])?;
        assert_eq!(switched_to(&decide(&rotation, Trigger::Start).0), Some("z"));
        let rotation = input(None, vec![high, busy])?;
        assert_eq!(switched_to(&decide(&rotation, Trigger::Start).0), Some("c"));
        Ok(())
    }

    #[test]
    fn disabled_and_unhealthy_accounts_are_never_chosen() -> Result<(), IdError> {
        let mut disabled = account("a2", usage(Some(0.0), None))?;
        disabled.enabled = false;
        let mut unhealthy = account("a3", usage(Some(0.0), None))?;
        unhealthy.healthy = false;
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(100.0), None))?,
                disabled,
                unhealthy,
            ],
        )?;
        assert_eq!(
            decide(&rotation, Trigger::LimitReached),
            (
                Decision::Wait { until: None },
                DecisionReason::NoCapacity {
                    manual_reset_candidates: vec![]
                }
            )
        );
        Ok(())
    }

    #[test]
    fn ac_rot_3_an_account_whose_window_reset_is_chosen() -> Result<(), IdError> {
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(100.0), Some(NOW.0 + 60_000)))?,
                account("a2", usage(Some(100.0), Some(NOW.0 - 1)))?,
            ],
        )?;
        let (decision, reason) = decide(&rotation, Trigger::LimitReached);
        assert_eq!(switched_to(&decision), Some("a2"));
        assert_eq!(
            reason,
            DecisionReason::Selected {
                tier: Tier::WindowReset
            }
        );
        Ok(())
    }

    #[test]
    fn reset_limit_flag_counts_as_window_reset() -> Result<(), IdError> {
        let reset_limit = AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(NOW),
            ..AccountUsage::default()
        };
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(100.0), None))?,
                account("a2", reset_limit)?,
            ],
        )?;
        let (decision, reason) = decide(&rotation, Trigger::LimitReached);
        assert_eq!(switched_to(&decision), Some("a2"));
        assert_eq!(
            reason,
            DecisionReason::Selected {
                tier: Tier::WindowReset
            }
        );
        Ok(())
    }

    #[test]
    fn ac_rot_6_wait_for_earliest_reset_then_resume_when_it_passes() -> Result<(), IdError> {
        let accounts = vec![
            account("a1", usage(Some(100.0), Some(NOW.0 + 90_000)))?,
            account("a2", usage(Some(100.0), Some(NOW.0 + 30_000)))?,
            account("a3", usage(Some(100.0), None))?,
        ];
        let rotation = input(Some("a1"), accounts)?;
        assert_eq!(
            decide(&rotation, Trigger::LimitReached).0,
            Decision::Wait {
                until: Some(Timestamp(NOW.0 + 30_000))
            }
        );
        let later = RotationInput {
            now: Timestamp(NOW.0 + 30_000),
            ..rotation
        };
        let (decision, reason) = decide(&later, Trigger::Start);
        assert_eq!(switched_to(&decision), Some("a2"));
        assert_eq!(
            reason,
            DecisionReason::Selected {
                tier: Tier::WindowReset
            }
        );
        Ok(())
    }

    #[test]
    fn wait_until_unknown_when_no_reset_is_known_and_lists_manual_resets() -> Result<(), IdError> {
        let mut with_manual = account("a2", usage(Some(100.0), None))?;
        with_manual.manual_resets_available = Some(1);
        let mut disabled_with_reset = account("a3", usage(Some(100.0), Some(NOW.0 + 1)))?;
        disabled_with_reset.enabled = false;
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(100.0), None))?,
                with_manual,
                disabled_with_reset,
            ],
        )?;
        assert_eq!(
            decide(&rotation, Trigger::LimitReached),
            (
                Decision::Wait { until: None },
                DecisionReason::NoCapacity {
                    manual_reset_candidates: vec![id("a2")?]
                }
            )
        );
        Ok(())
    }

    #[test]
    fn start_keeps_usable_current_and_replaces_exhausted_current() -> Result<(), IdError> {
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(30.0), None))?,
                account("a2", usage(Some(0.0), None))?,
            ],
        )?;
        assert_eq!(
            decide(&rotation, Trigger::Start),
            (Decision::Stay, DecisionReason::CurrentUsable)
        );
        let exhausted = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(99.0), None))?,
                account("a2", usage(Some(0.0), None))?,
            ],
        )?;
        assert_eq!(
            switched_to(&decide(&exhausted, Trigger::Start).0),
            Some("a2")
        );
        Ok(())
    }

    #[test]
    fn manual_account_choice_rules() -> Result<(), IdError> {
        let mut disabled = account("a3", usage(Some(0.0), None))?;
        disabled.enabled = false;
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(10.0), None))?,
                account("a2", usage(Some(100.0), Some(NOW.0 + 1_000)))?,
                disabled,
                account("a4", AccountUsage::default())?,
            ],
        )?;
        assert_eq!(
            decide(&rotation, manual("a2")?),
            (
                Decision::Stay,
                DecisionReason::ManualRefused(Refusal::Exhausted)
            )
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
        Ok(())
    }

    #[test]
    fn rotate_now_picks_the_best_other_account_or_refuses() -> Result<(), IdError> {
        let rotation = input(
            Some("a1"),
            vec![
                account("a1", usage(Some(10.0), None))?,
                account("a2", usage(Some(40.0), None))?,
            ],
        )?;
        let now = Trigger::Manual { account: None };
        assert_eq!(switched_to(&decide(&rotation, now.clone()).0), Some("a2"));
        let alone = input(Some("a1"), vec![account("a1", usage(Some(10.0), None))?])?;
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
        let rotation = input(Some("gone"), vec![account("a2", usage(Some(1.0), None))?])?;
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Proactive).0),
            Some("a2")
        );
        assert_eq!(
            switched_to(&decide(&rotation, Trigger::Start).0),
            Some("a2")
        );
        Ok(())
    }
}
