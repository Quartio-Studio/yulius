//! Yulius-run CI and repository hygiene (SPEC-001 DLV-006/007, SPEC-002 ADR-18).
//!
//! Yulius is the projects' only CI (F 2026-10-10: no GitHub Actions): it runs each sandbox's
//! **CI jobs** for every open pull request head into the integration branch and for the
//! integration and production heads, on its own machine, in parallel, and publishes the result
//! as the commit status [`STATUS_CONTEXT`]. This module holds the deterministic parts: the CI
//! configuration (a single gate command or `[[ci.jobs]]` with matrix splits), its source, run
//! and job states, the commit status of a run, the merge gate, and which branches and worktrees
//! are stale.

pub mod actions;

use crate::delivery::ChecksState;

/// Longest gate command.
pub const MAX_GATE_BYTES: usize = 2_000;
/// The commit status context Yulius publishes.
pub const STATUS_CONTEXT: &str = "yulius/ci";
/// GitHub's limit for a commit status description.
pub const MAX_STATUS_DESCRIPTION_CHARS: usize = 140;
/// Days without a commit after which a branch without a pull request counts as abandoned.
pub const IDLE_BRANCH_DAYS: i64 = 14;
/// Most `[[ci.jobs]]` entries.
pub const MAX_JOB_DEFINITIONS: usize = 16;
/// Most values in one job's matrix.
pub const MAX_MATRIX_VALUES: usize = 16;
/// Most jobs of one run after matrix expansion.
pub const MAX_JOBS: usize = 32;
/// Longest job name.
pub const MAX_JOB_NAME_BYTES: usize = 32;
/// Longest matrix value.
pub const MAX_MATRIX_VALUE_BYTES: usize = 64;
/// Name of the single job a gate command defines.
pub const GATE_JOB: &str = "gate";

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Where the CI configuration comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GateSource {
    /// The sandbox's delivery profile (`gate`, set by a repository administrator).
    Profile,
    /// `yulius.toml` (`[[ci.jobs]]` or `[commands] gate`) on the integration branch head.
    ProjectConfig,
    /// `yulius.toml` of the pull request itself, while the integration branch has no CI yet
    /// (bootstrap: the pull request that sets up CI is tested with it).
    PullRequest,
}

impl GateSource {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::ProjectConfig => "yulius_toml",
            Self::PullRequest => "pull_request",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "profile" => Some(Self::Profile),
            "yulius_toml" => Some(Self::ProjectConfig),
            "pull_request" => Some(Self::PullRequest),
            _ => None,
        }
    }
}

/// An invalid gate command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// Empty after trimming.
    #[error("the command is empty")]
    Empty,
    /// Longer than [`MAX_GATE_BYTES`].
    #[error("the command is longer than 2000 bytes")]
    TooLong,
    /// Contains a line break or NUL.
    #[error("the command must be a single line")]
    MultiLine,
}

/// Validates a gate or job command and returns it trimmed.
///
/// # Errors
/// [`GateError`] for empty, overlong or multi-line text.
pub fn validate_gate(text: &str) -> Result<String, GateError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(GateError::Empty);
    }
    if trimmed.len() > MAX_GATE_BYTES {
        return Err(GateError::TooLong);
    }
    if trimmed.contains(['\n', '\r', '\0']) {
        return Err(GateError::MultiLine);
    }
    Ok(trimmed.to_owned())
}

/// One `[[ci.jobs]]` entry as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDef {
    /// Name (`[a-z0-9][a-z0-9_-]*`, at most 32 bytes), unique.
    pub name: String,
    /// Shell text run as `/bin/sh -c <command>`.
    pub command: String,
    /// Matrix values: the job runs once per value, with the value in `YULIUS_CI_MATRIX`
    /// (never interpolated into the command). Empty: one job.
    pub matrix: Vec<String>,
}

/// One job of a run, after matrix expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Display name: the definition's name, plus ` (<value>)` for a matrix value.
    pub name: String,
    /// The definition's name (selects the job's persistent checkout and cache).
    pub key: String,
    /// Shell text.
    pub command: String,
    /// Matrix value passed as `YULIUS_CI_MATRIX`.
    pub matrix: Option<String>,
}

