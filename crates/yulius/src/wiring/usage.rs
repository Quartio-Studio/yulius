//! [`UsageProbe`] over HTTPS (SPEC-001 ROT-009): the endpoint Claude Code reads its plan usage
//! from — `GET https://api.anthropic.com/api/oauth/usage` (OAuth token,
//! `anthropic-beta: oauth-2025-04-20`; windows `five_hour` and `seven_day` with `utilization`
//! in percent and `resets_at`, plus the per-model weekly caps `seven_day_opus` and
//! `seven_day_sonnet` when the plan has them). Read-only: tokens are never refreshed here.
//!
//! The endpoint needs the OAuth scope `user:profile`; a `claude setup-token` sign-in has only
//! `user:inference` (Claude Code 2.1.292 does not even call it without that scope), so it answers
//! HTTP 403 for every subscription Yulius signs in. That is not an error and no refresh or
//! re-login changes it: the running sessions report the usage (Claude Code's status line,
//! SPEC-002 ADR-17). Failures carry one plain sentence for people, never the credential, the
//! response body or an HTTP code.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use yulius_app::model::Secret;
use yulius_app::ports::usage::{UsageFailure, UsageFailureKind, UsageProbe};
use yulius_domain::harness::Provider;
use yulius_domain::usage::{AccountUsage, UsageWindow};

const CLAUDE_API: &str = "https://api.anthropic.com";
const USAGE_PATH: &str = "/api/oauth/usage";
const MESSAGES_PATH: &str = "/v1/messages";
/// The cheapest model, for the one-token request that reads the rate-limit headers.
const PROBE_MODEL: &str = "claude-haiku-4-5-20251001";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Largest usage response accepted.
const MAX_BODY: usize = 256 * 1024;
/// What a temporary failure says.
const TRY_AGAIN: &str = "Usage could not be read just now; it updates again in a few minutes.";
/// What a refused sign-in says.
const REJECTED: &str =
    "Claude did not accept this subscription's sign-in. If its tasks fail too, sign in again.";

/// Usage of Claude accounts from the provider.
pub struct ProviderUsage {
    http: reqwest::Client,
    base: String,
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
        Ok(Self {
            http,
            base: CLAUDE_API.to_owned(),
        })
    }

    async fn get_json(&self, request: reqwest::RequestBuilder) -> Result<Value, UsageFailure> {
        let response = request.send().await.map_err(|e| {
            tracing::debug!(error = %e.without_url(), "usage request failed");
            UsageFailure::new(TRY_AGAIN)
        })?;
        let status = response.status();
        if let Some(failure) = failure_of(
            status.as_u16(),
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        ) {
            return Err(failure);
        }
        let bytes = response.bytes().await.map_err(|e| {
            tracing::debug!(error = %e.without_url(), "reading the usage answer failed");
            UsageFailure::new(TRY_AGAIN)
        })?;
        if bytes.len() > MAX_BODY {
            return Err(UsageFailure::new(TRY_AGAIN));
        }
        serde_json::from_slice(&bytes).map_err(|_| UsageFailure::new(TRY_AGAIN))
    }
}

/// The failure an HTTP status means (`None` for success): 403 is a sign-in without the
/// `user:profile` scope ([`UsageFailureKind::NotShared`]), 401 a refused sign-in, 429 throttling
/// (`Retry-After` in seconds honoured), anything else unsuccessful is temporary.
fn failure_of(status: u16, retry_after: Option<&str>) -> Option<UsageFailure> {
    match status {
        200..=299 => None,
        401 => Some(UsageFailure::of(UsageFailureKind::Rejected, REJECTED)),
        403 => Some(UsageFailure::not_shared()),
        429 => Some(throttled(retry_after)),
        _ => Some(UsageFailure::new(TRY_AGAIN)),
    }
}

/// HTTP 429 on the usage request: the provider throttles usage requests for this token (the
/// account itself may be fine). `Retry-After` in seconds, when given, says when to ask again.
fn throttled(retry_after: Option<&str>) -> UsageFailure {
    UsageFailure::throttled(
        retry_after
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs),
    )
}

