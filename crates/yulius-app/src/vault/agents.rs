//! Managing agent secrets (VLT-004, VLT-009, SPEC-040): sandbox secrets by members with write
//! access, instance-wide ones by instance administrators; members who can view a sandbox list
//! names and bindings; the people who manage a secret read its value back after a two-factor
//! unlock. Agents never do.

use yulius_contracts::api::secrets::{
    AgentSecretInput, AgentSecretPatch, AgentSecretView, RevealRequest, SecretReveal,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::vault::hosts::parse_hosts;
use yulius_domain::vault::{
    Injection, ROTATE_AFTER_MS, SecretName, check_agent_value, placeholder,
};

use super::team::{pick, reveal_request, shown};
use super::{AgentPayload, Revealed, field, invalid};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::{VaultSecretRecord, VaultSection};
use zeroize::Zeroizing;

/// Longest description.
const MAX_DESCRIPTION: usize = 1000;
/// Agent secrets per sandbox plus instance-wide ones.
const MAX_AGENT_SECRETS: usize = 200;

fn description(text: Option<&str>) -> AppResult<String> {
    let text = text.unwrap_or_default().trim();
    if text.chars().count() > MAX_DESCRIPTION {
        return Err(AppError::Invalid(format!(
            "the description is longer than {MAX_DESCRIPTION} characters"
        )));
    }
    Ok(text.to_owned())
}

fn username(text: Option<&str>) -> AppResult<Option<String>> {
    let value = field(text, "user name", true)?;
    if let Some(value) = &value
        && value.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(
            "the user name cannot contain control characters".into(),
        ));
    }
    Ok(value)
}

/// Checks how a user name fits the injection: Basic needs one, and a new one is taken only for
/// Basic (older secrets keep theirs for website logins).
fn check_username(injection: &Injection, username: Option<&str>, set: bool) -> AppResult<()> {
    if *injection == Injection::Basic && username.is_none() {
        return Err(AppError::Invalid(
            "Basic credentials need a user name".into(),
        ));
    }
    if set && username.is_some() && *injection != Injection::Basic {
        return Err(AppError::Invalid(
            "a user name is only for Basic credentials: put a login's user name in the notes"
                .into(),
        ));
    }
    Ok(())
}

/// Validated metadata of an agent secret.
struct Meta {
    name: String,
    hosts: Vec<String>,
    injection: Injection,
}

fn meta(name: &str, hosts: &[String], kind: Option<&str>, header: Option<&str>) -> AppResult<Meta> {
    let name = SecretName::parse(name).map_err(invalid)?;
    let hosts = parse_hosts(hosts).map_err(invalid)?;
    let injection = Injection::parse(kind.unwrap_or("none"), header).map_err(invalid)?;
    Ok(Meta {
        name: name.as_str().to_owned(),
        hosts: hosts.iter().map(ToString::to_string).collect(),
        injection,
    })
}

impl App {
    pub(super) async fn require_manage_agent(
        &self,
        ctx: &AuthContext,
        sandbox: Option<&SandboxId>,
    ) -> AppResult<()> {
        match sandbox {
            Some(sandbox) => self
                .require(&ctx.user, sandbox, Action::Configure)
                .await
                .map(|_| ()),
            None => {
                if ctx.user.is_superadmin || ctx.user.is_instance_admin {
                    Ok(())
                } else {
                    Err(AppError::Forbidden(
                        "secrets for every sandbox are managed by instance administrators".into(),
                    ))
                }
            }
        }
    }

    pub(super) async fn agent_view(
        &self,
        record: &VaultSecretRecord,
        manage: bool,
    ) -> AppResult<AgentSecretView> {
        let injection = Injection::parse(&record.injection, record.header_name.as_deref())
            .unwrap_or(Injection::None);
        let name = SecretName::parse(&record.name).map_err(invalid)?;
        Ok(AgentSecretView {
            id: record.id.clone(),
            name: record.name.clone(),
            description: record.description.clone(),
            scope: if record.sandbox_id.is_some() {
                "sandbox".into()
            } else {
                "instance".into()
            },
            hosts: record.hosts.clone(),
            requested_hosts: record.requested_hosts.clone(),
            requested_by: record.requested_by.clone(),
            env_files: record.env_files.as_str().into(),
            env_requested_by: record.env_requested_by.clone(),
            cd_prod: record.cd_prod,
            injection: injection.kind().into(),
            header_name: record.header_name.clone(),
            injection_text: injection.describe(),
            placeholder: placeholder(&name),
            has_username: record.fields.iter().any(|f| f == "username"),
            has_totp: record.fields.iter().any(|f| f == "totp"),
            created_by: self.login_of_id(&record.created_by).await,
            updated_at: rfc3339(record.updated_at),
            value_updated_at: rfc3339(record.value_updated_at),
            last_used_at: record.last_used_at.map(rfc3339),
            rotate_due: self.now().millis_since(record.value_updated_at) >= ROTATE_AFTER_MS,
            can_manage: manage,
        })
    }

