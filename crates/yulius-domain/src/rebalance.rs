//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629, ADR-0461):
//! the rebalance threshold of each harness, and what the rebalance pass does with an idle run:
//! [`idle_move`]. [`crate::rotation`] places runs off a subscription past the threshold while
//! another has room.

use crate::harness::Provider;
use crate::ids::AccountId;
use crate::rotation::{AccountView, REBALANCE, RotationInput, past_rebalance, ranked};
use crate::usage::{ModelFamily, UsageWindow};

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

/// Which window put a subscription past the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// The 5-hour window.
    FiveHour,
    /// The subscription's week.
    Week,
    /// The weekly window of one model family (Fable's allowance, Opus's or Sonnet's cap).
    ModelWeek(ModelFamily),
    /// A usage limit Claude reported is in effect.
    Limit,
}

/// How full a subscription past the threshold is: its fullest binding window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fullest {
    /// The window.
    pub window: Window,
    /// Its percent used; `None` for [`Window::Limit`].
    pub percent: Option<f64>,
}

impl Fullest {
    /// How full it is, in words after the subscription's name: "is at 96% of its 5-hour window",
    /// "is at 97% of its Fable week", "is at its usage limit".
    #[must_use]
    pub fn said(&self) -> String {
        let window = match self.window {
            Window::FiveHour => "5-hour window",
            Window::Week => "week",
            Window::ModelWeek(ModelFamily::Fable) => "Fable week",
            Window::ModelWeek(ModelFamily::Opus) => "Opus week",
            Window::ModelWeek(ModelFamily::Sonnet) => "Sonnet week",
            Window::ModelWeek(ModelFamily::Any) | Window::Limit => {
                return "is at its usage limit".into();
            }
        };
        let percent = self.percent.unwrap_or(100.0).clamp(0.0, 100.0).floor();
        format!("is at {percent}% of its {window}")
    }
}

/// What the rebalance pass does with an idle run (SPEC-629 RBL-003, RBL-007).
#[derive(Debug, Clone, PartialEq)]
pub enum IdleMove {
    /// Its subscription is below the threshold, its usage is unknown, or it has none.
    Stay,
    /// It moves to `account`, the best subscription below the threshold.
    Move {
        /// Where it goes.
        account: AccountId,
        /// Why it leaves.
        from: Fullest,
    },
    /// Its subscription is past the threshold and no other subscription is below it.
    NoRoom {
        /// How full its subscription is.
        from: Fullest,
    },
}

/// What the rebalance pass does with the run of `input.current`, launched with
/// `input.running` (the task's own model when `None`): past the input's rebalance threshold for
/// that model, it goes to the best candidate below the threshold for the run placed now
/// ([`ranked`]), and with none it stays and says so.
#[must_use]
pub fn idle_move(input: &RotationInput) -> IdleMove {
    let Some(current) = input
        .current
        .as_ref()
        .and_then(|id| input.accounts.iter().find(|a| &a.id == id))
        .filter(|a| a.provider == input.task_provider)
    else {
        return IdleMove::Stay;
    };
    let Some(from) = fullest_past(input, current, input.running.unwrap_or(input.family)) else {
        return IdleMove::Stay;
    };
    let family = crate::rotation::placing_family(input);
    match ranked(input, Some(&current.id))
        .into_iter()
        .find(|(account, _)| !past_rebalance(input, account, family))
    {
        Some((account, _)) => IdleMove::Move {
            account: account.id.clone(),
            from,
        },
        None => IdleMove::NoRoom { from },
    }
}

