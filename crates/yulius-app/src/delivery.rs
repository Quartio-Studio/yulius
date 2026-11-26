//! Git page, delivery profiles and the per-task delivery flow (SPEC-001 GIT-001/002,
//! DLV-001..005, DSC-002/007/008, J8; SPEC-002 §6.7, ADR-11, §8).
//!
//! The decision logic lives in [`yulius_domain::delivery`]; this module gathers the facts it
//! needs (spec validity from the task worktree, open steps, pull request state and checks from
//! GitHub, the worktree head), performs the side effects (push, pull request, merge,
//! promotion, environment runs) and persists the per-task stage.
//!
//! Rules enforced here:
//! - every merge, promotion and environment run is an explicit request of a person with write
//!   access; the only policy action is the profile's `auto_promote`, executed right after a
//!   merge (or the last integration environment) done through Yulius. Agents only get the
//!   read-only `delivery_status` tool (GIT-002, SEC-003);
//! - a pull request needs a valid task spec (DSC-002); a merge needs the domain gates (spec, a
//!   finished step of the plan, checks per profile) and the head that was checked, which GitHub
//!   verifies again (`expected_head`);
//! - reads degrade: GitHub or Git failures become `notes` instead of failing the page.
//!
//! Environment command trust contract (DLV-003, SPEC-002 §8): an environment `command` is
//! shell text of the sandbox profile, set by a repository administrator or seeded once from the
//! default branch's `yulius.toml` when the repository is imported; a run never reads it from the
//! repository. Each run needs an explicit confirmation, is audited with actor,
//! environment and command, and runs as `/bin/sh -c <command>` like a CD run of its stage
//! (SPEC-219, #656): the head of the stage's branch is fetched from origin into a mirror only
//! Yulius writes, its tree is written into the run's own directory with a fresh `HOME`, and the
//! command runs there through the process host in CD's containment (`crate::ci`'s
//! `environment` module), with a minimal environment, the output bounded to the last
//! [`ENVIRONMENT_OUTPUT_BYTES`] and the CI job timeout. Nothing of a run is read from the sandbox
//! clone or the sandbox's home, which its agents write. Commands are visible to everyone with
//! read access, so they must not contain secrets.

mod environments;
pub mod evidence;
mod merge;
mod open;
mod parking;
pub(crate) mod playground;
mod profile;
mod promote;
pub mod runner;
mod text;
mod view;

pub(crate) use self::parking::*;
pub use self::profile::*;
pub(crate) use self::text::*;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;
use yulius_contracts::api::git::{
    BlockedAction, ChecksAccess, CommitView, Delivery, EnvironmentRun, GitOverview, OpenPr,
    PromoteResult, PullView, RunEnvironment, TaskBranch, TaskChanges, TaskCommit, WorktreeState,
};
use yulius_contracts::api::sandboxes::{
    Environment as EnvironmentView, Profile, Sandbox, SandboxVidocView, UpdateSandbox,
};
use yulius_domain::branch_policy::{self, BranchPolicy, PullFacts};
use yulius_domain::ci::vidoc::{RepositoryVidoc, VIDOC_CONFIG, repository_vidoc};
use yulius_domain::delivery::{
    self as dlv, ChecksState, DeliveryAction, DeliveryError, DeliveryFacts, DeliveryProfile,
    DeliveryStage, Environment, EnvironmentEvidence, Initiator, MergeMethod, NextAction, PrState,
    Preset, PromoteMethod, Stage, Verify,
};
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::spec_format::{self, MAX_CONFIG_BYTES, MAX_PATH_BYTES, SpecReport};
use yulius_domain::steps::{DeliveryAsk, StepStatus};
use yulius_domain::task::PullSignal;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, StepRecord, TaskRecord, UserRecord};
use crate::ports::github::{self as gh, GhPull, NewPull};
use crate::ports::store::TaskFieldUpdate;
use runner::{CommandEnd, OutputRing};

/// What preparing a run may take before its command starts: the fetch (Git's 30-minute network
/// limit), the token, the files and the containment.
pub(crate) const PREPARE_ALLOWANCE: Duration = Duration::from_secs(35 * 60);
/// Output kept per environment run (the tail).
pub const ENVIRONMENT_OUTPUT_BYTES: usize = 1 << 20;
/// Largest spec file read.
pub const MAX_SPEC_BYTES: u64 = 256 * 1024;
/// Most Markdown files kept when discovering an unregistered spec, far above any real specs
/// directory: all of them are listed before the slug-named file is read first.
pub const MAX_SPEC_CANDIDATES: usize = 2_000;
/// Integration commits on the Git page.
pub const RECENT_COMMITS: u32 = 20;
/// Open pull requests examined on the Git page.
pub const MAX_PULLS: usize = 30;
/// Longest pull request title.
pub const MAX_PR_TITLE_CHARS: usize = 200;
/// Longest pull request body.
pub const MAX_PR_BODY_BYTES: usize = 64 * 1024;
/// Runs listed per sandbox.
pub const RUN_LIST_LIMIT: u32 = 20;
/// Output tail shown per run in lists.
const RUN_LIST_OUTPUT_BYTES: usize = 2 * 1024;
/// A `running` row older than the run limits plus this grace has no runner any more.
const LOST_RUN_GRACE_MS: i64 = 5 * 60 * 1000;
/// Longest Goal excerpt in a generated pull request body.
const GOAL_EXCERPT_CHARS: usize = 2_000;

const DIRTY_WORKTREE: &str = "the task worktree has uncommitted changes; commit (or discard) them first so the pull request contains exactly what was validated";

enum ObserveAccess<'a> {
    Display(Option<&'a UserRecord>),
    Provided(Option<&'a Secret>),
}

/// The persisted delivery stage of one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRecord {
    /// Task.
    pub task_id: TaskId,
    /// Stage.
    pub stage: DeliveryStage,
    /// Task pull request number, once known.
    pub pr_number: Option<u64>,
    /// Last change.
    pub updated_at: Timestamp,
}

impl DeliveryRecord {
    fn initial(task: &TaskId) -> Self {
        Self {
            task_id: task.clone(),
            stage: DeliveryStage::None,
            pr_number: None,
            updated_at: Timestamp(0),
        }
    }
}

/// State of an environment run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentRunState {
    /// The command is running.
    Running,
    /// Exit status 0 (or a manual confirmation).
    Succeeded,
    /// Non-zero exit, a signal or the timeout.
    Failed,
    /// Could not start, or no runner is attached any more (server restart).
    Error,
}

impl EnvironmentRunState {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Error => "error",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// What an environment run recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentRunKind {
    /// The environment command ran.
    Command,
    /// A person confirmed the environment (manual verification).
    Manual,
}

impl EnvironmentRunKind {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Manual => "manual",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "command" => Some(Self::Command),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// One environment run or manual confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentRunRecord {
    /// Run id.
    pub id: String,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Environment name.
    pub environment: String,
    /// Command run or manual confirmation.
    pub kind: EnvironmentRunKind,
    /// Requesting user.
    pub requested_by: UserId,
    /// Command text (audit).
    pub command: Option<String>,
    /// Commit checked out for the command.
    pub commit: Option<String>,
    /// State.
    pub state: EnvironmentRunState,
    /// Output tail.
    pub output: String,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub ended_at: Option<Timestamp>,
}

// ---------------------------------------------------------------- small helpers

/// Where `task`'s worktree is (SPEC-343): the Git overview says it, and the changes view reads
/// only a `Present` one. A task gets its branch when created and its worktree when it starts.
fn worktree_state(task: &TaskRecord) -> WorktreeState {
    match task.worktree_path.as_deref() {
        None => WorktreeState::NotCreated,
        Some(path) if Path::new(path).is_dir() => WorktreeState::Present,
        Some(_) => WorktreeState::Gone,
    }
}

