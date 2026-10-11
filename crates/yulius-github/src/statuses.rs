//! Commit statuses Yulius publishes for its own CI runs, and branch deletion for repository
//! hygiene (SPEC-001 DLV-006/007, SPEC-002 ADR-20).

use reqwest::Method;

use crate::client::GitHubClient;
use crate::error::GitHubError;
use crate::http::{Call, upstream_message};
use crate::secret::Secret;
use crate::validate;

/// Longest description GitHub accepts on a commit status.
pub const MAX_STATUS_DESCRIPTION_CHARS: usize = 140;
/// Longest status context accepted here.
pub const MAX_STATUS_CONTEXT_BYTES: usize = 100;

/// A commit status to publish (`POST /repos/{owner}/{repo}/statuses/{sha}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStatus {
    /// `pending`, `success`, `failure` or `error`.
    pub state: String,
    /// The status context, e.g. `yulius/ci`.
    pub context: String,
    /// Short description (clipped to [`MAX_STATUS_DESCRIPTION_CHARS`]).
    pub description: String,
    /// Link to the details.
    pub target_url: Option<String>,
}

impl GitHubClient {
    /// Publishes a commit status on `sha` (App permission "Commit statuses: write"). A newer
    /// status with the same context replaces the older one on GitHub. Not retried (a mutation);
    /// the caller retries later.
    ///
    /// # Errors
    /// [`GitHubError::InvalidInput`] for a bad state, context or URL; [`GitHubError::Forbidden`]
    /// when the App lacks the permission; other typed errors.
    pub async fn create_commit_status(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
        status: &CommitStatus,
    ) -> Result<(), GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        validate::sha(sha)?;
        if !matches!(
            status.state.as_str(),
            "pending" | "success" | "failure" | "error"
        ) {
            return Err(GitHubError::InvalidInput {
                what: "commit status state",
                reason: format!("unknown state {:?}", status.state),
            });
        }
        if status.context.is_empty()
            || status.context.len() > MAX_STATUS_CONTEXT_BYTES
            || status.context.chars().any(char::is_control)
        {
            return Err(GitHubError::InvalidInput {
                what: "commit status context",
                reason: "empty, too long or with control characters".into(),
            });
        }
        if let Some(url) = &status.target_url
            && !(url.starts_with("https://") || url.starts_with("http://"))
        {
            return Err(GitHubError::InvalidInput {
                what: "commit status target URL",
                reason: "not an http(s) URL".into(),
            });
        }
        let description: String = status
            .description
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_STATUS_DESCRIPTION_CHARS)
            .collect();
        let mut body = serde_json::json!({
            "state": status.state,
            "context": status.context,
            "description": description,
        });
        if let Some(url) = &status.target_url {
            body["target_url"] = serde_json::Value::String(url.clone());
        }
        let url = self.api_url(&["repos", owner, repo, "statuses", sha])?;
        let call = Call::new(Method::POST, url, Some(token.expose())).json(body);
        let reply = self.send_any(&call).await?;
        if reply.is_success() {
            Ok(())
        } else {
            Err(self.error_for(&reply, &call))
        }
    }

    /// Deletes branch `branch` (`DELETE /repos/{owner}/{repo}/git/refs/heads/{branch}`, App
    /// permission "Contents: write"). `Ok(false)` when the branch did not exist.
    ///
    /// # Errors
    /// [`GitHubError::Forbidden`] when protected or not permitted; other typed errors.
    pub async fn delete_branch(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<bool, GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        validate::branch(branch)?;
        let mut segments = vec!["repos", owner, repo, "git", "refs", "heads"];
        segments.extend(branch.split('/'));
        let url = self.api_url(&segments)?;
        let call = Call::new(Method::DELETE, url, Some(token.expose()));
        let reply = self.send_any(&call).await?;
        if reply.is_success() {
            return Ok(true);
        }
        if reply.status == 422 || reply.status == 404 {
            let message = upstream_message(&reply.body, &call.known_secrets());
            if reply.status == 404 || message.to_ascii_lowercase().contains("does not exist") {
                return Ok(false);
            }
        }
        Err(self.error_for(&reply, &call))
    }
}