/// An invalid CI job configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CiConfigError {
    /// Job name not `[a-z0-9][a-z0-9_-]*` or too long.
    #[error(
        "invalid CI job name '{0}': use 1-32 lowercase letters, digits, '-' or '_', starting with a letter or digit"
    )]
    InvalidJobName(String),
    /// Two jobs share a name.
    #[error("CI job '{0}' is defined twice")]
    DuplicateJob(String),
    /// A job without a valid command.
    #[error("CI job '{job}': {problem}")]
    InvalidCommand {
        /// Job name.
        job: String,
        /// What is wrong.
        problem: GateError,
    },
    /// A matrix value outside `[A-Za-z0-9._:/=+-]`, too long, repeated, or too many values.
    #[error(
        "CI job '{0}': matrix values are separated by spaces, use letters, digits and . _ : / = + -, at most 64 bytes each and 16 values"
    )]
    InvalidMatrix(String),
    /// More definitions or expanded jobs than allowed.
    #[error("CI may define at most 16 jobs and expand to at most 32")]
    TooManyJobs,
    /// Both `[commands] gate` and `[[ci.jobs]]`.
    #[error("use either [commands] gate (one job) or [[ci.jobs]] (several), not both")]
    GateAndJobs,
}

fn valid_job_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_JOB_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Splits a matrix string (`"1/3 2/3 3/3"`) into its values.
///
/// # Errors
/// [`CiConfigError::InvalidMatrix`] naming `job`.
pub fn parse_matrix(job: &str, text: &str) -> Result<Vec<String>, CiConfigError> {
    let values: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    let valid = !values.is_empty()
        && values.len() <= MAX_MATRIX_VALUES
        && values.iter().enumerate().all(|(i, value)| {
            value.len() <= MAX_MATRIX_VALUE_BYTES
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._:/=+-".contains(c))
                && !values[..i].contains(value)
        });
    if valid {
        Ok(values)
    } else {
        Err(CiConfigError::InvalidMatrix(job.to_owned()))
    }
}

/// Validates job definitions and expands their matrices, in order.
///
/// # Errors
/// The first [`CiConfigError`].
pub fn expand_jobs(defs: &[JobDef]) -> Result<Vec<Job>, CiConfigError> {
    if defs.len() > MAX_JOB_DEFINITIONS {
        return Err(CiConfigError::TooManyJobs);
    }
    let mut jobs = Vec::new();
    for (index, def) in defs.iter().enumerate() {
        if !valid_job_name(&def.name) {
            return Err(CiConfigError::InvalidJobName(def.name.clone()));
        }
        if defs[..index].iter().any(|other| other.name == def.name) {
            return Err(CiConfigError::DuplicateJob(def.name.clone()));
        }
        let command =
            validate_gate(&def.command).map_err(|problem| CiConfigError::InvalidCommand {
                job: def.name.clone(),
                problem,
            })?;
        if def.matrix.is_empty() {
            jobs.push(Job {
                name: def.name.clone(),
                key: def.name.clone(),
                command,
                matrix: None,
            });
        } else {
            for value in &def.matrix {
                jobs.push(Job {
                    name: format!("{} ({value})", def.name),
                    key: def.name.clone(),
                    command: command.clone(),
                    matrix: Some(value.clone()),
                });
            }
        }
        if jobs.len() > MAX_JOBS {
            return Err(CiConfigError::TooManyJobs);
        }
    }
    Ok(jobs)
}

/// The jobs of a project's `yulius.toml`: `[[ci.jobs]]`, else `[commands] gate` as one job
/// named [`GATE_JOB`].
///
/// # Errors
/// Both forms at once, or invalid jobs.
pub fn project_jobs(gate: Option<&str>, defs: &[JobDef]) -> Result<Vec<Job>, CiConfigError> {
    match (gate, defs.is_empty()) {
        (Some(_), false) => Err(CiConfigError::GateAndJobs),
        (_, false) => expand_jobs(defs),
        (Some(command), true) => Ok(vec![single_job(command).map_err(|problem| {
            CiConfigError::InvalidCommand {
                job: GATE_JOB.to_owned(),
                problem,
            }
        })?]),
        (None, true) => Ok(Vec::new()),
    }
}

fn single_job(command: &str) -> Result<Job, GateError> {
    Ok(Job {
        name: GATE_JOB.to_owned(),
        key: GATE_JOB.to_owned(),
        command: validate_gate(command)?,
        matrix: None,
    })
}

/// The CI a sandbox's runs execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// Jobs, run in parallel (at least one).
    pub jobs: Vec<Job>,
    /// Where they were configured.
    pub source: GateSource,
}