/// Maps a domain refusal to an API error (state conflicts are 409, request problems 422).
fn refusal(err: &DeliveryError) -> AppError {
    match err {
        DeliveryError::ConfirmationRequired(_)
        | DeliveryError::UnknownEnvironment(_)
        | DeliveryError::WrongEvidence { .. }
        | DeliveryError::NoCommand(_) => AppError::Invalid(err.to_string()),
        DeliveryError::UserActionRequired => AppError::Forbidden(err.to_string()),
        _ => AppError::Conflict(err.to_string()),
    }
}

pub(crate) fn domain_checks(state: gh::ChecksState) -> ChecksState {
    match state {
        gh::ChecksState::None => ChecksState::None,
        gh::ChecksState::Pending => ChecksState::Pending,
        gh::ChecksState::Success => ChecksState::Success,
        gh::ChecksState::Failure => ChecksState::Failure,
        gh::ChecksState::Unknown => ChecksState::Unknown,
    }
}

/// Why a head with commits to deliver is not ready to go out, if it is not: a task that needs a
/// spec needs its plan too; research, docs and chores may have none. What of the plan has to be
/// finished depends on who asks (SPEC-484 PLN-002).
fn not_ready(
    spec: &SpecCheck,
    steps: &[StepRecord],
    ask: DeliveryAsk,
    dirty: bool,
) -> Option<String> {
    if !spec.valid {
        Some(format!(
            "the spec is not valid ({}); fix it and commit",
            spec.problems.join(" ")
        ))
    } else if steps.is_empty() && spec.required {
        Some("the task has no plan: record its steps with `step_update`".to_owned())
    } else if let Some(hold) = plan_hold(steps, ask) {
        Some(hold)
    } else if dirty {
        Some("the worktree has uncommitted changes: commit them".to_owned())
    } else {
        None
    }
}

/// Why the task's plan holds a delivery asked for by `ask`, if it does.
pub(crate) fn plan_hold(steps: &[StepRecord], ask: DeliveryAsk) -> Option<String> {
    yulius_domain::steps::delivery_hold(&crate::discipline::step_states(steps, &[]), ask)
}

/// " N step(s) of your plan are still ahead (next: "…"): …", for the messages that tell an agent
/// its finished part is merged (SPEC-484 PLN-003); empty when none is.
pub(crate) fn steps_ahead_note(steps: &[StepRecord]) -> String {
    steps_ahead(steps, |open, next| {
        format!(
            " {open} step(s) of your plan are still ahead (next: \"{next}\"): they stay open, and \
             the work you commit for them on this branch is delivered next, by a \
             `delivery_merge` of its own."
        )
    })
}

/// [`steps_ahead_note`] for the answers that arm a merge: the work ahead waits for it, because a
/// commit on the branch drops the armed merge of the reviewed head (`merge_armed_task`).
pub(crate) fn steps_ahead_armed_note(steps: &[StepRecord]) -> String {
    steps_ahead(steps, |open, next| {
        format!(
            " {open} step(s) of your plan are still ahead (next: \"{next}\"): they stay open. \
             Start their work once `yulius wait` reports this merge: a commit on this branch \
             before that drops the armed merge."
        )
    })
}

fn steps_ahead(steps: &[StepRecord], say: impl Fn(usize, &str) -> String) -> String {
    let states = crate::discipline::step_states(steps, &[]);
    yulius_domain::steps::ahead(&states).map_or_else(String::new, |(open, next)| say(open, next))
}

/// An upstream failure as the reason in a note, without the adapter's `GitHub: ` prefix: the
/// GitHub error already says it is GitHub ("GitHub refused the request (HTTP 403): …"), so the
/// note does not repeat it (#111).
fn github_reason(err: &AppError) -> String {
    let message = err.public_message();
    match message.strip_prefix("GitHub: ") {
        Some(rest) => rest.to_owned(),
        None => message,
    }
}

/// `'Checks: read'`, `'Checks: read' and 'Commit statuses: read'`.
fn quoted_permissions(missing: &[&str]) -> String {
    missing
        .iter()
        .map(|p| format!("'{p}: read'"))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// What Yulius cannot read for `repo` and the permission the App needs (#111), or `None` when
/// every source could be read. `settings_url` is the App's permission page, for administrators.
fn checks_access(
    repo: &str,
    unreadable: &[gh::ChecksSource],
    settings_url: Option<String>,
) -> Option<ChecksAccess> {
    use gh::ChecksSource::{CheckRuns, CommitStatuses, WorkflowRuns};
    let missing_sources: Vec<gh::ChecksSource> = [CheckRuns, CommitStatuses]
        .into_iter()
        .filter(|s| unreadable.contains(s))
        .collect();
    if missing_sources.is_empty() {
        return None;
    }
    let missing: Vec<&str> = missing_sources.iter().map(|s| s.permission()).collect();
    // Actions runs are read only in place of check runs.
    let attempted: &[gh::ChecksSource] = if unreadable.contains(&CheckRuns) {
        &[CheckRuns, CommitStatuses, WorkflowRuns]
    } else {
        &[CheckRuns, CommitStatuses]
    };
    let read_from: Vec<&str> = attempted
        .iter()
        .filter(|s| !unreadable.contains(s))
        .map(|s| s.noun())
        .collect();
    let what = if missing_sources.len() > 1 {
        "check results"
    } else {
        missing_sources[0].noun()
    };
    let permission = if missing.len() > 1 {
        "permissions"
    } else {
        "permission"
    };
    let tail = if read_from.is_empty() {
        "Checks show as unknown, and merges that require them wait.".to_owned()
    } else {
        format!("Results come from {} only.", read_from.join(" and "))
    };
    Some(ChecksAccess {
        repo: repo.to_owned(),
        missing: missing.iter().map(|p| (*p).to_owned()).collect(),
        read_from: read_from.iter().map(|s| (*s).to_owned()).collect(),
        message: format!(
            "Yulius can't read {what} for {repo}: the GitHub App needs the {} {permission}. {tail}",
            quoted_permissions(&missing)
        ),
        settings_url,
    })
}

/// Adds `more` to `into`, sorted and without repeats.
fn merge_sources(into: &mut Vec<gh::ChecksSource>, more: &[gh::ChecksSource]) {
    into.extend_from_slice(more);
    into.sort();
    into.dedup();
}

fn pr_wire_state(pull: &GhPull) -> &'static str {
    if pull.merged {
        "merged"
    } else if pull.state == "open" {
        "open"
    } else {
        "closed"
    }
}

/// What the checks at a commit say ([`App::dlv_checks_at`]).
pub(crate) struct HeadChecks {
    /// What a gate reads.
    pub(crate) state: ChecksState,
    /// The word shown: `success`, `failure`, `pending`, `none`, `unknown`, and `error` for a
    /// Yulius CI run that could not happen.
    pub(crate) word: &'static str,
    /// What GitHub would not let Yulius read (nothing with Yulius CI).
    pub(crate) unreadable: Vec<gh::ChecksSource>,
}

impl App {
    /// What the checks at `sha` say, for whoever decides on them (DLV-006): a task's merge, the
    /// Git page, a change from the remote control and a promotion alike. Yulius CI is a
    /// project's only CI: with it, its newest run of the commit alone decides (none yet is
    /// pending), and checks GitHub reports from elsewhere do not count. Without it, GitHub's
    /// checks decide.
    ///
    /// # Errors
    /// GitHub's, when its checks are the ones to read.
    pub(crate) async fn dlv_checks_at(
        &self,
        sandbox: &SandboxRecord,
        token: &Secret,
        sha: &str,
    ) -> AppResult<HeadChecks> {
        let gate = self.ci_gate_result(sandbox, sha).await;
        self.checks_with_gate(sandbox, token, sha, gate).await
    }