/// Claude's `{"five_hour": {"utilization", "resets_at"}, "seven_day": {...},
/// "seven_day_opus": {...}|null, "seven_day_sonnet": {...}|null}`.
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
        weekly_opus: window("seven_day_opus", 7 * 24 * 60),
        weekly_sonnet: window("seven_day_sonnet", 7 * 24 * 60),
        ..AccountUsage::default()
    };
    if usage.primary.is_none() && usage.secondary.is_none() {
        return Err("Claude reported no usage for this subscription yet.".into());
    }
    Ok(usage)
}

/// Claude's `anthropic-ratelimit-unified-{5h,7d}-{utilization,reset}` headers: utilization is a
/// fraction (0.37 = 37 %), reset is Unix seconds. The same numbers Claude Code's status line shows.
pub fn parse_rate_limit_headers(headers: &reqwest::header::HeaderMap) -> Option<AccountUsage> {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let window = |key: &str, minutes: u32| -> Option<UsageWindow> {
        let used = get(&format!("anthropic-ratelimit-unified-{key}-utilization"))?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|u| u.is_finite() && *u >= 0.0)?;
        let resets_at = get(&format!("anthropic-ratelimit-unified-{key}-reset"))
            .and_then(|v| v.trim().parse::<i64>().ok())
            .and_then(|s| s.checked_mul(1000))
            .map(yulius_domain::time::Timestamp);
        Some(UsageWindow {
            used_percent: Some(used * 100.0),
            window_minutes: Some(minutes),
            resets_at,
        })
    };
    let usage = AccountUsage {
        primary: window("5h", 5 * 60),
        secondary: window("7d", 7 * 24 * 60),
        ..AccountUsage::default()
    };
    (usage.primary.is_some() || usage.secondary.is_some()).then_some(usage)
}

