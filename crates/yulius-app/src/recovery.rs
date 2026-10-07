//! Superadmin password recovery code (SPEC-001 ACC-002/011, SPEC-002 ADR-13).
//!
//! Contract:
//! - the code is 6 uniformly random digits, shown exactly once (setup, after a recovery, after
//!   a regeneration) and stored only as an Argon2id hash on the instance;
//! - `recover` checks, verifies and counts under one process-wide lock, so parallel guesses
//!   cannot exceed [`MAX_FAILURES`]; the failure is counted in the database before the answer;
//! - [`MAX_FAILURES`] wrong codes erase the code and lock recovery permanently; only a signed-in
//!   superadmin who re-enters the password can issue a new code, which clears the lock;
//! - a successful recovery sets the new password, revokes every superadmin session and token,
//!   and returns a new code (the used one is gone).

use crate::auth::{AuthContext, MIN_PASSWORD_LEN};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::Secret;

/// Wrong codes accepted before recovery is disabled for good.
pub const MAX_FAILURES: u32 = 3;
/// Digits of the code.
const DIGITS: u32 = 6;

/// Recovery availability (public).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryState {
    /// A code exists and recovery is not locked.
    pub available: bool,
    /// Recovery is permanently disabled.
    pub locked: bool,
    /// Wrong codes still accepted.
    pub attempts_left: u32,
}

/// Uniform 6-digit code from 32 random bytes (rejection sampling over u32).
fn code_from(random: &[u8; 32]) -> Option<String> {
    let modulus = 10_u32.pow(DIGITS);
    let limit = u32::MAX - (u32::MAX % modulus);
    random.as_chunks::<4>().0.iter().find_map(|chunk| {
        let value = u32::from_le_bytes(*chunk);
        (value < limit).then(|| format!("{:06}", value % modulus))
    })
}

impl App {
    fn new_recovery_code(&self) -> AppResult<String> {
        // Eight candidates per draw; a miss is astronomically unlikely, so redraw a few times.
        for _ in 0..4 {
            if let Some(code) = code_from(&self.deps.crypto.random_bytes()) {
                return Ok(code);
            }
        }
        Err(AppError::internal("could not draw a recovery code"))
    }

    /// Issues and stores a new recovery code (clears failures and the lock).
    pub(crate) async fn issue_recovery_code(&self) -> AppResult<Secret> {
        let code = self.new_recovery_code()?;
        let hash = self.deps.crypto.hash_password(&Secret(code.clone()))?;
        self.deps.store.set_recovery_code(&hash).await?;
        Ok(Secret(code))
    }

    /// Whether recovery is possible.
    ///
    /// # Errors
    /// Store failures.
    pub async fn recovery_state(&self) -> AppResult<RecoveryState> {
        let instance = self.deps.store.instance().await?;
        let (hash, failures, locked) = instance.map_or((None, 0, false), |i| {
            (
                i.recovery_hash,
                i.recovery_failures,
                i.recovery_locked_at.is_some(),
            )
        });
        Ok(RecoveryState {
            available: hash.is_some() && !locked,
            locked,
            attempts_left: if locked {
                0
            } else {
                MAX_FAILURES.saturating_sub(failures)
            },
        })
    }

    /// Resets the superadmin password with the recovery code and returns a new code.
    ///
    /// # Errors
    /// `Forbidden` when recovery is unavailable or locked (also after this attempt's failure
    /// locked it); `Invalid` for a wrong code (with the attempts left) or an unusable new
    /// password.
    pub async fn recover_password(
        &self,
        code: &str,
        password: &Secret,
        confirm: &Secret,
    ) -> AppResult<Secret> {
        if password.expose() != confirm.expose() {
            return Err(AppError::Invalid("passwords do not match".into()));
        }
        if password.expose().chars().count() < MIN_PASSWORD_LEN {
            return Err(AppError::Invalid(format!(
                "use at least {MIN_PASSWORD_LEN} characters"
            )));
        }
        let _guard = self.recovery_lock.lock().await;
        let now = self.now();
        let instance = self
            .deps
            .store
            .instance()
            .await?
            .ok_or_else(|| AppError::Forbidden("recovery is not available".into()))?;
        if instance.recovery_locked_at.is_some() {
            return Err(AppError::Forbidden(
                "password recovery is permanently disabled after too many wrong codes".into(),
            ));
        }
        let hash = instance.recovery_hash.ok_or_else(|| {
            AppError::Forbidden("no recovery code is set for this instance".into())
        })?;
        let given = code.trim();
        let well_formed = given.len() == 6 && given.bytes().all(|b| b.is_ascii_digit());
        if !well_formed
            || !self
                .deps
                .crypto
                .verify_password(&Secret(given.to_owned()), &hash)
        {
            let (failures, locked) = self
                .deps
                .store
                .record_recovery_failure(now, MAX_FAILURES)
                .await?;
            let _ = self
                .deps
                .store
                .audit(
                    now,
                    "superadmin",
                    "recovery.failed",
                    None,
                    Some(&format!("failures={failures}")),
                )
                .await;
            return Err(if locked {
                AppError::Forbidden(
                    "wrong recovery code; password recovery is now permanently disabled".into(),
                )
            } else {
                let left = MAX_FAILURES.saturating_sub(failures);
                AppError::Invalid(format!(
                    "wrong recovery code; {left} attempt{} left before recovery is disabled for good",
                    if left == 1 { "" } else { "s" }
                ))
            });
        }
        let admin = self
            .deps
            .store
            .superadmin()
            .await?
            .ok_or_else(|| AppError::Forbidden("no superadmin exists".into()))?;
        let phc = self.deps.crypto.hash_password(password)?;
        self.deps.store.set_password_hash(&admin.id, &phc).await?;
        self.deps
            .store
            .revoke_user_credentials(&admin.id, now)
            .await?;
        let fresh = self.issue_recovery_code().await?;
        let _ = self
            .deps
            .store
            .audit(now, "superadmin", "recovery.completed", None, None)
            .await;
        Ok(fresh)
    }

    /// A signed-in superadmin issues a new code after re-entering the password; this also
    /// re-enables a locked recovery.
    ///
    /// # Errors
    /// `Forbidden` for other users and for a wrong password.
    pub async fn regenerate_recovery_code(
        &self,
        ctx: &AuthContext,
        password: &Secret,
    ) -> AppResult<Secret> {
        self.require_superadmin(ctx)?;
        let _guard = self.recovery_lock.lock().await;
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
        if !ok {
            return Err(AppError::Forbidden("the password is not correct".into()));
        }
        let code = self.issue_recovery_code().await?;
        let _ = self
            .deps
            .store
            .audit(self.now(), "superadmin", "recovery.regenerated", None, None)
            .await;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits_and_reject_biased_values() {
        let mut random = [0_u8; 32];
        random[..4].copy_from_slice(&123_456_789_u32.to_le_bytes());
        assert_eq!(code_from(&random).as_deref(), Some("456789"));
        // Values above the largest multiple of 10^6 are skipped (no modulo bias).
        let mut biased = [0_u8; 32];
        biased[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        biased[4..8].copy_from_slice(&7_u32.to_le_bytes());
        assert_eq!(code_from(&biased).as_deref(), Some("000007"));
        assert_eq!(code_from(&[0xff; 32]), None);
    }
}
