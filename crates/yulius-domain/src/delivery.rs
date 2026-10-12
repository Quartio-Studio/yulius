//! Delivery profiles and the per-task delivery state machine (SPEC-001 DLV-001..005, GIT-002,
//! DSC-007/008; SPEC-002 §6.7, ADR-11).
//!
//! A profile names the integration and production branches, how pull requests are merged, how
//! the integration branch is promoted, whether promotion is automatic, and the environments
//! that follow each stage. Three presets cover the known repositories:
//!
//! | Preset | Branches | Merge | Promotion | Environments |
//! | --- | --- | --- | --- | --- |
//! | `standard` | `dev` → `main` | squash | merge commit, manual | none: the green merge is the dev delivery |
//! | `product` | `quartio` → `main` | merge | fast-forward, manual | `dev-vm` after integration, `rollout` after production |
//! | `studio` | `quartio` → `main` | merge | fast-forward, **automatic** | none; checks not required |
//!
//! Each task moves through [`DeliveryStage`]s:
//! `none → pr_open → merged_integration → env_done(name)… → promoted → env_done(name)… →
//! production_done`. [`next_actions`] lists what may happen now and [`apply`] validates one
//! transition against the profile and the observed [`DeliveryFacts`] (GitHub state as seen by
//! the caller). Merges, promotions and environment runs are user actions; the only policy
//! action is promotion under `auto_promote` (GIT-002). Environment runs need an explicit
//! confirmation on every run (DLV-003).

use std::fmt;

/// Longest branch name accepted in a profile.
pub const MAX_BRANCH_BYTES: usize = 200;
/// Longest environment name.
pub const MAX_ENVIRONMENT_NAME_BYTES: usize = 32;
/// Longest environment command.
pub const MAX_COMMAND_BYTES: usize = 2_000;
/// Most environments per profile.
pub const MAX_ENVIRONMENTS: usize = 8;

/// Named profile presets (DLV-002..004). `Custom` marks a profile that differs from every
/// preset's defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    /// `dev` → `main`; new projects (DLV-002).
    Standard,
    /// Quartio Product: `quartio` → dev VM → `main` → rollout (DLV-003).
    Product,
    /// Quartio Studio: `quartio` → `main` immediately (DLV-004).
    Studio,
    /// Anything else.
    Custom,
}

/// How a task pull request is merged into the integration branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeMethod {
    /// Squash merge.
    Squash,
    /// Merge commit.
    Merge,
    /// Rebase merge.
    Rebase,
}

/// How the integration branch is promoted to the production branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromoteMethod {
    /// A promotion pull request merged with a merge commit.
    MergeCommit,
    /// Fast-forward the production branch to the integration branch.
    FastForward,
}

/// The stage after which an environment runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// After the change is merged into the integration branch.
    Integration,
    /// After the integration branch is promoted to production.
    Production,
}

/// How an environment run is verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verify {
    /// A person confirms the environment works.
    Manual,
    /// The environment command's success is the verification.
    Command,
}

/// A deployment/verification step after a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// Lowercase name (`[a-z0-9][a-z0-9_-]*`, ≤ 32 bytes), unique in the profile.
    pub name: String,
    /// The stage it follows.
    pub after: Stage,
    /// Deploy command (opaque; run by the caller in the sandbox). `None` = a manual checkpoint
    /// whose deployment happens outside Yulius.
    pub command: Option<String>,
    /// Verification method.
    pub verify: Verify,
}

/// A sandbox's delivery profile (DLV-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryProfile {
    /// The preset this profile equals, or [`Preset::Custom`].
    pub preset: Preset,
    /// Branch that receives task pull requests.
    pub integration_branch: String,
    /// Branch that represents production.
    pub production_branch: String,
    /// Merge method for task pull requests.
    pub merge: MergeMethod,
    /// Promotion method.
    pub promote: PromoteMethod,
    /// Promote automatically once the change is merged and integration environments are done.
    pub auto_promote: bool,
    /// Whether merges wait for green checks. Studio has no required checks.
    pub require_checks: bool,
    /// Environments in execution order within each stage.
    pub environments: Vec<Environment>,
}

/// An invalid delivery profile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// A branch name Git would reject (or that is unsafe to pass around).
    #[error("invalid branch name '{branch}': {reason}")]
    InvalidBranch {
        /// The rejected name.
        branch: String,
        /// Why.
        reason: &'static str,
    },
    /// Integration and production are the same branch.
    #[error("integration and production branches must differ (both are '{0}')")]
    SameBranches(String),
    /// Environment name not `[a-z0-9][a-z0-9_-]*` or too long.
    #[error(
        "invalid environment name '{0}': use 1-32 lowercase letters, digits, '-' or '_', starting with a letter or digit"
    )]
    InvalidEnvironmentName(String),
    /// Two environments share a name.
    #[error("environment '{0}' is defined twice")]
    DuplicateEnvironment(String),
    /// `verify = command` without a command.
    #[error("environment '{0}' is verified by its command but has no command")]
    CommandVerifyWithoutCommand(String),
    /// Empty, multi-line or oversized command.
    #[error("environment '{0}' has an empty, multi-line or overlong command")]
    InvalidCommand(String),
    /// More than [`MAX_ENVIRONMENTS`].
    #[error("a profile may define at most {MAX_ENVIRONMENTS} environments")]
    TooManyEnvironments,
}

impl Preset {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Product => "product",
            Self::Studio => "studio",
            Self::Custom => "custom",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "standard" => Some(Self::Standard),
            "product" => Some(Self::Product),
            "studio" => Some(Self::Studio),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

impl MergeMethod {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Squash => "squash",
            Self::Merge => "merge",
            Self::Rebase => "rebase",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "squash" => Some(Self::Squash),
            "merge" => Some(Self::Merge),
            "rebase" => Some(Self::Rebase),
            _ => None,
        }
    }
}

impl PromoteMethod {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MergeCommit => "merge_commit",
            Self::FastForward => "fast_forward",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "merge_commit" => Some(Self::MergeCommit),
            "fast_forward" => Some(Self::FastForward),
            _ => None,
        }
    }
}

impl Stage {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Integration => "integration",
            Self::Production => "production",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "integration" => Some(Self::Integration),
            "production" => Some(Self::Production),
            _ => None,
        }
    }
}

