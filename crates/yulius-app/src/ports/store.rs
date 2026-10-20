//! Persistence ports, split per area. One adapter (SQLite) implements all of them.
//!
//! Compare-and-set methods return `Ok(false)` when the expected state no longer holds; callers
//! treat that as a lost race (one durable outcome, SPEC-002 §6).

use async_trait::async_trait;
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::permission::AccessLevel;
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::model::{
    AccountRecord, AccountStatus, ApiTokenRecord, AttemptRecord, CachedAccess, ControlRequest,
    DeviceGrantBounds, DeviceGrantInsert, DeviceGrantRecord, FeedbackRecord, GitHubTokenRecord,
    InboxEntry, InstanceRecord, MessageRecord, MetricsRecord, PersonPause, ProviderSessionRecord,
    RecordReservation, RecountOutcome, SandboxRecord, ScopeClaimRecord, Sealed, StepRecord,
    SwitchDecisionRecord, TaskRecord, TaskTimes, UserRecord, WebSessionRecord,
};

/// Instance singleton and setup.
#[async_trait]
pub trait InstanceStore: Send + Sync {
    /// The instance row, if initialized.
    async fn instance(&self) -> AppResult<Option<InstanceRecord>>;
    /// Creates the instance row if absent (idempotent); returns the current row.
    async fn init_instance(
        &self,
        public_url: &str,
        setup_token_hash: &str,
    ) -> AppResult<InstanceRecord>;
    /// Replaces the setup token hash while setup is pending.
    async fn set_setup_token_hash(&self, hash: &str) -> AppResult<()>;
    /// Atomically creates the superadmin and completes setup. Fails with `Conflict` when a
    /// superadmin already exists.
    async fn complete_setup(&self, superadmin: &UserRecord, at: Timestamp) -> AppResult<()>;
    /// Stores GitHub App credentials and the owner.
    async fn set_github_app(
        &self,
        sealed: &Sealed,
        owner_login: &str,
        owner_kind: &str,
    ) -> AppResult<()>;
    /// Records the App installation id.
    async fn set_installation(&self, installation_id: Option<u64>) -> AppResult<()>;
    /// Updates the public URL.
    async fn set_public_url(&self, url: &str) -> AppResult<()>;
    /// Replaces the recovery code hash, clearing failures and the lock (ACC-011).
    async fn set_recovery_code(&self, hash: &str) -> AppResult<()>;
    /// Counts a wrong recovery code atomically; at `max_failures` the code is erased and
    /// recovery locked at `at`. Returns (failures, locked).
    async fn record_recovery_failure(
        &self,
        at: Timestamp,
        max_failures: u32,
    ) -> AppResult<(u32, bool)>;
}

