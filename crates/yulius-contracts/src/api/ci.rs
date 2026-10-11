//! Yulius-run CI and repository hygiene (SPEC-001 DLV-006/007, SPEC-002 ADR-21; docs/api.md
//! "CI").

use serde::{Deserialize, Serialize};

/// One configured CI job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiJobSpec {
    /// Name (`test (1/2)` for a matrix value).
    pub name: String,
    /// Shell text run as `/bin/sh -c <command>` in the job's CI checkout.
    pub command: String,
    /// Matrix value passed as `YULIUS_CI_MATRIX`.
    #[serde(default)]
    pub matrix: Option<String>,
}

/// The CI a sandbox's runs execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiGate {
    /// Jobs, run in parallel.
    pub jobs: Vec<CiJobSpec>,
    /// `profile` (the sandbox's delivery profile `gate`) or `yulius_toml` (`[[ci.jobs]]` or
    /// `[commands] gate` on the integration branch).
    pub source: String,
}

/// One job of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiJob {
    /// Job id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Shell text as run.
    pub command: String,
    /// Matrix value.
    #[serde(default)]
    pub matrix: Option<String>,
    /// `queued`, `running`, `passed`, `failed`, `error` or `cancelled`.
    pub state: String,
    /// Why it errored or was cancelled.
    #[serde(default)]
    pub reason: Option<String>,
    /// Exit code.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Duration in milliseconds, once it ran.
    #[serde(default)]
    pub duration_ms: Option<i64>,
    /// Output (only in [`CiRunDetail`]): the last 512 KiB of its terminal output, escape
    /// sequences removed, plus Yulius notes; partial while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// One CI run (lists leave the output out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiRun {
    /// Run id.
    pub id: String,
    /// What it tests: `pr/<number>` or `branch/<name>`.
    pub reference: String,
    /// The same in words: `PR #12`, `dev`.
    pub label: String,
    /// Pull request number for `pr/` references.
    #[serde(default)]
    pub pr_number: Option<u64>,
    /// Commit tested.
    pub sha: String,
    /// `queued`, `running`, `passed`, `failed`, `error` or `cancelled` (from its jobs).
    pub state: String,
    /// Where its configuration came from (`profile`, `yulius_toml` or `pull_request`).
    pub source: String,
    /// Its jobs (outputs only in [`CiRunDetail`]).
    pub jobs: Vec<CiJob>,
    /// Why it errored or was cancelled.
    #[serde(default)]
    pub reason: Option<String>,
    /// Queued at.
    pub queued_at: String,
    /// Started at.
    #[serde(default)]
    pub started_at: Option<String>,
    /// Ended at.
    #[serde(default)]
    pub ended_at: Option<String>,
    /// Wall time in milliseconds, once it ran.
    #[serde(default)]
    pub duration_ms: Option<i64>,
    /// Who asked for it (`yulius` for runs the CI started itself).
    pub requested_by: String,
    /// What happened to the commit status on GitHub: `published`, `pending` (not yet) or a
    /// short reason it could not be published.
    pub status: String,
}

/// One run with its jobs' output (`GET /api/sandboxes/{sid}/ci/runs/{id}`).
pub type CiRunDetail = CiRun;

/// `GET /api/sandboxes/{sid}/ci`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiOverview {
    /// The gate in force, if any.
    pub gate: Option<CiGate>,
    /// Why no gate is in force (for example an invalid `yulius.toml`), when that is known.
    #[serde(default)]
    pub gate_problem: Option<String>,
    /// Most recent runs, newest first.
    pub runs: Vec<CiRun>,
    /// The caller may run again and cancel.
    pub can_run: bool,
    /// For instance administrators: commit statuses cannot be published (missing App
    /// permission), in words.
    #[serde(default)]
    pub status_access: Option<String>,
    /// GitHub Actions workflow files still on the integration branch (Yulius is the only CI;
    /// onboarding moves them into `[[ci.jobs]]` and deletes them), at most 20.
    #[serde(default)]
    pub actions_workflows: Vec<String>,
}

/// One stale branch or worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HygieneItem {
    /// `remote_branch`, `local_branch` or `worktree`.
    pub kind: String,
    /// Branch name, or the worktree path relative to the sandbox directory.
    pub name: String,
    /// Stable reason code (`pull_merged`, `pull_closed`, `merged`, `local_copy`, `idle`,
    /// `task_deleted`, `task_archived`, `task_merged`, `missing`).
    pub reason_code: String,
    /// The reason in words.
    pub reason: String,
    /// Removable without `force` (nothing unmerged is lost).
    pub safe: bool,
}

/// A repository hygiene report of one sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HygieneReport {
    /// When it was computed; null before the first scan.
    #[serde(default)]
    pub scanned_at: Option<String>,
    /// Stale items.
    pub items: Vec<HygieneItem>,
    /// Problems met while scanning (GitHub or Git unavailable, scan bounds reached).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// One item to clean up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HygieneTarget {
    /// `remote_branch`, `local_branch` or `worktree`.
    pub kind: String,
    /// As in the report.
    pub name: String,
}

/// `POST /api/sandboxes/{sid}/hygiene/cleanup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HygieneCleanup {
    /// Items to remove; empty means every safe item of the current report.
    #[serde(default)]
    pub items: Vec<HygieneTarget>,
    /// Also remove unmerged branches without a pull request (`idle`).
    #[serde(default)]
    pub force: bool,
}

/// An item cleanup did not remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HygieneSkip {
    /// The item.
    #[serde(flatten)]
    pub target: HygieneTarget,
    /// Why not.
    pub reason: String,
}

/// Result of a cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HygieneCleanupResult {
    /// Removed items.
    pub removed: Vec<HygieneTarget>,
    /// Items left in place, with the reason.
    pub skipped: Vec<HygieneSkip>,
    /// The report after the cleanup.
    pub report: HygieneReport,
}
