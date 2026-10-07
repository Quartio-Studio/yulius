//! HTTP transport: fixed headers, bounded bodies, status mapping and GET-only retries.

use std::sync::{Arc, Once};
use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use reqwest::{Method, Url};
use tokio::time::Instant;
use yulius_domain::time::Timestamp;

use crate::config::{Clock, GitHubConfig, RetryPolicy};
use crate::error::{GitHubError, scrub};

/// `User-Agent` sent on every request.
pub const USER_AGENT: &str = "yulius";
/// `Accept` for REST API requests.
pub const API_ACCEPT: &str = "application/vnd.github+json";
/// `Accept` for the OAuth token endpoint, which needs it to answer JSON instead of a form.
pub const OAUTH_ACCEPT: &str = "application/json";
/// `X-GitHub-Api-Version` sent on every request.
pub const API_VERSION: &str = "2022-11-28";
/// Largest response body read into memory.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Request body.
pub(crate) enum Body {
    None,
    Json(serde_json::Value),
    Form(Vec<(&'static str, String)>),
}

/// One logical request (retried as a whole when it is a GET).
pub(crate) struct Call<'a> {
    pub(crate) method: Method,
    pub(crate) url: Url,
    pub(crate) bearer: Option<&'a str>,
    pub(crate) body: Body,
    pub(crate) accept: &'static str,
    /// Extra values to scrub from error messages (client secrets, codes).
    pub(crate) sensitive: Vec<&'a str>,
}

impl<'a> Call<'a> {
    pub(crate) fn new(method: Method, url: Url, bearer: Option<&'a str>) -> Self {
        Self {
            method,
            url,
            bearer,
            body: Body::None,
            accept: API_ACCEPT,
            sensitive: Vec::new(),
        }
    }

    pub(crate) fn json(mut self, body: serde_json::Value) -> Self {
        self.body = Body::Json(body);
        self
    }

    pub(crate) fn known_secrets(&self) -> Vec<&str> {
        let mut known: Vec<&str> = self.sensitive.clone();
        if let Some(token) = self.bearer {
            known.push(token);
        }
        known
    }
}

/// A complete response.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

impl Reply {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub(crate) struct Transport {
    http: reqwest::Client,
    retry: RetryPolicy,
    request_timeout: Duration,
    clock: Arc<dyn Clock>,
}

static CRYPTO_PROVIDER: Once = Once::new();

/// Installs the `ring` rustls provider once per process (ignores "already installed").
pub(crate) fn install_crypto_provider() {
    CRYPTO_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Transport {
    pub(crate) fn new(config: &GitHubConfig, clock: Arc<dyn Clock>) -> Result<Self, GitHubError> {
        install_crypto_provider();
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|e| GitHubError::Transport(format!("client setup failed: {e}")))?;
        Ok(Self {
            http,
            retry: config.retry,
            request_timeout: config.request_timeout,
            clock,
        })
    }

    /// Sends `call`; returns any complete HTTP response (success or not). GETs are retried
    /// on 500/502/503/504, rate limits and transport errors within the retry policy.
    pub(crate) async fn send(&self, call: &Call<'_>) -> Result<Reply, GitHubError> {
        let idempotent = call.method == Method::GET;
        let max_attempts = if idempotent {
            self.retry.max_attempts.max(1)
        } else {
            1
        };
        let started = Instant::now();
        let mut attempt = 0_u32;
        loop {
            attempt += 1;
            let remaining = self.retry.max_total.saturating_sub(started.elapsed());
            let timeout = self.request_timeout.min(remaining).max(Duration::from_millis(1));
            let outcome = self.attempt(call, timeout).await;
            let delay = match &outcome {
                Ok(reply) if reply.is_success() => return outcome,
                Ok(reply) if is_retryable(reply) => retry_delay(reply, attempt, &self.retry, self.clock.now()),
                Ok(_) => return outcome,
                Err(GitHubError::Transport(_)) => backoff(self.retry.base_delay, attempt),
                Err(_) => return outcome,
            };
            if attempt >= max_attempts || started.elapsed() + delay >= self.retry.max_total {
                return outcome;
            }
            tokio::time::sleep(delay).await;
        }
    }

    async fn attempt(&self, call: &Call<'_>, timeout: Duration) -> Result<Reply, GitHubError> {
        let known = call.known_secrets();
        let mut request = self
            .http
            .request(call.method.clone(), call.url.clone())
            .timeout(timeout)
            .header(ACCEPT, HeaderValue::from_static(call.accept))
            .header("X-GitHub-Api-Version", HeaderValue::from_static(API_VERSION));
        if let Some(token) = call.bearer {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                GitHubError::InvalidInput {
                    what: "token",
                    reason: "contains characters not allowed in a header".into(),
                }
            })?;
            value.set_sensitive(true);
            request = request.header(reqwest::header::AUTHORIZATION, value);
        }
        request = match &call.body {
            Body::None => request,
            Body::Json(value) => request.json(value),
            Body::Form(fields) => request.form(fields),
        };
        let mut response = request
            .send()
            .await
            .map_err(|e| transport_error(&e, &known))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(GitHubError::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES,
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| transport_error(&e, &known))?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(GitHubError::ResponseTooLarge {
                    limit: MAX_RESPONSE_BYTES,
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Reply {
            status,
            headers,
            body,
        })
    }

