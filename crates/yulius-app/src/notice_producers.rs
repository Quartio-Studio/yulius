//! Yulius's own repeated notices (SPEC-624 NTC-010): each kind is posted under one key, so its
//! next state replaces the last, and removed once its problem is gone.
//!
//! - subscription problems: `account:<id>`, for instance administrators, from the event bus's
//!   `account.updated` ([`App::sync_account_notice`]); a usage limit expires at its reset;
//! - integration failures nobody repairs: `gate:<test>`, for the project's people, from the
//!   staging tick ([`App::sync_gate_notices`]).

use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use yulius_contracts::api::admin::{AccountUsageView, AccountView};
use yulius_contracts::api::ci::GateNotice;
use yulius_contracts::events::UiEvent;
use yulius_domain::notice::{EXPIRY_MAX_MS, Expiry, NoticeKey, NoticeTone};
use yulius_domain::notify::{NoticeKind, account_problem};
use yulius_domain::time::{Timestamp, parse_rfc3339};

use crate::context::App;
use crate::error::AppResult;
use crate::model::SandboxRecord;
use crate::notices::NewNotice;
use crate::ports::store_ext::NoticeRecord;

/// One subscription reconcile at a time: the tick's and the event loop's (review round 2 of
/// #1711).
static ACCOUNT_SYNC: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Where a subscription notice leads.
const ACCOUNTS_LINK: &str = "/admin/accounts";

/// The key of a subscription's notice.
#[must_use]
pub fn account_key(id: &str) -> NoticeKey {
    NoticeKey::lossy("account:", id)
}

/// Staging ticks in a row a project's workers may all be unable to take the repair before its
/// people are told of the failure anyway (review round 2 of #1711): ten minutes at a 60 s tick.
pub const GATE_GRACE_TICKS: u32 = 10;

/// Failures a project's people get one notice each for; the rest share one (review of #1711).
pub const GATE_NOTICES_MAX: usize = 20;

/// The key of the notice that counts the failures past [`GATE_NOTICES_MAX`].
fn gates_more_key() -> NoticeKey {
    NoticeKey::lossy("gates:", "more")
}

/// The key of an integration failure's notice.
#[must_use]
pub fn gate_key(test: &str) -> NoticeKey {
    NoticeKey::lossy("gate:", test)
}

/// When a usage limit ends: the subscription's own limit, else the latest reset of a window
/// used up; `None` when Claude did not say.
#[must_use]
pub fn limit_reset(usage: &AccountUsageView) -> Option<Timestamp> {
    let windows = [
        &usage.primary,
        &usage.secondary,
        &usage.weekly_opus,
        &usage.weekly_sonnet,
        &usage.weekly_fable,
    ];
    let used_up = windows.into_iter().flatten().filter_map(|w| {
        w.used_percent
            .filter(|p| *p >= 100.0)
            .and(w.resets_at.as_deref())
            .and_then(parse_rfc3339)
    });
    let own = usage
        .limit_reached
        .then_some(usage.limit_resets_at.as_deref())
        .flatten()
        .and_then(parse_rfc3339);
    // Its own limit without an end has none known, whatever a window says (review round 3),
    // and so has a used-up window without a reset time (round 5).
    let open = windows
        .into_iter()
        .flatten()
        .any(|w| w.used_percent.is_some_and(|p| p >= 100.0) && w.resets_at.is_none());
    if (usage.limit_reached && own.is_none()) || open {
        return None;
    }
    own.into_iter().chain(used_up).max()
}

