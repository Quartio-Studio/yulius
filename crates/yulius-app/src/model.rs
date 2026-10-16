//! Persistent records exchanged between application services and the store port.
//!
//! Records use domain types for closed sets and identifiers. Secrets are stored sealed
//! ([`Sealed`]); only the application decides when to open them.

use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, AttemptId, MessageId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::permission::AccessLevel;
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

/// Ciphertext produced by the secret box; never logged.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed(pub Vec<u8>);

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sealed({} bytes)", self.0.len())
    }
}

/// Plaintext secret with redacted formatting.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Secret {
    /// The secret text (use only at an I/O boundary).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The singleton instance row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRecord {
    /// GitHub owner of all projects.
    pub owner_login: Option<String>,
    /// `org` or `user`.
    pub owner_kind: Option<String>,
    /// Public base URL.
    pub public_url: String,
    /// Sealed GitHub App credentials JSON.
    pub github_app: Option<Sealed>,
    /// GitHub App installation id on the owner.
    pub installation_id: Option<u64>,
    /// SHA-256 hex of the one-time setup token, while setup is pending.
    pub setup_token_hash: Option<String>,
    /// When the superadmin was created.
    pub setup_completed_at: Option<Timestamp>,
    /// Argon2id PHC string of the superadmin recovery code (ACC-011); `None` when none is set
    /// or recovery is locked.
    pub recovery_hash: Option<String>,
    /// Consecutive wrong recovery codes.
    pub recovery_failures: u32,
    /// When recovery was permanently disabled by too many wrong codes.
    pub recovery_locked_at: Option<Timestamp>,
}

/// A user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    /// Id.
    pub id: UserId,
    /// GitHub login (absent for the superadmin until linked).
    pub github_login: Option<String>,
    /// GitHub numeric id once linked.
    pub github_id: Option<u64>,
    /// Display name.
    pub display_name: String,
    /// Avatar URL.
    pub avatar_url: Option<String>,
    /// Superadmin flag.
    pub is_superadmin: bool,
    /// Instance administrator flag.
    pub is_instance_admin: bool,
    /// Argon2id PHC string (superadmin only).
    pub password_hash: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Disabled time.
    pub disabled_at: Option<Timestamp>,
}

/// Stored GitHub user-to-server token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubTokenRecord {
    /// Owner.
    pub user_id: UserId,
    /// Sealed access token.
    pub access: Sealed,
    /// Sealed refresh token.
    pub refresh: Option<Sealed>,
    /// Access expiry.
    pub expires_at: Option<Timestamp>,
    /// Refresh expiry.
    pub refresh_expires_at: Option<Timestamp>,
}

/// A browser session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSessionRecord {
    /// SHA-256 hex of the cookie value.
    pub id_hash: String,
    /// User.
    pub user_id: UserId,
    /// CSRF token.
    pub csrf_token: String,
    /// Created.
    pub created_at: Timestamp,
    /// Expires.
    pub expires_at: Timestamp,
    /// Revoked.
    pub revoked_at: Option<Timestamp>,
}

/// A CLI API token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiTokenRecord {
    /// Id.
    pub id: String,
    /// User.
    pub user_id: UserId,
    /// SHA-256 hex of the token.
    pub token_hash: String,
    /// Label.
    pub label: String,
    /// Created.
    pub created_at: Timestamp,
    /// Revoked.
    pub revoked_at: Option<Timestamp>,
}

/// Device approval grant for `yulius login`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGrantRecord {
    /// Code the user types/confirms.
    pub user_code: String,
    /// SHA-256 hex of the device code.
    pub device_code_hash: String,
    /// `pending`, `approved`, `denied`, `consumed`.
    pub status: String,
    /// Approving user.
    pub user_id: Option<UserId>,
    /// Sealed issued token until consumed.
    pub token: Option<Sealed>,
    /// The starting client's limit key (bounds live grants per client, #169).
    pub client_key: String,
    /// Created.
    pub created_at: Timestamp,
    /// Expires.
    pub expires_at: Timestamp,
}

/// Bounds on live device grants (#169, ADR-0047): rows not expired and not consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceGrantBounds {
    /// Live grants of one client key.
    pub per_client: usize,
    /// Live grants of the instance.
    pub total: usize,
}

