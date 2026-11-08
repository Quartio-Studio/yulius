//! Copies of production secrets agents ask for (SPEC-440, ADR-0284). A production secret is
//! never used by agents (SPEC-134); when agent work needs the same value (a dev CI token that only
//! exists as a production secret), an agent asks with `secret_request_copy` for a new project
//! secret of that value, with its name and hosts. A person who manages both allows the copy on
//! the Secrets page after their two-factor unlock, as a reveal needs; nobody sees the value, and
//! the production secret stays production's.

use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::secrets::{AgentSecretView, CopyRequestAnswer, CopyRequestView};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::vault::Injection;
use yulius_domain::vault::hosts::parse_hosts;

use super::agents::{MAX_AGENT_SECRETS, Meta, NewAgentSecret, description};
use super::create::agent_made_name;
use super::requests::{patterns, reason, requester, same_hosts};
use super::{AgentPayload, Revealed, SecretUser, invalid};
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::{AgentScope, VaultCopyRequest};

/// Arguments of `secret_request_copy`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyArgs {
    secret: String,
    name: String,
    hosts: Vec<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// How a project's list shows a copy request.
pub(super) fn copy_view(request: VaultCopyRequest, can_answer: bool) -> CopyRequestView {
    CopyRequestView {
        name: request.name,
        hosts: request.hosts,
        requested_by: request.requested_by,
        reason: request.reason,
        can_answer,
    }
}

