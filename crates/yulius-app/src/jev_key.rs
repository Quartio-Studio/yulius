//! The Jev API key (SPEC-001 ADM-001, ADM-002; SPEC-002 ADR-16, ADR-18 §2.5): asked for at setup
//! (optional), and set, replaced, removed and tested by a superadmin later.
//!
//! A saved key is sealed with the instance master key like every other secret and wins over
//! `YULIUS_JEV_API_KEY_FILE`, which is still honoured while nothing is saved ("from config").
//! The key in use changes at once, without a restart. The key never leaves the server: it is not
//! logged, audited or returned by any API.

use std::sync::Arc;
use std::time::{Duration, Instant};

use yulius_contracts::api::updates::{JevKeyView, JevTestResult};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{Sealed, Secret};
use crate::ports::intake::{ChoiceEvaluator, JevCheck, JevConnector};

/// The saved key as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJevKey {
    /// The key, sealed with the master key.
    pub sealed: Sealed,
    /// When it was set.
    pub set_at: Timestamp,
    /// Who set it (login).
    pub set_by: Option<String>,
}

/// Where the key in use comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JevSource {
    /// Saved by a superadmin (setup or Settings).
    Settings,
    /// `YULIUS_JEV_API_KEY_FILE`.
    Config,
}

impl JevSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Config => "config",
        }
    }
}

/// The evaluator task intake uses now, with its key (for a test request).
pub struct ActiveJev {
    pub(crate) evaluator: Arc<dyn ChoiceEvaluator>,
    key: Option<Secret>,
    source: Option<JevSource>,
}

impl ActiveJev {
    /// An evaluator given at construction (tests, or a client without key management).
    pub(crate) fn fixed(evaluator: Arc<dyn ChoiceEvaluator>) -> Self {
        Self {
            evaluator,
            key: None,
            source: None,
        }
    }
}

/// Bound of one test request (the client's own deadline is shorter).
const TEST_DEADLINE: Duration = Duration::from_secs(12);

impl App {
    fn key_services(&self) -> AppResult<&crate::intake::IntakeServices> {
        self.intake
            .get()
            .filter(|s| s.connector.is_some())
            .ok_or_else(|| AppError::Conflict("task intake is not available on this server".into()))
    }

    fn connector(&self) -> AppResult<Arc<dyn JevConnector>> {
        self.key_services()?
            .connector
            .clone()
            .ok_or_else(|| AppError::Conflict("task intake is not available on this server".into()))
    }

    fn set_active_jev(&self, active: Option<ActiveJev>) {
        if let Some(services) = self.intake.get()
            && let Ok(mut slot) = services.jev.write()
        {
            *slot = active;
        }
    }

    fn active_source(&self) -> Option<JevSource> {
        let services = self.intake.get()?;
        let slot = services.jev.read().ok()?;
        slot.as_ref().and_then(|a| a.source)
    }

    /// The configured key as the evaluator in use, if it is usable.
    fn config_jev(&self) -> Option<ActiveJev> {
        let services = self.intake.get()?;
        let connector = services.connector.as_ref()?;
        let key = services.config_key.as_ref().ok()?.as_ref()?;
        match connector.connect(key) {
            Ok(evaluator) => Some(ActiveJev {
                evaluator,
                key: Some(key.clone()),
                source: Some(JevSource::Config),
            }),
            Err(err) => {
                tracing::warn!("the Jev key of YULIUS_JEV_API_KEY_FILE is not usable: {err}");
                None
            }
        }
    }

