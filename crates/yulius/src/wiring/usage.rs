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
//!
//! Codex accounts (SPEC-451 CDX-008): a ChatGPT sign-in's windows come from
//! `GET https://chatgpt.com/backend-api/wham/usage` (the access token of its `auth.json`,
//! `ChatGPT-Account-Id`; `rate_limit.primary_window` / `secondary_window` with `used_percent`,
//! `limit_window_seconds` and `reset_at` in epoch seconds, as Codex CLI 0.167 reads them), and its
//! email from the `id_token`'s claims. Read-only: tokens are never refreshed here (Codex rotates
//! refresh tokens; its runs refresh them and Yulius writes them back). An API-key account has no
//! plan windows: its runs report the rate limits.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use yulius_app::model::Secret;
use yulius_app::ports::usage::{UsageFailure, UsageFailureKind, UsageProbe, plausible_email};
use yulius_domain::harness::Provider;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

fn now() -> Timestamp {
    Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
    )
}

const CLAUDE_API: &str = "https://api.anthropic.com";
const CODEX_API: &str = "https://chatgpt.com/backend-api";
const CODEX_USAGE_PATH: &str = "/wham/usage";
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
    codex_base: String,
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
            codex_base: CODEX_API.to_owned(),
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

/// Codex's `{"rate_limit": {"primary_window": {...}, "secondary_window": {...}}}`.
pub fn parse_codex(body: &Value, now: Timestamp) -> Result<AccountUsage, String> {
    let limits = body
        .get("rate_limit")
        .ok_or("the provider reported no rate limits")?;
    let window = |key: &str| -> Option<UsageWindow> {
        let w = limits.get(key).filter(|w| w.is_object())?;
        let resets_at = w
            .get("reset_at")
            .and_then(Value::as_i64)
            .map(|secs| Timestamp(secs.saturating_mul(1000)))
            .or_else(|| {
                w.get("reset_after_seconds")
                    .and_then(Value::as_i64)
                    .map(|secs| now.plus_millis(secs.saturating_mul(1000)))
            });
        Some(UsageWindow {
            used_percent: w.get("used_percent").and_then(Value::as_f64),
            window_minutes: w
                .get("limit_window_seconds")
                .and_then(Value::as_u64)
                .and_then(|s| u32::try_from(s / 60).ok()),
            resets_at,
        })
    };
    let mut usage = AccountUsage {
        primary: window("primary_window"),
        secondary: window("secondary_window"),
        limit_reached: limits
            .get("limit_reached")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        observed_at: Some(now),
        ..AccountUsage::default()
    };
    if usage.limit_reached {
        // A reached limit resets with its full window; without one, with the latest known reset.
        let windows: Vec<UsageWindow> = usage
            .primary
            .iter()
            .chain(usage.secondary.iter())
            .copied()
            .collect();
        usage.limit_resets_at = windows
            .iter()
            .filter(|w| w.used_percent.is_some_and(|p| p >= 100.0))
            .filter_map(|w| w.resets_at)
            .max()
            .or_else(|| windows.iter().filter_map(|w| w.resets_at).max());
    }
    if usage.primary.is_none() && usage.secondary.is_none() {
        return Err("the provider reported no usage windows".into());
    }
    Ok(usage)
}

/// What a Codex `auth.json` signs in with.
enum CodexIdentity {
    /// A ChatGPT sign-in: its access token, account id and `id_token`.
    ChatGpt {
        access: String,
        account: Option<String>,
        id_token: Option<String>,
    },
    /// An OpenAI API key: no plan windows.
    ApiKey,
}

