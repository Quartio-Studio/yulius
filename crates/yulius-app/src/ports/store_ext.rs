//! Store areas added by later checkpoints (workflows, automations, skills, MCP, delivery,
//! imports, analytics). Each area owns its trait here and its implementation in the store
//! adapter; `ExtensionStores` bundles them into [`super::store::Store`].

use async_trait::async_trait;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::automations::{AuditEntry, AutomationRecord, AutomationRunRecord, RunState};
use crate::error::AppResult;
use crate::mcp_config::McpServerRecord;
use crate::notes::NoteRecord;
use crate::workflows::WorkflowRecord;

/// Workflow persistence (CP7).
#[async_trait]
pub trait WorkflowStore: Send + Sync {
    /// Instance workflows plus those of `sandbox`: instance first, then by name.
    async fn workflows(&self, sandbox: &SandboxId) -> AppResult<Vec<WorkflowRecord>>;
    /// Workflow by id.
    async fn workflow(&self, id: &str) -> AppResult<Option<WorkflowRecord>>;
    /// Atomically inserts `copy` (when given) and deletes the workflow `id`, clearing automation
    /// references to it unless `keep_references` (another workflow now answers to the id);
    /// `false` when `id` was absent (nothing is inserted then).
    async fn retire_workflow(
        &self,
        id: &str,
        copy: Option<&WorkflowRecord>,
        keep_references: bool,
    ) -> AppResult<bool>;
    /// Inserts a workflow; `Capacity` when its scope already holds `max_per_scope`.
    async fn insert_workflow(&self, workflow: &WorkflowRecord, max_per_scope: u32)
    -> AppResult<()>;
    /// Replaces name, description, steps, instructions and gates; `false` when absent.
    async fn update_workflow(&self, workflow: &WorkflowRecord) -> AppResult<bool>;
    /// Deletes a workflow and clears automation references to it; `false` when absent.
    async fn delete_workflow(&self, id: &str) -> AppResult<bool>;
}

/// Automation persistence (CP7). Run rows are unique per (automation, scheduled time).
#[async_trait]
pub trait AutomationStore: Send + Sync {
    /// Inserts an automation; `Capacity` when the sandbox already has `max_per_sandbox`.
    async fn insert_automation(
        &self,
        automation: &AutomationRecord,
        max_per_sandbox: u32,
    ) -> AppResult<()>;
    /// Automation by id.
    async fn automation(&self, id: &str) -> AppResult<Option<AutomationRecord>>;
    /// Automations of a sandbox by name.
    async fn automations(&self, sandbox: &SandboxId) -> AppResult<Vec<AutomationRecord>>;
    /// Enabled automations of active (not removed) sandboxes.
    async fn enabled_automations(&self) -> AppResult<Vec<AutomationRecord>>;
    /// Replaces the editable fields (name, cron, offset, prompt, workflow, harness, enabled);
    /// `false` when absent.
    async fn update_automation(&self, automation: &AutomationRecord) -> AppResult<bool>;
    /// Moves the schedule anchor (last handled time) forward to `at`; never backwards.
    async fn advance_automation_anchor(&self, id: &str, at: Timestamp) -> AppResult<()>;
    /// Deletes an automation and its run history; `false` when absent.
    async fn delete_automation(&self, id: &str) -> AppResult<bool>;
    /// Inserts a run row unless one exists for the same (automation, scheduled time);
    /// returns whether it was inserted.
    async fn insert_run(&self, run: &AutomationRunRecord) -> AppResult<bool>;
    /// One run.
    async fn automation_run(
        &self,
        automation: &str,
        scheduled_for: Timestamp,
    ) -> AppResult<Option<AutomationRunRecord>>;
    /// Moves a `scheduled` run to its outcome; `false` when it is no longer `scheduled`.
    async fn finish_run(
        &self,
        automation: &str,
        scheduled_for: Timestamp,
        state: RunState,
        task: Option<&TaskId>,
        error: Option<&str>,
    ) -> AppResult<bool>;
    /// Newest runs of an automation first.
    async fn runs(&self, automation: &str, limit: u32) -> AppResult<Vec<AutomationRunRecord>>;
    /// Runs still `scheduled` (left by a crash), oldest first.
    async fn pending_runs(&self, limit: u32) -> AppResult<Vec<AutomationRunRecord>>;
    /// The newest run that created or linked a task.
    async fn last_task_run(&self, automation: &str) -> AppResult<Option<AutomationRunRecord>>;
    /// The task created for an automation run key (`<automation>@<ms>`).
    async fn task_by_automation_run(&self, key: &str) -> AppResult<Option<TaskId>>;
    /// Audit entries targeting an automation, newest first.
    async fn automation_audit(&self, automation: &str, limit: u32) -> AppResult<Vec<AuditEntry>>;
}