    pub(crate) fn now(&self) -> Timestamp {
        self.clock.now()
    }
}

fn transport_error(error: &reqwest::Error, known: &[&str]) -> GitHubError {
    let kind = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_body() || error.is_decode() {
        "reading the response failed"
    } else {
        "request failed"
    };
    let mut detail = String::new();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        detail = cause.to_string();
        source = cause.source();
    }
    let text = if detail.is_empty() {
        kind.to_owned()
    } else {
        format!("{kind}: {detail}")
    };
    GitHubError::Transport(scrub(&text, known))
}

fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim)
}

fn is_rate_limited(status: u16, headers: &HeaderMap) -> bool {
    status == 429
        || (status == 403
            && (headers.contains_key("retry-after")
                || header(headers, "x-ratelimit-remaining") == Some("0")))
}

fn is_retryable(reply: &Reply) -> bool {
    matches!(reply.status, 500 | 502 | 503 | 504) || is_rate_limited(reply.status, &reply.headers)
}

/// Server-requested wait: `Retry-After` seconds, else time until `x-ratelimit-reset` when the
/// remaining quota is zero.
fn server_wait(headers: &HeaderMap, now: Timestamp) -> Option<Duration> {
    if let Some(seconds) = header(headers, "retry-after").and_then(|v| v.parse::<u64>().ok()) {
        return Some(Duration::from_secs(seconds));
    }
    if header(headers, "x-ratelimit-remaining") == Some("0") {
        let reset = header(headers, "x-ratelimit-reset")?.parse::<i64>().ok()?;
        let wait_ms = reset.saturating_mul(1000).saturating_sub(now.0).max(0);
        return Some(Duration::from_millis(u64::try_from(wait_ms).unwrap_or(0)));
    }
    None
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    base.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1)))
}

fn retry_delay(reply: &Reply, attempt: u32, policy: &RetryPolicy, now: Timestamp) -> Duration {
    server_wait(&reply.headers, now).unwrap_or_else(|| backoff(policy.base_delay, attempt))
}

/// The scrubbed `message` of a GitHub error body.
pub(crate) fn upstream_message(body: &[u8], known: &[&str]) -> String {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    scrub(&message, known)
}

/// Maps a non-success reply to a typed error.
pub(crate) fn status_error(reply: &Reply, known: &[&str], now: Timestamp) -> GitHubError {
    let message = || upstream_message(&reply.body, known);
    match reply.status {
        401 => GitHubError::Unauthorized,
        403 | 429 if is_rate_limited(reply.status, &reply.headers) => GitHubError::RateLimited {
            retry_after: server_wait(&reply.headers, now),
        },
        403 => GitHubError::Forbidden { message: message() },
        404 => GitHubError::NotFound,
        409 => GitHubError::Conflict { message: message() },
        422 => GitHubError::Invalid { message: message() },
        status => GitHubError::Upstream(status),
    }
}