/// Outcome of inserting a device grant under [`DeviceGrantBounds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceGrantInsert {
    /// Stored.
    Inserted,
    /// The client already has its bound of live grants; nothing stored.
    ClientFull,
    /// The instance already has its bound of live grants; nothing stored.
    InstanceFull,
}

/// Sandbox (project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRecord {
    /// Id.
    pub id: SandboxId,
    /// URL slug.
    pub slug: String,
    /// Repository owner.
    pub repo_owner: String,
    /// Repository name.
    pub repo_name: String,
    /// GitHub repository id.
    pub github_repo_id: Option<u64>,
    /// Default branch.
    pub default_branch: String,
    /// Delivery profile JSON (contracts `Profile`).
    pub profile_json: String,
    /// First dev-server port.
    pub port_base: u16,
    /// Port count.
    pub port_count: u16,
    /// Onboarding state wire name.
    pub onboarding_state: String,
    /// Preparation task.
    pub onboarding_task_id: Option<TaskId>,
    /// Creator.
    pub created_by: UserId,
    /// Created.
    pub created_at: Timestamp,
    /// Removed from Yulius.
    pub removed_at: Option<Timestamp>,
}

impl SandboxRecord {
    /// A playground (SPEC-099, ADR-0081): its `origin` is a bare repository on the server,
    /// outside the sandbox (`<data>/playgrounds/<id>.git`), and Yulius never calls GitHub for it. Marked by an empty repository owner, which
    /// a GitHub owner never is.
    #[must_use]
    pub fn is_playground(&self) -> bool {
        self.repo_owner.is_empty()
    }
}

/// Where a task came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskOrigin {
    /// Created in the UI or API.
    Ui,
    /// Created by an automation.
    Automation,
    /// Imported session.
    Import,
    /// Sandbox preparation meta-task.
    Onboarding,
    /// Created by another agent through MCP.
    Agent,
}

impl TaskOrigin {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Automation => "automation",
            Self::Import => "import",
            Self::Onboarding => "onboarding",
            Self::Agent => "agent",
        }
    }

    /// Parses a wire name (unknown → `Ui`).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "automation" => Self::Automation,
            "import" => Self::Import,
            "onboarding" => Self::Onboarding,
            "agent" => Self::Agent,
            _ => Self::Ui,
        }
    }
}

/// How the task prompt reaches the agent (SPEC-001 TSK-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TaskMode {
    /// The prompt becomes the session's goal through the harness's `/goal` command (default
    /// for new tasks).
    #[default]
    Goal,
    /// The prompt is the first message.
    Prompt,
}

impl TaskMode {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Prompt => "prompt",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "goal" => Some(Self::Goal),
            "prompt" => Some(Self::Prompt),
            _ => None,
        }
    }
}

/// A person's request for control of a task, waiting for its owner (SPEC-070).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlRequest {
    /// Who asks.
    pub by: UserId,
    /// When they asked.
    pub at: Timestamp,
}

/// A person's pause of a task's run (SPEC-191 PER-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonPause {
    /// Who paused it.
    pub by: UserId,
    /// When.
    pub at: Timestamp,
    /// The attempt it was taken on: the pause acts on that run only.
    pub attempt: AttemptId,
}

/// Since when a task is in its current state, as a person reads it (SPEC-087 TIM-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskTimes {
    /// The last change of its status or activity (not of anything else, nor a value set again).
    pub state_at: Timestamp,
    /// The start of its current working stretch, in which it has not waited for a person;
    /// `None` while it waits for one, is queued or has ended.
    pub working_since: Option<Timestamp>,
}

