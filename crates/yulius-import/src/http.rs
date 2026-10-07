//! Blocking HTTP client for the Yulius API: rustls (ring), bearer auth, bounded responses and
//! bounded retries of transient failures.

use std::io::Read;
use std::net::IpAddr;
use std::sync::Once;
use std::thread;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::Url;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::secret::Secret;

/// JSON responses larger than this are rejected (docs/api.md: JSON bodies ≤ 1 MiB).
const MAX_JSON_RESPONSE_BYTES: u64 = 1024 * 1024;
/// Error bodies are read up to this many bytes.
const MAX_ERROR_BODY_BYTES: u64 = 64 * 1024;
/// Server-provided error messages are truncated to this many characters.
const MAX_MESSAGE_CHARS: usize = 500;
/// Hard ceiling on the sum of retry backoff delays for one request.
const MAX_TOTAL_BACKOFF: Duration = Duration::from_secs(10);
/// Hard ceiling on attempts per request.
const MAX_ATTEMPTS: u32 = 10;
/// Per-request timeout for JSON calls.
pub(crate) const JSON_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-request timeout for chunk uploads (8 MiB on a slow link).
pub(crate) const CHUNK_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A failed API call.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The server URL is not `https` (plain `http` is accepted only for loopback hosts).
    #[error("server URL {0} must use https (plain http is accepted only for localhost)")]
    InsecureServerUrl(String),
    /// The server URL cannot be used as an API base.
    #[error("invalid server URL {0}")]
    InvalidServerUrl(String),
    /// The HTTP client could not be built.
    #[error("failed to initialize the HTTP client: {0}")]
    Client(#[source] reqwest::Error),
    /// `401`: missing, expired or revoked token.
    #[error("not signed in, or the token is no longer valid; run `yulius login --server <url>`")]
    Unauthenticated,
    /// `403`: the user lacks the required permission (for example repository admin, IMP-002).
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// `404`.
    #[error("not found: {0}")]
    NotFound(String),
    /// `409`: conflicting state (for example a chunk offset beyond the received length).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Another non-retryable status (`400`, `413`, `422`, …).
    #[error("request rejected with HTTP {status}: {message}")]
    Rejected {
        /// HTTP status code.
        status: u16,
        /// Server message (truncated).
        message: String,
    },
    /// Transport errors or `408`/`429`/`5xx` persisted through every attempt.
    #[error("server unavailable after {attempts} attempts: {last}")]
    Unavailable {
        /// Attempts made.
        attempts: u32,
        /// Last failure.
        last: String,
    },
    /// The response violated the protocol.
    #[error("unexpected server response: {0}")]
    Protocol(String),
}

/// Bounded retry of transient failures with exponential backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts per request (clamped to 1..=10).
    pub attempts: u32,
    /// Delay before the first retry.
    pub initial_backoff: Duration,
    /// Largest single delay (also caps a server `Retry-After`).
    pub max_backoff: Duration,
    /// Largest sum of delays per request (clamped to at most 10 s).
    pub max_total_backoff: Duration,
}

impl Default for RetryPolicy {
    /// Three attempts with 1 s then 2 s backoff (5 s per delay, 10 s total at most).
    fn default() -> Self {
        Self {
            attempts: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(5),
            max_total_backoff: MAX_TOTAL_BACKOFF,
        }
    }
}

impl RetryPolicy {
    /// `attempts` attempts without any delay (for tests and local servers).
    #[must_use]
    pub fn immediate(attempts: u32) -> Self {
        Self {
            attempts,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
            max_total_backoff: Duration::ZERO,
        }
    }

    fn attempts(&self) -> u32 {
        self.attempts.clamp(1, MAX_ATTEMPTS)
    }

