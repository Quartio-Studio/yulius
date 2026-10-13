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
//! - a pull request needs a valid task spec (DSC-002); a merge needs the domain gates (spec,
//!   open steps, checks per profile) and the head that was checked, which GitHub verifies
//!   again (`expected_head`);
//! - reads degrade: GitHub or Git failures become `notes` instead of failing the page.
//!
//! Environment command trust contract (DLV-003, SPEC-002 §8): an environment `command` is
//! shell text configured by a repository administrator in the sandbox profile (never read from
//! the repository). Each run needs an explicit confirmation, is audited with actor,
//! environment and command, and runs as `/bin/sh -c <command>` in the sandbox's primary clone
//! checked out at the head of the stage's branch, with a minimal environment (`PATH`,
//! `HOME` = sandbox home, `LANG`), the output bounded to the last
//! [`ENVIRONMENT_OUTPUT_BYTES`], and a [`ENVIRONMENT_TIMEOUT`] after which the process group is
//! killed and reaped ([`runner`]). It runs with the server's privileges (no containment), in a
//! directory whose `.git` and `HOME` the sandbox's agents can write: the checkout ignores
//! global/system Git config and disables hooks and fsmonitor, but repository-local config (for
//! example filter drivers) and tools reading `HOME` remain agent-influenced. Commands are
//! visible to everyone with read access, so they must not contain secrets.

pub mod runner;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;
use yulius_contracts::api::git::{
    BlockedAction, ChecksAccess, CommitView, Delivery, EnvironmentRun, GitOverview, OpenPr,
    PromoteResult, PullView, RunEnvironment, TaskBranch, TaskChanges, TaskCommit,
};
use yulius_contracts::api::sandboxes::{
    Environment as EnvironmentView, Profile, Sandbox, UpdateSandbox,
};
use yulius_domain::delivery::{
    self as dlv, ChecksState, DeliveryAction, DeliveryError, DeliveryFacts, DeliveryProfile,
    DeliveryStage, Environment, EnvironmentEvidence, Initiator, MergeMethod, NextAction, PrState,
    Preset, PromoteMethod, Stage, Verify,
};
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::spec_format::{self, MAX_CONFIG_BYTES, MAX_PATH_BYTES, SpecReport};
use yulius_domain::steps::StepStatus;
use yulius_domain::task::PullSignal;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, StepRecord, TaskRecord, UserRecord};
use crate::ports::github::{self as gh, GhPull, NewPull};
use crate::ports::store::TaskFieldUpdate;
use runner::{CommandEnd, CommandSpec, OutputRing, run_bounded};

/// Wall-clock limit of one environment command.
pub const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Limit of the checkout before an environment command.
pub const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(120);
/// Output kept per environment run (the tail).
pub const ENVIRONMENT_OUTPUT_BYTES: usize = 1 << 20;
/// Largest spec file read.
pub const MAX_SPEC_BYTES: u64 = 256 * 1024;
/// Most files examined when discovering an unregistered spec.
pub const MAX_SPEC_CANDIDATES: usize = 500;
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

// ---------------------------------------------------------------- profiles

fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

/// Converts and validates an API profile. Unknown names are rejected, blank commands mean "no
/// command", a missing `require_checks` takes the preset's default, and the result is
/// relabeled (`custom` unless every setting equals the named preset's defaults, DLV-005).
///
/// # Errors
/// `Invalid` with the first problem found.
pub fn profile_from_contract(profile: &Profile) -> AppResult<DeliveryProfile> {
    let preset = Preset::parse(&profile.preset)
        .ok_or_else(|| invalid("preset must be standard, product, studio or custom"))?;
    let merge = MergeMethod::parse(&profile.merge)
        .ok_or_else(|| invalid("merge must be squash, merge or rebase"))?;
    let promote = PromoteMethod::parse(&profile.promote)
        .ok_or_else(|| invalid("promote must be merge_commit or fast_forward"))?;
    let mut environments = Vec::with_capacity(profile.environments.len());
    for env in &profile.environments {
        let after = Stage::parse(&env.after).ok_or_else(|| {
            invalid(format!(
                "environment '{}': after must be integration or production",
                env.name
            ))
        })?;
        let verify = Verify::parse(&env.verify).ok_or_else(|| {
            invalid(format!(
                "environment '{}': verify must be manual or command",
                env.name
            ))
        })?;
        environments.push(Environment {
            name: env.name.trim().to_owned(),
            after,
            command: env
                .command
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .map(str::to_owned),
            verify,
        });
    }
    // DLV-010: the manual `dev` checkpoint of the former standard preset is gone.
    if preset == Preset::Standard
        && matches!(environments.as_slice(), [only] if DeliveryProfile::is_legacy_standard_checkpoint(only))
    {
        environments.clear();
    }
    let parsed = DeliveryProfile {
        preset,
        integration_branch: profile.integration_branch.trim().to_owned(),
        production_branch: profile.production_branch.trim().to_owned(),
        merge,
        promote,
        auto_promote: profile.auto_promote,
        require_checks: profile
            .require_checks
            .unwrap_or_else(|| DeliveryProfile::for_preset(preset).require_checks),
        environments,
    };
    parsed
        .validate()
        .map_err(|err| AppError::Invalid(err.to_string()))?;
    Ok(parsed.relabeled())
}

/// The API form of a profile.
#[must_use]
pub fn profile_to_contract(profile: &DeliveryProfile) -> Profile {
    Profile {
        preset: profile.preset.as_str().to_owned(),
        integration_branch: profile.integration_branch.clone(),
        production_branch: profile.production_branch.clone(),
        merge: profile.merge.as_str().to_owned(),
        promote: profile.promote.as_str().to_owned(),
        auto_promote: profile.auto_promote,
        require_checks: Some(profile.require_checks),
        environments: profile
            .environments
            .iter()
            .map(|env| EnvironmentView {
                name: env.name.clone(),
                after: env.after.as_str().to_owned(),
                command: env.command.clone(),
                verify: env.verify.as_str().to_owned(),
            })
            .collect(),
        gate: None,
    }
}

/// The CI gate stored on the sandbox's profile (DLV-006); `None` when absent or invalid.
#[must_use]
pub fn profile_gate(sandbox: &SandboxRecord) -> Option<String> {
    serde_json::from_str::<Profile>(&sandbox.profile_json)
        .ok()
        .and_then(|profile| profile.gate)
        .and_then(|gate| yulius_domain::ci::validate_gate(&gate).ok())
}

/// The sandbox's stored profile; an unreadable or invalid one falls back to `standard`.
#[must_use]
pub fn delivery_profile(sandbox: &SandboxRecord) -> DeliveryProfile {
    serde_json::from_str::<Profile>(&sandbox.profile_json)
        .map_err(|err| AppError::Invalid(err.to_string()))
        .and_then(|profile| profile_from_contract(&profile))
        .unwrap_or_else(|_| DeliveryProfile::standard())
}

/// The three named presets (DLV-002..004) in API form.
#[must_use]
pub fn preset_profiles() -> Vec<Profile> {
    [Preset::Standard, Preset::Product, Preset::Studio]
        .into_iter()
        .map(|preset| profile_to_contract(&DeliveryProfile::for_preset(preset)))
        .collect()
}

fn profile_summary(profile: &DeliveryProfile) -> String {
    let environments: Vec<String> = profile
        .environments
        .iter()
        .map(|env| {
            format!(
                "{} (after {}, verify {}, command: {})",
                env.name,
                env.after.as_str(),
                env.verify.as_str(),
                env.command.as_deref().unwrap_or("none")
            )
        })
        .collect();
    format!(
        "preset {}; {} -> {}; merge {}; promote {}; auto_promote {}; require_checks {}; environments: [{}]",
        profile.preset.as_str(),
        profile.integration_branch,
        profile.production_branch,
        profile.merge.as_str(),
        profile.promote.as_str(),
        profile.auto_promote,
        profile.require_checks,
        environments.join("; ")
    )
}

// ---------------------------------------------------------------- small helpers

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

fn domain_checks(state: gh::ChecksState) -> ChecksState {
    match state {
        gh::ChecksState::None => ChecksState::None,
        gh::ChecksState::Pending => ChecksState::Pending,
        gh::ChecksState::Success => ChecksState::Success,
        gh::ChecksState::Failure => ChecksState::Failure,
        gh::ChecksState::Unknown => ChecksState::Unknown,
    }
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

fn pull_view(pull: &GhPull, checks: &str) -> PullView {
    PullView {
        number: pull.number,
        url: pull.html_url.clone(),
        title: pull.title.clone(),
        state: pr_wire_state(pull).to_owned(),
        base: pull.base.clone(),
        head: pull.head.clone(),
        checks: checks.to_owned(),
    }
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
        checks: ChecksState::None,
        pr_state: PrState::Merged,
        head_current: true,
        production_diverged,
    }
}

fn is_hex_sha(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The `type(scope)!` part of a conventional commit subject or title (`docs: …`,
/// `fix(ui)!: …`), when it has one.
fn conventional_prefix(subject: &str) -> Option<&str> {
    let (head, rest) = subject.trim().split_once(": ")?;
    if rest.trim().is_empty() {
        return None;
    }
    let bare = head.strip_suffix('!').unwrap_or(head);
    let kind = match bare.split_once('(') {
        Some((kind, scope)) if scope.len() > 1 && scope.ends_with(')') => kind,
        Some(_) => return None,
        None => bare,
    };
    (!kind.is_empty() && kind.len() <= 10 && kind.bytes().all(|b| b.is_ascii_lowercase()))
        .then_some(head)
}

/// The default title of a task's pull request: the task's title, prefixed — when it is not a
/// conventional title itself — with the type and scope of the branch's oldest conventional
/// commit, so repositories that require conventional titles accept it ("Prepare this
/// repository for Yulius" → "docs: prepare this repository for Yulius", F 2026-10-11).
/// `subjects` are the branch's commit subjects, newest first.
fn default_pr_title(task_title: &str, subjects: &[String]) -> String {
    let title = task_title.trim();
    if conventional_prefix(title).is_some() {
        return title.to_owned();
    }
    let Some(prefix) = subjects.iter().rev().find_map(|s| conventional_prefix(s)) else {
        return title.to_owned();
    };
    // "Prepare …" reads "prepare …" after the prefix; an acronym ("API …") keeps its case.
    let mut chars = title.chars();
    let first = chars.next();
    let second_lower = chars.clone().next().is_some_and(char::is_lowercase);
    let rest = chars.as_str();
    match first {
        Some(c) if c.is_uppercase() && second_lower => {
            format!("{prefix}: {}{rest}", c.to_lowercase())
        }
        Some(c) => format!("{prefix}: {c}{rest}"),
        None => title.to_owned(),
    }
}

fn clip_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn clip_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    &text[..cut]
}

fn tail_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// A safe repository-relative path: non-empty, bounded, only normal components.
fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && !path.contains('\0')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Reads a repository file below `root`, refusing paths that escape it (also through
/// symlinks) and files larger than `max` bytes. Errors are user-facing and never contain
/// server paths.
async fn read_repo_file(root: &Path, relative: &str, max: u64) -> Result<String, String> {
    if !safe_relative(relative) {
        return Err(format!(
            "`{}` is not a safe repository-relative path",
            clip_chars(relative, 100)
        ));
    }
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| "the task worktree is missing".to_owned())?;
    let path = tokio::fs::canonicalize(root.join(relative))
        .await
        .map_err(|_| format!("`{relative}` does not exist in the task worktree"))?;
    if !path.starts_with(&root) {
        return Err(format!("`{relative}` points outside the task worktree"));
    }
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|_| format!("`{relative}` cannot be read"))?;
    if !meta.is_file() {
        return Err(format!("`{relative}` is not a file"));
    }
    if meta.len() > max {
        return Err(format!("`{relative}` is larger than {max} bytes"));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| format!("`{relative}` cannot be read"))?;
    String::from_utf8(bytes).map_err(|_| format!("`{relative}` is not UTF-8 text"))
}

/// Text of a level-2 section (outside code fences), trimmed.
fn section_text(markdown: &str, name: &str) -> Option<String> {
    let mut inside = false;
    let mut fence = false;
    let mut out = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
        }
        if !fence && trimmed.starts_with('#') {
            let level = trimmed.bytes().take_while(|b| *b == b'#').count();
            if level <= 2 {
                if inside {
                    break;
                }
                inside = level == 2 && trimmed[level..].trim().eq_ignore_ascii_case(name);
                continue;
            }
        }
        if inside {
            out.push(line);
        }
    }
    let text = out.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// Pull request body: the caller's text, or the spec's Goal, path and Plan checklist and the