/// The fullest window binding `family` on `account` when it is past the input's rebalance
/// threshold; `None` when it is not.
#[must_use]
pub fn fullest_past(
    input: &RotationInput,
    account: &AccountView,
    family: ModelFamily,
) -> Option<Fullest> {
    if !past_rebalance(input, account, family) {
        return None;
    }
    let usage = &account.usage;
    if usage.limit_in_effect(input.now) {
        return Some(Fullest {
            window: Window::Limit,
            percent: None,
        });
    }
    let windows: [(Option<&UsageWindow>, Window); 3] = [
        (usage.primary.as_ref(), Window::FiveHour),
        (usage.secondary.as_ref(), Window::Week),
        (usage.model_window(family), Window::ModelWeek(family)),
    ];
    windows
        .into_iter()
        .filter_map(|(window, name)| Some((window?.current_percent(input.now)?, name)))
        .filter(|(percent, _)| *percent >= input.rebalance)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(percent, window)| Fullest {
            window,
            percent: Some(percent),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::IdError;
    use crate::rotation::THRESHOLD;
    use crate::time::Timestamp;
    use crate::usage::AccountUsage;

    const NOW: Timestamp = Timestamp(10_000_000_000);
    const HOUR: i64 = 3_600_000;

    fn window(percent: f64, resets_in_hours: i64) -> UsageWindow {
        UsageWindow {
            used_percent: Some(percent),
            window_minutes: None,
            resets_at: Some(NOW.plus_millis(resets_in_hours * HOUR)),
        }
    }

    fn account(name: &str, five: f64, week: f64) -> Result<AccountView, IdError> {
        Ok(AccountView {
            id: AccountId::parse(name)?,
            provider: Provider::Anthropic,
            healthy: true,
            usage: AccountUsage {
                primary: Some(window(five, 2)),
                secondary: Some(window(week, 100)),
                ..AccountUsage::default()
            },
            in_use: 0,
        })
    }

    fn on(current: &str, accounts: Vec<AccountView>) -> Result<RotationInput, IdError> {
        Ok(RotationInput {
            current: Some(AccountId::parse(current)?),
            task_provider: Provider::Anthropic,
            family: ModelFamily::Opus,
            fallback: None,
            running: None,
            accounts,
            now: NOW,
            threshold: THRESHOLD,
            rebalance: REBALANCE,
        })
    }

    fn moves_to(decision: &IdleMove) -> Option<&str> {
        match decision {
            IdleMove::Move { account, .. } => Some(account.as_str()),
            _ => None,
        }
    }

    /// RBL-003: past the threshold, an idle run goes to the best subscription below it, never
    /// to another one past it; below it, or with its usage unknown, it stays.
    #[test]
    fn an_idle_run_leaves_a_subscription_past_the_threshold() -> Result<(), IdError> {
        let mut pool = on(
            "a-full",
            vec![
                account("a-full", 96.0, 40.0)?,
                account("b-past", 10.0, 95.5)?,
                account("c-room", 30.0, 60.0)?,
                account("d-most", 5.0, 20.0)?,
            ],
        )?;
        let decision = idle_move(&pool);
        assert_eq!(moves_to(&decision), Some("d-most"));
        assert_eq!(
            decision,
            IdleMove::Move {
                account: AccountId::parse("d-most")?,
                from: Fullest {
                    window: Window::FiveHour,
                    percent: Some(96.0)
                }
            }
        );
        // Below the threshold: it stays.
        pool.accounts[0] = account("a-full", 94.9, 40.0)?;
        assert_eq!(idle_move(&pool), IdleMove::Stay);
        // Unknown usage is not past it.
        pool.accounts[0].usage = AccountUsage::default();
        assert_eq!(idle_move(&pool), IdleMove::Stay);
        // Its harness's threshold decides: at 90 % a run at 92 % leaves too.
        pool.accounts[0] = account("a-full", 92.0, 40.0)?;
        assert_eq!(idle_move(&pool), IdleMove::Stay);
        pool.rebalance = 90.0;
        assert_eq!(moves_to(&idle_move(&pool)), Some("d-most"));
        // Another provider's subscription is never the target.
        pool.rebalance = REBALANCE;
        pool.accounts[0] = account("a-full", 97.0, 40.0)?;
        pool.accounts[3].provider = Provider::OpenAi;
        assert_eq!(moves_to(&idle_move(&pool)), Some("c-room"));
        Ok(())
    }

    /// RBL-007: with every other subscription past the threshold (or unusable), the run stays
    /// and the decision says how full its own is.
    #[test]
    fn nothing_has_room_keeps_the_run_and_says_why() -> Result<(), IdError> {
        let mut pool = on(
            "a-full",
            vec![
                account("a-full", 10.0, 97.0)?,
                account("b-past", 96.0, 10.0)?,
            ],
        )?;
        assert_eq!(
            idle_move(&pool),
            IdleMove::NoRoom {
                from: Fullest {
                    window: Window::Week,
                    percent: Some(97.0)
                }
            }
        );
        // A subscription below the threshold that is not healthy has no room either.
        pool.accounts.push(account("c-broken", 5.0, 5.0)?);
        pool.accounts[2].healthy = false;
        assert!(matches!(idle_move(&pool), IdleMove::NoRoom { .. }));
        pool.accounts[2].healthy = true;
        assert_eq!(moves_to(&idle_move(&pool)), Some("c-broken"));
        Ok(())
    }

    /// RBL-001: the model's own week counts for a run of that model, and a limit in effect is
    /// past any threshold.
    #[test]
    fn the_models_own_week_and_a_limit_count() -> Result<(), IdError> {
        let mut pool = on(
            "a-fable",
            vec![
                account("a-fable", 10.0, 10.0)?,
                account("b-room", 10.0, 10.0)?,
            ],
        )?;
        pool.family = ModelFamily::Fable;
        pool.accounts[0].usage.weekly_fable = Some(window(96.4, 50));
        let decision = idle_move(&pool);
        assert_eq!(moves_to(&decision), Some("b-room"));
        assert!(
            matches!(&decision, IdleMove::Move { from, .. } if from.said() == "is at 96% of its Fable week"),
            "{decision:?}"
        );
        // An Opus run on it is not bound by Fable's week.
        pool.running = Some(ModelFamily::Opus);
        assert_eq!(idle_move(&pool), IdleMove::Stay);
        // A limit in effect.
        pool.accounts[0].usage.limit_reached = true;
        let decision = idle_move(&pool);
        assert!(
            matches!(&decision, IdleMove::Move { from, .. } if from.said() == "is at its usage limit"),
            "{decision:?}"
        );
        assert_eq!(
            Fullest {
                window: Window::FiveHour,
                percent: Some(95.0)
            }
            .said(),
            "is at 95% of its 5-hour window"
        );
        Ok(())
    }

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