impl Verify {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Command => "command",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "manual" => Some(Self::Manual),
            "command" => Some(Self::Command),
            _ => None,
        }
    }
}

fn manual_checkpoint(name: &str, after: Stage) -> Environment {
    Environment {
        name: name.to_owned(),
        after,
        command: None,
        verify: Verify::Manual,
    }
}

impl DeliveryProfile {
    /// Standard: PR (squash) into `dev` on green checks, merge-commit promotion PR
    /// `dev` → `main`. No environment: the merge on green is the dev delivery (DLV-002, DLV-009).
    #[must_use]
    pub fn standard() -> Self {
        Self {
            preset: Preset::Standard,
            integration_branch: "dev".to_owned(),
            production_branch: "main".to_owned(),
            merge: MergeMethod::Squash,
            promote: PromoteMethod::MergeCommit,
            auto_promote: false,
            require_checks: true,
            environments: Vec::new(),
        }
    }

    /// Whether `env` is the manual `dev` checkpoint the standard preset carried before DLV-009.
    #[must_use]
    pub fn is_legacy_standard_checkpoint(env: &Environment) -> bool {
        *env == manual_checkpoint("dev", Stage::Integration)
    }

    /// Product: PR (merge) into `quartio`, explicit `dev-vm` deployment and verification,
    /// fast-forward `main` by a person, explicit `rollout` (DLV-003).
    #[must_use]
    pub fn product() -> Self {
        Self {
            preset: Preset::Product,
            integration_branch: "quartio".to_owned(),
            production_branch: "main".to_owned(),
            merge: MergeMethod::Merge,
            promote: PromoteMethod::FastForward,
            auto_promote: false,
            require_checks: true,
            environments: vec![
                manual_checkpoint("dev-vm", Stage::Integration),
                manual_checkpoint("rollout", Stage::Production),
            ],
        }
    }

    /// Studio: PR (merge) into `quartio`, then `main` fast-forwards automatically; no required
    /// checks and no environments (DLV-004).
    #[must_use]
    pub fn studio() -> Self {
        Self {
            preset: Preset::Studio,
            integration_branch: "quartio".to_owned(),
            production_branch: "main".to_owned(),
            merge: MergeMethod::Merge,
            promote: PromoteMethod::FastForward,
            auto_promote: true,
            require_checks: false,
            environments: Vec::new(),
        }
    }

    /// The defaults of `preset`; `Custom` starts from the standard settings.
    #[must_use]
    pub fn for_preset(preset: Preset) -> Self {
        match preset {
            Preset::Standard => Self::standard(),
            Preset::Product => Self::product(),
            Preset::Studio => Self::studio(),
            Preset::Custom => Self {
                preset: Preset::Custom,
                ..Self::standard()
            },
        }
    }

    /// Relabels the profile: keeps a named preset only while every setting equals that
    /// preset's defaults, otherwise [`Preset::Custom`]. Use after applying overrides, so a
    /// Studio profile with `auto_promote = false` is reported as custom (DLV-005).
    #[must_use]
    pub fn relabeled(mut self) -> Self {
        if self.preset != Preset::Custom && self != Self::for_preset(self.preset) {
            self.preset = Preset::Custom;
        }
        self
    }

    /// Checks branch names, environment names and commands.
    ///
    /// # Errors
    /// Returns the first [`ProfileError`] found.
    pub fn validate(&self) -> Result<(), ProfileError> {
        validate_branch(&self.integration_branch)?;
        validate_branch(&self.production_branch)?;
        if self.integration_branch == self.production_branch {
            return Err(ProfileError::SameBranches(self.production_branch.clone()));
        }
        if self.environments.len() > MAX_ENVIRONMENTS {
            return Err(ProfileError::TooManyEnvironments);
        }
        for (index, environment) in self.environments.iter().enumerate() {
            let name = &environment.name;
            let mut bytes = name.bytes();
            let valid_name = name.len() <= MAX_ENVIRONMENT_NAME_BYTES
                && bytes
                    .next()
                    .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                && bytes.all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                });
            if !valid_name {
                return Err(ProfileError::InvalidEnvironmentName(name.clone()));
            }
            if self.environments[..index]
                .iter()
                .any(|other| other.name == *name)
            {
                return Err(ProfileError::DuplicateEnvironment(name.clone()));
            }
            match &environment.command {
                Some(command)
                    if command.trim().is_empty()
                        || command.len() > MAX_COMMAND_BYTES
                        || command.contains(['\n', '\r', '\0']) =>
                {
                    return Err(ProfileError::InvalidCommand(name.clone()));
                }
                None if environment.verify == Verify::Command => {
                    return Err(ProfileError::CommandVerifyWithoutCommand(name.clone()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The named environment.
    #[must_use]
    pub fn environment(&self, name: &str) -> Option<&Environment> {
        self.environments.iter().find(|env| env.name == name)
    }

    /// Environments of one stage, in order.
    pub fn environments_after(&self, stage: Stage) -> impl Iterator<Item = &Environment> {
        self.environments
            .iter()
            .filter(move |env| env.after == stage)
    }
}

/// Branches task worktrees must never push to (for the local pre-push guard, DSC-007):
/// the integration and production branches, deduplicated, in that order.
#[must_use]
pub fn protected_branches(profile: &DeliveryProfile) -> Vec<String> {
    let mut branches = vec![profile.integration_branch.clone()];
    if profile.production_branch != profile.integration_branch {
        branches.push(profile.production_branch.clone());
    }
    branches
}

fn validate_branch(branch: &str) -> Result<(), ProfileError> {
    let invalid = |reason| {
        Err(ProfileError::InvalidBranch {
            branch: branch.to_owned(),
            reason,
        })
    };
    if branch.is_empty() {
        return invalid("empty");
    }
    if branch.len() > MAX_BRANCH_BYTES {
        return invalid("longer than 200 bytes");
    }
    if branch
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
    {
        return invalid("contains whitespace, a control character or one of ~^:?*[\\");
    }
    if branch.starts_with('-') || branch.starts_with('/') || branch.ends_with('/') {
        return invalid("starts with '-' or '/', or ends with '/'");
    }
    if branch.ends_with('.') || branch.ends_with(".lock") || branch == "@" {
        return invalid("ends with '.' or '.lock', or is '@'");
    }
    if branch.contains("..") || branch.contains("//") || branch.contains("@{") {
        return invalid("contains '..', '//' or '@{'");
    }
    if branch.split('/').any(|part| part.starts_with('.')) {
        return invalid("a path component starts with '.'");
    }
    Ok(())
}

/// Where a task's change is in the delivery path (SPEC-002 §6.7).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeliveryStage {
    /// No pull request yet.
    None,
    /// A pull request into the integration branch is open.
    PrOpen,
    /// The pull request is merged into the integration branch.
    MergedIntegration,
    /// The named environment (integration or production) is done and verified.
    EnvDone(String),
    /// The integration branch was promoted; production environments remain.
    Promoted,
    /// Delivery finished: promoted and every production environment done.
    ProductionDone,
}

impl DeliveryStage {
    /// API wire name (`env_done` without the environment name).
    #[must_use]
    pub fn wire_name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PrOpen => "pr_open",
            Self::MergedIntegration => "merged_integration",
            Self::EnvDone(_) => "env_done",
            Self::Promoted => "promoted",
            Self::ProductionDone => "production_done",
        }
    }

    /// Persisted form: the wire name, or `env_done:<name>`.
    #[must_use]
    pub fn encode(&self) -> String {
        match self {
            Self::EnvDone(name) => format!("env_done:{name}"),
            other => other.wire_name().to_owned(),
        }
    }

    /// Parses [`DeliveryStage::encode`] output.
    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        match text {
            "none" => Some(Self::None),
            "pr_open" => Some(Self::PrOpen),
            "merged_integration" => Some(Self::MergedIntegration),
            "promoted" => Some(Self::Promoted),
            "production_done" => Some(Self::ProductionDone),
            other => other
                .strip_prefix("env_done:")
                .filter(|name| !name.is_empty())
                .map(|name| Self::EnvDone(name.to_owned())),
        }
    }
}

