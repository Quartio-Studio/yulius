//! GitHub App: JWT, installation tokens, installations and the manifest flow (ADR-08).

use reqwest::{Method, Url};

use crate::client::{GitHubClient, build_url};
use crate::config::{AppCredentials, OwnerKind};
use crate::error::GitHubError;
use crate::http::Call;
use crate::secret::Secret;
use crate::time::parse_rfc3339;
use crate::types::{InstallationToken, Repo};
use crate::validate;
use crate::wire::{
    WireInstallation, WireInstallationPermissions, WireInstallationRepos, WireInstallationToken,
    WireManifestConversion,
};

/// Cached installation tokens are reused until this long before they expire.
pub const TOKEN_REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;
/// Bound on cached installation tokens.
const MAX_CACHED_TOKENS: usize = 64;
/// Route that receives the OAuth sign-in callback (docs/api.md).
pub const AUTH_CALLBACK_PATH: &str = "/api/auth/github/callback";
/// Route that receives the App manifest conversion code (docs/api.md).
pub const MANIFEST_CALLBACK_PATH: &str = "/api/admin/github/manifest/callback";
/// GitHub's limit on App names.
const MAX_APP_NAME: usize = 34;
/// The permissions of an agent's token (ADR-0114): enough to fetch, push task branches, open pull
/// requests, file issues and read checks; never administration (rulesets, settings, deploy keys)
/// or workflows.
pub const AGENT_TOKEN_PERMISSIONS: &[(&str, &str)] = &[
    ("contents", "write"),
    ("pull_requests", "write"),
    ("issues", "write"),
    ("checks", "read"),
    ("statuses", "read"),
    ("metadata", "read"),
];

impl GitHubClient {
    fn signer(&self) -> Result<&crate::jwt::AppSigner, GitHubError> {
        self.inner.signer.as_ref().ok_or(GitHubError::NotConfigured)
    }

    fn app(&self) -> Result<&AppCredentials, GitHubError> {
        self.inner
            .config
            .app
            .as_ref()
            .ok_or(GitHubError::NotConfigured)
    }

    /// A fresh App JWT (RS256, `iat` = now − 60 s, `exp` = now + 9 min, `iss` = client id).
    ///
    /// # Errors
    /// [`GitHubError::NotConfigured`] without App credentials.
    pub fn app_jwt(&self) -> Result<Secret, GitHubError> {
        self.signer()?.sign(self.now())
    }

    /// An installation access token, served from cache until five minutes before expiry.
    ///
    /// # Errors
    /// Typed GitHub errors; [`GitHubError::NotConfigured`] without App credentials.
    pub async fn create_installation_token(
        &self,
        installation_id: u64,
    ) -> Result<InstallationToken, GitHubError> {
        self.installation_token(installation_id, None, false).await
    }

    /// An installation access token restricted to the single repository `repository` (name
    /// within the installation's owner), cached like [`Self::create_installation_token`]. Give
    /// agents these: an unrestricted installation token works for every repository of the
    /// installation.
    ///
    /// # Errors
    /// Typed GitHub errors; [`GitHubError::InvalidInput`] for an invalid repository name.
    pub async fn create_repository_token(
        &self,
        installation_id: u64,
        repository: &str,
    ) -> Result<InstallationToken, GitHubError> {
        validate::repo_name(repository)?;
        self.installation_token(installation_id, Some(repository), false)
            .await
    }

    /// The token of an agent's Git and `gh` (ADR-0114): restricted to the single repository
    /// `repository` and to [`AGENT_TOKEN_PERMISSIONS`], cached apart from the others. With the
    /// production ruleset, GitHub refuses it every update of the production branch.
    ///
    /// # Errors
    /// Typed GitHub errors; [`GitHubError::InvalidInput`] for an invalid repository name.
    pub async fn create_agent_token(
        &self,
        installation_id: u64,
        repository: &str,
    ) -> Result<InstallationToken, GitHubError> {
        validate::repo_name(repository)?;
        self.installation_token(installation_id, Some(repository), true)
            .await
    }

