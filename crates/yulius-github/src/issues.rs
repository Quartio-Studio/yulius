//! Issues and their labels (SPEC-001 DLV-008): agents log follow-up work, bugs and flaky tests
//! as issues with Definition-of-Ready labels.

use reqwest::Method;
use serde::Deserialize;

use crate::client::GitHubClient;
use crate::error::GitHubError;
use crate::secret::Secret;
use crate::types::{IssueRef, NewIssue};
use crate::validate;

/// Most labels on one new issue.
pub const MAX_ISSUE_LABELS: usize = 10;
/// Longest label name, in bytes (GitHub's limit).
pub const MAX_LABEL_BYTES: usize = 50;
/// Longest issue title, in bytes.
pub const MAX_ISSUE_TITLE_BYTES: usize = 256;
/// Largest issue body, in bytes.
pub const MAX_ISSUE_BODY_BYTES: usize = 64 * 1024;
/// Color of labels created for an issue (neutral grey).
const NEW_LABEL_COLOR: &str = "ededed";

#[derive(Deserialize)]
struct WireIssue {
    number: u64,
    html_url: String,
}

fn invalid(what: &'static str, reason: &str) -> GitHubError {
    GitHubError::InvalidInput {
        what,
        reason: reason.to_owned(),
    }
}

fn check_issue(issue: &NewIssue) -> Result<(), GitHubError> {
    if issue.title.trim().is_empty() || issue.title.len() > MAX_ISSUE_TITLE_BYTES {
        return Err(invalid("issue title", "empty or longer than 256 bytes"));
    }
    if issue.body.len() > MAX_ISSUE_BODY_BYTES {
        return Err(invalid("issue body", "longer than 64 KiB"));
    }
    if issue.labels.len() > MAX_ISSUE_LABELS {
        return Err(invalid("issue labels", "more than 10"));
    }
    for label in &issue.labels {
        if label.trim().is_empty()
            || label.len() > MAX_LABEL_BYTES
            || label.chars().any(char::is_control)
        {
            return Err(invalid("issue label", "empty, too long or with control characters"));
        }
    }
    Ok(())
}

impl GitHubClient {
    /// Makes sure label `name` exists in the repository, creating it when missing. Returns
    /// whether it was created.
    ///
    /// # Errors
    /// Typed errors other than "already exists".
    pub async fn ensure_label(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        name: &str,
    ) -> Result<bool, GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        let url = self.api_url(&["repos", owner, repo, "labels", name])?;
        match self.get_json::<serde_json::Value>(url, Some(token)).await {
            Ok(_) => return Ok(false),
            Err(GitHubError::NotFound) => {}
            Err(err) => return Err(err),
        }
        let url = self.api_url(&["repos", owner, repo, "labels"])?;
        let body = serde_json::json!({"name": name, "color": NEW_LABEL_COLOR});
        match self
            .send_json::<serde_json::Value>(Method::POST, url, Some(token), body)
            .await
        {
            Ok(_) => Ok(true),
            // Created meanwhile by someone else.
            Err(GitHubError::Invalid { .. }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Opens an issue with its labels, creating labels the repository does not have yet.
    ///
    /// # Errors
    /// [`GitHubError::InvalidInput`] for an empty or oversized title, body or label list, or
    /// other typed errors.
    pub async fn create_issue(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        issue: &NewIssue,
    ) -> Result<IssueRef, GitHubError> {
        validate::owner(owner)?;
        validate::repo_name(repo)?;
        check_issue(issue)?;
        for label in &issue.labels {
            self.ensure_label(token, owner, repo, label).await?;
        }
        let url = self.api_url(&["repos", owner, repo, "issues"])?;
        let body = serde_json::json!({
            "title": issue.title,
            "body": issue.body,
            "labels": issue.labels,
        });
        let wire: WireIssue = self
            .send_json(Method::POST, url, Some(token), body)
            .await?;
        Ok(IssueRef {
            number: wire.number,
            url: wire.html_url,
        })
    }
}