    async fn checks_with_gate(
        &self,
        sandbox: &SandboxRecord,
        token: &Secret,
        sha: &str,
        gate: yulius_domain::ci::GateResult,
    ) -> AppResult<HeadChecks> {
        if let Some(word) = ci_checks_word(gate) {
            return Ok(HeadChecks {
                state: yulius_domain::ci::gate_checks(ChecksState::None, gate),
                word,
                unreadable: Vec::new(),
            });
        }
        let report = self
            .deps
            .github
            .checks(token, &sandbox.repo_owner, &sandbox.repo_name, sha)
            .await?;
        Ok(HeadChecks {
            state: domain_checks(report.state),
            word: report.state.as_str(),
            unreadable: report.unreadable,
        })
    }

    /// Refuses unless the checks at `sha` allow what requires them: passed, or none at all
    /// (with `or_pending`, also checks that have not reported yet). Unreadable checks never
    /// count as passed (#111).
    async fn dlv_checks_allow(
        &self,
        sandbox: &SandboxRecord,
        token: &Secret,
        sha: &str,
        or_pending: bool,
    ) -> AppResult<()> {
        match self.dlv_checks_at(sandbox, token, sha).await?.state {
            ChecksState::Failure => Err(refusal(&DeliveryError::ChecksFailing)),
            ChecksState::Pending if or_pending => Ok(()),
            ChecksState::Pending => Err(refusal(&DeliveryError::ChecksPending)),
            ChecksState::Unknown => Err(refusal(&DeliveryError::ChecksUnknown)),
            ChecksState::None | ChecksState::Success => Ok(()),
        }
    }
}

/// The checks word of a pull request head from Yulius CI's run there, when the project has Yulius
/// CI: an errored run is `error` (the run could not happen: disk, a timeout, the host), not a
/// failure of the code (S5, F 2026-10-13: infrastructure errors read "checks failing · Needs you").
pub(crate) fn ci_checks_word(gate: yulius_domain::ci::GateResult) -> Option<&'static str> {
    use yulius_domain::ci::{GateResult, RunState};
    match gate {
        GateResult::NotConfigured => None,
        GateResult::Run(None | Some(RunState::Queued | RunState::Running)) => Some("pending"),
        GateResult::Run(Some(RunState::Passed | RunState::Skipped)) => Some("success"),
        GateResult::Run(Some(RunState::Error)) => Some("error"),
        GateResult::Run(Some(RunState::Failed | RunState::Cancelled)) => Some("failure"),
    }
}

fn pull_view(pull: &GhPull, checks: &str) -> PullView {
    PullView {
        number: pull.number,
        url: pull.html_url.clone(),
        title: pull.title.clone(),
        state: pr_wire_state(pull).to_owned(),
        base: pull.base.clone(),
        head: pull.head.clone(),
        checks: checks.to_owned(),
        // Decided per task by `App::pull_conflicts`, never by GitHub's flag alone (SPEC-491).
        conflicts: false,
    }
}

/// The checks word of an open pull request as "tested" reads it (SPEC-491 DON-002): where the
/// delivery profile requires checks, a pull request whose checks have not reported is `pending`,
/// not "none required" (#68). The Git overview and `task_is_done` both ask here.
pub(crate) fn tested_checks_word(profile: &DeliveryProfile, word: &'static str) -> &'static str {
    if profile.require_checks && word == "none" {
        "pending"
    } else {
        word
    }
}

/// A protected branch head or an earlier revision selected through the trusted CD mirror.
#[derive(Clone, Copy)]
pub(crate) enum PromotionSource<'a> {
    Head(Option<&'a str>),
    Verified {
        sha: &'a str,
        head: &'a str,
        base: &'a str,
        mirror: &'a Path,
    },
}

fn pull_facts(pull: &GhPull) -> PullFacts<'_> {
    PullFacts {
        title: &pull.title,
        head: &pull.head,
        base: &pull.base,
        from_fork: pull.from_fork,
    }
}

/// The eligibility fields from two observations of the same open pull.
fn same_open_pull(before: &GhPull, after: &GhPull) -> bool {
    is_open(after)
        && before.number == after.number
        && before.head_sha == after.head_sha
        && before.head == after.head
        && before.from_fork == after.from_fork
        && before.base == after.base
        && before.mergeable == after.mergeable
        && before.behind == after.behind
}

fn is_open(pull: &GhPull) -> bool {
    !pull.merged && pull.state == "open"
}

pub(crate) fn past_merge(stage: &DeliveryStage) -> bool {
    matches!(
        stage,
        DeliveryStage::MergedIntegration
            | DeliveryStage::EnvDone(_)
            | DeliveryStage::Promoted
            | DeliveryStage::ProductionDone
    )
}

/// The stage region a recorded stage is in (`None` before the merge and when done).
fn region(stage: &DeliveryStage, profile: &DeliveryProfile) -> Option<Stage> {
    match stage {
        DeliveryStage::MergedIntegration => Some(Stage::Integration),
        DeliveryStage::Promoted => Some(Stage::Production),
        DeliveryStage::EnvDone(name) => profile.environment(name).map(|env| env.after),
        DeliveryStage::None | DeliveryStage::PrOpen | DeliveryStage::ProductionDone => None,
    }
}

/// The integration environment still due for a merged task, if any.
fn pending_integration_environment<'p>(
    stage: &DeliveryStage,
    profile: &'p DeliveryProfile,
) -> Option<&'p Environment> {
    let mut integration = profile.environments_after(Stage::Integration);
    match stage {
        DeliveryStage::MergedIntegration => integration.next(),
        DeliveryStage::EnvDone(done) => integration.skip_while(|env| env.name != *done).nth(1),
        _ => None,
    }
}

/// Whether a task is in `environment`'s stage and has not passed it yet (so running it now
/// would skip a due environment).
fn not_yet_reached(
    stage: &DeliveryStage,
    profile: &DeliveryProfile,
    environment: &Environment,
) -> bool {
    if region(stage, profile) != Some(environment.after) {
        return false;
    }
    let names: Vec<&str> = profile
        .environments_after(environment.after)
        .map(|env| env.name.as_str())
        .collect();
    let next_due = match stage {
        DeliveryStage::EnvDone(done) => names
            .iter()
            .position(|name| name == done)
            .map_or(names.len(), |index| index + 1),
        _ => 0,
    };
    names
        .iter()
        .position(|name| *name == environment.name)
        .is_some_and(|requested| requested >= next_due)
}

/// Facts of a change already merged (environment and promotion decisions).
fn merged_facts(production_diverged: bool) -> DeliveryFacts {
    DeliveryFacts {
        spec_valid: true,
        steps_open: 0,
        steps_finished: 0,
        checks: ChecksState::None,
        pr_state: PrState::Merged,
        head_current: true,
        production_diverged,
        base_is_integration: true,
    }
}

// ---------------------------------------------------------------- observations

/// Result of reading and validating the task spec.
#[derive(Debug, Clone, Default)]
struct SpecCheck {
    path: Option<String>,
    /// Valid, or not needed for this task (DSC-013).
    valid: bool,
    problems: Vec<String>,
    report: Option<SpecReport>,
    goal: Option<String>,
    /// The task needs a spec (its workflow, or a change beyond documentation, DSC-013).
    required: bool,
}

impl SpecCheck {
    fn problem(path: Option<String>, message: String) -> Self {
        Self {
            path,
            valid: false,
            problems: vec![message],
            report: None,
            goal: None,
            required: true,
        }
    }
}

/// The refreshed delivery view and the receipt for the protected merge that produced it.
pub(crate) struct MergedPull {
    /// Pull request and head accepted by GitHub's guarded merge request.
    pub number: u64,
    pub head: String,
    /// State after the merge for existing delivery callers.
    pub view: Delivery,
    /// The accepted PR head, returned commit and, when confirmed, its base branch.
    pub receipt: String,
}