/// Skill settings persistence (CP7).
#[async_trait]
pub trait SkillStore: Send + Sync {
    /// Explicit per-sandbox settings as (skill name, enabled); absent skills are enabled.
    async fn skill_settings(&self, sandbox: &SandboxId) -> AppResult<Vec<(String, bool)>>;
    /// Stores a per-sandbox setting.
    async fn set_skill_enabled(
        &self,
        sandbox: &SandboxId,
        name: &str,
        enabled: bool,
    ) -> AppResult<()>;
}

/// MCP server configuration persistence (CP7). Names are unique per scope under
/// [`crate::mcp_config::name_key`].
#[async_trait]
pub trait McpStore: Send + Sync {
    /// Instance servers plus those of `sandbox`, instance first, then by name.
    async fn mcp_servers(&self, sandbox: &SandboxId) -> AppResult<Vec<McpServerRecord>>;
    /// Server by id.
    async fn mcp_server(&self, id: &str) -> AppResult<Option<McpServerRecord>>;
    /// Inserts a server; `Conflict` on a duplicate name in its scope, `Capacity` when the scope
    /// already holds `max_per_scope`.
    async fn insert_mcp_server(
        &self,
        server: &McpServerRecord,
        max_per_scope: u32,
    ) -> AppResult<()>;
    /// Replaces a server's fields (scope is fixed); `Conflict` on a duplicate name, `false`
    /// when absent.
    async fn update_mcp_server(&self, server: &McpServerRecord) -> AppResult<bool>;
    /// Deletes a server; `false` when absent.
    async fn delete_mcp_server(&self, id: &str) -> AppResult<bool>;
}

/// Delivery and environment-run persistence (CP8). Records live in [`crate::delivery`].
#[async_trait]
pub trait DeliveryStore: Send + Sync {
    /// Delivery row of a task (`None` = stage `none`, never recorded).
    async fn delivery(
        &self,
        task: &yulius_domain::ids::TaskId,
    ) -> crate::error::AppResult<Option<crate::delivery::DeliveryRecord>>;
    /// Delivery rows of every task of a sandbox (archived tasks included).
    async fn deliveries(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
    ) -> crate::error::AppResult<Vec<crate::delivery::DeliveryRecord>>;
    /// Compare-and-set: writes `next` only while the task's current stage (`none` when no row
    /// exists) equals `expected`; `Ok(false)` when it changed concurrently.
    async fn transition_delivery(
        &self,
        expected: &yulius_domain::delivery::DeliveryStage,
        next: &crate::delivery::DeliveryRecord,
    ) -> crate::error::AppResult<bool>;
    /// Inserts a run. `Conflict` while another run of the same sandbox is `running`.
    async fn insert_environment_run(
        &self,
        run: &crate::delivery::EnvironmentRunRecord,
    ) -> crate::error::AppResult<()>;
    /// One run.
    async fn environment_run(
        &self,
        id: &str,
    ) -> crate::error::AppResult<Option<crate::delivery::EnvironmentRunRecord>>;
    /// Most recent runs of a sandbox, newest first.
    async fn environment_runs(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        limit: u32,
    ) -> crate::error::AppResult<Vec<crate::delivery::EnvironmentRunRecord>>;
    /// Runs still recorded as `running` (all sandboxes).
    async fn running_environment_runs(
        &self,
    ) -> crate::error::AppResult<Vec<crate::delivery::EnvironmentRunRecord>>;
    /// Replaces the output (and commit, when known) of a running run.
    async fn update_environment_run(
        &self,
        id: &str,
        output: &str,
        commit: Option<&str>,
    ) -> crate::error::AppResult<()>;
    /// Records the outcome of a `running` run; `Ok(false)` when it already ended.
    async fn finish_environment_run(
        &self,
        id: &str,
        state: crate::delivery::EnvironmentRunState,
        output: &str,
        exit_code: Option<i32>,
        ended_at: yulius_domain::time::Timestamp,
    ) -> crate::error::AppResult<bool>;
}