/// What a subscription's state calls for (the spec's table): its notice, or none when it is
/// usable. One that must sign in again waits for a person: a decision, never expiring. A limit
/// with a known reset ends there, and once that reset has passed it calls for none, though the
/// status still reads limited until the next usage reading. A problem without an end (an
/// error, a limit whose reset Claude did not give) holds until the subscription is usable: one
/// notice, not one a day (review round 2 of #1711).
#[must_use]
pub fn account_notice(account: &AccountView, now: Timestamp) -> Option<NewNotice> {
    let problem = account_problem(&account.status)?;
    let decision = account.status == "relogin";
    let expiry = match account.status.as_str() {
        "relogin" => None,
        "limited" => match limit_reset(&account.usage) {
            Some(at) if at <= now => return None,
            Some(at) if at.millis_since(now) <= EXPIRY_MAX_MS => Some(Expiry::At(at)),
            _ => Some(Expiry::Never),
        },
        _ => Some(Expiry::Never),
    };
    Some(NewNotice {
        admin_only: true,
        detail: account.last_error.clone(),
        link: Some(ACCOUNTS_LINK.into()),
        decision,
        expiry,
        ..NewNotice::new(
            None,
            account_key(&account.id),
            NoticeKind::Account,
            if account.status == "error" {
                NoticeTone::Error
            } else {
                NoticeTone::Warn
            },
            format!("{} {problem}", account.label),
        )
    })
}

/// Whether `stored` already says what `wanted` would, whose texts are `texts` as posting stores
/// them: the same texts and kind, and for a limit the same reset. A repeat of it is no news and
/// is not posted again.
fn says_the_same(
    stored: &NoticeRecord,
    wanted: &NewNotice,
    texts: &(String, Option<String>),
    now: Timestamp,
) -> bool {
    let same_end = match wanted.expiry {
        Some(Expiry::At(at)) => stored.expires_at == Some(at),
        None | Some(Expiry::Never) => stored.expires_at.is_none(),
        Some(Expiry::After(_)) => stored.expires_at.is_some_and(|at| at > now),
    };
    stored.tone == wanted.tone
        && stored.decision == wanted.decision
        && stored.link == wanted.link
        && stored.title == texts.0
        && stored.detail == texts.1
        && same_end
}

