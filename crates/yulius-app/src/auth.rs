//! Setup, sessions, GitHub sign-in, CLI device approval and API tokens (SPEC-001 ACC-001..005).

use std::collections::HashMap;
use std::sync::Mutex;

use yulius_contracts::api::auth::{DevicePoll, DeviceStart, InstanceInfo, Me, MeUser};
use yulius_domain::ids::UserId;
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{
    ApiTokenRecord, DeviceGrantRecord, GitHubTokenRecord, Secret, UserRecord, WebSessionRecord,
};
use crate::ports::github::{GhUserToken, GitHubAppConfig};

/// Minimum superadmin password length.
pub const MIN_PASSWORD_LEN: usize = 12;
/// Password-login attempts allowed per client key per minute (SPEC-001 ACC-013).
pub const LOGIN_ATTEMPTS_PER_MINUTE: usize = 5;
/// Clients the sign-in limiter tracks at once. Clients without an attempt in the last minute are
/// forgotten first; when that many still have one, a client not yet tracked waits a minute.
pub const MAX_LOGIN_CLIENTS: usize = 4096;
/// The window of [`LOGIN_ATTEMPTS_PER_MINUTE`] and [`DEVICE_STARTS_PER_MINUTE`].
const LOGIN_WINDOW_MS: i64 = 60_000;
/// CLI device sign-in starts allowed per client key per minute (#169, ADR-0042); the limiter
/// tracks at most [`MAX_LOGIN_CLIENTS`] keys, like sign-in's.
pub const DEVICE_STARTS_PER_MINUTE: usize = 5;
const DEVICE_TTL_MS: i64 = 10 * 60 * 1000;
const OAUTH_STATE_TTL_MS: i64 = 10 * 60 * 1000;
const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ23456789";

/// An authenticated caller.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// The user.
    pub user: UserRecord,
    /// CSRF token of a cookie session (`None` for bearer tokens, which need no CSRF).
    pub csrf_token: Option<String>,
    /// Cookie hash for logout.
    pub session_hash: Option<String>,
}

impl AuthContext {
    /// Whether the caller signed in through the web UI (a cookie session, which carries a CSRF
    /// token) rather than with an API token of the CLI or a script.
    #[must_use]
    pub fn is_browser(&self) -> bool {
        self.csrf_token.is_some()
    }
}

/// A newly created browser session.
#[derive(Debug, Clone)]
pub struct NewSession {
    /// Cookie value (secret; set as HttpOnly cookie).
    pub cookie: Secret,
    /// Session lifetime in seconds.
    pub max_age_s: i64,
    /// The session's view of the user.
    pub me: Me,
    /// A recovery code issued with this session (setup only), shown once.
    pub recovery_code: Option<Secret>,
}

/// Why a GitHub sign-in did not complete. The sign-in page explains each reason in the same
/// window (ACC-004), so none of them is shown as a raw API error.
#[derive(Debug, thiserror::Error)]
pub enum GitHubSignInError {
    /// The sign-in state is unknown, already used or expired (a reload of the callback page).
    #[error("sign-in expired; try again")]
    Expired,
    /// GitHub authenticated `login`, who has not been admitted. `next` is where the refused
    /// sign-in was going, so a retry with another account keeps it.
    #[error("GitHub user {login} has not been admitted to this instance")]
    NotAdmitted {
        /// The GitHub login GitHub signed in.
        login: String,
        /// The sign-in's destination path.
        next: String,
    },
    /// The admitted user is disabled.
    #[error("this user is disabled")]
    Disabled {
        /// The sign-in's destination path.
        next: String,
    },
    /// GitHub or the server failed.
    #[error(transparent)]
    Failed(#[from] AppError),
}

impl GitHubSignInError {
    /// Stable reason code for the sign-in page (`/signin?error=…`).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::NotAdmitted { .. } => "not_admitted",
            Self::Disabled { .. } => "disabled",
            Self::Failed(_) => "failed",
        }
    }
}

