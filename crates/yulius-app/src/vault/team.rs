//! Team secrets (VLT-001..003): flat access for every member, reveals behind a fresh
//! re-authentication, everything audited.

use yulius_contracts::api::secrets::{
    ReauthStatus, RevealRequest, TeamSecretInput, TeamSecretPatch, TeamSecretReveal,
    TeamSecretView, TotpCode, VaultActivity,
};
use yulius_domain::time::Timestamp;
use yulius_domain::vault::totp::{code_from_hmac, counter, parse_otp, remaining_seconds};
use yulius_domain::vault::{REAUTH_WINDOW_MS, ROTATE_AFTER_MS, reauth_fresh};

use super::{TeamPayload, field, invalid};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::Secret;
use crate::ports::vault::{VaultSecretRecord, VaultSection};

/// Longest title.
const MAX_TITLE: usize = 200;
/// Longest URL.
const MAX_URL: usize = 2048;
/// Re-authentication attempts per user and minute.
const REAUTH_ATTEMPTS_PER_MINUTE: usize = 5;

/// A reveal either succeeds or needs the person to confirm who they are first.
#[derive(Debug)]
pub enum Revealed<T> {
    /// The values.
    Values(T),
    /// No fresh re-authentication: how to get one.
    ReauthRequired(ReauthStatus),
}

fn title(text: &str) -> AppResult<String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_TITLE || text.chars().any(char::is_control) {
        return Err(AppError::Invalid(format!(
            "a title of 1–{MAX_TITLE} characters is required"
        )));
    }
    Ok(text.to_owned())
}

fn url(text: Option<&str>) -> AppResult<Option<String>> {
    let value = field(text, "url", true)?;
    if let Some(value) = &value
        && (value.len() > MAX_URL || value.chars().any(char::is_control))
    {
        return Err(AppError::Invalid("invalid URL".into()));
    }
    Ok(value)
}

fn totp_seed(text: Option<&str>) -> AppResult<Option<String>> {
    let value = field(text, "TOTP seed", true)?;
    if let Some(seed) = &value {
        parse_otp(seed).map_err(invalid)?;
    }
    Ok(value)
}

impl App {
    async fn team_view(&self, record: &VaultSecretRecord) -> AppResult<TeamSecretView> {
        let payload: TeamPayload = self.open_payload(record)?;
        let now = self.now();
        Ok(TeamSecretView {
            id: record.id.clone(),
            title: record.name.clone(),
            url: payload.url.clone(),
            username: payload.username.clone(),
            has_password: payload.password.is_some(),
            has_notes: payload.notes.is_some(),
            has_totp: payload.totp.is_some(),
            created_by: self.login_of_id(&record.created_by).await,
            updated_by: self.login_of_id(&record.updated_by).await,
            updated_at: rfc3339(record.updated_at),
            value_updated_at: rfc3339(record.value_updated_at),
            rotate_due: payload.password.is_some()
                && now.millis_since(record.value_updated_at) >= ROTATE_AFTER_MS,
        })
    }

    pub(super) async fn login_of_id(&self, id: &str) -> String {
        match yulius_domain::ids::UserId::parse(id) {
            Ok(user) => self.login_of(&user).await,
            Err(_) => "unknown".into(),
        }
    }

    async fn team_record(&self, id: &str) -> AppResult<VaultSecretRecord> {
        self.deps
            .store
            .vault_get(id)
            .await?
            .filter(|r| r.section == VaultSection::Team)
            .ok_or(AppError::NotFound)
    }

    /// `GET /api/team-secrets`: every member sees every team secret (titles, addresses and
    /// user names; secret fields only flagged).
    ///
    /// # Errors
    /// Store or decryption failures.
    pub async fn team_secrets(&self, _ctx: &AuthContext) -> AppResult<Vec<TeamSecretView>> {
        let mut out = Vec::new();
        for record in self.deps.store.vault_team().await? {
            out.push(self.team_view(&record).await?);
        }
        Ok(out)
    }

