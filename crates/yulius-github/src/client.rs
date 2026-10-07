//! The GitHub client: construction, URL building and request helpers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use reqwest::{Method, Url};
use serde::de::DeserializeOwned;

use crate::config::{Clock, GitHubConfig, SystemClock};
use crate::error::{GitHubError, scrub};
use crate::http::{Call, Reply, Transport, status_error};
use crate::jwt::AppSigner;
use crate::secret::Secret;
use crate::types::InstallationToken;

/// Most pages fetched by any paginated listing (100 items per page).
pub const MAX_PAGES: u32 = 10;
/// Items requested per page.
pub(crate) const PER_PAGE: usize = 100;

/// Async GitHub REST/App/OAuth client. Cheap to clone; clones share the connection pool and
/// the installation-token cache.
#[derive(Clone)]
pub struct GitHubClient {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) config: GitHubConfig,
    pub(crate) transport: Transport,
    pub(crate) signer: Option<AppSigner>,
    pub(crate) tokens: Mutex<HashMap<u64, InstallationToken>>,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("api_base", &self.inner.config.api_base.as_str())
            .field("web_base", &self.inner.config.web_base.as_str())
            .field("app_configured", &self.inner.signer.is_some())
            .finish_non_exhaustive()
    }
}

impl GitHubClient {
    /// Creates a client using the system clock.
    ///
    /// # Errors
    /// [`GitHubError::InvalidInput`] for an unusable configuration, [`GitHubError::InvalidKey`]
    /// when App credentials carry an unusable private key.
    pub fn new(config: GitHubConfig) -> Result<Self, GitHubError> {
        Self::with_clock(config, Arc::new(SystemClock))
    }

    /// Creates a client with an injected clock (token expiry and JWT claims).
    ///
    /// # Errors
    /// As [`GitHubClient::new`].
    pub fn with_clock(config: GitHubConfig, clock: Arc<dyn Clock>) -> Result<Self, GitHubError> {
        config.validate()?;
        let signer = config.app.as_ref().map(AppSigner::new).transpose()?;
        let transport = Transport::new(&config, clock)?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                transport,
                signer,
                tokens: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The configuration this client was built with.
    #[must_use]
    pub fn config(&self) -> &GitHubConfig {
        &self.inner.config
    }

    pub(crate) fn api_url(&self, segments: &[&str]) -> Result<Url, GitHubError> {
        build_url(&self.inner.config.api_base, segments)
    }

    pub(crate) fn web_url(&self, segments: &[&str]) -> Result<Url, GitHubError> {
        build_url(&self.inner.config.web_base, segments)
    }

    /// Sends and requires a 2xx reply.
    pub(crate) async fn send_ok(&self, call: Call<'_>) -> Result<Reply, GitHubError> {
        let reply = self.inner.transport.send(&call).await?;
        if reply.is_success() {
            Ok(reply)
        } else {
            Err(self.error_for(&reply, &call))
        }
    }

    /// Sends and returns whatever complete reply arrived.
    pub(crate) async fn send_any(&self, call: &Call<'_>) -> Result<Reply, GitHubError> {
        self.inner.transport.send(call).await
    }

    pub(crate) fn error_for(&self, reply: &Reply, call: &Call<'_>) -> GitHubError {
        status_error(reply, &call.known_secrets(), self.inner.transport.now())
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        url: Url,
        token: Option<&Secret>,
    ) -> Result<T, GitHubError> {
        let reply = self
            .send_ok(Call::new(Method::GET, url, token.map(Secret::expose)))
            .await?;
        decode(&reply)
    }

    pub(crate) async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        token: Option<&Secret>,
        body: serde_json::Value,
    ) -> Result<T, GitHubError> {
        let reply = self
            .send_ok(Call::new(method, url, token.map(Secret::expose)).json(body))
            .await?;
        decode(&reply)
    }

    /// Fetches up to [`MAX_PAGES`] pages of `url` (with `per_page`/`page` added), stopping at
    /// the first short page.
    pub(crate) async fn get_pages<T>(
        &self,
        url: Url,
        token: Option<&Secret>,
        extract: impl Fn(serde_json::Value) -> Result<Vec<T>, GitHubError>,
    ) -> Result<Vec<T>, GitHubError> {
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let mut page_url = url.clone();
            page_url
                .query_pairs_mut()
                .append_pair("per_page", &PER_PAGE.to_string())
                .append_pair("page", &page.to_string());
            let value: serde_json::Value = self.get_json(page_url, token).await?;
            let batch = extract(value)?;
            let short = batch.len() < PER_PAGE;
            items.extend(batch);
            if short {
                break;
            }
        }
        Ok(items)
    }

    pub(crate) fn now(&self) -> yulius_domain::time::Timestamp {
        self.inner.transport.now()
    }
}

/// Appends path segments (each percent-encoded) to a base URL.
pub(crate) fn build_url(base: &Url, segments: &[&str]) -> Result<Url, GitHubError> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|()| GitHubError::InvalidInput {
            what: "base URL",
            reason: "cannot be a base".into(),
        })?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

/// Decodes a JSON body into `T` with a scrubbed error.
pub(crate) fn decode<T: DeserializeOwned>(reply: &Reply) -> Result<T, GitHubError> {
    serde_json::from_slice(&reply.body).map_err(|e| GitHubError::Decode(scrub(&e.to_string(), &[])))
}
