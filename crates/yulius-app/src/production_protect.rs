//! `production_protect` (SPEC-157, ADR-0129): an agent protects its project's production branch
//! through Yulius, which adds a ruleset only people bypass as the task's owner, when GitHub says
//! that person is a repository admin. The agent's own token stays without Administration
//! (ADR-0114); the tool only ever adds protection.

use serde_json::{Value, json};

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::onboarding::production_ruleset_fix;
use crate::ports::github::{RepoAccess, RulesetRequest};

/// The ruleset `production_protect` adds: Restrict updates, Restrict deletions and Block force
/// pushes on `branch`, the Repository admin role the only bypass actor; no pull-request rule, so
/// the repository keeps its own merge rules. Its name never collides with the
/// `yulius-production-<branch>` ruleset of a project Yulius created.
#[must_use]
pub fn people_only_ruleset(branch: &str) -> RulesetRequest {
    RulesetRequest {
        name: format!("yulius-people-only-{branch}"),
        branch: branch.to_owned(),
        require_pr: false,
        merge_methods: Vec::new(),
        restrict_updates: true,
    }
}

impl App {
    /// `production_protect`: adds [`people_only_ruleset`] to the sandbox's production branch as
    /// the task's owner, unless the agents' token already cannot write it.
    ///
    /// # Errors
    /// `Conflict` (naming the setting and `needs_you`) in a playground, when the owner has no
    /// usable GitHub sign-in or is not a repository admin, or when GitHub refuses the ruleset;
    /// store failures.
    pub(crate) async fn tool_production_protect(&self, ctx: &AgentContext) -> AppResult<Value> {
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        crate::playground::refuse_in_playground(&sandbox, "Protecting the production branch")?;
        let branch = crate::delivery::delivery_profile(&sandbox).production_branch;
        let repo = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name);
        let fix = production_ruleset_fix(&repo, &branch);
        let refused = |why: String| {
            AppError::Conflict(format!(
                "{why} Ask a person with `needs_you`, quoting the setting: {fix}"
            ))
        };
        if self.agents_cannot_write(&sandbox, &branch).await {
            return Ok(json!({
                "protected": true,
                "branch": branch,
                "added": null,
                "message": format!("`{branch}` is already protected: agents' GitHub token cannot push to, merge into or delete it. Nothing was changed."),
            }));
        }
        let owner = self
            .deps
            .store
            .user(&ctx.task.owner)
            .await?
            .ok_or_else(|| refused("This task's owner no longer exists.".into()))?;
        let token = self
            .person_github_token(&owner, "Protecting the production branch")
            .await
            .map_err(|err| {
                refused(format!(
                    "Yulius cannot act as this task's owner on GitHub ({err})."
                ))
            })?;
        let login = owner.github_login.clone().unwrap_or_default();
        match self
            .deps
            .github
            .repo_access(&token, &sandbox.repo_owner, &sandbox.repo_name)
            .await
        {
            Ok(RepoAccess::Access { permissions, .. }) if permissions.admin => {}
            Ok(_) => {
                return Err(refused(format!(
                    "{login}, this task's owner, is not a repository admin of {repo}, and only an \
                     admin may protect its branches."
                )));
            }
            Err(err) => {
                return Err(refused(format!(
                    "GitHub did not say whether {login} is a repository admin of {repo} ({err})."
                )));
            }
        }
        let ruleset = people_only_ruleset(&branch);
        self.deps
            .github
            .create_ruleset(&token, &sandbox.repo_owner, &sandbox.repo_name, &ruleset)
            .await
            .map_err(|err| {
                refused(format!(
                    "GitHub refused the ruleset ({err}); a private repository on a free plan has no rulesets."
                ))
            })?;
        let detail = format!("{repo}: ruleset {} on {branch} as {login}", ruleset.name);
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &ctx.actor(),
                "task.production_protect",
                Some(ctx.task.id.as_str()),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "production_protect audit failed");
        }
        let protected = self.agents_cannot_write(&sandbox, &branch).await;
        Ok(json!({
            "protected": protected,
            "branch": branch,
            "added": ruleset.name,
            "acted_as": login,
            "message": if protected {
                format!("Added the ruleset {} on {repo} as {login}: only repository admins update, delete or force-push `{branch}`. If you asked a person for it with needs_you, call needs_you with resolved: true.", ruleset.name)
            } else {
                format!("Added the ruleset {} on {repo} as {login}, but GitHub does not yet show `{branch}` as closed to agents' token; check the rulesets at https://github.com/{repo}/settings/rules and ask a person with needs_you if it stays open.", ruleset.name)
            },
        }))
    }

    /// Whether GitHub says agents' token can neither update nor delete `branch` (ADR-0114); a
    /// check that fails is never taken as protected.
    async fn agents_cannot_write(&self, sandbox: &SandboxRecord, branch: &str) -> bool {
        let checked = match self.agent_github_token(sandbox).await {
            Ok(token) => {
                self.deps
                    .github
                    .branch_guard(&token, &sandbox.repo_owner, &sandbox.repo_name, branch)
                    .await
            }
            Err(err) => Err(err),
        };
        match checked {
            Ok(guard) => guard.is_guarded(),
            Err(err) => {
                tracing::info!(sandbox = %sandbox.id, error = %err, "checking the production branch failed");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ruleset_restricts_updates_without_imposing_pull_requests() {
        let ruleset = people_only_ruleset("main");
        assert_eq!(ruleset.name, "yulius-people-only-main");
        assert_eq!(ruleset.branch, "main");
        assert!(ruleset.restrict_updates);
        assert!(!ruleset.require_pr);
        assert!(ruleset.merge_methods.is_empty());
    }
}