impl fmt::Display for DeliveryStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

/// Combined CI check state of the pull request head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecksState {
    /// No checks reported (the repository has no CI).
    None,
    /// At least one check is still running and none failed.
    Pending,
    /// All checks passed.
    Success,
    /// At least one check failed.
    Failure,
    /// The checks cannot be read (GitHub refused, or the App lacks the permission for the
    /// sources that could report them). Never counts as passed: gates that require checks wait.
    Unknown,
}

impl ChecksState {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Unknown => "unknown",
        }
    }
}

/// The task pull request as GitHub reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrState {
    /// No pull request exists.
    Absent,
    /// Open.
    Open,
    /// Merged.
    Merged,
    /// Closed without merging.
    Closed,
}

impl PrState {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Open => "open",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// Observations the caller gathers before deciding (spec validator, steps, GitHub).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryFacts {
    /// The task spec exists and validates.
    pub spec_valid: bool,
    /// Steps neither done nor skipped.
    pub steps_open: usize,
    /// Checks on the pull request head.
    pub checks: ChecksState,
    /// Pull request state.
    pub pr_state: PrState,
    /// The pull request head is the commit the checks and spec validation refer to (stale
    /// heads are refused, GIT-002).
    pub head_current: bool,
    /// The production branch has commits the integration branch lacks, so a fast-forward is
    /// impossible.
    pub production_diverged: bool,
}

/// Who requests a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Initiator {
    /// A person with write access, through the UI or API.
    User,
    /// Yulius acting on an explicit profile policy (`auto_promote`) or recording an
    /// environment command's result. Never derived from model text.
    Policy,
}

/// Evidence that an environment is verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentEvidence {
    /// A person confirmed it works.
    UserConfirmed,
    /// The environment command exited successfully.
    CommandSucceeded,
}

/// A requested delivery transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryAction {
    /// Open the task pull request into the integration branch.
    OpenPr,
    /// Merge the pull request with the profile's merge method.
    Merge,
    /// Start the environment's command; `confirmed` is the explicit per-run confirmation.
    /// Authorization only: the stage is unchanged until the environment completes.
    RunEnvironment {
        /// Environment name.
        name: String,
        /// The user explicitly confirmed this run.
        confirmed: bool,
    },
    /// Record the environment as done and verified.
    CompleteEnvironment {
        /// Environment name.
        name: String,
        /// How it was verified.
        evidence: EnvironmentEvidence,
    },
    /// Promote the integration branch to production.
    Promote,
}

/// An action available now, as listed by [`next_actions`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NextAction {
    /// Open the pull request.
    OpenPr,
    /// Merge the pull request.
    Merge,
    /// Run the environment command (needs explicit confirmation).
    RunEnvironment(String),
    /// Confirm the environment manually.
    ConfirmEnvironment(String),
    /// Promote; `automatic` when the profile promotes without a user action.
    Promote {
        /// Yulius performs it itself (`auto_promote`).
        automatic: bool,
    },
}

impl NextAction {
    /// Wire form: `open_pr`, `merge`, `run_env:<name>`, `confirm_env:<name>`, `promote`.
    #[must_use]
    pub fn wire(&self) -> String {
        match self {
            Self::OpenPr => "open_pr".to_owned(),
            Self::Merge => "merge".to_owned(),
            Self::RunEnvironment(name) => format!("run_env:{name}"),
            Self::ConfirmEnvironment(name) => format!("confirm_env:{name}"),
            Self::Promote { .. } => "promote".to_owned(),
        }
    }

    /// The action to [`apply`] for this entry (environment runs carry `confirmed: true`).
    #[must_use]
    pub fn action(&self) -> DeliveryAction {
        match self {
            Self::OpenPr => DeliveryAction::OpenPr,
            Self::Merge => DeliveryAction::Merge,
            Self::RunEnvironment(name) => DeliveryAction::RunEnvironment {
                name: name.clone(),
                confirmed: true,
            },
            Self::ConfirmEnvironment(name) => DeliveryAction::CompleteEnvironment {
                name: name.clone(),
                evidence: EnvironmentEvidence::UserConfirmed,
            },
            Self::Promote { .. } => DeliveryAction::Promote,
        }
    }

    fn initiator(&self) -> Initiator {
        match self {
            Self::Promote { automatic: true } => Initiator::Policy,
            _ => Initiator::User,
        }
    }
}