/// Counts an attempt of `client_key` in `attempts`, a sliding one-minute window per client key
/// holding at most [`MAX_LOGIN_CLIENTS`] keys: keys without an attempt in the last minute are
/// forgotten first, and while that many still have one a key not yet tracked waits.
///
/// # Errors
/// `Capacity` with `refusal` past `per_minute` attempts or a full table.
fn count_client_attempt(
    attempts: &Mutex<HashMap<String, Vec<i64>>>,
    client_key: &str,
    now: Timestamp,
    per_minute: usize,
    refusal: &str,
) -> AppResult<()> {
    let mut attempts = attempts
        .lock()
        .map_err(|_| AppError::Internal("rate limiter poisoned".into()))?;
    if !attempts.contains_key(client_key) && attempts.len() >= MAX_LOGIN_CLIENTS {
        attempts.retain(|_, times| {
            times.retain(|t| now.0 - *t < LOGIN_WINDOW_MS);
            !times.is_empty()
        });
        if attempts.len() >= MAX_LOGIN_CLIENTS {
            return Err(AppError::Capacity(refusal.into()));
        }
    }
    let entry = attempts.entry(client_key.to_owned()).or_default();
    entry.retain(|t| now.0 - *t < LOGIN_WINDOW_MS);
    if entry.len() >= per_minute {
        return Err(AppError::Capacity(refusal.into()));
    }
    entry.push(now.0);
    Ok(())
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Longest return path kept across GitHub sign-in.
const MAX_RETURN_PATH: usize = 2048;

/// A same-site path to return to after sign-in: one leading `/`, no backslash and no control
/// character. Browsers read `\\` as `/` and drop tabs and newlines, so `/\\host` and
/// `/<tab>/host` would leave the site, and a newline cannot go into a `Location` header (#60).
#[must_use]
pub fn local_return_path(next: &str) -> Option<&str> {
    let local = next.starts_with('/')
        && !next.starts_with("//")
        && next.len() <= MAX_RETURN_PATH
        && !next.chars().any(|c| c == '\\' || c.is_control());
    local.then_some(next)
}

impl App {
    /// Ensures the instance row exists and, while no superadmin exists, issues a fresh one-time
    /// setup token (returned in plain text for the operator; only its hash is stored).
    ///
    /// # Errors
    /// Store failures.
    pub async fn prepare_setup(&self) -> AppResult<Option<Secret>> {
        let store = &self.deps.store;
        let token = hex_token(&self.deps.crypto.random_bytes()[..16]);
        let hash = self.deps.crypto.sha256_hex(token.as_bytes());
        let instance = store.init_instance(&self.config.public_url, &hash).await?;
        if instance.public_url != self.config.public_url {
            store.set_public_url(&self.config.public_url).await?;
        }
        if store.superadmin().await?.is_some() {
            return Ok(None);
        }
        store.set_setup_token_hash(&hash).await?;
        Ok(Some(Secret(token)))
    }

    /// Whether setup is still required.
    ///
    /// # Errors
    /// Store failures.
    pub async fn setup_required(&self) -> AppResult<bool> {
        Ok(self.deps.store.superadmin().await?.is_none())
    }

    /// Creates the superadmin (SPEC-001 ACC-001..003). The password is not recoverable.
    ///
    /// # Errors
    /// `Forbidden` for a wrong token, `Invalid` for weak/mismatched passwords or missing
    /// confirmation, `Conflict` when setup already happened.
    pub async fn complete_setup(
        &self,
        token: &str,
        password: &Secret,
        confirm: &Secret,
        confirm_saved: bool,
    ) -> AppResult<NewSession> {
        let store = &self.deps.store;
        let instance = store
            .instance()
            .await?
            .ok_or(AppError::Conflict("instance not initialized".into()))?;
        if instance.setup_completed_at.is_some() {
            return Err(AppError::Conflict("setup already completed".into()));
        }
        let expected = instance
            .setup_token_hash
            .ok_or_else(|| AppError::Forbidden("setup token unavailable".into()))?;
        let given = self.deps.crypto.sha256_hex(token.trim().as_bytes());
        if !constant_time_eq(given.as_bytes(), expected.as_bytes()) {
            return Err(AppError::Forbidden("invalid setup token".into()));
        }
        if !confirm_saved {
            return Err(AppError::Invalid(
                "confirm that you stored the password safely".into(),
            ));
        }
        if password.expose() != confirm.expose() {
            return Err(AppError::Invalid("passwords do not match".into()));
        }
        if password.expose().chars().count() < MIN_PASSWORD_LEN {
            return Err(AppError::Invalid(format!(
                "use at least {MIN_PASSWORD_LEN} characters"
            )));
        }
        let now = self.now();
        let user = UserRecord {
            id: UserId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            github_login: None,
            github_id: None,
            display_name: "Superadmin".into(),
            avatar_url: None,
            is_superadmin: true,
            is_instance_admin: true,
            password_hash: Some(self.deps.crypto.hash_password(password)?),
            created_at: now,
            disabled_at: None,
        };
        store.complete_setup(&user, now).await?;
        store
            .audit(now, "superadmin", "setup.completed", None, None)
            .await?;
        // The recovery code is shown once, with the first session (ACC-011).
        let code = self.issue_recovery_code().await?;
        let mut session = self.new_session(&user).await?;
        session.recovery_code = Some(code);
        Ok(session)
    }

    /// Superadmin password sign-in with per-client rate limiting (ACC-013). `client_key` is the
    /// client's limit key, chosen by the interface from the TCP peer or the trusted proxy's
    /// client address; it is also the detail of a failed attempt's audit entry.
    ///
    /// # Errors
    /// `Capacity` when rate limited, `Unauthenticated` for wrong credentials.
    pub async fn password_login(
        &self,
        password: &Secret,
        client_key: &str,
    ) -> AppResult<NewSession> {
        let now = self.now();
        count_client_attempt(
            &self.login_attempts,
            client_key,
            now,
            LOGIN_ATTEMPTS_PER_MINUTE,
            "too many sign-in attempts; wait a minute",
        )?;
        let admin = self
            .deps
            .store
            .superadmin()
            .await?
            .ok_or(AppError::Unauthenticated)?;
        let ok = admin
            .password_hash
            .as_deref()
            .is_some_and(|phc| self.deps.crypto.verify_password(password, phc));
        if !ok || admin.disabled_at.is_some() {
            self.deps
                .store
                .audit(now, "superadmin", "login.failed", None, Some(client_key))
                .await?;
            return Err(AppError::Unauthenticated);
        }
        self.new_session(&admin).await
    }

    async fn new_session(&self, user: &UserRecord) -> AppResult<NewSession> {
        let now = self.now();
        let cookie = hex_token(&self.deps.crypto.random_bytes());
        let csrf = hex_token(&self.deps.crypto.random_bytes()[..16]);
        let record = WebSessionRecord {
            id_hash: self.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: user.id.clone(),
            csrf_token: csrf.clone(),
            created_at: now,
            expires_at: now.plus_millis(self.config.session_ttl_ms),
            revoked_at: None,
        };
        self.deps.store.insert_web_session(&record).await?;
        let me = self.me(user, Some(csrf)).await?;
        Ok(NewSession {
            cookie: Secret(cookie),
            max_age_s: self.config.session_ttl_ms / 1000,
            me,
            recovery_code: None,
        })
    }

    /// Resolves a session cookie.
    ///
    /// # Errors
    /// Store failures; invalid cookies yield `Ok(None)`.
    pub async fn authenticate_cookie(&self, cookie: &str) -> AppResult<Option<AuthContext>> {
        let hash = self.deps.crypto.sha256_hex(cookie.as_bytes());
        let Some(session) = self.deps.store.web_session(&hash).await? else {
            return Ok(None);
        };
        if session.revoked_at.is_some() || session.expires_at <= self.now() {
            return Ok(None);
        }
        let Some(user) = self.deps.store.user(&session.user_id).await? else {
            return Ok(None);
        };
        if user.disabled_at.is_some() {
            return Ok(None);
        }
        Ok(Some(AuthContext {
            user,
            csrf_token: Some(session.csrf_token),
            session_hash: Some(hash),
        }))
    }

    /// Whether an authenticated context still holds: the user exists and is not disabled, and a
    /// cookie session is neither revoked nor expired. Open WebSockets re-check it, so logout,
    /// disabling and session expiry end them as they end new requests (#66).
    pub async fn still_signed_in(&self, ctx: &AuthContext) -> bool {
        let Ok(Some(user)) = self.deps.store.user(&ctx.user.id).await else {
            return false;
        };
        if user.disabled_at.is_some() {
            return false;
        }
        match &ctx.session_hash {
            Some(hash) => matches!(
                self.deps.store.web_session(hash).await,
                Ok(Some(session)) if session.revoked_at.is_none() && session.expires_at > self.now()
            ),
            None => true,
        }
    }

    /// Resolves a bearer API token (`yul_…`).
    ///
    /// # Errors
    /// Store failures; invalid tokens yield `Ok(None)`.
    pub async fn authenticate_bearer(&self, token: &str) -> AppResult<Option<AuthContext>> {
        let hash = self.deps.crypto.sha256_hex(token.as_bytes());
        let Some(record) = self.deps.store.api_token(&hash).await? else {
            return Ok(None);
        };
        let Some(user) = self.deps.store.user(&record.user_id).await? else {
            return Ok(None);
        };
        if user.disabled_at.is_some() {
            return Ok(None);
        }
        Ok(Some(AuthContext {
            user,
            csrf_token: None,
            session_hash: None,
        }))
    }

    /// Revokes the caller's session.
    ///
    /// # Errors
    /// Store failures.
    pub async fn logout(&self, ctx: &AuthContext) -> AppResult<()> {
        if let Some(hash) = &ctx.session_hash {
            self.deps.store.revoke_web_session(hash, self.now()).await?;
        }
        Ok(())
    }

    /// The caller's [`Me`] view.
    ///
    /// # Errors
    /// Store failures.
    pub async fn me(&self, user: &UserRecord, csrf: Option<String>) -> AppResult<Me> {
        let instance = self.deps.store.instance().await?;
        Ok(Me {
            user: MeUser {
                id: user.id.to_string(),
                login: user
                    .github_login
                    .clone()
                    .unwrap_or_else(|| "superadmin".into()),
                display_name: user.display_name.clone(),
                avatar_url: user.avatar_url.clone(),
                is_superadmin: user.is_superadmin,
                is_instance_admin: user.is_instance_admin || user.is_superadmin,
                github_linked: user.github_id.is_some(),
            },
            csrf_token: csrf.unwrap_or_default(),
            instance: InstanceInfo {
                owner_login: instance.as_ref().and_then(|i| i.owner_login.clone()),
                owner_kind: instance.as_ref().and_then(|i| i.owner_kind.clone()),
                public_url: self.config.public_url.clone(),
                github_configured: instance.as_ref().is_some_and(|i| i.github_app.is_some()),
                contained: self.config.contained,
                upload_max_bytes: self.config.upload_max_bytes,
            },
        })
    }

    /// Stored GitHub App configuration.
    ///
    /// # Errors
    /// `Conflict` when the App is not configured yet.
    pub async fn github_app(&self) -> AppResult<GitHubAppConfig> {
        let instance = self
            .deps
            .store
            .instance()
            .await?
            .ok_or(AppError::Conflict("instance not initialized".into()))?;
        let sealed = instance
            .github_app
            .ok_or_else(|| AppError::Conflict("the GitHub App is not configured yet".into()))?;
        let bytes = self.deps.crypto.open(&sealed)?;
        serde_json::from_slice(&bytes).map_err(AppError::internal)
    }

    pub(crate) async fn save_user_token(
        &self,
        user: &UserRecord,
        token: &GhUserToken,
    ) -> AppResult<()> {
        let crypto = &self.deps.crypto;
        let record = GitHubTokenRecord {
            user_id: user.id.clone(),
            access: crypto.seal(token.access.expose().as_bytes())?,
            refresh: token
                .refresh
                .as_ref()
                .map(|r| crypto.seal(r.expose().as_bytes()))
                .transpose()?,
            expires_at: token.expires_at,
            refresh_expires_at: token.refresh_expires_at,
        };
        self.deps.store.save_github_token(&record, self.now()).await
    }

    fn github_redirect_uri(&self) -> String {
        format!("{}/api/auth/github/callback", self.config.public_url)
    }

    /// Starts GitHub sign-in; returns the authorization URL. `select_account` makes GitHub show
    /// its account picker (another GitHub account than the browser's current one).
    ///
    /// # Errors
    /// `Conflict` when the App is not configured.
    pub async fn github_sign_in_url(&self, next: &str, select_account: bool) -> AppResult<String> {
        let app = self.github_app().await?;
        let state = hex_token(&self.deps.crypto.random_bytes());
        let now = self.now();
        let next = local_return_path(next).unwrap_or("/");
        self.deps
            .store
            .insert_oauth_state(
                &self.deps.crypto.sha256_hex(state.as_bytes()),
                next,
                now,
                now.plus_millis(OAUTH_STATE_TTL_MS),
            )
            .await?;
        Ok(self.deps.github.authorize_url(
            &app,
            &state,
            &self.github_redirect_uri(),
            select_account,
        ))
    }

    /// Forgets a sign-in state that GitHub returned without a code (the user denied access), so
    /// it cannot be replayed; returns where that sign-in was going (`None` for unknown states).
    ///
    /// # Errors
    /// Store failures.
    pub async fn github_sign_in_abandoned(&self, state: &str) -> AppResult<Option<String>> {
        self.deps
            .store
            .take_oauth_state(&self.deps.crypto.sha256_hex(state.as_bytes()), self.now())
            .await
    }

    /// Completes GitHub sign-in. Only users admitted by the superadmin may sign in.
    ///
    /// # Errors
    /// [`GitHubSignInError`]: `Expired` for an unknown, used or expired state; `NotAdmitted` and
    /// `Disabled` refusals; `Failed` for GitHub and store errors.
    pub async fn github_callback(
        &self,
        code: &str,
        state: &str,
    ) -> Result<(NewSession, String), GitHubSignInError> {
        let now = self.now();
        let store = &self.deps.store;
        let next = store
            .take_oauth_state(&self.deps.crypto.sha256_hex(state.as_bytes()), now)
            .await?
            .ok_or(GitHubSignInError::Expired)?;
        let app = self.github_app().await?;
        let token = self
            .deps
            .github
            .exchange_code(&app, code, &self.github_redirect_uri())
            .await?;
        let gh = self.deps.github.current_user(&token.access).await?;
        let user = match store.user_by_github_id(gh.id).await? {
            Some(user) => user,
            None => store.user_by_login(&gh.login).await?.ok_or_else(|| {
                GitHubSignInError::NotAdmitted {
                    login: gh.login.clone(),
                    next: next.clone(),
                }
            })?,
        };
        if user.disabled_at.is_some() {
            return Err(GitHubSignInError::Disabled { next });
        }
        let name = gh.name.clone().unwrap_or_else(|| gh.login.clone());
        store
            .link_github(&user.id, gh.id, &gh.login, &name, gh.avatar_url.as_deref())
            .await?;
        self.save_user_token(&user, &token).await?;
        store.clear_access(&user.id).await?;
        store
            .audit(now, &gh.login, "login.github", None, None)
            .await?;
        let user = store.user(&user.id).await?.ok_or(AppError::NotFound)?;
        Ok((self.new_session(&user).await?, next))
    }

    /// Starts CLI device approval for the client `client_key` (chosen by the interface like
    /// [`App::password_login`]'s), limited per client (#169, ADR-0042).
    ///
    /// # Errors
    /// `Capacity` when rate limited; store failures.
    pub async fn device_start(&self, client_key: &str) -> AppResult<DeviceStart> {
        let now = self.now();
        count_client_attempt(
            &self.device_starts,
            client_key,
            now,
            DEVICE_STARTS_PER_MINUTE,
            "too many device sign-ins; wait a minute",
        )?;
        let device_code = hex_token(&self.deps.crypto.random_bytes());
        let raw = self.deps.crypto.random_bytes();
        let letters: String = raw
            .iter()
            .take(8)
            .map(|b| char::from(USER_CODE_ALPHABET[usize::from(*b) % USER_CODE_ALPHABET.len()]))
            .collect();
        let user_code = format!("{}-{}", &letters[..4], &letters[4..]);
        self.deps
            .store
            .insert_device_grant(&DeviceGrantRecord {
                user_code: user_code.clone(),
                device_code_hash: self.deps.crypto.sha256_hex(device_code.as_bytes()),
                status: "pending".into(),
                user_id: None,
                token: None,
                created_at: now,
                expires_at: now.plus_millis(DEVICE_TTL_MS),
            })
            .await?;
        Ok(DeviceStart {
            device_code,
            verification_url: format!("{}/device?code={user_code}", self.config.public_url),
            user_code,
            interval_s: 3,
            expires_in_s: u32::try_from(DEVICE_TTL_MS / 1000).unwrap_or(600),
        })
    }

    /// Approves or denies a device grant as the signed-in user; approval issues an API token.
    ///
    /// # Errors
    /// `NotFound` for unknown/expired codes, `Conflict` when already resolved.
    pub async fn device_approve(
        &self,
        ctx: &AuthContext,
        user_code: &str,
        approve: bool,
    ) -> AppResult<()> {
        let now = self.now();
        let grant = self
            .deps
            .store
            .device_grant_by_user_code(user_code.trim())
            .await?
            .ok_or(AppError::NotFound)?;
        if grant.expires_at <= now {
            return Err(AppError::NotFound);
        }
        let (status, sealed) = if approve {
            let token = self.issue_api_token(&ctx.user, "yulius CLI").await?;
            (
                "approved",
                Some(self.deps.crypto.seal(token.expose().as_bytes())?),
            )
        } else {
            ("denied", None)
        };
        let changed = self
            .deps
            .store
            .resolve_device_grant(
                &grant.user_code,
                status,
                Some(&ctx.user.id),
                sealed.as_ref(),
            )
            .await?;
        if changed {
            Ok(())
        } else {
            Err(AppError::Conflict("already resolved".into()))
        }
    }

    /// Polls a device grant; returns the token exactly once.
    ///
    /// # Errors
    /// Store or crypto failures.
    pub async fn device_poll(&self, device_code: &str) -> AppResult<DevicePoll> {
        let hash = self.deps.crypto.sha256_hex(device_code.as_bytes());
        let Some(grant) = self.deps.store.device_grant_by_device(&hash).await? else {
            return Ok(DevicePoll {
                status: "expired".into(),
                token: None,
            });
        };
        if grant.status == "pending" && grant.expires_at <= self.now() {
            return Ok(DevicePoll {
                status: "expired".into(),
                token: None,
            });
        }
        match (grant.status.as_str(), grant.token) {
            ("approved", Some(sealed)) => {
                if !self.deps.store.consume_device_grant(&hash).await? {
                    return Ok(DevicePoll {
                        status: "expired".into(),
                        token: None,
                    });
                }
                let token = String::from_utf8(self.deps.crypto.open(&sealed)?)
                    .map_err(AppError::internal)?;
                Ok(DevicePoll {
                    status: "approved".into(),
                    token: Some(token),
                })
            }
            ("consumed", _) => Ok(DevicePoll {
                status: "expired".into(),
                token: None,
            }),
            (status, _) => Ok(DevicePoll {
                status: status.to_owned(),
                token: None,
            }),
        }
    }

    /// Issues a new API token for a user (stored hashed).
    ///
    /// # Errors
    /// Store failures.
    pub async fn issue_api_token(&self, user: &UserRecord, label: &str) -> AppResult<Secret> {
        let token = format!("yul_{}", hex_token(&self.deps.crypto.random_bytes()));
        self.deps
            .store
            .insert_api_token(&ApiTokenRecord {
                id: self.deps.crypto.new_id(),
                user_id: user.id.clone(),
                token_hash: self.deps.crypto.sha256_hex(token.as_bytes()),
                label: label.to_owned(),
                created_at: self.now(),
                revoked_at: None,
            })
            .await?;
        Ok(Secret(token))
    }

    /// Requires the superadmin.
    ///
    /// # Errors
    /// `Forbidden` otherwise.
    pub fn require_superadmin(&self, ctx: &AuthContext) -> AppResult<()> {
        if ctx.user.is_superadmin {
            Ok(())
        } else {
            Err(AppError::Forbidden("superadmin only".into()))
        }
    }

    /// Requires an instance administrator (or the superadmin).
    ///
    /// # Errors
    /// `Forbidden` otherwise.
    pub fn require_instance_admin(&self, ctx: &AuthContext) -> AppResult<()> {
        if ctx.user.is_superadmin || ctx.user.is_instance_admin {
            Ok(())
        } else {
            Err(AppError::Forbidden("instance administrators only".into()))
        }
    }
}

/// Lowercase hex of bytes.
#[must_use]
pub fn hex_token(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Converts a timestamp to RFC 3339 UTC (seconds precision).
#[must_use]
pub fn rfc3339(ts: Timestamp) -> String {
    let secs = ts.0.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 → (year, month, day) (Howard Hinnant's algorithm).
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 { y + 1 } else { y },
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(Timestamp(0)), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(Timestamp(951_782_400_000)), "2000-02-29T00:00:00Z");
        assert_eq!(
            rfc3339(Timestamp(1_791_331_200_000)),
            "2026-10-07T00:00:00Z"
        );
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