/// A task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    /// Id.
    pub id: TaskId,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Number within the sandbox.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Initial prompt.
    pub prompt: String,
    /// Current harness.
    pub harness: Harness,
    /// Model override.
    pub model: Option<String>,
    /// Effort override.
    pub effort: Option<String>,
    /// Workflow.
    pub workflow_id: Option<String>,
    /// Status.
    pub status: TaskStatus,
    /// Activity while running.
    pub activity: Option<Activity>,
    /// Branch.
    pub branch: Option<String>,
    /// Worktree path.
    pub worktree_path: Option<String>,
    /// Registered spec path (repository-relative).
    pub spec_path: Option<String>,
    /// Origin.
    pub origin: TaskOrigin,
    /// Automation run key (`<automation>@<ms>`).
    pub automation_run: Option<String>,
    /// Who asked for the task (the request's author; stays when control passes).
    pub created_by: UserId,
    /// Who owns the task (SPEC-070): it acts on GitHub with their token, and only they change
    /// it. The creator until a hand-over.
    pub owner: UserId,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
    /// Closed.
    pub closed_at: Option<Timestamp>,
    /// Archived.
    pub archived_at: Option<Timestamp>,
    /// When queued (FIFO order).
    pub queued_at: Option<Timestamp>,
    /// Consecutive discipline-gate blocks in the current turn.
    pub gate_blocks: u32,
    /// Whether idle-wake on messages is enabled.
    pub wake_enabled: bool,
    /// Goal or prompt mode (TSK-010).
    pub mode: TaskMode,
    /// Runs alone in its project (SPEC-071): it waits for the project's working tasks, and the
    /// project's other tasks wait for it.
    pub supertask: bool,
}

/// How an attempt was launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchKind {
    /// Fresh session.
    New,
    /// Native resume of the same provider session.
    Resume,
}

impl LaunchKind {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Resume => "resume",
        }
    }

    /// Parses a wire name (unknown → `New`).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "resume" => Self::Resume,
            _ => Self::New,
        }
    }
}

/// One process run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRecord {
    /// Id.
    pub id: AttemptId,
    /// Task.
    pub task_id: TaskId,
    /// Sequence in the task.
    pub seq: u32,
    /// Harness.
    pub harness: Harness,
    /// Account used.
    pub account_id: Option<AccountId>,
    /// Provider session id (known at launch).
    pub provider_session_id: Option<String>,
    /// Launch kind.
    pub launch_kind: LaunchKind,
    /// State.
    pub state: AttemptState,
    /// ptyhost run directory.
    pub run_dir: String,
    /// SHA-256 hex of the agent's MCP/hook token.
    pub token_hash: String,
    /// Sealed token the server uses to authenticate to the ptyhost (never given to the agent).
    pub host_token: Sealed,
    /// ptyhost pid.
    pub ptyhost_pid: Option<u32>,
    /// Agent pid.
    pub child_pid: Option<u32>,
    /// Started.
    pub started_at: Timestamp,
    /// Ended.
    pub ended_at: Option<Timestamp>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Exit reason.
    pub exit_reason: Option<String>,
}

/// A provider session in a task lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSessionRecord {
    /// Id.
    pub id: String,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Task.
    pub task_id: TaskId,
    /// Harness.
    pub harness: Harness,
    /// Provider session id.
    pub session_id: String,
    /// Stored transcript path (imported copies).
    pub transcript_ref: Option<String>,
    /// `native` or `imported`.
    pub origin: String,
    /// First seen.
    pub first_seen: Timestamp,
    /// Last seen.
    pub last_seen: Timestamp,
}

/// Account health/status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountStatus {
    /// Login not completed.
    PendingLogin,
    /// Usable.
    Ok,
    /// Usage limit reached until reset.
    Limited,
    /// Credentials rejected; needs re-login.
    Relogin,
    /// Other failure.
    Error,
}

impl AccountStatus {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingLogin => "pending_login",
            Self::Ok => "ok",
            Self::Limited => "limited",
            Self::Relogin => "relogin",
            Self::Error => "error",
        }
    }

    /// Parses a wire name (unknown → `Error`).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "pending_login" => Self::PendingLogin,
            "ok" => Self::Ok,
            "limited" => Self::Limited,
            "relogin" => Self::Relogin,
            _ => Self::Error,
        }
    }
}

/// A connected provider account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    /// Id.
    pub id: AccountId,
    /// Provider.
    pub provider: Provider,
    /// Label.
    pub label: String,
    /// Sealed credential (the Claude OAuth token).
    pub credential: Option<Sealed>,
    /// Status.
    pub status: AccountStatus,
    /// Usage snapshot JSON.
    pub usage_json: Option<String>,
    /// Last error.
    pub last_error: Option<String>,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
}

