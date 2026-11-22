//! Pull requests, checks, merges, fast-forwards and comparisons (GIT-001/002, DLV).

use std::time::Duration;

use reqwest::Method;
use tokio::time::Instant;

use crate::client::{GitHubClient, decode};
use crate::config::RetryPolicy;
use crate::error::GitHubError;
use crate::http::{Call, backoff, upstream_message};
use crate::secret::Secret;
use crate::types::{
    CheckSource, ChecksReport, CompareStatus, Comparison, MergeMethod, MergeOutcome, NewPull,
    PullFilter, PullRequest,
};
use crate::validate;
use crate::wire::{
    CheckReads, WireCheckRun, WireCheckRuns, WireCombinedStatus, WireCompare, WireMerge,
    WireMergeability, WirePull, WireWorkflowRuns, summarize,
};

/// Most check-run pages read for one commit.
const MAX_CHECK_PAGES: u32 = 5;

/// Most merge requests one [`GitHubClient::merge_pull`] sends while GitHub answers 405 for a pull
/// request it has not finished checking (SPEC-495 MRG-002).
pub const MERGE_ATTEMPTS: u32 = 4;

/// Most reads of a pull request per wait for GitHub to compute its mergeability (MRG-001).
const MERGEABILITY_READS: u32 = 6;

/// Longest a merge waits for GitHub, in all its sleeps (MRG-002): well below the 30 s after
/// which the CD page gives up on a deploy request, whatever the client's retry budget.
const MERGE_WAIT: Duration = Duration::from_secs(12);

/// The budget of one merge's waits: the client's retry budget, at most [`MERGE_WAIT`].
fn merge_budget(policy: &RetryPolicy) -> Duration {
    policy.max_total.min(MERGE_WAIT)
}

/// Whether a pull request GitHub refused with a transient 405 is worth merging again: open, not
/// a draft and mergeable (MRG-002). `blocked` and `behind` do not stop it: GitHub reads them for
/// a caller who may bypass the rule too (a repository admin's promotion past Yulius's production
/// ruleset), and a merge that is really blocked ends at its next, non-transient 405.
fn worth_retrying(pull: Option<&WireMergeability>) -> bool {
    pull.is_some_and(|p| {
        p.state == "open"
            && p.mergeable == Some(true)
            && p.draft != Some(true)
            && p.mergeable_state.as_deref() != Some("dirty")
    })
}

/// GitHub's 405 for a merge it could not judge yet: right after the pull request was opened or
/// its base moved, while `mergeable` is still being computed.
fn is_base_modified(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("base branch was modified")
}

/// The message of a merge GitHub refused (405): the reason read from the pull request, when it
/// names one, then GitHub's own words (MRG-003).
fn refusal(
    pull: Option<&WireMergeability>,
    github: &str,
    attempts: u32,
    transient: bool,
) -> String {
    let reason = pull.and_then(|p| {
        if p.merged == Some(true) || p.merged_at.is_some() {
            Some("it is already merged".to_owned())
        } else if p.state != "open" {
            Some("it is closed".to_owned())
        } else if p.draft == Some(true) {
            Some("it is a draft".to_owned())
        } else if p.mergeable == Some(false) || p.mergeable_state.as_deref() == Some("dirty") {
            Some(format!("it conflicts with '{}'", p.base()))
        } else if p.mergeable.is_none() {
            Some("GitHub had not finished checking whether it can merge".to_owned())
        } else {
            // A transient last refusal is GitHub's, not the rule's: a caller who may bypass a
            // rule reads `blocked` too, and a real block ends on a non-transient 405.
            match p.mergeable_state.as_deref() {
                _ if transient => {
                    let plural = if attempts == 1 { "" } else { "s" };
                    Some(format!(
                        "GitHub still refused it after {attempts} attempt{plural}"
                    ))
                }
                Some("blocked") => Some(
                    "branch protection or a ruleset blocks it (required reviews or checks, or restricted updates)".to_owned(),
                ),
                Some("behind") => Some(format!(
                    "it is behind '{}', which branch protection requires it to contain",
                    p.base()
                )),
                _ => None,
            }
        }
    });
    match reason {
        Some(reason) => format!("{reason} (GitHub: {github})"),
        None => github.to_owned(),
    }
}

/// Whether a 403 means the App lacks the permission for the endpoint (GitHub's wording for App
/// installation and user-to-server tokens), as opposed to another refusal.
fn missing_permission(err: &GitHubError) -> bool {
    matches!(err, GitHubError::Forbidden { message, .. }
        if message.to_ascii_lowercase().contains("not accessible by integration"))
}