    /// `POST /api/team-secrets`.
    ///
    /// # Errors
    /// `Invalid` input; store failures.
    pub async fn create_team_secret(
        &self,
        ctx: &AuthContext,
        input: &TeamSecretInput,
    ) -> AppResult<TeamSecretView> {
        let payload = TeamPayload {
            url: url(input.url.as_deref())?,
            username: field(input.username.as_deref(), "user name", true)?,
            password: field(input.password.as_deref(), "password", false)?,
            notes: field(input.notes.as_deref(), "notes", false)?,
            totp: totp_seed(input.totp.as_deref())?,
        };
        let id = self.deps.crypto.new_id();
        let now = self.now();
        let record = VaultSecretRecord {
            id: id.clone(),
            section: VaultSection::Team,
            sandbox_id: None,
            name: title(&input.title)?,
            description: String::new(),
            hosts: Vec::new(),
            injection: "none".into(),
            header_name: None,
            fields: payload.fields(),
            envelope: self.seal_payload(&id, VaultSection::Team, &payload)?,
            created_by: ctx.user.id.as_str().to_owned(),
            created_at: now,
            updated_by: ctx.user.id.as_str().to_owned(),
            updated_at: now,
            value_updated_at: now,
            last_used_at: None,
        };
        self.deps.store.vault_insert(&record).await?;
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.team.create",
            Some(&id),
            &record.name,
        )
        .await?;
        self.team_view(&record).await
    }

    /// `PATCH /api/team-secrets/{id}`: absent fields stay, empty ones are cleared. Changing a
    /// value needs no re-authentication (it reveals nothing).
    ///
    /// # Errors
    /// `NotFound`, `Invalid`, store failures.
    pub async fn update_team_secret(
        &self,
        ctx: &AuthContext,
        id: &str,
        patch: &TeamSecretPatch,
    ) -> AppResult<TeamSecretView> {
        let mut record = self.team_record(id).await?;
        let mut payload: TeamPayload = self.open_payload(&record)?;
        let mut changed: Vec<&str> = Vec::new();
        let mut value_changed = false;
        if let Some(text) = &patch.title {
            record.name = title(text)?;
            changed.push("title");
        }
        if patch.url.is_some() {
            payload.url = url(patch.url.as_deref())?;
            changed.push("url");
        }
        if patch.username.is_some() {
            payload.username = field(patch.username.as_deref(), "user name", true)?;
            changed.push("username");
        }
        if patch.password.is_some() {
            payload.password = field(patch.password.as_deref(), "password", false)?;
            changed.push("password");
            value_changed = true;
        }
        if patch.notes.is_some() {
            payload.notes = field(patch.notes.as_deref(), "notes", false)?;
            changed.push("notes");
            value_changed = true;
        }
        if patch.totp.is_some() {
            payload.totp = totp_seed(patch.totp.as_deref())?;
            changed.push("totp");
            value_changed = true;
        }
        let now = self.now();
        record.fields = payload.fields();
        record.envelope = self.seal_payload(&record.id, VaultSection::Team, &payload)?;
        record.updated_by = ctx.user.id.as_str().to_owned();
        record.updated_at = now;
        if value_changed {
            record.value_updated_at = now;
        }
        if !self.deps.store.vault_update(&record).await? {
            return Err(AppError::NotFound);
        }
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.team.update",
            Some(id),
            &format!("{} ({})", record.name, changed.join(", ")),
        )
        .await?;
        self.team_view(&record).await
    }

    /// `DELETE /api/team-secrets/{id}`.
    ///
    /// # Errors
    /// `NotFound`, store failures.
    pub async fn delete_team_secret(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let record = self.team_record(id).await?;
        if !self.deps.store.vault_delete(id).await? {
            return Err(AppError::NotFound);
        }
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.team.delete",
            Some(id),
            &record.name,
        )
        .await
    }

    /// When the caller's browser session last proved who they are: its sign-in or a
    /// re-authentication. API tokens never qualify.
    async fn authenticated_at(&self, ctx: &AuthContext) -> AppResult<Option<Timestamp>> {
        let Some(hash) = &ctx.session_hash else {
            return Ok(None);
        };
        let created = self
            .deps
            .store
            .web_session(hash)
            .await?
            .map(|s| s.created_at);
        let reauth = self.deps.store.vault_reauth(hash).await?;
        Ok(created.max(reauth))
    }

    /// `GET /api/auth/reauth`: whether reveals are allowed now and how to confirm.
    ///
    /// # Errors
    /// Store failures.
    pub async fn reauth_status(&self, ctx: &AuthContext) -> AppResult<ReauthStatus> {
        let mut methods = Vec::new();
        if ctx.session_hash.is_some() {
            if ctx.user.password_hash.is_some() {
                methods.push("password".to_owned());
            }
            if ctx.user.github_login.is_some() && self.github_app().await.is_ok() {
                methods.push("github".to_owned());
            }
        }
        let now = self.now();
        let at = self.authenticated_at(ctx).await?;
        let fresh = at.is_some_and(|at| reauth_fresh(now, at));
        Ok(ReauthStatus {
            fresh,
            fresh_until: at
                .filter(|_| fresh)
                .map(|at| rfc3339(at.plus_millis(REAUTH_WINDOW_MS))),
            methods,
        })
    }

    /// `POST /api/auth/reauth`: the superadmin confirms with the password (rate limited, audited).
    ///
    /// # Errors
    /// `Forbidden` without a browser session or a password and for a wrong password;
    /// `Capacity` when rate limited.
    pub async fn reauth_with_password(
        &self,
        ctx: &AuthContext,
        password: &Secret,
    ) -> AppResult<ReauthStatus> {
        let Some(hash) = &ctx.session_hash else {
            return Err(AppError::Forbidden(
                "confirming is done from a browser session".into(),
            ));
        };
        let Some(phc) = ctx.user.password_hash.as_deref() else {
            return Err(AppError::Forbidden(
                "this account has no password: sign in with GitHub again to confirm".into(),
            ));
        };
        let now = self.now();
        {
            let mut attempts = self
                .login_attempts
                .lock()
                .map_err(|_| AppError::Internal("rate limiter poisoned".into()))?;
            let entry = attempts
                .entry(format!("reauth:{}", ctx.user.id.as_str()))
                .or_default();
            entry.retain(|t| now.0 - *t < 60_000);
            if entry.len() >= REAUTH_ATTEMPTS_PER_MINUTE {
                return Err(AppError::Capacity(
                    "too many attempts; wait a minute".into(),
                ));
            }
            entry.push(now.0);
        }
        let actor = Self::vault_actor(ctx);
        if !self.deps.crypto.verify_password(password, phc) {
            self.vault_audit(&actor, "vault.reauth.failed", None, "password")
                .await?;
            // 403, not 401: the session stays signed in; only the confirmation failed.
            return Err(AppError::Forbidden("wrong password".into()));
        }
        self.deps.store.vault_set_reauth(hash, now).await?;
        self.vault_audit(&actor, "vault.reauth", None, "password")
            .await?;
        self.reauth_status(ctx).await
    }

    /// Checks the re-authentication window before a reveal.
    async fn fresh_or_status(&self, ctx: &AuthContext) -> AppResult<Option<ReauthStatus>> {
        if ctx.session_hash.is_none() {
            return Err(AppError::Forbidden(
                "secrets are revealed only in the web UI, after confirming it is you".into(),
            ));
        }
        let status = self.reauth_status(ctx).await?;
        Ok((!status.fresh).then_some(status))
    }

    /// `POST /api/team-secrets/{id}/reveal`: the requested secret fields, after a fresh
    /// re-authentication; audited as `vault.team.reveal` or `vault.team.copy`.
    ///
    /// # Errors
    /// `NotFound`, `Invalid` purpose or field, `Forbidden` for API tokens, store failures.
    pub async fn reveal_team_secret(
        &self,
        ctx: &AuthContext,
        id: &str,
        request: &RevealRequest,
    ) -> AppResult<Revealed<TeamSecretReveal>> {
        let purpose = request.purpose.as_deref().unwrap_or("show");
        if !matches!(purpose, "show" | "copy") {
            return Err(AppError::Invalid("purpose must be show or copy".into()));
        }
        let wanted: Vec<String> = request
            .fields
            .clone()
            .unwrap_or_else(|| vec!["password".into(), "notes".into(), "totp".into()]);
        if wanted.is_empty()
            || wanted
                .iter()
                .any(|f| !matches!(f.as_str(), "password" | "notes" | "totp"))
        {
            return Err(AppError::Invalid(
                "fields are password, notes and totp".into(),
            ));
        }
        let record = self.team_record(id).await?;
        if let Some(status) = self.fresh_or_status(ctx).await? {
            return Ok(Revealed::ReauthRequired(status));
        }
        let payload: TeamPayload = self.open_payload(&record)?;
        let pick = |name: &str, value: &Option<String>| {
            wanted
                .iter()
                .any(|w| w == name)
                .then(|| value.clone())
                .flatten()
        };
        let revealed = TeamSecretReveal {
            password: pick("password", &payload.password),
            notes: pick("notes", &payload.notes),
            totp: pick("totp", &payload.totp),
        };
        let shown: Vec<&str> = [
            ("password", revealed.password.is_some()),
            ("notes", revealed.notes.is_some()),
            ("totp", revealed.totp.is_some()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| name)
        .collect();
        self.vault_audit(
            &Self::vault_actor(ctx),
            if purpose == "copy" {
                "vault.team.copy"
            } else {
                "vault.team.reveal"
            },
            Some(id),
            &format!("{} ({})", record.name, shown.join(", ")),
        )
        .await?;
        Ok(Revealed::Values(revealed))
    }

    /// `POST /api/team-secrets/{id}/totp`: the current code, after a fresh re-authentication;
    /// audited as `vault.team.totp`.
    ///
    /// # Errors
    /// `NotFound` (also without a seed), `Forbidden` for API tokens, store failures.
    pub async fn team_secret_totp(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<Revealed<TotpCode>> {
        let record = self.team_record(id).await?;
        if let Some(status) = self.fresh_or_status(ctx).await? {
            return Ok(Revealed::ReauthRequired(status));
        }
        let payload: TeamPayload = self.open_payload(&record)?;
        let seed = payload
            .totp
            .as_deref()
            .ok_or_else(|| AppError::Invalid("this secret has no TOTP seed".into()))?;
        let code = self.totp_code(seed)?;
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.team.totp",
            Some(id),
            &record.name,
        )
        .await?;
        Ok(Revealed::Values(code))
    }

    /// The current code of a seed.
    pub(crate) fn totp_code(&self, seed: &str) -> AppResult<TotpCode> {
        let config = parse_otp(seed).map_err(invalid)?;
        let now = self.now().0;
        let hmac = self.deps.crypto.totp_hmac(
            config.algorithm,
            &config.secret,
            counter(now, config.period),
        );
        Ok(TotpCode {
            code: code_from_hmac(&hmac, config.digits),
            period: config.period,
            remaining: remaining_seconds(now, config.period),
        })
    }

    /// `GET /api/secrets/activity`: the vault's audit trail, newest first. Team entries and
    /// re-authentications are shown to every member; an agent secret's entries only to members
    /// who can view its sandbox (instance-wide and deleted ones: instance administrators and
    /// members who can view a sandbox, respectively instance administrators).
    ///
    /// # Errors
    /// Store failures.
    pub async fn vault_activity(
        &self,
        ctx: &AuthContext,
        limit: u32,
    ) -> AppResult<Vec<VaultActivity>> {
        use std::collections::HashMap;
        let admin = ctx.user.is_superadmin || ctx.user.is_instance_admin;
        let agents: HashMap<String, Option<yulius_domain::ids::SandboxId>> = self
            .deps
            .store
            .vault_agent(None)
            .await?
            .into_iter()
            .map(|r| (r.id, r.sandbox_id))
            .collect();
        let mut visible: HashMap<String, bool> = HashMap::new();
        let mut out = Vec::new();
        for entry in self.deps.store.vault_audit(limit.clamp(1, 500)).await? {
            if entry.action.starts_with("vault.agent.") {
                let allowed = match entry.target.as_ref().map(|t| agents.get(t)) {
                    Some(Some(Some(sandbox))) => match visible.get(sandbox.as_str()) {
                        Some(known) => *known,
                        None => {
                            let can = self.can_view(&ctx.user, sandbox).await;
                            visible.insert(sandbox.as_str().to_owned(), can);
                            can
                        }
                    },
                    Some(Some(None)) => true,
                    _ => admin,
                };
                if !allowed {
                    continue;
                }
            }
            out.push(VaultActivity {
                at: rfc3339(entry.at),
                actor: entry.actor,
                action: entry.action,
                secret_id: entry.target,
                detail: entry.detail,
            });
        }
        Ok(out)
    }
}
