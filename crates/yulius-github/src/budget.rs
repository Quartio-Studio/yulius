//! Installation quota and request attempts, shared by the plain and App clients.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use reqwest::header::HeaderMap;
use ring::digest::{SHA256, digest};
use yulius_contracts::api::status::{GitHubBudget, GitHubPurpose};
use yulius_domain::time::Timestamp;

use crate::GitHubError;

pub(crate) fn fingerprint(value: &str) -> [u8; 32] {
    let mut key = [0; 32];
    key.copy_from_slice(digest(&SHA256, value.as_bytes()).as_ref());
    key
}

#[derive(Default)]
struct Bucket {
    attempts: BTreeMap<GitHubPurpose, u64>,
    unchanged: u64,
}

#[derive(Copy, Clone)]
struct Quota {
    limit: u64,
    remaining: u64,
    reset: i64,
}
impl Quota {
    fn paused(self, pending: u64) -> bool {
        self.remaining.saturating_sub(pending) <= self.limit.div_ceil(10)
    }
}

#[derive(Default)]
struct Installation {
    seconds: BTreeMap<i64, Bucket>,
    pending: u64,
    quota: Option<Quota>,
}
impl Installation {
    fn current(&self, now: Timestamp) -> Option<Quota> {
        self.quota.filter(|q| q.reset > now.0 / 1000)
    }
}

#[derive(Default)]
struct State {
    tokens: HashMap<[u8; 32], (u64, Timestamp)>,
    installations: HashMap<u64, Installation>,
}

/// Local observations of one server's authenticated installation requests.
#[derive(Default, Clone)]
pub struct RequestMeter(Arc<Mutex<State>>);

impl RequestMeter {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn register(&self, id: u64, token: &str, expires: Timestamp, now: Timestamp) {
        let mut state = self.state();
        state.tokens.retain(|_, (_, expiry)| *expiry > now);
        state.tokens.insert(fingerprint(token), (id, expires));
    }

    /// Count an installation-authenticated request before sending it.
    ///
    /// # Errors
    /// Returns a display pause when this attempt would enter the operational reserve.
    pub fn attempt(
        &self,
        bearer: Option<&str>,
        purpose: GitHubPurpose,
        now: Timestamp,
    ) -> Result<Option<Attempt>, GitHubError> {
        let Some(bearer) = bearer else {
            return Ok(None);
        };
        let mut state = self.state();
        let Some(&(id, expires)) = state.tokens.get(&fingerprint(bearer)) else {
            return Ok(None);
        };
        if expires <= now {
            return Ok(None);
        }
        let app = state.installations.entry(id).or_default();
        if purpose == GitHubPurpose::Display
            && app.current(now).is_some_and(|q| q.paused(app.pending))
        {
            return Err(GitHubError::DisplayPaused);
        }
        let second = now.0 / 1000;
        app.seconds
            .retain(|at, _| *at > second - 3600 && *at <= second);
        *app.seconds
            .entry(second)
            .or_default()
            .attempts
            .entry(purpose)
            .or_default() += 1;
        app.pending += 1;
        Ok(Some(Attempt {
            meter: self.clone(),
            id,
            second,
        }))
    }

    /// Reads local observations. With no configured installation, use the sole observed one.
    #[must_use]
    pub fn snapshot(&self, installation: Option<u64>, now: Timestamp) -> GitHubBudget {
        let state = self.state();
        let app = match installation {
            Some(id) => state.installations.get(&id),
            None if state.installations.len() == 1 => state.installations.values().next(),
            None => None,
        };
        let mut view = GitHubBudget::empty();
        if let Some(app) = app {
            let second = now.0 / 1000;
            let start = app.current(now).map_or_else(
                || {
                    app.quota
                        .map_or(second - 3599, |q| q.reset.max(second - 3599))
                },
                |q| q.reset - 3600,
            );
            for (_, bucket) in app.seconds.range(start.min(second)..=second) {
                for (p, n) in &bucket.attempts {
                    *view.attempts.entry(*p).or_default() += n;
                }
                view.not_modified += bucket.unchanged;
            }
            if let Some(q) = app.current(now) {
                view.limit = Some(q.limit);
                let used = q.limit - q.remaining;
                view.used = Some(used);
                let observed = view.attempts.values().sum::<u64>() - view.not_modified;
                view.unattributed_minimum = Some(used.saturating_sub(observed));
                view.remaining = Some(q.remaining);
                view.resets_at = Some(q.reset);
                view.display_paused = q.paused(app.pending);
            }
        }
        view
    }
}

/// One admitted HTTP attempt. Dropping the guard releases its admission slot.
pub struct Attempt {
    meter: RequestMeter,
    id: u64,
    second: i64,
}

impl Attempt {
    /// Observe quota headers and conditional responses before reading the body.
    pub fn response(self, status: u16, headers: &HeaderMap) {
        let mut state = self.meter.state();
        let Some(app) = state.installations.get_mut(&self.id) else {
            return;
        };
        if status == 304
            && let Some(bucket) = app.seconds.get_mut(&self.second)
        {
            bucket.unchanged += 1;
        }
        if headers
            .get("x-ratelimit-resource")
            .is_some_and(|v| v != "core")
        {
            return;
        }
        let integer = |name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
        };
        if let (Some(limit), Some(remaining), Some(reset)) = (
            integer("x-ratelimit-limit"),
            integer("x-ratelimit-remaining"),
            integer("x-ratelimit-reset"),
        ) && let Ok(reset) = i64::try_from(reset)
            && limit > 0
            && remaining <= limit
            && app
                .quota
                .is_none_or(|q| q.reset < reset || (q.reset == reset && remaining <= q.remaining))
        {
            app.quota = Some(Quota {
                limit,
                remaining,
                reset,
            });
        }
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if let Some(app) = self.meter.state().installations.get_mut(&self.id) {
            app.pending = app.pending.saturating_sub(1);
        }
    }
}