/// A refused delivery transition. Messages are stable and user-facing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryError {
    /// The action does not apply at this stage.
    #[error("'{action}' is not possible at delivery stage '{stage}'")]
    NotAvailable {
        /// Requested action.
        action: &'static str,
        /// Current stage (encoded).
        stage: String,
    },
    /// Missing or invalid spec.
    #[error("the task spec is missing or invalid; fix it (spec_check) first")]
    SpecInvalid,
    /// Steps remain open.
    #[error("{0} step(s) are still open; finish or skip them before merging")]
    StepsOpen(usize),
    /// Checks failed.
    #[error("checks failed on the pull request")]
    ChecksFailing,
    /// Checks still running.
    #[error("checks are still running on the pull request")]
    ChecksPending,
    /// Checks cannot be read, so they do not count as passed.
    #[error("the pull request's checks can't be read, so they don't count as passed")]
    ChecksUnknown,
    /// The pull request is not in the required state.
    #[error("the pull request is {actual}; expected {expected}")]
    PullRequestState {
        /// Required state.
        expected: &'static str,
        /// Observed state.
        actual: &'static str,
    },
    /// The pull request head moved after it was checked.
    #[error("the pull request head changed since it was checked; refresh and try again")]
    StaleHead,
    /// Merges, promotions, environment runs and manual confirmations need a person.
    #[error("this action needs an explicit user action")]
    UserActionRequired,
    /// Policy promotion while `auto_promote` is off.
    #[error("automatic promotion is disabled in this delivery profile")]
    AutoPromotionDisabled,
    /// The environment run was not explicitly confirmed.
    #[error("running environment '{0}' requires explicit confirmation")]
    ConfirmationRequired(String),
    /// The environment is not in the profile.
    #[error("environment '{0}' is not defined in the delivery profile")]
    UnknownEnvironment(String),
    /// Another environment comes first.
    #[error("environment '{requested}' is not next; next is {expected}")]
    EnvironmentOutOfOrder {
        /// Requested environment.
        requested: String,
        /// The environment due now (quoted), or "none".
        expected: String,
    },
    /// The environment has no command to run.
    #[error("environment '{0}' has no command; confirm it manually after deploying")]
    NoCommand(String),
    /// Evidence does not match the environment's verification method.
    #[error("environment '{name}' is verified by {verify}")]
    WrongEvidence {
        /// Environment name.
        name: String,
        /// Expected verification (`a person` or `its command`).
        verify: &'static str,
    },
    /// Fast-forward impossible.
    #[error("'{production}' has diverged from '{integration}'; fast-forward is impossible")]
    ProductionDiverged {
        /// Production branch.
        production: String,
        /// Integration branch.
        integration: String,
    },
}

/// Position of a stage within a profile's path.
enum Position<'p> {
    NoPr,
    PrOpen,
    /// Merged into integration; `next` integration environment, or ready to promote.
    Integration(Option<&'p Environment>),
    /// Promoted; `next` production environment, or done.
    Production(Option<&'p Environment>),
    Done,
    /// The stage names an environment missing from the profile.
    Unknown,
}

fn position<'p>(stage: &DeliveryStage, profile: &'p DeliveryProfile) -> Position<'p> {
    match stage {
        DeliveryStage::None => Position::NoPr,
        DeliveryStage::PrOpen => Position::PrOpen,
        DeliveryStage::MergedIntegration => {
            Position::Integration(profile.environments_after(Stage::Integration).next())
        }
        DeliveryStage::Promoted => {
            Position::Production(profile.environments_after(Stage::Production).next())
        }
        DeliveryStage::ProductionDone => Position::Done,
        DeliveryStage::EnvDone(name) => match profile.environment(name) {
            Some(done) => {
                let next = profile
                    .environments_after(done.after)
                    .skip_while(|env| env.name != *name)
                    .nth(1);
                match done.after {
                    Stage::Integration => Position::Integration(next),
                    Stage::Production => Position::Production(next),
                }
            }
            None => Position::Unknown,
        },
    }
}

fn action_name(action: &DeliveryAction) -> &'static str {
    match action {
        DeliveryAction::OpenPr => "open_pr",
        DeliveryAction::Merge => "merge",
        DeliveryAction::RunEnvironment { .. } => "run_env",
        DeliveryAction::CompleteEnvironment { .. } => "confirm_env",
        DeliveryAction::Promote => "promote",
    }
}

/// Validates one transition and returns the resulting stage.
///
/// Enforced: a pull request needs a valid spec; a merge needs a user, an open pull request
/// with a current head, a valid spec, no open steps and — when the profile requires checks —
/// checks that are neither pending nor failing; environments run in order, each run needs an
/// explicit user confirmation, and manual environments need a person's confirmation;
/// promotion needs the merged change with all integration environments done, a user (or
/// `auto_promote` for [`Initiator::Policy`]) and, for fast-forward, an undiverged production
/// branch. [`DeliveryAction::RunEnvironment`] leaves the stage unchanged.
///
/// # Errors
/// Returns the [`DeliveryError`] explaining the refusal.
pub fn apply(
    stage: &DeliveryStage,
    action: &DeliveryAction,
    by: Initiator,
    profile: &DeliveryProfile,
    facts: &DeliveryFacts,
) -> Result<DeliveryStage, DeliveryError> {
    let not_available = || DeliveryError::NotAvailable {
        action: action_name(action),
        stage: stage.encode(),
    };
    match (position(stage, profile), action) {
        (Position::NoPr, DeliveryAction::OpenPr) => {
            if !facts.spec_valid {
                return Err(DeliveryError::SpecInvalid);
            }
            Ok(DeliveryStage::PrOpen)
        }
        (Position::PrOpen, DeliveryAction::Merge) => {
            check_merge(by, profile, facts)?;
            Ok(DeliveryStage::MergedIntegration)
        }
        (
            Position::Integration(next) | Position::Production(next),
            DeliveryAction::RunEnvironment { name, confirmed },
        ) => {
            let environment = due_environment(next, name, profile)?;
            if by != Initiator::User {
                return Err(DeliveryError::UserActionRequired);
            }
            if environment.command.is_none() {
                return Err(DeliveryError::NoCommand(name.clone()));
            }
            if !confirmed {
                return Err(DeliveryError::ConfirmationRequired(name.clone()));
            }
            Ok(stage.clone())
        }
        (
            Position::Integration(next) | Position::Production(next),
            DeliveryAction::CompleteEnvironment { name, evidence },
        ) => {
            let environment = due_environment(next, name, profile)?;
            match (environment.verify, evidence) {
                (Verify::Manual, EnvironmentEvidence::UserConfirmed) => {
                    if by != Initiator::User {
                        return Err(DeliveryError::UserActionRequired);
                    }
                }
                (Verify::Command, EnvironmentEvidence::CommandSucceeded) => {}
                (verify, _) => {
                    return Err(DeliveryError::WrongEvidence {
                        name: name.clone(),
                        verify: match verify {
                            Verify::Manual => "a person",
                            Verify::Command => "its command",
                        },
                    });
                }
            }
            let last_production = environment.after == Stage::Production
                && profile
                    .environments_after(Stage::Production)
                    .last()
                    .is_some_and(|last| last.name == environment.name);
            Ok(if last_production {
                DeliveryStage::ProductionDone
            } else {
                DeliveryStage::EnvDone(name.clone())
            })
        }
        (Position::Integration(None), DeliveryAction::Promote) => {
            check_promote(by, profile, facts)?;
            Ok(
                if profile
                    .environments_after(Stage::Production)
                    .next()
                    .is_some()
                {
                    DeliveryStage::Promoted
                } else {
                    DeliveryStage::ProductionDone
                },
            )
        }
        _ => Err(not_available()),
    }
}