    /// Delay before retry number `retry` (0-based), honoring `retry_after` up to the caps.
    fn delay(&self, retry: u32, retry_after: Option<Duration>, waited: Duration) -> Duration {
        let exponential = self
            .initial_backoff
            .saturating_mul(2_u32.saturating_pow(retry));
        let wanted = retry_after.unwrap_or(exponential).min(self.max_backoff);
        let budget = self.max_total_backoff.min(MAX_TOTAL_BACKOFF);
        wanted.min(budget.saturating_sub(waited))
    }
}

static INSTALL_PROVIDER: Once = Once::new();

/// Installs the ring crypto provider as the process default (shared TLS decision); an already
/// installed provider is kept.
fn install_crypto_provider() {
    INSTALL_PROVIDER.call_once(|| {
        // Err means another provider is already installed, which is acceptable.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Accepts `https` URLs and `http` URLs whose host is a loopback address or `localhost`.
pub(crate) fn validate_server_url(url: &Url) -> Result<(), ApiError> {
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err(ApiError::InvalidServerUrl(url.to_string()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ApiError::InvalidServerUrl(
            "server URLs must not contain credentials".to_owned(),
        ));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_host(url.host_str().unwrap_or_default()) => Ok(()),
        _ => Err(ApiError::InsecureServerUrl(url.to_string())),
    }
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: Option<String>,
    message: Option<String>,
}

/// Authenticated JSON client bound to one server.
pub(crate) struct ApiClient {
    base: Url,
    client: Client,
    token: Option<Secret>,
    retry: RetryPolicy,
}

impl ApiClient {
    pub(crate) fn new(base: Url, token: Option<Secret>) -> Result<Self, ApiError> {
        validate_server_url(&base)?;
        install_crypto_provider();
        let client = Client::builder()
            .user_agent(concat!("yulius-import/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CHUNK_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ApiError::Client)?;
        Ok(Self {
            base,
            client,
            token,
            retry: RetryPolicy::default(),
        })
    }

    pub(crate) fn set_retry(&mut self, retry: RetryPolicy) {
        self.retry = retry;
    }

    pub(crate) fn base(&self) -> &Url {
        &self.base
    }

    /// The base URL with `segments` appended, each percent-encoded as one path segment.
    pub(crate) fn endpoint(&self, segments: &[&str]) -> Result<Url, ApiError> {
        let mut url = self.base.clone();
        url.set_query(None);
        url.set_fragment(None);
        url.path_segments_mut()
            .map_err(|()| ApiError::InvalidServerUrl(self.base.to_string()))?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }

    /// Sends the request built by `build`, retrying transient failures, and decodes JSON.
    pub(crate) fn execute<T: DeserializeOwned>(
        &self,
        build: &dyn Fn(&Client) -> RequestBuilder,
    ) -> Result<T, ApiError> {
        let attempts = self.retry.attempts();
        let mut waited = Duration::ZERO;
        let mut retry_after = None;
        let mut last = String::new();
        for attempt in 0..attempts {
            if attempt > 0 {
                let delay = self.retry.delay(attempt - 1, retry_after, waited);
                thread::sleep(delay);
                waited += delay;
            }
            let mut request = build(&self.client);
            if let Some(token) = &self.token {
                request = request.bearer_auth(token.expose());
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(error) if error.is_builder() => {
                    return Err(ApiError::Protocol(error.to_string()));
                }
                Err(error) => {
                    last = error.to_string();
                    retry_after = None;
                    continue;
                }
            };
            let status = response.status();
            if status.is_success() {
                return decode_json(response);
            }
            retry_after = parse_retry_after(&response);
            let message = error_message(response, status);
            match status.as_u16() {
                401 => return Err(ApiError::Unauthenticated),
                403 => return Err(ApiError::Forbidden(message)),
                404 => return Err(ApiError::NotFound(message)),
                409 => return Err(ApiError::Conflict(message)),
                408 | 429 | 500..=599 => last = format!("HTTP {}: {message}", status.as_u16()),
                other => {
                    return Err(ApiError::Rejected {
                        status: other,
                        message,
                    });
                }
            }
        }
        Err(ApiError::Unavailable { attempts, last })
    }
}

fn decode_json<T: DeserializeOwned>(response: Response) -> Result<T, ApiError> {
    let mut body = Vec::new();
    response
        .take(MAX_JSON_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| ApiError::Protocol(format!("reading the response failed: {error}")))?;
    if body.len() as u64 > MAX_JSON_RESPONSE_BYTES {
        return Err(ApiError::Protocol("response body exceeds 1 MiB".to_owned()));
    }
    serde_json::from_slice(&body)
        .map_err(|error| ApiError::Protocol(format!("invalid JSON response: {error}")))
}

fn parse_retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// The server's `{"error":{"code","message"}}` message, the raw body, or the status reason.
fn error_message(response: Response, status: StatusCode) -> String {
    let mut body = Vec::new();
    // A truncated or unreadable error body still leaves the status as the message.
    let _ = response.take(MAX_ERROR_BODY_BYTES).read_to_end(&mut body);
    let text = match serde_json::from_slice::<ErrorBody>(&body) {
        Ok(parsed) => match (parsed.error.code, parsed.error.message) {
            (Some(code), Some(message)) => format!("{message} ({code})"),
            (None, Some(message)) => message,
            (Some(code), None) => code,
            (None, None) => String::new(),
        },
        Err(_) => String::from_utf8_lossy(&body).trim().to_owned(),
    };
    let text = if text.is_empty() {
        status.canonical_reason().unwrap_or("error").to_owned()
    } else {
        text
    };
    if text.chars().count() > MAX_MESSAGE_CHARS {
        let mut short: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
        short.push('…');
        short
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Result<Url, ApiError> {
        Url::parse(text).map_err(|error| ApiError::InvalidServerUrl(error.to_string()))
    }

    #[test]
    fn server_urls_require_https_except_loopback() -> Result<(), ApiError> {
        for good in [
            "https://yulius.quartio.ai",
            "https://yulius.quartio.ai/base/",
            "http://localhost:8080",
            "http://127.0.0.1:1234",
            "http://[::1]:1234",
        ] {
            assert!(validate_server_url(&url(good)?).is_ok(), "{good}");
        }
        for bad in [
            "http://yulius.quartio.ai",
            "http://10.0.0.1",
            "ftp://yulius.quartio.ai",
            "https://user:pw@yulius.quartio.ai",
            "mailto:a@b.c",
        ] {
            assert!(validate_server_url(&url(bad)?).is_err(), "{bad}");
        }
        Ok(())
    }

    #[test]
    fn endpoints_percent_encode_segments_and_keep_base_path() -> Result<(), ApiError> {
        let client = ApiClient::new(url("https://y.example/base/")?, None)?;
        let endpoint = client.endpoint(&["api", "import", "j1", "files", "sessions/claude/a.jsonl"])?;
        assert_eq!(
            endpoint.as_str(),
            "https://y.example/base/api/import/j1/files/sessions%2Fclaude%2Fa.jsonl"
        );
        Ok(())
    }

    #[test]
    fn backoff_is_exponential_and_bounded() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay(0, None, Duration::ZERO), Duration::from_secs(1));
        assert_eq!(policy.delay(1, None, Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(policy.delay(5, None, Duration::ZERO), Duration::from_secs(5));
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(60)), Duration::ZERO),
            Duration::from_secs(5)
        );
        assert_eq!(
            policy.delay(3, None, Duration::from_secs(9)),
            Duration::from_secs(1)
        );
        let greedy = RetryPolicy {
            attempts: 50,
            max_total_backoff: Duration::from_secs(600),
            ..RetryPolicy::default()
        };
        assert_eq!(greedy.attempts(), MAX_ATTEMPTS);
        assert_eq!(greedy.delay(2, None, Duration::from_secs(10)), Duration::ZERO);
        assert_eq!(RetryPolicy::immediate(0).attempts(), 1);
    }
}