impl App {
    /// `secret_request_copy` (SPEC-440 CPY-001..003): stores the copy of a production secret an
    /// agent or a Superagent asks for its project (a name and hosts; never a value or a binding),
    /// audits it and flags a task as needing a person.
    pub(crate) async fn tool_secret_request_copy(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let args: CopyArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let name = agent_made_name(&args.name)?;
        let hosts: Vec<String> = parse_hosts(&args.hosts)
            .map_err(invalid)?
            .iter()
            .map(ToString::to_string)
            .collect();
        if hosts.is_empty() {
            return Err(AppError::Invalid(
                "name at least one host the copy is sent to, like api.example.com".into(),
            ));
        }
        let resolved = self
            .resolved_agent_secrets(user.sandbox(), user.holder())
            .await?;
        let source = resolved
            .iter()
            .find(|r| r.name == args.secret.trim())
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {:?} applies to this project (see secrets_list)",
                    args.secret
                ))
            })?;
        if !source.cd_prod {
            return Err(AppError::Invalid(format!(
                "{} is no production secret: agents use it as it is. Ask for its hosts with \
                 secret_request_hosts, or for .env copies with secret_copy_to_env",
                source.name
            )));
        }
        if resolved.iter().any(|r| r.name == name.as_str()) {
            return Err(AppError::Conflict(format!(
                "an agent secret named {} already applies to this project (see secrets_list); it \
                 is never replaced: pick another name",
                name.as_str()
            )));
        }
        let store = &self.deps.store;
        let sandbox = store
            .sandbox(user.sandbox())
            .await?
            .ok_or(AppError::NotFound)?;
        let waiting = store
            .vault_copy_requests(user.sandbox())
            .await?
            .into_iter()
            .find(|c| c.secret_id == source.id);
        // The same copy asked again adds the asker; another name or hosts replace the request.
        let same = waiting
            .as_ref()
            .is_some_and(|w| w.name == name.as_str() && w.hosts == hosts);
        let by = requester(
            waiting.as_ref().map(|w| w.requested_by.as_str()),
            same,
            &user.asker(),
            None,
        );
        let why = reason(args.reason.as_deref());
        let request = VaultCopyRequest {
            secret_id: source.id.clone(),
            sandbox_id: user.sandbox().clone(),
            name: name.as_str().to_owned(),
            hosts,
            requested_by: by,
            reason: why.clone(),
            asked_at: self.now(),
        };
        if !store
            .vault_put_copy_request(&request, waiting.as_ref())
            .await?
        {
            return Err(AppError::Conflict(
                "the request changed meanwhile: ask again".into(),
            ));
        }
        self.announce_request(source);
        self.vault_audit(
            &user.actor(),
            "vault.agent.copy_request",
            Some(&source.id),
            &format!(
                "{} → {} on {}{}",
                source.name,
                request.name,
                request.hosts.join(", "),
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" ({why})")
                }
            ),
        )
        .await?;
        self.flag_secret_wait(user, "a copy request").await?;
        Ok(json!({
            "requested": request.name,
            "copy_of": source.name,
            "hosts": request.hosts,
            "requested_by": request.requested_by,
            "link": self.agent_secret_link(&sandbox.slug, source),
            "next": format!(
                "A person who manages {} and this project's secrets allows the copy on the Secrets \
                 page: print the link so they can. Until then {} does not exist; go on with other \
                 work and use it once secrets_list shows it (the proxy sends it to these hosts).",
                source.name, request.name
            ),
        }))
    }

    /// `POST /api/sandboxes/{sid}/agent-secrets/{id}/copy` (SPEC-440 CPY-004): a person who
    /// manages the production secret `id` and the project's secrets allows the copy an agent of
    /// the project asked for (after a two-factor unlock: a new project secret of its value, bound
    /// to the hosts asked for) or dismisses it. `answer` names what the person saw; the request
    /// is removed in the transaction that adds the copy.
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes and secrets, `Forbidden` for people who cannot manage
    /// both and, allowing, for API tokens, `Conflict` when nothing waits, the request changed
    /// meanwhile or the name is taken, `Capacity`, store failures.
    pub async fn answer_copy_request(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        id: &str,
        answer: &CopyRequestAnswer,
    ) -> AppResult<Revealed<AgentSecretView>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        let record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, &record.scope()).await?;
        let store = &self.deps.store;
        let request = store
            .vault_copy_requests(&sid)
            .await?
            .into_iter()
            .find(|c| c.secret_id == record.id)
            .ok_or_else(|| {
                AppError::Conflict("no copy of this secret waits for this project".into())
            })?;
        let seen = parse_hosts(&answer.hosts).map_err(invalid)?;
        if answer.name.trim() != request.name || !same_hosts(&seen, &patterns(&request.hosts)) {
            return Err(AppError::Conflict(format!(
                "an agent changed the request meanwhile: reload to see the copy that waits ({} on \
                 {})",
                request.name,
                request.hosts.join(", ")
            )));
        }
        let actor = Self::vault_actor(ctx);
        let detail = format!(
            "{} → {} on {} (asked by {})",
            record.name,
            request.name,
            request.hosts.join(", "),
            request.requested_by
        );
        if answer.allow {
            // The copy is a project secret: as adding one on the Secrets page.
            self.require(&ctx.user, &sid, Action::Configure).await?;
            if let Some(status) = self.fresh_or_status(ctx).await? {
                return Ok(Revealed::ReauthRequired(status));
            }
            let source: AgentPayload = self.open_payload(&record)?;
            let payload = AgentPayload {
                username: source.username.clone(),
                value: source.value.clone(),
                totp: source.totp.clone(),
            };
            drop(source);
            let about = format!(
                "Copy of {} for agents (asked by {}{})",
                record.name,
                request.requested_by,
                if request.reason.is_empty() {
                    String::new()
                } else {
                    format!(": {}", request.reason)
                }
            );
            let origin = format!("copy of {}, allowed by {actor}", record.name);
            let new = NewAgentSecret {
                sandbox: &sid,
                scope: AgentScope::Project(sid.clone()),
                meta: Meta {
                    name: request.name.clone(),
                    hosts: request.hosts.clone(),
                    injection: Injection::parse(&record.injection, record.header_name.as_deref())
                        .unwrap_or(Injection::None),
                },
                description: description(Some(&about))
                    .unwrap_or_else(|_| about.chars().take(1000).collect()),
                payload: &payload,
                by: ctx.user.id.as_str(),
                actor: &actor,
                origin: Some(&origin),
                // Further hosts and `.env` copies wait for a person, as for the production
                // secret it copies.
                auto_allow: false,
                env_files: false,
            };
            let copy = self.new_agent_record(&new)?;
            if !store
                .vault_answer_copy_request(&request, Some(&copy), MAX_AGENT_SECRETS)
                .await?
            {
                return Err(AppError::Conflict(
                    "the request changed meanwhile: reload and answer again".into(),
                ));
            }
            self.vault.bump();
            self.audit_agent_created(&copy, &new).await?;
            self.vault_audit(&actor, "vault.agent.copy_allow", Some(id), &detail)
                .await?;
        } else {
            if !store
                .vault_answer_copy_request(&request, None, MAX_AGENT_SECRETS)
                .await?
            {
                return Err(AppError::Conflict(
                    "the request changed meanwhile: reload and answer again".into(),
                ));
            }
            self.vault_audit(&actor, "vault.agent.copy_dismiss", Some(id), &detail)
                .await?;
        }
        self.announce_request(&record);
        self.settle_secret_waits(Some(&sid)).await;
        let view = self
            .agent_secrets(ctx, sandbox)
            .await?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or(AppError::NotFound)?;
        Ok(Revealed::Values(view))
    }
}
