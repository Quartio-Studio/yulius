//! Managing agent secrets (VLT-004, VLT-009, SPEC-040, SPEC-245 MAN): project secrets by members
//! with write access, global ones by instance administrators (who also switch them on per
//! project), personal ones by their owner alone (who switches them on for projects they work in;
//! to everyone else they do not exist); members who can view a project list its project and
//! global secrets' names and bindings; the people who manage a secret read its value back after a
//! two-factor unlock. Agents never do.

use yulius_contracts::api::secrets::{
    AgentSecretInput, AgentSecretPatch, AgentSecretView, RevealRequest, SecretReveal,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::vault::dotenv::flattened_key;
use yulius_domain::vault::hosts::{HostPattern, parse_hosts};
use yulius_domain::vault::{
    Injection, ROTATE_AFTER_MS, SecretName, check_agent_value, placeholder,
};

use super::team::{pick, reveal_request, shown};
use super::{AgentPayload, Revealed, field, invalid};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::{AgentScope, VaultSecretRecord, VaultSection};
use zeroize::Zeroizing;

/// Longest description.
const MAX_DESCRIPTION: usize = 1000;
/// Agent secrets that may reach one sandbox (its own, and global and personal ones switched on).
const MAX_AGENT_SECRETS: usize = 200;

pub(super) fn description(text: Option<&str>) -> AppResult<String> {
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
pub(super) struct Meta {
    pub(super) name: String,
    pub(super) hosts: Vec<String>,
    pub(super) injection: Injection,
}

/// An agent secret to store ([`App::insert_agent_secret`]).
pub(super) struct NewAgentSecret<'a> {
    /// The sandbox it is created from (its capacity is checked; a global or personal secret is
    /// switched on there).
    pub(super) sandbox: &'a SandboxId,
    /// Its scope.
    pub(super) scope: AgentScope,
    pub(super) meta: Meta,
    pub(super) description: String,
    pub(super) payload: &'a AgentPayload,
    /// The person's user id, recorded as its creator.
    pub(super) by: &'a str,
    /// The audit actor.
    pub(super) actor: &'a str,
    /// How it came in, appended to the audit detail.
    pub(super) origin: Option<&'a str>,
    /// Agents set it up themselves (SPEC-216): a person's choice, never an agent's.
    pub(super) auto_allow: bool,
}

/// Whether the caller administers the instance.
fn is_admin(ctx: &AuthContext) -> bool {
    ctx.user.is_superadmin || ctx.user.is_instance_admin
}

/// What a project's list says about a secret there (MAN-005).
#[derive(Default)]
pub(super) struct Here {
    /// It reaches the project's runs (a project secret always does).
    pub(super) enabled: bool,
    /// The scope of the secret of its name that wins there, when not itself.
    pub(super) shadowed_by: Option<&'static str>,
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
    /// Whether `ctx` may change, delete, reveal and answer for a secret of `scope` (MAN-001..003).
    pub(super) async fn require_manage_agent(
        &self,
        ctx: &AuthContext,
        scope: &AgentScope,
    ) -> AppResult<()> {
        match scope {
            AgentScope::Project(sandbox) => self
                .require(&ctx.user, sandbox, Action::Configure)
                .await
                .map(|_| ()),
            AgentScope::Global => {
                if is_admin(ctx) {
                    Ok(())
                } else {
                    Err(AppError::Forbidden(
                        "global secrets are managed by instance administrators".into(),
                    ))
                }
            }
            // Another person's answers like a missing secret (MAN-003).
            AgentScope::Personal(owner) if *owner == ctx.user.id => Ok(()),
            AgentScope::Personal(_) => Err(AppError::NotFound),
        }
    }

    pub(super) async fn agent_view(
        &self,
        record: &VaultSecretRecord,
        manage: bool,
    ) -> AppResult<AgentSecretView> {
        // Outside a project's list a project secret is on; a global or personal one reads off
        // (the list says where it is on).
        let here = Here {
            enabled: record.sandbox_id.is_some(),
            shadowed_by: None,
        };
        self.agent_view_in(record, manage, here).await
    }

    /// [`Self::agent_view`] in a project's list: whether it is switched on there and what
    /// shadows it (MAN-005).
    pub(super) async fn agent_view_in(
        &self,
        record: &VaultSecretRecord,
        manage: bool,
        here: Here,
    ) -> AppResult<AgentSecretView> {
        let injection = Injection::parse(&record.injection, record.header_name.as_deref())
            .unwrap_or(Injection::None);
        let name = SecretName::parse(&record.name).map_err(invalid)?;
        // Only its managers learn this about the value (SPEC-193 MLK-003). A payload that does
        // not open marks nothing: the list stays readable and a reveal reports the fault.
        let flattened = manage
            && self
                .open_payload::<AgentPayload>(record)
                .is_ok_and(|payload| flattened_key(&payload.value));
        Ok(AgentSecretView {
            id: record.id.clone(),
            name: record.name.clone(),
            description: record.description.clone(),
            scope: record.scope().as_str().into(),
            enabled: here.enabled,
            shadowed_by: here.shadowed_by.map(Into::into),
            hosts: record.hosts.clone(),
            requested_hosts: record.requested_hosts.clone(),
            requested_by: record.requested_by.clone(),
            env_files: record.env_files.as_str().into(),
            env_requested_by: record.env_requested_by.clone(),
            cd_prod: record.cd_prod,
            auto_allow: record.auto_allow,
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
            flattened,
            can_manage: manage,
        })
    }

    /// `GET /api/sandboxes/{sid}/agent-secrets`: the sandbox's project secrets, every global
    /// secret and the caller's own personal secrets (metadata only), each saying whether it is
    /// switched on for the sandbox and what shadows it (MAN-002, MAN-003, MAN-005). Needs read
    /// access to the sandbox.
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
        let admin = is_admin(ctx);
        let me = &ctx.user.id;
        let store = &self.deps.store;
        let on: std::collections::HashSet<String> =
            store.vault_switched_on(&sid).await?.into_iter().collect();
        let listed: Vec<VaultSecretRecord> = store
            .vault_agent_all()
            .await?
            .into_iter()
            .filter(|r| match r.scope() {
                AgentScope::Project(s) => s == sid,
                AgentScope::Global => true,
                AgentScope::Personal(owner) => owner == *me,
            })
            .collect();
        let reaching: Vec<VaultSecretRecord> = listed
            .iter()
            .filter(|r| r.sandbox_id.is_some() || on.contains(&r.id))
            .cloned()
            .collect();
        let shadows = super::reach::shadowed(&reaching, super::Holder::Person(me));
        let mut out = Vec::with_capacity(listed.len());
        for record in &listed {
            let manage = match record.scope() {
                AgentScope::Project(_) => level >= AccessLevel::Write,
                AgentScope::Global => admin,
                AgentScope::Personal(_) => true,
            };
            let here = Here {
                enabled: record.sandbox_id.is_some() || on.contains(&record.id),
                shadowed_by: shadows.get(&record.id).copied(),
            };
            out.push(self.agent_view_in(record, manage, here).await?);
        }
        Ok(out)
    }

    /// `PUT /api/sandboxes/{sid}/agent-secrets/{id}/enabled`: switches a global secret (instance
    /// administrators) or the caller's personal secret (in a sandbox they may work in) on or off
    /// for the sandbox; audited `vault.agent.enable` / `vault.agent.disable` (MAN-004).
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes and secrets, `Forbidden` for people who may not switch
    /// it, `Invalid` for a project secret.
    pub async fn switch_agent_secret(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        id: &str,
        on: bool,
    ) -> AppResult<AgentSecretView> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record_sandbox, _) = self.require(&ctx.user, &sid, Action::View).await?;
        let record = self.agent_record(ctx, id).await?;
        match record.scope() {
            AgentScope::Project(_) => {
                return Err(AppError::Invalid(
                    "a project secret always applies to its project; only global and personal \
                     secrets are switched on or off"
                        .into(),
                ));
            }
            AgentScope::Global => self.require_manage_agent(ctx, &AgentScope::Global).await?,
            AgentScope::Personal(owner) => {
                self.require_manage_agent(ctx, &AgentScope::Personal(owner))
                    .await?;
                self.require(&ctx.user, &sid, Action::Work).await?;
            }
        }
        if on
            && !self
                .deps
                .store
                .vault_switched_on(&sid)
                .await?
                .iter()
                .any(|s| s == id)
            && self.deps.store.vault_agent_in(&sid).await?.len() >= MAX_AGENT_SECRETS
        {
            return Err(AppError::Capacity(format!(
                "at most {MAX_AGENT_SECRETS} agent secrets apply to one sandbox"
            )));
        }
        let changed = self
            .deps
            .store
            .vault_switch(id, &sid, on, ctx.user.id.as_str(), self.now())
            .await?;
        if changed {
            self.vault.bump();
            self.vault_audit(
                &Self::vault_actor(ctx),
                if on {
                    "vault.agent.enable"
                } else {
                    "vault.agent.disable"
                },
                Some(id),
                &format!("{} → {}", record.name, record_sandbox.slug),
            )
            .await?;
            self.announce_request(&record);
            if !on {
                self.settle_secret_waits(Some(&sid)).await;
            }
        }
        let view = self
            .agent_secrets(ctx, sandbox)
            .await?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or(AppError::NotFound)?;
        Ok(view)
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
            "sandbox" => AgentScope::Project(sid.clone()),
            "instance" => AgentScope::Global,
            "personal" => {
                // Switched on here at once: only where its person works (MAN-003).
                self.require(&ctx.user, &sid, Action::Work).await?;
                AgentScope::Personal(ctx.user.id.clone())
            }
            _ => {
                return Err(AppError::Invalid(
                    "scope must be sandbox, instance or personal".into(),
                ));
            }
        };
        self.require_manage_agent(ctx, &scope).await?;
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
        let record = self
            .insert_agent_secret(NewAgentSecret {
                sandbox: &sid,
                scope,
                meta,
                description: description(input.description.as_deref())?,
                payload: &payload,
                by: ctx.user.id.as_str(),
                actor: &Self::vault_actor(ctx),
                origin: None,
                auto_allow: input.auto_allow.unwrap_or(true),
            })
            .await?;
        // Switched on for the sandbox it was added in (SCP-003).
        let here = Here {
            enabled: true,
            shadowed_by: None,
        };
        self.agent_view_in(&record, true, here).await
    }

    /// Stores a new agent secret and audits it (`vault.agent.create`, `origin` appended to the
    /// detail); a global or personal one is switched on for `sandbox` (SCP-003). At most
    /// [`MAX_AGENT_SECRETS`] may reach a sandbox, and a name taken in its scope is a `Conflict`
    /// (the store checks it in the same transaction).
    ///
    /// # Errors
    /// `Capacity`, `Conflict`, sealing and store failures.
    pub(super) async fn insert_agent_secret(
        &self,
        new: NewAgentSecret<'_>,
    ) -> AppResult<VaultSecretRecord> {
        let NewAgentSecret {
            sandbox,
            scope,
            meta,
            description,
            payload,
            by,
            actor,
            origin,
            auto_allow,
        } = new;
        if self.deps.store.vault_agent_in(sandbox).await?.len() >= MAX_AGENT_SECRETS {
            return Err(AppError::Capacity(format!(
                "at most {MAX_AGENT_SECRETS} agent secrets apply to one sandbox"
            )));
        }
        let (sandbox_id, owner_id) = match &scope {
            AgentScope::Project(s) => (Some(s.clone()), None),
            AgentScope::Global => (None, None),
            AgentScope::Personal(owner) => (None, Some(owner.clone())),
        };
        let id = self.deps.crypto.new_id();
        let now = self.now();
        let record = VaultSecretRecord {
            id: id.clone(),
            section: VaultSection::Agent,
            sandbox_id,
            owner_id,
            name: meta.name,
            description,
            hosts: meta.hosts,
            injection: meta.injection.kind().into(),
            header_name: match &meta.injection {
                Injection::Header(name) => Some(name.clone()),
                _ => None,
            },
            fields: payload.fields(),
            envelope: self.seal_payload(&id, VaultSection::Agent, payload)?,
            created_by: by.to_owned(),
            created_at: now,
            updated_by: by.to_owned(),
            updated_at: now,
            value_updated_at: now,
            last_used_at: None,
            requested_hosts: Vec::new(),
            requested_by: None,
            env_files: crate::ports::vault::EnvFiles::No,
            env_requested_by: None,
            cd_prod: false,
            auto_allow,
        };
        if matches!(scope, AgentScope::Project(_)) {
            self.deps.store.vault_insert(&record).await?;
        } else {
            self.deps
                .store
                .vault_insert_on(&record, sandbox, by, now)
                .await?;
        }
        self.vault.bump();
        self.vault_audit(
            actor,
            "vault.agent.create",
            Some(&id),
            &format!(
                "{} ({}) → {}{}{}",
                record.name,
                match scope {
                    AgentScope::Project(_) => "this sandbox",
                    AgentScope::Global => "global",
                    AgentScope::Personal(_) => "personal",
                },
                if record.hosts.is_empty() {
                    "no hosts yet".to_owned()
                } else {
                    record.hosts.join(", ")
                },
                if record.auto_allow {
                    ", agents set it up"
                } else {
                    ""
                },
                origin.map(|o| format!(", {o}")).unwrap_or_default()
            ),
        )
        .await?;
        Ok(record)
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
        match record.scope() {
            // Invisible sandboxes answer like a missing secret.
            AgentScope::Project(sandbox) => {
                self.require(&ctx.user, &sandbox, Action::View).await?;
            }
            AgentScope::Global => {}
            // So does another person's secret (MAN-003).
            AgentScope::Personal(owner) => {
                if owner != ctx.user.id {
                    return Err(AppError::NotFound);
                }
            }
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
        self.require_manage_agent(ctx, &record.scope()).await?;
        if record.cd_prod && patch.hosts.as_ref().is_some_and(|hosts| !hosts.is_empty()) {
            return Err(AppError::Conflict(super::PRODUCTION_SECRET.into()));
        }
        // SPEC-216 SSA-005 (review round 2): hosts edited from what the person saw, which an
        // agent may have changed since the dialog opened.
        // Agents bind hosts of a secret they set up at any time: its hosts change only against
        // those the editor saw (review round 4).
        if patch.hosts.is_some() && patch.expect_hosts.is_none() && record.auto_allow {
            return Err(AppError::Conflict(
                "agents bind this secret's hosts themselves: send expect_hosts, the hosts you saw, \
                 with the new hosts"
                    .into(),
            ));
        }
        if let Some(seen) = &patch.expect_hosts {
            let seen = parse_hosts(seen).map_err(invalid)?;
            let bound: Vec<HostPattern> = record
                .hosts
                .iter()
                .filter_map(|h| HostPattern::parse(h).ok())
                .collect();
            if seen.len() != bound.len() || !seen.iter().all(|h| bound.contains(h)) {
                return Err(AppError::Conflict(format!(
                    "its hosts changed since you opened it (now {}): reload and edit again",
                    if record.hosts.is_empty() {
                        "none".to_owned()
                    } else {
                        record.hosts.join(", ")
                    }
                )));
            }
        }
        // SPEC-216 SSA-004: a production-only secret is never set up by agents.
        if record.cd_prod && patch.auto_allow == Some(true) {
            return Err(AppError::Conflict(super::PRODUCTION_SECRET.into()));
        }
        let toggled = patch.auto_allow.filter(|on| *on != record.auto_allow);
        let seen = record.clone();
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
        if let Some(on) = toggled {
            record.auto_allow = on;
            changed.push(if on {
                "agents set it up"
            } else {
                "agents no longer set it up"
            });
        }
        if !self.deps.store.vault_update(&record, &seen).await? {
            // Gone; or its hosts or switch changed meanwhile (an agent bound a host, a person
            // switched it), or it was allowed for production: nothing is undone unseen.
            return Err(match self.deps.store.vault_get(id).await? {
                Some(now) if now.cd_prod && record.auto_allow => {
                    AppError::Conflict(super::PRODUCTION_SECRET.into())
                }
                Some(_) => AppError::Conflict(
                    "the secret changed meanwhile (an agent bound a host, or someone changed it): \
                     reload and save again"
                        .into(),
                ),
                None => AppError::NotFound,
            });
        }
        self.vault.bump();
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.agent.update",
            Some(id),
            &format!("{} ({})", record.name, changed.join(", ")),
        )
        .await?;
        if let Some(on) = toggled {
            self.vault_audit(
                &Self::vault_actor(ctx),
                if on {
                    "vault.agent.auto_allow_on"
                } else {
                    "vault.agent.auto_allow_off"
                },
                Some(id),
                &record.name,
            )
            .await?;
        }
        self.agent_view(&record, true).await
    }

    /// `DELETE /api/agent-secrets/{id}`.
    ///
    /// # Errors
    /// Access errors, `NotFound`.
    pub async fn delete_agent_secret(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, &record.scope()).await?;
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
        .await?;
        // Its requests went with it (SPEC-213).
        self.settle_secret_waits(record.sandbox_id.as_ref()).await;
        Ok(())
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
        self.require_manage_agent(ctx, &record.scope()).await?;
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
