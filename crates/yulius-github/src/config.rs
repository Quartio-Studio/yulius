//! Adapter configuration, App credentials, retry policy and the injected clock.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Url;
use yulius_domain::time::Timestamp;

use crate::error::GitHubError;
use crate::secret::Secret;

/// Default REST API base.
pub const DEFAULT_API_BASE: &str = "https://api.github.com/";
/// Default web base (OAuth and App manifest pages).
pub const DEFAULT_WEB_BASE: &str = "https://github.com/";

/// GitHub App credentials, as returned by the manifest conversion and stored encrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCredentials {
    /// Numeric App id.
    pub app_id: u64,
    /// OAuth client id (also the recommended JWT issuer).
    pub client_id: String,
    /// OAuth client secret.
    pub client_secret: Secret,
    /// PEM private key (PKCS#1 `RSA PRIVATE KEY` or PKCS#8 `PRIVATE KEY`).
    pub private_key_pem: Secret,
    /// App slug (used for the installation URL).
    pub slug: String,
    /// Webhook secret; `None` because Yulius registers no webhook.
    pub webhook_secret: Option<Secret>,
}

/// Retry bounds for idempotent GET requests (SPEC-002 §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts including the first (default 3).
    pub max_attempts: u32,
    /// Budget for all attempts and waits (default 30 s); a wait that would exceed it is
    /// not taken and the last error is returned.
    pub max_total: Duration,
    /// First backoff delay when the server gives no `Retry-After` (doubles per attempt).
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_total: Duration::from_secs(30),
            base_delay: Duration::from_millis(500),
        }
    }
}

/// Whether a GitHub account is a user or an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerKind {
    /// A personal account.
    User,
    /// An organization.
    Organization,
}

impl OwnerKind {
    /// Wire name used by the HTTP API contract (`user` / `organization`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Organization => "organization",
        }
    }
}

/// Adapter configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubConfig {
    /// REST API base (default `https://api.github.com/`). `http` is accepted only for
    /// loopback hosts (tests).
    pub api_base: Url,
    /// Web base for OAuth and manifest pages (default `https://github.com/`).
    pub web_base: Url,
    /// App credentials, once the manifest flow has completed.
    pub app: Option<AppCredentials>,
    /// Retry bounds for idempotent GETs.
    pub retry: RetryPolicy,
    /// Per-attempt request timeout (default 10 s), also capped by the retry budget.
    pub request_timeout: Duration,
}

impl GitHubConfig {
    /// github.com defaults without App credentials.
    ///
    /// # Errors
    /// Never in practice; the defaults are valid URLs.
    pub fn github_com() -> Result<Self, GitHubError> {
        Ok(Self {
            api_base: parse_base(DEFAULT_API_BASE)?,
            web_base: parse_base(DEFAULT_WEB_BASE)?,
            app: None,
            retry: RetryPolicy::default(),
            request_timeout: Duration::from_secs(10),
        })
    }

    /// Checks both bases are hierarchical `https` URLs (or `http` on loopback).
    pub(crate) fn validate(&self) -> Result<(), GitHubError> {
        for (what, url) in [("api_base", &self.api_base), ("web_base", &self.web_base)] {
            let loopback = matches!(
                url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            );
            let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && loopback);
            if !scheme_ok || url.cannot_be_a_base() || url.query().is_some() {
                return Err(GitHubError::InvalidInput {
                    what,
                    reason: "must be an https URL (http only on loopback) without a query".into(),
                });
            }
        }
        if self.retry.max_attempts == 0 {
            return Err(GitHubError::InvalidInput {
                what: "retry.max_attempts",
                reason: "must be at least 1".into(),
            });
        }
        Ok(())
    }
}

fn parse_base(text: &str) -> Result<Url, GitHubError> {
    Url::parse(text).map_err(|e| GitHubError::InvalidInput {
        what: "base URL",
        reason: e.to_string(),
    })
}

/// Source of the current time, injected for token expiry and JWT claims.
pub trait Clock: Send + Sync {
    /// Now, in milliseconds since the Unix epoch.
    fn now(&self) -> Timestamp;
}

/// The system wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        Timestamp(millis)
    }
}

impl fmt::Debug for dyn Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_plain_http_off_loopback() -> Result<(), GitHubError> {
        let mut config = GitHubConfig::github_com()?;
        config.validate()?;
        config.api_base = parse_base("http://api.example.com/")?;
        assert!(config.validate().is_err());
        config.api_base = parse_base("http://127.0.0.1:9999/api/")?;
        config.validate()?;
        config.web_base = parse_base("mailto:x@example.com")?;
        assert!(config.validate().is_err());
        Ok(())
    }
}