    /// Picks the key in use at start: the saved key, else `YULIUS_JEV_API_KEY_FILE`, else none
    /// (task intake checks nothing). Without key management nothing changes.
    pub async fn load_jev_key(&self) {
        let Ok(connector) = self.connector() else {
            return;
        };
        let saved = match self.deps.store.jev_key().await {
            Ok(saved) => saved,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the saved Jev key");
                None
            }
        };
        let from_settings = saved.and_then(|stored| {
            let opened = self
                .deps
                .crypto
                .open(&stored.sealed)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(Secret);
            let Some(key) = opened else {
                tracing::warn!("the saved Jev key cannot be opened with this master key");
                return None;
            };
            match connector.connect(&key) {
                Ok(evaluator) => Some(ActiveJev {
                    evaluator,
                    key: Some(key),
                    source: Some(JevSource::Settings),
                }),
                Err(err) => {
                    tracing::warn!("the saved Jev key is not usable: {err}");
                    None
                }
            }
        });
        let active = from_settings.or_else(|| self.config_jev());
        match active.as_ref().and_then(|a| a.source) {
            Some(JevSource::Settings) => {
                tracing::info!("task intake is on (Jev jev-1.13.0, key saved in settings)");
            }
            Some(JevSource::Config) => {
                tracing::info!(
                    "task intake is on (Jev jev-1.13.0, key from YULIUS_JEV_API_KEY_FILE)"
                );
            }
            None => tracing::info!("task intake is off: no Jev key"),
        }
        self.set_active_jev(active);
    }

    /// `GET /api/admin/jev` (superadmin): the key's state, never the key.
    ///
    /// # Errors
    /// `Forbidden` for others; store failures.
    pub async fn jev_key_view(&self, ctx: &AuthContext) -> AppResult<JevKeyView> {
        self.require_superadmin(ctx)?;
        let saved = self.deps.store.jev_key().await?;
        let (config_present, config_error) = match self.intake.get().map(|s| &s.config_key) {
            Some(Ok(Some(_))) => (true, None),
            Some(Err(err)) => (false, Some(err.clone())),
            _ => (false, None),
        };
        let source = self.active_source();
        Ok(JevKeyView {
            configured: self.jev().is_some(),
            source: source.map(|s| s.as_str().to_owned()),
            set_at: saved.as_ref().map(|s| rfc3339(s.set_at)),
            set_by: saved.and_then(|s| s.set_by),
            config_present,
            config_error,
        })
    }

    /// `PUT /api/admin/jev` (superadmin): saves `key` sealed and uses it at once.
    ///
    /// # Errors
    /// `Forbidden` for others, `Invalid` for a malformed key, `Conflict` without key
    /// management.
    pub async fn set_jev_key(&self, ctx: &AuthContext, key: &Secret) -> AppResult<JevKeyView> {
        self.require_superadmin(ctx)?;
        let connector = self.connector()?;
        let key = Secret(key.expose().trim().to_owned());
        let evaluator = connector.connect(&key).map_err(AppError::Invalid)?;
        let sealed = self.deps.crypto.seal(key.expose().as_bytes())?;
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        self.deps
            .store
            .set_jev_key(Some((&sealed, &login)), now)
            .await?;
        self.set_active_jev(Some(ActiveJev {
            evaluator,
            key: Some(key),
            source: Some(JevSource::Settings),
        }));
        self.deps
            .store
            .audit(now, &login, "jev.key_set", None, None)
            .await?;
        tracing::info!("task intake is on (Jev key saved in settings)");
        self.jev_key_view(ctx).await
    }

    /// `POST /api/admin/jev/adopt` (superadmin): saves the key of `YULIUS_JEV_API_KEY_FILE` in
    /// settings, so the file can be removed from the service.
    ///
    /// # Errors
    /// `Conflict` when no usable key file is configured.
    pub async fn adopt_config_jev_key(&self, ctx: &AuthContext) -> AppResult<JevKeyView> {
        self.require_superadmin(ctx)?;
        let key = match &self.key_services()?.config_key {
            Ok(Some(key)) => key.clone(),
            _ => {
                return Err(AppError::Conflict(
                    "YULIUS_JEV_API_KEY_FILE holds no usable key".into(),
                ));
            }
        };
        self.set_jev_key(ctx, &key).await
    }

    /// `DELETE /api/admin/jev` (superadmin): removes the saved key; the key of
    /// `YULIUS_JEV_API_KEY_FILE`, if any, is used again, else task intake checks nothing.
    ///
    /// # Errors
    /// `Forbidden` for others; store failures.
    pub async fn remove_jev_key(&self, ctx: &AuthContext) -> AppResult<JevKeyView> {
        self.require_superadmin(ctx)?;
        let _ = self.connector()?;
        let now = self.now();
        self.deps.store.set_jev_key(None, now).await?;
        self.set_active_jev(self.config_jev());
        let login = self.login_of(&ctx.user.id).await;
        self.deps
            .store
            .audit(now, &login, "jev.key_removed", None, None)
            .await?;
        self.jev_key_view(ctx).await
    }

    /// `POST /api/admin/jev/test` (superadmin): one small request with the key in use.
    ///
    /// # Errors
    /// `Forbidden` for others, `Conflict` without key management.
    pub async fn test_jev_key(&self, ctx: &AuthContext) -> AppResult<JevTestResult> {
        self.require_superadmin(ctx)?;
        let connector = self.connector()?;
        let key = self
            .intake
            .get()
            .and_then(|s| s.jev.read().ok()?.as_ref()?.key.clone());
        let Some(key) = key else {
            return Ok(JevTestResult {
                ok: false,
                result: "none".into(),
                message: "No Jev key is set.".into(),
                latency_ms: None,
            });
        };
        let started = Instant::now();
        let check = tokio::time::timeout(TEST_DEADLINE, connector.check(&key))
            .await
            .unwrap_or(JevCheck::Timeout);
        let latency_ms = u64::try_from(started.elapsed().as_millis()).ok();
        let (result, message) = match check {
            JevCheck::Ok => ("ok", "Jev answered: the key works."),
            JevCheck::Refused => (
                "refused",
                "Jev refused this key. Check it and save it again.",
            ),
            JevCheck::Unreachable => (
                "unreachable",
                "Jev could not be reached or answered with an error. Try again later.",
            ),
            JevCheck::Timeout => ("timeout", "Jev did not answer in time. Try again later."),
            JevCheck::Invalid => (
                "invalid",
                "Jev answered, but not in the expected form. Task intake treats that as no answer.",
            ),
        };
        Ok(JevTestResult {
            ok: check == JevCheck::Ok,
            result: result.into(),
            message: message.into(),
            latency_ms,
        })
    }
}
