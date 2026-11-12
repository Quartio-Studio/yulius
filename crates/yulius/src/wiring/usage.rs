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
//!
//! The account's email comes from `GET https://api.anthropic.com/api/oauth/profile`
//! (`account.email`), the request Claude Code itself makes after a `setup-token` exchange
//! (SPEC-244, ADR-0185). It may answer 403 for the same missing scope; that is a definite
//! "unknown", never retried for the same sign-in.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use yulius_app::model::Secret;
use yulius_app::ports::usage::{UsageFailure, UsageFailureKind, UsageProbe, plausible_email};
use yulius_domain::harness::Provider;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{
    AccountUsage, LimitScope, ModelAnswer, ReportedLimit, SESSION_WINDOW_MINUTES, UsageWindow,
    WEEKLY_WINDOW_MINUTES,
};

const CLAUDE_API: &str = "https://api.anthropic.com";
const USAGE_PATH: &str = "/api/oauth/usage";
const PROFILE_PATH: &str = "/api/oauth/profile";
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

    /// A probe that asks `base` (`http://127.0.0.1:PORT`) instead of Claude's API: a local
    /// stand-in in tests.
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn at(base: impl Into<String>) -> Result<Self, String> {
        Ok(Self {
            base: base.into(),
            ..Self::new()?
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

/// The account's email in Claude's profile answer: `account.email` (also `email_address`, the
/// token exchange's name for it), when it is an address.
pub fn parse_profile_email(body: &Value) -> Option<String> {
    let account = body.get("account")?;
    ["email", "email_address"]
        .into_iter()
        .find_map(|key| account.get(key).and_then(Value::as_str))
        .and_then(plausible_email)
}

/// A header's value as text.
fn header<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// A header that holds a time in Unix seconds.
fn header_time(headers: &reqwest::header::HeaderMap, name: &str) -> Option<Timestamp> {
    header(headers, name)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .and_then(|s| s.checked_mul(1000))
        .map(Timestamp)
}

/// One window of Claude's `anthropic-ratelimit-unified-{key}-{utilization,reset}` headers:
/// utilization is a fraction (0.37 = 37 %), reset is Unix seconds. `key` is `5h`, `7d`, or
/// `7d_oi` for a model's weekly allowance of usage credits, as Claude Code 2.1.322 reads them.
fn header_window(
    headers: &reqwest::header::HeaderMap,
    key: &str,
    minutes: u32,
) -> Option<UsageWindow> {
    let used = header(
        headers,
        &format!("anthropic-ratelimit-unified-{key}-utilization"),
    )?
    .trim()
    .parse::<f64>()
    .ok()
    .filter(|u| u.is_finite() && *u >= 0.0)?;
    Some(UsageWindow {
        used_percent: Some(used.min(1.0) * 100.0),
        window_minutes: Some(minutes),
        resets_at: header_time(headers, &format!("anthropic-ratelimit-unified-{key}-reset")),
    })
}

/// Claude's `anthropic-ratelimit-unified-{5h,7d}-{utilization,reset}` headers as a usage
/// reading. The same numbers Claude Code's status line shows.
pub fn parse_rate_limit_headers(headers: &reqwest::header::HeaderMap) -> Option<AccountUsage> {
    let usage = AccountUsage {
        primary: header_window(headers, "5h", SESSION_WINDOW_MINUTES),
        secondary: header_window(headers, "7d", WEEKLY_WINDOW_MINUTES),
        ..AccountUsage::default()
    };
    (usage.primary.is_some() || usage.secondary.is_some()).then_some(usage)
}

/// Whether the body of a refused response says the model needs usage credits
/// (`error.details.error_code: "credits_required"`, as Claude Code 2.1.322 reads it).
fn credits_required(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body).is_ok_and(|v| {
        v.pointer("/error/details/error_code")
            .and_then(Value::as_str)
            == Some("credits_required")
    })
}

/// What a response to a one-token request of a model says about that model (SPEC-473
/// RFR-001), read as Claude Code 2.1.322 reads its own responses.
///
/// A response that ran says the model runs, whatever its headers say: one that ran on usage
/// credits beyond the plan carries `unified-status: rejected` beside `overage-status: allowed`.
/// HTTP 429 is a refusal for a usage limit when it names one, by its claim
/// (`anthropic-ratelimit-unified-representative-claim`) or by a body that asks for usage
/// credits; the limit's reset is `anthropic-ratelimit-unified-reset`, and for a model's own
/// limit that names none, the reset of its allowance (`7d_oi-reset`). A 429 that names no limit
/// is a throttle. The windows the headers carry are read in either case.
///
/// # Errors
/// What is no answer about the limit: a throttle, a refused sign-in (HTTP 401), any other
/// status.
pub fn parse_model_answer(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> Result<ModelAnswer, UsageFailure> {
    let answer = ModelAnswer {
        refused: None,
        five_hour: header_window(headers, "5h", SESSION_WINDOW_MINUTES),
        seven_day: header_window(headers, "7d", WEEKLY_WINDOW_MINUTES),
        allowance: header_window(headers, "7d_oi", WEEKLY_WINDOW_MINUTES),
    };
    match status {
        200..=299 => Ok(answer),
        429 => {
            let claim = header(headers, "anthropic-ratelimit-unified-representative-claim")
                .map(str::trim)
                .filter(|claim| !claim.is_empty());
            let credits = credits_required(body);
            if claim.is_none() && !credits {
                return Err(throttled(header(headers, "retry-after")));
            }
            let mut limit = ReportedLimit::of_refusal(
                claim,
                credits,
                header_time(headers, "anthropic-ratelimit-unified-reset"),
            );
            if limit.scope == LimitScope::Model(None) {
                limit.resets_at = limit
                    .resets_at
                    .or_else(|| header_time(headers, "anthropic-ratelimit-unified-7d_oi-reset"));
            }
            Ok(ModelAnswer {
                refused: Some(limit),
                ..answer
            })
        }
        401 => Err(UsageFailure::of(UsageFailureKind::Rejected, REJECTED)),
        _ => Err(UsageFailure::new(TRY_AGAIN)),
    }
}

impl ProviderUsage {
    /// One request of one output token with `model`: the least that makes Claude answer with
    /// the rate-limit headers of the subscription and of that model.
    async fn ask(
        &self,
        credential: &Secret,
        model: &str,
    ) -> Result<reqwest::Response, UsageFailure> {
        self.http
            .post(format!("{}{MESSAGES_PATH}", self.base))
            .bearer_auth(credential.expose().trim())
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "system": "You are Claude Code, Anthropic's official CLI for Claude.",
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await
            .map_err(|e| {
                tracing::debug!(error = %e.without_url(), "usage header request failed");
                UsageFailure::new(TRY_AGAIN)
            })
    }

    /// Reads usage the way Claude Code does: from the rate-limit headers of an API response, for
    /// sign-ins the usage endpoint refuses (`setup-token`, no `user:profile`). One request of one
    /// output token on the cheapest model.
    async fn read_headers(&self, credential: &Secret) -> Result<AccountUsage, UsageFailure> {
        let response = self.ask(credential, PROBE_MODEL).await?;
        let status = response.status().as_u16();
        // A limited subscription answers 429 and still carries the headers.
        if (response.status().is_success() || status == 429)
            && let Some(usage) = parse_rate_limit_headers(response.headers())
        {
            return Ok(usage);
        }
        tracing::debug!(
            status,
            "the usage probe message carried no rate-limit headers"
        );
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
                    // A `setup-token` may not read the endpoint (403, or 429 on production);
                    // the headers of an API response say the same. When those fail too, the
                    // endpoint's own answer (and its `Retry-After`) stands.
                    Err(f)
                        if matches!(
                            f.kind,
                            UsageFailureKind::NotShared | UsageFailureKind::Throttled
                        ) =>
                    {
                        self.read_headers(credential).await.map_err(|h| {
                            // Only a refused probe sign-in is news; anything else keeps the
                            // endpoint's own answer (its back-off and `Retry-After`).
                            if h.kind == UsageFailureKind::Rejected {
                                h
                            } else {
                                f
                            }
                        })
                    }
                    Err(f) => Err(f),
                }
            }
        }
    }

    async fn model_answer(
        &self,
        provider: Provider,
        credential: &Secret,
        model: &str,
    ) -> Result<ModelAnswer, UsageFailure> {
        match provider {
            Provider::Anthropic => {
                let response = self.ask(credential, model).await?;
                let status = response.status().as_u16();
                let headers = response.headers().clone();
                // Only a refusal's body is read: it says whether the model needs usage credits.
                let body = if status == 429 {
                    response.bytes().await.unwrap_or_default()
                } else {
                    Default::default()
                };
                let body = if body.len() > MAX_BODY {
                    &[][..]
                } else {
                    &body[..]
                };
                parse_model_answer(status, &headers, body)
            }
        }
    }

    async fn account_email(
        &self,
        provider: Provider,
        credential: &Secret,
    ) -> Result<Option<String>, UsageFailure> {
        match provider {
            Provider::Anthropic => {
                let request = self
                    .http
                    .get(format!("{}{PROFILE_PATH}", self.base))
                    .bearer_auth(credential.expose().trim())
                    .header("anthropic-beta", "oauth-2025-04-20")
                    .header("accept", "application/json");
                Ok(parse_profile_email(&self.get_json(request).await?))
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
        // Never above 100 %, never negative.
        let mut odd = HeaderMap::new();
        odd.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            HeaderValue::from_static("37"),
        );
        odd.insert(
            "anthropic-ratelimit-unified-7d-utilization",
            HeaderValue::from_static("-1"),
        );
        let usage = parse_rate_limit_headers(&odd).ok_or("odd")?;
        assert_eq!(usage.primary.and_then(|w| w.used_percent), Some(100.0));
        assert!(usage.secondary.is_none());
        Ok(())
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, reqwest::header::HeaderValue::from_static(value));
        }
        map
    }

    const CLAIM: &str = "anthropic-ratelimit-unified-representative-claim";
    const RESET: &str = "anthropic-ratelimit-unified-reset";
    const ALLOWANCE_RESET: &str = "anthropic-ratelimit-unified-7d_oi-reset";
    const CREDITS_BODY: &[u8] = br#"{"type":"error","error":{"type":"rate_limit_error","message":"credits","details":{"error_code":"credits_required","disabled_reason":"out_of_credits"}}}"#;

    /// SPEC-473 RFR-001: a model that ran says so by the response's status, with the windows
    /// its headers carry: the 5-hour and weekly ones, and the model's weekly allowance of
    /// usage credits (`7d_oi`, which only that model's responses carry).
    #[test]
    fn a_model_that_ran_is_read_with_its_allowance() -> Result<(), String> {
        use yulius_domain::time::Timestamp;
        let answer = parse_model_answer(
            200,
            &headers(&[
                ("anthropic-ratelimit-unified-status", "allowed"),
                ("anthropic-ratelimit-unified-5h-utilization", "0.25"),
                ("anthropic-ratelimit-unified-5h-reset", "1790000000"),
                ("anthropic-ratelimit-unified-7d-utilization", "0.5"),
                ("anthropic-ratelimit-unified-7d_oi-utilization", "0.42"),
                (ALLOWANCE_RESET, "1790600000"),
            ]),
            b"{}",
        )
        .map_err(|f| f.message)?;
        assert_eq!(answer.refused, None);
        assert_eq!(answer.five_hour.and_then(|w| w.used_percent), Some(25.0));
        assert_eq!(answer.seven_day.and_then(|w| w.used_percent), Some(50.0));
        let allowance = answer.allowance.ok_or("the allowance")?;
        assert!((allowance.used_percent.ok_or("percent")? - 42.0).abs() < 1e-9);
        assert_eq!(allowance.window_minutes, Some(7 * 24 * 60));
        assert_eq!(allowance.resets_at, Some(Timestamp(1_790_600_000_000)));

        // A response that ran on usage credits beyond the plan says `rejected` in its headers:
        // the status of the response is what says the model ran.
        let overage = parse_model_answer(
            201,
            &headers(&[
                ("anthropic-ratelimit-unified-status", "rejected"),
                ("anthropic-ratelimit-unified-overage-status", "allowed"),
                (CLAIM, "seven_day_overage_included"),
                ("anthropic-ratelimit-unified-7d_oi-utilization", "1.0"),
            ]),
            CREDITS_BODY,
        )
        .map_err(|f| f.message)?;
        assert_eq!(overage.refused, None);
        assert_eq!(overage.allowance.and_then(|w| w.used_percent), Some(100.0));

        // No headers at all: the model ran, and nothing was read.
        assert_eq!(
            parse_model_answer(200, &headers(&[]), b"").map_err(|f| f.message)?,
            ModelAnswer::default()
        );
        Ok(())
    }

    /// SPEC-473 RFR-001: a refusal names its limit by the response's claim and its reset by
    /// `unified-reset`; a refusal for usage credits is the model's whatever it claims, and then
    /// only the allowance's own reset says when the model runs again.
    #[test]
    fn a_refused_model_is_read_with_its_claim_and_reset() -> Result<(), String> {
        use yulius_domain::time::Timestamp;
        use yulius_domain::usage::{LimitScope, ModelFamily, ReportedLimit};
        let refused = |pairs: &[(&'static str, &'static str)], body: &[u8]| {
            parse_model_answer(429, &headers(pairs), body)
                .map_err(|f| f.message)?
                .refused
                .ok_or_else(|| "a refusal".to_owned())
        };
        let week = Some(7 * 24 * 60);
        let at = |seconds: i64| Some(Timestamp(seconds * 1000));

        assert_eq!(
            refused(
                &[(CLAIM, "seven_day_overage_included"), (RESET, "1790000000")],
                b""
            )?,
            ReportedLimit {
                scope: LimitScope::Model(None),
                window_minutes: week,
                resets_at: at(1_790_000_000),
            }
        );
        assert_eq!(
            refused(&[(CLAIM, "five_hour"), (RESET, "1790000000")], b"{}")?,
            ReportedLimit {
                scope: LimitScope::Account,
                window_minutes: Some(300),
                resets_at: at(1_790_000_000),
            }
        );
        assert_eq!(
            refused(&[(CLAIM, " seven_day_opus "), (RESET, "1790000000")], b"")?,
            ReportedLimit {
                scope: LimitScope::Model(Some(ModelFamily::Opus)),
                window_minutes: week,
                resets_at: at(1_790_000_000),
            }
        );
        // Refused for usage credits while the response claims the 5-hour window: the model's
        // limit, and the reset is the allowance's own, not the 5-hour window's.
        assert_eq!(
            refused(
                &[
                    (CLAIM, "five_hour"),
                    (RESET, "1790000000"),
                    (ALLOWANCE_RESET, "1790600000"),
                ],
                CREDITS_BODY
            )?,
            ReportedLimit {
                scope: LimitScope::Model(None),
                window_minutes: None,
                resets_at: at(1_790_600_000),
            }
        );
        // The body alone says it, and nothing says when.
        assert_eq!(
            refused(&[], CREDITS_BODY)?,
            ReportedLimit {
                scope: LimitScope::Model(None),
                window_minutes: None,
                resets_at: None,
            }
        );
        // The allowance's claim without `unified-reset`: its own reset header.
        assert_eq!(
            refused(
                &[
                    (CLAIM, "seven_day_overage_included"),
                    (ALLOWANCE_RESET, "1790600000"),
                ],
                b""
            )?
            .resets_at,
            at(1_790_600_000)
        );
        // The claim's reset comes first when both are there.
        assert_eq!(
            refused(
                &[
                    (CLAIM, "seven_day_overage_included"),
                    (RESET, "1790000000"),
                    (ALLOWANCE_RESET, "1790600000"),
                ],
                b""
            )?
            .resets_at,
            at(1_790_000_000)
        );
        // The allowance's reset is not the 5-hour window's.
        assert_eq!(
            refused(
                &[(CLAIM, "five_hour"), (ALLOWANCE_RESET, "1790600000")],
                b""
            )?
            .resets_at,
            None
        );
        // A reset that is no number is no reset.
        assert_eq!(
            refused(&[(CLAIM, "seven_day"), (RESET, "soon")], b"")?.resets_at,
            None
        );
        // The windows a refusal carries are read beside it.
        let answer = parse_model_answer(
            429,
            &headers(&[
                (CLAIM, "seven_day_overage_included"),
                ("anthropic-ratelimit-unified-5h-utilization", "0.3"),
                ("anthropic-ratelimit-unified-7d_oi-utilization", "1"),
            ]),
            b"",
        )
        .map_err(|f| f.message)?;
        assert_eq!(answer.five_hour.and_then(|w| w.used_percent), Some(30.0));
        assert_eq!(answer.allowance.and_then(|w| w.used_percent), Some(100.0));
        Ok(())
    }

    /// SPEC-473 RFR-001: what is no answer about the limit is a failure with its kind and one
    /// plain sentence: a throttle (a 429 that names no limit), a refused sign-in, anything
    /// else.
    #[test]
    fn an_answer_that_says_nothing_about_the_limit_is_a_failure() -> Result<(), String> {
        let failure = |status: u16, pairs: &[(&'static str, &'static str)], body: &[u8]| {
            parse_model_answer(status, &headers(pairs), body)
                .err()
                .ok_or_else(|| format!("{status} is a failure"))
        };
        let throttle = failure(
            429,
            &[("retry-after", "30")],
            br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        )?;
        assert_eq!(throttle.kind, UsageFailureKind::Throttled);
        assert_eq!(throttle.retry_after, Some(Duration::from_secs(30)));
        // An empty claim names no limit; a body that is no JSON says nothing of credits.
        for (pairs, body) in [
            (&[(CLAIM, "  ")][..], &b""[..]),
            (&[][..], &b"credits_required"[..]),
            (
                &[][..],
                &br#"{"error":{"details":{"error_code":"something_else"}}}"#[..],
            ),
        ] {
            assert_eq!(failure(429, pairs, body)?.kind, UsageFailureKind::Throttled);
        }
        assert_eq!(failure(401, &[], b"")?.kind, UsageFailureKind::Rejected);
        // A claim on anything but a refusal is not one.
        for status in [400, 403, 404, 500, 529] {
            let other = failure(status, &[(CLAIM, "five_hour")], CREDITS_BODY)?;
            assert_eq!(other.kind, UsageFailureKind::Unavailable, "{status}");
            assert!(!other.message.contains("HTTP"), "{}", other.message);
        }
        Ok(())
    }

    /// A `setup-token` sign-in: the usage endpoint says 403, the messages answer carries the
    /// headers, and that is the reading.
    #[tokio::test]
    async fn a_refused_sign_in_is_read_from_the_headers() -> Result<(), Box<dyn std::error::Error>>
    {
        headers_after("403 Forbidden").await
    }

    /// Production answers the usage endpoint with 429 for every subscription: same fallback.
    #[tokio::test]
    async fn a_throttled_usage_endpoint_is_read_from_the_headers()
    -> Result<(), Box<dyn std::error::Error>> {
        headers_after("429 Too Many Requests").await
    }

    /// Both the endpoint and the probe throttle: the endpoint's `Retry-After` stands.
    #[tokio::test]
    async fn a_failing_probe_keeps_the_endpoints_retry_after()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let reply = if head.starts_with("GET /api/oauth/usage") {
                    "HTTP/1.1 429 Too Many Requests\r\nretry-after: 3600\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                };
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let mut probe = ProviderUsage::new()?;
        probe.base = base;
        let failure = probe
            .fetch(Provider::Anthropic, &Secret("token".into()))
            .await
            .err()
            .ok_or("a failure")?;
        assert_eq!(failure.kind, UsageFailureKind::Throttled);
        assert_eq!(failure.retry_after, Some(Duration::from_secs(3600)));
        Ok(())
    }

    async fn headers_after(usage_status: &'static str) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let throttle = format!(
                    "HTTP/1.1 {usage_status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                let reply = if head.starts_with("GET /api/oauth/usage") {
                    throttle.as_str()
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
    fn the_profile_email_is_read_when_it_is_an_address() {
        let profile = |account: Value| json!({"account": account, "organization": {"uuid": "o"}});
        assert_eq!(
            parse_profile_email(&profile(json!({"uuid": "u", "email": "pawel@example.com"})))
                .as_deref(),
            Some("pawel@example.com")
        );
        assert_eq!(
            parse_profile_email(&profile(json!({"email_address": "p@example.com"}))).as_deref(),
            Some("p@example.com")
        );
        assert_eq!(
            parse_profile_email(&profile(json!({"email": "not an address"}))),
            None
        );
        assert_eq!(parse_profile_email(&profile(json!({"email": 7}))), None);
        assert_eq!(parse_profile_email(&json!({})), None);
    }

    /// One answer per request path; everything else is 404.
    async fn serve_profile(reply: &'static str) -> Result<String, Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let authorized = head
                    .to_ascii_lowercase()
                    .contains("authorization: bearer token");
                let answer = if head.starts_with("GET /api/oauth/profile") && authorized {
                    reply
                } else {
                    "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                };
                let _ = sock.write_all(answer.as_bytes()).await;
            }
        });
        Ok(base)
    }

    async fn email_from(
        reply: &'static str,
    ) -> Result<Result<Option<String>, UsageFailure>, Box<dyn std::error::Error>> {
        let mut probe = ProviderUsage::new()?;
        probe.base = serve_profile(reply).await?;
        Ok(probe
            .account_email(Provider::Anthropic, &Secret(" token\n".into()))
            .await)
    }

    /// The profile's email, the scope refusal every `setup-token` may get, a refused sign-in and
    /// a server error, each as the app needs it (SPEC-244 SUB-003, SUB-004).
    #[tokio::test]
    async fn the_account_email_comes_from_the_profile() -> Result<(), Box<dyn std::error::Error>> {
        let body = r#"{"account":{"uuid":"u","email":"pawel@example.com"}}"#;
        let ok: &'static str = Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        );
        assert_eq!(
            email_from(ok).await?.map_err(|f| f.message)?.as_deref(),
            Some("pawel@example.com")
        );
        let none = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        assert_eq!(email_from(none).await?.map_err(|f| f.message)?, None);
        for (reply, kind) in [
            (
                "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                UsageFailureKind::NotShared,
            ),
            (
                "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                UsageFailureKind::Rejected,
            ),
            (
                "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                UsageFailureKind::Unavailable,
            ),
        ] {
            let failure = email_from(reply).await?.err().ok_or("a failure")?;
            assert_eq!(failure.kind, kind);
        }
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
