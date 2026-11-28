//! Yulius-run CI and repository hygiene (SPEC-001 DLV-006/007, SPEC-002 ADR-21).
//!
//! Yulius is the projects' only CI (F 2026-10-10: no GitHub Actions): it runs each sandbox's
//! **CI jobs** for every open pull request head into the integration branch and for the
//! integration and production heads, on its own machine, in parallel, and publishes the result
//! as the commit status [`STATUS_CONTEXT`]. This module holds the deterministic parts: the CI
//! configuration (a single gate command or `[[ci.jobs]]` with matrix splits), its source, run
//! and job states, the commit status of a run, the merge gate, and which branches and worktrees
//! are stale.

pub mod actions;
pub mod failures;
pub mod flaky;
pub mod infra;
pub mod vidoc;

use crate::delivery::ChecksState;
use crate::resources::Need;

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
/// Most agent secrets one job may name (SPEC-142).
pub const MAX_JOB_SECRETS: usize = 8;
/// Largest `timeout_minutes` a job may set (as the instance's `YULIUS_CI_JOB_TIMEOUT_MIN`).
pub const MAX_JOB_TIMEOUT_MINUTES: u64 = 360;
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
    /// The integration branch's last valid `yulius.toml`, while its own head's is invalid
    /// (SPEC-504 CFG-003): the gate that judged it before, until a fix merges.
    LastValid,
}

impl GateSource {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::ProjectConfig => "yulius_toml",
            Self::PullRequest => "pull_request",
            Self::LastValid => "last_valid_yulius_toml",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "profile" => Some(Self::Profile),
            "yulius_toml" => Some(Self::ProjectConfig),
            "pull_request" => Some(Self::PullRequest),
            "last_valid_yulius_toml" => Some(Self::LastValid),
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
    /// The CPUs and memory each of its jobs declares (`cpus`, `memory_gb`; SPEC-082 RES-002).
    pub need: Need,
    /// Agent secrets its jobs may use through the egress proxy (`secrets`, SPEC-142). Only the
    /// integration branch's list counts: Yulius reads it when a job starts, for a job with that
    /// entry's name and command (SPEC-264).
    pub secrets: Vec<String>,
    /// Its own time limit in minutes (`timeout_minutes`, SPEC-435 JTL-003); `None`: the
    /// instance's.
    pub timeout_minutes: Option<u32>,
    /// One run of each of its jobs at a time in the sandbox (`exclusive`, SPEC-435 JTL-001).
    pub exclusive: bool,
    /// The files it tests (`paths`, SPEC-493 CJP-001): scope-claim globs, normalized. Empty:
    /// every change.
    pub paths: Vec<String>,
    /// Part of the full suite (`after_merge`, SPEC-575 MFS-002): with a merge-first environment
    /// it runs on the merged revision, not on pull request heads.
    pub after_merge: bool,
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
    /// What it declares it needs (SPEC-082 RES-002).
    pub need: Need,
    /// The agent secrets its definition names (SPEC-142).
    pub secrets: Vec<String>,
    /// Its definition's own time limit in minutes (SPEC-435); `None`: the instance's.
    pub timeout_minutes: Option<u32>,
    /// One run of it at a time in the sandbox (SPEC-435 JTL-001).
    pub exclusive: bool,
    /// Its definition's `paths` (SPEC-493 CJP-001).
    pub paths: Vec<String>,
    /// Its definition's `after_merge`, in force only while the integration head has a
    /// merge-first environment that deploys ([`after_merge_in_force`], SPEC-575 MFS-002).
    pub after_merge: bool,
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
    /// A `secrets` list with an invalid or repeated name, or too many names.
    #[error(
        "CI job '{0}': secrets are agent secret names separated by spaces (A-Z, 0-9, _), each once, at most 8"
    )]
    InvalidSecrets(String),
    /// More definitions or expanded jobs than allowed.
    #[error("CI may define at most 16 jobs and expand to at most 32")]
    TooManyJobs,
    /// A `paths` list that is empty, too long, repeated or holds an invalid glob.
    #[error(
        "CI job '{0}': paths are 1 to 50 repository-relative globs (`*`, `?`, `**`, a trailing `/` for a folder), each once"
    )]
    InvalidPaths(String),
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

/// Splits a job's `secrets` string (`"VIDOC_API_KEY OTHER"`) into agent secret names.
///
/// # Errors
/// [`CiConfigError::InvalidSecrets`] naming `job`: an empty list, an invalid or repeated name, or
/// more than [`MAX_JOB_SECRETS`].
pub fn parse_secrets(job: &str, text: &str) -> Result<Vec<String>, CiConfigError> {
    let names: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    let valid = !names.is_empty()
        && names.len() <= MAX_JOB_SECRETS
        && names
            .iter()
            .enumerate()
            .all(|(i, name)| crate::cd::valid_secret_name(name) && !names[..i].contains(name));
    if valid {
        Ok(names)
    } else {
        Err(CiConfigError::InvalidSecrets(job.to_owned()))
    }
}

/// Most `paths` a job may name (SPEC-493 CJP-001).
pub const MAX_JOB_PATHS: usize = 50;

/// The project file that defines the jobs: a change to it runs every job (SPEC-493 CJP-003).
pub const JOBS_FILE: &str = "yulius.toml";

/// How the reason of a job skipped for its `paths` begins (a requeue keeps that skip).
pub const PATHS_SKIP_REASON: &str = "no file this job tests changed";

/// A job's `paths` (`["crates/a/**", "docs/"]`), normalized as scope claims are.
///
/// # Errors
/// [`CiConfigError::InvalidPaths`] naming `job`: none, more than [`MAX_JOB_PATHS`], a glob the
/// scope claims refuse, or one given twice.
pub fn parse_paths(job: &str, items: &[String]) -> Result<Vec<String>, CiConfigError> {
    let invalid = || CiConfigError::InvalidPaths(job.to_owned());
    if items.is_empty() || items.len() > MAX_JOB_PATHS {
        return Err(invalid());
    }
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let pattern = crate::scope::Pattern::parse(item.trim()).map_err(|_| invalid())?;
        let text = pattern.as_str().to_owned();
        if out.contains(&text) {
            return Err(invalid());
        }
        out.push(text);
    }
    Ok(out)
}

/// What a skipped job of the full suite says (SPEC-575 MFS-002); a verdict copied from such a
/// job keeps it, so a branch's run can tell a source that never ran the job.
pub const AFTER_MERGE_SKIP_REASON: &str =
    "part of the full suite: it runs on the merged revision (merge-first)";