    async fn installation_token(
        &self,
        installation_id: u64,
        repository: Option<&str>,
        agent: bool,
    ) -> Result<InstallationToken, GitHubError> {
        let now = self.now();
        let key = (
            installation_id,
            repository.map(str::to_ascii_lowercase),
            agent,
        );
        if let Some(cached) = self.cached_token(&key, now) {
            return Ok(cached);
        }
        let jwt = self.app_jwt()?;
        let id = installation_id.to_string();
        let url = self.api_url(&["app", "installations", &id, "access_tokens"])?;
        let mut body = match repository {
            Some(name) => serde_json::json!({ "repositories": [name] }),
            None => serde_json::json!({}),
        };
        if agent {
            body["permissions"] = self.agent_permissions(installation_id, &jwt).await?;
        }
        let wire: WireInstallationToken =
            self.send_json(Method::POST, url, Some(&jwt), body).await?;
        let expires_at = parse_rfc3339(&wire.expires_at)
            .ok_or_else(|| GitHubError::Decode("unparseable expires_at".into()))?;
        let token = InstallationToken {
            token: Secret::new(wire.token),
            expires_at,
        };
        if let Ok(mut cache) = self.inner.tokens.lock() {
            if cache.len() >= MAX_CACHED_TOKENS {
                cache.retain(|_, t| t.expires_at.millis_since(now) > TOKEN_REFRESH_MARGIN_MS);
                if cache.len() >= MAX_CACHED_TOKENS {
                    cache.clear();
                }
            }
            cache.insert(key, token.clone());
        }
        Ok(token)
    }

    /// [`AGENT_TOKEN_PERMISSIONS`] as far as the installation grants them: GitHub refuses a token
    /// request for a permission the installation lacks, and an App created before one was
    /// requested lacks it (`docs/operations.md`). A write it grants only as read is asked as read.
    async fn agent_permissions(
        &self,
        installation_id: u64,
        jwt: &Secret,
    ) -> Result<serde_json::Value, GitHubError> {
        let id = installation_id.to_string();
        let url = self.api_url(&["app", "installations", &id])?;
        let installation: WireInstallationPermissions = self.get_json(url, Some(jwt)).await?;
        let granted = |name: &str| installation.permissions.get(name).map(String::as_str);
        let mut asked = serde_json::Map::new();
        for (name, access) in AGENT_TOKEN_PERMISSIONS {
            let access = match granted(name) {
                Some("write") => *access,
                Some("read") => "read",
                _ => continue,
            };
            asked.insert((*name).to_owned(), access.into());
        }
        if asked.get("contents").and_then(serde_json::Value::as_str) != Some("write") {
            return Err(GitHubError::Forbidden {
                message: "the GitHub App has no Contents: write permission; agents get no token"
                    .into(),
            });
        }
        Ok(asked.into())
    }

    fn cached_token(
        &self,
        key: &(u64, Option<String>, bool),
        now: yulius_domain::time::Timestamp,
    ) -> Option<InstallationToken> {
        let cache = self.inner.tokens.lock().ok()?;
        cache
            .get(key)
            .filter(|t| t.expires_at.millis_since(now) > TOKEN_REFRESH_MARGIN_MS)
            .cloned()
    }

    /// The App's installation on `owner` (user or organization login, case-insensitive).
    ///
    /// # Errors
    /// Typed GitHub errors; [`GitHubError::NotConfigured`] without App credentials.
    pub async fn find_installation(&self, owner: &str) -> Result<Option<u64>, GitHubError> {
        validate::owner(owner)?;
        let jwt = self.app_jwt()?;
        let url = self.api_url(&["app", "installations"])?;
        let installations = self
            .get_pages(url, Some(&jwt), |value| {
                serde_json::from_value::<Vec<WireInstallation>>(value)
                    .map_err(|e| GitHubError::Decode(e.to_string()))
            })
            .await?;
        Ok(installations
            .into_iter()
            .find(|i| {
                i.account
                    .as_ref()
                    .is_some_and(|a| a.login.eq_ignore_ascii_case(owner))
            })
            .map(|i| i.id))
    }

    /// Repositories the installation can access.
    ///
    /// # Errors
    /// Typed GitHub errors; [`GitHubError::NotConfigured`] without App credentials.
    pub async fn installation_repositories(
        &self,
        installation_id: u64,
    ) -> Result<Vec<Repo>, GitHubError> {
        let token = self.create_installation_token(installation_id).await?;
        let url = self.api_url(&["installation", "repositories"])?;
        self.get_pages(url, Some(&token.token), |value| {
            serde_json::from_value::<WireInstallationRepos>(value)
                .map(|page| {
                    page.repositories
                        .into_iter()
                        .map(|r| r.into_repo())
                        .collect()
                })
                .map_err(|e| GitHubError::Decode(e.to_string()))
        })
        .await
    }

