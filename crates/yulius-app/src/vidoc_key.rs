//! The instance Vidoc key (SPEC-195 VDK-001, VDK-002): optional, offered at setup, and set,
//! replaced, removed, adopted and tested by a superadmin later, like the Jev key (`jev_key.rs`).
//!
//! A saved key is sealed with the master key and wins over `YULIUS_VIDOC_API_KEY_FILE`, which is
//! honoured while nothing is saved ("from config"). The key in use changes at once. It never
//! leaves the server in clear: agents and CI jobs get the placeholder of the virtual agent secret
//! `VIDOC_API_KEY`, which the egress proxy replaces toward [`VIDOC_HOST`] only (VDK-005,
//! `vault/runtime.rs`). Without a key Vidoc is simply off.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use yulius_contracts::api::updates::{VidocKeyView, VidocTestResult};
use yulius_domain::briefing::VidocBriefing;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{Sealed, Secret};
use crate::ports::vidoc::{VidocCheck, VidocConnector};

/// Vidoc's Messages API host: the only host the instance key is sent to.
pub const VIDOC_HOST: &str = "model.vidocsecurity.com";
/// The agent secret name the instance key stands in for (a sandbox's own secret wins).
pub const VIDOC_SECRET: &str = "VIDOC_API_KEY";
/// The audit target of its uses (`vault.agent.use`).
pub const VIDOC_SECRET_ID: &str = "instance:vidoc";

/// The saved key as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVidocKey {
    /// The key, sealed with the master key.
    pub sealed: Sealed,
    /// When it was set.
    pub set_at: Timestamp,
    /// Who set it (login).
    pub set_by: Option<String>,
}

/// The instance Vidoc switch while off (SPEC-356 VIO-001): when and by whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VidocOff {
    /// When it was switched off.
    pub at: Timestamp,
    /// Who switched it off (login).
    pub by: Option<String>,
}

/// Where the key in use comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VidocSource {
    Settings,
    Config,
}

impl VidocSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Config => "config",
        }
    }
}

/// The Vidoc services an instance runs with.
pub struct VidocServices {
    connector: Arc<dyn VidocConnector>,
    /// The key of `YULIUS_VIDOC_API_KEY_FILE`, or why it is not usable.
    config_key: Result<Option<Secret>, String>,
    active: RwLock<Option<(Secret, VidocSource)>>,
    /// The instance switch, while off (SPEC-356): read at start, changed by a superadmin.
    off: RwLock<Option<VidocOff>>,
    /// Serializes switch changes, so the stored and the cached switch agree.
    switching: tokio::sync::Mutex<()>,
}

impl VidocServices {
    /// Services testing keys with `connector`; `config_key` is used while nothing is saved.
    #[must_use]
    pub fn new(
        connector: Arc<dyn VidocConnector>,
        config_key: Result<Option<Secret>, String>,
    ) -> Self {
        Self {
            connector,
            config_key,
            active: RwLock::new(None),
            off: RwLock::new(None),
            switching: tokio::sync::Mutex::new(()),
        }
    }
}

/// Bound of one test request (the connector's own deadline is shorter).
const TEST_DEADLINE: Duration = Duration::from_secs(12);

/// A key the API accepts: 8–4096 printable ASCII characters without spaces.
fn valid_key(key: &str) -> Result<Secret, AppError> {
    let key = key.trim();
    if (8..=4096).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_graphic()) {
        Ok(Secret(key.to_owned()))
    } else {
        Err(AppError::Invalid(
            "the Vidoc key must be 8-4096 printable characters without spaces".into(),
        ))
    }
}

impl App {
    /// Installs the Vidoc services (once). Returns whether they were set.
    pub fn set_vidoc(&self, services: VidocServices) -> bool {
        self.vidoc.set(services).is_ok()
    }

    fn vidoc_services(&self) -> AppResult<&VidocServices> {
        self.vidoc
            .get()
            .ok_or_else(|| AppError::Conflict("Vidoc is not available on this server".into()))
    }

