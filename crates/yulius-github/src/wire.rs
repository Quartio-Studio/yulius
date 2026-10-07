//! Private GitHub wire types and their conversion to adapter types.

use serde::Deserialize;
use yulius_domain::permission::GitHubPermissions;

use crate::types::{ChecksSummary, PullRequest, PullState, Repo};

#[derive(Debug, Deserialize)]
pub(crate) struct WireOwner {
    pub(crate) login: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct WirePermissions {
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    maintain: bool,
    #[serde(default)]
    push: bool,
    #[serde(default)]
    triage: bool,
    #[serde(default)]
    pull: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireRepo {
    id: u64,
    name: String,
    #[serde(default)]
    full_name: String,
    owner: WireOwner,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    clone_url: String,
    #[serde(default)]
    permissions: Option<WirePermissions>,
}

impl WireRepo {
    pub(crate) fn into_parts(self) -> (Repo, Option<GitHubPermissions>) {
        let permissions = self.permissions.map(|p| GitHubPermissions {
            admin: p.admin,
            maintain: p.maintain,
            push: p.push,
            triage: p.triage,
            pull: p.pull,
        });
        let full_name = if self.full_name.is_empty() {
            format!("{}/{}", self.owner.login, self.name)
        } else {
            self.full_name
        };
        let repo = Repo {
            id: self.id,
            owner: self.owner.login,
            name: self.name,
            full_name,
            private: self.private,
            default_branch: self
                .default_branch
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| "main".to_owned()),
            html_url: self.html_url,
            clone_url: self.clone_url,
        };
        (repo, permissions)
    }

    pub(crate) fn into_repo(self) -> Repo {
        self.into_parts().0
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireInstallationToken {
    pub(crate) token: String,
    pub(crate) expires_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireInstallation {
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) account: Option<WireOwner>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireInstallationRepos {
    #[serde(default)]
    pub(crate) repositories: Vec<WireRepo>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireManifestConversion {
    pub(crate) id: u64,
    pub(crate) slug: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    #[serde(default)]
    pub(crate) webhook_secret: Option<String>,
    pub(crate) pem: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireUser {
    pub(crate) id: u64,
    pub(crate) login: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireRef {
    #[serde(rename = "ref")]
    name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WirePull {
    number: u64,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    title: String,
    state: String,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    merged: Option<bool>,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    mergeable: Option<bool>,
    head: WireRef,
    base: WireRef,
    #[serde(default)]
    merge_commit_sha: Option<String>,
}

impl From<WirePull> for PullRequest {
    fn from(pull: WirePull) -> Self {
        let merged = pull.merged.unwrap_or(false) || pull.merged_at.is_some();
        let state = if merged {
            PullState::Merged
        } else if pull.state == "open" {
            PullState::Open
        } else {
            PullState::Closed
        };
        Self {
            number: pull.number,
            url: pull.html_url,
            title: pull.title,
            state,
            base: pull.base.name,
            base_sha: pull.base.sha,
            head: pull.head.name,
            head_sha: pull.head.sha,
            draft: pull.draft.unwrap_or(false),
            mergeable: pull.mergeable,
            merge_commit_sha: if merged { pull.merge_commit_sha } else { None },
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireCheckRuns {
    #[serde(default)]
    pub(crate) total_count: u64,
    #[serde(default)]
    pub(crate) check_runs: Vec<WireCheckRun>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireCheckRun {
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) conclusion: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireCombinedStatus {
    #[serde(default)]
    pub(crate) statuses: Vec<WireStatus>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireStatus {
    pub(crate) state: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireMerge {
    pub(crate) sha: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireId {
    pub(crate) id: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireCompare {
    pub(crate) status: String,
    pub(crate) ahead_by: u64,
    pub(crate) behind_by: u64,
}

/// Aggregates check runs and commit statuses (see [`ChecksSummary`]).
pub(crate) fn aggregate_checks(runs: &[WireCheckRun], statuses: &[WireStatus]) -> ChecksSummary {
    if runs.is_empty() && statuses.is_empty() {
        return ChecksSummary::None;
    }
    let mut pending = false;
    for run in runs {
        if run.status != "completed" {
            pending = true;
            continue;
        }
        match run.conclusion.as_deref() {
            Some("success" | "neutral" | "skipped") => {}
            _ => return ChecksSummary::Failure,
        }
    }
    for status in statuses {
        match status.state.as_str() {
            "success" => {}
            "pending" => pending = true,
            _ => return ChecksSummary::Failure,
        }
    }
    if pending {
        ChecksSummary::Pending
    } else {
        ChecksSummary::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: &str, conclusion: Option<&str>) -> WireCheckRun {
        WireCheckRun {
            status: status.into(),
            conclusion: conclusion.map(Into::into),
        }
    }

    fn status(state: &str) -> WireStatus {
        WireStatus {
            state: state.into(),
        }
    }

    #[test]
    fn aggregation_rules() {
        assert_eq!(aggregate_checks(&[], &[]), ChecksSummary::None);
        assert_eq!(
            aggregate_checks(
                &[
                    run("completed", Some("success")),
                    run("completed", Some("skipped"))
                ],
                &[status("success")]
            ),
            ChecksSummary::Success
        );
        assert_eq!(
            aggregate_checks(
                &[run("in_progress", None), run("completed", Some("neutral"))],
                &[]
            ),
            ChecksSummary::Pending
        );
        assert_eq!(
            aggregate_checks(
                &[run("queued", None), run("completed", Some("timed_out"))],
                &[]
            ),
            ChecksSummary::Failure
        );
        assert_eq!(
            aggregate_checks(&[], &[status("pending")]),
            ChecksSummary::Pending
        );
        assert_eq!(
            aggregate_checks(&[run("completed", Some("success"))], &[status("error")]),
            ChecksSummary::Failure
        );
        assert_eq!(
            aggregate_checks(&[run("completed", Some("action_required"))], &[]),
            ChecksSummary::Failure
        );
    }
}