/// task steps; always ending with `link` back to the task. Bounded to [`MAX_PR_BODY_BYTES`].
fn pull_request_body(
    link: &str,
    spec: &SpecCheck,
    steps: &[StepRecord],
    custom: Option<&str>,
) -> String {
    let mut body = String::new();
    match custom.map(str::trim).filter(|b| !b.is_empty()) {
        Some(custom) => body.push_str(custom),
        None => {
            if let Some(goal) = &spec.goal {
                body.push_str("## Goal\n\n");
                body.push_str(&clip_chars(goal, GOAL_EXCERPT_CHARS));
                body.push_str("\n\n");
            }
            if let Some(path) = &spec.path {
                body.push_str(&format!("**Spec:** `{path}`\n\n"));
            }
            if let Some(report) = &spec.report
                && !report.plan.is_empty()
            {
                body.push_str("## Plan\n\n");
                for item in &report.plan {
                    body.push_str(&format!(
                        "{}- [{}] {}\n",
                        "  ".repeat(item.depth.min(4)),
                        if item.checked { "x" } else { " " },
                        item.text
                    ));
                }
                body.push('\n');
            }
            if !steps.is_empty() {
                body.push_str("## Steps\n\n");
                for step in steps {
                    body.push_str(&format!("- {} ({})\n", step.title, step.status));
                }
                body.push('\n');
            }
        }
    }
    let room = MAX_PR_BODY_BYTES.saturating_sub(link.len() + 8);
    let mut out = clip_bytes(&body, room).trim_end().to_owned();
    out.push_str("\n\n---\n");
    out.push_str(link);
    out
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

/// Everything observed about one task before a delivery decision.
#[derive(Debug, Clone)]
struct Observed {
    record: DeliveryRecord,
    spec: SpecCheck,
    steps: Vec<StepRecord>,
    steps_open: usize,
    pr: Option<GhPull>,
    checks: ChecksState,
    /// Check sources the App may not read for the pull request head.
    checks_unreadable: Vec<gh::ChecksSource>,
    worktree_head: Option<String>,
    dirty: bool,
    notes: Vec<String>,
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
            checks: self.checks,
            pr_state: self.pr_state(),
            head_current: matches!(
                (&self.pr, &self.worktree_head),
                (Some(pull), Some(head)) if pull.head_sha == *head
            ),
            production_diverged: false,
        }
    }
}

impl App {
    async fn dlv_sandbox_of(&self, task: &TaskRecord) -> AppResult<SandboxRecord> {
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
        Ok(self.require(&ctx.user, &sid, action).await?.0)
    }