    /// `GET /api/sandboxes/{sid}/agent-secrets`: the sandbox's agent secrets and the
    /// instance-wide ones (metadata only). Needs read access to the sandbox.
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes; store failures.
    pub async fn agent_secrets(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<Vec<AgentSecretView>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (_, level) = self.require(&ctx.user, &sid, Action::View).await?;
        let admin = ctx.user.is_superadmin || ctx.user.is_instance_admin;
        let mut out = Vec::new();
        for record in self.deps.store.vault_agent(Some(&sid)).await? {
            let manage = if record.sandbox_id.is_some() {
                level >= AccessLevel::Write
            } else {
                admin
            };
            out.push(self.agent_view(&record, manage).await?);
        }
        Ok(out)
    }

    /// `POST /api/sandboxes/{sid}/agent-secrets`.
    ///
    /// # Errors
    /// Access, validation and duplicate-name (`Conflict`) errors.
    pub async fn create_agent_secret(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        input: &AgentSecretInput,
    ) -> AppResult<AgentSecretView> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        let scope = match input.scope.as_deref().unwrap_or("sandbox") {
            "sandbox" => Some(sid.clone()),
            "instance" => None,
            _ => {
                return Err(AppError::Invalid(
                    "scope must be sandbox or instance".into(),
                ));
            }
        };
        self.require_manage_agent(ctx, scope.as_ref()).await?;
        let meta = meta(
            &input.name,
            &input.hosts,
            input.injection.as_deref(),
            input.header_name.as_deref(),
        )?;
        check_agent_value(&input.value).map_err(invalid)?;
        let payload = AgentPayload {
            username: username(input.username.as_deref())?,
            value: input.value.clone(),
            totp: None,
        };
        check_username(&meta.injection, payload.username.as_deref(), true)?;
        if self.deps.store.vault_agent(Some(&sid)).await?.len() >= MAX_AGENT_SECRETS {
            return Err(AppError::Capacity(format!(
                "at most {MAX_AGENT_SECRETS} agent secrets apply to one sandbox"
            )));
        }
        let id = self.deps.crypto.new_id();
        let now = self.now();
        let record = VaultSecretRecord {
            id: id.clone(),
            section: VaultSection::Agent,
            sandbox_id: scope.clone(),
            name: meta.name,
            description: description(input.description.as_deref())?,
            hosts: meta.hosts,
            injection: meta.injection.kind().into(),
            header_name: match &meta.injection {
                Injection::Header(name) => Some(name.clone()),
                _ => None,
            },
            fields: payload.fields(),
            envelope: self.seal_payload(&id, VaultSection::Agent, &payload)?,
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
        };
        self.deps.store.vault_insert(&record).await?;
        self.vault.bump();
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.agent.create",
            Some(&id),
            &format!(
                "{} ({}) → {}",
                record.name,
                if scope.is_some() {
                    "this sandbox"
                } else {
                    "every sandbox"
                },
                if record.hosts.is_empty() {
                    "no hosts yet".to_owned()
                } else {
                    record.hosts.join(", ")
                }
            ),
        )
        .await?;
        self.agent_view(&record, true).await
    }

    pub(super) async fn agent_record(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<VaultSecretRecord> {
        let record = self
            .deps
            .store
            .vault_get(id)
            .await?
            .filter(|r| r.section == VaultSection::Agent)
            .ok_or(AppError::NotFound)?;
        if let Some(sandbox) = &record.sandbox_id {
            // Invisible sandboxes answer like a missing secret.
            self.require(&ctx.user, sandbox, Action::View).await?;
        }
        Ok(record)
    }

    /// `PATCH /api/agent-secrets/{id}`.
    ///
    /// # Errors
    /// Access, validation and duplicate-name errors.
    pub async fn update_agent_secret(
        &self,
        ctx: &AuthContext,
        id: &str,
        patch: &AgentSecretPatch,
    ) -> AppResult<AgentSecretView> {
        let mut record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        if record.cd_prod && patch.hosts.as_ref().is_some_and(|hosts| !hosts.is_empty()) {
            return Err(AppError::Conflict(super::PRODUCTION_SECRET.into()));
        }
        let mut payload: AgentPayload = self.open_payload(&record)?;
        let meta = meta(
            patch.name.as_deref().unwrap_or(&record.name),
            patch.hosts.as_deref().unwrap_or(&record.hosts),
            Some(patch.injection.as_deref().unwrap_or(&record.injection)),
            patch
                .header_name
                .as_deref()
                .or(record.header_name.as_deref()),
        )?;
        let mut changed: Vec<&str> = Vec::new();
        if patch.name.is_some() {
            changed.push("name");
        }
        if patch.hosts.is_some() {
            changed.push("hosts");
        }
        if patch.injection.is_some() || patch.header_name.is_some() {
            changed.push("injection");
        }
        if let Some(text) = &patch.description {
            record.description = description(Some(text))?;
            changed.push("description");
        }
        if patch.username.is_some() {
            payload.username = username(patch.username.as_deref())?;
            changed.push("username");
        }
        let now = self.now();
        if let Some(value) = &patch.value {
            check_agent_value(value).map_err(invalid)?;
            payload.value.clone_from(value);
            record.value_updated_at = now;
            changed.push("value");
        }
        check_username(
            &meta.injection,
            payload.username.as_deref(),
            patch.username.is_some(),
        )?;
        record.name = meta.name;
        record.hosts = meta.hosts;
        record.injection = meta.injection.kind().into();
        record.header_name = match &meta.injection {
            Injection::Header(name) => Some(name.clone()),
            _ => None,
        };
        record.fields = payload.fields();
        record.envelope = self.seal_payload(&record.id, VaultSection::Agent, &payload)?;
        record.updated_by = ctx.user.id.as_str().to_owned();
        record.updated_at = now;
        if !self.deps.store.vault_update(&record).await? {
            return Err(AppError::NotFound);
        }
        self.vault.bump();
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.agent.update",
            Some(id),
            &format!("{} ({})", record.name, changed.join(", ")),
        )
        .await?;
        self.agent_view(&record, true).await
    }

    /// `DELETE /api/agent-secrets/{id}`.
    ///
    /// # Errors
    /// Access errors, `NotFound`.
    pub async fn delete_agent_secret(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        if !self.deps.store.vault_delete(id).await? {
            return Err(AppError::NotFound);
        }
        self.vault.bump();
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.agent.delete",
            Some(id),
            &record.name,
        )
        .await
    }

    /// `POST /api/agent-secrets/{id}/reveal`: the value and the notes (its description) for a
    /// person who manages the secret, after a two-factor unlock; audited as
    /// `vault.agent.reveal` or `vault.agent.copy`.
    ///
    /// # Errors
    /// `NotFound` for invisible secrets, `Forbidden` for people who cannot manage it and for API
    /// tokens, `Invalid` requests, store failures.
    pub async fn reveal_agent_secret(
        &self,
        ctx: &AuthContext,
        id: &str,
        request: &RevealRequest,
    ) -> AppResult<Revealed<SecretReveal>> {
        let (purpose, wanted) = reveal_request(request)?;
        let record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        if let Some(status) = self.fresh_or_status(ctx).await? {
            return Ok(Revealed::ReauthRequired(status));
        }
        let payload: AgentPayload = self.open_payload(&record)?;
        // Basic credentials are the user name and the value; older secrets also kept a website
        // login's user name and a TOTP seed: they lead the notes so their managers can read them.
        let user_line = payload
            .username
            .as_ref()
            .map(|u| Zeroizing::new(format!("User name: {u}")));
        let seed_line = payload
            .totp
            .as_ref()
            .map(|t| Zeroizing::new(format!("Two-factor seed: {t}")));
        let head = Zeroizing::new(
            [user_line.as_deref(), seed_line.as_deref()]
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let notes = Zeroizing::new(
            [head.as_str(), record.description.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        let notes = Zeroizing::new((!notes.is_empty()).then(|| notes.to_string()));
        let value = Zeroizing::new(Some(payload.value.clone()));
        let revealed = pick(&wanted, &value, &notes);
        self.vault_audit(
            &Self::vault_actor(ctx),
            if purpose == "copy" {
                "vault.agent.copy"
            } else {
                "vault.agent.reveal"
            },
            Some(id),
            &format!("{} ({})", record.name, shown(&revealed)),
        )
        .await?;
        Ok(Revealed::Values(revealed))
    }
}