/// Users and GitHub tokens.
#[async_trait]
pub trait UserStore: Send + Sync {
    /// User by id.
    async fn user(&self, id: &UserId) -> AppResult<Option<UserRecord>>;
    /// User by GitHub login (case-insensitive).
    async fn user_by_login(&self, login: &str) -> AppResult<Option<UserRecord>>;
    /// User by GitHub numeric id.
    async fn user_by_github_id(&self, github_id: u64) -> AppResult<Option<UserRecord>>;
    /// The superadmin.
    async fn superadmin(&self) -> AppResult<Option<UserRecord>>;
    /// All users.
    async fn users(&self) -> AppResult<Vec<UserRecord>>;
    /// Inserts a user (`Conflict` on duplicate login).
    async fn insert_user(&self, user: &UserRecord) -> AppResult<()>;
    /// Updates flags.
    async fn update_user_flags(
        &self,
        id: &UserId,
        disabled_at: Option<Option<Timestamp>>,
        is_instance_admin: Option<bool>,
    ) -> AppResult<()>;
    /// Replaces a user's password hash (superadmin recovery, ACC-011).
    async fn set_password_hash(&self, id: &UserId, phc: &str) -> AppResult<()>;
    /// Links GitHub identity details after OAuth.
    async fn link_github(
        &self,
        id: &UserId,
        github_id: u64,
        login: &str,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> AppResult<()>;
    /// Stores the user's GitHub token.
    async fn save_github_token(&self, token: &GitHubTokenRecord, at: Timestamp) -> AppResult<()>;
    /// The user's GitHub token.
    async fn github_token(&self, user: &UserId) -> AppResult<Option<GitHubTokenRecord>>;
}

/// Web sessions, CLI tokens, device grants and OAuth states.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Inserts a web session.
    async fn insert_web_session(&self, session: &WebSessionRecord) -> AppResult<()>;
    /// Web session by cookie hash.
    async fn web_session(&self, id_hash: &str) -> AppResult<Option<WebSessionRecord>>;
    /// Revokes one session.
    async fn revoke_web_session(&self, id_hash: &str, at: Timestamp) -> AppResult<()>;
    /// Revokes all sessions and tokens of a user.
    async fn revoke_user_credentials(&self, user: &UserId, at: Timestamp) -> AppResult<()>;
    /// Inserts an API token.
    async fn insert_api_token(&self, token: &ApiTokenRecord) -> AppResult<()>;
    /// API token by hash (not revoked).
    async fn api_token(&self, token_hash: &str) -> AppResult<Option<ApiTokenRecord>>;
    /// Inserts a device grant unless its client or the instance already holds `bounds` live
    /// grants (not expired at `grant.created_at`, not consumed); deletes expired and consumed
    /// grants first, in the same transaction.
    async fn insert_device_grant(
        &self,
        grant: &DeviceGrantRecord,
        bounds: DeviceGrantBounds,
    ) -> AppResult<DeviceGrantInsert>;
    /// Deletes device grants expired at `now` (any status) and consumed ones; returns how many.
    async fn purge_device_grants(&self, now: Timestamp) -> AppResult<usize>;
    /// Device grant by device-code hash.
    async fn device_grant_by_device(
        &self,
        device_code_hash: &str,
    ) -> AppResult<Option<DeviceGrantRecord>>;
    /// Device grant by user code.
    async fn device_grant_by_user_code(
        &self,
        user_code: &str,
    ) -> AppResult<Option<DeviceGrantRecord>>;
    /// Sets status/user/token of a pending grant; `Ok(false)` if not pending.
    async fn resolve_device_grant(
        &self,
        user_code: &str,
        status: &str,
        user: Option<&UserId>,
        token: Option<&Sealed>,
    ) -> AppResult<bool>;
    /// Marks an approved grant consumed and clears its sealed token; `Ok(false)` if not approved.
    async fn consume_device_grant(&self, device_code_hash: &str) -> AppResult<bool>;
    /// Stores an OAuth state.
    async fn insert_oauth_state(
        &self,
        state_hash: &str,
        next_path: &str,
        at: Timestamp,
        expires_at: Timestamp,
    ) -> AppResult<()>;
    /// Removes and returns an unexpired OAuth state's next path.
    async fn take_oauth_state(&self, state_hash: &str, now: Timestamp)
    -> AppResult<Option<String>>;
}