/// The jobs with `after_merge` kept only when the integration head has a merge-first
/// environment that deploys (`merge_first`); without one each job runs on every commit, so a
/// project without a shared staging target keeps its full suite on the pull request (SPEC-575
/// MFS-002).
#[must_use]
pub fn after_merge_in_force(mut jobs: Vec<Job>, merge_first: bool) -> Vec<Job> {
    if !merge_first {
        for job in &mut jobs {
            job.after_merge = false;
        }
    }
    jobs
}

/// Why a job of the full suite is skipped for a run of `reference`, or `None` when it runs: a
/// pull request head, a merge train and a playground task skip it; the integration and
/// production branches' runs (the merged revision) run it (SPEC-575 MFS-002).
#[must_use]
pub fn after_merge_skip(job: &Job, reference: &CiRef) -> Option<String> {
    let off_the_merge = matches!(
        reference,
        CiRef::PullRequest(_) | CiRef::Train(_) | CiRef::Task(_)
    );
    (job.after_merge && off_the_merge).then(|| AFTER_MERGE_SKIP_REASON.to_owned())
}

/// A job ran and returned the CI skip exit code. A copied verdict retains this
/// distinction from a job the scheduler excluded before execution.
pub const EXECUTED_SKIP_REASON: &str = "job exited with the CI skip code";

/// Carry the origin of a skipped verdict across tree reuse and merge trains.
/// Old copies with ambiguous reasons remain ambiguous.
#[must_use]
pub fn copied_job_reason(state: RunState, source: Option<&str>, note: &str) -> String {
    if state == RunState::Skipped {
        let kind = match source {
            None => Some(EXECUTED_SKIP_REASON),
            Some(reason) => [
                AFTER_MERGE_SKIP_REASON,
                PATHS_SKIP_REASON,
                EXECUTED_SKIP_REASON,
            ]
            .into_iter()
            .find(|prefix| reason.starts_with(prefix)),
        };
        if let Some(kind) = kind {
            return format!("{kind}; {note}");
        }
    }
    note.to_owned()
}

/// Whether a job of a run was skipped as part of the full suite there (its own skip, or a copy
/// of one): such a run never ran it (SPEC-575 MFS-002).
#[must_use]
pub fn skipped_after_merge(state: RunState, reason: Option<&str>) -> bool {
    state == RunState::Skipped && reason.is_some_and(|r| r.starts_with(AFTER_MERGE_SKIP_REASON))
}

