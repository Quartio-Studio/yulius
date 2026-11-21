//! Credential-scoped display GETs with one in-flight refresh per URL.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use reqwest::header::ETAG;
use tokio::sync::Mutex as AsyncMutex;

use crate::GitHubError;
use crate::budget::fingerprint;
use crate::http::{Call, Reply, Transport, status_error};

const FRESH_MS: i64 = 30_000;
const ENTRIES: usize = 256;
const BODY_BUDGET: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct Refresh {
    at: i64,
    reply: Option<Reply>,
    failure: Option<(i64, GitHubError)>,
}

type Key = (String, [u8; 32]);
type Entry = Arc<AsyncMutex<Refresh>>;
#[derive(Default)]
struct Cache(HashMap<Key, (Entry, usize)>);

impl Cache {
    fn evict_oldest_idle(&mut self) -> bool {
        let oldest = self
            .0
            .iter()
            .filter(|(_, (entry, _))| Arc::strong_count(entry) == 1)
            .min_by_key(|(_, (entry, _))| entry.try_lock().ok().map(|state| state.at))
            .map(|(key, _)| key.clone());
        oldest.is_some_and(|key| self.0.remove(&key).is_some())
    }

    fn reserve_body(&mut self, key: &Key, bytes: usize) -> bool {
        if let Some((_, size)) = self.0.get_mut(key) {
            *size = 0;
        }
        while self.0.values().map(|(_, size)| *size).sum::<usize>() + bytes > BODY_BUDGET {
            if !self.evict_oldest_idle() {
                return false;
            }
        }
        if let Some((_, size)) = self.0.get_mut(key) {
            *size = bytes;
        }
        true
    }
}

#[derive(Default)]
pub(crate) struct DisplayCache(Mutex<Cache>);

impl DisplayCache {
    fn cache(&self) -> MutexGuard<'_, Cache> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) async fn read(
        &self,
        mut call: Call<'_>,
        transport: &Transport,
    ) -> Result<Reply, GitHubError> {
        let key = (
            call.url.to_string(),
            fingerprint(call.bearer.unwrap_or_default()),
        );
        let entry = {
            let mut cache = self.cache();
            if cache.0.len() == ENTRIES && !cache.0.contains_key(&key) && !cache.evict_oldest_idle()
            {
                return Err(GitHubError::DisplayBusy);
            }
            cache.0.entry(key.clone()).or_default().0.clone()
        };
        let mut state = tokio::time::timeout(Duration::from_secs(30), entry.lock())
            .await
            .map_err(|_| GitHubError::DisplayBusy)?;
        let now = transport.now().0;
        if let Some((at, error)) = &state.failure
            && (0..2_000).contains(&now.saturating_sub(*at))
        {
            return Err(error.clone());
        }
        if let Some(previous) = &state.reply {
            if (0..FRESH_MS).contains(&now.saturating_sub(state.at)) {
                return Ok(previous.clone());
            }
            call.etag = previous.headers.get(ETAG).cloned();
        }
        let response = transport.send(&call).await.and_then(|reply| {
            if reply.is_success() || (reply.status == 304 && call.etag.is_some()) {
                Ok(reply)
            } else {
                Err(status_error(&reply, &call.known_secrets(), transport.now()))
            }
        });
        let reply = match response {
            Ok(reply) => reply,
            // A local reserve refusal is immediately reversible at the reset.
            Err(GitHubError::DisplayPaused) => return Err(GitHubError::DisplayPaused),
            Err(error) => {
                state.failure = Some((transport.now().0, error.clone()));
                return Err(error);
            }
        };
        state.at = transport.now().0;
        state.failure = None;
        if reply.status == 304
            && let Some(previous) = &state.reply
        {
            return Ok(previous.clone());
        }
        state.reply = self
            .cache()
            .reserve_body(&key, reply.body.len())
            .then(|| reply.clone());
        Ok(reply)
    }
}
