//! User-to-server OAuth for the GitHub App (SPEC-001 ACC-005).

use reqwest::{Method, Url};
use yulius_domain::time::Timestamp;

use crate::client::{GitHubClient, decode};
use crate::config::AppCredentials;
use crate::error::{GitHubError, scrub};
use crate::http::{Body, Call, OAUTH_ACCEPT};
use crate::secret::Secret;
use crate::types::{GitHubUser, UserToken};
use crate::validate;
use crate::wire::WireUser;

/// The code [`GitHubClient::callback_registered`] sends: never one GitHub issued.
const CALLBACK_PROBE_CODE: &str = "yulius-callback-check";
/// A callback URL no App can hold (`.invalid` is a reserved top-level domain): the control
/// question of [`GitHubClient::callback_registered`].
const CALLBACK_CONTROL_URI: &str = "https://callback-check.invalid/api/auth/github/callback";

impl GitHubClient {
    fn oauth_app(&self) -> Result<&AppCredentials, GitHubError> {
        self.inner
            .config
            .app
            .as_ref()
            .ok_or(GitHubError::NotConfigured)
    }

    /// The GitHub authorization page for signing in (`{web}/login/oauth/authorize`).
    /// `select_account` adds `prompt=select_account`, so GitHub shows its account picker instead
    /// of silently using the account signed in to github.com in this browser.
    ///
    /// # Errors
    /// [`GitHubError::NotConfigured`] without App credentials; [`GitHubError::InvalidInput`]
    /// for a malformed state or redirect URI.
    pub fn authorize_url(
        &self,
        state: &str,
        redirect_uri: &str,
        select_account: bool,
    ) -> Result<String, GitHubError> {
        let app = self.oauth_app()?;
        validate::opaque("state", state)?;
        Url::parse(redirect_uri).map_err(|e| GitHubError::InvalidInput {
            what: "redirect URI",
            reason: e.to_string(),
        })?;
        let mut url = self.web_url(&["login", "oauth", "authorize"])?;
        url.query_pairs_mut()
            .append_pair("client_id", &app.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("allow_signup", "false");
        if select_account {
            url.query_pairs_mut()
                .append_pair("prompt", "select_account");
        }
        Ok(url.into())
    }

    /// Exchanges an authorization `code` for user tokens.
    ///
    /// # Errors
    /// [`GitHubError::OAuth`] with GitHub's error code (e.g. `bad_verification_code`), or
    /// other typed errors.
    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<UserToken, GitHubError> {
        validate::opaque("code", code)?;
        let app = self.oauth_app()?;
        let fields = vec![
            ("client_id", app.client_id.clone()),
            ("client_secret", app.client_secret.expose().to_owned()),
            ("code", code.to_owned()),
            ("redirect_uri", redirect_uri.to_owned()),
        ];
        self.token_request(fields, &[app.client_secret.expose(), code])
            .await
    }

    /// Whether `redirect_uri` is one of the App's callback URLs (SPEC-466 MVC-001). GitHub has no
    /// API to read them, but its token endpoint compares the URI before it looks at the code: a
    /// made-up code answers `redirect_uri_mismatch` when the URI is not registered and
    /// `bad_verification_code` when it is. "Registered" is believed only when GitHub also
    /// refuses a URL that cannot be registered ([`CALLBACK_CONTROL_URI`]): `None` when it takes
    /// that one too, so it does not say for this App. One request, or two for a URI it takes,
    /// with the App's own client and nobody's token.
    ///
    /// # Errors
    /// Any other answer: another OAuth error (`incorrect_client_credentials`), an HTTP error, a
    /// timeout.
    pub async fn callback_registered(
        &self,
        redirect_uri: &str,
    ) -> Result<Option<bool>, GitHubError> {
        if !self.callback_taken(redirect_uri).await? {
            return Ok(Some(false));
        }
        let compared = !self.callback_taken(CALLBACK_CONTROL_URI).await?;
        Ok(compared.then_some(true))
    }

    /// Whether the token endpoint takes `redirect_uri` with a made-up code.
    async fn callback_taken(&self, redirect_uri: &str) -> Result<bool, GitHubError> {
        let app = self.oauth_app()?;
        let fields = vec![
            ("client_id", app.client_id.clone()),
            ("client_secret", app.client_secret.expose().to_owned()),
            ("code", CALLBACK_PROBE_CODE.to_owned()),
            ("redirect_uri", redirect_uri.to_owned()),
        ];
        match self
            .token_request(fields, &[app.client_secret.expose()])
            .await
        {
            Err(GitHubError::OAuth(code)) if code == "bad_verification_code" => Ok(true),
            Err(GitHubError::OAuth(code)) if code == "redirect_uri_mismatch" => Ok(false),
            Err(other) => Err(other),
            Ok(_) => Err(GitHubError::Decode("a made-up code was accepted".into())),
        }
    }

    /// Exchanges a refresh token for a new token set (the old refresh token is consumed).
    ///
    /// # Errors
    /// [`GitHubError::OAuth`] (e.g. `bad_refresh_token`) or other typed errors.
    pub async fn refresh_user_token(&self, refresh: &Secret) -> Result<UserToken, GitHubError> {
        validate::opaque("refresh token", refresh.expose())?;
        let app = self.oauth_app()?;
        let fields = vec![
            ("client_id", app.client_id.clone()),
            ("client_secret", app.client_secret.expose().to_owned()),
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh.expose().to_owned()),
        ];
        self.token_request(fields, &[app.client_secret.expose(), refresh.expose()])
            .await
    }

    async fn token_request(
        &self,
        fields: Vec<(&'static str, String)>,
        sensitive: &[&str],
    ) -> Result<UserToken, GitHubError> {
        let url = self.web_url(&["login", "oauth", "access_token"])?;
        let mut call = Call::new(Method::POST, url, None);
        call.body = Body::Form(fields);
        call.accept = OAUTH_ACCEPT;
        call.sensitive = sensitive.to_vec();
        let reply = self.send_ok(call).await?;
        let value: serde_json::Value = decode(&reply)?;
        parse_user_token(&value, self.now(), sensitive)
    }

    /// The user the token belongs to (`GET /user`).
    ///
    /// # Errors
    /// [`GitHubError::Unauthorized`] for an invalid or expired token, or other typed errors.
    pub async fn current_user(&self, token: &Secret) -> Result<GitHubUser, GitHubError> {
        let url = self.api_url(&["user"])?;
        let wire: WireUser = self.get_json(url, Some(token)).await?;
        Ok(GitHubUser {
            id: wire.id,
            login: wire.login,
            name: wire.name,
            avatar_url: wire.avatar_url,
        })
    }
}

fn expiry(now: Timestamp, value: &serde_json::Value, field: &str) -> Option<Timestamp> {
    value
        .get(field)
        .and_then(serde_json::Value::as_i64)
        .map(|seconds| now.plus_millis(seconds.saturating_mul(1000)))
}

fn parse_user_token(
    value: &serde_json::Value,
    now: Timestamp,
    sensitive: &[&str],
) -> Result<UserToken, GitHubError> {
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return Err(GitHubError::OAuth(scrub(error, sensitive)));
    }
    let access = value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| GitHubError::Decode("missing access_token".into()))?;
    let refresh = value
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .map(Secret::new);
    Ok(UserToken {
        access: Secret::new(access),
        refresh,
        expires_at: expiry(now, value, "expires_in"),
        refresh_expires_at: expiry(now, value, "refresh_token_expires_in"),
    })
}