    fn set_active_vidoc(&self, active: Option<(Secret, VidocSource)>) {
        if let Some(services) = self.vidoc.get()
            && let Ok(mut slot) = services.active.write()
        {
            *slot = active;
        }
    }

    fn vidoc_source(&self) -> Option<VidocSource> {
        let services = self.vidoc.get()?;
        let slot = services.active.read().ok()?;
        slot.as_ref().map(|(_, source)| *source)
    }

    /// Whether Vidoc scans are on for the instance (SPEC-356 VIO-001): no superadmin switched
    /// them off. Without Vidoc services there is nothing to switch, and it is on.
    #[must_use]
    pub fn vidoc_enabled(&self) -> bool {
        self.vidoc.get().is_none_or(|services| {
            services.off.read().map(|off| off.is_none()).unwrap_or(true)
        })
    }

    /// Whether `sandbox` scans with Vidoc: the instance switch (SPEC-356 VIO-002) and its own
    /// (SPEC-338 VOF-002) are both on. Its own switch alone is what its view shows.
    #[must_use]
    pub fn vidoc_scans_on(&self, sandbox: &crate::model::SandboxRecord) -> bool {
        self.vidoc_enabled() && crate::delivery::vidoc_switched_on(sandbox)
    }

    /// What `sandbox`'s agents are told about the Vidoc scan: off when a superadmin switched
    /// Vidoc off for the instance (SPEC-356 VIO-003) or a person switched its scans off (SPEC-338
    /// VOF-003), whatever key it has; else on with the instance key.
    #[must_use]
    pub fn vidoc_briefing(&self, sandbox: &crate::model::SandboxRecord) -> VidocBriefing {
        if !self.vidoc_enabled() {
            VidocBriefing::OffForInstance
        } else if !crate::delivery::vidoc_switched_on(sandbox) {
            VidocBriefing::Off
        } else if self.vidoc_key().is_some() {
            VidocBriefing::On
        } else {
            VidocBriefing::Absent
        }
    }

    /// The instance key in use (VDK-005: for the egress proxy only), if any.
    #[must_use]
    pub fn vidoc_key(&self) -> Option<Secret> {
        let services = self.vidoc.get()?;
        let slot = services.active.read().ok()?;
        slot.as_ref().map(|(key, _)| key.clone())
    }

    fn config_vidoc(&self) -> Option<(Secret, VidocSource)> {
        let key = self.vidoc.get()?.config_key.as_ref().ok()?.as_ref()?;
        Some((key.clone(), VidocSource::Config))
    }

