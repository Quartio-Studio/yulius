//! [`UsageProbe`] over HTTPS (SPEC-001 ROT-009): the endpoint Claude Code reads its plan usage
//! from — `GET https://api.anthropic.com/api/oauth/usage` (OAuth token,
//! `anthropic-beta: oauth-2025-04-20`; windows `five_hour` and `seven_day` with `utilization`
//! in percent and `resets_at`). Read-only: tokens are never refreshed here. Errors carry the
//! status, never the credential or the response body.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use yulius_app::model::Secret;
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_domain::harness::Provider;
use yulius_domain::usage::{AccountUsage, UsageWindow};

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Largest usage response accepted.
const MAX_BODY: usize = 256 * 1024;

/// Usage of Claude accounts from the provider.
pub struct ProviderUsage {
    http: reqwest::Client,
}

impl ProviderUsage {
    /// A probe with its own HTTPS client.
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn new() -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("usage client setup failed: {e}"))?;
        Ok(Self { http })
    }

    async fn get_json(&self, request: reqwest::RequestBuilder) -> Result<Value, UsageFailure> {
        let response = request.send().await.map_err(|e| {
            UsageFailure::new(format!(
                "the provider is not reachable ({})",
                e.without_url()
            ))
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(UsageFailure::new(format!(
                "the provider refused this account's token for usage (HTTP {}); it refreshes on the next run or needs a re-login",
                status.as_u16()
            )));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(throttled(
                response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok()),
            ));
        }
        if !status.is_success() {
            return Err(UsageFailure::new(format!(
                "the provider answered HTTP {}",
                status.as_u16()
            )));
        }
        let bytes = response.bytes().await.map_err(|e| {
            UsageFailure::new(format!(
                "reading the usage answer failed ({})",
                e.without_url()
            ))
        })?;
        if bytes.len() > MAX_BODY {
            return Err(UsageFailure::new("the usage answer is too large"));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| UsageFailure::new("the usage answer is not JSON"))
    }
}

/// HTTP 429 on the usage request: the provider throttles usage requests for this token (the
/// account itself may be fine). `Retry-After` in seconds, when given, says when to ask again.
fn throttled(retry_after: Option<&str>) -> UsageFailure {
    let retry_after = retry_after
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let when = retry_after.map_or_else(
        || "later".to_owned(),
        |d| format!("in about {} min", d.as_secs().div_ceil(60).max(1)),
    );
    UsageFailure {
        message: format!(
            "the provider is limiting usage requests for this account (HTTP 429); Yulius asks again {when}"
        ),
        retry_after,
    }
}

/// Claude's `{"five_hour": {"utilization", "resets_at"}, "seven_day": {...}}`.
pub fn parse_claude(body: &Value) -> Result<AccountUsage, String> {
    let window = |key: &str, minutes: u32| -> Option<UsageWindow> {
        let w = body.get(key).filter(|w| w.is_object())?;
        Some(UsageWindow {
            used_percent: w.get("utilization").and_then(Value::as_f64),
            window_minutes: Some(minutes),
            resets_at: w
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(yulius_harness::time::parse_rfc3339),
        })
    };
    let usage = AccountUsage {
        primary: window("five_hour", 5 * 60),
        secondary: window("seven_day", 7 * 24 * 60),
        ..AccountUsage::default()
    };
    if usage.primary.is_none() && usage.secondary.is_none() {
        return Err("the provider reported no usage windows".into());
    }
    Ok(usage)
}

#[async_trait]
impl UsageProbe for ProviderUsage {
    async fn fetch(
        &self,
        provider: Provider,
        credential: &Secret,
    ) -> Result<AccountUsage, UsageFailure> {
        match provider {
            Provider::Anthropic => {
                let request = self
                    .http
                    .get(CLAUDE_USAGE_URL)
                    .bearer_auth(credential.expose().trim())
                    .header("anthropic-beta", "oauth-2025-04-20")
                    .header("accept", "application/json");
                parse_claude(&self.get_json(request).await?).map_err(UsageFailure::new)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn claude_windows_parse_in_percent() -> Result<(), String> {
        let usage = parse_claude(&json!({
            "five_hour": {"utilization": 37.0, "resets_at": "2026-10-07T20:00:00Z"},
            "seven_day": {"utilization": 12.5, "resets_at": "2026-10-10T08:00:00.000+00:00"},
            "seven_day_opus": null
        }))?;
        let primary = usage.primary.ok_or("primary")?;
        assert_eq!(primary.used_percent, Some(37.0));
        assert_eq!(primary.window_minutes, Some(300));
        assert!(primary.resets_at.is_some());
        assert_eq!(usage.secondary.ok_or("secondary")?.used_percent, Some(12.5));
        assert!(parse_claude(&json!({})).is_err());
        Ok(())
    }

    #[test]
    fn throttling_reads_retry_after_seconds() {
        let with = throttled(Some("120"));
        assert_eq!(with.retry_after, Some(Duration::from_secs(120)));
        assert!(with.message.contains("HTTP 429"));
        assert!(with.message.ends_with("in about 2 min"));
        let date = throttled(Some("Wed, 21 Oct 2026 07:28:00 GMT"));
        assert_eq!(date.retry_after, None, "only seconds are honoured");
        assert!(date.message.ends_with("later"));
        assert_eq!(throttled(None).retry_after, None);
    }
}