/// GitHub permission cache.
#[async_trait]
pub trait PermissionStore: Send + Sync {
    /// Cached access.
    async fn cached_access(
        &self,
        user: &UserId,
        sandbox: &SandboxId,
    ) -> AppResult<Option<CachedAccess>>;
    /// Stores access.
    async fn put_access(
        &self,
        user: &UserId,
        sandbox: &SandboxId,
        level: AccessLevel,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Drops a user's cache (sign-in, disable).
    async fn clear_access(&self, user: &UserId) -> AppResult<()>;
    /// A counter that grows with every write that can change who may type into a terminal: a
    /// task's owner (hand-over, take-over, delete), a sandbox's removal, a user's flags or GitHub
    /// link, a revoked session or token, a cached GitHub level that changed, a cleared cache.
    /// Read in memory, without the database: a terminal viewer's write grant holds while it is
    /// unchanged (SPEC-118 KEY-002).
    fn access_epoch(&self) -> u64;
}

/// Sandboxes.
#[async_trait]
pub trait SandboxStore: Send + Sync {
    /// Inserts a sandbox (`Conflict` when the repository is already linked).
    async fn insert_sandbox(&self, sandbox: &SandboxRecord) -> AppResult<()>;
    /// Sandbox by id (including removed).
    async fn sandbox(&self, id: &SandboxId) -> AppResult<Option<SandboxRecord>>;
    /// Active sandbox by slug.
    async fn sandbox_by_slug(&self, slug: &str) -> AppResult<Option<SandboxRecord>>;
    /// Active sandbox by repository.
    async fn sandbox_by_repo(&self, owner: &str, name: &str) -> AppResult<Option<SandboxRecord>>;
    /// Active sandboxes.
    async fn sandboxes(&self) -> AppResult<Vec<SandboxRecord>>;
    /// Removed sandboxes, oldest removal first (their leftovers, SPEC-215).
    async fn removed_sandboxes(&self) -> AppResult<Vec<SandboxRecord>>;
    /// Next free port base (multiples of `count` from `start`).
    async fn next_port_base(&self, start: u16, count: u16) -> AppResult<u16>;
    /// Replaces the profile JSON.
    async fn set_profile(&self, id: &SandboxId, profile_json: &str) -> AppResult<()>;
    /// Replaces the recorded default branch (an empty repository's, SPEC-241 EMP-002).
    async fn set_default_branch(&self, id: &SandboxId, branch: &str) -> AppResult<()>;
    /// Sets onboarding state and task.
    async fn set_onboarding(
        &self,
        id: &SandboxId,
        state: &str,
        task: Option<&TaskId>,
    ) -> AppResult<()>;
    /// Moves the onboarding state from `from` to `to` only while it is still `from`; `false`
    /// (nothing written) when it changed meanwhile (SPEC-153 OLC-001).
    async fn transition_onboarding(&self, id: &SandboxId, from: &str, to: &str) -> AppResult<bool>;
    /// Marks removed.
    async fn remove_sandbox(&self, id: &SandboxId, at: Timestamp) -> AppResult<()>;
    /// Records the live playground `id` as the GitHub repository `owner/name` (SPEC-176), only
    /// while it is still a playground; `false` (nothing written) otherwise. `Conflict` when
    /// another sandbox holds that repository. Access changes, so the access epoch grows.
    async fn move_to_github(
        &self,
        id: &SandboxId,
        owner: &str,
        name: &str,
        github_repo_id: Option<u64>,
    ) -> AppResult<bool>;
    /// Renames the slug of a removed sandbox of `owner/name` holding `slug` to
    /// `<slug>-removed-<removed_at ms>`, freeing it for the same repository added again; `false`
    /// when no such removed sandbox holds it.
    async fn release_removed_slug(&self, slug: &str, owner: &str, name: &str) -> AppResult<bool>;
    /// Records `by`'s request to purge the playground `id`'s code (SPEC-171); `false` when one
    /// is already recorded.
    async fn request_purge(&self, id: &SandboxId, by: &UserId, at: Timestamp) -> AppResult<bool>;
    /// Sandboxes whose purge was requested and is not done yet, oldest request first.
    async fn pending_purges(&self) -> AppResult<Vec<SandboxId>>;
    /// Marks the purge of `id` done; `false` when it was done already.
    async fn finish_purge(&self, id: &SandboxId, at: Timestamp) -> AppResult<bool>;
}

/// Field updates for a task (only `Some` fields change).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFieldUpdate {
    /// Title.
    pub title: Option<String>,
    /// Harness.
    pub harness: Option<yulius_domain::harness::Harness>,
    /// Branch.
    pub branch: Option<String>,
    /// Worktree.
    pub worktree_path: Option<String>,
    /// Spec path.
    pub spec_path: Option<String>,
    /// Model.
    pub model: Option<Option<String>>,
    /// Effort.
    pub effort: Option<Option<String>>,
    /// Archived time.
    pub archived_at: Option<Option<Timestamp>>,
    /// Gate block counter.
    pub gate_blocks: Option<u32>,
    /// Idle wake flag.
    pub wake_enabled: Option<bool>,
}

/// Field updates for an attempt's terminal transition or pids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttemptUpdate {
    /// ptyhost pid.
    pub ptyhost_pid: Option<u32>,
    /// Child pid.
    pub child_pid: Option<u32>,
    /// End time.
    pub ended_at: Option<Timestamp>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Exit reason.
    pub exit_reason: Option<String>,
}