/// The CI in force: the profile's gate when set, else the project's jobs (read from the
/// integration branch, so a pull request cannot change the CI it is tested with). Invalid or
/// empty values count as absent.
#[must_use]
pub fn effective_gate(profile: Option<&str>, project: &[Job]) -> Option<Gate> {
    if let Some(job) = profile.and_then(|text| single_job(text).ok()) {
        return Some(Gate {
            jobs: vec![job],
            source: GateSource::Profile,
        });
    }
    (!project.is_empty()).then(|| Gate {
        jobs: project.to_vec(),
        source: GateSource::ProjectConfig,
    })
}

/// The state of a run from its jobs' states: running while any job is active, then failed if
/// any failed, error if any errored, cancelled if any was cancelled, else passed. A run without
/// jobs is an error.
#[must_use]
pub fn aggregate(jobs: &[RunState]) -> RunState {
    if jobs.is_empty() {
        return RunState::Error;
    }
    if jobs.contains(&RunState::Running) {
        return RunState::Running;
    }
    if jobs.iter().all(|s| *s == RunState::Queued) {
        return RunState::Queued;
    }
    if jobs.iter().any(|s| s.is_active()) {
        return RunState::Running;
    }
    for state in [RunState::Failed, RunState::Error, RunState::Cancelled] {
        if jobs.contains(&state) {
            return state;
        }
    }
    RunState::Passed
}

/// State of one CI run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunState {
    /// Waiting for a free runner.
    Queued,
    /// The gate command is running.
    Running,
    /// Exit status 0.
    Passed,
    /// Non-zero exit or a signal.
    Failed,
    /// Could not run: checkout or spawn failure, timeout, lost runner.
    Error,
    /// Cancelled by a person or superseded by a newer head.
    Cancelled,
}

impl RunState {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "passed" => Some(Self::Passed),
            "failed" => Some(Self::Failed),
            "error" => Some(Self::Error),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Queued or running.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// What a run tests: a pull request head or a branch head (a push).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CiRef {
    /// Open pull request into the integration branch.
    PullRequest(u64),
    /// The integration or production branch.
    Branch(String),
}

impl CiRef {
    /// Stable key (`pr/12`, `branch/dev`): one active run per key.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::PullRequest(number) => format!("pr/{number}"),
            Self::Branch(name) => format!("branch/{name}"),
        }
    }

    /// Parses a key.
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        if let Some(number) = key.strip_prefix("pr/") {
            return number
                .parse()
                .ok()
                .filter(|n| *n > 0)
                .map(Self::PullRequest);
        }
        key.strip_prefix("branch/")
            .filter(|name| !name.is_empty())
            .map(|name| Self::Branch(name.to_owned()))
    }

    /// Words for people: `PR #12`, `dev`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::PullRequest(number) => format!("PR #{number}"),
            Self::Branch(name) => name.clone(),
        }
    }
}

/// A head the CI should test now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// What it is.
    pub reference: CiRef,
    /// Its head commit.
    pub sha: String,
}

/// A run as the scheduler sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFacts {
    /// Run id.
    pub id: String,
    /// What it tests.
    pub reference: CiRef,
    /// Commit.
    pub sha: String,
    /// State.
    pub state: RunState,
}

/// What the scheduler does with the current targets and runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Targets to queue a run for.
    pub queue: Vec<Target>,
    /// Active runs to cancel because their reference moved to another commit or is gone
    /// (closed pull request), with the commit that superseded them when there is one.
    pub supersede: Vec<(String, Option<String>)>,
}

/// Plans one scheduling round (pure).
///
/// - A target is queued unless some run of the same commit is active or finished with a
///   verdict (`passed`, `failed`, `error`): a commit is tested once, whichever reference
///   carries it. Cancelled runs do not count, so a head whose run was cancelled is retried
///   only by "Run again".
/// - An active run whose reference now points to another commit, or that no longer exists
///   (pull request closed or merged), is superseded.
#[must_use]
pub fn plan(targets: &[Target], runs: &[RunFacts]) -> Plan {
    let mut out = Plan::default();
    for target in targets {
        let tested = runs
            .iter()
            .any(|run| run.sha == target.sha && (run.state.is_active() || has_verdict(run.state)));
        let cancelled_here = runs.iter().any(|run| {
            run.sha == target.sha
                && run.reference == target.reference
                && run.state == RunState::Cancelled
        });
        let queued_already = out.queue.iter().any(|t| t.sha == target.sha);
        if !tested && !cancelled_here && !queued_already {
            out.queue.push(target.clone());
        }
    }
    for run in runs.iter().filter(|run| run.state.is_active()) {
        let current = targets.iter().find(|t| t.reference == run.reference);
        match current {
            Some(target) if target.sha == run.sha => {}
            Some(target) => out
                .supersede
                .push((run.id.clone(), Some(target.sha.clone()))),
            None => out.supersede.push((run.id.clone(), None)),
        }
    }
    out
}