/// How a task paused for lack of subscription capacity (SPEC-002 §6.1, ADR-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PauseKind {
    /// Held at a turn boundary (the Stop hook waits, or an agent without hooks is idle); the
    /// process stays alive.
    Turn,
    /// A submitted prompt is held at the `UserPromptSubmit` hook; the process stays alive.
    Prompt,
    /// The CLI reported a usage limit mid-turn; the process stays alive at the limit message.
    Limit,
    /// No process: the task waits to start (new task, Continue, a switch that found no
    /// account).
    Start,
}

impl PauseKind {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Prompt => "prompt",
            Self::Limit => "limit",
            Self::Start => "start",
        }
    }

    /// Parses a wire name (unknown names read as `start`, the kind that needs no process).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "turn" => Self::Turn,
            "prompt" => Self::Prompt,
            "limit" => Self::Limit,
            _ => Self::Start,
        }
    }

    /// Whether the agent's process stays alive during the pause.
    #[must_use]
    pub fn holds_process(self) -> bool {
        !matches!(self, Self::Start)
    }
}

/// One pause of a task for lack of capacity (`capacity_pauses`). At most one per task is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseRecord {
    /// Id.
    pub id: String,
    /// Task.
    pub task_id: TaskId,
    /// How the task is held.
    pub kind: PauseKind,
    /// The account the paused process runs on (`None` without a process).
    pub account_id: Option<AccountId>,
    /// Text for the resumed session when it resumes on another account: the held prompt
    /// (`prompt`); `None` otherwise.
    pub continuation: Option<String>,
    /// When the pause began.
    pub started_at: Timestamp,
    /// When it ended; `None` while open.
    pub ended_at: Option<Timestamp>,
    /// `released`, `switched`, `started`, `cancelled`, `ended` once closed.
    pub outcome: Option<String>,
}

/// One provider reading of one usage window (`usage_samples`), kept 8 days for the burn rate.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSampleRecord {
    /// Account.
    pub account_id: AccountId,
    /// When it was read.
    pub at: Timestamp,
    /// `five_hour`, `seven_day`, `seven_day_opus` or `seven_day_sonnet`.
    pub window: String,
    /// Utilization in percent.
    pub used_percent: f64,
    /// When the window resets, when known.
    pub resets_at: Option<Timestamp>,
    /// Who read it: [`USAGE_FROM_PROBE`] or [`USAGE_FROM_SESSION`].
    pub source: String,
}

/// One run's lifetime, subscription and metrics, for the pool's token use (SPEC-110).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenRunRecord {
    /// The subscription it ran on.
    pub account_id: Option<AccountId>,
    /// Start.
    pub started_at: Timestamp,
    /// End, `None` while it runs.
    pub ended_at: Option<Timestamp>,
    /// Its metrics, when any were recorded.
    pub metrics: Option<MetricsRecord>,
}

/// A usage reading from the provider's usage endpoint.
pub const USAGE_FROM_PROBE: &str = "probe";
/// A usage reading a running session passed on (Claude Code's status line, ADR-17).
pub const USAGE_FROM_SESSION: &str = "session";

/// A recorded rotation decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchDecisionRecord {
    /// Id.
    pub id: String,
    /// Task.
    pub task_id: TaskId,
    /// Time.
    pub at: Timestamp,
    /// Reason code.
    pub reason: String,
    /// Inputs JSON (audit).
    pub inputs: String,
    /// From description.
    pub from_desc: String,
    /// To description.
    pub to_desc: String,
    /// `started`, `completed`, `failed`, `waiting`, `refused`.
    pub outcome: String,
}

/// A task step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    /// Id.
    pub id: StepId,
    /// Task.
    pub task_id: TaskId,
    /// Position.
    pub ordinal: u32,
    /// Title.
    pub title: String,
    /// Status wire name.
    pub status: String,
    /// Note.
    pub note: Option<String>,
    /// Last actor (`task:<n>`, `user:<login>`, `system`).
    pub updated_by: Option<String>,
    /// Last update.
    pub updated_at: Option<Timestamp>,
    /// Declared effort, 1 to [`yulius_domain::steps::MAX_EFFORT`] (STP-006).
    pub effort: u8,
}

