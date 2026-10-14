//! Agent secrets copied into a project's `.env` files (SPEC-129, ADR-0105). A project keeps its
//! keys in a git-ignored `.env`, named without values in the tracked `.env.example`. When the
//! project's own code needs a secret's real value (it runs outside the egress proxy), the agent
//! asks Yulius to copy it there: the agent only ever sees `[secret:NAME]`, so it cannot write the
//! value itself. A value in a file leaves the secret's bound hosts, so a person who manages the
//! secret allows each secret once, as they allow its hosts (ADR-0098).

use yulius_contracts::api::secrets::{AgentSecretView, EnvFilesAnswer};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::EnvFiles;

impl App {
    /// `POST /api/agent-secrets/{id}/env`: a person who manages the secret allows agents to copy
    /// it into `.env` files (answering their request), dismisses the request, or revokes the
    /// allowance. Compare and set on what was read, so a concurrent answer is never undone.
    ///
    /// # Errors
    /// `NotFound` for invisible secrets, `Forbidden` for people who cannot manage it,
    /// `Conflict` when nothing waits to be allowed or dismissed (or it changed meanwhile).
    pub async fn answer_env_files(
        &self,
        ctx: &AuthContext,
        id: &str,
        answer: &EnvFilesAnswer,
    ) -> AppResult<AgentSecretView> {
        let mut record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        let action = match (record.env_files, answer.allow) {
            (EnvFiles::Asked, true) => "vault.agent.env_allow",
            (EnvFiles::Asked, false) => "vault.agent.env_dismiss",
            (EnvFiles::Allowed, false) => "vault.agent.env_revoke",
            (EnvFiles::Allowed, true) => return self.agent_view(&record, true).await,
            (EnvFiles::No, _) => {
                return Err(AppError::Conflict(
                    "no agent asks to copy this secret into .env files".into(),
                ));
            }
        };
        let to = if answer.allow {
            EnvFiles::Allowed
        } else {
            EnvFiles::No
        };
        let by = record.env_requested_by.take();
        if !self
            .deps
            .store
            .vault_set_env_files(id, (record.env_files, by.as_deref()), (to, None))
            .await?
        {
            return Err(AppError::Conflict(
                "the secret changed meanwhile: reload and answer again".into(),
            ));
        }
        record.env_files = to;
        self.vault_audit(
            &Self::vault_actor(ctx),
            action,
            Some(id),
            &match by {
                Some(by) => format!("{} → .env files (asked by {by})", record.name),
                None => format!("{} → .env files", record.name),
            },
        )
        .await?;
        self.agent_view(&record, true).await
    }
}