/// Tasks, attempts, lineage, metrics, feedback and switch decisions.
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Inserts a task, allocating the next number in its sandbox; returns the stored record.
    async fn insert_task(&self, task: &TaskRecord) -> AppResult<TaskRecord>;
    /// Task by id.
    async fn task(&self, id: &TaskId) -> AppResult<Option<TaskRecord>>;
    /// Task by sandbox and number.
    async fn task_by_number(
        &self,
        sandbox: &SandboxId,
        number: u32,
    ) -> AppResult<Option<TaskRecord>>;
    /// Tasks of a sandbox, archived or not.
    async fn tasks(&self, sandbox: &SandboxId, archived: bool) -> AppResult<Vec<TaskRecord>>;
    /// Tasks in any of the given statuses (all sandboxes).
    async fn tasks_with_status(&self, statuses: &[TaskStatus]) -> AppResult<Vec<TaskRecord>>;
    /// CAS status change; `expected = None` means any. Sets `updated_at`, `closed_at`/
    /// `queued_at` as appropriate, and the state's time when the status or activity changes
    /// (SPEC-087 TIM-001).
    async fn transition_task(
        &self,
        id: &TaskId,
        expected: Option<TaskStatus>,
        next: TaskStatus,
        activity: Option<Activity>,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Sets activity while running; the state's time moves only when it changes (SPEC-087).
    async fn set_activity(
        &self,
        id: &TaskId,
        activity: Option<Activity>,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Sets the activity of a running task only while it still reads `from`: that activity
    /// (or none), entered at that state time (SPEC-213); whether it did.
    async fn set_activity_from(
        &self,
        id: &TaskId,
        from: (Option<Activity>, Timestamp),
        activity: Option<Activity>,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Since when `task` is in its current state and working (SPEC-087 TIM-001), if it exists.
    async fn task_times(&self, task: &TaskId) -> AppResult<Option<TaskTimes>>;
    /// Records the start of `task`'s working stretch, or its end with `None` (SPEC-087
    /// TIM-002). The task's other times stay.
    async fn set_working_since(&self, task: &TaskId, since: Option<Timestamp>) -> AppResult<()>;
    /// A person's pause of `task` (SPEC-191 PER-001), if any.
    async fn person_pause(&self, task: &TaskId) -> AppResult<Option<PersonPause>>;
    /// Records `by`'s pause of `task`'s run `attempt` unless a pause is recorded; returns the
    /// pause in force.
    async fn set_person_pause(
        &self,
        task: &TaskId,
        by: &UserId,
        attempt: &AttemptId,
        at: Timestamp,
    ) -> AppResult<PersonPause>;
    /// Clears the pause of `task`; whether one was recorded.
    async fn clear_person_pause(&self, task: &TaskId, at: Timestamp) -> AppResult<bool>;
    /// The tasks a person paused, with their pauses (at most 1000).
    async fn person_paused_tasks(&self) -> AppResult<Vec<(TaskId, PersonPause)>>;
    /// The request for control of `task` that waits for its owner (SPEC-070), if any.
    async fn control_request(&self, task: &TaskId) -> AppResult<Option<ControlRequest>>;
    /// Records `by`'s request for control of `task` unless another person's request waits (one
    /// at a time); a request of `by` that already waits stays as it is. Returns whether `by`'s
    /// request waits now.
    async fn request_control(&self, task: &TaskId, by: &UserId, at: Timestamp) -> AppResult<bool>;
    /// Drops `by`'s waiting request for control of `task`; false when none of theirs waits.
    async fn drop_control_request(
        &self,
        task: &TaskId,
        by: &UserId,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Passes control of `task` from `from` to `to`, whose request waits, and drops the request,
    /// in one statement: false (and nothing changes) unless `from` still owns the task and `to`'s
    /// request still waits.
    async fn pass_control(
        &self,
        task: &TaskId,
        from: &UserId,
        to: &UserId,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Passes control of `task` from `from`, an owner who can no longer act, to `to` whatever
    /// request waits, and drops that request, in one statement: false unless `from` still owns
    /// the task (SPEC-070 R5).
    async fn take_control(
        &self,
        task: &TaskId,
        from: &UserId,
        to: &UserId,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Appends `addition` to the prompt of a task that has never started (SPEC-052 QMS-002): it is
    /// `queued`, `chained` or `waiting_capacity` with no attempt and no provider session, and the
    /// prompt stays within `max_bytes`. One statement, so a start's compare-and-set to `starting`
    /// either follows it (and reads the addition) or makes it fail. Returns whether it appended.
    async fn append_unstarted_prompt(
        &self,
        id: &TaskId,
        addition: &str,
        max_bytes: usize,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Updates fields.
    async fn update_task(
        &self,
        id: &TaskId,
        update: &TaskFieldUpdate,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Deletes a task and everything recorded for it (attempts, lineage, steps, messages to or
    /// from it, metrics, feedback, delivery, switch decisions, claims, its chain link and held
    /// prompts); references from automation runs, sandbox onboarding, chained successors and
    /// held prompts of other tasks are cleared. Returns whether it existed.
    async fn delete_task(&self, id: &TaskId) -> AppResult<bool>;
    /// Inserts an attempt (`Conflict` when another live attempt exists for the task).
    async fn insert_attempt(&self, attempt: &AttemptRecord) -> AppResult<()>;
    /// Attempt by id.
    async fn attempt(&self, id: &AttemptId) -> AppResult<Option<AttemptRecord>>;
    /// Attempt by token hash.
    async fn attempt_by_token(&self, token_hash: &str) -> AppResult<Option<AttemptRecord>>;
    /// Attempts of a task in order.
    async fn attempts(&self, task: &TaskId) -> AppResult<Vec<AttemptRecord>>;
    /// Whether the task ever had an attempt (cheaper than [`Self::attempts`]).
    async fn has_attempts(&self, task: &TaskId) -> AppResult<bool>;
    /// The spawning/live attempt of a task.
    async fn live_attempt(&self, task: &TaskId) -> AppResult<Option<AttemptRecord>>;
    /// All spawning/live attempts.
    async fn live_attempts(&self) -> AppResult<Vec<AttemptRecord>>;
    /// CAS attempt state change (only allowed transitions per `AttemptState::can_become`).
    async fn transition_attempt(
        &self,
        id: &AttemptId,
        next: AttemptState,
        update: &AttemptUpdate,
    ) -> AppResult<bool>;
    /// Records the provider session id of an attempt.
    async fn set_attempt_session(&self, id: &AttemptId, session_id: &str) -> AppResult<()>;
    /// Upserts a lineage entry (by sandbox, harness, session id).
    async fn upsert_provider_session(&self, record: &ProviderSessionRecord) -> AppResult<()>;
    /// Lineage of a task, oldest first.
    async fn provider_sessions(&self, task: &TaskId) -> AppResult<Vec<ProviderSessionRecord>>;
    /// Whether a provider session is already known in a sandbox.
    async fn provider_session_exists(
        &self,
        sandbox: &SandboxId,
        harness: &str,
        session_id: &str,
    ) -> AppResult<bool>;
    /// Upserts metrics for an attempt.
    async fn put_metrics(
        &self,
        attempt: &AttemptId,
        task: &TaskId,
        metrics: &MetricsRecord,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Metrics of all attempts of a task.
    async fn task_metrics(&self, task: &TaskId) -> AppResult<Vec<MetricsRecord>>;
    /// Runs queued for one recount of their metrics from their transcripts: those recorded
    /// before schema version 23 and not recorded yet, oldest first (SPEC-091 RCT-003).
    async fn queued_metrics_recounts(&self) -> AppResult<Vec<AttemptId>>;
    /// Records a queued run's recount, in one transaction: with `metrics`, the run's stored
    /// figures are replaced (a model it does not name keeps the stored one); the figures after
    /// are recorded with the outcome (those before were when it was queued). `false` when the run was not queued (already
    /// recorded, or never queued): nothing changes (RCT-004).
    async fn finish_metrics_recount(
        &self,
        attempt: &AttemptId,
        outcome: RecountOutcome,
        metrics: Option<&MetricsRecord>,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Upserts feedback.
    async fn put_feedback(&self, feedback: &FeedbackRecord) -> AppResult<()>;
    /// Feedback of a task.
    async fn feedback(&self, task: &TaskId) -> AppResult<Vec<FeedbackRecord>>;
    /// Records a switch decision.
    async fn insert_switch(&self, decision: &SwitchDecisionRecord) -> AppResult<()>;
    /// Updates a switch outcome.
    async fn set_switch_outcome(&self, id: &str, outcome: &str) -> AppResult<()>;
    /// Switch decisions of a task.
    async fn switches(&self, task: &TaskId) -> AppResult<Vec<SwitchDecisionRecord>>;
}

/// Provider accounts.
#[async_trait]
pub trait AccountStore: Send + Sync {
    /// Inserts an account.
    async fn insert_account(&self, account: &AccountRecord) -> AppResult<()>;
    /// Account by id.
    async fn account(&self, id: &AccountId) -> AppResult<Option<AccountRecord>>;
    /// All accounts.
    async fn accounts(&self) -> AppResult<Vec<AccountRecord>>;
    /// Renames an account.
    async fn update_account(&self, id: &AccountId, label: &str, at: Timestamp) -> AppResult<()>;
    /// Stores the sealed credential and status.
    async fn set_account_credential(
        &self,
        id: &AccountId,
        credential: &Sealed,
        status: AccountStatus,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Stores status, usage snapshot and error.
    async fn set_account_state(
        &self,
        id: &AccountId,
        status: AccountStatus,
        usage_json: Option<&str>,
        last_error: Option<&str>,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Records an account's last error only, leaving its status and usage as they are now
    /// (a failed usage request must not write back an older copy, #72).
    async fn set_account_error(
        &self,
        id: &AccountId,
        last_error: &str,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Records a note about reading an account's usage (`usage: …`) as its last error unless
    /// another error is there: a sign-in or re-login error stays (ROT-009).
    async fn set_usage_error(&self, id: &AccountId, note: &str, at: Timestamp) -> AppResult<()>;
    /// Clears an account's last error when it is a note about reading its usage (`usage: …`);
    /// any other error stays.
    async fn clear_usage_error(&self, id: &AccountId, at: Timestamp) -> AppResult<()>;
    /// Deletes an account.
    async fn delete_account(&self, id: &AccountId) -> AppResult<()>;
    /// Token totals per account of the runs started at or after `since`.
    async fn account_tokens(&self, since: Timestamp)
    -> AppResult<Vec<crate::model::AccountTokens>>;
}

/// Steps, messages, scope claims and record numbers.
#[async_trait]
pub trait CoordinationStore: Send + Sync {
    /// Replaces the step list of a task (records events for status changes).
    async fn replace_steps(
        &self,
        task: &TaskId,
        steps: &[StepRecord],
        actor: &str,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Steps of a task in order.
    async fn steps(&self, task: &TaskId) -> AppResult<Vec<StepRecord>>;
    /// When each step of a task last started and finished, from `step_events`.
    async fn step_timings(&self, task: &TaskId) -> AppResult<Vec<crate::model::StepTiming>>;
    /// Updates one step and records an event; `NotFound` if absent.
    async fn update_step(
        &self,
        step: &StepId,
        status: &str,
        note: Option<&str>,
        actor: &str,
        at: Timestamp,
    ) -> AppResult<StepRecord>;
    /// Inserts a message and receipts for the recipients.
    async fn insert_message(&self, message: &MessageRecord, recipients: &[TaskId])
    -> AppResult<()>;
    /// Messages addressed to a task (history: nothing writes messages since SPEC-028),
    /// newest last.
    async fn inbox(
        &self,
        task: &TaskId,
        unread_only: bool,
        limit: u32,
    ) -> AppResult<Vec<InboxEntry>>;
    /// Marks messages delivered (hook context) for a task.
    async fn mark_delivered(&self, task: &TaskId, at: Timestamp) -> AppResult<u32>;
    /// Marks messages read for a task.
    async fn mark_read(&self, task: &TaskId, at: Timestamp) -> AppResult<u32>;
    /// Unread count for a task.
    async fn unread_count(&self, task: &TaskId) -> AppResult<u32>;
    /// Messages of a sandbox involving a task (sent or received), newest last.
    async fn task_messages(&self, task: &TaskId, limit: u32) -> AppResult<Vec<InboxEntry>>;
    /// Inserts a scope claim.
    async fn insert_claim(&self, claim: &ScopeClaimRecord) -> AppResult<()>;
    /// Active claims of a sandbox.
    async fn active_claims(&self, sandbox: &SandboxId) -> AppResult<Vec<ScopeClaimRecord>>;
    /// Releases claims of a task (all when `pattern` is `None`); returns count.
    async fn release_claims(
        &self,
        task: &TaskId,
        pattern: Option<&str>,
        at: Timestamp,
    ) -> AppResult<u32>;
    /// Reserves a record number (SPEC-083); `false` when that number of that directory and
    /// prefix is already reserved.
    async fn reserve_record_number(&self, reservation: &RecordReservation) -> AppResult<bool>;
    /// The record numbers reserved in a sandbox's directory `dir`, oldest first.
    async fn record_reservations(
        &self,
        sandbox: &SandboxId,
        dir: &str,
    ) -> AppResult<Vec<RecordReservation>>;
}

/// Append-only audit log.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Appends an entry.
    async fn audit(
        &self,
        at: Timestamp,
        actor: &str,
        action: &str,
        target: Option<&str>,
        detail: Option<&str>,
    ) -> AppResult<()>;
}

/// Every store area; the SQLite adapter implements this.
pub trait Store:
    InstanceStore
    + UserStore
    + SessionStore
    + PermissionStore
    + SandboxStore
    + TaskStore
    + AccountStore
    + CoordinationStore
    + AuditStore
    + crate::ports::store_ext::ExtensionStores
{
}

impl<T> Store for T where
    T: InstanceStore
        + UserStore
        + SessionStore
        + PermissionStore
        + SandboxStore
        + TaskStore
        + AccountStore
        + CoordinationStore
        + AuditStore
        + crate::ports::store_ext::ExtensionStores
{
}
