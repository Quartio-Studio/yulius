//! Repository permissions, creation, lookup and branch rulesets.

use reqwest::Method;

use crate::client::GitHubClient;
use crate::error::GitHubError;
use crate::http::Call;
use crate::secret::Secret;
use crate::types::{BranchGuard, CreateRepo, Repo, RepoAccess, RulesetSpec};
use crate::validate;
use crate::wire::{WireBranchRule, WireId, WireRepo, WireRuleset};

/// The repository admin role, as rulesets name it among bypass actors.
const REPOSITORY_ADMIN_ROLE: u64 = 5;

impl GitHubClient {
    /// The user's permission on `owner/repo` (SPEC-001 ACC-007). GitHub's 404 (missing or
    /// invisible) is [`RepoAccess::NoAccess`], not an error.
    ///
    /// # Errors
    /// [`GitHubError::Unauthorized`] for a bad token, or other typed errors.
    pub async fn repo_permissions(
        &self,
        user_token: &Secret,
        owner: &str,
        repo: &str,
    ) -> Result<RepoAccess, GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        let url = self.api_url(&["repos", owner, repo])?;
        let call = Call::new(Method::GET, url, Some(user_token.expose()));
        let reply = self.send_any(&call).await?;
        if reply.status == 404 {
            return Ok(RepoAccess::NoAccess);
        }
        if !reply.is_success() {
            return Err(self.error_for(&reply, &call));
        }
        let wire: WireRepo = crate::client::decode(&reply)?;
        let (repo, permissions) = wire.into_parts();
        Ok(RepoAccess::Granted {
            // Fail closed: missing flags grant nothing.
            permissions: permissions.unwrap_or_default(),
            repo,
        })
    }

    /// Creates a repository in organization `org` with an installation token.
    ///
    /// # Errors
    /// [`GitHubError::Invalid`] when the name is taken, or other typed errors.
    pub async fn create_org_repo(
        &self,
        installation_token: &Secret,
        org: &str,
        spec: &CreateRepo,
    ) -> Result<Repo, GitHubError> {
        validate::owner(org)?;
        let url = self.api_url(&["orgs", org, "repos"])?;
        self.create_repo(installation_token, url, spec).await
    }

    /// Creates a repository for the user who owns `user_token` (personal accounts, ADR-08).
    ///
    /// # Errors
    /// [`GitHubError::Invalid`] when the name is taken, or other typed errors.
    pub async fn create_user_repo(
        &self,
        user_token: &Secret,
        spec: &CreateRepo,
    ) -> Result<Repo, GitHubError> {
        let url = self.api_url(&["user", "repos"])?;
        self.create_repo(user_token, url, spec).await
    }

    async fn create_repo(
        &self,
        token: &Secret,
        url: reqwest::Url,
        spec: &CreateRepo,
    ) -> Result<Repo, GitHubError> {
        validate::repo_name(&spec.name)?;
        let mut body = serde_json::json!({
            "name": spec.name,
            "private": spec.private,
            "auto_init": spec.auto_init,
        });
        if let Some(description) = &spec.description {
            body["description"] = serde_json::Value::String(description.clone());
        }
        let wire: WireRepo = self.send_json(Method::POST, url, Some(token), body).await?;
        Ok(wire.into_repo())
    }

    /// Repository metadata.
    ///
    /// # Errors
    /// [`GitHubError::NotFound`] or other typed errors.
    pub async fn get_repo(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
    ) -> Result<Repo, GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        let url = self.api_url(&["repos", owner, repo])?;
        let wire: WireRepo = self.get_json(url, Some(token)).await?;
        Ok(wire.into_repo())
    }

    /// Creates an active branch ruleset on `refs/heads/<branch>` and returns its id. Private
    /// repositories need a paid plan for rulesets; GitHub then answers 403/422 (typed).
    ///
    /// # Errors
    /// Typed GitHub errors.
    pub async fn create_branch_ruleset(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        spec: &RulesetSpec,
    ) -> Result<u64, GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        validate::branch(&spec.branch)?;
        if spec.name.trim().is_empty() || spec.name.len() > 100 {
            return Err(GitHubError::InvalidInput {
                what: "ruleset name",
                reason: "empty or longer than 100 bytes".into(),
            });
        }
        let mut rules = Vec::new();
        if spec.require_pr {
            let mut parameters = serde_json::json!({
                "required_approving_review_count": 0,
                "dismiss_stale_reviews_on_push": false,
                "require_code_owner_review": false,
                "require_last_push_approval": false,
                "required_review_thread_resolution": false
            });
            if !spec.allowed_merge_methods.is_empty() {
                parameters["allowed_merge_methods"] = spec
                    .allowed_merge_methods
                    .iter()
                    .map(|m| serde_json::Value::String(m.as_str().to_owned()))
                    .collect();
            }
            rules.push(serde_json::json!({"type": "pull_request", "parameters": parameters}));
        }
        if spec.block_force_push {
            rules.push(serde_json::json!({"type": "non_fast_forward"}));
        }
        if spec.block_deletion {
            rules.push(serde_json::json!({"type": "deletion"}));
        }
        if spec.restrict_updates {
            rules.push(serde_json::json!({"type": "update"}));
        }
        let mut body = serde_json::json!({
            "name": spec.name,
            "target": "branch",
            "enforcement": "active",
            "conditions": {"ref_name": {"include": [format!("refs/heads/{}", spec.branch)], "exclude": []}},
            "rules": rules,
        });
        if spec.restrict_updates {
            body["bypass_actors"] = serde_json::json!([{
                "actor_id": REPOSITORY_ADMIN_ROLE,
                "actor_type": "RepositoryRole",
                "bypass_mode": "always"
            }]);
        }
        let url = self.api_url(&["repos", owner, repo, "rulesets"])?;
        let wire: WireId = self.send_json(Method::POST, url, Some(token), body).await?;
        Ok(wire.id)
    }

    /// Whether `token` could write `branch` (ADR-0114): the rules GitHub applies to it, and for
    /// each `update` or `deletion` rule whether its ruleset lets this token bypass it
    /// (`current_user_can_bypass`; anything but `never` counts as a bypass).
    ///
    /// # Errors
    /// Typed GitHub errors: a rule or ruleset that cannot be read is never "guarded".
    pub async fn branch_guard(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<BranchGuard, GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        validate::branch(branch)?;
        let mut segments = vec!["repos", owner, repo, "rules", "branches"];
        segments.extend(branch.split('/'));
        let url = self.api_url(&segments)?;
        let rules = self
            .get_pages(url, Some(token), |value| {
                serde_json::from_value::<Vec<WireBranchRule>>(value)
                    .map_err(|e| GitHubError::Decode(e.to_string()))
            })
            .await?;
        let mut never: std::collections::HashMap<u64, bool> = std::collections::HashMap::new();
        let mut guard = BranchGuard::default();
        for rule in rules {
            let Some(id) = rule.ruleset_id else { continue };
            if rule.kind != "update" && rule.kind != "deletion" {
                continue;
            }
            let binding = if let Some(known) = never.get(&id) {
                *known
            } else {
                let id_text = id.to_string();
                let mut url = self.api_url(&["repos", owner, repo, "rulesets", &id_text])?;
                url.query_pairs_mut()
                    .append_pair("includes_parents", "true");
                let ruleset: WireRuleset = self.get_json(url, Some(token)).await?;
                let binds = ruleset.current_user_can_bypass.as_deref() == Some("never");
                never.insert(id, binds);
                binds
            };
            if binding {
                match rule.kind.as_str() {
                    "update" => guard.updates_restricted = true,
                    _ => guard.deletion_restricted = true,
                }
            }
        }
        Ok(guard)
    }
}