/// A source's read: its value, `None` (recorded in `unreadable`) when the App may not read it,
/// or the error.
fn readable<T>(
    read: Result<T, GitHubError>,
    source: CheckSource,
    unreadable: &mut Vec<CheckSource>,
) -> Result<Option<T>, GitHubError> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(err) if missing_permission(&err) => {
            unreadable.push(source);
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn repo_args(owner: &str, repo: &str) -> Result<(), GitHubError> {
    validate::owner(owner)?;
    validate::repo_name(repo)
}

/// A pull request title GitHub accepts: not blank, at most 256 bytes.
fn pull_title(title: &str) -> Result<(), GitHubError> {
    if title.trim().is_empty() || title.len() > 256 {
        return Err(GitHubError::InvalidInput {
            what: "pull request title",
            reason: "empty or longer than 256 bytes".into(),
        });
    }
    Ok(())
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
        pull_title(&pull.title)?;
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

    /// Confirms a contradictory display observation against the numbered resource.
    ///
    /// # Errors
    /// Reports a display pause, inaccessible pull or network error.
    pub async fn confirm_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, GitHubError> {
        repo_args(owner, repo)?;
        let url = self.api_url(&["repos", owner, repo, "pulls", &number.to_string()])?;
        let call = Call::new(Method::GET, url, Some(token.expose()));
        let reply = self
            .inner
            .display
            .read_after(call, &self.inner.transport, Some(std::time::Instant::now()))
            .await?;
        let wire: WirePull = decode(&reply)?;
        Ok(wire.into())
    }

    /// Sets the title of pull request `number`; its body, base and state stay.
    ///
    /// # Errors
    /// [`GitHubError::InvalidInput`] for an empty title or one longer than 256 bytes,
    /// [`GitHubError::NotFound`] or other typed errors.
    pub async fn update_pull_title(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
        title: &str,
    ) -> Result<PullRequest, GitHubError> {
        repo_args(owner, repo)?;
        pull_title(title)?;
        let number = number.to_string();
        let url = self.api_url(&["repos", owner, repo, "pulls", &number])?;
        let body = serde_json::json!({ "title": title });
        let wire: WirePull = self
            .send_json(Method::PATCH, url, Some(token), body)
            .await?;
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

    /// Aggregate CI state of commit `sha`: check runs plus the combined commit status, each read
    /// on its own. A source the App may not read (HTTP 403 "Resource not accessible by
    /// integration") is reported in [`ChecksReport::unreadable`] instead of failing the read;
    /// without check runs, the commit's GitHub Actions workflow runs stand in for them. See
    /// [`crate::ChecksSummary`] for how unreadable sources affect the result: never green.
    ///
    /// # Errors
    /// Typed GitHub errors other than a missing permission (rate limits, 5xx, 401, 404, …).
    pub async fn check_status(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<ChecksReport, GitHubError> {
        repo_args(owner, repo)?;
        validate::sha(sha)?;
        let mut reads = CheckReads::default();
        reads.check_runs = readable(
            self.check_runs(token, owner, repo, sha).await,
            CheckSource::CheckRuns,
            &mut reads.unreadable,
        )?;
        let mut url = self.api_url(&["repos", owner, repo, "commits", sha, "status"])?;
        url.query_pairs_mut().append_pair("per_page", "100");
        let combined: Result<WireCombinedStatus, GitHubError> =
            self.get_json(url, Some(token)).await;
        reads.statuses = readable(
            combined.map(|c| c.statuses),
            CheckSource::CommitStatuses,
            &mut reads.unreadable,
        )?;
        if reads.check_runs.is_none() {
            reads.workflow_runs = readable(
                self.workflow_runs(token, owner, repo, sha).await,
                CheckSource::WorkflowRuns,
                &mut reads.unreadable,
            )?;
        }
        Ok(summarize(reads))
    }

    /// The check runs of `sha` (at most [`MAX_CHECK_PAGES`] pages).
    async fn check_runs(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<WireCheckRun>, GitHubError> {
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
        Ok(runs)
    }

    /// The GitHub Actions workflow runs whose head is `sha` (at most [`MAX_CHECK_PAGES`] pages).
    async fn workflow_runs(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<WireCheckRun>, GitHubError> {
        let mut runs: Vec<WireCheckRun> = Vec::new();
        for page in 1..=MAX_CHECK_PAGES {
            let mut url = self.api_url(&["repos", owner, repo, "actions", "runs"])?;
            url.query_pairs_mut()
                .append_pair("head_sha", sha)
                .append_pair("per_page", "100")
                .append_pair("page", &page.to_string());
            let batch: WireWorkflowRuns = self.get_json(url, Some(token)).await?;
            let fetched = batch.workflow_runs.len();
            runs.extend(batch.workflow_runs);
            if fetched < 100 || runs.len() as u64 >= batch.total_count {
                break;
            }
        }
        Ok(runs)
    }

    /// Merges pull request `number` only if its head is still `expected_head_sha`
    /// (GIT-002: stale heads are refused by GitHub with 409).
    ///
    /// A 405 is not taken at its word at once (SPEC-495): right after a pull request was opened
    /// or its base moved GitHub answers "Base branch was modified" while it computes the
    /// mergeability. The pull request is read until GitHub has computed it, and a merge GitHub
    /// then reports possible is sent again, with doubling backoff, at most [`MERGE_ATTEMPTS`]
    /// merge requests in all. Its sleeps end within the retry policy's `max_total`, at most 12 s;
    /// each request has its own timeout (and each read its GET retries) on top. A merge that
    /// succeeds at once sends one request.
    ///
    /// # Errors
    /// [`GitHubError::Conflict`] when the head moved, [`GitHubError::NotMergeable`] (405) when
    /// GitHub refuses the merge (its message names the reason: a conflict, branch protection, a
    /// base the branch must contain, a draft, a closed pull request, a mergeability GitHub did
    /// not finish computing), or other typed errors.
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
        let policy = self.inner.config.retry;
        let deadline = Instant::now() + merge_budget(&policy);
        let mut attempt = 0_u32;
        loop {
            attempt += 1;
            let call =
                Call::new(Method::PUT, url.clone(), Some(token.expose())).json(serde_json::json!({
                    "merge_method": method.as_str(),
                    "sha": expected_head_sha,
                }));
            let reply = self.send_any(&call).await?;
            if reply.status != 405 {
                if !reply.is_success() {
                    return Err(self.error_for(&reply, &call));
                }
                let wire: WireMerge = decode(&reply)?;
                return Ok(MergeOutcome { sha: wire.sha });
            }
            let github = upstream_message(&reply.body, &call.known_secrets());
            let read = self
                .settled_mergeability(token, owner, repo, &number, deadline, policy.base_delay)
                .await;
            let (pull, computing) = match &read {
                Some((pull, computing)) => (Some(pull), *computing),
                None => (None, false),
            };
            let transient = computing || is_base_modified(&github);
            let delay = backoff(policy.base_delay, attempt);
            if transient
                && worth_retrying(pull)
                && attempt < MERGE_ATTEMPTS
                && Instant::now() + delay < deadline
            {
                tokio::time::sleep(delay).await;
                continue;
            }
            return Err(GitHubError::NotMergeable {
                message: refusal(pull, &github, attempt, transient),
            });
        }
    }

    /// Reads pull request `number` until GitHub has computed its mergeability (`mergeable` not
    /// `null`), at most [`MERGEABILITY_READS`] reads with doubling backoff, before `deadline`.
    /// Returns the last read and whether GitHub was still computing at the first one; `None`
    /// when the pull request could not be read (the caller keeps GitHub's refusal).
    async fn settled_mergeability(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: &str,
        deadline: Instant,
        base_delay: Duration,
    ) -> Option<(WireMergeability, bool)> {
        let url = self
            .api_url(&["repos", owner, repo, "pulls", number])
            .ok()?;
        let mut computing = false;
        let mut read = 0_u32;
        loop {
            read += 1;
            let pull: WireMergeability = self.get_json(url.clone(), Some(token)).await.ok()?;
            if pull.mergeable.is_some() || pull.state != "open" {
                return Some((pull, computing));
            }
            computing = true;
            let delay = backoff(base_delay, read);
            if read >= MERGEABILITY_READS || Instant::now() + delay >= deadline {
                return Some((pull, computing));
            }
            tokio::time::sleep(delay).await;
        }
    }

    /// Creates a new branch at an exact SHA. An existing ref is never updated.
    ///
    /// # Errors
    /// Input validation, a GitHub refusal (including an existing ref), or transport failure.
    pub async fn create_branch_ref(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
        sha: &str,
    ) -> Result<(), GitHubError> {
        repo_args(owner, repo)?;
        validate::branch(branch)?;
        validate::sha(sha)?;
        let url = self.api_url(&["repos", owner, repo, "git", "refs"])?;
        let call = Call::new(Method::POST, url, Some(token.expose()))
            .json(serde_json::json!({"ref": format!("refs/heads/{branch}"), "sha": sha}));
        let reply = self.send_any(&call).await?;
        if reply.is_success() {
            Ok(())
        } else {
            Err(self.error_for(&reply, &call))
        }
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        MERGE_WAIT, WireMergeability, is_base_modified, merge_budget, refusal, worth_retrying,
    };

    fn pull(fields: serde_json::Value) -> WireMergeability {
        let mut value = serde_json::json!({
            "state": "open", "mergeable": true, "mergeable_state": "clean",
            "base": {"ref": "main", "sha": "0".repeat(40)},
        });
        if let (Some(base), Some(extra)) = (value.as_object_mut(), fields.as_object()) {
            for (key, field) in extra {
                base.insert(key.clone(), field.clone());
            }
        }
        serde_json::from_value(value).unwrap_or_else(|err| panic!("{err}"))
    }

    /// SPEC-495 MRG-003: a refusal says which kind it is, then GitHub's words.
    #[test]
    fn a_refusal_names_its_reason() {
        let github = "Pull Request is not mergeable";
        for (fields, reason) in [
            (
                serde_json::json!({"mergeable": false, "mergeable_state": "dirty"}),
                "it conflicts with 'main'",
            ),
            (
                serde_json::json!({"mergeable_state": "blocked"}),
                "branch protection or a ruleset blocks it (required reviews or checks, or restricted updates)",
            ),
            (
                serde_json::json!({"mergeable_state": "behind"}),
                "it is behind 'main', which branch protection requires it to contain",
            ),
            (
                serde_json::json!({"draft": true, "mergeable_state": "draft"}),
                "it is a draft",
            ),
            (serde_json::json!({"state": "closed"}), "it is closed"),
            (
                serde_json::json!({"state": "closed", "merged": true}),
                "it is already merged",
            ),
            (
                serde_json::json!({"mergeable": null, "mergeable_state": "unknown"}),
                "GitHub had not finished checking whether it can merge",
            ),
        ] {
            assert_eq!(
                refusal(Some(&pull(fields)), github, 1, false),
                format!("{reason} (GitHub: {github})")
            );
        }
        // A transient refusal that outlived its retries says so.
        assert_eq!(
            refusal(
                Some(&pull(serde_json::json!({}))),
                "Base branch was modified.",
                4,
                true
            ),
            "GitHub still refused it after 4 attempts (GitHub: Base branch was modified.)"
        );
        // A transient last refusal does not blame a rule the caller may bypass.
        assert_eq!(
            refusal(
                Some(&pull(serde_json::json!({"mergeable_state": "blocked"}))),
                "Base branch was modified.",
                4,
                true
            ),
            "GitHub still refused it after 4 attempts (GitHub: Base branch was modified.)"
        );
        // One attempt is one attempt.
        assert_eq!(
            refusal(
                Some(&pull(serde_json::json!({}))),
                "Base branch was modified.",
                1,
                true
            ),
            "GitHub still refused it after 1 attempt (GitHub: Base branch was modified.)"
        );
        // No reason to add: GitHub's words alone.
        assert_eq!(
            refusal(Some(&pull(serde_json::json!({}))), github, 1, false),
            github
        );
        assert_eq!(refusal(None, github, 1, true), github);
    }

    /// A transient 405 is retried on a pull request GitHub reports open, mergeable and not a
    /// draft (MRG-002). `blocked` and `behind` stay retried: a caller who may bypass the rule (an
    /// admin's promotion) reads them too, and a really blocked merge ends at its next 405.
    #[test]
    fn only_a_mergeable_pull_request_is_merged_again() {
        for state in ["clean", "unstable", "blocked", "behind", "has_hooks"] {
            assert!(
                worth_retrying(Some(&pull(serde_json::json!({"mergeable_state": state})))),
                "{state}"
            );
        }
        for fields in [
            serde_json::json!({"draft": true}),
            serde_json::json!({"mergeable": false, "mergeable_state": "dirty"}),
            serde_json::json!({"mergeable": null, "mergeable_state": "unknown"}),
            serde_json::json!({"state": "closed"}),
        ] {
            assert!(!worth_retrying(Some(&pull(fields.clone()))), "{fields}");
        }
        assert!(!worth_retrying(None));
    }

    /// The merge's waits end well before the CD page gives up on its request (30 s), whatever
    /// the client's retry budget (MRG-002).
    #[test]
    fn the_merge_wait_is_capped_below_the_client_budget() {
        let policy = |secs| crate::config::RetryPolicy {
            max_total: Duration::from_secs(secs),
            ..crate::config::RetryPolicy::default()
        };
        assert_eq!(merge_budget(&policy(30)), MERGE_WAIT);
        assert_eq!(merge_budget(&policy(5)), Duration::from_secs(5));
        assert!(MERGE_WAIT <= Duration::from_secs(12));
    }

    #[test]
    fn only_base_branch_was_modified_reads_as_transient() {
        assert!(is_base_modified(
            "Base branch was modified. Review and try the merge again."
        ));
        assert!(!is_base_modified(
            "Head branch was modified. Review and try the merge again."
        ));
        assert!(!is_base_modified("Pull Request is not mergeable"));
    }
}