fn due_environment<'p>(
    next: Option<&'p Environment>,
    requested: &str,
    profile: &DeliveryProfile,
) -> Result<&'p Environment, DeliveryError> {
    match next {
        Some(environment) if environment.name == requested => Ok(environment),
        _ if profile.environment(requested).is_none() => {
            Err(DeliveryError::UnknownEnvironment(requested.to_owned()))
        }
        _ => Err(DeliveryError::EnvironmentOutOfOrder {
            requested: requested.to_owned(),
            expected: next.map_or_else(|| "none".to_owned(), |env| format!("'{}'", env.name)),
        }),
    }
}

fn check_merge(
    by: Initiator,
    profile: &DeliveryProfile,
    facts: &DeliveryFacts,
) -> Result<(), DeliveryError> {
    if by != Initiator::User {
        return Err(DeliveryError::UserActionRequired);
    }
    if facts.pr_state != PrState::Open {
        return Err(DeliveryError::PullRequestState {
            expected: "open",
            actual: facts.pr_state.as_str(),
        });
    }
    if !facts.head_current {
        return Err(DeliveryError::StaleHead);
    }
    if !facts.spec_valid {
        return Err(DeliveryError::SpecInvalid);
    }
    if facts.steps_open > 0 {
        return Err(DeliveryError::StepsOpen(facts.steps_open));
    }
    if profile.require_checks {
        match facts.checks {
            ChecksState::Failure => return Err(DeliveryError::ChecksFailing),
            ChecksState::Pending => return Err(DeliveryError::ChecksPending),
            ChecksState::Unknown => return Err(DeliveryError::ChecksUnknown),
            ChecksState::None | ChecksState::Success => {}
        }
    }
    Ok(())
}

fn check_promote(
    by: Initiator,
    profile: &DeliveryProfile,
    facts: &DeliveryFacts,
) -> Result<(), DeliveryError> {
    if by == Initiator::Policy && !profile.auto_promote {
        return Err(DeliveryError::AutoPromotionDisabled);
    }
    if facts.pr_state != PrState::Merged {
        return Err(DeliveryError::PullRequestState {
            expected: "merged",
            actual: facts.pr_state.as_str(),
        });
    }
    if profile.promote == PromoteMethod::FastForward && facts.production_diverged {
        return Err(DeliveryError::ProductionDiverged {
            production: profile.production_branch.clone(),
            integration: profile.integration_branch.clone(),
        });
    }
    Ok(())
}

fn candidates(stage: &DeliveryStage, profile: &DeliveryProfile) -> Vec<NextAction> {
    let environment_actions = |environment: &Environment| {
        let mut actions = Vec::new();
        if environment.command.is_some() {
            actions.push(NextAction::RunEnvironment(environment.name.clone()));
        }
        if environment.verify == Verify::Manual {
            actions.push(NextAction::ConfirmEnvironment(environment.name.clone()));
        }
        actions
    };
    match position(stage, profile) {
        Position::NoPr => vec![NextAction::OpenPr],
        Position::PrOpen => vec![NextAction::Merge],
        Position::Integration(Some(environment)) | Position::Production(Some(environment)) => {
            environment_actions(environment)
        }
        Position::Integration(None) => vec![NextAction::Promote {
            automatic: profile.auto_promote,
        }],
        Position::Production(None) | Position::Done | Position::Unknown => Vec::new(),
    }
}

/// Actions that [`apply`] would accept now. Automatic promotion is listed as
/// `Promote { automatic: true }`; the caller performs it with [`Initiator::Policy`].
/// A stage naming an environment missing from the profile yields no actions.
#[must_use]
pub fn next_actions(
    stage: &DeliveryStage,
    profile: &DeliveryProfile,
    facts: &DeliveryFacts,
) -> Vec<NextAction> {
    candidates(stage, profile)
        .into_iter()
        .filter(|next| apply(stage, &next.action(), next.initiator(), profile, facts).is_ok())
        .collect()
}

/// Actions that apply at this stage but are refused now, with the reason (for disabled
/// controls and explanations).
#[must_use]
pub fn blocked_actions(
    stage: &DeliveryStage,
    profile: &DeliveryProfile,
    facts: &DeliveryFacts,
) -> Vec<(NextAction, DeliveryError)> {
    candidates(stage, profile)
        .into_iter()
        .filter_map(|next| {
            apply(stage, &next.action(), next.initiator(), profile, facts)
                .err()
                .map(|error| (next, error))
        })
        .collect()
}