    /// Picks the key in use at start: the saved key, else `YULIUS_VIDOC_API_KEY_FILE`, else none.
    pub async fn load_vidoc_key(&self) {
        if self.vidoc.get().is_none() {
            return;
        }
        let saved = match self.deps.store.vidoc_key().await {
            Ok(saved) => saved,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the saved Vidoc key");
                None
            }
        };
        let from_settings = saved.and_then(|stored| {
            let opened = self
                .deps
                .crypto
                .open(&stored.sealed)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok());
            if opened.is_none() {
                tracing::warn!("the saved Vidoc key cannot be opened with this master key");
            }
            opened.map(|key| (Secret(key), VidocSource::Settings))
        });
        let active = from_settings.or_else(|| self.config_vidoc());
        let off = match self.deps.store.vidoc_off().await {
            Ok(off) => off,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the instance Vidoc switch");
                None
            }
        };
        if off.is_some() {
            tracing::info!("Vidoc scans are switched off for the instance");
        }
        self.set_vidoc_off_cache(off);
        match active.as_ref().map(|(_, source)| *source) {
            Some(VidocSource::Settings) => {
                tracing::info!("Vidoc is on (instance key saved in settings)");
            }
            Some(VidocSource::Config) => {
                tracing::info!("Vidoc is on (instance key from YULIUS_VIDOC_API_KEY_FILE)");
            }
            None => tracing::info!("Vidoc is off: no instance key"),
        }
        self.set_active_vidoc(active);
    }

    fn set_vidoc_off_cache(&self, off: Option<VidocOff>) {
        if let Some(services) = self.vidoc.get()
            && let Ok(mut slot) = services.off.write()
        {
            *slot = off;
        }
    }

    fn vidoc_off_now(&self) -> Option<VidocOff> {
        self.vidoc.get()?.off.read().ok()?.clone()
    }

    /// `PUT /api/admin/vidoc/enabled` (superadmin, SPEC-356 VIO-001): switches Vidoc scans off
    /// for every project of the instance, keeping the key, or on again; at once and stored.
    ///
    /// # Errors
    /// `Forbidden` for others, `Conflict` without Vidoc services; store failures.
    pub async fn set_vidoc_enabled(
        &self,
        ctx: &AuthContext,
        enabled: bool,
    ) -> AppResult<VidocKeyView> {
        self.require_superadmin(ctx)?;
        let services = self.vidoc_services()?;
        let _switching = services.switching.lock().await;
        if self.vidoc_enabled() != enabled {
            let login = self.login_of(&ctx.user.id).await;
            let now = self.now();
            let off = (!enabled).then(|| VidocOff {
                at: now,
                by: Some(login.clone()),
            });
            self.deps
                .store
                .set_vidoc_off((!enabled).then_some(login.as_str()), now)
                .await?;
            self.set_vidoc_off_cache(off);
            // The virtual key follows the switch from the next request on.
            self.bump_vault_generation();
            let action = if enabled { "vidoc.enabled" } else { "vidoc.disabled" };
            self.deps
                .store
                .audit(now, &login, action, Some(VIDOC_SECRET_ID), None)
                .await?;
            tracing::info!(enabled, "the instance Vidoc switch changed");
        }
        self.vidoc_key_view(ctx).await
    }

    /// `GET /api/admin/vidoc` (superadmin): the key's state, never the key.
    ///
    /// # Errors
    /// `Forbidden` for others; store failures.
    pub async fn vidoc_key_view(&self, ctx: &AuthContext) -> AppResult<VidocKeyView> {
        self.require_superadmin(ctx)?;
        let saved = self.deps.store.vidoc_key().await?;
        let (config_present, config_error) = match self.vidoc.get().map(|s| &s.config_key) {
            Some(Ok(Some(_))) => (true, None),
            Some(Err(err)) => (false, Some(err.clone())),
            _ => (false, None),
        };
        Ok(VidocKeyView {
            configured: self.vidoc_key().is_some(),
            source: self.vidoc_source().map(|s| s.as_str().to_owned()),
            set_at: saved.as_ref().map(|s| rfc3339(s.set_at)),
            set_by: saved.and_then(|s| s.set_by),
            config_present,
            config_error,
            enabled: self.vidoc_enabled(),
            disabled_at: self.vidoc_off_now().map(|off| rfc3339(off.at)),
            disabled_by: self.vidoc_off_now().and_then(|off| off.by),
        })
    }

    /// `PUT /api/admin/vidoc` (superadmin): saves `key` sealed and uses it at once.
    ///
    /// # Errors
    /// `Forbidden` for others, `Invalid` for a malformed key, `Conflict` without Vidoc services.
    pub async fn set_vidoc_key(&self, ctx: &AuthContext, key: &Secret) -> AppResult<VidocKeyView> {
        self.require_superadmin(ctx)?;
        self.vidoc_services()?;
        let key = valid_key(key.expose())?;
        let sealed = self.deps.crypto.seal(key.expose().as_bytes())?;
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        self.deps
            .store
            .set_vidoc_key(Some((&sealed, &login)), now)
            .await?;
        self.set_active_vidoc(Some((key, VidocSource::Settings)));
        // Redaction knows the new key before any request can carry it.
        self.bump_vault_generation();
        self.deps
            .store
            .audit(now, &login, "vidoc.key_set", Some(VIDOC_SECRET_ID), None)
            .await?;
        tracing::info!("Vidoc is on (instance key saved in settings)");
        self.vidoc_key_view(ctx).await
    }

    /// `POST /api/admin/vidoc/adopt` (superadmin): saves the key of `YULIUS_VIDOC_API_KEY_FILE`
    /// in settings, so the file can be removed from the service.
    ///
    /// # Errors
    /// `Conflict` when no usable key file is configured.
    pub async fn adopt_config_vidoc_key(&self, ctx: &AuthContext) -> AppResult<VidocKeyView> {
        self.require_superadmin(ctx)?;
        let key = match &self.vidoc_services()?.config_key {
            Ok(Some(key)) => key.clone(),
            _ => {
                return Err(AppError::Conflict(
                    "YULIUS_VIDOC_API_KEY_FILE holds no usable key".into(),
                ));
            }
        };
        self.set_vidoc_key(ctx, &key).await
    }

    /// `DELETE /api/admin/vidoc` (superadmin): removes the saved key; the key of
    /// `YULIUS_VIDOC_API_KEY_FILE`, if any, is used again, else Vidoc is off.
    ///
    /// # Errors
    /// `Forbidden` for others; `Conflict` without Vidoc services; store failures.
    pub async fn remove_vidoc_key(&self, ctx: &AuthContext) -> AppResult<VidocKeyView> {
        self.require_superadmin(ctx)?;
        self.vidoc_services()?;
        let now = self.now();
        let saved = self.deps.store.vidoc_key().await?.is_some();
        self.deps.store.set_vidoc_key(None, now).await?;
        self.set_active_vidoc(self.config_vidoc());
        self.bump_vault_generation();
        if saved {
            let login = self.login_of(&ctx.user.id).await;
            self.deps
                .store
                .audit(
                    now,
                    &login,
                    "vidoc.key_removed",
                    Some(VIDOC_SECRET_ID),
                    None,
                )
                .await?;
        }
        self.vidoc_key_view(ctx).await
    }

    /// `POST /api/admin/vidoc/test` (superadmin): one small request with the key in use.
    ///
    /// # Errors
    /// `Forbidden` for others, `Conflict` without Vidoc services.
    pub async fn test_vidoc_key(&self, ctx: &AuthContext) -> AppResult<VidocTestResult> {
        self.require_superadmin(ctx)?;
        let connector = self.vidoc_services()?.connector.clone();
        let Some(key) = self.vidoc_key() else {
            return Ok(VidocTestResult {
                ok: false,
                result: "none".into(),
                message: "No Vidoc key is set.".into(),
                latency_ms: None,
            });
        };
        let started = Instant::now();
        let check = tokio::time::timeout(TEST_DEADLINE, connector.check(&key))
            .await
            .unwrap_or(VidocCheck::Timeout);
        let latency_ms = u64::try_from(started.elapsed().as_millis()).ok();
        let (result, message) = match check {
            VidocCheck::Ok => (
                "ok",
                "Vidoc answered: the key works and vidoc-1 is available.",
            ),
            VidocCheck::Refused => (
                "refused",
                "Vidoc refused this key. Check it and save it again.",
            ),
            VidocCheck::Unreachable => (
                "unreachable",
                "Vidoc could not be reached or answered with an error. Try again later.",
            ),
            VidocCheck::Timeout => ("timeout", "Vidoc did not answer in time. Try again later."),
            VidocCheck::Invalid => (
                "invalid",
                "Vidoc answered, but does not list the vidoc-1 model for this key.",
            ),
        };
        Ok(VidocTestResult {
            ok: check == VidocCheck::Ok,
            result: result.into(),
            message: message.into(),
            latency_ms,
        })
    }
}