impl App {
    /// Starts Yulius's own notice producers once (SPEC-624 NTC-010): every subscription is
    /// reconciled now, then each `account.updated` as it comes; a lag reconciles all again.
    pub fn start_notice_producers(self: &Arc<Self>) {
        if self.notice_producers.set(()).is_err() {
            return;
        }
        let mut rx = self.events.subscribe();
        let app = Arc::clone(self);
        tokio::spawn(async move {
            app.sync_all_account_notices().await;
            loop {
                match rx.recv().await {
                    Ok(envelope) => {
                        if let UiEvent::AccountUpdated { account } = &envelope.event {
                            app.sync_account_event(account).await;
                        }
                    }
                    Err(RecvError::Lagged(_)) => app.sync_all_account_notices().await,
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    /// Reconciles every subscription's notice with its stored state, and removes the notices of
    /// subscriptions that no longer exist. The background tick runs it too: a status written
    /// without an `account.updated` (a usage reading, a refused sign-in, a removal) is caught
    /// within a tick (review of #1711).
    pub async fn sync_all_account_notices(&self) {
        let _one = ACCOUNT_SYNC.lock().await;
        let store = &self.deps.store;
        let (Ok(accounts), Ok(live)) = (store.accounts().await, self.working_attempts().await)
        else {
            tracing::warn!("subscription notices could not be reconciled");
            return;
        };
        let known: std::collections::HashSet<NoticeKey> = accounts
            .iter()
            .map(|a| account_key(a.id.as_str()))
            .collect();
        for account in accounts {
            self.sync_account_notice(&crate::accounts::account_view(&account, &live))
                .await;
        }
        match store.notices_by_prefix(None, "account:", self.now()).await {
            Ok(stored) => {
                for gone in stored
                    .iter()
                    .filter(|n| n.user.is_none() && !known.contains(&n.key))
                {
                    if let Err(err) = self.remove_notice(None, None, &gone.key).await {
                        tracing::warn!(error = %err, "a removed subscription's notice stays");
                    }
                }
            }
            Err(err) => tracing::warn!(error = %err, "subscription notices could not be read"),
        }
    }

    /// An `account.updated` event: reconciled from the stored subscription, read under the
    /// same lock as the tick's, so the two never post a state the other has moved past (review
    /// round 2 of #1711); one the store does not hold is taken as the event says.
    async fn sync_account_event(&self, event: &AccountView) {
        let _one = ACCOUNT_SYNC.lock().await;
        let stored = match yulius_domain::ids::AccountId::parse(&event.id) {
            Ok(id) => self.deps.store.account(&id).await.ok().flatten(),
            Err(_) => None,
        };
        match (stored, self.working_attempts().await) {
            (Some(account), Ok(live)) => {
                self.sync_account_notice(&crate::accounts::account_view(&account, &live))
                    .await;
            }
            _ => self.sync_account_notice(event).await,
        }
    }

    /// Posts, replaces or removes `account`'s notice as its state calls for (NTC-010).
    pub async fn sync_account_notice(&self, account: &AccountView) {
        let key = account_key(&account.id);
        let outcome = async {
            match account_notice(account, self.now()) {
                None => self.remove_notice(None, None, &key).await.map(|_| ()),
                Some(wanted) => self.post_unless_same(None, wanted).await,
            }
        }
        .await;
        if let Err(err) = outcome {
            tracing::warn!(account = %account.id, error = %err, "a subscription notice failed");
        }
    }

    /// Posts `wanted` unless its stored notice already says the same.
    async fn post_unless_same(
        &self,
        sandbox: Option<&yulius_domain::ids::SandboxId>,
        wanted: NewNotice,
    ) -> AppResult<()> {
        let now = self.now();
        let stored = self
            .deps
            .store
            .notices_by_prefix(sandbox, wanted.key.as_str(), now)
            .await?;
        // Compared as posting would store it: redacted, on one line and cut (review of #1711).
        let texts = self
            .stored_texts(sandbox, &wanted.title, wanted.detail.as_deref())
            .await;
        if stored
            .iter()
            .filter(|n| n.key == wanted.key && n.user.is_none())
            .any(|n| says_the_same(n, &wanted, &texts, now))
        {
            return Ok(());
        }
        self.post_notice(wanted).await.map(|_| ())
    }

    /// Reconciles `sandbox`'s integration failure notices with `failures`, the ones its people
    /// see (no repair worker): each is posted once under `gate:<test>`, and one no longer
    /// failing, or now owned by a worker, is removed (NTC-010).
    ///
    /// # Errors
    /// Store failures; the next staging tick tries again.
    pub async fn sync_gate_notices(
        &self,
        sandbox: &SandboxRecord,
        failures: &[GateNotice],
        hold_new: bool,
    ) -> AppResult<()> {
        let mut wanted = Vec::new();
        for failure in failures.iter().take(GATE_NOTICES_MAX) {
            let origin = failure.merge_task.map_or_else(
                || "an untracked merge".into(),
                |n| format!("task #{n}'s merge"),
            );
            let revision: String = failure.revision.chars().take(12).collect();
            wanted.push(NewNotice {
                detail: Some(format!(
                    "It first failed at {revision}, after {origin}. No task works on the repair; \
                     the CI page has the run."
                )),
                link: Some(format!("/s/{}/ci", sandbox.slug)),
                expiry: Some(Expiry::Never),
                ..NewNotice::new(
                    Some(sandbox.id.clone()),
                    gate_key(&failure.test),
                    NoticeKind::Failed,
                    NoticeTone::Error,
                    format!("Integration gate: {} fails", failure.test),
                )
            });
        }
        if failures.len() > GATE_NOTICES_MAX {
            let more = failures.len() - GATE_NOTICES_MAX;
            wanted.push(NewNotice {
                detail: Some("No task works on the repair. The CI page lists them all.".into()),
                link: Some(format!("/s/{}/ci", sandbox.slug)),
                expiry: Some(Expiry::Never),
                ..NewNotice::new(
                    Some(sandbox.id.clone()),
                    // Its own prefix: no test's key can equal it (review round 2 of #1711).
                    gates_more_key(),
                    NoticeKind::Failed,
                    NoticeTone::Error,
                    format!("Integration gate: {more} more failing tests"),
                )
            });
        }
        let stored = self
            .deps
            .store
            .notices_by_prefix(Some(&sandbox.id), "gate", self.now())
            .await?;
        let ours = |key: &NoticeKey| key.as_str().starts_with("gate:") || *key == gates_more_key();
        // Within the grace period what was told stays and nothing new is (review round 3).
        if hold_new {
            wanted.retain(|w| stored.iter().any(|n| n.user.is_none() && n.key == w.key));
        }
        for old in stored.iter().filter(|n| n.user.is_none() && ours(&n.key)) {
            if !wanted.iter().any(|w| w.key == old.key) {
                self.remove_notice(Some(&sandbox.id), None, &old.key)
                    .await?;
            }
        }
        for notice in wanted {
            self.post_unless_same(Some(&sandbox.id), notice).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_contracts::api::admin::UsageWindowView;

    fn view(status: &str, usage: AccountUsageView) -> AccountView {
        AccountView {
            id: "acc1".into(),
            provider: "anthropic".into(),
            label: "Team Max".into(),
            status: status.into(),
            usage,
            last_error: None,
            email: None,
            email_typed: false,
            in_use_by: 0,
            tokens_24h: None,
            tokens_7d: None,
            usage_read: None,
        }
    }

    fn window(percent: f64, resets_at: &str) -> Option<UsageWindowView> {
        Some(UsageWindowView {
            used_percent: Some(percent),
            resets_at: Some(resets_at.into()),
            window_minutes: None,
        })
    }

    /// The spec's table: a limit ends at its reset, a sign-in waits for a person, an error lasts
    /// until the subscription is usable, a usable subscription has none.
    #[test]
    fn a_subscription_notice_follows_the_table() {
        let now = parse_rfc3339("2026-11-22T10:00:00Z").unwrap_or_default();
        let limited = view(
            "limited",
            AccountUsageView {
                primary: window(100.0, "2026-11-22T13:00:00Z"),
                secondary: window(40.0, "2026-11-25T00:00:00Z"),
                ..AccountUsageView::default()
            },
        );
        let notice = account_notice(&limited, now);
        assert_eq!(
            notice.as_ref().and_then(|n| n.expiry),
            parse_rfc3339("2026-11-22T13:00:00Z").map(Expiry::At)
        );
        assert_eq!(
            notice.map(|n| (n.key.to_string(), n.title, n.admin_only)),
            Some((
                "account:acc1".into(),
                "Team Max reached its usage limit".into(),
                true
            ))
        );
        let unknown = account_notice(&view("limited", AccountUsageView::default()), now);
        assert_eq!(unknown.and_then(|n| n.expiry), Some(Expiry::Never));
        let passed = view(
            "limited",
            AccountUsageView {
                primary: window(100.0, "2026-11-22T09:00:00Z"),
                ..AccountUsageView::default()
            },
        );
        assert!(
            account_notice(&passed, now).is_none(),
            "its reset has passed"
        );
        let error = account_notice(&view("error", AccountUsageView::default()), now);
        assert_eq!(error.and_then(|n| n.expiry), Some(Expiry::Never));
        let relogin = account_notice(&view("relogin", AccountUsageView::default()), now);
        assert_eq!(relogin.map(|n| (n.decision, n.expiry)), Some((true, None)));
        assert!(account_notice(&view("ok", AccountUsageView::default()), now).is_none());
        let own = AccountUsageView {
            limit_reached: true,
            limit_resets_at: Some("2026-11-23T00:00:00Z".into()),
            primary: window(100.0, "2026-11-22T13:00:00Z"),
            ..AccountUsageView::default()
        };
        assert_eq!(limit_reset(&own), parse_rfc3339("2026-11-23T00:00:00Z"));
        // The subscription's own limit without an end: unknown, whatever a window says (review
        // round 3 of #1711).
        let endless = AccountUsageView {
            limit_reached: true,
            limit_resets_at: None,
            primary: window(100.0, "2026-11-22T11:00:00Z"),
            ..AccountUsageView::default()
        };
        assert_eq!(limit_reset(&endless), None);
        // A window used up without a reset time binds with no known end too (review round 5).
        let open_window = AccountUsageView {
            primary: Some(UsageWindowView {
                used_percent: Some(100.0),
                resets_at: None,
                window_minutes: None,
            }),
            secondary: window(100.0, "2026-11-22T13:00:00Z"),
            ..AccountUsageView::default()
        };
        assert_eq!(limit_reset(&open_window), None);
        let later = parse_rfc3339("2026-11-22T12:00:00Z").unwrap_or_default();
        assert_eq!(
            account_notice(&view("limited", endless), later).and_then(|n| n.expiry),
            Some(Expiry::Never)
        );
    }
}
