//! A terminal viewer's write grant (SPEC-118, ADR-0096): a keystroke is written to the PTY at once
//! when this viewer's write access was confirmed and nothing that decides access changed since
//! (the store's access epoch). A grant confirmed at an older epoch is checked again before the
//! next input; one older than [`GRANT_REFRESH_MS`] is re-confirmed beside the input, so a GitHub
//! downgrade still reaches the keyboard once the permission cache expires.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use crate::context::App;

/// Age after which a confirmed grant is re-confirmed in the background.
pub const GRANT_REFRESH_MS: i64 = 30_000;

/// What to do with an input frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Write it: the grant holds.
    Allow,
    /// Write it, and re-confirm the grant beside it (it is old).
    AllowAndRefresh,
    /// Check write access first: nothing is confirmed at the current epoch.
    Check,
}

#[derive(Debug, Clone, Copy)]
struct Confirmed {
    epoch: u64,
    at_ms: i64,
}

#[derive(Debug, Default)]
struct State {
    confirmed: Option<Confirmed>,
    refreshing: bool,
}

/// One viewer's write grant (cheap to share).
#[derive(Debug, Default)]
pub struct WriteGrant(Mutex<State>);

impl WriteGrant {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What to do with input at access epoch `epoch` and time `now_ms`. While a background
    /// re-confirmation is pending, no other is asked for.
    pub fn decide(&self, epoch: u64, now_ms: i64) -> Decision {
        let mut state = self.state();
        match state.confirmed {
            Some(c) if c.epoch == epoch => {
                if now_ms.saturating_sub(c.at_ms) >= GRANT_REFRESH_MS && !state.refreshing {
                    state.refreshing = true;
                    Decision::AllowAndRefresh
                } else {
                    Decision::Allow
                }
            }
            _ => Decision::Check,
        }
    }

    /// Records a check that began at `epoch` and `at_ms`: a refusal drops the grant (the next
    /// input is checked again).
    pub fn settle(&self, epoch: u64, at_ms: i64, allowed: bool) {
        let mut state = self.state();
        state.refreshing = false;
        state.confirmed = allowed.then_some(Confirmed { epoch, at_ms });
    }

    /// Re-confirms the grant in a background task (a check that began at `epoch` and `at_ms`).
    /// A check that fails or is cancelled counts as a refusal, so a stuck flag never stops later
    /// re-confirmations. The task is not tied to the viewer: it ends with the check (GitHub calls
    /// have their own timeout). Normally one runs per grant; a check before the input (after an
    /// access change) may let one more start, which costs only an extra check.
    pub fn refresh(
        self: &Arc<Self>,
        epoch: u64,
        at_ms: i64,
        check: impl Future<Output = bool> + Send + 'static,
    ) -> tokio::task::JoinHandle<()> {
        let verdict = Verdict {
            grant: Arc::clone(self),
            epoch,
            at_ms,
            allowed: false,
        };
        tokio::spawn(async move {
            // Bind the guard itself: assigning to a field alone would capture only that field,
            // and the guard would settle (as a refusal) before the check ran.
            let mut verdict = verdict;
            verdict.allowed = check.await;
        })
    }
}

/// Settles a background re-confirmation when it ends, however it ends.
struct Verdict {
    grant: Arc<WriteGrant>,
    epoch: u64,
    at_ms: i64,
    allowed: bool,
}

impl Drop for Verdict {
    fn drop(&mut self) {
        self.grant.settle(self.epoch, self.at_ms, self.allowed);
    }
}

impl App {
    /// Whether a viewer holding `grant` may write now. `check` is the full write-access check
    /// (database and, when the permission cache expired, GitHub); it runs before the input only
    /// when the grant does not hold at the current access epoch, and beside it when the grant is
    /// merely old.
    pub(crate) async fn write_granted<F, Fut>(
        self: &Arc<Self>,
        grant: &Arc<WriteGrant>,
        check: F,
    ) -> bool
    where
        F: FnOnce(Arc<App>) -> Fut,
        Fut: Future<Output = bool> + Send + 'static,
    {
        let epoch = self.deps.store.access_epoch();
        let now = self.now().0;
        match grant.decide(epoch, now) {
            Decision::Allow => true,
            Decision::AllowAndRefresh => {
                drop(grant.refresh(epoch, now, check(Arc::clone(self))));
                true
            }
            Decision::Check => {
                let allowed = check(Arc::clone(self)).await;
                grant.settle(epoch, now, allowed);
                allowed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decision, GRANT_REFRESH_MS, WriteGrant};

    #[test]
    fn a_new_grant_is_checked_first() {
        assert_eq!(WriteGrant::default().decide(0, 0), Decision::Check);
    }

    #[test]
    fn a_confirmed_grant_lets_input_through_while_the_epoch_holds() {
        let grant = WriteGrant::default();
        grant.settle(3, 1_000, true);
        assert_eq!(grant.decide(3, 1_000), Decision::Allow);
        assert_eq!(
            grant.decide(3, 1_000 + GRANT_REFRESH_MS - 1),
            Decision::Allow
        );
        // KEY-002: anything that changes access checks the very next input.
        assert_eq!(grant.decide(4, 1_001), Decision::Check);
    }

    #[test]
    fn an_old_grant_is_re_confirmed_once_beside_the_input() {
        let grant = WriteGrant::default();
        grant.settle(1, 0, true);
        let old = GRANT_REFRESH_MS;
        assert_eq!(grant.decide(1, old), Decision::AllowAndRefresh);
        assert_eq!(
            grant.decide(1, old + 5),
            Decision::Allow,
            "one re-confirmation at a time"
        );
        grant.settle(1, old, true);
        assert_eq!(grant.decide(1, old + 5), Decision::Allow);
        assert_eq!(grant.decide(1, 2 * old), Decision::AllowAndRefresh);
    }

    #[tokio::test]
    async fn input_flows_while_a_re_confirmation_is_pending() {
        let grant = std::sync::Arc::new(WriteGrant::default());
        grant.settle(1, 0, true);
        assert_eq!(grant.decide(1, GRANT_REFRESH_MS), Decision::AllowAndRefresh);
        let pending = grant.refresh(1, GRANT_REFRESH_MS, std::future::pending::<bool>());
        tokio::task::yield_now().await;
        assert_eq!(grant.decide(1, GRANT_REFRESH_MS + 1), Decision::Allow);
        pending.abort();
    }

    #[tokio::test]
    async fn a_re_confirmation_that_fails_checks_the_next_input() {
        let grant = std::sync::Arc::new(WriteGrant::default());
        grant.settle(1, 0, true);
        assert_eq!(grant.decide(1, GRANT_REFRESH_MS), Decision::AllowAndRefresh);
        let failed = grant
            .refresh(1, GRANT_REFRESH_MS, async {
                unreachable!("the check failed")
            })
            .await;
        assert!(failed.is_err(), "the check panicked");
        assert_eq!(grant.decide(1, GRANT_REFRESH_MS + 1), Decision::Check);
    }

    #[test]
    fn a_refusal_checks_the_next_input() {
        let grant = WriteGrant::default();
        grant.settle(1, 0, true);
        assert_eq!(grant.decide(1, GRANT_REFRESH_MS), Decision::AllowAndRefresh);
        grant.settle(1, GRANT_REFRESH_MS, false);
        assert_eq!(grant.decide(1, GRANT_REFRESH_MS + 1), Decision::Check);
    }
}