/// One `yulius import` upload job (SPEC-002 §6.5, table `import_jobs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportJobRecord {
    /// Job id (opaque, used in chunk and completion paths).
    pub id: String,
    /// Target sandbox.
    pub sandbox_id: Option<yulius_domain::ids::SandboxId>,
    /// Importing user (only they may upload to or complete the job).
    pub user_id: yulius_domain::ids::UserId,
    /// `created`, `receiving`, `received`, `applying`, `completed` or `failed`.
    pub state: String,
    /// Manifest JSON (contracts `ImportManifest`, canonical serialization).
    pub manifest: String,
    /// Contiguous bytes received per manifest file name.
    pub received: std::collections::BTreeMap<String, u64>,
    /// Applied-import JSON once completed.
    pub result: Option<String>,
    /// Created.
    pub created_at: yulius_domain::time::Timestamp,
    /// Last change.
    pub updated_at: yulius_domain::time::Timestamp,
}

/// Import job persistence (CP10).
#[async_trait]
pub trait ImportStore: Send + Sync {
    /// Inserts a job.
    async fn insert_import_job(&self, job: &ImportJobRecord) -> crate::error::AppResult<()>;
    /// Job by id.
    async fn import_job(&self, id: &str) -> crate::error::AppResult<Option<ImportJobRecord>>;
    /// The most recent jobs of a sandbox, newest first, at most `limit`.
    async fn import_jobs(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        limit: u32,
    ) -> crate::error::AppResult<Vec<ImportJobRecord>>;
    /// Compare-and-set of one file's received length: sets `received` only while the stored
    /// value equals `expected`; `Ok(false)` otherwise (lost race).
    async fn advance_import_file(
        &self,
        id: &str,
        name: &str,
        expected: u64,
        received: u64,
        at: yulius_domain::time::Timestamp,
    ) -> crate::error::AppResult<bool>;
    /// Compare-and-set of the job state: moves to `to` only while the state is one of `from`
    /// (and, when given, `updated_at` equals `expected_updated`); stores `result` when given.
    async fn transition_import_job(
        &self,
        id: &str,
        from: &[&str],
        to: &str,
        expected_updated: Option<yulius_domain::time::Timestamp>,
        result: Option<&str>,
        at: yulius_domain::time::Timestamp,
    ) -> crate::error::AppResult<bool>;
}

/// Analytics queries (CP11): read-only views over tasks, attempts, metrics, feedback and
/// switch decisions.
#[async_trait]
pub trait AnalyticsStore: Send + Sync {
    /// Tasks of `query.sandboxes` created in `[from, to)` (newest `limit`, returned oldest
    /// first) with their attempts and metrics, switch decisions and feedback, read consistently.
    async fn analytics_data(
        &self,
        query: &crate::analytics::AnalyticsQuery,
    ) -> crate::error::AppResult<crate::analytics::AnalyticsData>;
}

/// Subscription pool persistence (SPEC-002 ADR-17): pauses for lack of capacity and provider
/// usage readings for the burn rate.
#[async_trait]
pub trait CapacityStore: Send + Sync {
    /// Opens a pause; `false` (nothing written) when the task already has an open one.
    async fn insert_pause(&self, pause: &crate::model::PauseRecord) -> AppResult<bool>;
    /// The open pause of a task.
    async fn open_pause(&self, task: &TaskId) -> AppResult<Option<crate::model::PauseRecord>>;
    /// Every open pause, oldest first.
    async fn open_pauses(&self) -> AppResult<Vec<crate::model::PauseRecord>>;
    /// Changes how an open pause holds its task; `false` when the task has none.
    async fn update_open_pause(
        &self,
        task: &TaskId,
        kind: crate::model::PauseKind,
        continuation: Option<&str>,
    ) -> AppResult<bool>;
    /// Closes the open pause of a task with `outcome`; `false` when there was none.
    async fn end_pause(&self, task: &TaskId, at: Timestamp, outcome: &str) -> AppResult<bool>;
    /// Pauses open at or after `since` (still open or ended later), oldest first.
    async fn pauses_since(&self, since: Timestamp) -> AppResult<Vec<crate::model::PauseRecord>>;
    /// Stores provider readings and drops readings older than `keep_after`.
    async fn record_usage_samples(
        &self,
        samples: &[crate::model::UsageSampleRecord],
        keep_after: Timestamp,
    ) -> AppResult<()>;
    /// Readings at or after `since`, oldest first.
    async fn usage_samples(
        &self,
        since: Timestamp,
    ) -> AppResult<Vec<crate::model::UsageSampleRecord>>;
    /// Attempts that ran at or after `since` (still live, or ended at or after it): their
    /// lifetimes, subscription and metrics.
    async fn token_runs(&self, since: Timestamp) -> AppResult<Vec<crate::model::TokenRunRecord>>;
}