    /// The App manifest for the manifest flow: private App with contents, pull requests, commit
    /// statuses (Yulius CI), workflows (onboarding removes GitHub Actions) and administration
    /// write, checks, metadata and members read (the full list
    /// and what each is for: `docs/operations.md`, "GitHub App permissions"); user-to-server
    /// OAuth whose callback is [`AUTH_CALLBACK_PATH`] and whose manifest redirect is
    /// [`MANIFEST_CALLBACK_PATH`] under `public_url`. No webhook is registered. New Apps use
    /// expiring user tokens by default; [`GitHubClient::refresh_user_token`] handles them.
    ///
    /// # Errors
    /// [`GitHubError::InvalidInput`] for a bad login or a non-https public URL.
    pub fn app_manifest(
        &self,
        public_url: &Url,
        owner_login: &str,
        owner_kind: OwnerKind,
    ) -> Result<serde_json::Value, GitHubError> {
        validate::owner(owner_login)?;
        let loopback = matches!(
            public_url.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]")
        );
        if !(public_url.scheme() == "https" || (public_url.scheme() == "http" && loopback)) {
            return Err(GitHubError::InvalidInput {
                what: "public URL",
                reason: "must be https".into(),
            });
        }
        let path_url = |path: &str| -> Result<String, GitHubError> {
            let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            build_url(public_url, &segments).map(String::from)
        };
        let mut name = format!("Yulius {owner_login}");
        name.truncate(MAX_APP_NAME);
        let kind = match owner_kind {
            OwnerKind::User => "account",
            OwnerKind::Organization => "organization",
        };
        Ok(serde_json::json!({
            "name": name,
            "url": public_url.as_str(),
            "description": format!("Yulius coding workspace for the {owner_login} {kind}."),
            "public": false,
            "redirect_url": path_url(MANIFEST_CALLBACK_PATH)?,
            "callback_urls": [path_url(AUTH_CALLBACK_PATH)?],
            "request_oauth_on_install": false,
            "setup_on_update": false,
            "default_permissions": {
                "contents": "write",
                "pull_requests": "write",
                "issues": "write",
                "checks": "read",
                "statuses": "write",
                "workflows": "write",
                "administration": "write",
                "metadata": "read",
                "members": "read"
            },
            "default_events": []
        }))
    }

    /// Where the browser posts the manifest form: `{web}/settings/apps/new` for a user,
    /// `{web}/organizations/{org}/settings/apps/new` for an organization.
    ///
    /// # Errors
    /// [`GitHubError::InvalidInput`] for a bad login.
    pub fn manifest_post_url(
        &self,
        owner_login: &str,
        owner_kind: OwnerKind,
    ) -> Result<String, GitHubError> {
        validate::owner(owner_login)?;
        let url = match owner_kind {
            OwnerKind::User => self.web_url(&["settings", "apps", "new"])?,
            OwnerKind::Organization => {
                self.web_url(&["organizations", owner_login, "settings", "apps", "new"])?
            }
        };
        Ok(url.into())
    }

    /// [`GitHubClient::manifest_post_url`] with a `state` value that GitHub echoes to the
    /// manifest redirect (CSRF protection for the conversion callback).
    ///
    /// # Errors
    /// [`GitHubError::InvalidInput`] for a bad login or state.
    pub fn manifest_post_url_with_state(
        &self,
        owner_login: &str,
        owner_kind: OwnerKind,
        state: &str,
    ) -> Result<String, GitHubError> {
        validate::opaque("state", state)?;
        let mut url =
            Url::parse(&self.manifest_post_url(owner_login, owner_kind)?).map_err(|e| {
                GitHubError::InvalidInput {
                    what: "manifest URL",
                    reason: e.to_string(),
                }
            })?;
        url.query_pairs_mut().append_pair("state", state);
        Ok(url.into())
    }

    /// Exchanges the manifest `code` for the new App's credentials.
    ///
    /// # Errors
    /// Typed GitHub errors (404 for an unknown or expired code).
    pub async fn convert_manifest(&self, code: &str) -> Result<AppCredentials, GitHubError> {
        let code_ok = !code.is_empty()
            && code.len() <= 128
            && code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !code_ok {
            return Err(GitHubError::InvalidInput {
                what: "manifest code",
                reason: "unexpected characters".into(),
            });
        }
        let url = self.api_url(&["app-manifests", code, "conversions"])?;
        let mut call = Call::new(Method::POST, url, None);
        call.sensitive.push(code);
        let reply = self.send_ok(call).await?;
        let wire: WireManifestConversion = crate::client::decode(&reply)?;
        Ok(AppCredentials {
            app_id: wire.id,
            client_id: wire.client_id,
            client_secret: Secret::new(wire.client_secret),
            private_key_pem: Secret::new(wire.pem),
            slug: wire.slug,
            webhook_secret: wire.webhook_secret.map(Secret::new),
        })
    }

    /// The page where an owner installs the configured App
    /// (`{web}/apps/{slug}/installations/new`).
    ///
    /// # Errors
    /// [`GitHubError::NotConfigured`] without App credentials.
    pub fn install_url(&self) -> Result<String, GitHubError> {
        let slug = &self.app()?.slug;
        validate::repo_name(slug)?;
        Ok(self
            .web_url(&["apps", slug, "installations", "new"])?
            .into())
    }
}