impl ProviderUsage {
    /// Reads usage the way Claude Code does: from the rate-limit headers of an API response, for
    /// sign-ins the usage endpoint refuses (`setup-token`, no `user:profile`). One request of one
    /// output token on the cheapest model.
    async fn read_headers(&self, credential: &Secret) -> Result<AccountUsage, UsageFailure> {
        let response = self
            .http
            .post(format!("{}{MESSAGES_PATH}", self.base))
            .bearer_auth(credential.expose().trim())
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({
                "model": PROBE_MODEL,
                "max_tokens": 1,
                "system": "You are Claude Code, Anthropic's official CLI for Claude.",
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await
            .map_err(|e| {
                tracing::debug!(error = %e.without_url(), "usage header request failed");
                UsageFailure::new(TRY_AGAIN)
            })?;
        let status = response.status().as_u16();
        // A limited subscription answers 429 and still carries the headers.
        if let Some(usage) = parse_rate_limit_headers(response.headers()) {
            return Ok(usage);
        }
        match failure_of(status, None) {
            Some(failure) if failure.kind != UsageFailureKind::NotShared => Err(failure),
            _ => Err(UsageFailure::not_shared()),
        }
    }
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
                    .get(format!("{}{USAGE_PATH}", self.base))
                    .bearer_auth(credential.expose().trim())
                    .header("anthropic-beta", "oauth-2025-04-20")
                    .header("accept", "application/json");
                match self.get_json(request).await {
                    Ok(body) => parse_claude(&body).map_err(UsageFailure::new),
                    // A `setup-token` may not read the endpoint; its headers say the same.
                    Err(f) if f.kind == UsageFailureKind::NotShared => {
                        self.read_headers(credential).await
                    }
                    Err(f) => Err(f),
                }
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
            "seven_day_opus": null,
            "seven_day_sonnet": {"utilization": 81.0, "resets_at": "2026-10-10T08:00:00Z"}
        }))?;
        let primary = usage.primary.ok_or("primary")?;
        assert_eq!(primary.used_percent, Some(37.0));
        assert_eq!(primary.window_minutes, Some(300));
        assert!(primary.resets_at.is_some());
        assert_eq!(usage.secondary.ok_or("secondary")?.used_percent, Some(12.5));
        assert_eq!(usage.weekly_opus, None, "a null cap is unknown, not zero");
        assert_eq!(
            usage.weekly_sonnet.ok_or("sonnet cap")?.used_percent,
            Some(81.0)
        );
        assert!(parse_claude(&json!({})).is_err());
        Ok(())
    }

    #[test]
    fn rate_limit_headers_become_percent_windows() -> Result<(), String> {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("anthropic-ratelimit-unified-5h-utilization", "0.37"),
            ("anthropic-ratelimit-unified-5h-reset", "1790000000"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.125"),
        ] {
            h.insert(k, HeaderValue::from_static(v));
        }
        let usage = parse_rate_limit_headers(&h).ok_or("usage")?;
        let primary = usage.primary.ok_or("primary")?;
        assert!((primary.used_percent.ok_or("pct")? - 37.0).abs() < 1e-9);
        assert_eq!(primary.resets_at.map(|t| t.0), Some(1_790_000_000_000));
        assert_eq!(usage.secondary.ok_or("weekly")?.resets_at, None);
        assert!(parse_rate_limit_headers(&HeaderMap::new()).is_none());
        Ok(())
    }

    /// A `setup-token` sign-in: the usage endpoint says 403, the messages answer carries the
    /// headers, and that is the reading.
    #[tokio::test]
    async fn a_refused_sign_in_is_read_from_the_headers() -> Result<(), Box<dyn std::error::Error>>
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let reply = if head.starts_with("GET /api/oauth/usage") {
                    "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                } else if head.starts_with("POST /v1/messages") {
                    "HTTP/1.1 200 OK\r\nanthropic-ratelimit-unified-5h-utilization: 0.25\r\nanthropic-ratelimit-unified-5h-reset: 1790000000\r\nanthropic-ratelimit-unified-7d-utilization: 0.5\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}"
                } else {
                    "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                };
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let mut probe = ProviderUsage::new()?;
        probe.base = base;
        let usage = probe
            .fetch(Provider::Anthropic, &Secret("token".into()))
            .await
            .map_err(|f| f.message)?;
        assert_eq!(usage.primary.and_then(|w| w.used_percent), Some(25.0));
        assert_eq!(usage.secondary.and_then(|w| w.used_percent), Some(50.0));
        Ok(())
    }

    #[test]
    fn throttling_reads_retry_after_seconds() {
        let with = throttled(Some("120"));
        assert_eq!(with.retry_after, Some(Duration::from_secs(120)));
        assert_eq!(with.kind, UsageFailureKind::Throttled);
        assert_eq!(with.message, "Usage updates again in about 2 min.");
        let date = throttled(Some("Wed, 21 Oct 2026 07:28:00 GMT"));
        assert_eq!(date.retry_after, None, "only seconds are honoured");
        assert_eq!(date.message, "Usage updates again in a few minutes.");
        assert_eq!(throttled(None).retry_after, None);
    }

    /// The 403 every `setup-token` sign-in gets (no `user:profile` scope) is not an error, and
    /// no message names an HTTP code.
    #[test]
    fn statuses_become_kinds_and_plain_sentences() {
        assert!(failure_of(200, None).is_none());
        let kinds: Vec<_> = [401, 403, 429, 500, 502, 404]
            .into_iter()
            .filter_map(|status| failure_of(status, None))
            .map(|f| {
                assert!(!f.message.contains("HTTP"), "{}", f.message);
                assert!(f.message.ends_with('.'), "{}", f.message);
                f.kind
            })
            .collect();
        assert_eq!(
            kinds,
            [
                UsageFailureKind::Rejected,
                UsageFailureKind::NotShared,
                UsageFailureKind::Throttled,
                UsageFailureKind::Unavailable,
                UsageFailureKind::Unavailable,
                UsageFailureKind::Unavailable
            ]
        );
    }
}
