//! Managing agent secrets (VLT-004, VLT-009): sandbox secrets by members with write access,
//! instance-wide ones by instance administrators; members who can view a sandbox list names and
//! bindings; values are write-only for everyone.

use yulius_contracts::api::secrets::{AgentSecretInput, AgentSecretPatch, AgentSecretView};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::vault::hosts::parse_hosts;
use yulius_domain::vault::totp::parse_otp;
use yulius_domain::vault::{
    Injection, ROTATE_AFTER_MS, SecretName, check_agent_value, placeholder,
};

use super::{AgentPayload, field, invalid};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::{VaultSecretRecord, VaultSection};

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
        return Err(AppError::Invalid("the user name cannot contain control characters".into()));
    }
    Ok(value)
}

fn totp(text: Option<&str>) -> AppResult<Option<String>> {
    let value = field(text, "TOTP seed", true)?;
    if let Some(seed) = &value {
        parse_otp(seed).map_err(invalid)?;
    }
    Ok(value)
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
    let injection = Injection::parse(kind.unwrap_or("bearer"), header).map_err(invalid)?;
    Ok(Meta {
        name: name.as_str().to_owned(),
        hosts: hosts.iter().map(ToString::to_string).collect(),
        injection,
    })
}

impl App {
    async fn require_manage_agent(
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

    async fn agent_view(
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
            _ => return Err(AppError::Invalid("scope must be sandbox or instance".into())),
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
            totp: totp(input.totp.as_deref())?,
        };
        if meta.injection == Injection::Basic && payload.username.is_none() {
            return Err(AppError::Invalid(
                "Basic credentials need a user name".into(),
            ));
        }
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
                if scope.is_some() { "this sandbox" } else { "every sandbox" },
                record.hosts.join(", ")
            ),
        )
        .await?;
        self.agent_view(&record, true).await
    }

    async fn agent_record(&self, ctx: &AuthContext, id: &str) -> AppResult<VaultSecretRecord> {
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
        if patch.totp.is_some() {
            payload.totp = totp(patch.totp.as_deref())?;
            changed.push("totp");
        }
        if meta.injection == Injection::Basic && payload.username.is_none() {
            return Err(AppError::Invalid(
                "Basic credentials need a user name".into(),
            ));
        }
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
        self.vault_audit(&Self::vault_actor(ctx), "vault.agent.delete", Some(id), &record.name)
            .await
    }
}
