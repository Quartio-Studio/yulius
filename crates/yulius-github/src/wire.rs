//! Private GitHub wire types and their conversion to adapter types.

use serde::Deserialize;
use yulius_domain::permission::GitHubPermissions;

use crate::types::{CheckSource, ChecksReport, ChecksSummary, PullRequest, PullState, Repo};

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
    /// `null` when the head's repository (a fork) was deleted.
    #[serde(default)]
    repo: Option<WireRefRepo>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireRefRepo {
    #[serde(default)]
    id: u64,
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
    #[serde(default)]
    mergeable_state: Option<String>,
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
        let cross_repository = match (&pull.head.repo, &pull.base.repo) {
            (Some(head), Some(base)) => head.id != base.id,
            // Lists and fixtures without repositories describe this repository's branches.
            (None, None) => false,
            _ => true,
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
            behind: pull.mergeable_state.as_deref() == Some("behind"),
            cross_repository,
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

/// `GET …/actions/runs`: a workflow run has the same `status`/`conclusion` shape as a check run.
#[derive(Debug, Deserialize)]
pub(crate) struct WireWorkflowRuns {
    #[serde(default)]
    pub(crate) total_count: u64,
    #[serde(default)]
    pub(crate) workflow_runs: Vec<WireCheckRun>,
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

/// What the readable sources of one commit reported (`None`: the source was not read).
#[derive(Debug, Default)]
pub(crate) struct CheckReads {
    pub(crate) check_runs: Option<Vec<WireCheckRun>>,
    pub(crate) statuses: Option<Vec<WireStatus>>,
    pub(crate) workflow_runs: Option<Vec<WireCheckRun>>,
    pub(crate) unreadable: Vec<CheckSource>,
}

/// Aggregates the sources that could be read. A source that could not be read never makes the
/// result green: with nothing readable, or nothing reported while a source is unreadable, the
/// result is [`ChecksSummary::Unknown`]; a failure or a run in progress is reported as such.
pub(crate) fn summarize(reads: CheckReads) -> ChecksReport {
    let CheckReads {
        check_runs,
        statuses,
        workflow_runs,
        mut unreadable,
    } = reads;
    unreadable.sort();
    unreadable.dedup();
    let read_any = check_runs.is_some() || statuses.is_some() || workflow_runs.is_some();
    let mut runs = check_runs.unwrap_or_default();
    runs.extend(workflow_runs.unwrap_or_default());
    let summary = if read_any {
        match aggregate_checks(&runs, &statuses.unwrap_or_default()) {
            ChecksSummary::None if !unreadable.is_empty() => ChecksSummary::Unknown,
            other => other,
        }
    } else {
        ChecksSummary::Unknown
    };
    ChecksReport {
        summary,
        unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F 2026-10-11: a pull request behind its base under a strict ruleset is refused with
    /// "required status checks are expected"; GitHub says so in `mergeable_state`.
    #[test]
    fn a_pull_request_behind_its_base_is_marked() -> Result<(), serde_json::Error> {
        let pull = |state: &str| {
            serde_json::from_value::<WirePull>(serde_json::json!({
                "number": 7,
                "state": "open",
                "mergeable": true,
                "mergeable_state": state,
                "head": {"ref": "feat/7", "sha": "h"},
                "base": {"ref": "dev", "sha": "b"},
            }))
            .map(PullRequest::from)
        };
        assert!(pull("behind")?.behind);
        assert!(!pull("clean")?.behind);
        Ok(())
    }

    /// SPEC-069: a head in another repository (a fork, or a deleted one) is marked.
    #[test]
    fn a_head_from_another_repository_is_marked() -> Result<(), serde_json::Error> {
        let pull = |head_repo: serde_json::Value| {
            serde_json::from_value::<WirePull>(serde_json::json!({
                "number": 7,
                "state": "open",
                "head": {"ref": "dev", "sha": "h", "repo": head_repo},
                "base": {"ref": "main", "sha": "b", "repo": {"id": 1}},
            }))
            .map(PullRequest::from)
        };
        assert!(!pull(serde_json::json!({"id": 1}))?.cross_repository);
        assert!(pull(serde_json::json!({"id": 2}))?.cross_repository);
        assert!(pull(serde_json::Value::Null)?.cross_repository);
        Ok(())
    }

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

    #[test]
    fn unreadable_sources_never_make_checks_green() {
        let report = |reads: CheckReads| summarize(reads);
        // Everything readable: as before.
        let full = report(CheckReads {
            check_runs: Some(vec![run("completed", Some("success"))]),
            statuses: Some(vec![]),
            ..CheckReads::default()
        });
        assert_eq!(full.summary, ChecksSummary::Success);
        assert!(full.unreadable.is_empty());
        // Statuses unreadable: the check runs decide what they report.
        for (runs, expected) in [
            (
                vec![run("completed", Some("success"))],
                ChecksSummary::Success,
            ),
            (vec![run("in_progress", None)], ChecksSummary::Pending),
            (
                vec![run("completed", Some("failure"))],
                ChecksSummary::Failure,
            ),
            (vec![], ChecksSummary::Unknown),
        ] {
            let partial = report(CheckReads {
                check_runs: Some(runs),
                unreadable: vec![CheckSource::CommitStatuses],
                ..CheckReads::default()
            });
            assert_eq!(partial.summary, expected);
            assert_eq!(partial.unreadable, vec![CheckSource::CommitStatuses]);
        }
        // Check runs unreadable: statuses and Actions runs count together.
        let fallback = report(CheckReads {
            statuses: Some(vec![status("success")]),
            workflow_runs: Some(vec![run("completed", Some("failure"))]),
            unreadable: vec![CheckSource::CheckRuns],
            ..CheckReads::default()
        });
        assert_eq!(fallback.summary, ChecksSummary::Failure);
        // Nothing readable: unknown, each source named once.
        let none = report(CheckReads {
            unreadable: vec![
                CheckSource::WorkflowRuns,
                CheckSource::CheckRuns,
                CheckSource::CommitStatuses,
                CheckSource::CheckRuns,
            ],
            ..CheckReads::default()
        });
        assert_eq!(none.summary, ChecksSummary::Unknown);
        assert_eq!(
            none.unreadable,
            vec![
                CheckSource::CheckRuns,
                CheckSource::CommitStatuses,
                CheckSource::WorkflowRuns
            ]
        );
    }
}
