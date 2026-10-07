//! CLI device approval login (SPEC-001 §7.5, docs/api.md).

use std::thread;
use std::time::{Duration, Instant};

use reqwest::Url;
use yulius_contracts::import::{DevicePoll, DevicePollRequest, DeviceStart, DeviceStatus};

use crate::http::{ApiClient, ApiError, JSON_TIMEOUT, RetryPolicy};
use crate::secret::Secret;

/// Upper bound on the total time spent waiting for approval.
pub const MAX_DEVICE_WAIT: Duration = Duration::from_secs(10 * 60);
/// Server-requested poll intervals are capped at this value.
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Polling behavior of [`device_login`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceLoginOptions {
    /// Minimum delay before each poll; the server's `interval_s` raises it.
    pub poll_interval: Duration,
    /// Total wait bound, clamped to [`MAX_DEVICE_WAIT`] and the codes' expiry.
    pub max_wait: Duration,
    /// Retry policy for each HTTP request.
    pub retry: RetryPolicy,
}

impl Default for DeviceLoginOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            max_wait: MAX_DEVICE_WAIT,
            retry: RetryPolicy::default(),
        }
    }
}

/// Why a device login ended without a token.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// The API call failed.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// A user denied the request in the web UI.
    #[error("the login request was denied")]
    Denied,
    /// The server reported the codes expired.
    #[error("the login request expired; run the login again")]
    Expired,
    /// No decision within the wait bound.
    #[error("no approval within {} s; run the login again", .0.as_secs())]
    TimedOut(Duration),
    /// The server approved without returning a token.
    #[error("the server approved the login but returned no token")]
    MissingToken,
}

/// Starts a device login on `server`, hands the codes to `open_url` (which shows the user code
/// and verification URL, and may open a browser), then polls until the request is approved,
/// denied or expired, or the bounded wait ends. The returned token is never printed.
///
/// Polls happen after waiting `max(options.poll_interval, interval_s)`; the number of polls is
/// bounded by the wait divided by that interval (at least one second per poll for this bound).
///
/// # Errors
/// [`DeviceError::Denied`], [`DeviceError::Expired`], [`DeviceError::TimedOut`],
/// [`DeviceError::MissingToken`] or an API failure.
pub fn device_login(
    server: &Url,
    open_url: &mut dyn FnMut(&DeviceStart),
    options: &DeviceLoginOptions,
) -> Result<Secret, DeviceError> {
    let mut api = ApiClient::new(server.clone(), None)?;
    api.set_retry(options.retry);
    let start_url = api.endpoint(&["api", "device", "start"])?;
    let start: DeviceStart =
        api.execute(&|client| client.post(start_url.clone()).timeout(JSON_TIMEOUT))?;
    if start.device_code.is_empty() {
        return Err(ApiError::Protocol("empty device code".to_owned()).into());
    }
    open_url(&start);

    let interval = options
        .poll_interval
        .max(Duration::from_secs(start.interval_s).min(MAX_POLL_INTERVAL));
    let wait = options
        .max_wait
        .min(MAX_DEVICE_WAIT)
        .min(Duration::from_secs(start.expires_in_s));
    let per_poll = interval.max(Duration::from_secs(1));
    let max_polls = wait.as_millis().div_ceil(per_poll.as_millis()).max(1);
    let deadline = Instant::now() + wait;
    let poll_url = api.endpoint(&["api", "device", "poll"])?;
    let body = DevicePollRequest {
        device_code: start.device_code.clone(),
    };
    let mut polls = 0_u128;
    while polls < max_polls {
        if polls > 0 && Instant::now() >= deadline {
            break;
        }
        thread::sleep(interval);
        polls += 1;
        let poll: DevicePoll = api.execute(&|client| {
            client
                .post(poll_url.clone())
                .timeout(JSON_TIMEOUT)
                .json(&body)
        })?;
        match poll.status {
            DeviceStatus::Pending => {}
            DeviceStatus::Approved => {
                return poll
                    .token
                    .filter(|token| !token.is_empty())
                    .map(Secret::new)
                    .ok_or(DeviceError::MissingToken);
            }
            DeviceStatus::Denied => return Err(DeviceError::Denied),
            DeviceStatus::Expired => return Err(DeviceError::Expired),
        }
    }
    Err(DeviceError::TimedOut(wait))
}
