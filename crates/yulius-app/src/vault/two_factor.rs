//! Two-factor on the vault (SPEC-040, ADR-0034): each person sets up their own authenticator
//! once (after a recent sign-in or confirmation), and every reveal needs a current code entered
//! in the browser session within the last 10 minutes. Codes work once and are rate limited, and
//! 10 wrong codes in a row lock codes until the person confirms it is them again.

use yulius_contracts::api::secrets::{ReauthStatus, TwoFactorSetup};
use yulius_domain::vault::totp::{base32_encode, code_from_hmac, counter, otpauth_uri, parse_otp};
use zeroize::Zeroizing;

use super::Revealed;
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::TwoFactorRecord;
use yulius_domain::time::Timestamp;
use zeroize::Zeroize as _;

/// Seed length in bytes (160 bits, RFC 4226's recommendation).
const SEED_BYTES: usize = 20;
/// Accepted clock drift in time steps either way.
const DRIFT_STEPS: i64 = 1;
/// Name shown in authenticator apps.
const ISSUER: &str = "Yulius";
/// Wrong codes in a row after which codes are refused until the person confirms it is them
/// (NIST SP 800-63B §5.2.2 caps them at 100; a 6-digit code needs far fewer).
pub(super) const MAX_FAILED_CODES: u32 = 10;

/// Whether `record` is locked: too many wrong codes, and no identity confirmation since.
fn locked(record: &TwoFactorRecord, identity: Option<Timestamp>) -> bool {
    record.failed_attempts >= MAX_FAILED_CODES
        && !identity
            .zip(record.failed_at)
            .is_some_and(|(confirmed, failed)| confirmed > failed)
}

fn aad(user_id: &str) -> Vec<u8> {
    format!("{user_id}|2fa").into_bytes()
}

fn browser_session(ctx: &AuthContext) -> AppResult<&str> {
    ctx.session_hash
        .as_deref()
        .ok_or_else(|| AppError::Forbidden("two-factor is used only in the web UI".into()))
}

impl App {
    /// Whether the caller's seed is no longer the one in `checked`.
    async fn seed_replaced(&self, user_id: &str, checked: &TwoFactorRecord) -> AppResult<bool> {
        Ok(self
            .deps
            .store
            .vault_two_factor(user_id)
            .await?
            .is_none_or(|now| now.envelope.ciphertext != checked.envelope.ciphertext))
    }