fn has_verdict(state: RunState) -> bool {
    matches!(state, RunState::Passed | RunState::Failed | RunState::Error)
}

/// A GitHub commit status state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitState {
    /// `pending`.
    Pending,
    /// `success`.
    Success,
    /// `failure`.
    Failure,
    /// `error`.
    Error,
}

impl CommitState {
    /// GitHub's wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Error => "error",
        }
    }
}

/// What a run publishes on its commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStatus {
    /// State.
    pub state: CommitState,
    /// Description (at most [`MAX_STATUS_DESCRIPTION_CHARS`] characters).
    pub description: String,
}

/// The facts a status description is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSummary<'a> {
    /// Jobs of the run.
    pub jobs: usize,
    /// Jobs that passed.
    pub passed: usize,
    /// The first failed job and its exit code, when one failed.
    pub failed_job: Option<(&'a str, Option<i32>)>,
    /// Wall time of the run, once it ran.
    pub duration_ms: Option<i64>,
    /// Why it ended without a verdict (error, cancellation).
    pub reason: Option<&'a str>,
}

/// Human duration: `45 s`, `3 min 12 s`, `1 h 4 min`.
#[must_use]
pub fn format_duration(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{} h {} min", secs / 3600, (secs % 3600) / 60)
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_STATUS_DESCRIPTION_CHARS {
        return text.to_owned();
    }
    let mut out: String = text
        .chars()
        .take(MAX_STATUS_DESCRIPTION_CHARS - 1)
        .collect();
    out.push('…');
    out
}

fn jobs_word(n: usize) -> String {
    if n == 1 {
        "1 job".to_owned()
    } else {
        format!("{n} jobs")
    }
}

/// The commit status of a run in `state`.
#[must_use]
pub fn commit_status(state: RunState, summary: RunSummary<'_>) -> CommitStatus {
    let took = summary
        .duration_ms
        .map(|ms| format!(" in {}", format_duration(ms)))
        .unwrap_or_default();
    let (state, description) = match state {
        RunState::Queued => (
            CommitState::Pending,
            format!("Queued on Yulius ({})", jobs_word(summary.jobs)),
        ),
        RunState::Running => (
            CommitState::Pending,
            format!(
                "Running on Yulius: {} of {} passed",
                summary.passed,
                jobs_word(summary.jobs)
            ),
        ),
        RunState::Passed => (
            CommitState::Success,
            format!("Passed: {}{took}", jobs_word(summary.jobs)),
        ),
        RunState::Failed => (
            CommitState::Failure,
            match summary.failed_job {
                Some((job, Some(code))) => format!("Failed: {job} (exit code {code}){took}"),
                Some((job, None)) => format!("Failed: {job}{took}"),
                None => format!("Failed{took}"),
            },
        ),
        RunState::Error => (
            CommitState::Error,
            format!(
                "Error: {}",
                summary.reason.unwrap_or("the CI could not run")
            ),
        ),
        RunState::Cancelled => (
            CommitState::Error,
            format!("Cancelled: {}", summary.reason.unwrap_or("stopped")),
        ),
    };
    CommitStatus {
        state,
        description: clip(&description),
    }
}

/// The CI's verdict for a merge gate at one head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateResult {
    /// The sandbox has no Yulius CI (not onboarded yet): GitHub's checks decide.
    NotConfigured,
    /// The sandbox has Yulius CI and this is the latest run state at the head (`None`: no run
    /// yet).
    Run(Option<RunState>),
}

/// The checks a merge gate sees. With Yulius CI configured, its run at the head **alone**
/// decides (it is the projects' only CI; F 2026-10-10): no run yet, queued or running is
/// pending; passed is success; failed, errored or cancelled is failure (run it again). Checks
/// GitHub reports from elsewhere (including leftover Actions) do not count then. Without
/// Yulius CI, GitHub's checks decide as before.
#[must_use]
pub fn gate_checks(github: ChecksState, gate: GateResult) -> ChecksState {
    match gate {
        GateResult::NotConfigured => github,
        GateResult::Run(None | Some(RunState::Queued | RunState::Running)) => ChecksState::Pending,
        GateResult::Run(Some(RunState::Passed)) => ChecksState::Success,
        GateResult::Run(Some(RunState::Failed | RunState::Error | RunState::Cancelled)) => {
            ChecksState::Failure
        }
    }
}