/// Why a merge of a task's pull request did not happen. Nothing was merged either way.
#[derive(Debug)]
pub(crate) enum MergeFailure {
    /// GitHub could not take the pull request's new title for now (a rate limit, a 5xx, a
    /// timeout, the network; SPEC-069 BPL-008): the same merge may succeed on the next pass, so
    /// an armed merge stays armed.
    Later(AppError),
    /// Refused: a delivery rule, GitHub's refusal, or a failure on the way.
    Refused(AppError),
}

impl From<AppError> for MergeFailure {
    fn from(err: AppError) -> Self {
        Self::Refused(err)
    }
}

impl MergeFailure {
    /// The error to answer with when nothing will try again by itself.
    pub(crate) fn into_error(self) -> AppError {
        match self {
            Self::Later(err) | Self::Refused(err) => err,
        }
    }
}

/// Whether a failure of a GitHub write may pass by itself: [`GitHubPort::update_pull_title`]
/// answers `Upstream` for a rate limit, a 5xx, a timeout or the network, and another error for
/// GitHub's refusal (a 4xx).
///
/// [`GitHubPort::update_pull_title`]: gh::GitHubPort::update_pull_title
fn may_pass(err: &AppError) -> bool {
    matches!(
        err,
        AppError::Upstream(_) | AppError::Capacity(_) | AppError::Internal(_)
    )
}

/// Everything observed about one task before a delivery decision.
#[derive(Debug, Clone)]
struct Observed {
    record: DeliveryRecord,
    spec: SpecCheck,
    steps: Vec<StepRecord>,
    steps_open: usize,
    steps_finished: usize,
    pr: Option<GhPull>,
    checks: ChecksState,
    /// Check sources the App may not read for the pull request head.
    checks_unreadable: Vec<gh::ChecksSource>,
    /// Yulius CI's run at the head could not run (errored): the checks read `error`.
    checks_errored: bool,
    worktree_head: Option<String>,
    dirty: bool,
    /// Whether `has_changes` answered cleanly (fail-closed for automatic pushes when `false`).
    dirty_known: bool,
    notes: Vec<String>,
    /// The profile's integration branch: where the task's pull request must go.
    integration: String,
}

impl Observed {
    fn pr_state(&self) -> PrState {
        match &self.pr {
            Some(pull) if pull.merged => PrState::Merged,
            Some(pull) if is_open(pull) => PrState::Open,
            Some(_) => PrState::Closed,
            None if past_merge(&self.record.stage) => PrState::Merged,
            None => PrState::Absent,
        }
    }

    fn facts(&self) -> DeliveryFacts {
        DeliveryFacts {
            spec_valid: self.spec.valid,
            steps_open: self.steps_open,
            steps_finished: self.steps_finished,
            checks: self.checks,
            pr_state: self.pr_state(),
            head_current: matches!(
                (&self.pr, &self.worktree_head),
                (Some(pull), Some(head)) if pull.head_sha == *head
            ),
            production_diverged: false,
            base_is_integration: self
                .pr
                .as_ref()
                .is_none_or(|pull| pull.base == self.integration && !pull.from_fork),
        }
    }
}

/// Why a task's head is not pushed or not delivered, for `yulius wait` (SPEC-050 DLV-201,
/// DLV-204) and `pr_watch` failed-publication notices (SPEC-553 REP-004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushNote {
    /// The head it is about.
    pub(crate) head: String,
    /// What the agent reads, with what to do.
    pub(crate) text: String,
    /// A push of that head failed (not a "not delivered" note): GitHub's branch reaching the
    /// head makes it stale (SPEC-139 PSH-002).
    pub(crate) failed_push: bool,
    /// Whether `pr_watch` should deliver this note once to an idle session when `yulius wait` is
    /// not running (true for push failures and blocked open-PR CI repairs; false for routine
    /// multi-slice step holds).
    pub(crate) actionable_in_watch: bool,
}

/// Why one automatic delivery failed ([`App::try_auto_open_pr`]): the error, and the note this
/// delivery wrote for it when a push failed, which says what to do (SPEC-139 PSH-003).
#[derive(Debug)]
pub(crate) struct AutoDeliveryError {
    pub(crate) error: AppError,
    pub(crate) note: Option<String>,
}

impl From<AppError> for AutoDeliveryError {
    fn from(error: AppError) -> Self {
        Self { error, note: None }
    }
}

/// Which step of opening a pull request failed ([`App::try_auto_open_pr`]).
enum Failed {
    /// The delivery rules refused it: nothing was pushed.
    Refused,
    /// The push.
    Push,
    /// Opening the pull request, after the push.
    Open,
}

/// What one automatic delivery of a task's head did ([`App::try_auto_open_pr`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutoDelivery {
    /// Nothing to do: nothing new, or nothing Yulius delivers by itself.
    Unchanged,
    /// The head is not ready to be delivered, and why (also noted for `yulius wait`).
    Blocked(String),
    /// Pushed and the pull request opened or updated, as said to the agent.
    Delivered(String),
}

/// What a merged task's worktree head holds beyond its merged pull request (SPEC-425).
enum AfterMerge {
    /// Work the merged pull request did not carry: delivered anew, as this pull request's
    /// follow-up.
    FollowUp(u64),
    /// Nothing new: the task stays merged.
    Covered,
    /// Yulius cannot tell now, and why (what `delivery_merge` answers).
    Unknown(String),
}

