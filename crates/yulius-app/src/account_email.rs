//! The email of the Claude account behind each subscription (SPEC-244 SUB-003/SUB-004,
//! ADR-0185): read from Claude's profile with the subscription's own credential right after a
//! sign-in stored it, and once for subscriptions signed in before, by the background usage round.
//!
//! A definite answer (an email, a profile without one, a refusal) is stored with its time, so
//! nobody asks again until the next sign-in clears both (`set_account_credential`). A temporary
//! failure stores nothing and backs that subscription off for the provider's `Retry-After`, or
//! [`USAGE_BACKOFF_MS`] without one. The email is never logged.

use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;

use crate::accounts::{USAGE_BACKOFF_MAX_MS, USAGE_BACKOFF_MS};
use crate::context::App;
use crate::model::{AccountRecord, AccountStatus, Secret};
use crate::ports::usage::{UsageFailureKind, plausible_email};

/// What one email request ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmailRead {
    /// Claude answered definitely; the answer is stored.
    Answered,
    /// No answer for now: ask again after this many milliseconds.
    RetryAfter(i64),
}

impl App {
    /// Asks Claude for the email of `account`'s Claude account with its stored credential and
    /// stores a definite answer, unless the account was signed in again meanwhile (the answer
    /// would be the old sign-in's).
    pub(crate) async fn record_account_email(&self, account: &AccountRecord) -> EmailRead {
        let Some(sealed) = account.credential.as_ref() else {
            return EmailRead::RetryAfter(USAGE_BACKOFF_MS);
        };
        let Some(credential) = self
            .deps
            .crypto
            .open(sealed)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(Secret)
        else {
            return EmailRead::RetryAfter(USAGE_BACKOFF_MS);
        };
        let email = match self
            .deps
            .usage
            .account_email(account.provider, &credential)
            .await
        {
            Ok(email) => email.as_deref().and_then(plausible_email),
            Err(failure)
                if matches!(
                    failure.kind,
                    UsageFailureKind::NotShared | UsageFailureKind::Rejected
                ) =>
            {
                None
            }
            Err(failure) => {
                let wait = failure.retry_after.map_or(USAGE_BACKOFF_MS, |d| {
                    i64::try_from(d.as_millis())
                        .unwrap_or(USAGE_BACKOFF_MAX_MS)
                        .clamp(60_000, USAGE_BACKOFF_MAX_MS)
                });
                return EmailRead::RetryAfter(wait);
            }
        };
        let now = self.now();
        // Written only while the account still holds the credential asked about: a sign-in that
        // overtook this request has its own read (SUB-003).
        match self
            .deps
            .store
            .set_account_email(&account.id, sealed, email.as_deref(), Some(now), now)
            .await
        {
            Ok(true) => {}
            Ok(false) => return EmailRead::Answered,
            Err(_) => return EmailRead::RetryAfter(USAGE_BACKOFF_MS),
        }
        tracing::info!(
            account = %account.id,
            known = email.is_some(),
            "account email read"
        );
        self.publish_account_id(&account.id).await;
        EmailRead::Answered
    }

    /// Reads the email once for every signed-in account that never got an answer for its
    /// current sign-in, the subscriptions connected before emails were read among them
    /// (SUB-004). `not_before` holds each account's back-off after a temporary failure.
    pub(crate) async fn backfill_account_emails(
        &self,
        not_before: &mut std::collections::HashMap<AccountId, Timestamp>,
    ) {
        let Ok(accounts) = self.deps.store.accounts().await else {
            return;
        };
        for account in accounts.iter().filter(|a| {
            a.credential.is_some()
                && a.email_checked_at.is_none()
                && !matches!(
                    a.status,
                    AccountStatus::PendingLogin | AccountStatus::Relogin
                )
        }) {
            let now = self.now();
            if not_before.get(&account.id).is_some_and(|at| *at > now) {
                continue;
            }
            match self.record_account_email(account).await {
                EmailRead::Answered => {
                    not_before.remove(&account.id);
                }
                EmailRead::RetryAfter(wait) => {
                    not_before.insert(account.id.clone(), now.plus_millis(wait));
                }
            }
        }
    }
}
