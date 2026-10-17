//! Team secrets (VLT-001..002, SPEC-040): a title, a value and notes; reveals behind a two-factor
//! unlock, everything audited. A team secret belongs to one sandbox, read by the people who can
//! view it and changed by those who write to it, or is team-wide, for every member (SPEC-216
//! SCO-001/002).

use yulius_contracts::api::secrets::{
    ReauthStatus, RevealRequest, SecretReveal, TeamSecretInput, TeamSecretPatch, TeamSecretView,
    TotpCode, VaultActivity,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
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

/// Validates a reveal request: its purpose (`show` or `copy`) and fields (`value`, `notes`).
pub(super) fn reveal_request(request: &RevealRequest) -> AppResult<(&str, Vec<String>)> {
    let purpose = request.purpose.as_deref().unwrap_or("show");
    if !matches!(purpose, "show" | "copy") {
        return Err(AppError::Invalid("purpose must be show or copy".into()));
    }
    let wanted = request
        .fields
        .clone()
        .unwrap_or_else(|| vec!["value".into(), "notes".into()]);
    if wanted.is_empty()
        || wanted
            .iter()
            .any(|f| !matches!(f.as_str(), "value" | "notes"))
    {
        return Err(AppError::Invalid("fields are value and notes".into()));
    }
    Ok((purpose, wanted))
}

/// The asked fields that are stored.
pub(super) fn pick(
    wanted: &[String],
    value: &Option<String>,
    notes: &Option<String>,
) -> SecretReveal {
    let pick = |name: &str, field: &Option<String>| {
        wanted
            .iter()
            .any(|w| w == name)
            .then(|| field.clone())
            .flatten()
    };
    SecretReveal {
        value: pick("value", value),
        notes: pick("notes", notes),
    }
}

/// The revealed field names, for the audit trail.
pub(super) fn shown(revealed: &SecretReveal) -> String {
    [
        ("value", revealed.value.is_some()),
        ("notes", revealed.notes.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(name, _)| name)
    .collect::<Vec<_>>()
    .join(", ")
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

impl App {
    async fn team_view(&self, record: &VaultSecretRecord) -> AppResult<TeamSecretView> {
        let payload: TeamPayload = self.open_payload(record)?;
        let now = self.now();
        Ok(TeamSecretView {
            id: record.id.clone(),
            title: record.name.clone(),
            scope: if record.sandbox_id.is_some() {
                "sandbox".into()
            } else {
                "instance".into()
            },
            has_value: payload.value.is_some(),
            has_notes: payload.notes.is_some(),
            created_by: self.login_of_id(&record.created_by).await,
            updated_by: self.login_of_id(&record.updated_by).await,
            updated_at: rfc3339(record.updated_at),
            value_updated_at: rfc3339(record.value_updated_at),
            rotate_due: payload.value.is_some()
                && now.millis_since(record.value_updated_at) >= ROTATE_AFTER_MS,
        })
    }

    pub(super) async fn login_of_id(&self, id: &str) -> String {
        match yulius_domain::ids::UserId::parse(id) {
            Ok(user) => self.login_of(&user).await,
            Err(_) => "unknown".into(),
        }
    }

    /// A team secret the caller may see: a sandbox's one needs view access to that sandbox
    /// (otherwise it answers like a missing one).
    async fn team_record(&self, ctx: &AuthContext, id: &str) -> AppResult<VaultSecretRecord> {
        let record = self
            .deps
            .store
            .vault_get(id)
            .await?
            .filter(|r| r.section == VaultSection::Team)
            .ok_or(AppError::NotFound)?;
        if let Some(sandbox) = &record.sandbox_id {
            self.require(&ctx.user, sandbox, Action::View).await?;
        }
        Ok(record)
    }

    /// A team secret the caller may change: a sandbox's one needs write access to that sandbox
    /// (people who only read it see and reveal it); a team-wide one, membership as before.
    async fn team_record_to_change(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<VaultSecretRecord> {
        let record = self.team_record(ctx, id).await?;
        if let Some(sandbox) = &record.sandbox_id {
            self.require(&ctx.user, sandbox, Action::Work).await?;
        }
        Ok(record)
    }

    /// `GET /api/team-secrets`: the team-wide secrets, which every member sees (titles; the
    /// value and the notes only flagged).
    ///
    /// # Errors
    /// Store or decryption failures.
    pub async fn team_secrets(&self, _ctx: &AuthContext) -> AppResult<Vec<TeamSecretView>> {
        let mut out = Vec::new();
        for record in self.deps.store.vault_team(None).await? {
            if record.sandbox_id.is_none() {
                out.push(self.team_view(&record).await?);
            }
        }
        Ok(out)
    }

    /// `GET /api/sandboxes/{sid}/team-secrets`: the sandbox's team secrets and the team-wide
    /// ones. Needs view access to the sandbox.
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes; store or decryption failures.
    pub async fn sandbox_team_secrets(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<Vec<TeamSecretView>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        let mut out = Vec::new();
        for record in self.deps.store.vault_team(Some(&sid)).await? {
            out.push(self.team_view(&record).await?);
        }
        Ok(out)
    }

    /// `POST /api/sandboxes/{sid}/team-secrets` (`sandbox`; `scope` defaults to it) and
    /// `POST /api/team-secrets` (team-wide).
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes, `Invalid` input; store failures.
    pub async fn create_team_secret(
        &self,
        ctx: &AuthContext,
        sandbox: Option<&str>,
        input: &TeamSecretInput,
    ) -> AppResult<TeamSecretView> {
        let sid = match sandbox {
            Some(text) => {
                let sid = SandboxId::parse(text).map_err(|_| AppError::NotFound)?;
                self.require(&ctx.user, &sid, Action::View).await?;
                Some(sid)
            }
            None => None,
        };
        let scope = match (input.scope.as_deref(), sid) {
            (Some("instance"), _) | (None, None) => None,
            (Some("sandbox") | None, Some(sid)) => {
                self.require(&ctx.user, &sid, Action::Work).await?;
                Some(sid)
            }
            (Some("sandbox"), None) => {
                return Err(AppError::Invalid(
                    "a sandbox's team secret is added under that sandbox".into(),
                ));
            }
            _ => {
                return Err(AppError::Invalid(
                    "scope must be sandbox or instance".into(),
                ));
            }
        };
        let payload = TeamPayload {
            value: field(input.value.as_deref(), "value", false)?,
            notes: field(input.notes.as_deref(), "notes", false)?,
        };
        let id = self.deps.crypto.new_id();
        let now = self.now();
        let record = VaultSecretRecord {
            id: id.clone(),
            section: VaultSection::Team,
            sandbox_id: scope.clone(),
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
            requested_hosts: Vec::new(),
            requested_by: None,
            env_files: crate::ports::vault::EnvFiles::No,
            env_requested_by: None,
            cd_prod: false,
            auto_allow: false,
        };
        self.deps.store.vault_insert(&record).await?;
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.team.create",
            Some(&id),
            &format!(
                "{} ({})",
                record.name,
                if scope.is_some() {
                    "this sandbox"
                } else {
                    "team-wide"
                }
            ),
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
        let mut record = self.team_record_to_change(ctx, id).await?;
        let seen = record.clone();
        let mut payload: TeamPayload = self.open_payload(&record)?;
        let mut changed: Vec<&str> = Vec::new();
        let mut value_changed = false;
        if let Some(text) = &patch.title {
            record.name = title(text)?;
            changed.push("title");
        }
        if patch.value.is_some() {
            payload.value = field(patch.value.as_deref(), "value", false)?;
            changed.push("value");
            value_changed = true;
        }
        if patch.notes.is_some() {
            payload.notes = field(patch.notes.as_deref(), "notes", false)?;
            changed.push("notes");
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
        if !self.deps.store.vault_update(&record, &seen).await? {
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
        let record = self.team_record_to_change(ctx, id).await?;
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
    pub(super) async fn authenticated_at(&self, ctx: &AuthContext) -> AppResult<Option<Timestamp>> {
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

    /// `GET /api/auth/reauth`: whether the vault is unlocked, the caller's two-factor, and how
    /// to confirm identity (to set two-factor up).
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
        let identity = self.authenticated_at(ctx).await?;
        let unlocked = match &ctx.session_hash {
            Some(hash) => self.deps.store.vault_unlock(hash).await?,
            None => None,
        };
        let (two_factor, confirmed) = self.two_factor_state(ctx, identity).await?;
        // An unlock counts only with a confirmed seed: one stored by an unlock that raced an
        // administrator's reset (SPEC-061) opens nothing, also once a new pending seed locks.
        let fresh = confirmed && unlocked.is_some_and(|at| reauth_fresh(now, at));
        Ok(ReauthStatus {
            fresh,
            fresh_until: unlocked
                .filter(|_| fresh)
                .map(|at| rfc3339(at.plus_millis(REAUTH_WINDOW_MS))),
            two_factor: two_factor.to_owned(),
            identity_fresh: identity.is_some_and(|at| reauth_fresh(now, at)),
            methods,
        })
    }

    /// Counts an attempt of `key` (a user's password confirmations or codes): at most 5 a minute.
    pub(super) fn limit_attempts(&self, key: &str) -> AppResult<()> {
        let now = self.now();
        let mut attempts = self
            .login_attempts
            .lock()
            .map_err(|_| AppError::Internal("rate limiter poisoned".into()))?;
        let entry = attempts.entry(key.to_owned()).or_default();
        entry.retain(|t| now.0 - *t < 60_000);
        if entry.len() >= REAUTH_ATTEMPTS_PER_MINUTE {
            return Err(AppError::Capacity(
                "too many attempts; wait a minute".into(),
            ));
        }
        entry.push(now.0);
        Ok(())
    }

    /// `POST /api/auth/reauth`: the superadmin confirms it is them with the password (rate
    /// limited, audited), which allows setting up two-factor.
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
        self.limit_attempts(&format!("reauth:{}", ctx.user.id.as_str()))?;
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

    /// Checks the two-factor unlock before a reveal (SPEC-040 R4).
    pub(super) async fn fresh_or_status(
        &self,
        ctx: &AuthContext,
    ) -> AppResult<Option<ReauthStatus>> {
        if ctx.session_hash.is_none() {
            return Err(AppError::Forbidden(
                "secrets are revealed only in the web UI, after entering your two-factor code"
                    .into(),
            ));
        }
        let status = self.reauth_status(ctx).await?;
        Ok((!status.fresh).then_some(status))
    }

    /// `POST /api/team-secrets/{id}/reveal`: the value and/or the notes, after a two-factor
    /// unlock; audited as `vault.team.reveal` or `vault.team.copy`.
    ///
    /// # Errors
    /// `NotFound`, `Invalid` purpose or field, `Forbidden` for API tokens, store failures.
    pub async fn reveal_team_secret(
        &self,
        ctx: &AuthContext,
        id: &str,
        request: &RevealRequest,
    ) -> AppResult<Revealed<SecretReveal>> {
        let (purpose, wanted) = reveal_request(request)?;
        let record = self.team_record(ctx, id).await?;
        if let Some(status) = self.fresh_or_status(ctx).await? {
            return Ok(Revealed::ReauthRequired(status));
        }
        let payload: TeamPayload = self.open_payload(&record)?;
        let revealed = pick(&wanted, &payload.value, &payload.notes);
        self.vault_audit(
            &Self::vault_actor(ctx),
            if purpose == "copy" {
                "vault.team.copy"
            } else {
                "vault.team.reveal"
            },
            Some(id),
            &format!("{} ({})", record.name, shown(&revealed)),
        )
        .await?;
        Ok(Revealed::Values(revealed))
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

    /// `GET /api/secrets/activity`: the vault's audit trail, newest first. Re-authentications
    /// and team-wide secrets' entries are shown to every member; a sandbox's secret's entries
    /// (team or agent) only to members who can view that sandbox; an instance-wide agent
    /// secret's to every member, and a deleted secret's only to instance administrators.
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
        let store = &self.deps.store;
        let secrets: HashMap<String, Option<SandboxId>> = store
            .vault_agent(None)
            .await?
            .into_iter()
            .chain(store.vault_team(None).await?)
            .map(|r| (r.id, r.sandbox_id))
            .collect();
        let mut visible: HashMap<String, bool> = HashMap::new();
        let mut out = Vec::new();
        for entry in self.deps.store.vault_audit(limit.clamp(1, 500)).await? {
            if entry.action.starts_with("vault.agent.") || entry.action.starts_with("vault.team.") {
                let allowed = match entry.target.as_ref().map(|t| secrets.get(t)) {
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