impl App {
    pub(crate) async fn dlv_sandbox_of(&self, task: &TaskRecord) -> AppResult<SandboxRecord> {
        self.deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)
    }

    async fn dlv_require_sandbox(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        action: Action,
    ) -> AppResult<SandboxRecord> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        Ok(self.require_for(ctx, &sid, action).await?.0)
    }

    /// Short-lived installation token for Git transport (push, fetch).
    pub(crate) async fn dlv_installation_token(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Secret> {
        crate::playground::refuse_in_playground(sandbox, "A GitHub token")?;
        let app = self.github_app().await?;
        let installed = self
            .deps
            .store
            .instance()
            .await?
            .and_then(|instance| instance.installation_id);
        let id = match installed {
            Some(id) => id,
            None => self
                .deps
                .github
                .find_installation(&app, &sandbox.repo_owner)
                .await?
                .ok_or_else(|| {
                    AppError::Conflict(
                        "the GitHub App is not installed on the repository owner".into(),
                    )
                })?,
        };
        self.deps.github.installation_token(&app, id).await
    }

    /// Token that authors a pull request a person opens: their own GitHub token; the
    /// superadmin without a GitHub identity opens it through the App, as Yulius does for a ready
    /// task (ACC-012).
    pub(crate) async fn dlv_author_token(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<Secret> {
        if user.is_superadmin && user.github_login.is_none() {
            return self.dlv_installation_token(sandbox).await;
        }
        self.user_github_token(user).await
    }

    /// Token for GitHub reads: the caller's user token, else the installation token.
    pub(crate) async fn dlv_read_token(
        &self,
        sandbox: &SandboxRecord,
        user: Option<&UserRecord>,
    ) -> Result<Secret, String> {
        // A playground has no GitHub repository: its views say so instead (SPEC-099 PLY-008).
        if sandbox.is_playground() {
            return Err(crate::playground::PLAYGROUND_NOTE.to_owned());
        }
        if let Some(user) = user
            && let Ok(token) = self.user_github_token(user).await
        {
            return Ok(token);
        }
        self.dlv_installation_token(sandbox)
            .await
            .map_err(|err| format!("GitHub is not available: {}", github_reason(&err)))
    }

    /// Fetches the sandbox's primary clone.
    pub(crate) async fn dlv_refresh_clone(&self, sandbox: &SandboxRecord) -> AppResult<()> {
        let repo = self.sandbox_repo(&sandbox.id);
        if !repo.join(".git").exists() {
            return Err(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ));
        }
        // A playground fetches its own `origin` on this server, by its path (SPEC-099).
        if sandbox.is_playground() {
            return self
                .deps
                .git
                .fetch_branches_from(&repo, &self.playground_origin(&sandbox.id))
                .await;
        }
        let token = self.dlv_installation_token(sandbox).await?;
        self.deps
            .git
            .sync_clone(&self.origin_url(sandbox), &repo, &token)
            .await
    }

    /// The store's compare-and-set of a task's delivery stage; a stage that moved tells the
    /// sandbox's browsers to read its pull requests again (SPEC-108 PRL-002).
    pub(crate) async fn dlv_transition(
        &self,
        sandbox: &SandboxId,
        expected: &DeliveryStage,
        next: &DeliveryRecord,
    ) -> AppResult<bool> {
        self.dlv_transition_at(sandbox, expected, next, None, None)
            .await
    }

    /// [`Self::dlv_transition`] of a merge whose merged head is known: the merge record says
    /// whether a recorded review round approved it (SPEC-493 REV-005).
    pub(crate) async fn dlv_transition_at(
        &self,
        sandbox: &SandboxId,
        expected: &DeliveryStage,
        next: &DeliveryRecord,
        opening_base: Option<&str>,
        merged: Option<&crate::review::MergedHead>,
    ) -> AppResult<bool> {
        let written = self
            .deps
            .store
            .transition_with_landing(
                expected,
                next,
                opening_base,
                merged.and_then(|m| m.landing.as_ref()),
            )
            .await?;
        if written && next.stage != *expected {
            self.publish_pulls(sandbox);
        }
        // The merge record says who wrote and who reviewed (SPEC-493 REV-005): the store closed
        // the pull request's rounds in the same write.
        if written && next.stage == DeliveryStage::MergedIntegration && *expected != next.stage {
            self.record_review_models(&next.task_id, next.pr_number, next.updated_at, merged)
                .await;
        }
        Ok(written)
    }

    /// Tells the browsers watching `sandbox` that its pull requests may have changed (SPEC-108).
    pub(crate) fn publish_pulls(&self, sandbox: &SandboxId) {
        self.events.publish(
            Some(sandbox.clone()),
            false,
            yulius_contracts::events::UiEvent::PullsUpdated {
                sandbox_id: sandbox.to_string(),
            },
        );
    }

    pub(crate) async fn dlv_audit(&self, actor: &str, action: &str, target: &str, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(self.now(), actor, action, Some(target), Some(detail))
            .await
        {
            tracing::error!(error = %err, action, "audit write failed");
        }
    }

    pub(crate) fn dlv_actor_login(user: &UserRecord) -> String {
        user.label()
    }

    /// Why the head under review cannot go out now (SPEC-493 CIR-001): a spec that is not
    /// valid, a missing plan or uncommitted changes, as for any delivery; `None` when it can.
    pub(crate) async fn review_head_not_ready(&self, task: &TaskRecord) -> Option<String> {
        let worktree = task.worktree_path.as_deref()?;
        let steps = self.deps.store.steps(&task.id).await.unwrap_or_default();
        let spec = self.dlv_check_spec(task).await;
        let dirty = self
            .deps
            .git
            .has_changes(Path::new(worktree))
            .await
            .unwrap_or(false);
        not_ready(&spec, &steps, DeliveryAsk::Review, dirty)
    }

    // ---------------------------------------------------------------- agent tool

    /// `spec_check` agent tool (DSC-003): validates this task's spec in the Yulius format (the
    /// same check and fix-it message as the Stop gate) and the project documentation.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_spec_check(&self, ctx: &AgentContext) -> AppResult<Value> {
        let task = self
            .deps
            .store
            .task(&ctx.task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let spec = self.dlv_check_spec(&task).await;
        let message = match (&spec.report, &spec.path) {
            (Some(report), Some(path)) => report.agent_message(path),
            _ => spec.problems.join("\n"),
        };
        let root = task
            .worktree_path
            .as_ref()
            .map_or_else(|| self.sandbox_repo(&task.sandbox_id), PathBuf::from);
        let sandbox_dir = self.sandbox_dir(&task.sandbox_id);
        let (docs, _) = tokio::task::spawn_blocking(move || {
            crate::onboarding::validate_sandbox_checkout(&sandbox_dir, &root)
        })
        .await
        .map_err(AppError::internal)?;
        let message = if spec.required {
            message
        } else {
            "This task needs no spec (research, a question, docs or a chore by its workflow, or a change to documentation only); keep its plan in your steps.".to_owned()
        };
        let plan: Vec<(&str, u8)> = spec
            .report
            .as_ref()
            .map(|r| {
                r.plan
                    .iter()
                    .filter(|item| item.depth == 0)
                    .map(|item| (item.text.as_str(), 1))
                    .collect()
            })
            .unwrap_or_default();
        let mut nudges: Vec<String> = yulius_domain::steps::plan_nudges(&plan, !spec.required)
            .iter()
            .map(ToString::to_string)
            .collect();
        // Every Plan item ticked beside a status that does not say done (SPEC-491 DON-007).
        if let Some(report) = &spec.report
            && !plan.is_empty()
            && report
                .plan
                .iter()
                .filter(|item| item.depth == 0)
                .all(|item| item.checked)
            && let Some(note) = report
                .status
                .and_then(spec_format::SpecStatus::unfinished_note)
        {
            nudges.push(note);
        }
        // A task that needs no spec and has none has nothing to fix: no "No spec is
        // registered…" problem beside "needs no spec" (F 2026-10-11).
        let unneeded = !spec.required && spec.path.is_none();
        let (valid, problems) = if unneeded {
            (true, Vec::new())
        } else {
            (spec.valid, spec.problems.clone())
        };
        Ok(json!({
            "spec": {
                "required": spec.required,
                "path": spec.path,
                "valid": valid,
                "problems": problems,
                "message": message,
                "plan_nudges": nudges,
            },
            "docs": {
                "valid": docs.is_valid(),
                "problems": docs.problems.iter().take(50).map(ToString::to_string).collect::<Vec<_>>(),
            },
        }))
    }

    /// `delivery_status` agent tool (read-only): the task's delivery state, pull request,
    /// checks and profile. There is deliberately no tool that merges, promotes or deploys
    /// (GIT-002, SEC-003).
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_delivery_status(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        crate::ports::github::for_purpose(
            crate::ports::github::GitHubPurpose::Display,
            self.delivery_status_read(ctx, args),
        )
        .await
    }

    async fn delivery_status_read(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let task = self
            .deps
            .store
            .task(&ctx.task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let profile = delivery_profile(&sandbox);
        let view = self.delivery_view(&task, None).await?;
        let before = args["before"].as_str();
        let run_id = args["run_id"].as_str();
        let selected_sha = args["merged_sha"].as_str();
        // A merged task of a merge-first project asks for staging (SPEC-575 MFS-013); a page of
        // older evidence does not read it again.
        let staging = if selected_sha.is_some() || before.is_some() || run_id.is_some() {
            None
        } else {
            self.staging_note(&task, &sandbox, None).await
        };
        let evidence = if selected_sha.is_some() || before.is_some() || run_id.is_some() {
            let sha = selected_sha
                .or_else(|| view.evidence.as_ref().map(|e| e.merged_sha.as_str()))
                .ok_or(AppError::NotFound)?;
            let mut selected = self
                .deps
                .store
                .merge_evidence(&task.id, sha, run_id, before)
                .await?
                .ok_or(AppError::NotFound)?;
            if let Some(id) = run_id {
                selected.url.push_str(&format!(
                    "?run_id={}",
                    crate::browser::percent_encode(id, false)
                ));
            }
            Some(selected)
        } else {
            view.evidence
        };
        Ok(json!({
            "stage": view.stage,
            "environment": view.environment,
            "pull_request": view.pr,
            "spec_path": view.spec_path,
            "spec_valid": view.spec_valid,
            "spec_problems": view.spec_problems,
            "steps_open": view.steps_open,
            "next_actions_for_people": view.next_actions,
            "blocked": view.blocked,
            "notes": view.notes,
            "staging": staging,
            "checks_access": view.checks_access.as_ref().map(|a| &a.message),
            "review_rounds": view.review_rounds,
            "evidence": evidence.map(|mut evidence| {
                evidence.url = format!("{}{}", self.config.public_url.trim_end_matches('/'), evidence.url);
                evidence
            }),
            "profile": profile_to_contract(&profile),
            "policy": "You deliver your own work (DLV-008): Yulius opens the pull request when your work is ready; merge it on green with delivery_merge after an independent review of its head, and run each command-verified integration environment with delivery_deploy. The same checks a person's action needs apply. Production (promotion and its deploys) is always a person's (ADR-0110): ask an administrator of this project with needs_you, naming them in `who`; CD runs are cd_status and cd_deploy.",
        }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pull_requests_get_a_conventional_title_from_the_branch() {
        let subjects = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // Without a branch policy, any conventional-looking prefix counts.
        let none = BranchPolicy::none();
        let title =
            |task: &str, list: &[&str]| default_pr_title(task, &subjects(list), "topic", &none);
        assert_eq!(
            title(
                "Prepare this repository for Yulius",
                &["docs: fix links", "docs: prepare the repository for Yulius"]
            ),
            "docs: prepare this repository for Yulius"
        );
        assert_eq!(
            title("API rate limits", &["fix(api)!: limit per token"]),
            "fix(api)!: API rate limits"
        );
        assert_eq!(
            title("feat(ui): dark mode", &["wip"]),
            "feat(ui): dark mode"
        );
        assert_eq!(
            title("Add search", &["wip", "Merge branch 'dev'"]),
            "Add search"
        );
        assert_eq!(title("Tidy", &["wip: x"]), "wip: tidy");
        assert_eq!(conventional_prefix("Note: not a type"), None);
        assert_eq!(conventional_prefix("feat(: broken"), None);
        assert_eq!(conventional_prefix("chore: "), None);
    }

    use super::*;

    #[test]
    fn branch_publication_discards_verified_ids_absent_from_the_final_open_list() -> AppResult<()> {
        let cache = crate::sandboxes::ObservedPulls::default();
        let sandbox = SandboxId::parse("sandbox").map_err(AppError::internal)?;
        let task = TaskId::parse("task").map_err(AppError::internal)?;
        let mut discovery =
            FollowupDiscovery::new(Some(cache.begin(&sandbox)?), 1, tokio::time::Instant::now());
        discovery.open = Some(vec![2, 3]);
        discovery.publish(
            &task,
            &ParkedPullRead {
                pr: Some(yulius_contracts::api::tasks::ParkedPr {
                    number: 4,
                    open: true,
                }),
                ..ParkedPullRead::default()
            },
            Some(PullSignal::Tested),
        );
        assert_eq!(
            cache
                .known_pull(&sandbox, &task)
                .flatten()
                .map(|pr| pr.number),
            Some(3)
        );
        Ok(())
    }

    #[test]
    fn only_live_ci_can_wake_a_parked_task_after_pending_checks() {
        for enabled in [true, false] {
            assert_eq!(
                PrVerification::WatchPending.watches_pending(enabled),
                enabled
            );
            assert!(!PrVerification::Settled.watches_pending(enabled));
            assert!(!PrVerification::Display.watches_pending(enabled));
        }
    }

    fn subjects(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Pull request #381's commits (task #74), newest first as `review_commits` lists them.
    const PR_381: [&str; 4] = [
        "fix(check): report each conflicted file once and skip symbolic links (#364)",
        "docs: the repository check's conflict markers in development and changes (#364)",
        "fix(check): the repository check fails on committed conflict markers (#364)",
        "docs(spec): SPEC-089 the repository check catches conflict markers (#364)",
    ];
    /// Pull request #382's commits (task #73), newest first.
    const PR_382: [&str; 7] = [
        "docs(spec): SPEC-088 manual evidence at the reviewed wording",
        "fix(doctor): clearer gh warning wording; accurate review record (SPEC-088 review 2)",
        "docs(spec): SPEC-088 evidence and done",
        "test(doctor): the gh warning names both commands and the base rebuild (SPEC-088 review)",
        "docs(operations): a GitHub CLI of at least 2.82.1 from GitHub's package repository (SPEC-088)",
        "feat(doctor): warn about a GitHub CLI older than 2.82.1 (SPEC-088)",
        "docs(spec): SPEC-088 doctor warns about a GitHub CLI older than 2.82.1",
    ];

    /// SPEC-069 BPL-008: the default title takes the type and scope of the branch's most
    /// significant commit, not of its oldest (the spec commit). The squash commits of #381 and
    /// #382 (2026-10-14) read `docs(spec)` for a fix and a feature.
    #[test]
    fn the_default_title_names_the_most_significant_change() {
        let standard = BranchPolicy::standard();
        assert_eq!(
            default_pr_title(
                "Fix #364: the repository check catches committed conflict markers",
                &subjects(&PR_381),
                "feat/74-fix-364-the-repository-check-catches-com",
                &standard,
            ),
            "fix(check): fix #364: the repository check catches committed conflict markers"
        );
        assert_eq!(
            default_pr_title(
                "yulius doctor warns about a GitHub CLI older than 2.82.1",
                &subjects(&PR_382),
                "feat/73-yulius-doctor-warns-about-a-github-cli-o",
                &standard,
            ),
            "feat(doctor): yulius doctor warns about a GitHub CLI older than 2.82.1"
        );
        let none = BranchPolicy::none();
        let title = |list: &[&str], policy: &BranchPolicy| {
            default_pr_title("Tidy", &subjects(list), "topic", policy)
        };
        // The ranking, each against the next one down, whichever came first.
        for (higher, lower) in [
            ("feat(a)", "fix(b)"),
            ("fix(a)", "perf(b)"),
            ("perf(a)", "refactor(b)"),
            ("refactor(a)", "revert(b)"),
            ("revert(a)", "build(b)"),
            ("build(a)", "test(b)"),
            ("ci(a)", "test(b)"),
            ("test(a)", "docs(b)"),
            ("docs(a)", "style(b)"),
            ("style(a)", "chore(b)"),
            ("chore(a)", "wip(b)"),
        ] {
            let (h, l) = (format!("{higher}: x"), format!("{lower}: y"));
            let expected = format!("{higher}: tidy");
            assert_eq!(title(&[h.as_str(), l.as_str()], &none), expected);
            assert_eq!(title(&[l.as_str(), h.as_str()], &none), expected);
        }
        // A tie goes to the oldest; a breaking change keeps its mark.
        assert_eq!(
            title(&["fix(new): y", "ci: z", "fix(old): x"], &none),
            "fix(old): tidy"
        );
        assert_eq!(
            title(&["fix(api)!: y", "docs: x"], &none),
            "fix(api)!: tidy"
        );
        // An enforced policy takes only its own types (`style` is not a standard one).
        assert_eq!(
            title(&["style(ui): y", "docs(ui): x"], &standard),
            "docs(ui): tidy"
        );
        // A repository without conventional commits keeps the task's title.
        assert_eq!(title(&["Add search", "Merge branch 'dev'"], &none), "Tidy");
    }

    /// SPEC-069 BPL-008: before a merge, a title Yulius gave the pull request (by the rule then
    /// or now, for the commits there were) becomes the default of the final commits; a title a
    /// person or an agent wrote stays, and so does one that is already right.
    #[test]
    fn a_title_yulius_gave_is_renewed_before_the_merge_and_a_written_one_kept() {
        let standard = BranchPolicy::standard();
        let none = BranchPolicy::none();
        let doctor = "yulius doctor warns about a GitHub CLI older than 2.82.1";
        let branch = "feat/73-yulius-doctor-warns-about-a-github-cli-o";
        let renewed = |current: &str, list: &[&str], policy: &BranchPolicy| {
            merge_title(current, doctor, &subjects(list), branch, policy)
        };
        let feat = "feat(doctor): yulius doctor warns about a GitHub CLI older than 2.82.1";
        // #382: titled by its oldest commit (the rule before), or opened after the spec commit
        // alone (the rule now): renewed from its final commits, with or without a policy.
        for policy in [&standard, &none] {
            assert_eq!(
                renewed(
                    "docs(spec): yulius doctor warns about a GitHub CLI older than 2.82.1",
                    &PR_382,
                    policy
                )
                .as_deref(),
                Some(feat)
            );
        }
        // Opened before any typed commit: the branch's type.
        assert_eq!(
            renewed(
                "feat: yulius doctor warns about a GitHub CLI older than 2.82.1",
                &["fix(doctor): y", "Spec"],
                &standard
            )
            .as_deref(),
            Some("fix(doctor): yulius doctor warns about a GitHub CLI older than 2.82.1")
        );
        // Already right, or written by a person or an agent: kept.
        assert_eq!(renewed(feat, &PR_382, &standard), None);
        for written in [
            "feat(doctor): warn about an old GitHub CLI",
            "docs(spec): SPEC-088 doctor warns about a GitHub CLI older than 2.82.1",
            "Yulius doctor warns about a GitHub CLI older than 2.82.1",
        ] {
            assert_eq!(renewed(written, &PR_382, &standard), None, "{written}");
        }
        // A conventional task title is the default whatever the commits.
        assert_eq!(
            merge_title(
                "fix(ci): retry",
                "fix(ci): retry",
                &subjects(&["feat(ci): x"]),
                "feat/4-retry",
                &standard
            ),
            None
        );
        // A repository without conventional titles: nothing changes.
        assert_eq!(
            merge_title(
                "Add search",
                "Add search",
                &subjects(&["Add the box", "Merge branch 'dev'"]),
                "search-box",
                &none
            ),
            None
        );
        // A long title Yulius clipped to the policy's length is recognised clipped.
        let long = format!("Fix #223: the merge gate {}", "and more ".repeat(12));
        let clipped = clip_title(
            &format!(
                "docs(spec): fix #223: the merge gate {}",
                "and more ".repeat(12)
            ),
            branch_policy::MAX_TITLE_CHARS,
        );
        let renewed = merge_title(
            &clipped,
            &long,
            &subjects(&["fix(delivery): gate", "docs(spec): SPEC-069"]),
            "feat/59-fix-223",
            &standard,
        )
        .unwrap_or_default();
        assert!(renewed.starts_with("fix(delivery): fix #223"), "{renewed}");
        assert_eq!(
            renewed.chars().count(),
            branch_policy::MAX_TITLE_CHARS,
            "{renewed}"
        );
    }

    /// S5: an errored Yulius CI run reads `error`, not a failure of the code.
    #[test]
    fn an_errored_ci_run_is_not_a_failure() {
        use yulius_domain::ci::{GateResult, RunState};
        assert_eq!(ci_checks_word(GateResult::NotConfigured), None);
        assert_eq!(
            ci_checks_word(GateResult::Run(Some(RunState::Error))),
            Some("error")
        );
        assert_eq!(
            ci_checks_word(GateResult::Run(Some(RunState::Failed))),
            Some("failure")
        );
        assert_eq!(ci_checks_word(GateResult::Run(None)), Some("pending"));
        assert_eq!(
            ci_checks_word(GateResult::Run(Some(RunState::Passed))),
            Some("success")
        );
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_spec_open_reports_missing_unreadable_and_outside_paths() {
        let missing = RepoReadError::from_open("spec.md", AppError::NotFound);
        assert!(matches!(missing, RepoReadError::Missing(path) if path == "spec.md"));
        let outside = AppError::Forbidden(crate::files::OUTSIDE.into());
        let denied = crate::files::io_error(&std::io::ErrorKind::PermissionDenied.into());
        assert_eq!(
            RepoReadError::from_open("spec.md", outside).to_string(),
            "`spec.md` points outside the task worktree"
        );
        assert_eq!(
            RepoReadError::from_open("spec.md", denied).to_string(),
            "`spec.md` cannot be read"
        );
    }

    #[tokio::test]
    async fn a_missing_spec_and_an_unreadable_path_have_distinct_errors() -> TestResult {
        let repo = tempfile::tempdir()?;
        let result = read_spec(repo.path(), "gone.md").await;
        assert!(matches!(result, Err(RepoReadError::Missing(path)) if path == "gone.md"));

        std::fs::write(repo.path().join("file"), "text")?;
        let result = read_spec(repo.path(), "file/spec.md").await;
        assert!(matches!(result, Err(RepoReadError::Unreadable(_))));
        Ok(())
    }

    /// SPEC-069 BPL-006: the titles Yulius generates for task pull requests pass the policy.
    #[test]
    fn generated_titles_pass_the_policy() {
        let policy = BranchPolicy::standard();
        let subjects = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let generated = |task: &str, list: &[&str], branch: &str| {
            clip_title(
                &default_pr_title(task, &subjects(list), branch, &policy),
                branch_policy::MAX_TITLE_CHARS,
            )
        };
        let long = format!(
            "Fix #223: Yulius's merge gate enforces the branch policy {}",
            "and more ".repeat(12)
        );
        for (task, list, branch, expected) in [
            (
                "Checkout API",
                &["Add feature", "Spec"][..],
                "feat/1-checkout-api",
                Some("feat: checkout API"),
            ),
            (
                "Tidy",
                &["wip: x", "docs(readme): tidy"][..],
                "feat/2-tidy",
                Some("docs(readme): tidy"),
            ),
            (
                "Fix #223: the gate",
                &["Merge branch 'dev'"][..],
                "feat/3-fix",
                Some("feat: fix #223: the gate"),
            ),
            (
                "fix(ci): retry",
                &[][..],
                "feat/4-retry",
                Some("fix(ci): retry"),
            ),
            (
                long.as_str(),
                &["fix(delivery): gate"][..],
                "feat/59-fix-223",
                None,
            ),
        ] {
            let title = generated(task, list, branch);
            if let Some(expected) = expected {
                assert_eq!(title, expected);
            }
            assert_eq!(
                policy.check(
                    &PullFacts {
                        title: &title,
                        head: branch,
                        base: "dev",
                        from_fork: false
                    },
                    "dev",
                    "main"
                ),
                Ok(()),
                "{title}"
            );
        }
        assert_eq!(clip_title("abcdef", 6), "abcdef");
        assert_eq!(clip_title("abcdefg", 6), "abcde…");
    }

    #[test]
    fn presets_round_trip_through_the_contract() -> TestResult {
        for profile in preset_profiles() {
            let parsed = profile_from_contract(&profile)?;
            assert_eq!(parsed.preset.as_str(), profile.preset);
            assert_eq!(profile_to_contract(&parsed), profile);
        }
        Ok(())
    }

    #[test]
    fn missing_require_checks_takes_the_preset_default() -> TestResult {
        let mut studio = profile_to_contract(&DeliveryProfile::studio());
        studio.require_checks = None;
        let parsed = profile_from_contract(&studio)?;
        assert!(!parsed.require_checks);
        assert_eq!(parsed.preset, Preset::Studio, "still the studio preset");
        Ok(())
    }

    #[test]
    fn overrides_relabel_and_invalid_profiles_are_rejected() -> TestResult {
        let mut gated = profile_to_contract(&DeliveryProfile::studio());
        gated.auto_promote = false;
        assert_eq!(profile_from_contract(&gated)?.preset, Preset::Custom);

        let mut same = profile_to_contract(&DeliveryProfile::standard());
        same.production_branch = "dev".into();
        assert!(matches!(
            profile_from_contract(&same),
            Err(AppError::Invalid(_))
        ));

        let mut bad_env = profile_to_contract(&DeliveryProfile::product());
        bad_env.environments[0].command = Some("deploy\nrm -rf /".into());
        assert!(matches!(
            profile_from_contract(&bad_env),
            Err(AppError::Invalid(_))
        ));

        let mut blank = profile_to_contract(&DeliveryProfile::product());
        blank.environments[0].command = Some("   ".into());
        assert_eq!(profile_from_contract(&blank)?.environments[0].command, None);

        let mut unknown = profile_to_contract(&DeliveryProfile::standard());
        unknown.merge = "octopus".into();
        assert!(matches!(
            profile_from_contract(&unknown),
            Err(AppError::Invalid(_))
        ));
        Ok(())
    }

    #[test]
    fn legacy_standard_checkpoint_reads_as_the_new_standard() -> TestResult {
        let mut legacy = profile_to_contract(&DeliveryProfile::standard());
        legacy.environments = vec![EnvironmentView {
            name: "dev".into(),
            after: "integration".into(),
            command: None,
            verify: "manual".into(),
        }];
        assert_eq!(profile_from_contract(&legacy)?, DeliveryProfile::standard());

        // A dev environment with a command is a deliberate choice and stays.
        legacy.environments[0].command = Some("./deploy-dev.sh".into());
        legacy.environments[0].verify = "command".into();
        assert_eq!(profile_from_contract(&legacy)?.environments.len(), 1);

        // Product keeps its manual checkpoints.
        let product = profile_to_contract(&DeliveryProfile::product());
        assert_eq!(profile_from_contract(&product)?, DeliveryProfile::product());
        Ok(())
    }

    #[test]
    fn stored_garbage_falls_back_to_standard() -> TestResult {
        let record = SandboxRecord {
            id: SandboxId::parse("s")?,
            slug: "s".into(),
            repo_owner: "o".into(),
            repo_name: "r".into(),
            github_repo_id: None,
            default_branch: "main".into(),
            profile_json: "{}".into(),
            port_base: 1,
            port_count: 1,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: UserId::parse("u")?,
            created_at: Timestamp(0),
            removed_at: None,
        };
        assert_eq!(delivery_profile(&record), DeliveryProfile::standard());
        Ok(())
    }

    #[test]
    fn relative_paths_are_checked() {
        assert!(safe_relative("docs/specs/a.md"));
        assert!(!safe_relative("../etc/passwd"));
        assert!(!safe_relative("/etc/passwd"));
        assert!(!safe_relative("./a.md"));
        assert!(!safe_relative(""));
    }

    #[test]
    fn goal_section_is_extracted_outside_code() {
        let spec = "---\nyulius-task: t\n---\n# T\n## Goal\nShip it.\n```\n## Not a heading\n```\nMore.\n## Context\nx\n";
        assert_eq!(
            section_text(spec, "Goal").as_deref(),
            Some("Ship it.\n```\n## Not a heading\n```\nMore.")
        );
        assert_eq!(section_text(spec, "Plan"), None);
    }

    #[test]
    fn pending_integration_environments_follow_the_profile() {
        let profile = DeliveryProfile::product();
        assert_eq!(
            pending_integration_environment(&DeliveryStage::MergedIntegration, &profile)
                .map(|e| e.name.as_str()),
            Some("dev-vm")
        );
        assert!(
            pending_integration_environment(&DeliveryStage::EnvDone("dev-vm".into()), &profile)
                .is_none()
        );
        assert_eq!(
            region(&DeliveryStage::EnvDone("rollout".into()), &profile),
            Some(Stage::Production)
        );
        assert_eq!(region(&DeliveryStage::PrOpen, &profile), None);
    }

    #[test]
    fn only_skipping_ahead_counts_as_out_of_order() {
        let mut profile = DeliveryProfile::product();
        profile.environments.insert(
            1,
            Environment {
                name: "staging".into(),
                after: Stage::Integration,
                command: None,
                verify: Verify::Manual,
            },
        );
        let dev_vm = profile.environments[0].clone();
        let staging = profile.environments[1].clone();
        let rollout = profile.environments[2].clone();
        let merged = DeliveryStage::MergedIntegration;
        assert!(not_yet_reached(&merged, &profile, &dev_vm));
        assert!(not_yet_reached(&merged, &profile, &staging));
        assert!(!not_yet_reached(&merged, &profile, &rollout), "other stage");
        let deployed = DeliveryStage::EnvDone("dev-vm".into());
        assert!(!not_yet_reached(&deployed, &profile, &dev_vm), "a redeploy");
        assert!(not_yet_reached(&deployed, &profile, &staging));
        assert!(!not_yet_reached(
            &DeliveryStage::ProductionDone,
            &profile,
            &rollout
        ));
        assert!(not_yet_reached(
            &DeliveryStage::Promoted,
            &profile,
            &rollout
        ));
    }

    #[test]
    fn pull_request_body_summarizes_the_spec_and_links_back() -> TestResult {
        let text = "---\nyulius-task: t1\nstatus: draft\n---\n## Goal\nCustomers pay.\n## Context\nc\n## Requirements\nr\n## Plan\n- [x] Spec\n  - [ ] Nested\n## Acceptance criteria\na\n## Test plan\nt\n## Decisions\nd\n";
        let report = spec_format::validate_spec_for(text, &TaskId::parse("t1")?);
        let spec = SpecCheck {
            path: Some("docs/specs/x.md".into()),
            valid: report.is_valid(),
            problems: Vec::new(),
            goal: section_text(text, "Goal"),
            report: Some(report),
            required: true,
        };
        let steps = vec![StepRecord {
            id: yulius_domain::ids::StepId::parse("s1")?,
            task_id: TaskId::parse("t1")?,
            ordinal: 1,
            title: "Implement".into(),
            status: "in_progress".into(),
            note: None,
            updated_by: None,
            updated_at: None,
            effort: 1,
        }];
        let body = pull_request_body("LINK", &spec, &steps, None);
        assert!(body.starts_with("## Goal\n\nCustomers pay."), "{body}");
        assert!(body.contains("**Spec:** `docs/specs/x.md`"));
        assert!(body.contains("- [x] Spec\n  - [ ] Nested"));
        assert!(body.contains("- Implement (in_progress)"));
        assert!(body.ends_with("---\nLINK"));
        let custom = pull_request_body("LINK", &spec, &steps, Some("  Mine  "));
        assert_eq!(custom, "Mine\n\n---\nLINK");
        let huge = "x".repeat(MAX_PR_BODY_BYTES * 2);
        assert!(pull_request_body("LINK", &spec, &[], Some(&huge)).len() <= MAX_PR_BODY_BYTES);
        Ok(())
    }

    #[test]
    fn hex_and_clipping_helpers() {
        assert!(is_hex_sha(&"a".repeat(40)));
        assert!(!is_hex_sha("--upload-pack=x"));
        assert_eq!(clip_chars("abcdef", 3), "abc…");
        assert_eq!(clip_bytes("żółw", 3), "ż");
        assert_eq!(tail_bytes("żółw", 3), "łw");
    }
}