// ---------------------------------------------------------------- hygiene

/// Why a branch or worktree is stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    /// Its pull request was merged.
    PullMerged(u64),
    /// Its pull request was closed without merging (GitHub keeps the commits on the pull
    /// request).
    PullClosed(u64),
    /// Its commits are already in the integration or production branch.
    Merged,
    /// A local branch no worktree uses whose commits are on GitHub.
    LocalCopy,
    /// No pull request and no commit for this many days: unmerged work, removed only with
    /// `force`.
    Idle(i64),
    /// A task worktree whose task was deleted.
    TaskDeleted,
    /// A task worktree whose task is archived.
    TaskArchived(u32),
    /// A task worktree whose task's pull request is merged.
    TaskMerged(u32),
    /// A registered worktree whose directory is gone.
    Missing,
}

impl StaleReason {
    /// Stable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::PullMerged(_) => "pull_merged",
            Self::PullClosed(_) => "pull_closed",
            Self::Merged => "merged",
            Self::LocalCopy => "local_copy",
            Self::Idle(_) => "idle",
            Self::TaskDeleted => "task_deleted",
            Self::TaskArchived(_) => "task_archived",
            Self::TaskMerged(_) => "task_merged",
            Self::Missing => "missing",
        }
    }

    /// Words for people and agents.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::PullMerged(n) => format!("pull request #{n} was merged"),
            Self::PullClosed(n) => format!("pull request #{n} was closed without merging"),
            Self::Merged => "its commits are already in the integration branch".to_owned(),
            Self::LocalCopy => "no worktree uses it and its commits are on GitHub".to_owned(),
            Self::Idle(days) => format!("no pull request and no commit for {days} days"),
            Self::TaskDeleted => "its task was deleted".to_owned(),
            Self::TaskArchived(n) => format!("task #{n} is archived"),
            Self::TaskMerged(n) => format!("task #{n}'s pull request is merged"),
            Self::Missing => "its directory is gone".to_owned(),
        }
    }

    /// Whether cleanup may remove it without `force`: everything except unmerged work.
    #[must_use]
    pub fn safe(&self) -> bool {
        !matches!(self, Self::Idle(_))
    }
}

/// A pull request found for a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchPull {
    /// Number.
    pub number: u64,
    /// Still open.
    pub open: bool,
    /// Merged.
    pub merged: bool,
}

/// What is known about a remote branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranchFacts {
    /// The integration, production or default branch.
    pub protected: bool,
    /// Its tip is contained in the integration or production branch.
    pub merged: bool,
    /// Pull requests whose head is this branch, newest first (`None`: not looked up).
    pub pulls: Option<Vec<BranchPull>>,
    /// Time of its last commit.
    pub last_commit_ms: i64,
}

/// Whether a remote branch is stale, and why (`now_ms` for idleness). Branches with an open
/// pull request, protected branches and recent work are kept.
#[must_use]
pub fn classify_remote_branch(facts: &RemoteBranchFacts, now_ms: i64) -> Option<StaleReason> {
    if facts.protected {
        return None;
    }
    let pulls = facts.pulls.as_deref().unwrap_or_default();
    if pulls.iter().any(|p| p.open) {
        return None;
    }
    if facts.merged {
        return Some(StaleReason::Merged);
    }
    if let Some(last) = pulls.first() {
        return Some(if last.merged {
            StaleReason::PullMerged(last.number)
        } else {
            StaleReason::PullClosed(last.number)
        });
    }
    // Not looked up (scan bound): unknown, keep.
    facts.pulls.as_ref()?;
    let days = (now_ms - facts.last_commit_ms) / DAY_MS;
    (days >= IDLE_BRANCH_DAYS).then_some(StaleReason::Idle(days))
}

/// What is known about a local branch of the sandbox clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalBranchFacts {
    /// The integration, production or default branch.
    pub protected: bool,
    /// Checked out in some worktree.
    pub checked_out: bool,
    /// Its tip is contained in a remote-tracking branch (nothing would be lost).
    pub on_remote: bool,
}

/// Whether a local branch is stale: unused by any worktree and fully on GitHub.
#[must_use]
pub fn classify_local_branch(facts: LocalBranchFacts) -> Option<StaleReason> {
    (!facts.protected && !facts.checked_out && facts.on_remote).then_some(StaleReason::LocalCopy)
}