fn codex_identity(auth_json: &str) -> Result<CodexIdentity, UsageFailure> {
    let unreadable = || {
        UsageFailure::of(
            UsageFailureKind::Rejected,
            "The stored Codex sign-in is not readable; sign in again.",
        )
    };
    let auth: Value = serde_json::from_str(auth_json).map_err(|_| unreadable())?;
    let text = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    if let Some(tokens) = auth.get("tokens").filter(|t| t.is_object())
        && let Some(access) = text(tokens.get("access_token"))
    {
        return Ok(CodexIdentity::ChatGpt {
            access,
            account: text(tokens.get("account_id")),
            id_token: text(tokens.get("id_token")),
        });
    }
    if text(auth.get("OPENAI_API_KEY")).is_some() {
        return Ok(CodexIdentity::ApiKey);
    }
    Err(unreadable())
}

/// The `email` claim of a JWT's payload (not verified: it only names the account).
fn jwt_email(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("email")
        .and_then(Value::as_str)
        .and_then(plausible_email)
}

const CODEX_REJECTED: &str =
    "OpenAI did not accept this Codex sign-in. If its tasks fail too, sign in again.";

/// A Codex sign-in that may not read its usage (403): its runs' rollouts still report it.
const CODEX_NOT_SHARED: &str =
    "ChatGPT shares this Codex subscription's usage with its running Codex sessions only.";

