//! Provider usage probe (SPEC-001 ROT-009): the account's current usage windows as the provider
//! reports them (Claude: `/api/oauth/usage`).

use std::time::Duration;

use async_trait::async_trait;
use yulius_domain::harness::Provider;
use yulius_domain::usage::AccountUsage;

use crate::model::Secret;

/// Why the provider did not report an account's usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFailure {
    /// Short, secret-free explanation.
    pub message: String,
    /// How long the provider asked to wait before asking again (HTTP 429 `Retry-After`).
    pub retry_after: Option<Duration>,
}

impl UsageFailure {
    /// A failure without a retry hint.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
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