/// Why a job with `paths` is skipped for a change of `changed` files (SPEC-493 CJP-002,
/// CJP-003), or `None` when it runs: a job without `paths`, an unknown change (`None`), a change
/// to [`JOBS_FILE`] and a change touching one of its paths all run it.
#[must_use]
pub fn paths_skip(paths: &[String], changed: Option<&[String]>) -> Option<String> {
    let changed = changed?;
    if paths.is_empty() || changed.iter().any(|file| file == JOBS_FILE) {
        return None;
    }
    let mut patterns: Vec<crate::scope::Pattern> = Vec::with_capacity(paths.len() * 2);
    for text in paths {
        // A path that no longer parses tests everything: never skipped on doubt.
        let Ok(pattern) = crate::scope::Pattern::parse(text) else {
            return None;
        };
        // A name without a glob (`crates/web`) is the file and the folder of that name.
        if !text.contains(['*', '?'])
            && let Ok(folder) = crate::scope::Pattern::parse(&format!("{text}/"))
        {
            patterns.push(folder);
        }
        patterns.push(pattern);
    }
    // A changed path no pattern can be matched against (a backslash, a control character)
    // could be one the job tests: it runs.
    if changed
        .iter()
        .any(|file| crate::scope::Pattern::parse(file).is_err())
    {
        return None;
    }
    let touched = changed.iter().any(|file| {
        patterns
            .iter()
            .any(|pattern| crate::scope::pattern_matches(pattern, file))
    });
    (!touched).then(|| format!("{PATHS_SKIP_REASON} (paths: {})", paths.join(", ")))
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
                need: def.need,
                secrets: def.secrets.clone(),
                timeout_minutes: def.timeout_minutes,
                exclusive: def.exclusive,
                paths: def.paths.clone(),
                after_merge: def.after_merge,
            });
        } else {
            for value in &def.matrix {
                jobs.push(Job {
                    name: format!("{} ({value})", def.name),
                    key: def.name.clone(),
                    command: command.clone(),
                    matrix: Some(value.clone()),
                    need: def.need,
                    secrets: def.secrets.clone(),
                    timeout_minutes: def.timeout_minutes,
                    exclusive: def.exclusive,
                    paths: def.paths.clone(),
                    after_merge: def.after_merge,
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
        need: Need::default(),
        secrets: Vec::new(),
        timeout_minutes: None,
        exclusive: false,
        paths: Vec::new(),
        after_merge: false,
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

/// The key of the built-in Vidoc security job (SPEC-217 VDP-001).
pub const SECURITY_JOB: &str = "security";
/// What the built-in security job runs: Yulius's embedded scanner in CI mode (SPEC-195).
pub const SECURITY_COMMAND: &str = "yulius security-scan --ci";
/// The agent secret the built-in security job is granted.
pub const VIDOC_SECRET: &str = "VIDOC_API_KEY";

/// Whether `job` runs the security scan: keyed `security`, or running the scanner.
fn scans(job: &Job) -> bool {
    job.key == SECURITY_JOB
        || job.command.contains("security-scan")
        || job.command.contains("vidoc_scan")
}

fn built_in_security_job() -> Job {
    Job {
        name: SECURITY_JOB.to_owned(),
        key: SECURITY_JOB.to_owned(),
        command: SECURITY_COMMAND.to_owned(),
        matrix: None,
        need: Need::default(),
        secrets: vec![VIDOC_SECRET.to_owned()],
        timeout_minutes: None,
        exclusive: false,
        paths: Vec::new(),
        after_merge: false,
    }
}

/// `gate` with the built-in security job (SPEC-217 VDP-001) added, unless it already has a job
/// keyed `security` or one that runs the scanner (`security-scan`, `vidoc_scan`). For the trusted
/// CI (the profile's or the integration branch's) only. It joins even a gate at [`MAX_JOBS`]: the
/// limit bounds what a project configures, not the scan Yulius adds.
#[must_use]
pub fn with_security_job(mut gate: Gate) -> Gate {
    if !gate.jobs.iter().any(scans) {
        gate.jobs.push(built_in_security_job());
    }
    gate
}

/// The jobs of a pull request tested with its own configuration (SPEC-217 VDP-001), with the
/// trusted security job(s) it cannot drop: the scanning jobs of `trusted` (the integration
/// branch's CI, with the built-in job already added), else, with `vidoc`, the built-in job. Each
/// replaces the pull request's job of the same key, so a job it named `security` or one that
/// only mentions the scanner never stands in for the scan.
#[must_use]
pub fn keep_security_jobs(mut own: Gate, trusted: Option<&Gate>, vidoc: bool) -> Gate {
    let mut kept: Vec<Job> = trusted
        .map(|gate| gate.jobs.iter().filter(|job| scans(job)).cloned().collect())
        .unwrap_or_default();
    if kept.is_empty() && vidoc {
        kept.push(built_in_security_job());
    }
    own.jobs
        .retain(|job| !kept.iter().any(|k| k.key == job.key));
    // Even past [`MAX_JOBS`]: the trusted gate's own jobs are bounded by it.
    own.jobs.extend(kept);
    own
}

/// What names a job of a change's own beside the gate's (SPEC-504 CFG-001).
pub const CHANGE_SUFFIX: &str = " (this change)";
/// What its checkout's name adds to the job's key: `<key>.change` (SPEC-504 review round 2).
pub const CHANGE_CHECKOUT: &str = "change";

/// `gate` (the integration branch's) with the jobs of a change's own `yulius.toml` (`own`) that
/// the change itself changed or added (in any field: its command, matrix, budget, time limit,
/// `exclusive`, secrets or `paths`): those that differ from all of `gate`'s and from all of
/// the jobs at its merge base with the integration branch (`at_base`; unknown: every own job
/// that differs from the gate's), named with [`CHANGE_SUFFIX`] and keeping their key (SPEC-504
/// CFG-001, ADR-0360). Every job of `gate` still runs, so a change never weakens the gate that
/// judges it, and a job it changes or adds runs before it merges, so a failing one never lands
/// untested to judge its own repair; a job it only carries from its base (one the integration
/// branch changed or removed since) is not its change and does not run (review round 2). None
/// takes the key of the gate's scanning job (SPEC-217 VDP-001: the trusted scan stays the only
/// one), and a profile's gate (not `yulius.toml`'s) gets none. Nothing is left out for
/// [`MAX_JOBS`]: the change's own file is bounded by it, so the run holds at most twice that.
#[must_use]
pub fn with_changed_jobs(mut gate: Gate, own: Vec<Job>, at_base: Option<&[Job]>) -> Gate {
    if gate.source == GateSource::Profile {
        return gate;
    }
    // Equal in everything that decides how it runs (command, matrix, budget, time limit,
    // `exclusive`, secrets, `paths`): a change to any of it is the change's (review round 3).
    // Moving a job into or out of the full suite (`after_merge`, SPEC-575) is not how it runs,
    // and a job a change adds or changes runs before it merges (ADR-0360): the change's own jobs
    // and what they are compared with are read without the flag.
    let flagless = |jobs: &[Job]| -> Vec<Job> {
        jobs.iter()
            .map(|job| Job {
                after_merge: false,
                ..job.clone()
            })
            .collect()
    };
    let (kept, at_base) = (flagless(&gate.jobs), at_base.map(flagless));
    let own = flagless(&own);
    let same = |a: &Job, b: &Job| a == b;
    let scanning: Vec<String> = gate
        .jobs
        .iter()
        .filter(|job| scans(job))
        .map(|job| job.key.clone())
        .collect();
    let changed: Vec<Job> = own
        .into_iter()
        .filter(|job| !kept.iter().any(|k| same(k, job)))
        .filter(|job| {
            !at_base
                .as_ref()
                .is_some_and(|base| base.iter().any(|old| same(old, job)))
        })
        .filter(|job| !scanning.contains(&job.key))
        .map(|job| Job {
            name: format!("{}{CHANGE_SUFFIX}", job.name),
            ..job
        })
        .collect();
    gate.jobs.extend(changed);
    gate
}

/// The state of a run from its jobs' states: running while any job is active, then failed if
/// any failed, error if any errored, cancelled if any was cancelled, else passed (skipped jobs
/// included, SPEC-162). A run without jobs is an error.
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
    /// A job that exited with [`SKIP_EXIT_CODE`]: it checked nothing and says so (SPEC-162). It
    /// blocks no merge and is never counted as passed; a run is never skipped itself.
    Skipped,
}

/// The exit code with which a CI job says it was skipped (SPEC-162 SKP-001, ADR-0133), given to
/// every job as `YULIUS_CI_SKIP_CODE`: `EX_CONFIG` from `sysexits.h`, Drone's "skip" code.
pub const SKIP_EXIT_CODE: i32 = 78;

/// The exit code with which a CI job says it could not check everything for a passing reason
/// (an outage of the service it asks; SPEC-273 NOI-006, ADR-0202), given to every job as
/// `YULIUS_CI_INCOMPLETE_CODE`: `EX_TEMPFAIL` from `sysexits.h`. The job ends
/// [`RunState::Error`], which blocks the merge until a rerun passes, with
/// [`incomplete_reason`].
pub const INCOMPLETE_EXIT_CODE: i32 = 75;

/// The prefix of the output line with which such a job names what it left unchecked.
pub const INCOMPLETE_MARK: &str = "::yulius-incomplete ";

/// The reason of a job that exited [`INCOMPLETE_EXIT_CODE`]: the text of the last
/// [`INCOMPLETE_MARK`] line of its `output` (control characters dropped, at most 300
/// characters), else a generic one. It says infrastructure, never a finding.
#[must_use]
pub fn incomplete_reason(output: &str) -> String {
    let said = output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(INCOMPLETE_MARK))
        .map(|text| {
            let text: String = text.chars().filter(|c| !c.is_control()).take(300).collect();
            text.trim().to_owned()
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "the job could not check everything".to_owned());
    format!("incomplete: {said}; infrastructure, not a finding: run it again")
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
            Self::Skipped => "skipped",
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
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }

    /// Queued or running.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// What a run tests: a pull request head, a branch head (a push) or a merge train.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CiRef {
    /// Open pull request into the integration branch.
    PullRequest(u64),
    /// The integration or production branch.
    Branch(String),
    /// A merge train (SPEC-084): the integration branch with these pull requests merged in this
    /// order, a commit that exists only in Yulius's clone (no commit status is published for it).
    Train(Vec<u64>),
    /// A playground task's head merged into the integration branch (SPEC-163): a commit that
    /// exists only in Yulius's clone, tested when its agent calls `delivery_merge`.
    Task(u64),
    /// A merged revision of the integration branch (its full SHA) that a merge-first staging
    /// request waits on (SPEC-575 MFS-003): the full gate on exactly that commit, started and
    /// kept by the staging queue, never superseded by the branch moving on.
    Candidate(String),
}

impl CiRef {
    /// Stable key (`pr/12`, `branch/dev`, `train/12+15`, `task/7`): one active run per key.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::PullRequest(number) => format!("pr/{number}"),
            Self::Branch(name) => format!("branch/{name}"),
            Self::Train(numbers) => format!("train/{}", joined(numbers, "+", "")),
            Self::Task(number) => format!("task/{number}"),
            Self::Candidate(sha) => format!("candidate/{sha}"),
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
        if let Some(number) = key.strip_prefix("task/") {
            return number.parse().ok().filter(|n| *n > 0).map(Self::Task);
        }
        if let Some(sha) = key.strip_prefix("candidate/") {
            let hex =
                !sha.is_empty() && sha.len() <= 64 && sha.bytes().all(|b| b.is_ascii_hexdigit());
            return hex.then(|| Self::Candidate(sha.to_owned()));
        }
        if let Some(numbers) = key.strip_prefix("train/") {
            let parsed: Option<Vec<u64>> = numbers
                .split('+')
                .map(|n| n.parse::<u64>().ok().filter(|n| *n > 0))
                .collect();
            return parsed.filter(|n| !n.is_empty()).map(Self::Train);
        }
        key.strip_prefix("branch/")
            .filter(|name| !name.is_empty())
            .map(|name| Self::Branch(name.to_owned()))
    }

    /// Words for people: `PR #12`, `dev`, `Merge train #12 + #15`, `Task #7`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::PullRequest(number) => format!("PR #{number}"),
            Self::Branch(name) => name.clone(),
            Self::Train(numbers) => format!("Merge train {}", joined(numbers, " + ", "#")),
            Self::Task(number) => format!("Task #{number}"),
            Self::Candidate(sha) => format!("Candidate {}", sha.get(..9).unwrap_or(sha)),
        }
    }

    /// Whether this run tests pull request `number`: its head, or a merge train with it.
    #[must_use]
    pub fn covers(&self, number: u64) -> bool {
        match self {
            Self::PullRequest(n) => *n == number,
            Self::Train(numbers) => numbers.contains(&number),
            Self::Branch(_) | Self::Task(_) | Self::Candidate(_) => false,
        }
    }
}