/// The task a worktree under `worktrees/<task id>` belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeOwner {
    /// No task has this id any more.
    Deleted,
    /// The task exists.
    Task {
        /// Task number.
        number: u32,
        /// Archived.
        archived: bool,
        /// Its pull request is merged (delivery past the merge).
        merged: bool,
        /// A process of the task is live (starting, running, switching, stopping).
        live: bool,
    },
    /// Not a task worktree (unknown owner): never stale.
    Other,
}

/// Whether a worktree is stale. A live task's worktree never is.
#[must_use]
pub fn classify_worktree(owner: WorktreeOwner, prunable: bool) -> Option<StaleReason> {
    if prunable {
        return Some(StaleReason::Missing);
    }
    match owner {
        WorktreeOwner::Deleted => Some(StaleReason::TaskDeleted),
        WorktreeOwner::Task { live: true, .. } | WorktreeOwner::Other => None,
        WorktreeOwner::Task {
            number,
            archived: true,
            ..
        } => Some(StaleReason::TaskArchived(number)),
        WorktreeOwner::Task {
            number,
            merged: true,
            ..
        } => Some(StaleReason::TaskMerged(number)),
        WorktreeOwner::Task { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(reference: CiRef, sha: &str) -> Target {
        Target {
            reference,
            sha: sha.to_owned(),
        }
    }

    fn run(id: &str, reference: CiRef, sha: &str, state: RunState) -> RunFacts {
        RunFacts {
            id: id.to_owned(),
            reference,
            sha: sha.to_owned(),
            state,
        }
    }

    #[test]
    fn gate_validation_and_precedence() -> Result<(), CiConfigError> {
        assert_eq!(validate_gate("  make ci  "), Ok("make ci".to_owned()));
        assert_eq!(validate_gate("   "), Err(GateError::Empty));
        assert_eq!(validate_gate("a\nb"), Err(GateError::MultiLine));
        assert_eq!(
            validate_gate(&"x".repeat(MAX_GATE_BYTES + 1)),
            Err(GateError::TooLong)
        );
        let project = project_jobs(Some("cargo test"), &[])?;
        let profile = effective_gate(Some("scripts/dev ci"), &project);
        assert_eq!(
            profile.map(|g| (g.source, g.jobs[0].command.clone())),
            Some((GateSource::Profile, "scripts/dev ci".into()))
        );
        let from_project = effective_gate(Some("  "), &project);
        assert_eq!(
            from_project.map(|g| (g.source, g.jobs.len())),
            Some((GateSource::ProjectConfig, 1))
        );
        assert_eq!(effective_gate(None, &[]), None);
        Ok(())
    }

    #[test]
    fn jobs_expand_matrices_and_reject_bad_definitions() -> Result<(), CiConfigError> {
        let def = |name: &str, command: &str, matrix: &str| JobDef {
            name: name.into(),
            command: command.into(),
            matrix: if matrix.is_empty() {
                Vec::new()
            } else {
                parse_matrix(name, matrix).unwrap_or_default()
            },
        };
        let jobs = expand_jobs(&[
            def("lint", "cargo clippy", ""),
            def("test", "cargo test", "1/2 2/2"),
        ])?;
        let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, ["lint", "test (1/2)", "test (2/2)"]);
        assert_eq!(jobs[2].key, "test");
        assert_eq!(jobs[2].matrix.as_deref(), Some("2/2"));
        assert!(matches!(
            expand_jobs(&[def("Lint", "x", "")]),
            Err(CiConfigError::InvalidJobName(_))
        ));
        assert!(matches!(
            expand_jobs(&[def("a", "x", ""), def("a", "y", "")]),
            Err(CiConfigError::DuplicateJob(_))
        ));
        assert!(matches!(
            expand_jobs(&[def("a", " ", "")]),
            Err(CiConfigError::InvalidCommand { .. })
        ));
        assert!(parse_matrix("t", "a a").is_err());
        assert!(parse_matrix("t", "$(rm -rf /)").is_err());
        assert!(parse_matrix("t", "   ").is_err());
        let many: Vec<String> = (0..MAX_MATRIX_VALUES).map(|i| i.to_string()).collect();
        let wide = JobDef {
            name: "w".into(),
            command: "x".into(),
            matrix: many,
        };
        let mut defs = vec![
            wide.clone(),
            JobDef {
                name: "v".into(),
                ..wide
            },
        ];
        defs.push(def("u", "x", ""));
        assert_eq!(expand_jobs(&defs), Err(CiConfigError::TooManyJobs));
        assert_eq!(
            project_jobs(Some("x"), &[def("a", "y", "")]),
            Err(CiConfigError::GateAndJobs)
        );
        assert!(project_jobs(None, &[])?.is_empty());
        Ok(())
    }

    #[test]
    fn run_state_follows_its_jobs() {
        use RunState as S;
        assert_eq!(aggregate(&[]), S::Error);
        assert_eq!(aggregate(&[S::Queued, S::Queued]), S::Queued);
        assert_eq!(aggregate(&[S::Passed, S::Queued]), S::Running);
        assert_eq!(aggregate(&[S::Failed, S::Running]), S::Running);
        assert_eq!(aggregate(&[S::Passed, S::Failed, S::Error]), S::Failed);
        assert_eq!(aggregate(&[S::Passed, S::Error, S::Cancelled]), S::Error);
        assert_eq!(aggregate(&[S::Passed, S::Cancelled]), S::Cancelled);
        assert_eq!(aggregate(&[S::Passed, S::Passed]), S::Passed);
    }

    #[test]
    fn refs_round_trip() {
        for reference in [CiRef::PullRequest(12), CiRef::Branch("dev".into())] {
            assert_eq!(CiRef::parse(&reference.key()), Some(reference.clone()));
        }
        assert_eq!(CiRef::parse("pr/0"), None);
        assert_eq!(CiRef::parse("branch/"), None);
        assert_eq!(CiRef::parse("tag/x"), None);
        assert_eq!(CiRef::PullRequest(7).label(), "PR #7");
        for state in [
            RunState::Queued,
            RunState::Running,
            RunState::Passed,
            RunState::Failed,
            RunState::Error,
            RunState::Cancelled,
        ] {
            assert_eq!(RunState::parse(state.as_str()), Some(state));
        }
    }

    #[test]
    fn a_commit_is_tested_once_and_moved_heads_supersede() {
        let pr = CiRef::PullRequest(3);
        let dev = CiRef::Branch("dev".into());
        // New heads are queued once, even when two references carry the same commit.
        let fresh = plan(&[target(pr.clone(), "a1"), target(dev.clone(), "a1")], &[]);
        assert_eq!(fresh.queue, vec![target(pr.clone(), "a1")]);
        // A finished verdict for the commit (under any reference) means no new run.
        let done = plan(
            &[target(dev.clone(), "a1")],
            &[run("r1", pr.clone(), "a1", RunState::Passed)],
        );
        assert!(done.queue.is_empty() && done.supersede.is_empty());
        // The pull request moved: the active run is superseded and the new head queued.
        let moved = plan(
            &[target(pr.clone(), "b2")],
            &[run("r1", pr.clone(), "a1", RunState::Running)],
        );
        assert_eq!(moved.queue, vec![target(pr.clone(), "b2")]);
        assert_eq!(moved.supersede, vec![("r1".into(), Some("b2".into()))]);
        // The pull request is gone: its queued run is superseded without a successor.
        let gone = plan(&[], &[run("r2", pr.clone(), "a1", RunState::Queued)]);
        assert_eq!(gone.supersede, vec![("r2".into(), None)]);
        // A cancelled run is not retried automatically for the same reference.
        let cancelled = plan(
            &[target(pr.clone(), "a1")],
            &[run("r3", pr, "a1", RunState::Cancelled)],
        );
        assert!(cancelled.queue.is_empty());
    }

    #[test]
    fn statuses_describe_each_state_within_githubs_limit() {
        let passed = commit_status(
            RunState::Passed,
            RunSummary {
                jobs: 3,
                passed: 3,
                duration_ms: Some(192_000),
                ..RunSummary::default()
            },
        );
        assert_eq!(passed.state, CommitState::Success);
        assert_eq!(passed.description, "Passed: 3 jobs in 3 min 12 s");
        let failed = commit_status(
            RunState::Failed,
            RunSummary {
                jobs: 2,
                passed: 1,
                failed_job: Some(("test (1/2)", Some(101))),
                duration_ms: Some(5_000),
                reason: None,
            },
        );
        assert_eq!(failed.state, CommitState::Failure);
        assert_eq!(
            failed.description,
            "Failed: test (1/2) (exit code 101) in 5 s"
        );
        let long = "x".repeat(400);
        let error = commit_status(
            RunState::Error,
            RunSummary {
                reason: Some(&long),
                ..RunSummary::default()
            },
        );
        assert_eq!(error.state, CommitState::Error);
        assert_eq!(
            error.description.chars().count(),
            MAX_STATUS_DESCRIPTION_CHARS
        );
        let running = commit_status(
            RunState::Running,
            RunSummary {
                jobs: 1,
                ..RunSummary::default()
            },
        );
        assert_eq!(running.state, CommitState::Pending);
        assert_eq!(running.description, "Running on Yulius: 0 of 1 job passed");
        assert_eq!(format_duration(3_900_000), "1 h 5 min");
    }

    #[test]
    fn yulius_ci_alone_decides_once_configured() {
        use ChecksState as C;
        assert_eq!(gate_checks(C::None, GateResult::NotConfigured), C::None);
        assert_eq!(
            gate_checks(C::Failure, GateResult::NotConfigured),
            C::Failure
        );
        assert_eq!(gate_checks(C::None, GateResult::Run(None)), C::Pending);
        assert_eq!(
            gate_checks(C::Failure, GateResult::Run(Some(RunState::Passed))),
            C::Success
        );
        assert_eq!(
            gate_checks(C::Unknown, GateResult::Run(Some(RunState::Passed))),
            C::Success
        );
        assert_eq!(
            gate_checks(C::Success, GateResult::Run(Some(RunState::Running))),
            C::Pending
        );
        assert_eq!(
            gate_checks(C::Success, GateResult::Run(Some(RunState::Cancelled))),
            C::Failure
        );
    }

    #[test]
    fn remote_branches_are_stale_only_without_open_work() {
        let now = 100 * DAY_MS;
        let base = RemoteBranchFacts {
            protected: false,
            merged: false,
            pulls: Some(Vec::new()),
            last_commit_ms: now - DAY_MS,
        };
        assert_eq!(classify_remote_branch(&base, now), None);
        let protected = RemoteBranchFacts {
            protected: true,
            merged: true,
            ..base.clone()
        };
        assert_eq!(classify_remote_branch(&protected, now), None);
        let open = RemoteBranchFacts {
            merged: true,
            pulls: Some(vec![BranchPull {
                number: 4,
                open: true,
                merged: false,
            }]),
            ..base.clone()
        };
        assert_eq!(classify_remote_branch(&open, now), None);
        let merged = RemoteBranchFacts {
            merged: true,
            ..base.clone()
        };
        assert_eq!(
            classify_remote_branch(&merged, now),
            Some(StaleReason::Merged)
        );
        let squashed = RemoteBranchFacts {
            pulls: Some(vec![
                BranchPull {
                    number: 9,
                    open: false,
                    merged: true,
                },
                BranchPull {
                    number: 2,
                    open: false,
                    merged: false,
                },
            ]),
            ..base.clone()
        };
        assert_eq!(
            classify_remote_branch(&squashed, now),
            Some(StaleReason::PullMerged(9))
        );
        let idle = RemoteBranchFacts {
            last_commit_ms: now - 20 * DAY_MS,
            ..base.clone()
        };
        let reason = classify_remote_branch(&idle, now);
        assert_eq!(reason, Some(StaleReason::Idle(20)));
        assert!(!reason.is_some_and(|r| r.safe()));
        let unknown = RemoteBranchFacts {
            pulls: None,
            last_commit_ms: now - 90 * DAY_MS,
            ..base
        };
        assert_eq!(classify_remote_branch(&unknown, now), None);
    }

    #[test]
    fn local_branches_and_worktrees() {
        let facts = LocalBranchFacts {
            protected: false,
            checked_out: false,
            on_remote: true,
        };
        assert_eq!(classify_local_branch(facts), Some(StaleReason::LocalCopy));
        for unsafe_facts in [
            LocalBranchFacts {
                on_remote: false,
                ..facts
            },
            LocalBranchFacts {
                checked_out: true,
                ..facts
            },
            LocalBranchFacts {
                protected: true,
                ..facts
            },
        ] {
            assert_eq!(classify_local_branch(unsafe_facts), None);
        }
        let task = |archived, merged, live| WorktreeOwner::Task {
            number: 5,
            archived,
            merged,
            live,
        };
        assert_eq!(
            classify_worktree(WorktreeOwner::Deleted, false),
            Some(StaleReason::TaskDeleted)
        );
        assert_eq!(
            classify_worktree(task(true, false, false), false),
            Some(StaleReason::TaskArchived(5))
        );
        assert_eq!(
            classify_worktree(task(false, true, false), false),
            Some(StaleReason::TaskMerged(5))
        );
        assert_eq!(classify_worktree(task(true, true, true), false), None);
        assert_eq!(classify_worktree(task(false, false, false), false), None);
        assert_eq!(classify_worktree(WorktreeOwner::Other, false), None);
        assert_eq!(
            classify_worktree(WorktreeOwner::Other, true),
            Some(StaleReason::Missing)
        );
    }
}