const API_KEY_NO_WINDOWS: &str =
    "An API-key account has no plan usage windows; its runs report the rate limits.";

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
            used_percent: Some(used.min(1.0) * 100.0),
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
            Provider::OpenAi => match codex_identity(credential.expose())? {
                CodexIdentity::ApiKey => Err(UsageFailure::of(
                    UsageFailureKind::NotShared,
                    API_KEY_NO_WINDOWS,
                )),
                CodexIdentity::ChatGpt {
                    access, account, ..
                } => {
                    let mut request = self
                        .http
                        .get(format!("{}{CODEX_USAGE_PATH}", self.codex_base))
                        .bearer_auth(access)
                        .header("accept", "application/json");
                    if let Some(account) = account {
                        request = request.header("ChatGPT-Account-Id", account);
                    }
                    let body = self.get_json(request).await.map_err(|f| match f.kind {
                        UsageFailureKind::Rejected => UsageFailure::of(f.kind, CODEX_REJECTED),
                        UsageFailureKind::NotShared => UsageFailure::of(f.kind, CODEX_NOT_SHARED),
                        _ => f,
                    })?;
                    parse_codex(&body, now()).map_err(UsageFailure::new)
                }
            },
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
            Provider::OpenAi => match codex_identity(credential.expose())? {
                CodexIdentity::ChatGpt { id_token, .. } => {
                    Ok(id_token.as_deref().and_then(jwt_email))
                }
                CodexIdentity::ApiKey => Ok(None),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// SPEC-451 CDX-008: Codex windows parse with epoch or relative resets.
    #[test]
    fn codex_windows_parse_with_epoch_or_relative_resets() -> Result<(), String> {
        let usage = parse_codex(
            &json!({"plan_type": "plus", "rate_limit": {
                "limit_reached": false,
                "primary_window": {"used_percent": 61, "limit_window_seconds": 18000, "reset_at": 1_794_230_000},
                "secondary_window": {"used_percent": 9.5, "limit_window_seconds": 604800, "reset_after_seconds": 3600}
            }}),
            Timestamp(1_000),
        )?;
        let primary = usage.primary.ok_or("primary")?;
        assert_eq!(primary.used_percent, Some(61.0));
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(primary.resets_at, Some(Timestamp(1_794_230_000_000)));
        let secondary = usage.secondary.ok_or("secondary")?;
        assert_eq!(secondary.window_minutes, Some(10_080));
        assert_eq!(secondary.resets_at, Some(Timestamp(3_601_000)));
        assert_eq!(usage.limit_resets_at, None, "no limit reached");
        // A reached limit resets with its full window, else with the latest known reset.
        let full = parse_codex(
            &json!({"rate_limit": {"limit_reached": true,
                "primary_window": {"used_percent": 100, "limit_window_seconds": 18000, "reset_at": 1_794_230_000},
                "secondary_window": {"used_percent": 40, "limit_window_seconds": 604800, "reset_at": 1_794_700_000}}}),
            Timestamp(1_000),
        )?;
        assert!(full.limit_reached);
        assert_eq!(full.limit_resets_at, Some(Timestamp(1_794_230_000_000)));
        let none_full = parse_codex(
            &json!({"rate_limit": {"limit_reached": true,
                "primary_window": {"used_percent": 70, "limit_window_seconds": 18000, "reset_at": 1_794_230_000},
                "secondary_window": {"used_percent": 90, "limit_window_seconds": 604800, "reset_at": 1_794_700_000}}}),
            Timestamp(1_000),
        )?;
        assert_eq!(
            none_full.limit_resets_at,
            Some(Timestamp(1_794_700_000_000))
        );
        assert!(parse_codex(&json!({"rate_limit": {}}), Timestamp(0)).is_err());
        assert!(parse_codex(&json!({}), Timestamp(0)).is_err());
        Ok(())
    }

    /// SPEC-451 CDX-008: a Codex sign-in the usage endpoint will not answer (403) says so in
    /// Codex's words, never Claude's.
    #[tokio::test]
    async fn a_codex_usage_403_speaks_of_codex() -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let reply =
                    "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let mut probe = ProviderUsage::new()?;
        probe.codex_base = base;
        let auth =
            Secret(r#"{"tokens":{"access_token":"synthetic-access","account_id":"acc-1"}}"#.into());
        let failure = probe
            .fetch(Provider::OpenAi, &auth)
            .await
            .err()
            .ok_or("a failure")?;
        assert_eq!(failure.kind, UsageFailureKind::NotShared);
        assert!(!failure.message.contains("Claude"), "{}", failure.message);
        assert!(failure.message.contains("Codex"), "{}", failure.message);
        Ok(())
    }

    /// SPEC-451 CDX-008: the probe asks with the sign-in's access token and account, and an
    /// API-key account has no windows; the email is the id token's claim.
    #[tokio::test]
    async fn codex_usage_is_read_with_the_access_token() -> Result<(), Box<dyn std::error::Error>> {
        use base64::Engine as _;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                let body = r#"{"rate_limit":{"primary_window":{"used_percent":42,"limit_window_seconds":18000,"reset_at":1794230000}}}"#;
                let reply = if head.starts_with("get /wham/usage")
                    && head.contains("authorization: bearer synthetic-access")
                    && head.contains("chatgpt-account-id: acc-1")
                {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .to_owned()
                };
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let mut probe = ProviderUsage::new()?;
        probe.codex_base = base;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"dev@example.com"}"#);
        let auth = format!(
            r#"{{"tokens":{{"access_token":"synthetic-access","account_id":"acc-1","id_token":"h.{claims}.s"}}}}"#
        );
        let usage = probe
            .fetch(Provider::OpenAi, &Secret(auth.clone()))
            .await
            .map_err(|f| f.message)?;
        assert_eq!(usage.primary.and_then(|w| w.used_percent), Some(42.0));
        assert_eq!(
            probe
                .account_email(Provider::OpenAi, &Secret(auth))
                .await
                .map_err(|f| f.message)?,
            Some("dev@example.com".to_owned())
        );
        let refused = probe
            .fetch(
                Provider::OpenAi,
                &Secret(r#"{"tokens":{"access_token":"other"}}"#.into()),
            )
            .await
            .err()
            .ok_or("refused")?;
        assert_eq!(refused.kind, UsageFailureKind::Rejected);
        let key = Secret(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-x"}"#.into());
        let none = probe
            .fetch(Provider::OpenAi, &key)
            .await
            .err()
            .ok_or("none")?;
        assert_eq!(none.kind, UsageFailureKind::NotShared);
        assert_eq!(
            probe
                .account_email(Provider::OpenAi, &key)
                .await
                .map_err(|f| f.message)?,
            None
        );
        let broken = probe
            .fetch(Provider::OpenAi, &Secret("not json".into()))
            .await
            .err()
            .ok_or("broken")?;
        assert_eq!(broken.kind, UsageFailureKind::Rejected);
        Ok(())
    }

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