/// When a step last started and finished, from its event history (STP-007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepTiming {
    /// Step.
    pub step_id: StepId,
    /// Last move to `in_progress`.
    pub started_at: Option<Timestamp>,
    /// Last move to `done` or `skipped`.
    pub finished_at: Option<Timestamp>,
}

/// An inter-task or user message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRecord {
    /// Id.
    pub id: MessageId,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Sending task.
    pub from_task: Option<TaskId>,
    /// Sending user.
    pub from_user: Option<UserId>,
    /// Recipient task (`None` = to people; a broadcast only in history).
    pub to_task: Option<TaskId>,
    /// `info`, `question`, `request`, `scope`.
    pub kind: String,
    /// Subject.
    pub subject: String,
    /// Body.
    pub body: String,
    /// Created.
    pub created_at: Timestamp,
}

/// A message plus the receipt state for one recipient task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEntry {
    /// The message.
    pub message: MessageRecord,
    /// Delivered to the agent (hook context).
    pub delivered: bool,
    /// Read via the inbox tool.
    pub read: bool,
}

/// An advisory scope claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeClaimRecord {
    /// Id.
    pub id: String,
    /// Claiming task.
    pub task_id: TaskId,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Path glob.
    pub pattern: String,
    /// Note.
    pub note: Option<String>,
    /// Created.
    pub created_at: Timestamp,
}

/// A number Yulius gave a task for a numbered file (SPEC-083 NUM-002): a decision record, a spec,
/// a migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordReservation {
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Repository-relative directory of the numbered files.
    pub dir: String,
    /// The text before the number (`""` for `0066-x.md`, `"SPEC-"`).
    pub prefix: String,
    /// The number.
    pub number: u64,
    /// The task it was given to.
    pub task_id: TaskId,
    /// When.
    pub reserved_at: Timestamp,
}

/// Per-attempt usage metrics; `None` = unknown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetricsRecord {
    /// Model.
    pub model: Option<String>,
    /// Input tokens.
    pub input_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: Option<u64>,
    /// Cache-read tokens.
    pub cache_read: Option<u64>,
    /// Cache-write tokens.
    pub cache_write: Option<u64>,
    /// Completed turns.
    pub turns: Option<u32>,
    /// Active milliseconds.
    pub active_ms: Option<u64>,
}

/// What recounting a run recorded before SPEC-087 from its transcript did (SPEC-091 RCT-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecountOutcome {
    /// Its figures were replaced by its transcript's.
    Recomputed,
    /// No session, worktree or transcript: its figures stay as they were.
    NoTranscript,
    /// The transcript holds nothing of the run: its figures stay.
    NoLines,
    /// The run still runs: its watch reads its transcript from its start (MET-002).
    Live,
    /// The run was deleted.
    Gone,
}

impl RecountOutcome {
    /// Stored name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recomputed => "recomputed",
            Self::NoTranscript => "no_transcript",
            Self::NoLines => "no_lines",
            Self::Live => "live",
            Self::Gone => "gone",
        }
    }
}

/// Token totals of one account's runs since a time (ROT-009).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountTokens {
    /// Account.
    pub account_id: String,
    /// Runs started since the time.
    pub runs: u32,
    /// Distinct tasks.
    pub tasks: u32,
    /// Input tokens (`None` when no run reported usage).
    pub input: Option<u64>,
    /// Output tokens.
    pub output: Option<u64>,
    /// Cache-read tokens.
    pub cache_read: Option<u64>,
    /// Cache-write tokens.
    pub cache_write: Option<u64>,
}

/// Feedback of one user on one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackRecord {
    /// Task.
    pub task_id: TaskId,
    /// User.
    pub user_id: UserId,
    /// `correct` or `incorrect`.
    pub verdict: String,
    /// Comment.
    pub comment: String,
    /// Time.
    pub at: Timestamp,
}

/// Cached GitHub-derived access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedAccess {
    /// Level.
    pub level: AccessLevel,
    /// When checked.
    pub checked_at: Timestamp,
}