    /// `missing`, `pending`, `on` or `locked` (too many wrong codes) for the caller, whose
    /// identity was last confirmed at `identity`.
    pub(super) async fn two_factor_state(
        &self,
        ctx: &AuthContext,
        identity: Option<Timestamp>,
    ) -> AppResult<&'static str> {
        Ok(
            match self
                .deps
                .store
                .vault_two_factor(ctx.user.id.as_str())
                .await?
            {
                None => "missing",
                Some(record) if locked(&record, identity) => "locked",
                Some(record) if record.confirmed_at.is_none() => "pending",
                Some(_) => "on",
            },
        )
    }

    /// `POST /api/auth/two-factor`: a new seed for the caller's authenticator. Needs a browser
    /// session that signed in or confirmed it is them in the last 10 minutes; a confirmed seed is
    /// never replaced.
    ///
    /// # Errors
    /// `Forbidden` for API tokens, `Conflict` when two-factor is already on, store failures.
    pub async fn setup_two_factor(&self, ctx: &AuthContext) -> AppResult<Revealed<TwoFactorSetup>> {
        browser_session(ctx)?;
        self.limit_attempts(&format!("2fa-setup:{}", ctx.user.id.as_str()))?;
        let status = self.reauth_status(ctx).await?;
        if status.two_factor == "on" {
            return Err(AppError::Conflict("two-factor is already set up".into()));
        }
        if !status.identity_fresh {
            return Ok(Revealed::ReauthRequired(status));
        }
        let random = Zeroizing::new(self.deps.crypto.random_bytes());
        let secret = Zeroizing::new(base32_encode(&random[..SEED_BYTES]));
        let user_id = ctx.user.id.as_str();
        let record = TwoFactorRecord {
            user_id: user_id.to_owned(),
            envelope: self
                .deps
                .crypto
                .envelope_seal(&aad(user_id), secret.as_bytes())?,
            created_at: self.now(),
            confirmed_at: None,
            last_counter: None,
            failed_attempts: 0,
            failed_at: None,
        };
        if !self.deps.store.vault_put_two_factor(&record).await? {
            return Err(AppError::Conflict("two-factor is already set up".into()));
        }
        let actor = Self::vault_actor(ctx);
        self.vault_audit(&actor, "vault.2fa.setup", None, "")
            .await?;
        Ok(Revealed::Values(TwoFactorSetup {
            uri: otpauth_uri(ISSUER, &actor, &secret),
            secret: secret.to_string(),
        }))
    }

    /// `POST /api/auth/unlock`: a current code of the caller's authenticator unlocks the vault
    /// for this browser session for 10 minutes; the first one also confirms the setup.
    ///
    /// # Errors
    /// `Forbidden` for API tokens, wrong or reused codes (audited) and while locked after
    /// 10 wrong codes in a row, `Invalid` without two-factor, `Capacity` after 5
    /// attempts a minute, store failures.
    pub async fn unlock_vault(&self, ctx: &AuthContext, code: &str) -> AppResult<ReauthStatus> {
        let session = browser_session(ctx)?;
        self.limit_attempts(&format!("unlock:{}", ctx.user.id.as_str()))?;
        let user_id = ctx.user.id.as_str();
        let Some(record) = self.deps.store.vault_two_factor(user_id).await? else {
            return Err(AppError::Invalid("set up two-factor first".into()));
        };
        if record.failed_attempts >= MAX_FAILED_CODES {
            if locked(&record, self.authenticated_at(ctx).await?) {
                return Err(AppError::Forbidden(
                    "too many wrong codes: confirm it is you (password or GitHub sign-in), then try again"
                        .into(),
                ));
            }
            self.deps
                .store
                .vault_two_factor_clear_failures(user_id)
                .await?;
        }
        let seed = self
            .deps
            .crypto
            .envelope_open(&aad(user_id), &record.envelope)?;
        let seed =
            std::str::from_utf8(&seed).map_err(|_| AppError::Internal("two-factor seed".into()))?;
        let mut config = parse_otp(seed).map_err(super::invalid)?;
        let now = self.now();
        let current = counter(now.0, config.period);
        let code = code.trim();
        let mut matched = None;
        for step in -DRIFT_STEPS..=DRIFT_STEPS {
            let Some(at) = current.checked_add_signed(step) else {
                continue;
            };
            let hmac = self
                .deps
                .crypto
                .totp_hmac(config.algorithm, &config.secret, at);
            let expected = code_from_hmac(&hmac, config.digits);
            if self
                .deps
                .crypto
                .constant_time_eq(expected.as_bytes(), code.as_bytes())
            {
                matched = Some(at);
            }
        }
        config.secret.zeroize();
        let actor = Self::vault_actor(ctx);
        let accepted = match matched {
            Some(at) => {
                self.deps
                    .store
                    .vault_accept_two_factor(user_id, &record.envelope.ciphertext, at, now)
                    .await?
            }
            None => false,
        };
        if !accepted {
            self.vault_audit(&actor, "vault.unlock.failed", None, "")
                .await?;
            if matched.is_none()
                && self
                    .deps
                    .store
                    .vault_two_factor_failed(user_id, now)
                    .await?
                    == MAX_FAILED_CODES
            {
                self.vault_audit(&actor, "vault.2fa.locked", None, "")
                    .await?;
            }
            let message = match matched {
                None => "wrong code",
                // The checked seed was replaced by a new setup meanwhile (#295).
                Some(_) if self.seed_replaced(user_id, &record).await? => {
                    "two-factor was set up again meanwhile: enter a code for the new key"
                }
                Some(_) => "this code was used already: wait for the next one",
            };
            return Err(AppError::Forbidden(message.into()));
        }
        self.deps.store.vault_set_unlock(session, now).await?;
        let action = if record.confirmed_at.is_none() {
            "vault.2fa.confirm"
        } else {
            "vault.unlock"
        };
        self.vault_audit(&actor, action, None, "").await?;
        self.reauth_status(ctx).await
    }
}