fn joined(numbers: &[u64], separator: &str, prefix: &str) -> String {
    numbers
        .iter()
        .map(|n| format!("{prefix}{n}"))
        .collect::<Vec<_>>()
        .join(separator)
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
    /// It skipped a job a branch run would run (a `paths` skip, an `after_merge` job): no
    /// verdict for a branch at its commit (#1532).
    pub partial: bool,
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
///   only by "Run again". For a branch, a run that skipped a job the branch runs (`partial`)
///   counts neither: the branch's own run runs every job (#1532).
/// - An active run whose reference now points to another commit, or that no longer exists
///   (pull request closed or merged), is superseded.
#[must_use]
pub fn plan(targets: &[Target], runs: &[RunFacts]) -> Plan {
    let mut out = Plan::default();
    for target in targets {
        let branch = matches!(target.reference, CiRef::Branch(_));
        let tested = runs.iter().any(|run| {
            run.sha == target.sha
                && covers_shared_commit(run.state)
                && !(branch && run.partial && run.reference != target.reference)
        });
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
    // A candidate run is the staging queue's: the branch moving on never supersedes it
    // (SPEC-575 MFS-003), and it still counts as testing its commit above.
    for run in runs
        .iter()
        .filter(|run| run.state.is_active() && !matches!(run.reference, CiRef::Candidate(_)))
    {
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

/// The state and exit code of a job copied from a passed run (a tree reused, a merge train's
/// verdict): a job that was skipped there stays skipped (SPEC-162), every other one passed.
#[must_use]
pub fn copied_job_verdict(source: Option<RunState>) -> (RunState, i32) {
    match source {
        Some(RunState::Skipped) => (RunState::Skipped, SKIP_EXIT_CODE),
        _ => (RunState::Passed, 0),
    }
}

/// Whether the scheduler can use a run to cover another reference at the same commit.
#[must_use]
pub fn covers_shared_commit(state: RunState) -> bool {
    state.is_active() || matches!(state, RunState::Passed | RunState::Failed | RunState::Error)
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
    /// Jobs that were skipped (SPEC-162).
    pub skipped: usize,
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
            match summary.reason {
                Some(reason) => format!("Queued on Yulius ({}): {reason}", jobs_word(summary.jobs)),
                None => format!("Queued on Yulius ({})", jobs_word(summary.jobs)),
            },
        ),
        RunState::Running => (
            CommitState::Pending,
            // A failed job that stopped none of the others (the security job, SPEC-273) shows
            // while they run.
            format!(
                "Running on Yulius: {} of {} passed{}{}",
                summary.passed,
                jobs_word(summary.jobs),
                match summary.skipped {
                    0 => String::new(),
                    n => format!(", {n} skipped"),
                },
                summary
                    .failed_job
                    .map(|(job, _)| format!(", {job} failed"))
                    .unwrap_or_default()
            ),
        ),
        RunState::Passed | RunState::Skipped => (
            CommitState::Success,
            match summary.skipped {
                0 => format!("Passed: {}{took}", jobs_word(summary.jobs)),
                n => format!("Passed: {}, {n} skipped{took}", jobs_word(summary.jobs)),
            },
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
        GateResult::Run(Some(RunState::Passed | RunState::Skipped)) => ChecksState::Success,
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
    /// Its commits are already in the integration or production branch, and no pull request had
    /// it as its head (or none was looked up): a person removes it, not the automatic round.
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
    /// A task worktree whose task is closed or was cancelled (its branch keeps the work;
    /// Continue recreates the worktree).
    TaskEnded(u32),
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
            Self::TaskEnded(_) => "task_ended",
            Self::Missing => "missing",
        }
    }

    /// Words for people and agents.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::PullMerged(n) => format!("pull request #{n} was merged"),
            Self::PullClosed(n) => format!("pull request #{n} was closed without merging"),
            Self::Merged => {
                "its commits are already in the integration or production branch".to_owned()
            }
            Self::LocalCopy => "no worktree uses it and its commits are on GitHub".to_owned(),
            Self::Idle(days) => format!("no pull request and no commit for {days} days"),
            Self::TaskDeleted => "its task was deleted".to_owned(),
            Self::TaskArchived(n) => format!("task #{n} is archived"),
            Self::TaskMerged(n) => format!("task #{n}'s pull request is merged"),
            Self::TaskEnded(n) => format!("task #{n} is closed or cancelled"),
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
    /// The branch's tip is the pull request's head: GitHub keeps every commit of the branch on
    /// the pull request (SPEC-269 review round 1). Commits pushed after it closed are not.
    pub tip_on_pull: bool,
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
    // A closed or merged pull request keeps the branch's commits only while the branch has
    // nothing after its head; later commits are unmerged work, judged as such below. It is the
    // reason even for a branch already in the integration or production branch: `Merged` then
    // means no merged pull request was found for it (none had it, or its lookup was skipped),
    // which the automatic round leaves to a person (SPEC-269).
    if let Some(last) = pulls.first().filter(|p| p.tip_on_pull) {
        return Some(if last.merged {
            StaleReason::PullMerged(last.number)
        } else {
            StaleReason::PullClosed(last.number)
        });
    }
    if facts.merged {
        return Some(StaleReason::Merged);
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
        /// The task is closed or was cancelled.
        ended: bool,
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
        WorktreeOwner::Task {
            number,
            ended: true,
            ..
        } => Some(StaleReason::TaskEnded(number)),
        WorktreeOwner::Task { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(key: &str, after_merge: bool) -> Job {
        Job {
            name: key.to_owned(),
            key: key.to_owned(),
            command: format!("run {key}"),
            matrix: None,
            need: Need::default(),
            secrets: Vec::new(),
            timeout_minutes: None,
            exclusive: false,
            paths: Vec::new(),
            after_merge,
        }
    }

    /// SPEC-575 MFS-002: `after_merge` is in force only with a merge-first environment that
    /// deploys; a project without a shared target keeps its full suite everywhere.
    #[test]
    fn after_merge_is_in_force_only_with_a_merge_first_target() {
        let jobs = vec![plain("quick", false), plain("e2e", true)];
        let off = after_merge_in_force(jobs.clone(), false);
        assert!(off.iter().all(|job| !job.after_merge), "{off:?}");
        assert_eq!(after_merge_in_force(jobs.clone(), true), jobs);
    }

    /// SPEC-575 MFS-002: a job of the full suite is skipped, with the reason, on a pull request
    /// head, a merge train and a playground task; the branches' runs run it, and a job of the
    /// quick checks runs everywhere.
    #[test]
    fn the_full_suite_is_skipped_off_the_merged_revision() {
        let full = plain("e2e", true);
        let quick = plain("quick", false);
        for reference in [
            CiRef::PullRequest(7),
            CiRef::Train(vec![7, 8]),
            CiRef::Task(3),
        ] {
            let why = after_merge_skip(&full, &reference).unwrap_or_default();
            assert!(why.starts_with(AFTER_MERGE_SKIP_REASON), "{why}");
            assert_eq!(after_merge_skip(&quick, &reference), None);
        }
        assert_eq!(after_merge_skip(&full, &CiRef::Branch("dev".into())), None);
        assert_eq!(after_merge_skip(&full, &CiRef::Branch("main".into())), None);
    }

    /// SPEC-575 MFS-002 (review round 1): a copied verdict keeps saying the full suite was
    /// skipped, and only such a skip counts as one.
    #[test]
    fn a_copied_verdict_keeps_the_full_suites_skip() {
        let own = after_merge_skip(&plain("e2e", true), &CiRef::PullRequest(1));
        let copied = copied_job_reason(
            RunState::Skipped,
            own.as_deref(),
            "not run again: same tree",
        );
        assert!(
            skipped_after_merge(RunState::Skipped, Some(&copied)),
            "{copied}"
        );
        assert!(copied.ends_with("not run again: same tree"), "{copied}");
        let twice = copied_job_reason(RunState::Skipped, Some(&copied), "a train's verdict");
        assert!(
            skipped_after_merge(RunState::Skipped, Some(&twice)),
            "{twice}"
        );
        let paths = copied_job_reason(RunState::Skipped, Some(PATHS_SKIP_REASON), "n");
        assert!(paths.starts_with(PATHS_SKIP_REASON));
        assert!(
            copied_job_reason(RunState::Skipped, Some(&paths), "again")
                .starts_with(PATHS_SKIP_REASON)
        );
        let runtime = copied_job_reason(RunState::Skipped, None, "n");
        assert!(runtime.starts_with(EXECUTED_SKIP_REASON));
        assert!(
            copied_job_reason(RunState::Skipped, Some(&runtime), "copy")
                .starts_with(EXECUTED_SKIP_REASON)
        );
        assert_eq!(copied_job_reason(RunState::Passed, None, "n"), "n");
        assert!(!skipped_after_merge(RunState::Passed, Some(&copied)));
        assert!(!skipped_after_merge(
            RunState::Skipped,
            Some(PATHS_SKIP_REASON)
        ));
        assert!(!skipped_after_merge(RunState::Skipped, None));
    }

    /// SPEC-575 MFS-002: a change that only moves a job into or out of the full suite changes
    /// nothing about how the job runs, so no twin of it runs beside the gate's; a job a change
    /// adds or changes runs before it merges (ADR-0360), so its twin is never part of the full
    /// suite.
    #[test]
    fn a_changes_own_jobs_never_leave_its_pull_request() {
        let gate = Gate {
            jobs: vec![plain("e2e", false), plain("slow", true)],
            source: GateSource::ProjectConfig,
        };
        let moved = with_changed_jobs(
            gate.clone(),
            vec![plain("e2e", true), plain("slow", false)],
            None,
        );
        assert_eq!(moved, gate);
        let added = with_changed_jobs(gate.clone(), vec![plain("new", true)], None);
        let twin = added
            .jobs
            .last()
            .cloned()
            .unwrap_or_else(|| plain("", true));
        assert_eq!(
            (twin.name.as_str(), twin.after_merge),
            ("new (this change)", false)
        );
        assert_eq!(&added.jobs[..2], &gate.jobs[..], "the gate keeps its flags");
    }

    /// SPEC-504 CFG-001: a change's own jobs run beside the gate's, never instead: changed and
    /// added ones, named apart and keeping their key; equal ones once; within [`MAX_JOBS`].
    #[test]
    fn a_changes_own_jobs_run_beside_the_gates() {
        let job = |key: &str, command: &str| Job {
            name: key.to_owned(),
            key: key.to_owned(),
            command: command.to_owned(),
            matrix: None,
            need: Need::default(),
            secrets: Vec::new(),
            timeout_minutes: None,
            exclusive: false,
            paths: Vec::new(),
            after_merge: false,
        };
        let gate = Gate {
            jobs: vec![job("check", "make check"), job("lint", "make lint")],
            source: GateSource::ProjectConfig,
        };
        let own = vec![
            job("check", "make check"),
            job("lint", "true"),
            job("added", "make more"),
        ];
        let both = with_changed_jobs(gate.clone(), own.clone(), None);
        let names: Vec<&str> = both.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(
            names,
            ["check", "lint", "lint (this change)", "added (this change)"]
        );
        assert_eq!(both.jobs[2].key, "lint");
        assert_eq!(both.jobs[2].command, "true");
        assert_eq!(both.source, GateSource::ProjectConfig);
        assert_eq!(
            with_changed_jobs(gate.clone(), Vec::new(), None),
            gate,
            "no own jobs: the gate"
        );
        // What it only carries from its base is not its change (review round 2): the gate
        // changed `lint` and removed `added` since.
        let base = vec![job("lint", "true"), job("added", "make more")];
        assert_eq!(with_changed_jobs(gate.clone(), own, Some(&base)), gate);
        // Never past what its own file can hold, never dropped.
        let many: Vec<Job> = (0..MAX_JOBS).map(|n| job(&format!("j{n}"), "x")).collect();
        assert_eq!(
            with_changed_jobs(gate.clone(), many, None).jobs.len(),
            2 + MAX_JOBS
        );
        // A change to anything else of a job (its time limit, its budget, `exclusive`, its
        // secrets or `paths`) is a change too: its twin runs with the new value (review round 3).
        let slower = Job {
            timeout_minutes: Some(5),
            ..job("check", "make check")
        };
        let both = with_changed_jobs(gate.clone(), vec![slower], Some(&gate.jobs));
        assert_eq!(both.jobs.len(), 3, "{both:?}");
        assert_eq!(both.jobs[2].name, "check (this change)");
        assert_eq!(both.jobs[2].timeout_minutes, Some(5));
        // A profile's gate is not `yulius.toml`'s: nothing of the file runs beside it.
        let profile = Gate {
            source: GateSource::Profile,
            ..gate.clone()
        };
        assert_eq!(
            with_changed_jobs(profile.clone(), vec![job("x", "y")], None),
            profile
        );
        // A job of its own under the trusted scan's key never runs beside it.
        let scanned = with_security_job(gate);
        let key = scanned
            .jobs
            .last()
            .map(|j| j.key.clone())
            .unwrap_or_default();
        let fake = with_changed_jobs(scanned.clone(), vec![job(&key, "echo fake pass")], None);
        assert_eq!(fake, scanned);
    }

    /// SPEC-493 CJP-002, CJP-003 (AC-16): a job with `paths` is skipped, with the reason, only
    /// for a known change that touches none of them and not `yulius.toml`.
    #[test]
    fn a_job_runs_only_for_a_change_to_its_paths() {
        let paths = vec!["crates/a/**".to_owned(), "docs/*.md".to_owned()];
        let skip = |changed: &[&str]| {
            let changed: Vec<String> = changed.iter().map(|s| (*s).to_owned()).collect();
            paths_skip(&paths, Some(&changed))
        };
        assert_eq!(
            skip(&["web/x.js"]).as_deref(),
            Some("no file this job tests changed (paths: crates/a/**, docs/*.md)")
        );
        assert_eq!(skip(&["web/x.js", "crates/a/src/lib.rs"]), None);
        assert_eq!(skip(&["docs/x.md"]), None);
        assert!(
            skip(&["docs/sub/x.md"]).is_some(),
            "`*` stays in its folder"
        );
        assert_eq!(skip(&["yulius.toml"]), None, "its own definition");
        assert_eq!(paths_skip(&paths, None), None, "never skipped on doubt");
        assert_eq!(paths_skip(&[], Some(&["web/x.js".to_owned()])), None);
        assert!(skip(&[]).is_some(), "a change of no file touches none");
        // A name without a glob is the folder of that name too.
        let folder = vec!["crates/pomerado".to_owned()];
        let touches = |file: &str| paths_skip(&folder, Some(&[file.to_owned()])).is_none();
        assert!(touches("crates/pomerado/src/lib.rs") && touches("crates/pomerado"));
        assert!(!touches("crates/pomerado2/src/lib.rs"));
        // A changed path that cannot be matched runs the job.
        assert_eq!(skip(&["web\\x.js"]), None);
    }

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
            partial: false,
        }
    }

    /// #1532: a branch moved straight to a pull request head whose run skipped a job (`paths`
    /// or an `after_merge` job) gets a run of its own, which runs every job; a head whose run
    /// ran every job is still tested once; and pull requests keep reusing the partial verdict.
    #[test]
    fn a_partial_verdict_is_no_verdict_for_a_branch() {
        let dev = target(CiRef::Branch("dev".into()), "abc");
        let partial = RunFacts {
            partial: true,
            ..run("r1", CiRef::PullRequest(1), "abc", RunState::Passed)
        };
        assert_eq!(
            plan(std::slice::from_ref(&dev), &[partial.clone()]).queue,
            [dev.clone()]
        );
        let whole = run("r1", CiRef::PullRequest(1), "abc", RunState::Passed);
        assert!(plan(std::slice::from_ref(&dev), &[whole]).queue.is_empty());
        let pr = target(CiRef::PullRequest(2), "abc");
        assert!(
            plan(std::slice::from_ref(&pr), &[partial.clone()])
                .queue
                .is_empty()
        );
        // An active partial run does not stand for the branch either; the branch's own does.
        let active = RunFacts {
            state: RunState::Running,
            ..partial
        };
        assert_eq!(
            plan(std::slice::from_ref(&dev), &[active]).queue,
            [dev.clone()]
        );
        let own = run("r2", CiRef::Branch("dev".into()), "abc", RunState::Running);
        assert!(plan(std::slice::from_ref(&dev), &[own]).queue.is_empty());
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

    /// SPEC-217 VDP-001: the built-in security job joins a gate once, granted the Vidoc key; a
    /// gate that already scans keeps its own job.
    #[test]
    fn the_built_in_security_job_joins_a_gate_that_does_not_scan() -> Result<(), CiConfigError> {
        let def = |name: &str, command: &str| JobDef {
            name: name.into(),
            command: command.into(),
            matrix: Vec::new(),
            need: Need::default(),
            secrets: Vec::new(),
            timeout_minutes: None,
            exclusive: false,
            paths: Vec::new(),
            after_merge: false,
        };
        let gate = |defs: &[JobDef]| -> Result<Gate, CiConfigError> {
            Ok(Gate {
                jobs: expand_jobs(defs)?,
                source: GateSource::ProjectConfig,
            })
        };
        let added = with_security_job(gate(&[def("test", "make test")])?);
        let names: Vec<_> = added.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, ["test", "security"]);
        assert_eq!(added.jobs[1].command, "yulius security-scan --ci");
        assert_eq!(added.jobs[1].secrets, ["VIDOC_API_KEY"]);
        assert_eq!(with_security_job(added.clone()), added, "added once");
        for own in [
            def("security", "make audit"),
            def("scan", "yulius security-scan --ci"),
            def("vidoc", "python3 scripts/vidoc_scan.py --ci"),
        ] {
            let mine = gate(&[def("test", "make test"), own])?;
            assert_eq!(with_security_job(mine.clone()), mine);
        }
        Ok(())
    }

    /// SPEC-217 VDP-001 (the branch's own CI scan, medium): a pull request tested with its own
    /// jobs cannot drop or replace the trusted security job, by its key or by a command that
    /// merely mentions the scanner.
    #[test]
    fn a_pull_request_with_its_own_jobs_keeps_the_trusted_security_job() -> Result<(), CiConfigError>
    {
        let def = |name: &str, command: &str| JobDef {
            name: name.into(),
            command: command.into(),
            matrix: Vec::new(),
            need: Need::default(),
            secrets: Vec::new(),
            timeout_minutes: None,
            exclusive: false,
            paths: Vec::new(),
            after_merge: false,
        };
        let gate = |source: GateSource, defs: &[JobDef]| -> Result<Gate, CiConfigError> {
            Ok(Gate {
                jobs: expand_jobs(defs)?,
                source,
            })
        };
        let commands = |gate: &Gate| -> Vec<(String, String)> {
            gate.jobs
                .iter()
                .map(|j| (j.key.clone(), j.command.clone()))
                .collect()
        };
        let trusted = with_security_job(gate(GateSource::ProjectConfig, &[def("test", "make")])?);
        let own = gate(
            GateSource::PullRequest,
            &[
                def("test", "make other"),
                def("security", "echo fake pass"),
                def("lint", "true # security-scan"),
            ],
        )?;
        let kept = keep_security_jobs(own.clone(), Some(&trusted), false);
        assert_eq!(
            commands(&kept),
            [
                ("test".to_owned(), "make other".to_owned()),
                ("lint".to_owned(), "true # security-scan".to_owned()),
                ("security".to_owned(), SECURITY_COMMAND.to_owned()),
            ]
        );
        assert_eq!(kept.source, GateSource::PullRequest);
        // Bootstrap (no trusted CI yet): with a Vidoc key, the built-in job; without, unchanged.
        let built_in = keep_security_jobs(own.clone(), None, true);
        assert_eq!(
            built_in.jobs.last().map(|j| j.command.as_str()),
            Some(SECURITY_COMMAND)
        );
        assert_eq!(
            built_in.jobs.len(),
            3,
            "the pull request's `security` job is replaced"
        );
        assert_eq!(keep_security_jobs(own.clone(), None, false), own);
        // The integration branch's own scanning job (this repository's) is the one kept.
        let mine = gate(
            GateSource::ProjectConfig,
            &[def("security", "python3 scripts/vidoc_scan.py --ci")],
        )?;
        let kept = keep_security_jobs(own, Some(&mine), true);
        assert_eq!(
            kept.jobs.last().map(|j| j.command.as_str()),
            Some("python3 scripts/vidoc_scan.py --ci")
        );
        // A pull request at the job limit still gets the scan (review round 6), and so does a
        // trusted gate at the limit.
        let one = expand_jobs(&[def("job", "true")])?.remove(0);
        let full = |source: GateSource| Gate {
            jobs: (0..MAX_JOBS)
                .map(|i| Job {
                    name: format!("job{i}"),
                    key: format!("job{i}"),
                    ..one.clone()
                })
                .collect(),
            source,
        };
        let kept = keep_security_jobs(full(GateSource::PullRequest), None, true);
        assert_eq!(
            kept.jobs.last().map(|j| j.command.as_str()),
            Some(SECURITY_COMMAND)
        );
        let trusted = with_security_job(full(GateSource::ProjectConfig));
        assert_eq!(
            trusted.jobs.last().map(|j| j.command.as_str()),
            Some(SECURITY_COMMAND)
        );
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
            need: Need::default(),
            secrets: Vec::new(),
            timeout_minutes: None,
            exclusive: false,
            paths: Vec::new(),
            after_merge: false,
        };
        let jobs = expand_jobs(&[
            def("lint", "cargo clippy", ""),
            def("test", "cargo test", "1/2 2/2"),
        ])?;
        let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, ["lint", "test (1/2)", "test (2/2)"]);
        assert_eq!(jobs[2].key, "test");
        assert_eq!(jobs[2].matrix.as_deref(), Some("2/2"));
        // A job's name is its key, which names its checkout directory: no path in it.
        for name in ["Lint", "../x", "..", ".x", "a/b", "/tmp", "a.b", ""] {
            assert!(
                matches!(
                    expand_jobs(&[def(name, "x", "")]),
                    Err(CiConfigError::InvalidJobName(_))
                ),
                "{name:?}"
            );
        }
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
            need: Need::default(),
            secrets: Vec::new(),
            timeout_minutes: None,
            exclusive: false,
            paths: Vec::new(),
            after_merge: false,
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
        // SPEC-162 SKP-002: a skipped job does not hold a run back, and never hides a failure.
        assert_eq!(aggregate(&[S::Passed, S::Skipped]), S::Passed);
        assert_eq!(aggregate(&[S::Skipped]), S::Passed);
        assert_eq!(aggregate(&[S::Skipped, S::Running]), S::Running);
        assert_eq!(aggregate(&[S::Skipped, S::Failed]), S::Failed);
        assert_eq!(aggregate(&[S::Skipped, S::Cancelled]), S::Cancelled);
    }

    /// SPEC-273 NOI-006: the reason names what the job said it missed, bounded and on one line.
    #[test]
    fn an_incomplete_job_s_reason_is_its_last_mark() {
        let output = "::yulius-incomplete old\nscanning\n::yulius-incomplete 1 file(s) not \
                      scanned (a.rs): HTTP 503\r\nbye\n";
        assert_eq!(
            incomplete_reason(output),
            "incomplete: 1 file(s) not scanned (a.rs): HTTP 503; infrastructure, not a finding: \
             run it again"
        );
        assert_eq!(
            incomplete_reason("nothing said\n::yulius-incomplete  \n"),
            "incomplete: the job could not check everything; infrastructure, not a finding: run \
             it again"
        );
        let long = format!("{INCOMPLETE_MARK}{}\u{1b}[31m", "x".repeat(500));
        let reason = incomplete_reason(&long);
        assert!(!reason.contains('\u{1b}'));
        assert_eq!(reason.matches('x').count(), 300, "{reason}");
    }

    /// SPEC-162: a copied verdict never turns a skipped job into a passed one.
    #[test]
    fn a_copied_verdict_keeps_skipped_jobs_skipped() {
        use RunState as S;
        assert_eq!(copied_job_verdict(Some(S::Skipped)), (S::Skipped, 78));
        assert_eq!(copied_job_verdict(Some(S::Passed)), (S::Passed, 0));
        assert_eq!(copied_job_verdict(None), (S::Passed, 0));
    }

    #[test]
    fn refs_round_trip() {
        for reference in [
            CiRef::PullRequest(12),
            CiRef::Branch("dev".into()),
            CiRef::Train(vec![12, 15, 3]),
            CiRef::Task(7),
            CiRef::Candidate("0123456789abcdef0123456789abcdef01234567".into()),
        ] {
            assert_eq!(CiRef::parse(&reference.key()), Some(reference.clone()));
        }
        assert_eq!(CiRef::parse("pr/0"), None);
        assert_eq!(CiRef::parse("branch/"), None);
        assert_eq!(CiRef::parse("tag/x"), None);
        for bad in [
            "train/",
            "train/1+",
            "train/0",
            "train/1+x",
            "task/",
            "task/0",
            "task/x",
            "candidate/",
            "candidate/xyz",
            "candidate/../dev",
        ] {
            assert_eq!(CiRef::parse(bad), None, "{bad}");
        }
        assert_eq!(CiRef::PullRequest(7).label(), "PR #7");
        assert_eq!(CiRef::Train(vec![7, 9]).key(), "train/7+9");
        assert_eq!(CiRef::Train(vec![7, 9]).label(), "Merge train #7 + #9");
        assert!(CiRef::Train(vec![7, 9]).covers(9) && !CiRef::Train(vec![7]).covers(9));
        assert!(CiRef::PullRequest(9).covers(9) && !CiRef::Branch("dev".into()).covers(9));
        // A playground task's run (SPEC-163) is no pull request's.
        assert_eq!(CiRef::Task(7).key(), "task/7");
        assert_eq!(CiRef::Task(7).label(), "Task #7");
        assert!(!CiRef::Task(9).covers(9));
        // A merged revision's full gate before staging (SPEC-575 MFS-003).
        let candidate = CiRef::Candidate("0123456789abcdef".into());
        assert_eq!(candidate.label(), "Candidate 012345678");
        assert!(!candidate.covers(9));
        assert_eq!(after_merge_skip(&plain("e2e", true), &candidate), None);
        for state in [
            RunState::Queued,
            RunState::Running,
            RunState::Passed,
            RunState::Failed,
            RunState::Error,
            RunState::Cancelled,
            RunState::Skipped,
        ] {
            assert_eq!(RunState::parse(state.as_str()), Some(state));
        }
    }

    /// SPEC-575 MFS-003 (review round 1 of slice 2): a candidate run is never superseded (it
    /// has no target of its own) and still counts as testing its commit, so the branch reaching
    /// that commit queues no second full run of it.
    #[test]
    fn a_candidate_run_is_kept_and_counts_as_testing_its_commit() {
        let dev = CiRef::Branch("dev".into());
        let candidate = CiRef::Candidate("a1".into());
        let both = plan(
            &[target(dev.clone(), "a1")],
            &[run("c1", candidate.clone(), "a1", RunState::Running)],
        );
        assert!(both.queue.is_empty(), "{both:?}");
        assert!(both.supersede.is_empty(), "{both:?}");
        let moved = plan(
            &[target(dev, "b2")],
            &[run("c1", candidate, "a1", RunState::Queued)],
        );
        assert!(
            moved.supersede.is_empty(),
            "the branch moving on keeps it: {moved:?}"
        );
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
        // SPEC-162 SKP-003: the status says how many jobs were skipped, never "all passed".
        let skipped = commit_status(
            RunState::Passed,
            RunSummary {
                jobs: 3,
                passed: 2,
                skipped: 1,
                duration_ms: Some(192_000),
                ..RunSummary::default()
            },
        );
        assert_eq!(skipped.state, CommitState::Success);
        assert_eq!(
            skipped.description,
            "Passed: 3 jobs, 1 skipped in 3 min 12 s"
        );
        let failed = commit_status(
            RunState::Failed,
            RunSummary {
                jobs: 2,
                passed: 1,
                skipped: 0,
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
        let running = commit_status(
            RunState::Running,
            RunSummary {
                jobs: 3,
                passed: 1,
                skipped: 1,
                ..RunSummary::default()
            },
        );
        assert_eq!(
            running.description,
            "Running on Yulius: 1 of 3 jobs passed, 1 skipped"
        );
        // SPEC-273 (review round 1): a failed scan shows while the tests still run.
        let running = commit_status(
            RunState::Running,
            RunSummary {
                jobs: 3,
                failed_job: Some(("security", Some(1))),
                ..RunSummary::default()
            },
        );
        assert_eq!(running.state, CommitState::Pending);
        assert_eq!(
            running.description,
            "Running on Yulius: 0 of 3 jobs passed, security failed"
        );
        // SPEC-059 CI-301: a run waiting for disk stays pending and says why.
        let waiting = commit_status(
            RunState::Queued,
            RunSummary {
                jobs: 2,
                reason: Some("waiting for disk space"),
                ..RunSummary::default()
            },
        );
        assert_eq!(waiting.state, CommitState::Pending);
        assert_eq!(
            waiting.description,
            "Queued on Yulius (2 jobs): waiting for disk space"
        );
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
                tip_on_pull: true,
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
                    tip_on_pull: true,
                },
                BranchPull {
                    number: 2,
                    open: false,
                    merged: false,
                    tip_on_pull: true,
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
        // Commits pushed after the pull request closed are not on it: unmerged work.
        let pushed_after = RemoteBranchFacts {
            pulls: Some(vec![BranchPull {
                number: 3,
                open: false,
                merged: false,
                tip_on_pull: false,
            }]),
            ..base.clone()
        };
        assert_eq!(classify_remote_branch(&pushed_after, now), None);
        // A merge-commit merge: in dev and its pull request merged.
        let landed = RemoteBranchFacts {
            merged: true,
            pulls: Some(vec![BranchPull {
                number: 6,
                open: false,
                merged: true,
                tip_on_pull: true,
            }]),
            ..base.clone()
        };
        assert_eq!(
            classify_remote_branch(&landed, now),
            Some(StaleReason::PullMerged(6))
        );
        let pushed_long_ago = RemoteBranchFacts {
            last_commit_ms: now - 20 * DAY_MS,
            ..pushed_after
        };
        assert_eq!(
            classify_remote_branch(&pushed_long_ago, now),
            Some(StaleReason::Idle(20))
        );
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
            ended: false,
        };
        let ended = WorktreeOwner::Task {
            number: 5,
            archived: false,
            merged: false,
            live: false,
            ended: true,
        };
        assert_eq!(
            classify_worktree(ended, false),
            Some(StaleReason::TaskEnded(5))
        );
        assert!(StaleReason::TaskEnded(5).safe());
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
