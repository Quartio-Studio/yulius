//! Provider usage probe (SPEC-001 ROT-009): the account's current usage windows as the provider
//! reports them (Claude: `/api/oauth/usage`; Codex: `/backend-api/wham/usage`).

use async_trait::async_trait;
use yulius_domain::harness::Provider;
use yulius_domain::usage::AccountUsage;

use crate::model::Secret;

/// Asks providers for an account's usage.
#[async_trait]
pub trait UsageProbe: Send + Sync {
    /// Usage windows of the account behind `credential` (the stored credential: a Claude OAuth
    /// token or a Codex `auth.json`). The error is a short, secret-free explanation.
    async fn fetch(&self, provider: Provider, credential: &Secret) -> Result<AccountUsage, String>;
}

/// A probe that never reaches a provider (tests and offline builds).
pub struct NoUsageProbe;

#[async_trait]
impl UsageProbe for NoUsageProbe {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, String> {
        Err("provider usage is not available in this build".into())
    }
}
