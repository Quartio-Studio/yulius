//! [`UsageProbe`] over HTTPS (SPEC-001 ROT-009): the endpoints the official CLIs read their plan
//! usage from — Claude Code `GET https://api.anthropic.com/api/oauth/usage` (OAuth token,
//! `anthropic-beta: oauth-2025-04-20`; windows `five_hour` and `seven_day` with `utilization`
//! in percent and `resets_at`) and Codex `GET https://chatgpt.com/backend-api/wham/usage`
//! (`ChatGPT-Account-Id`; `rate_limit.primary_window` / `secondary_window` with `used_percent`,
//! `limit_window_seconds` and `reset_at` in epoch seconds). Read-only: tokens are never
//! refreshed here (Codex refresh tokens rotate; runs refresh them). Errors carry the status,
//! never the credential or the response body.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use yulius_app::model::Secret;
use yulius_app::ports::usage::UsageProbe;
use yulius_domain::harness::Provider;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Largest usage response accepted.
const MAX_BODY: usize = 256 * 1024;

/// Usage of Claude and Codex accounts from the providers.
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

    async fn get_json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let response = request
            .send()
            .await
            .map_err(|e| format!("the provider is not reachable ({})", e.without_url()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(format!(
                "the provider refused this account's token for usage (HTTP {}); it refreshes on the next run or needs a re-login",
                status.as_u16()
            ));
        }
        if !status.is_success() {
            return Err(format!("the provider answered HTTP {}", status.as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("reading the usage answer failed ({})", e.without_url()))?;
        if bytes.len() > MAX_BODY {
            return Err("the usage answer is too large".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "the usage answer is not JSON".into())
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
    let usage = AccountUsage {
        primary: window("primary_window"),
        secondary: window("secondary_window"),
        limit_reached: limits
            .get("limit_reached")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..AccountUsage::default()
    };
    if usage.primary.is_none() && usage.secondary.is_none() {
        return Err("the provider reported no usage windows".into());
    }
    Ok(usage)
}

/// Access token and ChatGPT account id from a Codex `auth.json`.
fn codex_identity(auth_json: &str) -> Result<(String, Option<String>), String> {
    let auth: Value =
        serde_json::from_str(auth_json).map_err(|_| "the stored Codex sign-in is not readable")?;
    let tokens = auth
        .get("tokens")
        .ok_or("the Codex sign-in has no tokens")?;
    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or("the Codex sign-in has no access token")?;
    let account = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((access.to_owned(), account))
}

#[async_trait]
impl UsageProbe for ProviderUsage {
    async fn fetch(&self, provider: Provider, credential: &Secret) -> Result<AccountUsage, String> {
        match provider {
            Provider::Anthropic => {
                let request = self
                    .http
                    .get(CLAUDE_USAGE_URL)
                    .bearer_auth(credential.expose().trim())
                    .header("anthropic-beta", "oauth-2025-04-20")
                    .header("accept", "application/json");
                parse_claude(&self.get_json(request).await?)
            }
            Provider::OpenAi => {
                let (access, account) = codex_identity(credential.expose())?;
                let mut request = self
                    .http
                    .get(CODEX_USAGE_URL)
                    .bearer_auth(access)
                    .header("accept", "application/json");
                if let Some(account) = account {
                    request = request.header("ChatGPT-Account-Id", account);
                }
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
                parse_codex(&self.get_json(request).await?, Timestamp(now_ms))
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
    fn codex_windows_parse_with_epoch_or_relative_resets() -> Result<(), String> {
        let usage = parse_codex(
            &json!({"plan_type": "plus", "rate_limit": {
                "limit_reached": false,
                "primary_window": {"used_percent": 61, "limit_window_seconds": 18000, "reset_at": 1_791_410_000},
                "secondary_window": {"used_percent": 9.5, "limit_window_seconds": 604800, "reset_after_seconds": 3600}
            }}),
            Timestamp(1_000),
        )?;
        let primary = usage.primary.ok_or("primary")?;
        assert_eq!(primary.used_percent, Some(61.0));
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(primary.resets_at, Some(Timestamp(1_791_410_000_000)));
        let secondary = usage.secondary.ok_or("secondary")?;
        assert_eq!(secondary.resets_at, Some(Timestamp(3_601_000)));
        assert!(parse_codex(&json!({"rate_limit": {}}), Timestamp(0)).is_err());
        Ok(())
    }

    #[test]
    fn codex_identity_reads_the_access_token_and_account() -> Result<(), String> {
        let (token, account) = codex_identity(
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"at","refresh_token":"rt","account_id":"acc-1"}}"#,
        )?;
        assert_eq!((token.as_str(), account.as_deref()), ("at", Some("acc-1")));
        assert!(codex_identity("{}").is_err());
        Ok(())
    }
}