    /// Short-lived installation token for Git transport (push, fetch).
    pub(crate) async fn dlv_installation_token(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Secret> {
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
    async fn dlv_author_token(
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
    async fn dlv_read_token(
        &self,
        sandbox: &SandboxRecord,
        user: Option<&UserRecord>,
    ) -> Result<Secret, String> {
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
        let token = self.dlv_installation_token(sandbox).await?;
        let url = format!(
            "https://github.com/{}/{}.git",
            sandbox.repo_owner, sandbox.repo_name
        );
        self.deps.git.sync_clone(&url, &repo, &token).await
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
        user.github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into())
    }

    /// Reads and validates the task spec; a task that needs none (DSC-013: research, a
    /// question, docs or a chore by its workflow, or a docs-only change without one) counts as
    /// valid without it.
    async fn dlv_check_spec(&self, task: &TaskRecord) -> SpecCheck {
        let mut check = self.dlv_read_spec(task).await;
        // A valid spec settles it; only a missing or invalid one asks whether it is needed
        // (which may read the branch's changed paths).
        if !check.valid && !self.spec_required(task).await.unwrap_or(true) {
            check.required = false;
            check.valid = true;
            check.problems.clear();
        }
        check
    }

    /// Reads and validates the task spec; registers a discovered spec path.
    async fn dlv_read_spec(&self, task: &TaskRecord) -> SpecCheck {
        let Some(worktree) = task.worktree_path.as_deref().map(PathBuf::from) else {
            return SpecCheck::problem(
                task.spec_path.clone(),
                "The task has no worktree yet, so its spec cannot be read.".into(),
            );
        };
        let path = match self.task_spec_path(task, &worktree).await {
            Ok(path) => path,
            Err(dir) => {
                return SpecCheck::problem(
                    None,
                    format!(
                        "No spec is registered for this task and no Markdown file under `{dir}` has `yulius-task: {}` frontmatter; write the task spec there.",
                        task.id
                    ),
                );
            }
        };
        match read_repo_file(&worktree, &path, MAX_SPEC_BYTES).await {
            Ok(text) => {
                let report = spec_format::validate_spec_for(&text, &task.id);
                SpecCheck {
                    path: Some(path),
                    valid: report.is_valid(),
                    problems: report.problems.iter().map(ToString::to_string).collect(),
                    goal: section_text(&text, "Goal"),
                    report: Some(report),
                    required: true,
                }
            }
            Err(message) => SpecCheck::problem(Some(path.clone()), format!("Spec {message}.")),
        }
    }

    /// Gives a task that has no spec path yet the next free `SPEC-NNN-<slug>.md` of its specs
    /// directory, when the repository numbers its specs that way: parallel tasks each took the
    /// same number (nine `SPEC-010`s on 2026-10-12). The number is free in the worktree's specs
    /// and among the paths given to the sandbox's other tasks; allocation is serialized.
    pub(crate) async fn allocate_spec_path(&self, task: &TaskRecord, worktree: &Path) {
        static ALLOCATING: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        if task.spec_path.is_some() {
            return;
        }
        let _guard = ALLOCATING
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (dir, found) = self.dlv_discover_spec(task, worktree).await;
        if found.is_some() || !safe_relative(&dir) {
            return;
        }
        let number_of = |name: &str| -> Option<u32> {
            let rest = name.rsplit('/').next()?.strip_prefix("SPEC-")?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty() && rest[digits.len()..].starts_with('-'))
                .then(|| digits.parse().ok())
                .flatten()
        };
        let mut numbers: Vec<u32> = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(worktree.join(&dir)).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Some(n) = number_of(&entry.file_name().to_string_lossy()) {
                    numbers.push(n);
                }
            }
        }
        // Only a repository that numbers its specs this way gets a number.
        if numbers.is_empty() {
            return;
        }
        if let Ok(tasks) = self.deps.store.tasks(&task.sandbox_id, false).await {
            numbers.extend(
                tasks
                    .iter()
                    .filter_map(|t| t.spec_path.as_deref().and_then(number_of)),
            );
        }
        let next = numbers.iter().max().copied().unwrap_or(0) + 1;
        let slug = task
            .branch
            .as_deref()
            .and_then(|b| b.split_once('/').map(|(_, rest)| rest))
            .and_then(|rest| rest.split_once('-').map(|(_, slug)| slug))
            .filter(|slug| !slug.is_empty())
            .unwrap_or("task");
        let path = format!("{dir}/SPEC-{next:03}-{slug}.md");
        let update = TaskFieldUpdate {
            spec_path: Some(path),
            ..TaskFieldUpdate::default()
        };
        if let Err(err) = self
            .deps
            .store
            .update_task(&task.id, &update, self.now())
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "allocating the spec path failed");
        }
    }

    /// The repository path of the task's spec: the registered one while that file exists, else
    /// the one whose frontmatter names the task — registered on the way, so a renamed spec is
    /// followed instead of blocking the agent on a path it cannot change (F 2026-10-11).
    /// `Err(dir)` when there is none; `dir` is the specs directory searched.
    pub(crate) async fn task_spec_path(
        &self,
        task: &TaskRecord,
        worktree: &Path,
    ) -> Result<String, String> {
        if let Some(path) = &task.spec_path
            && safe_relative(path)
            && worktree.join(path).is_file()
        {
            return Ok(path.clone());
        }
        match self.dlv_discover_spec(task, worktree).await {
            (_, Some(found)) => {
                if task.spec_path.as_deref() != Some(found.as_str()) {
                    let update = TaskFieldUpdate {
                        spec_path: Some(found.clone()),
                        ..TaskFieldUpdate::default()
                    };
                    if let Err(err) = self
                        .deps
                        .store
                        .update_task(&task.id, &update, self.now())
                        .await
                    {
                        tracing::warn!(error = %err, "registering the discovered spec failed");
                    }
                }
                Ok(found)
            }
            (dir, None) => Err(dir),
        }
    }

    /// Looks for a spec naming this task under the project's specs directory. Returns the
    /// directory searched and the repository-relative path found.
    async fn dlv_discover_spec(
        &self,
        task: &TaskRecord,
        worktree: &Path,
    ) -> (String, Option<String>) {
        let config = read_repo_file(
            worktree,
            "yulius.toml",
            u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX),
        )
        .await
        .ok();
        let layout = spec_format::project_layout(config.as_deref());
        let dir = layout.specs_dir.trim_end_matches('/').to_owned();
        if !safe_relative(&dir) {
            return (dir, None);
        }
        let Ok(mut entries) = tokio::fs::read_dir(worktree.join(&dir)).await else {
            return (dir, None);
        };
        let mut names = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if names.len() >= MAX_SPEC_CANDIDATES {
                break;
            }
            let is_file = entry.file_type().await.is_ok_and(|t| t.is_file());
            if let Some(name) = entry.file_name().to_str()
                && is_file
                && name.ends_with(".md")
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        for name in names {
            let relative = format!("{dir}/{name}");
            let Ok(text) = read_repo_file(worktree, &relative, MAX_SPEC_BYTES).await else {
                continue;
            };
            if text.contains(task.id.as_str())
                && spec_format::validate_spec(&text).task_id.as_deref() == Some(task.id.as_str())
            {
                return (dir, Some(relative));
            }
        }
        (dir, None)
    }

    /// Gathers the facts of one task and reconciles its stage with GitHub (a pull request
    /// opened, merged or closed outside Yulius).
    async fn dlv_observe(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        profile: &DeliveryProfile,
        token: Option<&Secret>,
    ) -> AppResult<Observed> {
        let store = &self.deps.store;
        let record = store
            .delivery(&task.id)
            .await?
            .unwrap_or_else(|| DeliveryRecord::initial(&task.id));
        let spec = self.dlv_check_spec(task).await;
        let steps = store.steps(&task.id).await?;
        let steps_open = steps
            .iter()
            .filter(|s| !StepStatus::parse(&s.status).is_some_and(StepStatus::is_resolved))
            .count();
        let mut notes = Vec::new();
        let pre_merge = matches!(record.stage, DeliveryStage::None | DeliveryStage::PrOpen);
        let mut worktree_head = None;
        let mut dirty = false;
        if let Some(worktree) = task.worktree_path.as_deref().map(Path::new)
            && pre_merge
        {
            match self.deps.git.log(worktree, "HEAD", 1).await {
                Ok(commits) => worktree_head = commits.into_iter().next().map(|c| c.sha),
                Err(err) => notes.push(format!(
                    "Git: the task branch head cannot be read: {}",
                    err.public_message()
                )),
            }
            if record.stage == DeliveryStage::None {
                dirty = self.deps.git.has_changes(worktree).await.unwrap_or(false);
            }
        }
        let mut pr = None;
        // Only an answer from GitHub may change the recorded stage; failures never do.
        let mut pr_known = false;
        let mut checks = ChecksState::None;
        let mut checks_unreadable = Vec::new();
        if let Some(token) = token {
            let github = &self.deps.github;
            let (owner, repo) = (&sandbox.repo_owner, &sandbox.repo_name);
            if let Some(number) = record.pr_number {
                match github.get_pull(token, owner, repo, number).await {
                    Ok(pull) => {
                        pr = Some(pull);
                        pr_known = true;
                    }
                    Err(err) => notes.push(format!(
                        "Pull request #{number} can't be read: {}",
                        github_reason(&err)
                    )),
                }
            } else if let (Some(branch), DeliveryStage::None) = (&task.branch, &record.stage) {
                // Filtered here rather than through the adapter's `head` parameter, whose
                // GitHub form is `owner:branch`.
                match github.list_pulls(token, owner, repo, None).await {
                    Ok(pulls) => {
                        pr_known = true;
                        pr = pulls.into_iter().find(|p| {
                            p.head == *branch && p.base == profile.integration_branch && is_open(p)
                        });
                    }
                    Err(err) => notes.push(format!(
                        "Pull requests can't be listed: {}",
                        github_reason(&err)
                    )),
                }
            }
            if let Some(pull) = pr.as_ref().filter(|p| is_open(p)) {
                let gate = self.ci_gate_result(sandbox, &pull.head_sha).await;
                if gate == yulius_domain::ci::GateResult::NotConfigured {
                    match github.checks(token, owner, repo, &pull.head_sha).await {
                        Ok(report) => {
                            checks = domain_checks(report.state);
                            checks_unreadable = report.unreadable;
                        }
                        Err(err) => {
                            // Unknown checks never allow a merge that requires them.
                            checks = ChecksState::Unknown;
                            notes.push(format!("Checks can't be read: {}", github_reason(&err)));
                        }
                    }
                } else {
                    // Yulius CI is the project's only CI: its run at this head alone decides
                    // (DLV-006); checks GitHub reports from elsewhere do not count.
                    checks = yulius_domain::ci::gate_checks(ChecksState::None, gate);
                }
            }
        }
        let mut observed = Observed {
            record,
            spec,
            steps,
            steps_open,
            pr,
            checks,
            checks_unreadable,
            worktree_head,
            dirty,
            notes,
        };
        if !pr_known {
            return Ok(observed);
        }
        let next = dlv::reconcile(&observed.record.stage, &observed.facts());
        let found_number = observed.pr.as_ref().map(|p| p.number);
        if next != observed.record.stage
            || (found_number.is_some() && observed.record.pr_number.is_none())
        {
            let reconciled = DeliveryRecord {
                task_id: task.id.clone(),
                pr_number: if next == DeliveryStage::None {
                    None
                } else {
                    found_number.or(observed.record.pr_number)
                },
                stage: next,
                updated_at: self.now(),
            };
            if store
                .transition_delivery(&observed.record.stage, &reconciled)
                .await?
            {
                if reconciled.stage != observed.record.stage {
                    self.dlv_audit(
                        "yulius",
                        "delivery.reconcile",
                        &format!("{}#{}", sandbox.slug, task.number),
                        &format!(
                            "{} -> {} (GitHub state)",
                            observed.record.stage, reconciled.stage
                        ),
                    )
                    .await;
                }
                if reconciled.stage == DeliveryStage::None {
                    observed.pr = None;
                }
                observed.record = reconciled;
            }
        }
        Ok(observed)
    }

    fn dlv_delivery_of(
        profile: &DeliveryProfile,
        observed: &Observed,
        mut notes: Vec<String>,
        checks_access: Option<ChecksAccess>,
    ) -> Delivery {
        let facts = observed.facts();
        let stage = &observed.record.stage;
        let next_actions = dlv::next_actions(stage, profile, &facts)
            .iter()
            .map(NextAction::wire)
            .collect();
        let blocked = dlv::blocked_actions(stage, profile, &facts)
            .into_iter()
            .map(|(action, err)| BlockedAction {
                action: action.wire(),
                reason: err.to_string(),
            })
            .collect();
        if observed.dirty && *stage == DeliveryStage::None {
            notes.push(format!("Git: {DIRTY_WORKTREE}."));
        }
        notes.extend(observed.notes.iter().cloned());
        let checks = if observed.pr.as_ref().is_some_and(is_open) {
            observed.checks.as_str()
        } else {
            "none"
        };
        Delivery {
            stage: stage.wire_name().to_owned(),
            environment: match stage {
                DeliveryStage::EnvDone(name) => Some(name.clone()),
                _ => None,
            },
            pr: observed.pr.as_ref().map(|p| pull_view(p, checks)),
            spec_path: observed.spec.path.clone(),
            spec_valid: observed.spec.valid,
            spec_problems: observed.spec.problems.clone(),
            steps_open: u32::try_from(observed.steps_open).unwrap_or(u32::MAX),
            next_actions,
            blocked,
            notes,
            checks_access,
        }
    }

    /// The GitHub App's permission settings page, for instance administrators (#111): where a
    /// missing check permission is added. `None` for everyone else and when the App or its
    /// owner is not known.
    async fn dlv_app_settings_url(&self, user: Option<&UserRecord>) -> Option<String> {
        if !user.is_some_and(|u| u.is_superadmin || u.is_instance_admin) {
            return None;
        }
        let app = self.github_app().await.ok()?;
        let instance = self.deps.store.instance().await.ok()??;
        let owner = instance.owner_login?;
        let slug = &app.slug;
        let ok = |text: &str| {
            !text.is_empty()
                && text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        if !ok(&owner) || !ok(slug) {
            return None;
        }
        match instance.owner_kind.as_deref() {
            Some("org") => Some(format!(
                "https://github.com/organizations/{owner}/settings/apps/{slug}/permissions"
            )),
            Some("user") => Some(format!(
                "https://github.com/settings/apps/{slug}/permissions"
            )),
            _ => None,
        }
    }

    /// [`checks_access`] for the sandbox's repository, with the settings link for `user`.
    async fn dlv_checks_access(
        &self,
        sandbox: &SandboxRecord,
        unreadable: &[gh::ChecksSource],
        user: Option<&UserRecord>,
    ) -> Option<ChecksAccess> {
        if unreadable.is_empty() {
            return None;
        }
        let repo = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name);
        checks_access(&repo, unreadable, self.dlv_app_settings_url(user).await)
    }

    // ---------------------------------------------------------------- reads

    /// Delivery state of a task for `user` (or for an agent with `None`, using the App's
    /// installation token for GitHub reads). Reconciles with GitHub; GitHub and Git failures
    /// become notes.
    ///
    /// # Errors
    /// Store failures and a missing sandbox.
    pub async fn delivery_view(
        &self,
        task: &TaskRecord,
        user: Option<&UserRecord>,
    ) -> AppResult<Delivery> {
        let sandbox = self.dlv_sandbox_of(task).await?;
        let profile = delivery_profile(&sandbox);
        let mut notes = Vec::new();
        let token = match self.dlv_read_token(&sandbox, user).await {
            Ok(token) => Some(token),
            Err(note) => {
                notes.push(note);
                None
            }
        };
        let observed = self
            .dlv_observe(&sandbox, task, &profile, token.as_ref())
            .await?;
        // Checks are read only for an open pull request, so this is empty otherwise.
        let access = self
            .dlv_checks_access(&sandbox, &observed.checks_unreadable, user)
            .await;
        Ok(Self::dlv_delivery_of(&profile, &observed, notes, access))
    }

    /// The `TaskDetail.delivery` value: the full view, or the stored stage with a note when
    /// even that cannot be computed.
    pub async fn task_detail_delivery(&self, task: &TaskRecord, user: &UserRecord) -> Delivery {
        match self.delivery_view(task, Some(user)).await {
            Ok(view) => view,
            Err(err) => Delivery {
                stage: "none".into(),
                notes: vec![format!(
                    "The delivery state is unavailable: {}",
                    err.public_message()
                )],
                ..Delivery::default()
            },
        }
    }

    /// `GET /api/tasks/{tid}/delivery`.
    ///
    /// # Errors
    /// `NotFound` without read access.
    pub async fn task_delivery(&self, ctx: &AuthContext, task_id: &str) -> AppResult<Delivery> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        self.delivery_view(&task, Some(&ctx.user)).await
    }

    /// `GET /api/sandboxes/{sid}/git`: branches, per-task branch facts, recent integration
    /// commits and open pull requests with checks (GIT-001). Degrades to partial data with
    /// notes when Git or GitHub fail.
    ///
    /// # Errors
    /// `NotFound` without read access; store failures.
    pub async fn git_overview(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<GitOverview> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::View)
            .await?;
        let profile = delivery_profile(&sandbox);
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        let repo = self.sandbox_repo(&sandbox.id);
        let cloned = repo.join(".git").exists();
        let integration_ref = format!("origin/{}", profile.integration_branch);
        let mut notes = Vec::new();
        let token = match self.dlv_read_token(&sandbox, Some(&ctx.user)).await {
            Ok(token) => Some(token),
            Err(note) => {
                notes.push(note);
                None
            }
        };
        let mut open_pulls = Vec::new();
        let mut checks_by_pr = Vec::new();
        let mut checks_unreadable = Vec::new();
        if let Some(token) = &token {
            match self.deps.github.list_pulls(token, owner, name, None).await {
                Ok(pulls) => {
                    open_pulls = pulls.into_iter().filter(is_open).take(MAX_PULLS).collect()
                }
                Err(err) => notes.push(format!(
                    "Pull requests can't be listed: {}",
                    github_reason(&err)
                )),
            }
            let mut checks_failed = false;
            for pull in &open_pulls {
                let state = match self
                    .deps
                    .github
                    .checks(token, owner, name, &pull.head_sha)
                    .await
                {
                    Ok(report) => {
                        // A missing permission is said once for the repository, not per PR.
                        merge_sources(&mut checks_unreadable, &report.unreadable);
                        report.state.as_str()
                    }
                    Err(err) => {
                        if !checks_failed {
                            checks_failed = true;
                            notes.push(format!("Checks can't be read: {}", github_reason(&err)));
                        }
                        "unknown"
                    }
                };
                checks_by_pr.push((pull.number, state));
            }
        }
        let checks_access = self
            .dlv_checks_access(&sandbox, &checks_unreadable, Some(&ctx.user))
            .await;
        // With required checks, a pull request whose checks have not reported yet is pending,
        // not "none": it is not tested until they pass (#68).
        let checks_of = |number: u64| {
            let state = checks_by_pr
                .iter()
                .find(|(n, _)| *n == number)
                .map_or("unknown", |(_, state)| *state);
            if profile.require_checks && state == "none" {
                "pending"
            } else {
                state
            }
        };
        let pulls: Vec<PullView> = open_pulls
            .iter()
            .map(|p| pull_view(p, checks_of(p.number)))
            .collect();

        if !cloned {
            notes.push("Git: the sandbox repository is not cloned yet.".into());
        }
        let mut tasks = Vec::new();
        let mut observed = std::collections::HashMap::new();
        let mut git_failed = false;
        for task in self.deps.store.tasks(&sandbox.id, false).await? {
            let Some(branch) = task.branch.clone() else {
                continue;
            };
            let mut view = TaskBranch {
                task_id: task.id.to_string(),
                title: task.title.clone(),
                branch: branch.clone(),
                ahead: 0,
                behind: 0,
                files_changed: 0,
                insertions: 0,
                deletions: 0,
                pr: open_pulls
                    .iter()
                    .find(|p| p.head == branch)
                    .map(|p| pull_view(p, checks_of(p.number))),
            };
            if cloned
                && self
                    .deps
                    .git
                    .branch_exists(&repo, &branch)
                    .await
                    .unwrap_or(false)
            {
                let git = &self.deps.git;
                match (
                    git.ahead_behind(&repo, &integration_ref, &branch).await,
                    git.diff_stat(&repo, &integration_ref, &branch).await,
                ) {
                    (Ok((ahead, behind)), Ok(stat)) => {
                        view.ahead = ahead;
                        view.behind = behind;
                        view.files_changed = stat.files;
                        view.insertions = stat.insertions;
                        view.deletions = stat.deletions;
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        if !git_failed {
                            git_failed = true;
                            notes.push(format!(
                                "Git: branch statistics are incomplete: {}",
                                err.public_message()
                            ));
                        }
                    }
                }
            }
            if view.pr.is_none()
                && let Some(token) = &token
                && let Ok(Some(record)) = self.deps.store.delivery(&task.id).await
                && record.stage == DeliveryStage::PrOpen
                && let Some(number) = record.pr_number
                && let Ok(pull) = self.deps.github.get_pull(token, owner, name, number).await
            {
                // Merged or closed outside Yulius: show it and reconcile the stage. An open one
                // missing from the listed page has unread checks (#68).
                view.pr = Some(pull_view(
                    &pull,
                    if is_open(&pull) { "unknown" } else { "none" },
                ));
                let observed_state = if pull.merged {
                    PrState::Merged
                } else if is_open(&pull) {
                    PrState::Open
                } else {
                    PrState::Closed
                };
                let facts = DeliveryFacts {
                    pr_state: observed_state,
                    ..merged_facts(false)
                };
                let next = dlv::reconcile(&record.stage, &facts);
                if next != record.stage {
                    let reconciled = DeliveryRecord {
                        pr_number: (next != DeliveryStage::None).then_some(number),
                        stage: next,
                        updated_at: self.now(),
                        ..record.clone()
                    };
                    let _ = self
                        .deps
                        .store
                        .transition_delivery(&record.stage, &reconciled)
                        .await;
                }
            }
            if view.pr.is_none()
                && let Ok(Some(record)) = self.deps.store.delivery(&task.id).await
                && past_merge(&record.stage)
                && let Some(number) = record.pr_number
            {
                // Merged earlier: no longer listed as open, but still this task's pull request,
                // so the task stays "Done · merged" (#68). No GitHub call.
                view.pr = Some(PullView {
                    number,
                    url: format!("https://github.com/{owner}/{name}/pull/{number}"),
                    title: task.title.clone(),
                    state: "merged".into(),
                    base: profile.integration_branch.clone(),
                    head: branch.clone(),
                    checks: "none".into(),
                });
            }
            if let Some(signal) = view
                .pr
                .as_ref()
                .and_then(|pr| PullSignal::from_wire(&pr.state, &pr.checks))
            {
                observed.insert(task.id.clone(), signal);
            }
            tasks.push(view);
        }
        // The "needs you" counts read what this overview said (sandboxes.rs, #147).
        self.observed_pulls.record(&sandbox.id, observed);

        let mut recent_commits = Vec::new();
        if cloned {
            match self
                .deps
                .git
                .log(&repo, &integration_ref, RECENT_COMMITS)
                .await
            {
                Ok(commits) => {
                    recent_commits = commits
                        .into_iter()
                        .map(|c| CommitView {
                            sha: c.sha,
                            subject: c.subject,
                            author: c.author,
                            at: rfc3339(Timestamp(c.at_ms)),
                        })
                        .collect();
                }
                Err(err) => notes.push(format!(
                    "Git: recent commits of {} cannot be read: {}",
                    profile.integration_branch,
                    err.public_message()
                )),
            }
        }
        Ok(GitOverview {
            integration_branch: profile.integration_branch.clone(),
            production_branch: profile.production_branch.clone(),
            default_branch: sandbox.default_branch.clone(),
            tasks,
            recent_commits,
            pulls,
            notes,
            checks_access,
        })
    }

    // ---------------------------------------------------------------- profile

    /// `PATCH /api/sandboxes/{sid}` (admin): validates and stores the delivery profile.
    ///
    /// # Errors
    /// `Forbidden` below admin, `Invalid` profiles.
    pub async fn update_sandbox_profile(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        req: &UpdateSandbox,
    ) -> AppResult<Sandbox> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require(&ctx.user, &sid, Action::Administer).await?;
        let Some(requested) = &req.profile else {
            return self.sandbox_view(&sandbox, level).await;
        };
        let before = delivery_profile(&sandbox);
        let profile = profile_from_contract(requested)?;
        let gate = match requested.gate.as_deref() {
            None => None,
            Some(text) if text.trim().is_empty() => None,
            Some(text) => Some(
                yulius_domain::ci::validate_gate(text)
                    .map_err(|err| invalid(format!("gate: {err}")))?,
            ),
        };
        let mut stored = profile_to_contract(&profile);
        stored.gate.clone_from(&gate);
        let json = serde_json::to_string(&stored).map_err(AppError::internal)?;
        self.deps.store.set_profile(&sid, &json).await?;
        self.ci_forget_gate(&sid);
        self.dlv_audit(
            &Self::dlv_actor_login(&ctx.user),
            "delivery.profile.update",
            &sandbox.slug,
            &format!(
                "from preset {}: {}, gate {}",
                before.preset.as_str(),
                profile_summary(&profile),
                gate.as_deref().unwrap_or("(from yulius.toml)")
            ),
        )
        .await;
        let updated = self
            .deps
            .store
            .sandbox(&sid)
            .await?
            .ok_or(AppError::NotFound)?;
        self.sandbox_view(&updated, level).await
    }

    // ---------------------------------------------------------------- pull request

    /// `POST /api/tasks/{tid}/delivery/pr`: pushes the task branch with an installation token
    /// and opens the pull request into the integration branch with the caller's token (the
    /// superadmin, who has no GitHub identity, opens it through the App, ACC-012).
    /// Refused without a valid spec (DSC-002) or with uncommitted changes.
    ///
    /// # Errors
    /// `Conflict` refusals, `Upstream` Git/GitHub failures.
    pub async fn open_task_pr(
        &self,
        ctx: &AuthContext,
        task_id: &str,
        req: &OpenPr,
    ) -> AppResult<Delivery> {
        let task = self.load_task(ctx, task_id, Action::Work).await?;
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let profile = delivery_profile(&sandbox);
        let branch = task.branch.clone().ok_or_else(|| {
            AppError::Conflict("the task has no branch yet; start it first".into())
        })?;
        if task.worktree_path.is_none() {
            return Err(AppError::Conflict(
                "the task has no worktree yet; start it first".into(),
            ));
        }
        if branch == profile.integration_branch || branch == profile.production_branch {
            return Err(AppError::Invalid(format!(
                "the task branch '{branch}' is a protected delivery branch"
            )));
        }
        let token = self.dlv_author_token(&ctx.user, &sandbox).await?;
        let observed = self
            .dlv_observe(&sandbox, &task, &profile, Some(&token))
            .await?;
        dlv::apply(
            &observed.record.stage,
            &DeliveryAction::OpenPr,
            Initiator::User,
            &profile,
            &observed.facts(),
        )
        .map_err(|err| match err {
            DeliveryError::SpecInvalid => AppError::Conflict(format!(
                "{err} Problems: {}",
                observed.spec.problems.join(" ")
            )),
            other => refusal(&other),
        })?;
        if observed.dirty {
            return Err(AppError::Conflict(DIRTY_WORKTREE.into()));
        }
        self.dlv_open_pull(
            &sandbox,
            &task,
            &profile,
            &observed,
            &token,
            req.title.as_deref(),
            req.body.as_deref(),
            &Self::dlv_actor_login(&ctx.user),
        )
        .await?;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.delivery_view(&task, Some(&ctx.user)).await
    }

    /// Pushes the task branch and opens its pull request into the integration branch, both with
    /// `token` (its account pushes and authors the pull request: the person who delivers, or for
    /// Yulius the task's owner, F 2026-10-11); records `pr_open`, audits it as `actor` and
    /// publishes the task. The caller has checked the domain rules and the worktree
    /// (`open_task_pr` for a person, `try_auto_open_pr` for Yulius).
    #[allow(clippy::too_many_arguments)]
    async fn dlv_open_pull(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        profile: &DeliveryProfile,
        observed: &Observed,
        token: &Secret,
        title: Option<&str>,
        body: Option<&str>,
        actor: &str,
    ) -> AppResult<u64> {
        let (Some(branch), Some(worktree)) = (task.branch.clone(), task.worktree_path.clone())
        else {
            return Err(AppError::Conflict(
                "the task has no branch or worktree yet; start it first".into(),
            ));
        };
        self.deps
            .git
            .push(Path::new(&worktree), &branch, &branch, token)
            .await?;
        let title = match title.map(str::trim).filter(|t| !t.is_empty()) {
            Some(title) => title.to_owned(),
            None => {
                let base = format!("origin/{}", profile.integration_branch);
                let subjects: Vec<String> = self
                    .deps
                    .git
                    .review_commits(Path::new(&worktree), &base, "HEAD", 50)
                    .await
                    .map(|commits| commits.into_iter().map(|c| c.subject).collect())
                    .unwrap_or_default();
                default_pr_title(&task.title, &subjects)
            }
        }
        .replace(['\n', '\r'], " ");
        let pull = self
            .deps
            .github
            .create_pull(
                token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                &NewPull {
                    head: branch.clone(),
                    base: profile.integration_branch.clone(),
                    title: clip_chars(&title, MAX_PR_TITLE_CHARS),
                    body: pull_request_body(
                        &format!(
                            "Delivered with Yulius task #{}: {}/s/{}/t/{}",
                            task.number, self.config.public_url, sandbox.slug, task.number
                        ),
                        &observed.spec,
                        &observed.steps,
                        body,
                    ),
                },
            )
            .await?;
        let next = DeliveryRecord {
            task_id: task.id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(pull.number),
            updated_at: self.now(),
        };
        if !self
            .deps
            .store
            .transition_delivery(&observed.record.stage, &next)
            .await?
        {
            return Err(AppError::Conflict(
                "the delivery state changed concurrently; refresh".into(),
            ));
        }
        self.dlv_audit(
            actor,
            "delivery.pr.open",
            &format!("{}#{}", sandbox.slug, task.number),
            &format!(
                "pull request #{} {} -> {} at {}",
                pull.number, branch, profile.integration_branch, pull.head_sha
            ),
        )
        .await;
        self.publish_task(&task.id).await;
        Ok(pull.number)
    }

    // ---------------------------------------------------------------- task review

    /// Commits listed on a task's review.
    const REVIEW_COMMITS: u32 = 100;

    /// `GET /api/tasks/{tid}/changes` (read): the task's changes for review (UI-031) — its
    /// worktree, committed and uncommitted work and untracked files, against the merge base with
    /// `origin/<integration>`, bounded.
    ///
    /// # Errors
    /// `NotFound` without access; `Conflict` when the task has no worktree (any more);
    /// `Upstream` Git failures.
    pub async fn task_changes(&self, ctx: &AuthContext, task_id: &str) -> AppResult<TaskChanges> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        let (worktree, base, integration) = self.review_target(&task).await?;
        let changes = self.deps.git.review_changes(&worktree, &base).await?;
        let (ahead, behind) = self
            .deps
            .git
            .ahead_behind(&worktree, &base, "HEAD")
            .await
            .unwrap_or((0, 0));
        Ok(TaskChanges {
            base: integration,
            head: task.branch.clone().unwrap_or_default(),
            base_sha: changes.base_sha,
            head_sha: changes.head_sha,
            ahead,
            behind,
            uncommitted: changes.uncommitted,
            truncated: changes.truncated,
            files: changes.files,
        })
    }

    /// `GET /api/tasks/{tid}/commits` (read): the task branch's commits not on the integration
    /// branch, newest first, each with its size (UI-031).
    ///
    /// # Errors
    /// As [`App::task_changes`].
    pub async fn task_commits(
        &self,
        ctx: &AuthContext,
        task_id: &str,
    ) -> AppResult<Vec<TaskCommit>> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        let (worktree, base, _) = self.review_target(&task).await?;
        self.deps
            .git
            .review_commits(&worktree, &base, "HEAD", Self::REVIEW_COMMITS)
            .await
    }

    /// The task's worktree, `origin/<integration>` and the integration branch name.
    async fn review_target(&self, task: &TaskRecord) -> AppResult<(PathBuf, String, String)> {
        let sandbox = self.dlv_sandbox_of(task).await?;
        let integration = delivery_profile(&sandbox).integration_branch;
        let worktree = task
            .worktree_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .ok_or_else(|| {
                AppError::Conflict(
                    "the task's worktree is gone; its branch and pull request are on GitHub".into(),
                )
            })?;
        Ok((worktree, format!("origin/{integration}"), integration))
    }

    // ---------------------------------------------------------------- automatic delivery

    /// Opens the task's pull request by itself when the task is ready for delivery (#35), so
    /// nobody has to: called when the agent ends a turn cleanly. Merging and promotion stay
    /// human (GIT-002). The outcome is said in the task's terminal; failures are logged and not
    /// retried for the same head.
    pub async fn auto_open_pr(&self, task_id: &TaskId) {
        match self.try_auto_open_pr(task_id).await {
            Ok(Some(done)) => self.runtime(task_id).hub.notice(done),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "automatic pull request failed");
                self.runtime(task_id).hub.notice(format!(
                    "Yulius could not open the pull request by itself: {err} Use Deliver… on the task."
                ));
            }
        }
    }

    /// Records why Yulius did not push or open the pull request at the end of a turn, for
    /// `yulius wait` (SPEC-050 DLV-204): an agent that expects delivery learns what blocks it
    /// instead of waiting for a pull request that never comes (#34 waited 42 minutes). Only for
    /// a branch with commits to deliver.
    async fn note_undelivered(&self, task: &TaskRecord, worktree: &str, reason: &str) {
        let path = Path::new(worktree);
        let Some(head) = self
            .deps
            .git
            .log(path, "HEAD", 1)
            .await
            .ok()
            .and_then(|log| log.into_iter().next())
            .map(|commit| commit.sha)
        else {
            return;
        };
        let Ok(sandbox) = self.dlv_sandbox_of(task).await else {
            return;
        };
        let integration = delivery_profile(&sandbox).integration_branch;
        let ahead = self
            .deps
            .git
            .ahead_behind(path, &format!("origin/{integration}"), "HEAD")
            .await
            .map(|(ahead, _)| ahead)
            .unwrap_or(0);
        if ahead == 0 {
            return;
        }
        let short = head.get(..9).unwrap_or(&head);
        let text = format!(
            "Yulius has not delivered {short} (no push, no pull request): {reason}. It delivers \
             at the end of your turn once nothing blocks it."
        );
        if let Ok(mut problem) = self.runtime(&task.id).push_problem.lock() {
            *problem = Some((head, text));
        }
    }

    /// Records why the agent's head could not be pushed, for `yulius wait` (DLV-201). A branch
    /// rewritten after it was pushed is refused as not a fast-forward; the remedy merges the
    /// pushed head back in, so nothing is force-pushed.
    pub(crate) fn set_push_problem(
        &self,
        task_id: &TaskId,
        head: &str,
        branch: &str,
        pull: Option<u64>,
        err: &AppError,
    ) {
        let reason = err.public_message();
        let target = pull.map_or_else(|| format!("`{branch}`"), |n| format!("pull request #{n}"));
        let short = head.get(..9).unwrap_or(head);
        let text = if ["non-fast-forward", "fetch first", "[rejected]"]
            .iter()
            .any(|needle| reason.contains(needle))
        {
            format!(
                "Yulius could not push {short} to {target}: GitHub refused it because the branch \
                 there is not an ancestor of your HEAD (your branch was rebased or amended after \
                 it was pushed). Do not force-push. Merge the pushed head back in: `git fetch \
                 origin && git merge --no-edit origin/{branch}` (the same changes merge \
                 cleanly), rerun the tests if anything changed, and end your turn: Yulius pushes \
                 again. Bring the integration branch in by merging it, never by rebasing."
            )
        } else {
            format!(
                "Yulius could not push {short} to {target}: {reason} It tries again at the end of \
                 your next turn; `delivery_status` shows the branch state."
            )
        };
        if let Ok(mut problem) = self.runtime(task_id).push_problem.lock() {
            *problem = Some((head.to_owned(), text));
        }
    }

    fn clear_push_problem(&self, task_id: &TaskId) {
        if let Ok(mut problem) = self.runtime(task_id).push_problem.lock() {
            *problem = None;
        }
    }

    /// What Yulius did for a ready task: opened its pull request, or pushed commits made since
    /// to the open one (#68). `None` while the task is not ready (no branch or worktree, an
    /// invalid spec, an open step, uncommitted work, no commits ahead of the integration branch)
    /// or nothing is new.
    async fn try_auto_open_pr(&self, task_id: &TaskId) -> AppResult<Option<String>> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let (Some(branch), Some(worktree)) = (task.branch.clone(), task.worktree_path.clone())
        else {
            return Ok(None);
        };
        // Cheap checks first: most turns end with steps still open, and they need no GitHub call.
        let steps = store.steps(&task.id).await?;
        let open_step = steps
            .iter()
            .find(|s| !StepStatus::parse(&s.status).is_some_and(StepStatus::is_resolved));
        if (steps.is_empty() && !self.branch_changed(&task).await?)
            || store
                .delivery(&task.id)
                .await?
                .is_some_and(|r| !matches!(r.stage, DeliveryStage::None | DeliveryStage::PrOpen))
        {
            return Ok(None);
        }
        if let Some(step) = open_step {
            self.note_undelivered(
                &task,
                &worktree,
                &format!(
                    "step \"{}\" is {}: finish it, or resolve it with `step_update` and a note",
                    step.title, step.status
                ),
            )
            .await;
            return Ok(None);
        }
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let profile = delivery_profile(&sandbox);
        if branch == profile.integration_branch || branch == profile.production_branch {
            return Ok(None);
        }
        // The task's owner pushes and authors the pull request (their GitHub account, as if
        // they had pressed Deliver…, F 2026-10-11); without a usable token the App does.
        let token = self.task_github_token(&task, &sandbox).await?;
        let observed = self
            .dlv_observe(&sandbox, &task, &profile, Some(&token))
            .await?;
        // A task that needs a spec needs its plan too; research, docs and chores may have none.
        let not_ready = if !observed.spec.valid {
            Some(format!(
                "the spec is not valid ({}); fix it and commit",
                observed.spec.problems.join(" ")
            ))
        } else if observed.steps.is_empty() && observed.spec.required {
            Some("the task has no plan: record its steps with `step_update`".to_owned())
        } else if observed.steps_open > 0 {
            Some("a step is still open: finish it or resolve it with `step_update`".to_owned())
        } else if observed.dirty {
            Some("the worktree has uncommitted changes: commit them".to_owned())
        } else {
            None
        };
        if let Some(reason) = not_ready {
            self.note_undelivered(&task, &worktree, &reason).await;
            return Ok(None);
        }
        let Some(head) = observed.worktree_head.clone() else {
            return Ok(None);
        };
        // The pull request exists: push what was committed since, as the briefing promises,
        // so it does not stay on an old head while the task shows Done (#68).
        if observed.record.stage == DeliveryStage::PrOpen || observed.pr.is_some() {
            let Some(pull) = observed.pr.as_ref().filter(|p| is_open(p)) else {
                return Ok(None);
            };
            if pull.head_sha == head || !self.claim_auto_head(task_id, &head)? {
                return Ok(None);
            }
            let pushed = self
                .deps
                .git
                .push(Path::new(&worktree), &branch, &branch, &token)
                .await;
            if let Err(err) = pushed {
                self.release_auto_head(task_id);
                self.set_push_problem(task_id, &head, &branch, Some(pull.number), &err);
                return Err(err);
            }
            self.clear_push_problem(task_id);
            return Ok(Some(format!(
                "Yulius pushed your new commits to pull request #{}.",
                pull.number
            )));
        }
        let (ahead, _) = self
            .deps
            .git
            .ahead_behind(
                Path::new(&worktree),
                &format!("origin/{}", profile.integration_branch),
                "HEAD",
            )
            .await?;
        if ahead == 0 || !self.claim_auto_head(task_id, &head)? {
            return Ok(None);
        }
        let opened = match dlv::apply(
            &observed.record.stage,
            &DeliveryAction::OpenPr,
            Initiator::Policy,
            &profile,
            &observed.facts(),
        ) {
            Ok(_) => {
                self.dlv_open_pull(
                    &sandbox, &task, &profile, &observed, &token, None, None, "yulius",
                )
                .await
            }
            Err(err) => Err(refusal(&err)),
        };
        match opened {
            Ok(number) => {
                self.clear_push_problem(task_id);
                Ok(Some(format!(
                    "Yulius opened pull request #{number} into {}: the task is ready for review.",
                    profile.integration_branch
                )))
            }
            Err(err) => {
                self.release_auto_head(task_id);
                self.set_push_problem(task_id, &head, &branch, None, &err);
                Err(err)
            }
        }
    }

    /// The part of [`App::task_is_done`] that needs no GitHub call: the turn ended, no background
    /// shell runs, and the goal is achieved or the plan is complete with the change merged. The
    /// session limit counts tasks by it (SES-101), many times a minute.
    pub(crate) async fn task_is_done_locally(&self, task: &TaskRecord) -> bool {
        use yulius_domain::task::{Activity, TaskStatus};
        if task.status != TaskStatus::Running
            || !task.activity.is_some_and(Activity::is_idle)
            || self.background_shells_of(&task.id).unwrap_or(0) > 0
        {
            return false;
        }
        if task.activity == Some(Activity::Done) {
            return true;
        }
        match self.deps.store.steps(&task.id).await {
            Ok(steps) if !crate::sandboxes::plan_open(&steps) => {}
            _ => return false,
        }
        matches!(
            self.deps.store.delivery(&task.id).await,
            Ok(Some(record)) if past_merge(&record.stage)
        )
    }

    /// Whether a task is Done (#35, the sidebar's rule; SPEC-001 TSK-012, TSK-014): its turn
    /// ended (the agent is idle, or the session was closed), no background shell of the agent is
    /// known to run, and either Claude Code recorded its goal as achieved (activity `done`) or
    /// its plan is complete and its pull request is merged, or open with checks green or none
    /// required. Anything unknown (no pull request, GitHub unreachable) is not Done.
    pub(crate) async fn task_is_done(&self, task: &TaskRecord) -> bool {
        use yulius_domain::task::{Activity, TaskStatus};
        let running = task.status == TaskStatus::Running;
        let turn_ended = (running && task.activity.is_some_and(Activity::is_idle))
            || task.status == TaskStatus::Closed;
        if !turn_ended || (running && self.background_shells_of(&task.id).unwrap_or(0) > 0) {
            return false;
        }
        if running && task.activity == Some(Activity::Done) {
            return true;
        }
        // A pull request makes a task Done only when its plan is complete (#139, TSK-014).
        match self.deps.store.steps(&task.id).await {
            Ok(steps) if !crate::sandboxes::plan_open(&steps) => {}
            _ => return false,
        }
        let Ok(Some(record)) = self.deps.store.delivery(&task.id).await else {
            return false;
        };
        if past_merge(&record.stage) {
            return true;
        }
        if record.stage != DeliveryStage::PrOpen {
            return false;
        }
        let Ok(sandbox) = self.dlv_sandbox_of(task).await else {
            return false;
        };
        let Ok(token) = self.dlv_installation_token(&sandbox).await else {
            return false;
        };
        let profile = delivery_profile(&sandbox);
        match self
            .dlv_observe(&sandbox, task, &profile, Some(&token))
            .await
        {
            Ok(observed) => match observed.pr_state() {
                PrState::Merged => true,
                PrState::Open => {
                    matches!(observed.checks, ChecksState::Success | ChecksState::None)
                }
                PrState::Absent | PrState::Closed => false,
            },
            Err(_) => false,
        }
    }

    /// Whether a task's pull request is recorded as merged.
    pub(crate) async fn task_merged(&self, task: &TaskId) -> bool {
        matches!(self.deps.store.delivery(task).await, Ok(Some(record)) if past_merge(&record.stage))
    }

    /// Claims `head` for one automatic attempt, so two Stop hooks never push or open twice;
    /// `false` when this head was already handled.
    fn claim_auto_head(&self, task_id: &TaskId, head: &str) -> AppResult<bool> {
        let runtime = self.runtime(task_id);
        let mut tried = runtime.auto_pr_head.lock().map_err(AppError::internal)?;
        if tried.as_deref() == Some(head) {
            return Ok(false);
        }
        *tried = Some(head.to_owned());
        Ok(true)
    }

    /// Forgets a failed attempt, so the next clean Stop tries again (#68).
    fn release_auto_head(&self, task_id: &TaskId) {
        if let Ok(mut tried) = self.runtime(task_id).auto_pr_head.lock() {
            *tried = None;
        }
    }

    /// Reconciles deliveries in progress with GitHub in the background (#35): a pull request
    /// merged or closed outside Yulius moves its task on the Git board without anyone opening
    /// the task. Uses each sandbox's installation token; failures leave the stage as recorded.
    pub async fn reconcile_deliveries(&self) {
        let store = &self.deps.store;
        let Ok(sandboxes) = store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            let Ok(records) = store.deliveries(&sandbox.id).await else {
                continue;
            };
            let mut open = Vec::new();
            // Only open pull requests can change on GitHub (merged, closed); later stages move
            // through Yulius, so re-reading them would only cost requests (#68).
            for record in records
                .into_iter()
                .filter(|r| r.stage == DeliveryStage::PrOpen)
            {
                if let Ok(Some(task)) = store.task(&record.task_id).await
                    && task.archived_at.is_none()
                {
                    open.push((task, record.stage));
                }
            }
            if open.is_empty() {
                continue;
            }
            let Ok(token) = self.dlv_installation_token(sandbox).await else {
                continue;
            };
            let profile = delivery_profile(sandbox);
            for (task, before) in open {
                if let Ok(observed) = self
                    .dlv_observe(sandbox, &task, &profile, Some(&token))
                    .await
                {
                    self.retry_errored_checks(&task, &observed).await;
                    if let Ok(Some(after)) = store.delivery(&task.id).await
                        && after.stage != before
                    {
                        self.publish_task(&task.id).await;
                    }
                }
            }
        }
    }

    /// Queues the latest Yulius CI run at `sha` again when it ended in an error (it could not
    /// run): retried once by itself; two errored runs at one commit wait for the agent.
    async fn retry_errored_run(&self, task: &TaskRecord, sha: &str) {
        let Ok(runs) = self.deps.store.ci_runs_for_sha(&task.sandbox_id, sha).await else {
            return;
        };
        let Some(latest) = runs.first() else {
            return;
        };
        let errored = runs
            .iter()
            .filter(|r| r.state == yulius_domain::ci::RunState::Error)
            .count();
        if latest.state == yulius_domain::ci::RunState::Error
            && errored < 2
            && let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await
        {
            let _ = self.ci_rerun(&sandbox, &latest.id, "yulius").await;
        }
    }

    /// An errored Yulius CI run at an open pull request's head (it could not run) is queued
    /// again once by itself (SPEC-020); `yulius wait` and `ci_status` say the rest.
    async fn retry_errored_checks(&self, task: &TaskRecord, observed: &Observed) {
        let Some(pull) = observed.pr.as_ref().filter(|p| is_open(p)) else {
            return;
        };
        if observed.checks == ChecksState::Failure && pull.mergeable != Some(false) {
            self.retry_errored_run(task, &pull.head_sha).await;
        }
    }

    // ---------------------------------------------------------------- merge

    /// `POST /api/tasks/{tid}/delivery/merge`: merges the task pull request with the profile's
    /// method at the head that was checked, after the domain gates (spec, open steps, checks
    /// per profile, current head). With `auto_promote`, promotes right after (policy). It acts
    /// as the caller on GitHub, so the superadmin without a GitHub identity is told to merge on
    /// GitHub (ACC-012).
    ///
    /// # Errors
    /// `Conflict` refusals (including stale heads), `Forbidden` without a GitHub identity,
    /// `Upstream` GitHub failures.
    pub async fn merge_task_pr(&self, ctx: &AuthContext, task_id: &str) -> AppResult<Delivery> {
        let task = self.load_task(ctx, task_id, Action::Work).await?;
        let token = self
            .person_github_token(&ctx.user, "Merging a pull request")
            .await?;
        self.dlv_merge(ctx, task, &token).await
    }

    /// [`App::merge_task_pr`] for the task's agent (`delivery_merge`) under its owner's
    /// authority `ctx`: GitHub sees the owner merge, or the App when the owner has no usable
    /// GitHub token (F 2026-10-11).
    ///
    /// # Errors
    /// As [`App::merge_task_pr`], without the refusal for an account with no GitHub identity.
    pub(crate) async fn merge_task_pr_for_task(
        &self,
        ctx: &AuthContext,
        task_id: &str,
    ) -> AppResult<Delivery> {
        let task = self.load_task(ctx, task_id, Action::Work).await?;
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let token = self.task_github_token(&task, &sandbox).await?;
        self.dlv_merge(ctx, task, &token).await
    }

    /// Merges `task`'s pull request with `token` (see [`App::merge_task_pr`]).
    async fn dlv_merge(
        &self,
        ctx: &AuthContext,
        task: TaskRecord,
        token: &Secret,
    ) -> AppResult<Delivery> {
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let profile = delivery_profile(&sandbox);
        let observed = self
            .dlv_observe(&sandbox, &task, &profile, Some(token))
            .await?;
        dlv::apply(
            &observed.record.stage,
            &DeliveryAction::Merge,
            Initiator::User,
            &profile,
            &observed.facts(),
        )
        .map_err(|err| refusal(&err))?;
        let pull = observed
            .pr
            .clone()
            .ok_or_else(|| AppError::Conflict("the pull request cannot be read".into()))?;
        // GitHub's own refusal ("required status checks are expected") reads like a missing
        // permission; say what it means and what to do (F 2026-10-11).
        if pull.behind {
            return Err(AppError::Conflict(format!(
                "the branch is behind `{base}` and the repository requires it to be up to date: \
                 merge `origin/{base}` into it, push, wait for the checks on the new head, then \
                 merge that head",
                base = pull.base
            )));
        }
        let merge_sha = self
            .deps
            .github
            .merge_pull(
                token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                pull.number,
                profile.merge.as_str(),
                &pull.head_sha,
            )
            .await?;
        let next = DeliveryRecord {
            task_id: task.id.clone(),
            stage: DeliveryStage::MergedIntegration,
            pr_number: Some(pull.number),
            updated_at: self.now(),
        };
        self.deps
            .store
            .transition_delivery(&observed.record.stage, &next)
            .await?;
        let actor = Self::dlv_actor_login(&ctx.user);
        self.dlv_audit(
            &actor,
            "delivery.merge",
            &format!("{}#{}", sandbox.slug, task.number),
            &format!(
                "pull request #{} merged into {} ({}) at head {} -> {}",
                pull.number,
                profile.integration_branch,
                profile.merge.as_str(),
                pull.head_sha,
                merge_sha
            ),
        )
        .await;
        let fetched = match self.dlv_refresh_clone(&sandbox).await {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(error = %err, "refreshing the sandbox clone after a merge failed");
                false
            }
        };
        let mut notes = Vec::new();
        // Hygiene (DLV-007): the merged branch goes, unless protected or still in use.
        if let Some(note) = self.delete_merged_branch(&sandbox, &pull).await {
            notes.push(note);
        }
        if let Some(note) = self
            .dlv_auto_promote_if_enabled(&sandbox, &profile, &ctx.user, token, fetched)
            .await
        {
            notes.push(note);
        }
        self.publish_task(&task.id).await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let mut view = self.delivery_view(&task, Some(&ctx.user)).await?;
        view.notes.extend(notes);
        Ok(view)
    }

    // ---------------------------------------------------------------- promotion

    /// Tasks of the sandbox (archived ones included) with their delivery rows.
    async fn dlv_sandbox_deliveries(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Vec<(TaskRecord, DeliveryRecord)>> {
        let store = &self.deps.store;
        let records = store.deliveries(&sandbox.id).await?;
        let mut tasks = store.tasks(&sandbox.id, false).await?;
        tasks.extend(store.tasks(&sandbox.id, true).await?);
        tasks.sort_by_key(|t| t.number);
        Ok(tasks
            .into_iter()
            .map(|task| {
                let record = records
                    .iter()
                    .find(|r| r.task_id == task.id)
                    .cloned()
                    .unwrap_or_else(|| DeliveryRecord::initial(&task.id));
                (task, record)
            })
            .collect())
    }

    /// Moves tasks to their computed stages (compare-and-set each); returns the numbers moved.
    async fn dlv_advance(
        &self,
        moves: Vec<(TaskRecord, DeliveryRecord, DeliveryStage)>,
    ) -> AppResult<Vec<u32>> {
        let mut moved = Vec::new();
        for (task, record, next) in moves {
            if next == record.stage {
                continue;
            }
            let updated = DeliveryRecord {
                stage: next,
                updated_at: self.now(),
                ..record.clone()
            };
            if self
                .deps
                .store
                .transition_delivery(&record.stage, &updated)
                .await?
            {
                moved.push(task.number);
                self.publish_task(&task.id).await;
            }
        }
        Ok(moved)
    }

    /// `(integration-only commits, production-only commits)` from the sandbox clone, fetched
    /// first unless `fetched`; `None` when Git cannot tell.
    async fn dlv_branch_relation(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        fetched: bool,
    ) -> Option<(u32, u32)> {
        if !fetched {
            self.dlv_refresh_clone(sandbox).await.ok()?;
        }
        let repo = self.sandbox_repo(&sandbox.id);
        self.deps
            .git
            .ahead_behind(
                &repo,
                &format!("origin/{}", profile.production_branch),
                &format!("origin/{}", profile.integration_branch),
            )
            .await
            .ok()
    }

    /// Promotes the integration branch: fast-forward, or open/merge the promotion pull request
    /// (merge commit). Tasks waiting for promotion advance once production moved. `fetched`
    /// says the sandbox clone was fetched just before.
    async fn dlv_promote(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        by: Initiator,
        actor: &UserRecord,
        token: &Secret,
        fetched: bool,
    ) -> AppResult<PromoteResult> {
        let candidates: Vec<(TaskRecord, DeliveryRecord)> = self
            .dlv_sandbox_deliveries(sandbox)
            .await?
            .into_iter()
            .filter(|(_, record)| region(&record.stage, profile) == Some(Stage::Integration))
            .collect();
        for (task, record) in &candidates {
            if let Some(env) = pending_integration_environment(&record.stage, profile) {
                return Err(AppError::Conflict(format!(
                    "task #{}: environment '{}' is not done yet; finish it before promoting",
                    task.number, env.name
                )));
            }
        }
        if by == Initiator::Policy && candidates.is_empty() {
            return Err(AppError::Conflict(
                "nothing merged through Yulius is waiting for promotion".into(),
            ));
        }
        let github = &self.deps.github;
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        let integration_sha = github
            .branch_sha(token, owner, name, &profile.integration_branch)
            .await?
            .ok_or_else(|| {
                AppError::Conflict(format!(
                    "branch '{}' does not exist on GitHub",
                    profile.integration_branch
                ))
            })?;
        let production_sha = github
            .branch_sha(token, owner, name, &profile.production_branch)
            .await?;
        let relation = self.dlv_branch_relation(sandbox, profile, fetched).await;
        let contained = production_sha.as_deref() == Some(integration_sha.as_str())
            || relation.is_some_and(|(ahead, _)| ahead == 0);
        if contained {
            if candidates.is_empty() {
                return Err(AppError::Conflict(format!(
                    "nothing to promote: '{}' already contains '{}'",
                    profile.production_branch, profile.integration_branch
                )));
            }
            // Promoted outside Yulius (for example the promotion pull request merged on
            // GitHub): record it instead of leaving the tasks waiting.
            let facts = merged_facts(false);
            let mut moves = Vec::new();
            for (task, record) in candidates {
                let next = dlv::apply(&record.stage, &DeliveryAction::Promote, by, profile, &facts)
                    .map_err(|err| refusal(&err))?;
                moves.push((task, record, next));
            }
            let numbers: Vec<String> = moves
                .iter()
                .map(|(t, _, _)| format!("#{}", t.number))
                .collect();
            self.dlv_audit(
                &Self::dlv_policy_actor(by, actor),
                "delivery.promote.reconciled",
                &sandbox.slug,
                &format!(
                    "'{}' already contains '{}'; tasks [{}] recorded as promoted",
                    profile.production_branch,
                    profile.integration_branch,
                    numbers.join(", ")
                ),
            )
            .await;
            self.dlv_advance(moves).await?;
            return Ok(PromoteResult {
                result: "promoted".into(),
                sha: production_sha,
            });
        }
        let diverged = profile.promote == PromoteMethod::FastForward
            && relation.is_some_and(|(_, behind)| behind > 0);
        let facts = merged_facts(diverged);
        let mut moves = Vec::new();
        if candidates.is_empty() {
            if diverged {
                return Err(refusal(&DeliveryError::ProductionDiverged {
                    production: profile.production_branch.clone(),
                    integration: profile.integration_branch.clone(),
                }));
            }
        } else {
            for (task, record) in candidates {
                let next = dlv::apply(&record.stage, &DeliveryAction::Promote, by, profile, &facts)
                    .map_err(|err| refusal(&err))?;
                moves.push((task, record, next));
            }
        }
        let numbers: Vec<String> = moves
            .iter()
            .map(|(t, _, _)| format!("#{}", t.number))
            .collect();
        let (result, sha) = match profile.promote {
            PromoteMethod::FastForward => {
                github
                    .fast_forward(
                        token,
                        owner,
                        name,
                        &profile.production_branch,
                        &integration_sha,
                    )
                    .await?;
                ("promoted", integration_sha.clone())
            }
            PromoteMethod::MergeCommit => {
                let existing = github
                    .list_pulls(token, owner, name, None)
                    .await?
                    .into_iter()
                    .find(|p| {
                        is_open(p)
                            && p.head == profile.integration_branch
                            && p.base == profile.production_branch
                    });
                match existing {
                    None => {
                        let mut body = format!(
                            "Promotes `{}` to `{}` with a merge commit.\n\n",
                            profile.integration_branch, profile.production_branch
                        );
                        for (task, _, _) in &moves {
                            body.push_str(&format!("- #{} {}\n", task.number, task.title));
                        }
                        body.push_str(&format!(
                            "\n---\nOpened from Yulius: {}/s/{}/git",
                            self.config.public_url, sandbox.slug
                        ));
                        let pull = github
                            .create_pull(
                                token,
                                owner,
                                name,
                                &NewPull {
                                    head: profile.integration_branch.clone(),
                                    base: profile.production_branch.clone(),
                                    title: format!(
                                        "Promote {} to {}",
                                        profile.integration_branch, profile.production_branch
                                    ),
                                    body: clip_bytes(&body, MAX_PR_BODY_BYTES).to_owned(),
                                },
                            )
                            .await?;
                        self.dlv_audit(
                            &Self::dlv_policy_actor(by, actor),
                            "delivery.promotion_pr.open",
                            &sandbox.slug,
                            &format!(
                                "pull request #{} {} -> {} for tasks [{}]",
                                pull.number,
                                profile.integration_branch,
                                profile.production_branch,
                                numbers.join(", ")
                            ),
                        )
                        .await;
                        return Ok(PromoteResult {
                            result: "pr_opened".into(),
                            sha: Some(pull.html_url),
                        });
                    }
                    Some(_) if by == Initiator::Policy => {
                        return Err(AppError::Conflict(
                            "the promotion pull request waits for a person to merge it".into(),
                        ));
                    }
                    Some(pull) => {
                        if profile.require_checks {
                            let report = github.checks(token, owner, name, &pull.head_sha).await?;
                            match domain_checks(report.state) {
                                ChecksState::Failure => {
                                    return Err(refusal(&DeliveryError::ChecksFailing));
                                }
                                ChecksState::Pending => {
                                    return Err(refusal(&DeliveryError::ChecksPending));
                                }
                                // Unreadable checks never count as passed (#111).
                                ChecksState::Unknown => {
                                    return Err(refusal(&DeliveryError::ChecksUnknown));
                                }
                                ChecksState::None | ChecksState::Success => {}
                            }
                        }
                        let sha = github
                            .merge_pull(
                                token,
                                owner,
                                name,
                                pull.number,
                                MergeMethod::Merge.as_str(),
                                &pull.head_sha,
                            )
                            .await?;
                        ("promoted", sha)
                    }
                }
            }
        };
        self.dlv_audit(
            &Self::dlv_policy_actor(by, actor),
            "delivery.promote",
            &sandbox.slug,
            &format!(
                "{} -> {} ({}) at {}; tasks [{}]",
                profile.integration_branch,
                profile.production_branch,
                profile.promote.as_str(),
                sha,
                numbers.join(", ")
            ),
        )
        .await;
        self.dlv_advance(moves).await?;
        Ok(PromoteResult {
            result: result.into(),
            sha: Some(sha),
        })
    }

    fn dlv_policy_actor(by: Initiator, actor: &UserRecord) -> String {
        match by {
            Initiator::User => Self::dlv_actor_login(actor),
            Initiator::Policy => format!(
                "policy:auto_promote (after {})",
                Self::dlv_actor_login(actor)
            ),
        }
    }

    /// Runs the profile's automatic promotion when it is enabled and every merged change has
    /// finished its integration environments. Returns a note describing the outcome.
    async fn dlv_auto_promote_if_enabled(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        actor: &UserRecord,
        token: &Secret,
        fetched: bool,
    ) -> Option<String> {
        if !profile.auto_promote {
            return None;
        }
        let deliveries = self.dlv_sandbox_deliveries(sandbox).await.ok()?;
        let waiting: Vec<&DeliveryRecord> = deliveries
            .iter()
            .map(|(_, record)| record)
            .filter(|record| region(&record.stage, profile) == Some(Stage::Integration))
            .collect();
        if waiting.is_empty()
            || waiting
                .iter()
                .any(|record| pending_integration_environment(&record.stage, profile).is_some())
        {
            return None;
        }
        match self
            .dlv_promote(sandbox, profile, Initiator::Policy, actor, token, fetched)
            .await
        {
            Ok(result) => Some(format!(
                "Automatic promotion ({}): {} {}",
                profile.promote.as_str(),
                result.result,
                result.sha.unwrap_or_default()
            )),
            Err(err) => {
                self.dlv_audit(
                    &Self::dlv_policy_actor(Initiator::Policy, actor),
                    "delivery.promote.failed",
                    &sandbox.slug,
                    &err.public_message(),
                )
                .await;
                Some(format!(
                    "Automatic promotion did not happen: {}",
                    err.public_message()
                ))
            }
        }
    }

    /// `POST /api/sandboxes/{sid}/delivery/promote` (write): an explicit promotion. It acts as
    /// the caller on GitHub, so the superadmin without a GitHub identity is told to release on
    /// GitHub (ACC-012).
    ///
    /// # Errors
    /// `Conflict` when environments are pending, production diverged or nothing changed;
    /// `Forbidden` without a GitHub identity.
    pub async fn promote_sandbox(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<PromoteResult> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::Work)
            .await?;
        let profile = delivery_profile(&sandbox);
        let token = self
            .person_github_token(
                &ctx.user,
                &format!("Releasing to {}", profile.production_branch),
            )
            .await?;
        self.dlv_promote(
            &sandbox,
            &profile,
            Initiator::User,
            &ctx.user,
            &token,
            false,
        )
        .await
    }

    /// [`App::promote_sandbox`] for `task`'s agent (`delivery_promote`) under its owner's
    /// authority `ctx`: GitHub sees the owner promote, or the App when the owner has no
    /// usable GitHub token (F 2026-10-11).
    ///
    /// # Errors
    /// As [`App::promote_sandbox`], without the refusal for an account with no GitHub identity.
    pub(crate) async fn promote_sandbox_for_task(
        &self,
        ctx: &AuthContext,
        task: &TaskRecord,
    ) -> AppResult<PromoteResult> {
        let sandbox = self
            .dlv_require_sandbox(ctx, task.sandbox_id.as_str(), Action::Work)
            .await?;
        let profile = delivery_profile(&sandbox);
        let token = self.task_github_token(task, &sandbox).await?;
        self.dlv_promote(
            &sandbox,
            &profile,
            Initiator::User,
            &ctx.user,
            &token,
            false,
        )
        .await
    }

    // ---------------------------------------------------------------- environments

    /// Tasks for which `action` applies now. When none does but some task in the
    /// environment's stage has not reached it yet, that task's refusal is returned (for example
    /// an environment out of order). Otherwise the action is a standalone run (a redeploy, or
    /// no delivered change waiting), which changes no stage.
    async fn dlv_due_tasks(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        environment: &Environment,
        action: &DeliveryAction,
        by: Initiator,
    ) -> AppResult<Vec<(TaskRecord, DeliveryRecord, DeliveryStage)>> {
        let facts = merged_facts(false);
        let mut due = Vec::new();
        let mut refused = None;
        for (task, record) in self.dlv_sandbox_deliveries(sandbox).await? {
            match dlv::apply(&record.stage, action, by, profile, &facts) {
                Ok(next) => due.push((task, record, next)),
                Err(err) => {
                    if refused.is_none() && not_yet_reached(&record.stage, profile, environment) {
                        refused = Some((task.number, err));
                    }
                }
            }
        }
        if due.is_empty()
            && let Some((number, err)) = refused
        {
            return Err(match refusal(&err) {
                AppError::Conflict(message) => {
                    AppError::Conflict(format!("task #{number}: {message}"))
                }
                other => other,
            });
        }
        Ok(due)
    }

    fn dlv_environment_named<'p>(
        profile: &'p DeliveryProfile,
        name: &str,
    ) -> AppResult<&'p Environment> {
        profile
            .environment(name)
            .ok_or_else(|| refusal(&DeliveryError::UnknownEnvironment(clip_chars(name, 64))))
    }

    /// `POST /api/sandboxes/{sid}/delivery/environments/{env}/runs` (write). Requires
    /// `{"confirm": true}`. An environment with a command starts it in the background (see the
    /// module trust contract); one without a command records the person's confirmation.
    ///
    /// # Errors
    /// `Invalid` without confirmation or for unknown environments, `Conflict` when out of order
    /// or while another run of the sandbox is in progress.
    pub async fn start_environment_run(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &RunEnvironment,
    ) -> AppResult<EnvironmentRun> {
        self.dlv_start_environment_run(ctx, sandbox_id, environment, req, None)
            .await
    }

    /// [`App::start_environment_run`]; for a task's agent (`delivery_deploy`, `for_task`) the
    /// automatic promotion after the run uses the task's GitHub token (its owner's, else the
    /// App's, F 2026-10-11) instead of the caller's own.
    pub(crate) async fn dlv_start_environment_run(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &RunEnvironment,
        for_task: Option<&TaskRecord>,
    ) -> AppResult<EnvironmentRun> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::Work)
            .await?;
        let profile = delivery_profile(&sandbox);
        let env = Self::dlv_environment_named(&profile, environment)?.clone();
        let Some(command) = env.command.clone() else {
            return self
                .dlv_record_confirmation(ctx, &sandbox, &profile, &env, req.confirm)
                .await;
        };
        if !req.confirm {
            return Err(refusal(&DeliveryError::ConfirmationRequired(
                env.name.clone(),
            )));
        }
        let action = DeliveryAction::RunEnvironment {
            name: env.name.clone(),
            confirmed: true,
        };
        self.dlv_due_tasks(&sandbox, &profile, &env, &action, Initiator::User)
            .await?;
        let run = EnvironmentRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: env.name.clone(),
            kind: EnvironmentRunKind::Command,
            requested_by: ctx.user.id.clone(),
            command: Some(command.clone()),
            commit: None,
            state: EnvironmentRunState::Running,
            output: String::new(),
            exit_code: None,
            started_at: self.now(),
            ended_at: None,
        };
        self.deps
            .store
            .insert_environment_run(&run)
            .await
            .map_err(|err| match err {
                AppError::Conflict(_) => AppError::Conflict(
                    "another environment run is in progress in this project; wait for it to finish"
                        .into(),
                ),
                other => other,
            })?;
        // The audit entry is part of the trust contract: no entry, no run.
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &Self::dlv_actor_login(&ctx.user),
                "delivery.environment.run",
                Some(&format!("{}/{}", sandbox.slug, env.name)),
                Some(&format!("run {}; command: {command}", run.id)),
            )
            .await
        {
            let _ = self
                .deps
                .store
                .finish_environment_run(
                    &run.id,
                    EnvironmentRunState::Error,
                    "[yulius] not started: the audit entry could not be written\n",
                    None,
                    self.now(),
                )
                .await;
            return Err(err);
        }
        let app = Arc::clone(self);
        let user = ctx.user.clone();
        let run_id = run.id.clone();
        let for_task = for_task.cloned();
        tokio::spawn(async move {
            app.dlv_execute_environment_run(sandbox, profile, env, run_id, user, for_task)
                .await;
        });
        self.dlv_run_view(&run, None).await
    }

    /// `POST /api/sandboxes/{sid}/delivery/environments/{env}/confirm` (write): a person
    /// confirms a manually verified environment (`{"confirm": true}` required).
    ///
    /// # Errors
    /// `Invalid` without confirmation or for command-verified environments, `Conflict` out of
    /// order.
    pub async fn confirm_environment(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &RunEnvironment,
    ) -> AppResult<EnvironmentRun> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::Work)
            .await?;
        let profile = delivery_profile(&sandbox);
        let env = Self::dlv_environment_named(&profile, environment)?.clone();
        self.dlv_record_confirmation(ctx, &sandbox, &profile, &env, req.confirm)
            .await
    }

    async fn dlv_record_confirmation(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        env: &Environment,
        confirmed: bool,
    ) -> AppResult<EnvironmentRun> {
        if !confirmed {
            return Err(AppError::Invalid(format!(
                "confirming environment '{}' requires explicit confirmation",
                env.name
            )));
        }
        if env.verify != Verify::Manual {
            return Err(refusal(&DeliveryError::WrongEvidence {
                name: env.name.clone(),
                verify: "its command",
            }));
        }
        let action = DeliveryAction::CompleteEnvironment {
            name: env.name.clone(),
            evidence: EnvironmentEvidence::UserConfirmed,
        };
        let due = self
            .dlv_due_tasks(sandbox, profile, env, &action, Initiator::User)
            .await?;
        let login = Self::dlv_actor_login(&ctx.user);
        let now = self.now();
        let moved = self.dlv_advance(due).await?;
        let mut output = format!(
            "Environment '{}' confirmed by {login} at {}.\n",
            env.name,
            rfc3339(now)
        );
        if moved.is_empty() {
            output.push_str("No delivered task was waiting for this environment.\n");
        } else {
            let numbers: Vec<String> = moved.iter().map(|n| format!("#{n}")).collect();
            output.push_str(&format!("Tasks advanced: {}.\n", numbers.join(", ")));
        }
        if env.after == Stage::Integration
            && let Ok(token) = self.user_github_token(&ctx.user).await
            && let Some(note) = self
                .dlv_auto_promote_if_enabled(sandbox, profile, &ctx.user, &token, false)
                .await
        {
            output.push_str(&note);
            output.push('\n');
        }
        let run = EnvironmentRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: env.name.clone(),
            kind: EnvironmentRunKind::Manual,
            requested_by: ctx.user.id.clone(),
            command: None,
            commit: None,
            state: EnvironmentRunState::Succeeded,
            output,
            exit_code: None,
            started_at: now,
            ended_at: Some(now),
        };
        self.deps.store.insert_environment_run(&run).await?;
        self.dlv_audit(
            &login,
            "delivery.environment.confirm",
            &format!("{}/{}", sandbox.slug, env.name),
            &format!("run {}; tasks advanced: {moved:?}", run.id),
        )
        .await;
        self.dlv_run_view(&run, None).await
    }

    /// Background part of a command run: fetch, check out the stage's branch head, run the
    /// command, record the outcome and — for command-verified environments that succeeded —
    /// advance the waiting tasks (and auto-promote when enabled, with `for_task`'s GitHub token
    /// when a task's agent started the run, else `user`'s).
    async fn dlv_execute_environment_run(
        self: Arc<Self>,
        sandbox: SandboxRecord,
        profile: DeliveryProfile,
        env: Environment,
        run_id: String,
        user: UserRecord,
        for_task: Option<TaskRecord>,
    ) {
        let mut ring = OutputRing::new(ENVIRONMENT_OUTPUT_BYTES);
        let (tx, mut rx) = watch::channel(String::new());
        let store = self.deps.store.clone();
        let writer_id = run_id.clone();
        let writer = tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let text = rx.borrow_and_update().clone();
                if let Err(err) = store.update_environment_run(&writer_id, &text, None).await {
                    tracing::warn!(error = %err, "storing environment run output failed");
                }
            }
        });
        let outcome = self
            .dlv_environment_steps(&sandbox, &profile, &env, &run_id, &mut ring, &tx)
            .await;
        drop(tx);
        let _ = writer.await;
        let (state, exit_code) = match &outcome {
            Ok(end) if end.succeeded() => (EnvironmentRunState::Succeeded, Some(0)),
            Ok(end) => {
                ring.push_str(&match end {
                    CommandEnd::Exited(code) => format!("\n[yulius] exited with status {code}\n"),
                    CommandEnd::Signaled(signal) => format!("\n[yulius] terminated by signal {signal}\n"),
                    CommandEnd::TimedOut => format!(
                        "\n[yulius] the command exceeded {} minutes; its process group was killed\n",
                        ENVIRONMENT_TIMEOUT.as_secs() / 60
                    ),
                    CommandEnd::SpawnFailed(message) => format!("\n[yulius] could not start: {message}\n"),
                });
                let state = if matches!(end, CommandEnd::SpawnFailed(_)) {
                    EnvironmentRunState::Error
                } else {
                    EnvironmentRunState::Failed
                };
                (state, end.exit_code())
            }
            Err(message) => {
                ring.push_str(&format!("\n[yulius] {message}\n"));
                (EnvironmentRunState::Error, None)
            }
        };
        let mut advanced = Vec::new();
        let mut note = None;
        if state == EnvironmentRunState::Succeeded && env.verify == Verify::Command {
            let action = DeliveryAction::CompleteEnvironment {
                name: env.name.clone(),
                evidence: EnvironmentEvidence::CommandSucceeded,
            };
            match self
                .dlv_due_tasks(&sandbox, &profile, &env, &action, Initiator::Policy)
                .await
            {
                Ok(due) => advanced = self.dlv_advance(due).await.unwrap_or_default(),
                Err(err) => note = Some(err.public_message()),
            }
            if env.after == Stage::Integration {
                let token = match &for_task {
                    Some(task) => self.task_github_token(task, &sandbox).await,
                    None => self.user_github_token(&user).await,
                };
                if let Ok(token) = token {
                    note = self
                        .dlv_auto_promote_if_enabled(&sandbox, &profile, &user, &token, false)
                        .await
                        .or(note);
                }
            }
        }
        if !advanced.is_empty() {
            let numbers: Vec<String> = advanced.iter().map(|n| format!("#{n}")).collect();
            ring.push_str(&format!(
                "[yulius] tasks advanced: {}\n",
                numbers.join(", ")
            ));
        }
        if let Some(note) = note {
            ring.push_str(&format!("[yulius] {note}\n"));
        }
        let finished = self
            .deps
            .store
            .finish_environment_run(&run_id, state, &ring.text(), exit_code, self.now())
            .await;
        if let Err(err) = finished {
            tracing::error!(error = %err, run = %run_id, "recording the environment run outcome failed");
        }
        self.dlv_audit(
            &Self::dlv_actor_login(&user),
            "delivery.environment.finished",
            &format!("{}/{}", sandbox.slug, env.name),
            &format!("run {run_id}: {} (exit {exit_code:?})", state.as_str()),
        )
        .await;
    }

    async fn dlv_environment_steps(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        env: &Environment,
        run_id: &str,
        ring: &mut OutputRing,
        tx: &watch::Sender<String>,
    ) -> Result<CommandEnd, String> {
        let command = env
            .command
            .clone()
            .ok_or_else(|| format!("environment '{}' has no command", env.name))?;
        let repo = self.sandbox_repo(&sandbox.id);
        let branch = match env.after {
            Stage::Integration => &profile.integration_branch,
            Stage::Production => &profile.production_branch,
        };
        ring.push_str(&format!(
            "[yulius] fetching {}/{}\n",
            sandbox.repo_owner, sandbox.repo_name
        ));
        let _ = tx.send(ring.text());
        self.dlv_refresh_clone(sandbox)
            .await
            .map_err(|err| format!("fetching the repository failed: {}", err.public_message()))?;
        let head = self
            .deps
            .git
            .log(&repo, &format!("origin/{branch}"), 1)
            .await
            .map_err(|err| format!("reading {branch} failed: {}", err.public_message()))?
            .into_iter()
            .next()
            .map(|commit| commit.sha)
            .filter(|sha| is_hex_sha(sha))
            .ok_or_else(|| format!("branch {branch} has no readable head commit"))?;
        let home = self.sandbox_dir(&sandbox.id).join("home");
        tokio::fs::create_dir_all(&home)
            .await
            .map_err(|_| "the sandbox home directory cannot be created".to_owned())?;
        let mut env_vars = vec![
            (
                "PATH".to_owned(),
                std::env::var("PATH")
                    .ok()
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into()),
            ),
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ];
        ring.push_str(&format!("[yulius] checking out {branch} at {head}\n"));
        // The clone's `.git` and the sandbox home are writable by the sandbox's agents: ignore
        // global/system config and disable hooks and fsmonitor for this server-side checkout.
        let mut checkout_env = env_vars.clone();
        checkout_env.extend(
            [
                ("GIT_TERMINAL_PROMPT", "0"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_CONFIG_NOSYSTEM", "1"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned())),
        );
        let checkout = CommandSpec {
            argv: [
                "git",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "checkout",
                "--quiet",
                "--force",
                "--detach",
                head.as_str(),
            ]
            .map(str::to_owned)
            .to_vec(),
            cwd: repo.clone(),
            env: checkout_env,
            timeout: CHECKOUT_TIMEOUT,
        };
        let end = run_bounded(&checkout, ring, Some(tx)).await;
        if !end.succeeded() {
            return Err(format!("checking out {head} failed ({end:?})"));
        }
        let _ = self
            .deps
            .store
            .update_environment_run(run_id, &ring.text(), Some(&head))
            .await;
        env_vars.push(("YULIUS_ENVIRONMENT".into(), env.name.clone()));
        env_vars.push(("YULIUS_BRANCH".into(), branch.clone()));
        env_vars.push(("YULIUS_COMMIT".into(), head.clone()));
        ring.push_str(&format!("[yulius] $ {command}\n"));
        let spec = CommandSpec {
            argv: vec!["/bin/sh".into(), "-c".into(), command],
            cwd: repo,
            env: env_vars,
            timeout: ENVIRONMENT_TIMEOUT,
        };
        Ok(run_bounded(&spec, ring, Some(tx)).await)
    }

    async fn dlv_run_view(
        &self,
        run: &EnvironmentRunRecord,
        output_tail: Option<usize>,
    ) -> AppResult<EnvironmentRun> {
        Ok(EnvironmentRun {
            run_id: run.id.clone(),
            environment: run.environment.clone(),
            kind: run.kind.as_str().to_owned(),
            state: run.state.as_str().to_owned(),
            command: run.command.clone(),
            commit: run.commit.clone(),
            requested_by: self.login_of(&run.requested_by).await,
            output: match output_tail {
                Some(max) => tail_bytes(&run.output, max).to_owned(),
                None => run.output.clone(),
            },
            exit_code: run.exit_code,
            started_at: rfc3339(run.started_at),
            ended_at: run.ended_at.map(rfc3339),
        })
    }

    /// A `running` row older than every run limit has no runner any more: record it as
    /// `error`.
    async fn dlv_expire_lost_run(
        &self,
        run: EnvironmentRunRecord,
    ) -> AppResult<EnvironmentRunRecord> {
        let limit = i64::try_from((ENVIRONMENT_TIMEOUT + CHECKOUT_TIMEOUT).as_millis())
            .unwrap_or(i64::MAX)
            .saturating_add(LOST_RUN_GRACE_MS);
        if run.state != EnvironmentRunState::Running
            || self.now().millis_since(run.started_at) <= limit
        {
            return Ok(run);
        }
        let output = format!(
            "{}\n[yulius] no runner is attached to this run any more (server restart?); its outcome is unknown\n",
            run.output
        );
        let now = self.now();
        self.deps
            .store
            .finish_environment_run(&run.id, EnvironmentRunState::Error, &output, None, now)
            .await?;
        Ok(EnvironmentRunRecord {
            state: EnvironmentRunState::Error,
            output,
            ended_at: Some(now),
            ..run
        })
    }

    /// `GET /api/sandboxes/{sid}/delivery/runs/{run_id}` (read).
    ///
    /// # Errors
    /// `NotFound` for unknown runs or other sandboxes' runs.
    pub async fn environment_run(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<EnvironmentRun> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::View)
            .await?;
        let run = self
            .deps
            .store
            .environment_run(run_id)
            .await?
            .filter(|run| run.sandbox_id == sandbox.id)
            .ok_or(AppError::NotFound)?;
        let run = self.dlv_expire_lost_run(run).await?;
        self.dlv_run_view(&run, None).await
    }

    /// `GET /api/sandboxes/{sid}/delivery/runs` (read): recent runs with output tails.
    ///
    /// # Errors
    /// `NotFound` without read access.
    pub async fn environment_runs(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<Vec<EnvironmentRun>> {
        let sandbox = self
            .dlv_require_sandbox(ctx, sandbox_id, Action::View)
            .await?;
        let mut out = Vec::new();
        for run in self
            .deps
            .store
            .environment_runs(&sandbox.id, RUN_LIST_LIMIT)
            .await?
        {
            let run = self.dlv_expire_lost_run(run).await?;
            out.push(self.dlv_run_view(&run, Some(RUN_LIST_OUTPUT_BYTES)).await?);
        }
        Ok(out)
    }

    /// Startup reconciliation: runs recorded as `running` have no runner after a restart and
    /// are recorded as `error`. Returns how many were closed. Call once before serving.
    ///
    /// # Errors
    /// Store failures.
    pub async fn reconcile_environment_runs(&self) -> AppResult<u32> {
        let mut closed = 0;
        for run in self.deps.store.running_environment_runs().await? {
            let output = format!(
                "{}\n[yulius] interrupted by a server restart; the command may have been killed or may have finished unobserved\n",
                run.output
            );
            if self
                .deps
                .store
                .finish_environment_run(
                    &run.id,
                    EnvironmentRunState::Error,
                    &output,
                    None,
                    self.now(),
                )
                .await?
            {
                closed += 1;
            }
        }
        Ok(closed)
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
        let (docs, _) =
            tokio::task::spawn_blocking(move || crate::onboarding::validate_checkout(&root))
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
        let nudges: Vec<String> = yulius_domain::steps::plan_nudges(&plan, !spec.required)
            .iter()
            .map(ToString::to_string)
            .collect();
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
    pub async fn tool_delivery_status(&self, ctx: &AgentContext) -> AppResult<Value> {
        let task = self
            .deps
            .store
            .task(&ctx.task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let sandbox = self.dlv_sandbox_of(&task).await?;
        let profile = delivery_profile(&sandbox);
        let view = self.delivery_view(&task, None).await?;
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
            "checks_access": view.checks_access.as_ref().map(|a| &a.message),
            "profile": profile_to_contract(&profile),
            "policy": "You deliver your own work (DLV-008): Yulius opens the pull request when your work is ready; merge it on green with delivery_merge after an independent review of its head, run each command-verified environment with delivery_deploy, and promote with delivery_promote. The same checks a person's action needs apply; environments a person must verify stay with people.",
        }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pull_requests_get_a_conventional_title_from_the_branch() {
        let subjects = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            default_pr_title(
                "Prepare this repository for Yulius",
                &subjects(&["docs: fix links", "docs: prepare the repository for Yulius"])
            ),
            "docs: prepare this repository for Yulius"
        );
        assert_eq!(
            default_pr_title(
                "API rate limits",
                &subjects(&["fix(api)!: limit per token"])
            ),
            "fix(api)!: API rate limits"
        );
        assert_eq!(
            default_pr_title("feat(ui): dark mode", &subjects(&["wip"])),
            "feat(ui): dark mode"
        );
        assert_eq!(
            default_pr_title("Add search", &subjects(&["wip", "Merge branch 'dev'"])),
            "Add search"
        );
        assert_eq!(conventional_prefix("Note: not a type"), None);
        assert_eq!(conventional_prefix("feat(: broken"), None);
        assert_eq!(conventional_prefix("chore: "), None);
    }

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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