/// Chained tasks and held prompts (SPEC-001 TSK-012, TSK-013).
#[async_trait]
pub trait IntakeStore: Send + Sync {
    /// Records that `chain.task_id` runs after `chain.after_task_id`.
    async fn insert_chain(&self, chain: &crate::intake::ChainRecord) -> AppResult<()>;
    /// The chain link of a task (whether it waits or already started).
    async fn chain(&self, task: &TaskId) -> AppResult<Option<crate::intake::ChainRecord>>;
    /// Tasks chained after `task`, oldest first.
    async fn chain_successors(&self, task: &TaskId) -> AppResult<Vec<crate::intake::ChainRecord>>;
    /// Sets or clears why a chained task needs a person.
    async fn set_chain_blocked(&self, task: &TaskId, reason: Option<&str>) -> AppResult<()>;
    /// Sets the time a prepared task starts by itself, or clears it (SPEC-105).
    async fn set_task_schedule(
        &self,
        task: &TaskId,
        start_at: Option<Timestamp>,
        at: Timestamp,
    ) -> AppResult<()>;
    /// The time a task was scheduled to start, whatever its status.
    async fn task_schedule(&self, task: &TaskId) -> AppResult<Option<Timestamp>>;
    /// Prepared tasks whose time is at or before `now`, earliest first (at most 100 per call).
    async fn due_prepared(&self, now: Timestamp) -> AppResult<Vec<TaskId>>;
    /// `prepared → queued` for the tick, in one statement: only while the task is prepared and
    /// its stored time is at or before `now` (a time moved or cleared meanwhile wins). `false`
    /// when either no longer holds.
    async fn release_scheduled(&self, task: &TaskId, now: Timestamp) -> AppResult<bool>;
    /// Replaces how a waiting task starts (SPEC-126), in one transaction: only while the task is
    /// still `change.from`, it takes `change.to` (with `queued`: in line from `change.at`), its
    /// chain becomes `change.chain` and its start time `change.start_at`. `false` when the task
    /// left `change.from`.
    async fn change_start(&self, change: &crate::intake::StartChange) -> AppResult<bool>;
    /// `chained → queued` for an evaluation of `evaluated` (SPEC-126 CHS-005), in one statement:
    /// only while the task is chained and its stored chain is still the one evaluated (the same
    /// predecessor, rule and time). `false` when either no longer holds.
    async fn release_chain(
        &self,
        evaluated: &crate::intake::ChainRecord,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Marks the chain `evaluated` as needing a person, only while it is still the task's stored
    /// chain (as [`IntakeStore::release_chain`] checks it). `false` when it was replaced.
    async fn block_chain(
        &self,
        evaluated: &crate::intake::ChainRecord,
        reason: &str,
    ) -> AppResult<bool>;
    /// Inserts a pending held prompt; earlier pending holds of the task become `superseded`.
    async fn insert_hold(&self, hold: &crate::intake::HoldRecord) -> AppResult<()>;
    /// A held prompt by id.
    async fn hold(&self, id: &str) -> AppResult<Option<crate::intake::HoldRecord>>;
    /// The task's pending held prompt, if any.
    async fn pending_hold(&self, task: &TaskId) -> AppResult<Option<crate::intake::HoldRecord>>;
    /// Moves a hold from `expected` to `state` (compare-and-set); `false` when it was not in
    /// `expected` any more.
    async fn resolve_hold(
        &self,
        id: &str,
        expected: &str,
        state: &str,
        by: &str,
        at: Timestamp,
        new_task: Option<&TaskId>,
    ) -> AppResult<bool>;
    /// Consumes a "send anyway" of the task for this exact prompt (SHA-256 hex) resolved at or
    /// after `since`; `true` once, when one existed.
    async fn take_sent_hold(
        &self,
        task: &TaskId,
        sha256: &str,
        since: Timestamp,
    ) -> AppResult<bool>;
    /// Records how Auto chose a new task's mode and workflow (TSK-015; one row per task).
    async fn insert_auto_choice(
        &self,
        choice: &crate::auto_choice::AutoChoiceRecord,
    ) -> AppResult<()>;
    /// The task's Auto choice, if it was created with anything on Auto.
    async fn auto_choice(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<crate::auto_choice::AutoChoiceRecord>>;
}

/// Instance updates (SPEC-002 ADR-18) and the saved Jev key (SPEC-001 ADM-002).
#[async_trait]
pub trait UpdateStore: Send + Sync {
    /// Inserts a run; `false` when another run is open (at most one at a time).
    async fn insert_update_run(&self, run: &crate::updates::UpdateRunRecord) -> AppResult<bool>;
    /// The open run, if any.
    async fn open_update_run(&self) -> AppResult<Option<crate::updates::UpdateRunRecord>>;
    /// The newest run, open or not.
    async fn last_update_run(&self) -> AppResult<Option<crate::updates::UpdateRunRecord>>;
    /// Moves the open run `id` from `from` to `to` (compare-and-set): records `outcome` (the
    /// state a resuming run ends in) and `error` when given, and closes the run when `to` is
    /// terminal. `false` when the run was not in `from` any more.
    async fn move_update_run(
        &self,
        id: &str,
        from: yulius_domain::update::RunState,
        to: yulius_domain::update::RunState,
        outcome: Option<yulius_domain::update::RunState>,
        error: Option<&str>,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// The saved Jev key, sealed, with when and by whom it was set.
    async fn jev_key(&self) -> AppResult<Option<crate::jev_key::StoredJevKey>>;
    /// Saves the sealed Jev key set by `by` (`Some`) or removes it (`None`).
    async fn set_jev_key(
        &self,
        key: Option<(&crate::model::Sealed, &str)>,
        at: Timestamp,
    ) -> AppResult<()>;
}

/// Yulius-run CI (SPEC-001 DLV-006, SPEC-002 ADR-21, tables `ci_runs` and `ci_jobs`).
#[async_trait]
pub trait CiStore: Send + Sync {
    /// Inserts a run with its jobs atomically.
    async fn insert_ci_run(
        &self,
        run: &crate::ci::CiRunRecord,
        jobs: &[crate::ci::CiJobRecord],
    ) -> AppResult<()>;
    /// One run.
    async fn ci_run(&self, id: &str) -> AppResult<Option<crate::ci::CiRunRecord>>;
    /// The jobs of a run in order; outputs only when `with_output`.
    async fn ci_jobs(&self, run: &str, with_output: bool)
    -> AppResult<Vec<crate::ci::CiJobRecord>>;
    /// Most recent runs of a sandbox (by queue time), newest first, at most `limit`.
    async fn ci_runs(
        &self,
        sandbox: &SandboxId,
        limit: u32,
    ) -> AppResult<Vec<crate::ci::CiRunRecord>>;
    /// Runs of a sandbox for one commit, newest first.
    async fn ci_runs_for_sha(
        &self,
        sandbox: &SandboxId,
        sha: &str,
    ) -> AppResult<Vec<crate::ci::CiRunRecord>>;
    /// Queued and running runs of every sandbox, oldest first.
    async fn active_ci_runs(&self) -> AppResult<Vec<crate::ci::CiRunRecord>>;
    /// Compare-and-set of a run's state with `update`; `false` when it was in none of
    /// `from`. `Conflict` when another run of the sandbox is already running.
    async fn transition_ci_run(
        &self,
        id: &str,
        from: &[yulius_domain::ci::RunState],
        to: yulius_domain::ci::RunState,
        update: &crate::ci::CiRunUpdate,
    ) -> AppResult<bool>;
    /// Compare-and-set of a job's state with `update`; `false` when it was in none of `from`.
    async fn transition_ci_job(
        &self,
        id: &str,
        from: &[yulius_domain::ci::RunState],
        to: yulius_domain::ci::RunState,
        update: &crate::ci::CiJobUpdate,
    ) -> AppResult<bool>;
    /// Replaces the output of a running job.
    async fn set_ci_job_output(&self, id: &str, output: &str) -> AppResult<()>;
    /// Puts a `running` run and all its jobs back to `queued` (after a server restart, or a job
    /// refused over the disk quota), clearing times and replacing outputs with `note`; `false`
    /// when the run was not running.
    async fn requeue_ci_run(&self, id: &str, note: &str) -> AppResult<bool>;
    /// Records a commit status publication: `published` is the run state GitHub accepted the
    /// status of, `None` on failure with `note`; counts the attempt.
    async fn record_ci_status(
        &self,
        id: &str,
        published: Option<yulius_domain::ci::RunState>,
        note: Option<&str>,
    ) -> AppResult<()>;
    /// Runs whose current state's commit status was not accepted yet, queued at or after
    /// `since`, with fewer than `max_attempts` attempts, oldest first (at most `limit`).
    async fn unpublished_ci_runs(
        &self,
        since: Timestamp,
        max_attempts: u32,
        limit: u32,
    ) -> AppResult<Vec<crate::ci::CiRunRecord>>;
    /// Deletes finished runs (and their jobs) beyond the newest `keep` of each sandbox and
    /// those queued before `before`; returns how many runs were deleted.
    async fn prune_ci_runs(&self, keep: u32, before: Timestamp) -> AppResult<u64>;
}

/// CD (deployment) runs (SPEC-134, table `cd_runs`).
#[async_trait]
pub trait CdStore: Send + Sync {
    /// Inserts a running run. `Conflict` while another run of the same environment of the
    /// sandbox is running.
    async fn insert_cd_run(&self, run: &crate::ci::cd::CdRunRecord) -> AppResult<()>;
    /// One run; its output only when `with_output`.
    async fn cd_run(
        &self,
        id: &str,
        with_output: bool,
    ) -> AppResult<Option<crate::ci::cd::CdRunRecord>>;
    /// Most recent runs of a sandbox, newest first, at most `limit`, without output.
    async fn cd_runs(
        &self,
        sandbox: &SandboxId,
        limit: u32,
    ) -> AppResult<Vec<crate::ci::cd::CdRunRecord>>;
    /// Running runs of every sandbox, without output.
    async fn running_cd_runs(&self) -> AppResult<Vec<crate::ci::cd::CdRunRecord>>;
    /// Replaces the output of a running run.
    async fn set_cd_run_output(&self, id: &str, output: &str) -> AppResult<()>;
    /// Ends a running run; `false` when it was not running.
    async fn finish_cd_run(&self, id: &str, end: &crate::ci::cd::CdRunEnd) -> AppResult<bool>;
    /// Deletes ended runs beyond the newest `keep` of each sandbox and environment and those
    /// started before `before`, keeping each environment's newest passed run (what is deployed);
    /// returns how many were deleted.
    async fn prune_cd_runs(&self, keep: u32, before: Timestamp) -> AppResult<u64>;
}

/// What a task's agent asked a person to do (SPEC-134 CDT-004, column `tasks.needs_you`).
#[async_trait]
pub trait NeedsYouStore: Send + Sync {
    /// The open request of a task.
    async fn task_needs_you(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<crate::needs_you::NeedsYouRecord>>;
    /// Records (`Some`) or ends (`None`) the request; `true` when something changed.
    async fn set_task_needs_you(
        &self,
        task: &TaskId,
        request: Option<&crate::needs_you::NeedsYouRecord>,
    ) -> AppResult<bool>;
    /// Tasks of a sandbox with an open request.
    async fn needs_you_tasks(&self, sandbox: &SandboxId) -> AppResult<Vec<TaskId>>;
}

/// Notepads (SPEC-041): one row per scope (`team`, `sandbox:<sid>`).
#[async_trait]
pub trait NoteStore: Send + Sync {
    /// The stored note of `scope`, if it was ever written.
    async fn note(&self, scope: &str) -> AppResult<Option<NoteRecord>>;
    /// Compare-and-set: stores `body` as `base_revision + 1` when the stored revision is
    /// `base_revision` (0 = never written) and returns the new note; `None` (nothing stored)
    /// when the stored revision differs.
    async fn put_note(
        &self,
        scope: &str,
        body: &str,
        base_revision: u64,
        updated_by: &str,
        at: Timestamp,
    ) -> AppResult<Option<NoteRecord>>;
}

/// All extension areas.
pub trait ExtensionStores:
    WorkflowStore
    + AutomationStore
    + SkillStore
    + McpStore
    + DeliveryStore
    + ImportStore
    + AnalyticsStore
    + CapacityStore
    + IntakeStore
    + UpdateStore
    + crate::ports::vault::VaultStore
    + CiStore
    + CdStore
    + NeedsYouStore
    + crate::ports::session_docs::SessionDocStore
    + NoteStore
{
}

impl<T> ExtensionStores for T where
    T: WorkflowStore
        + AutomationStore
        + SkillStore
        + McpStore
        + DeliveryStore
        + ImportStore
        + AnalyticsStore
        + CapacityStore
        + IntakeStore
        + UpdateStore
        + crate::ports::vault::VaultStore
        + CiStore
        + CdStore
        + NeedsYouStore
        + NeedsYouStore
        + crate::ports::session_docs::SessionDocStore
        + NoteStore
{
}
