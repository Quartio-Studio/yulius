//! A person's API tokens for their own agents and scripts (SPEC-247 RMT-001..003, ADR-0188):
//! created on purpose with a scope, shown once, stored hashed, listed with their last use and
//! revoked at once.

use yulius_contracts::api::auth::{ApiTokenView, CreateApiToken, CreatedApiToken};
use yulius_domain::permission::TokenScope;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::ApiTokenRecord;

/// Live tokens a person may hold.
pub const MAX_TOKENS_PER_USER: usize = 50;
/// Characters of a token's label.
pub const MAX_LABEL_CHARS: usize = 80;
/// How often a token's last use is written at most.
const LAST_USE_RESOLUTION_MS: i64 = 60_000;

/// The view of a stored token.
fn view(record: &ApiTokenRecord) -> ApiTokenView {
    ApiTokenView {
        id: record.id.clone(),
        label: record.label.clone(),
        scope: record.scope.as_str().to_owned(),
        created_at: rfc3339(record.created_at),
        last_used_at: record.last_used_at.map(rfc3339),
    }
}

/// The `claude mcp add` command for `token` on the instance at `public_url`.
#[must_use]
pub fn claude_command(public_url: &str, token: &str) -> String {
    format!(
        "claude mcp add --transport http yulius {} --header \"Authorization: Bearer {token}\"",
        mcp_url(public_url)
    )
}

/// The remote MCP endpoint of the instance at `public_url`.
#[must_use]
pub fn mcp_url(public_url: &str) -> String {
    format!("{}/mcp", public_url.trim_end_matches('/'))
}

impl App {
    /// `GET /api/me/tokens`: the caller's live tokens, newest first.
    ///
    /// # Errors
    /// Store failures.
    pub async fn api_tokens(&self, ctx: &AuthContext) -> AppResult<Vec<ApiTokenView>> {
        let tokens = self.deps.store.api_tokens_of(&ctx.user.id).await?;
        Ok(tokens.iter().map(view).collect())
    }

    /// `POST /api/me/tokens`: a new `read` or `operate` token of the caller, returned once.
    /// Only a person in the browser makes one: a token cannot mint tokens.
    ///
    /// # Errors
    /// `Forbidden` for a bearer request, `Invalid` label or scope, `Capacity` past
    /// [`MAX_TOKENS_PER_USER`]; store failures.
    pub async fn create_api_token(
        &self,
        ctx: &AuthContext,
        req: &CreateApiToken,
    ) -> AppResult<CreatedApiToken> {
        if !ctx.is_browser() {
            return Err(AppError::Forbidden(
                "API tokens are made on Settings in the browser".into(),
            ));
        }
        let label = req.label.trim();
        if label.is_empty()
            || label.chars().count() > MAX_LABEL_CHARS
            || label.chars().any(char::is_control)
        {
            return Err(AppError::Invalid(format!(
                "a label of 1-{MAX_LABEL_CHARS} characters is required"
            )));
        }
        let scope = match TokenScope::parse(req.scope.trim()) {
            Some(scope @ (TokenScope::Read | TokenScope::Operate)) => scope,
            _ => {
                return Err(AppError::Invalid("scope must be operate or read".into()));
            }
        };
        if self.deps.store.api_tokens_of(&ctx.user.id).await?.len() >= MAX_TOKENS_PER_USER {
            return Err(AppError::Capacity(format!(
                "you hold {MAX_TOKENS_PER_USER} tokens; revoke one first"
            )));
        }
        let (token, record) = self.insert_token(&ctx.user, label, scope).await?;
        let login = self.login_of(&ctx.user.id).await;
        self.deps
            .store
            .audit(
                self.now(),
                &login,
                "api_token.create",
                Some(&record.id),
                Some(&format!("{label} ({})", scope.as_str())),
            )
            .await?;
        let public_url = &self.config.public_url;
        Ok(CreatedApiToken {
            claude_command: claude_command(public_url, token.expose()),
            cli_command: format!("yulius remote login {public_url}"),
            mcp_url: mcp_url(public_url),
            token: token.expose().to_owned(),
            view: view(&record),
        })
    }

    /// `DELETE /api/me/tokens/{id}`: revokes one of the caller's tokens at once, from their
    /// browser or any of their tokens that may change things (a lost laptop's from another one).
    ///
    /// # Errors
    /// `NotFound` for an id that is not a live token of the caller; store failures.
    pub async fn revoke_api_token(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let tokens = self.deps.store.api_tokens_of(&ctx.user.id).await?;
        let record = tokens
            .iter()
            .find(|t| t.id == id)
            .ok_or(AppError::NotFound)?;
        if !self
            .deps
            .store
            .revoke_api_token(&ctx.user.id, id, self.now())
            .await?
        {
            return Err(AppError::NotFound);
        }
        let login = self.login_of(&ctx.user.id).await;
        self.deps
            .store
            .audit(
                self.now(),
                &login,
                "api_token.revoke",
                Some(id),
                Some(&format!("{} ({})", record.label, record.scope.as_str())),
            )
            .await?;
        Ok(())
    }

    /// Records a token's use, at most once a minute; a failed write is logged, never refused.
    pub(crate) async fn note_token_use(&self, record: &ApiTokenRecord) {
        let now = self.now();
        if record
            .last_used_at
            .is_some_and(|at| now.0 - at.0 < LAST_USE_RESOLUTION_MS)
        {
            return;
        }
        if let Err(err) = self.deps.store.touch_api_token(&record.id, now).await {
            tracing::warn!(error = %err, "recording an API token's use failed");
        }
    }
}