/// Aligns a recorded stage with GitHub after out-of-band changes: a pull request opened,
/// merged or closed outside Yulius.
#[must_use]
pub fn reconcile(stage: &DeliveryStage, facts: &DeliveryFacts) -> DeliveryStage {
    match (stage, facts.pr_state) {
        (DeliveryStage::None, PrState::Open) => DeliveryStage::PrOpen,
        (DeliveryStage::None | DeliveryStage::PrOpen, PrState::Merged) => {
            DeliveryStage::MergedIntegration
        }
        (DeliveryStage::PrOpen, PrState::Closed | PrState::Absent) => DeliveryStage::None,
        (other, _) => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn facts() -> DeliveryFacts {
        DeliveryFacts {
            spec_valid: true,
            steps_open: 0,
            checks: ChecksState::Success,
            pr_state: PrState::Absent,
            head_current: true,
            production_diverged: false,
        }
    }

    fn with_pr(state: PrState) -> DeliveryFacts {
        DeliveryFacts {
            pr_state: state,
            ..facts()
        }
    }

    fn user(
        stage: &DeliveryStage,
        action: &DeliveryAction,
        profile: &DeliveryProfile,
        facts: &DeliveryFacts,
    ) -> Result<DeliveryStage, DeliveryError> {
        apply(stage, action, Initiator::User, profile, facts)
    }

    fn confirm(name: &str) -> DeliveryAction {
        DeliveryAction::CompleteEnvironment {
            name: name.to_owned(),
            evidence: EnvironmentEvidence::UserConfirmed,
        }
    }

    fn run(name: &str, confirmed: bool) -> DeliveryAction {
        DeliveryAction::RunEnvironment {
            name: name.to_owned(),
            confirmed,
        }
    }

    #[test]
    fn presets_match_the_specified_paths() -> TestResult {
        let standard = DeliveryProfile::standard();
        assert_eq!(
            (
                standard.integration_branch.as_str(),
                standard.production_branch.as_str()
            ),
            ("dev", "main")
        );
        assert_eq!(standard.merge, MergeMethod::Squash);
        assert_eq!(standard.promote, PromoteMethod::MergeCommit);
        assert!(!standard.auto_promote && standard.require_checks);
        // DLV-009: the green merge is the dev delivery; no person confirms it.
        assert!(standard.environments.is_empty());

        let product = DeliveryProfile::product();
        assert_eq!(product.integration_branch, "quartio");
        assert_eq!(
            (product.merge, product.promote),
            (MergeMethod::Merge, PromoteMethod::FastForward)
        );
        assert!(!product.auto_promote);
        let names: Vec<(&str, Stage)> = product
            .environments
            .iter()
            .map(|env| (env.name.as_str(), env.after))
            .collect();
        assert_eq!(
            names,
            vec![
                ("dev-vm", Stage::Integration),
                ("rollout", Stage::Production)
            ]
        );

        let studio = DeliveryProfile::studio();
        assert_eq!(studio.integration_branch, "quartio");
        assert_eq!(studio.promote, PromoteMethod::FastForward);
        assert!(studio.auto_promote && !studio.require_checks && studio.environments.is_empty());

        for preset in [
            Preset::Standard,
            Preset::Product,
            Preset::Studio,
            Preset::Custom,
        ] {
            let profile = DeliveryProfile::for_preset(preset);
            profile.validate()?;
            assert_eq!(profile.preset, preset);
            assert_eq!(profile.clone().relabeled(), profile);
            assert_eq!(Preset::parse(preset.as_str()), Some(preset));
        }
        Ok(())
    }

    #[test]
    fn protected_branches_follow_the_profile() {
        assert_eq!(
            protected_branches(&DeliveryProfile::standard()),
            vec!["dev", "main"]
        );
        assert_eq!(
            protected_branches(&DeliveryProfile::studio()),
            vec!["quartio", "main"]
        );
    }

    #[test]
    fn standard_flow_ac_dlv_1() -> TestResult {
        let profile = DeliveryProfile::standard();
        let mut stage = DeliveryStage::None;
        assert_eq!(
            next_actions(&stage, &profile, &facts()),
            vec![NextAction::OpenPr]
        );
        let invalid_spec = DeliveryFacts {
            spec_valid: false,
            ..facts()
        };
        assert_eq!(
            user(&stage, &DeliveryAction::OpenPr, &profile, &invalid_spec),
            Err(DeliveryError::SpecInvalid)
        );
        assert!(next_actions(&stage, &profile, &invalid_spec).is_empty());
        stage = user(&stage, &DeliveryAction::OpenPr, &profile, &facts())?;
        assert_eq!(stage, DeliveryStage::PrOpen);

        // Merge waits for green checks; checks that can't be read are not green (#111).
        let open = with_pr(PrState::Open);
        for (checks, error) in [
            (ChecksState::Pending, DeliveryError::ChecksPending),
            (ChecksState::Failure, DeliveryError::ChecksFailing),
            (ChecksState::Unknown, DeliveryError::ChecksUnknown),
        ] {
            let observed = DeliveryFacts {
                checks,
                ..open.clone()
            };
            assert_eq!(
                user(&stage, &DeliveryAction::Merge, &profile, &observed),
                Err(error)
            );
            assert!(next_actions(&stage, &profile, &observed).is_empty());
        }
        assert_eq!(
            next_actions(&stage, &profile, &open),
            vec![NextAction::Merge]
        );
        stage = user(&stage, &DeliveryAction::Merge, &profile, &open)?;
        assert_eq!(stage, DeliveryStage::MergedIntegration);

        // DLV-009: nothing waits for a person between the merge and promotion.
        let merged = with_pr(PrState::Merged);
        assert_eq!(
            next_actions(&stage, &profile, &merged),
            vec![NextAction::Promote { automatic: false }]
        );
        assert_eq!(
            apply(
                &stage,
                &DeliveryAction::Promote,
                Initiator::Policy,
                &profile,
                &merged
            ),
            Err(DeliveryError::AutoPromotionDisabled)
        );
        stage = user(&stage, &DeliveryAction::Promote, &profile, &merged)?;
        assert_eq!(stage, DeliveryStage::ProductionDone);
        assert!(next_actions(&stage, &profile, &merged).is_empty());
        Ok(())
    }

    #[test]
    fn merge_gates() -> TestResult {
        let profile = DeliveryProfile::standard();
        let stage = DeliveryStage::PrOpen;
        let open = with_pr(PrState::Open);
        assert_eq!(
            apply(
                &stage,
                &DeliveryAction::Merge,
                Initiator::Policy,
                &profile,
                &open
            ),
            Err(DeliveryError::UserActionRequired)
        );
        let cases = [
            (
                with_pr(PrState::Closed),
                DeliveryError::PullRequestState {
                    expected: "open",
                    actual: "closed",
                },
            ),
            (
                DeliveryFacts {
                    head_current: false,
                    ..open.clone()
                },
                DeliveryError::StaleHead,
            ),
            (
                DeliveryFacts {
                    spec_valid: false,
                    ..open.clone()
                },
                DeliveryError::SpecInvalid,
            ),
            (
                DeliveryFacts {
                    steps_open: 2,
                    ..open.clone()
                },
                DeliveryError::StepsOpen(2),
            ),
        ];
        for (observed, error) in cases {
            assert_eq!(
                user(&stage, &DeliveryAction::Merge, &profile, &observed),
                Err(error.clone())
            );
            let blocked = blocked_actions(&stage, &profile, &observed);
            assert_eq!(blocked, vec![(NextAction::Merge, error)]);
        }
        // No CI at all: nothing can fail, so the merge may proceed.
        let no_ci = DeliveryFacts {
            checks: ChecksState::None,
            ..open
        };
        assert_eq!(
            user(&stage, &DeliveryAction::Merge, &profile, &no_ci)?,
            DeliveryStage::MergedIntegration
        );
        // Actions from other stages are refused with the stage named.
        let refused = user(
            &DeliveryStage::None,
            &DeliveryAction::Merge,
            &profile,
            &no_ci,
        );
        assert_eq!(
            refused.map_err(|e| e.to_string()),
            Err("'merge' is not possible at delivery stage 'none'".to_owned())
        );
        Ok(())
    }

    #[test]
    fn product_flow_ac_dlv_2() -> TestResult {
        let mut profile = DeliveryProfile::product();
        let open = with_pr(PrState::Open);
        let merged = with_pr(PrState::Merged);
        let mut stage = user(
            &DeliveryStage::PrOpen,
            &DeliveryAction::Merge,
            &profile,
            &open,
        )?;
        assert_eq!(stage, DeliveryStage::MergedIntegration);

        // Preset environments have no command: a manual checkpoint only.
        assert_eq!(
            next_actions(&stage, &profile, &merged),
            vec![NextAction::ConfirmEnvironment("dev-vm".to_owned())]
        );
        assert_eq!(
            user(&stage, &run("dev-vm", true), &profile, &merged),
            Err(DeliveryError::NoCommand("dev-vm".to_owned()))
        );

        // With a configured deploy command the run needs explicit confirmation by a person.
        profile.environments[0].command = Some("node scripts/deploy.mjs dev-vm".to_owned());
        let profile = profile.relabeled();
        assert_eq!(profile.preset, Preset::Custom);
        assert_eq!(
            next_actions(&stage, &profile, &merged),
            vec![
                NextAction::RunEnvironment("dev-vm".to_owned()),
                NextAction::ConfirmEnvironment("dev-vm".to_owned())
            ]
        );
        assert_eq!(
            user(&stage, &run("dev-vm", false), &profile, &merged),
            Err(DeliveryError::ConfirmationRequired("dev-vm".to_owned()))
        );
        assert_eq!(
            apply(
                &stage,
                &run("dev-vm", true),
                Initiator::Policy,
                &profile,
                &merged
            ),
            Err(DeliveryError::UserActionRequired)
        );
        assert_eq!(
            user(&stage, &run("dev-vm", true), &profile, &merged)?,
            stage
        );
        // Out-of-order and unknown environments are refused.
        assert!(matches!(
            user(&stage, &confirm("rollout"), &profile, &merged),
            Err(DeliveryError::EnvironmentOutOfOrder { .. })
        ));
        assert_eq!(
            user(&stage, &confirm("staging"), &profile, &merged),
            Err(DeliveryError::UnknownEnvironment("staging".to_owned()))
        );
        // Manual verification needs a person, not a command result or policy.
        assert!(matches!(
            apply(
                &stage,
                &DeliveryAction::CompleteEnvironment {
                    name: "dev-vm".to_owned(),
                    evidence: EnvironmentEvidence::CommandSucceeded
                },
                Initiator::Policy,
                &profile,
                &merged
            ),
            Err(DeliveryError::WrongEvidence {
                verify: "a person",
                ..
            })
        ));
        assert_eq!(
            apply(
                &stage,
                &confirm("dev-vm"),
                Initiator::Policy,
                &profile,
                &merged
            ),
            Err(DeliveryError::UserActionRequired)
        );
        stage = user(&stage, &confirm("dev-vm"), &profile, &merged)?;

        // Fast-forward promotion by a person, refused when main diverged.
        let diverged = DeliveryFacts {
            production_diverged: true,
            ..merged.clone()
        };
        assert!(matches!(
            user(&stage, &DeliveryAction::Promote, &profile, &diverged),
            Err(DeliveryError::ProductionDiverged { .. })
        ));
        stage = user(&stage, &DeliveryAction::Promote, &profile, &merged)?;
        assert_eq!(stage, DeliveryStage::Promoted);

        // Rollout after production, then done.
        assert_eq!(
            next_actions(&stage, &profile, &merged),
            vec![NextAction::ConfirmEnvironment("rollout".to_owned())]
        );
        stage = user(&stage, &confirm("rollout"), &profile, &merged)?;
        assert_eq!(stage, DeliveryStage::ProductionDone);
        Ok(())
    }

    #[test]
    fn command_verified_environment_completes_from_command_result() -> TestResult {
        let mut profile = DeliveryProfile::standard();
        profile
            .environments
            .push(manual_checkpoint("dev", Stage::Integration));
        profile.environments[0].command = Some("make deploy-dev".to_owned());
        profile.environments[0].verify = Verify::Command;
        profile.validate()?;
        let stage = DeliveryStage::MergedIntegration;
        let merged = with_pr(PrState::Merged);
        assert_eq!(
            next_actions(&stage, &profile, &merged),
            vec![NextAction::RunEnvironment("dev".to_owned())]
        );
        let succeeded = DeliveryAction::CompleteEnvironment {
            name: "dev".to_owned(),
            evidence: EnvironmentEvidence::CommandSucceeded,
        };
        assert_eq!(
            apply(&stage, &succeeded, Initiator::Policy, &profile, &merged)?,
            DeliveryStage::EnvDone("dev".to_owned())
        );
        assert!(matches!(
            user(&stage, &confirm("dev"), &profile, &merged),
            Err(DeliveryError::WrongEvidence {
                verify: "its command",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn studio_flow_ac_dlv_3() -> TestResult {
        let profile = DeliveryProfile::studio();
        // No required checks: pending, failing or unreadable checks do not block the merge.
        for checks in [
            ChecksState::Pending,
            ChecksState::Failure,
            ChecksState::None,
            ChecksState::Unknown,
        ] {
            let observed = DeliveryFacts {
                checks,
                ..with_pr(PrState::Open)
            };
            assert_eq!(
                next_actions(&DeliveryStage::PrOpen, &profile, &observed),
                vec![NextAction::Merge]
            );
        }
        let stage = user(
            &DeliveryStage::PrOpen,
            &DeliveryAction::Merge,
            &profile,
            &with_pr(PrState::Open),
        )?;
        let merged = with_pr(PrState::Merged);
        assert_eq!(
            next_actions(&stage, &profile, &merged),
            vec![NextAction::Promote { automatic: true }]
        );
        let promoted = apply(
            &stage,
            &DeliveryAction::Promote,
            Initiator::Policy,
            &profile,
            &merged,
        )?;
        assert_eq!(promoted, DeliveryStage::ProductionDone);
        Ok(())
    }

    #[test]
    fn changing_studio_to_gated_disables_auto_promotion() -> TestResult {
        let gated = DeliveryProfile {
            auto_promote: false,
            ..DeliveryProfile::studio()
        }
        .relabeled();
        assert_eq!(gated.preset, Preset::Custom);
        let stage = DeliveryStage::MergedIntegration;
        let merged = with_pr(PrState::Merged);
        assert_eq!(
            next_actions(&stage, &gated, &merged),
            vec![NextAction::Promote { automatic: false }]
        );
        assert_eq!(
            apply(
                &stage,
                &DeliveryAction::Promote,
                Initiator::Policy,
                &gated,
                &merged
            ),
            Err(DeliveryError::AutoPromotionDisabled)
        );
        assert_eq!(
            user(&stage, &DeliveryAction::Promote, &gated, &merged)?,
            DeliveryStage::ProductionDone
        );
        // Promotion needs the merged change.
        assert!(matches!(
            user(
                &stage,
                &DeliveryAction::Promote,
                &gated,
                &with_pr(PrState::Open)
            ),
            Err(DeliveryError::PullRequestState {
                expected: "merged",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn stage_encoding_and_wire_names() {
        let stages = [
            DeliveryStage::None,
            DeliveryStage::PrOpen,
            DeliveryStage::MergedIntegration,
            DeliveryStage::EnvDone("dev-vm".to_owned()),
            DeliveryStage::Promoted,
            DeliveryStage::ProductionDone,
        ];
        for stage in stages {
            assert_eq!(DeliveryStage::decode(&stage.encode()), Some(stage.clone()));
        }
        assert_eq!(
            DeliveryStage::EnvDone("x".to_owned()).wire_name(),
            "env_done"
        );
        assert_eq!(DeliveryStage::decode("env_done:"), None);
        assert_eq!(DeliveryStage::decode("merged"), None);
        assert_eq!(
            NextAction::RunEnvironment("dev".to_owned()).wire(),
            "run_env:dev"
        );
        assert_eq!(
            NextAction::ConfirmEnvironment("dev".to_owned()).wire(),
            "confirm_env:dev"
        );
        assert_eq!(NextAction::Promote { automatic: true }.wire(), "promote");
    }

    #[test]
    fn unknown_environment_in_stage_yields_no_actions() {
        let stage = DeliveryStage::EnvDone("removed".to_owned());
        let profile = DeliveryProfile::standard();
        assert!(next_actions(&stage, &profile, &with_pr(PrState::Merged)).is_empty());
    }

    #[test]
    fn reconcile_follows_out_of_band_changes() {
        assert_eq!(
            reconcile(&DeliveryStage::None, &with_pr(PrState::Open)),
            DeliveryStage::PrOpen
        );
        assert_eq!(
            reconcile(&DeliveryStage::PrOpen, &with_pr(PrState::Merged)),
            DeliveryStage::MergedIntegration
        );
        assert_eq!(
            reconcile(&DeliveryStage::PrOpen, &with_pr(PrState::Closed)),
            DeliveryStage::None
        );
        assert_eq!(
            reconcile(&DeliveryStage::Promoted, &with_pr(PrState::Merged)),
            DeliveryStage::Promoted
        );
    }

    #[test]
    fn profile_validation() {
        let mut base = DeliveryProfile::standard();
        base.environments
            .push(manual_checkpoint("dev", Stage::Integration));
        let same = DeliveryProfile {
            production_branch: "dev".to_owned(),
            ..base.clone()
        };
        assert_eq!(
            same.validate(),
            Err(ProfileError::SameBranches("dev".to_owned()))
        );
        for bad in [
            "", "-x", "a..b", "a b", "x.lock", "feat/", "a:b", "@", ".hidden", "a/.b", "a@{1}",
        ] {
            let profile = DeliveryProfile {
                integration_branch: bad.to_owned(),
                ..base.clone()
            };
            assert!(
                matches!(profile.validate(), Err(ProfileError::InvalidBranch { .. })),
                "{bad:?}"
            );
        }
        let mut duplicate = base.clone();
        duplicate
            .environments
            .push(manual_checkpoint("dev", Stage::Production));
        assert_eq!(
            duplicate.validate(),
            Err(ProfileError::DuplicateEnvironment("dev".to_owned()))
        );
        let mut bad_name = base.clone();
        bad_name.environments[0].name = "Dev VM".to_owned();
        assert!(matches!(
            bad_name.validate(),
            Err(ProfileError::InvalidEnvironmentName(_))
        ));
        let mut no_command = base.clone();
        no_command.environments[0].verify = Verify::Command;
        assert_eq!(
            no_command.validate(),
            Err(ProfileError::CommandVerifyWithoutCommand("dev".to_owned()))
        );
        let mut multiline = base.clone();
        multiline.environments[0].command = Some("deploy\nrm -rf /".to_owned());
        assert_eq!(
            multiline.validate(),
            Err(ProfileError::InvalidCommand("dev".to_owned()))
        );
        let mut many = base;
        many.environments = (0..9)
            .map(|i| manual_checkpoint(&format!("e{i}"), Stage::Integration))
            .collect();
        assert_eq!(many.validate(), Err(ProfileError::TooManyEnvironments));
    }
}
