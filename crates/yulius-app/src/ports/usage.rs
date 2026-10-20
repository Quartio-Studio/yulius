//! Provider usage probe (SPEC-001 ROT-009): the account's current usage windows as the provider
//! reports them (Claude: `/api/oauth/usage`).
//!
//! The usage endpoint needs the OAuth scope `user:profile`. A `claude setup-token` sign-in — the
//! one Yulius stores — has only `user:inference`, so the endpoint refuses it with HTTP 403
//! ([`UsageFailureKind::NotShared`]); its usage comes from the running sessions instead (Claude
//! Code's status line, SPEC-002 ADR-17).
//!
//! The same adapter reads the email of the Claude account behind a credential
//! (`/api/oauth/profile`, SPEC-244 SUB-003), which may be refused for the same reason.

use std::time::Duration;

use async_trait::async_trait;
use yulius_domain::harness::Provider;
use yulius_domain::usage::AccountUsage;

use crate::model::Secret;

/// What kind of failure a usage request had: whether it is temporary, needs a person, or simply
/// means the sign-in cannot read usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageFailureKind {
    /// The sign-in may not read usage (Claude: HTTP 403 for a `setup-token` without the
    /// `user:profile` scope). Not an error: the running sessions report the usage.
    NotShared,
    /// The provider did not accept the sign-in (HTTP 401): it may need to be signed in again.
    Rejected,
    /// The provider throttles usage requests for now (HTTP 429).
    Throttled,
    /// Temporarily unavailable: no answer, a server error or an unreadable answer.
    Unavailable,
}

/// Why the provider did not report an account's usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFailure {
    /// One plain sentence for people, without HTTP codes or secrets.
    pub message: String,
    /// How long the provider asked to wait before asking again (HTTP 429 `Retry-After`).
    pub retry_after: Option<Duration>,
    /// What kind of failure it is.
    pub kind: UsageFailureKind,
}

impl UsageFailure {
    /// A temporary failure without a retry hint.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self::of(UsageFailureKind::Unavailable, message)
    }

    /// A failure of `kind`.
    #[must_use]
    pub fn of(kind: UsageFailureKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
            kind,
        }
    }

    /// The sign-in may not read usage (see [`UsageFailureKind::NotShared`]).
    #[must_use]
    pub fn not_shared() -> Self {
        Self::of(
            UsageFailureKind::NotShared,
            "Claude shares this subscription's usage with its running sessions only.",
        )
    }

    /// The provider throttles usage requests; `retry_after` when it said how long to wait.
    #[must_use]
    pub fn throttled(retry_after: Option<Duration>) -> Self {
        let when = retry_after.map_or_else(
            || "in a few minutes".to_owned(),
            |d| format!("in about {} min", d.as_secs().div_ceil(60).max(1)),
        );
        Self {
            message: format!("Usage updates again {when}."),
            retry_after,
            kind: UsageFailureKind::Throttled,
        }
    }
}

/// Asks providers for an account's usage.
#[async_trait]
pub trait UsageProbe: Send + Sync {
    /// Usage windows of the account behind `credential` (the stored Claude OAuth token).
    async fn fetch(
        &self,
        provider: Provider,
        credential: &Secret,
    ) -> Result<AccountUsage, UsageFailure>;

    /// The email of the account behind `credential` (Claude: `account.email` of
    /// `/api/oauth/profile`). `Ok(None)`: the provider answered without a usable address. A
    /// failure is classified as for usage: [`UsageFailureKind::NotShared`] (the sign-in may not
    /// read the profile) and [`UsageFailureKind::Rejected`] are definite answers,
    /// [`UsageFailureKind::Throttled`] and [`UsageFailureKind::Unavailable`] temporary.
    async fn account_email(
        &self,
        provider: Provider,
        credential: &Secret,
    ) -> Result<Option<String>, UsageFailure> {
        let _ = (provider, credential);
        Err(UsageFailure::new(
            "the account email is not available in this build",
        ))
    }
}

/// Longest email address stored (RFC 5321 path limit).
pub const EMAIL_MAX: usize = 254;

/// `raw` trimmed when it looks like an email address: one `@` with text on both sides, no
/// whitespace or control characters, at most [`EMAIL_MAX`] characters (SPEC-244 SUB-006).
#[must_use]
pub fn plausible_email(raw: &str) -> Option<String> {
    let email = raw.trim();
    let (local, domain) = email.split_once('@')?;
    let printable = email.chars().all(|c| !c.is_whitespace() && !c.is_control());
    (printable
        && !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && email.chars().count() <= EMAIL_MAX)
        .then(|| email.to_owned())
}

/// A probe that never reaches a provider (tests and offline builds).
pub struct NoUsageProbe;

#[async_trait]
impl UsageProbe for NoUsageProbe {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
        Err(UsageFailure::new(
            "provider usage is not available in this build",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_are_emails() {
        assert_eq!(
            plausible_email("  pawel@example.com ").as_deref(),
            Some("pawel@example.com")
        );
        for bad in [
            "",
            "pawel",
            "@example.com",
            "pawel@",
            "a@b@c",
            "pa wel@example.com",
            "pawel@exa\nmple.com",
        ] {
            assert_eq!(plausible_email(bad), None, "{bad:?}");
        }
        let long = format!("{}@example.com", "a".repeat(EMAIL_MAX));
        assert_eq!(plausible_email(&long), None);
    }
}
