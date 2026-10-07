//! Pull requests, checks, merges, fast-forwards and comparisons (GIT-001/002, DLV).

use reqwest::Method;

use crate::client::{GitHubClient, decode};
use crate::error::GitHubError;
use crate::http::{Call, upstream_message};
use crate::secret::Secret;
use crate::types::{
    ChecksSummary, CompareStatus, Comparison, MergeMethod, MergeOutcome, NewPull, PullFilter,
    PullRequest,
};
use crate::validate;
use crate::wire::{
    WireCheckRun, WireCheckRuns, WireCombinedStatus, WireCompare, WireMerge, WirePull,
};

/// Most check-run pages read for one commit.
const MAX_CHECK_PAGES: u32 = 5;

fn repo_args(owner: &str, repo: &str) -> Result<(), GitHubError> {
    validate::owner(owner)?;
    validate::repo_name(repo)
}

impl GitHubClient {
    /// Opens a pull request from `head` into `base` in the same repository.
    ///
    /// # Errors
    /// [`GitHubError::Invalid`] when one already exists or there is nothing to merge, or
    /// other typed errors.
    pub async fn create_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        pull: &NewPull,
    ) -> Result<PullRequest, GitHubError> {
        repo_args(owner, repo)?;
        validate::branch(&pull.head)?;
        validate::branch(&pull.base)?;
        if pull.title.trim().is_empty() || pull.title.len() > 256 {
            return Err(GitHubError::InvalidInput {
                what: "pull request title",
                reason: "empty or longer than 256 bytes".into(),
            });
        }
        let url = self.api_url(&["repos", owner, repo, "pulls"])?;
        let body = serde_json::json!({
            "title": pull.title,
            "head": pull.head,
            "base": pull.base,
            "body": pull.body,
        });
        let wire: WirePull = self.send_json(Method::POST, url, Some(token), body).await?;
        Ok(wire.into())
    }

    /// One pull request.
    ///
    /// # Errors
    /// [`GitHubError::NotFound`] or other typed errors.
    pub async fn get_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, GitHubError> {
        repo_args(owner, repo)?;
        let number = number.to_string();
        let url = self.api_url(&["repos", owner, repo, "pulls", &number])?;
        let wire: WirePull = self.get_json(url, Some(token)).await?;
        Ok(wire.into())
    }

    /// Pull requests by state, optionally only those whose head is branch `head` of `owner`.
    ///
    /// # Errors
    /// Typed GitHub errors.
    pub async fn list_pulls(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        state: PullFilter,
        head: Option<&str>,
    ) -> Result<Vec<PullRequest>, GitHubError> {
        repo_args(owner, repo)?;
        let mut url = self.api_url(&["repos", owner, repo, "pulls"])?;
        url.query_pairs_mut().append_pair("state", state.as_str());
        if let Some(branch) = head {
            validate::branch(branch)?;
            url.query_pairs_mut()
                .append_pair("head", &format!("{owner}:{branch}"));
        }
        self.get_pages(url, Some(token), |value| {
            serde_json::from_value::<Vec<WirePull>>(value)
                .map(|pulls| pulls.into_iter().map(PullRequest::from).collect())
                .map_err(|e| GitHubError::Decode(e.to_string()))
        })
        .await
    }

    /// Aggregate CI state of commit `sha`: check runs plus the combined commit status.
    ///
    /// # Errors
    /// Typed GitHub errors.
    pub async fn check_status(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<ChecksSummary, GitHubError> {
        repo_args(owner, repo)?;
        validate::sha(sha)?;
        let mut runs: Vec<WireCheckRun> = Vec::new();
        for page in 1..=MAX_CHECK_PAGES {
            let mut url = self.api_url(&["repos", owner, repo, "commits", sha, "check-runs"])?;
            url.query_pairs_mut()
                .append_pair("per_page", "100")
                .append_pair("page", &page.to_string());
            let batch: WireCheckRuns = self.get_json(url, Some(token)).await?;
            let fetched = batch.check_runs.len();
            runs.extend(batch.check_runs);
            if fetched < 100 || runs.len() as u64 >= batch.total_count {
                break;
            }
        }
        let mut url = self.api_url(&["repos", owner, repo, "commits", sha, "status"])?;
        url.query_pairs_mut().append_pair("per_page", "100");
        let combined: WireCombinedStatus = self.get_json(url, Some(token)).await?;
        Ok(crate::wire::aggregate_checks(&runs, &combined.statuses))
    }

    /// Merges pull request `number` only if its head is still `expected_head_sha`
    /// (GIT-002: stale heads are refused by GitHub with 409).
    ///
    /// # Errors
    /// [`GitHubError::Conflict`] when the head moved, [`GitHubError::NotMergeable`] (405) when
    /// GitHub refuses the merge, or other typed errors.
    pub async fn merge_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
        method: MergeMethod,
        expected_head_sha: &str,
    ) -> Result<MergeOutcome, GitHubError> {
        repo_args(owner, repo)?;
        validate::sha(expected_head_sha)?;
        let number = number.to_string();
        let url = self.api_url(&["repos", owner, repo, "pulls", &number, "merge"])?;
        let call = Call::new(Method::PUT, url, Some(token.expose())).json(serde_json::json!({
            "merge_method": method.as_str(),
            "sha": expected_head_sha,
        }));
        let reply = self.send_any(&call).await?;
        if reply.status == 405 {
            return Err(GitHubError::NotMergeable {
                message: upstream_message(&reply.body, &call.known_secrets()),
            });
        }
        if !reply.is_success() {
            return Err(self.error_for(&reply, &call));
        }
        let wire: WireMerge = decode(&reply)?;
        Ok(MergeOutcome { sha: wire.sha })
    }

    /// Moves `branch` to `to_sha` only as a fast-forward (`force: false`).
    ///
    /// # Errors
    /// [`GitHubError::NotFastForward`] when `to_sha` does not descend from the branch tip,
    /// [`GitHubError::Invalid`] for other 422s (e.g. missing ref), or other typed errors.
    pub async fn fast_forward_ref(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
        to_sha: &str,
    ) -> Result<(), GitHubError> {
        repo_args(owner, repo)?;
        validate::branch(branch)?;
        validate::sha(to_sha)?;
        let mut segments = vec!["repos", owner, repo, "git", "refs", "heads"];
        segments.extend(branch.split('/'));
        let url = self.api_url(&segments)?;
        let call = Call::new(Method::PATCH, url, Some(token.expose()))
            .json(serde_json::json!({"sha": to_sha, "force": false}));
        let reply = self.send_any(&call).await?;
        if reply.is_success() {
            return Ok(());
        }
        if reply.status == 422 {
            let message = upstream_message(&reply.body, &call.known_secrets());
            if message.to_ascii_lowercase().contains("fast forward")
                || message.to_ascii_lowercase().contains("fast-forward")
            {
                return Err(GitHubError::NotFastForward);
            }
            return Err(GitHubError::Invalid { message });
        }
        Err(self.error_for(&reply, &call))
    }

    /// Compares `base...head` (branch names or full commit ids).
    ///
    /// # Errors
    /// [`GitHubError::NotFound`] for unknown refs, or other typed errors.
    pub async fn compare(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<Comparison, GitHubError> {
        repo_args(owner, repo)?;
        validate::branch_or_sha(base)?;
        validate::branch_or_sha(head)?;
        let basehead = format!("{base}...{head}");
        let mut segments = vec!["repos", owner, repo, "compare"];
        segments.extend(basehead.split('/'));
        let mut url = self.api_url(&segments)?;
        url.query_pairs_mut().append_pair("per_page", "1");
        let wire: WireCompare = self.get_json(url, Some(token)).await?;
        let status = match wire.status.as_str() {
            "identical" => CompareStatus::Identical,
            "ahead" => CompareStatus::Ahead,
            "behind" => CompareStatus::Behind,
            "diverged" => CompareStatus::Diverged,
            other => {
                return Err(GitHubError::Decode(format!(
                    "unknown compare status {other:?}"
                )));
            }
        };
        Ok(Comparison {
            ahead_by: wire.ahead_by,
            behind_by: wire.behind_by,
            status,
        })
    }
}
