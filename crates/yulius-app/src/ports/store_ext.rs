//! Store areas added by later checkpoints (workflows, automations, skills, MCP, delivery,
//! imports, analytics). Each area owns its trait here and its implementation in the store
//! adapter; `ExtensionStores` bundles them into [`super::store::Store`].

use async_trait::async_trait;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
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
    /// The team servers of the instance and of `sandbox`, plus `person`'s personal servers of
    /// both (SPEC-332); instance first, then by name.
    async fn mcp_servers(
        &self,
        sandbox: &SandboxId,
        person: Option<&yulius_domain::ids::UserId>,
    ) -> AppResult<Vec<McpServerRecord>>;
    /// Every server that can reach a run of `sandbox`: the team servers of the instance and of
    /// `sandbox` and every person's personal servers of both (what redaction covers).
    async fn mcp_servers_reaching(&self, sandbox: &SandboxId) -> AppResult<Vec<McpServerRecord>>;
    /// Server by id.
    async fn mcp_server(&self, id: &str) -> AppResult<Option<McpServerRecord>>;
    /// Inserts a server; `Conflict` on a duplicate name in its scope (sandbox and owner),
    /// `Capacity` when the scope already holds `max_per_scope`.
    async fn insert_mcp_server(
        &self,
        server: &McpServerRecord,
        max_per_scope: u32,
    ) -> AppResult<()>;
    /// Replaces a server's fields, its scope included; `Conflict` on a duplicate name in its
    /// scope, `Capacity` when it moves into a scope that already holds `max_per_scope`, `false`
    /// when absent.
    async fn update_mcp_server(
        &self,
        server: &McpServerRecord,
        max_per_scope: u32,
    ) -> AppResult<bool>;
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
    /// One coherent read of the open owner PR, task, decisions and pauses before typing.
    /// An unavailable recipient or an unreadable current attention reason defers the notice.
    async fn armed_notice_recipient(
        &self,
        task: &yulius_domain::ids::TaskId,
        pr: u64,
        branch: &str,
    ) -> crate::error::AppResult<Option<crate::pr_watch::NoticeRecipient>>;
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
    ) -> crate::error::AppResult<bool> {
        self.transition_with_landing(expected, next, None, None)
            .await
    }
    /// Same compare-and-set, recording the original PR base at opening or the merge identity at landing.
    async fn transition_with_landing(
        &self,
        expected: &yulius_domain::delivery::DeliveryStage,
        next: &crate::delivery::DeliveryRecord,
        opening_base: Option<&str>,
        landing: Option<&crate::delivery::evidence::Landing>,
    ) -> crate::error::AppResult<bool>;
    /// SHA for this task's recorded PR. A confirmed GitHub landing can recover a legacy row.
    async fn landed_sha(
        &self,
        task: &yulius_domain::ids::TaskId,
        pr: Option<u64>,
        known: Option<&crate::delivery::evidence::Landing>,
    ) -> crate::error::AppResult<Option<String>>;
    /// Historical compact outcomes at one task landing, optionally for a single attempt.
    async fn merge_evidence(
        &self,
        task: &yulius_domain::ids::TaskId,
        sha: &str,
        run_id: Option<&str>,
        before: Option<&str>,
    ) -> crate::error::AppResult<Option<crate::delivery::evidence::MergeEvidence>>;
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
    /// The most recent jobs a person created, newest first, at most `limit` (a new playground's
    /// upload is resumed by its person before its sandbox exists, SPEC-171).
    async fn user_import_jobs(
        &self,
        user: &yulius_domain::ids::UserId,
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
    /// The observed pause and whether it owned a live or spawning attempt, in one snapshot.
    async fn open_pause_with_process(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<(crate::model::PauseRecord, bool)>>;
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
    async fn end_pause(&self, task: &TaskId, at: Timestamp, outcome: &str) -> AppResult<bool> {
        self.end_pause_if(task, None, at, outcome).await
    }
    /// Closes only the observed open pause when its ID is supplied.
    async fn end_pause_if(
        &self,
        task: &TaskId,
        observed: Option<&str>,
        at: Timestamp,
        outcome: &str,
    ) -> AppResult<bool>;
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

/// Instance updates (SPEC-002 ADR-18), the saved Jev key (SPEC-001 ADM-002) and the saved Vidoc
/// key (SPEC-195 VDK-001).
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
    /// The saved instance Vidoc key, sealed, with when and by whom it was set (SPEC-195).
    async fn vidoc_key(&self) -> AppResult<Option<crate::vidoc_key::StoredVidocKey>>;
    /// Saves the sealed Vidoc key set by `by` (`Some`) or removes it (`None`).
    async fn set_vidoc_key(
        &self,
        key: Option<(&crate::model::Sealed, &str)>,
        at: Timestamp,
    ) -> AppResult<()>;
    /// When and by whom Vidoc was switched off for the instance (SPEC-356); `None` while on.
    async fn vidoc_off(&self) -> AppResult<Option<crate::vidoc_key::VidocOff>>;
    /// Switches Vidoc off for the instance, by `by` (`Some`), or on again (`None`).
    async fn set_vidoc_off(&self, by: Option<&str>, at: Timestamp) -> AppResult<()>;
    /// The instance's resource limits record (SPEC-459): a JSON document, `None` before the
    /// first one is saved.
    async fn limits(&self) -> AppResult<Option<String>>;
    /// Saves the limits record.
    ///
    /// # Errors
    /// `Conflict` while the instance is not set up: there is no row to keep it.
    async fn set_limits(&self, doc: &str) -> AppResult<()>;
    /// The Claude Code release channel chosen in Settings, with when and by whom (SPEC-461
    /// CCM-009); `None` while the server's configuration is followed.
    async fn claude_channel(&self) -> AppResult<Option<crate::claude_code::ChosenChannel>>;
    /// Stores the channel `by` chose (`Some((channel, by))`), or forgets it (`None`).
    async fn set_claude_channel(
        &self,
        channel: Option<(&str, &str)>,
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
    /// The commits in this bounded Git history batch with a retained passing CI run, in input order.
    async fn ci_shas_with_pass(
        &self,
        sandbox: &SandboxId,
        commits: &[String],
    ) -> AppResult<Vec<String>>;
    /// Bounded scheduler history, most recently admitted first. A new run may
    /// inherit an older queue position from the head it superseded.
    async fn ci_planning_runs(
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
    /// Runs at a full or uniquely abbreviated PR head, latest admission first.
    /// A transferred queue place can precede an older verdict at the same head.
    async fn ci_runs_for_pull_head(
        &self,
        sandbox: &SandboxId,
        head: crate::ci::PullHead<'_>,
    ) -> AppResult<Vec<crate::ci::CiRunRecord>>;
    /// Queued and running runs of every sandbox, oldest first.
    async fn active_ci_runs(&self) -> AppResult<Vec<crate::ci::CiRunRecord>>;
    /// A running gate with an executed pass, no queued job and at most one running job
    /// (including an executor finalizing the verdict). Used by the planned-install drain.
    async fn has_finishing_ci_run(&self) -> AppResult<bool>;
    /// Puts queued run `id` before the other queued runs of its sandbox (SPEC-487 MRG-004): its
    /// place becomes a millisecond before the earliest of theirs, but never before another run
    /// of the same commit (the latest run of a commit is read by place). `false`, and nothing
    /// changed, when it is not queued, no queued run of its sandbox comes before it, or a run of
    /// its commit sits right before it.
    async fn ci_run_to_front(&self, id: &str) -> AppResult<bool>;
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
    /// `sandbox`'s tally of CI minutes for the UTC days of `[from, to)` (SPEC-497 OFC-003).
    async fn ci_minutes(
        &self,
        sandbox: &SandboxId,
        from: Option<Timestamp>,
        to: Option<Timestamp>,
    ) -> AppResult<Vec<crate::ci::CiMinutesRow>>;
    /// CI jobs of running runs that have not started (SPEC-497).
    async fn ci_jobs_waiting(&self) -> AppResult<u32>;
    /// Records that `tests` failed in job `job_key` of run `run` of `reference` (`pr/<n>`,
    /// `branch/<name>`) at commit `sha` at `at`, and forgets failures before `forget_before`
    /// (SPEC-497 OFC-002).
    async fn record_test_failures(
        &self,
        sandbox: &SandboxId,
        job_key: &str,
        (reference, run, sha): (&str, &str, &str),
        tests: &[String],
        at: Timestamp,
        forget_before: Timestamp,
    ) -> AppResult<()>;
    /// The failures of `test` in job `job_key` of `sandbox` since `since`.
    async fn test_failures(
        &self,
        sandbox: &SandboxId,
        job_key: &str,
        test: &str,
        since: Timestamp,
    ) -> AppResult<Vec<yulius_domain::ci::flaky::TestFailure>>;
    /// Quarantines `entry`'s test; `false` when it already was.
    async fn quarantine_test(&self, entry: &crate::ci::Quarantined) -> AppResult<bool>;
    /// The tests quarantined in `sandbox`, or in every sandbox.
    async fn quarantined_tests(
        &self,
        sandbox: Option<&SandboxId>,
    ) -> AppResult<Vec<crate::ci::Quarantined>>;
    /// Ends the quarantine of `test` in `sandbox` and forgets its failures, so the failures that
    /// made it flaky never count again (SPEC-497 OFC-002, review round 1).
    async fn release_test(&self, sandbox: &SandboxId, test: &str) -> AppResult<()>;
    /// Keeps one minute's reading of the host for CI and forgets those before `forget_before`
    /// (SPEC-497).
    async fn record_ci_load(
        &self,
        minute: yulius_domain::resources::LoadMinute,
        forget_before: Timestamp,
    ) -> AppResult<()>;
    /// The minutes read in `[from, to)`, oldest first (SPEC-497).
    async fn ci_load(
        &self,
        from: Option<Timestamp>,
        to: Option<Timestamp>,
    ) -> AppResult<Vec<yulius_domain::resources::LoadMinute>>;
    /// Sets or clears the reason of a `queued` job: what it waits for (SPEC-497 HBA-005).
    async fn set_queued_ci_job_reason(&self, id: &str, reason: Option<&str>) -> AppResult<()>;
    /// Puts a `running` run and all its jobs back to `queued` (after a server restart, or a job
    /// refused over the disk quota), clearing times and replacing outputs with `note`; `false`
    /// when the run was not running.
    async fn requeue_ci_run(&self, id: &str, note: &str, at: Timestamp) -> AppResult<bool>;
    /// Records a commit status publication: `published` is the run state GitHub accepted the
    /// status of, `None` on failure with `note`. A failure adds an attempt and a published status
    /// resets them, so attempts count the failures in a row (SPEC-198 RCS-004).
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
    /// Claims a Vidoc finding (`fingerprint` in `file`; `anchored` when it names a line's text)
    /// for `sandbox` before its issue is opened (SPEC-217 VSI-002); `false` when it was claimed or
    /// filed before, unless that claim got no issue within an hour (its process stopped).
    async fn claim_security_finding(
        &self,
        sandbox: &SandboxId,
        finding: (&str, &str, bool),
        at: Timestamp,
    ) -> AppResult<bool>;
    /// One filed finding of `sandbox` (with its issue), if any.
    async fn filed_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
    ) -> AppResult<Option<crate::ci::FiledFinding>>;
    /// The filed findings of `sandbox` not resolved yet, at most `limit`, a random sample (so
    /// findings past the limit are looked at too).
    async fn unresolved_security_findings(
        &self,
        sandbox: &SandboxId,
        limit: u32,
    ) -> AppResult<Vec<crate::ci::FiledFinding>>;
    /// Forgets a filed finding whose issue `issue` GitHub no longer has, so it is filed anew when
    /// it is reported again (SPEC-217 VSI-004); a record another run made since (a new claim, a
    /// new issue) is kept.
    async fn forget_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
        issue: u64,
    ) -> AppResult<()>;
    /// Marks a filed finding resolved at `at` (its issue was closed), or, with `None`, not
    /// resolved (reported again) (SPEC-217 VSI-004).
    async fn resolve_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
        at: Option<Timestamp>,
    ) -> AppResult<()>;
    /// Records the issue a claimed finding was filed as, or, with `None` (opening it failed),
    /// releases the claim so a later run files it.
    async fn settle_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
        issue: Option<u64>,
    ) -> AppResult<()>;
}

/// Which CD outcomes can supply a historical run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CdRunHistory {
    /// Include the run in progress.
    Latest,
    /// The latest terminal verdict, including failures.
    Finished,
    /// Only a successful run.
    Passed,
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
    /// The newest passed run of an environment of a sandbox as an environment of `stage` (what
    /// is deployed there), without output.
    async fn last_passed_cd_run(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        stage: yulius_domain::cd::CdStage,
    ) -> AppResult<Option<crate::ci::cd::CdRunRecord>>;
    /// The newest run a task's agent started, without output.
    async fn latest_cd_run_of_task(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<crate::ci::cd::CdRunRecord>>;
    /// Running runs of every sandbox, without output.
    async fn running_cd_runs(&self) -> AppResult<Vec<crate::ci::cd::CdRunRecord>>;
    /// Replaces the output of a running run.
    async fn set_cd_run_output(&self, id: &str, output: &str) -> AppResult<()>;
    /// Records that the task's `yulius wait` reported the ended run `id`; `true` the first time.
    async fn mark_cd_run_reported(&self, id: &str) -> AppResult<bool>;
    /// Ends a running run; `false` when it was not running.
    async fn finish_cd_run(&self, id: &str, end: &crate::ci::cd::CdRunEnd) -> AppResult<bool>;
    /// Deletes ended runs beyond the newest `keep` of each sandbox and environment and those
    /// started before `before`, keeping each environment's newest passed run (what is deployed);
    /// returns how many were deleted.
    async fn prune_cd_runs(&self, keep: u32, before: Timestamp) -> AppResult<u64>;
    /// The newest run of `kind` of an environment of a sandbox as an environment of `stage`,
    /// without output (SPEC-415: the newest drill).
    async fn last_cd_run_of_kind(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        stage: yulius_domain::cd::CdStage,
        kind: yulius_domain::rollback::CdRunKind,
        history: CdRunHistory,
    ) -> AppResult<Option<crate::ci::cd::CdRunRecord>>;
    /// Records releases of an environment of `stage` (in order, the last the newest) and keeps
    /// the newest [`yulius_domain::rollback::KEEP_RELEASE_RECORDS`] of each part.
    async fn insert_cd_releases(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        stage: yulius_domain::cd::CdStage,
        records: &[yulius_domain::rollback::ReleaseRecord],
    ) -> AppResult<()>;
    /// The release records of an environment of `stage`, every part, newest first.
    async fn cd_releases(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        stage: yulius_domain::cd::CdStage,
    ) -> AppResult<Vec<yulius_domain::rollback::ReleaseRecord>>;
}

/// One handoff as stored (SPEC-512 HOF-003..HOF-006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRecord {
    /// Its id.
    pub id: String,
    /// The task that asked.
    pub source_task: TaskId,
    /// Its sandbox.
    pub source_sandbox: SandboxId,
    /// The sandbox it hands to.
    pub target_sandbox: SandboxId,
    /// The source task's owner, who the new task belongs to.
    pub owner: UserId,
    /// The arguments, as JSON (`title`, `prompt`, options): exactly what is created.
    pub args: String,
    /// The sandboxes of its chain, the source's first.
    pub chain: Vec<String>,
    /// Its depth: 1 for a task no handoff made.
    pub depth: u32,
    /// `requested`, `creating` (claimed, its task being made), `created`, `declined` or
    /// `refused`.
    pub state: String,
    /// Why it was refused or declined.
    pub reason: Option<String>,
    /// The task it made.
    pub target_task: Option<TaskId>,
    /// When it was asked.
    pub created_at: Timestamp,
}

/// Tasks hand work to another sandbox (SPEC-512 part 5, migration 0076).
#[async_trait]
pub trait HandoffStore: Send + Sync {
    /// The instance's setting as stored (JSON; `None`: never set).
    async fn handoff_setting(&self) -> AppResult<Option<String>>;
    /// Saves the setting.
    ///
    /// # Errors
    /// `Conflict` while the instance is not set up.
    async fn set_handoff_setting(&self, doc: &str) -> AppResult<()>;
    /// Stores a new handoff.
    async fn insert_handoff(&self, handoff: &HandoffRecord) -> AppResult<()>;
    /// One handoff.
    async fn handoff(&self, id: &str) -> AppResult<Option<HandoffRecord>>;
    /// A task's own handoffs, newest first.
    async fn handoffs_of_source(&self, task: &TaskId) -> AppResult<Vec<HandoffRecord>>;
    /// The handoff that made `task`, if one did.
    async fn handoff_of_target(&self, task: &TaskId) -> AppResult<Option<HandoffRecord>>;
    /// Moves a handoff from `from` to `to` (with its reason and the task it made) only while it
    /// is still `from`; `false` when it was not.
    async fn decide_handoff(
        &self,
        id: &str,
        from: &str,
        to: &str,
        reason: Option<&str>,
        target_task: Option<&TaskId>,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// How many handoffs `owner` asked for since `since` that were not refused.
    async fn handoffs_since(&self, owner: &UserId, since: Timestamp) -> AppResult<u32>;
    /// The created handoffs whose outcome was not told yet.
    async fn untold_handoffs(&self) -> AppResult<Vec<HandoffRecord>>;
    /// Records the outcome as told; `false` when it already was.
    async fn mark_handoff_told(&self, id: &str) -> AppResult<bool>;
    /// At start: a handoff left `creating` by a server that stopped is `created` when its task
    /// was recorded, else `refused` (asked again by its agent, never created twice); how many.
    async fn settle_creating_handoffs(&self, at: Timestamp) -> AppResult<u64>;
}

/// One request to change a project's merge-first staging target (SPEC-575, migration 0078).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingRequestRecord {
    /// Its id.
    pub id: String,
    /// Its sandbox.
    pub sandbox_id: SandboxId,
    /// The merge-first environment's name.
    pub environment: String,
    /// The task it delivers; `None`: a person's request for the integration head.
    pub task_id: Option<TaskId>,
    /// The integration branch's commit it deploys.
    pub revision: String,
    /// Where it stands.
    pub state: yulius_domain::staging::StagingState,
    /// The full gate's run that judged its revision.
    pub ci_run_id: Option<String>,
    /// The CD run that deployed it, or the verified one that includes it.
    pub cd_run_id: Option<String>,
    /// Deploy attempts ended so far.
    pub attempts: u32,
    /// What its state means for people (the failed jobs, why it was refused or superseded).
    pub note: Option<String>,
    /// Who asked: `task:<number>`.
    pub requested_by: String,
    /// Whether its outcome was told to its task's session.
    pub told: bool,
    /// When it was asked.
    pub created_at: Timestamp,
    /// Its last change.
    pub updated_at: Timestamp,
}

/// What a staging request's move also records; `None` keeps the stored value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StagingChange {
    /// The full gate's run.
    pub ci_run_id: Option<String>,
    /// The CD run.
    pub cd_run_id: Option<String>,
    /// Deploy attempts ended.
    pub attempts: Option<u32>,
    /// Its note (replaces the stored one).
    pub note: Option<String>,
    /// Forget its CD run (a new attempt starts): a crash before the new run is recorded then
    /// reads as lost, never as the old run's end.
    pub clear_cd_run: bool,
}

/// A sandbox's recorded Infra bootstrap revision (SPEC-575 MFS-012, migration 0079).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapUpgrade {
    /// The revision.
    pub revision: u32,
    /// `laid`, `claimed` or `linked`.
    pub state: String,
    /// Its upgrade task, once linked (and not deleted since).
    pub task_id: Option<TaskId>,
    /// When it was recorded (for a claim: claimed).
    pub claimed_at: Timestamp,
}

/// Merge-first staging requests (SPEC-575 MFS-005, MFS-008, migration 0078). Every move is a
/// compare-and-set from the state it was decided on, so a restart or a second tick never moves a
/// request twice.
#[async_trait]
pub trait StagingStore: Send + Sync {
    /// Stores a new request; `false` when the same requester's request for that revision (or a
    /// second success of the task, or a second deploy) already exists, so nothing was written.
    async fn insert_staging_request(&self, request: &StagingRequestRecord) -> AppResult<bool>;
    /// One request.
    async fn staging_request(&self, id: &str) -> AppResult<Option<StagingRequestRecord>>;
    /// Latest staging request whose result still needs to reach this task.
    async fn task_staging_wait(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<yulius_domain::staging::StagingState>>;
    /// The requests of `environment` asked by the same requester: the task's, or (for `None`) a
    /// person's for `revision`; oldest first.
    async fn staging_requests_of(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        task: Option<&TaskId>,
        revision: &str,
    ) -> AppResult<Vec<StagingRequestRecord>>;
    /// The requests of `environment` still on their way (testing, ready, deploying), oldest
    /// first.
    async fn pending_staging_requests(
        &self,
        sandbox: &SandboxId,
        environment: &str,
    ) -> AppResult<Vec<StagingRequestRecord>>;
    /// The sandboxes and environments with a pending request, or a deploy that failed since
    /// `failed_since` (staging is restored to its last verified release after it).
    async fn staging_queues(&self, failed_since: Timestamp) -> AppResult<Vec<(SandboxId, String)>>;
    /// Whether staging recorded a successful request for this exact revision.
    async fn staging_verified(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        revision: &str,
    ) -> AppResult<bool>;
    /// The newest requests of `environment`, newest first.
    async fn recent_staging_requests(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        limit: u32,
    ) -> AppResult<Vec<StagingRequestRecord>>;
    /// Moves a request from `from` to `to` with `change`, only while it is still `from`;
    /// `false` when it was not (or the move would break a unique rule).
    async fn move_staging_request(
        &self,
        id: &str,
        from: yulius_domain::staging::StagingState,
        to: yulius_domain::staging::StagingState,
        change: &StagingChange,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// The requests of tasks whose outcome (verified, included, gate failed, deploy failed,
    /// superseded) was not told yet, oldest first.
    async fn untold_staging_requests(&self) -> AppResult<Vec<StagingRequestRecord>>;
    /// Acknowledge exactly the observed terminal decision. A retried request owns its new outcome.
    async fn mark_staging_told(&self, outcome: &StagingRequestRecord) -> AppResult<bool>;
    /// Claims making the upgrade task of Infra bootstrap `revision` for `sandbox` (SPEC-575
    /// MFS-012, migration 0079): `true` when it recorded the claim, which happens only while
    /// its recorded revision is older (or none). A claim stays until its task is linked.
    async fn claim_bootstrap_upgrade(
        &self,
        sandbox: &SandboxId,
        revision: u32,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Records that `sandbox` is at `revision` with nothing to upgrade (created or onboarded at
    /// it), unless a record at it or later exists.
    async fn lay_bootstrap_revision(
        &self,
        sandbox: &SandboxId,
        revision: u32,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Links the upgrade task of a claimed revision, only while it is claimed: `false`
    /// otherwise, so a task is adopted or linked exactly once.
    async fn link_bootstrap_task(
        &self,
        sandbox: &SandboxId,
        revision: u32,
        task: &TaskId,
    ) -> AppResult<bool>;
    /// The sandbox's recorded bootstrap revision, its state, task and when it was recorded.
    async fn bootstrap_upgrade(&self, sandbox: &SandboxId) -> AppResult<Option<BootstrapUpgrade>>;
    /// Newest confirmed integration landing for the current merged PR. A merge receipt is
    /// scoped to its own PR. An unconfirmed final merge never falls back to an older slice.
    async fn staging_landing(
        &self,
        task: &TaskId,
        branch: &str,
        receipt_pr: Option<u64>,
    ) -> AppResult<Option<String>>;
}

/// One person's stored digest (SPEC-512 DIG-005).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDigest {
    /// Its id.
    pub id: String,
    /// When it was made.
    pub created_at: Timestamp,
    /// Its `DigestView` as JSON.
    pub body: String,
}

/// The end-of-day digest (SPEC-512 part 3): the instance's setting, its last run and each
/// person's digests.
#[async_trait]
pub trait DigestStore: Send + Sync {
    /// The setting as stored (JSON; `None`: never set) and when the digest last ran.
    async fn digest_setting(&self) -> AppResult<(Option<String>, Option<Timestamp>)>;
    /// Saves the setting and `ran_at` as the last run, so a digest turned on is first sent at
    /// its next time.
    ///
    /// # Errors
    /// `Conflict` while the instance is not set up.
    async fn set_digest_setting(&self, doc: &str, ran_at: Timestamp) -> AppResult<()>;
    /// Takes the day's run: records `now` as the last run, and as when a run was last claimed,
    /// only while the last run is still `seen`; `false` when another took it (DIG-002).
    async fn take_digest_run(&self, seen: Option<Timestamp>, now: Timestamp) -> AppResult<bool>;
    /// When a run was last claimed (DIG-002); a save of the setting leaves it.
    async fn digest_sent_at(&self) -> AppResult<Option<Timestamp>>;
    /// Where the last digest's listing ended (DIG-004): it listed what was parked before it.
    async fn digest_listed_at(&self) -> AppResult<Option<Timestamp>>;
    /// Records where a digest's listing ended.
    ///
    /// # Errors
    /// `Conflict` while the instance is not set up.
    async fn set_digest_listed_at(&self, at: Timestamp) -> AppResult<()>;
    /// Keeps `digest` for `user`, and only their last seven.
    async fn put_digest(&self, user: &UserId, digest: &StoredDigest) -> AppResult<()>;
    /// The person's latest digest.
    async fn latest_digest(&self, user: &UserId) -> AppResult<Option<StoredDigest>>;
}

/// One timestamped Needs You system or agent message recorded in a task's history (SPEC-578).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitMessageRecord {
    /// Task id.
    pub task_id: TaskId,
    /// Stable wait-episode key (`UNIQUE (task_id, episode)`).
    pub episode: String,
    /// `agent`, `browser`, `terminal`, or `system`.
    pub source: String,
    /// What happened.
    pub happened: String,
    /// Optional problem/context.
    pub problem: Option<String>,
    /// Concrete next action.
    pub action: String,
    /// Relevant Yulius link.
    pub link: String,
    /// Whether the person was asked to decide.
    pub decision: bool,
    /// When the wait episode was recorded.
    pub created_at: Timestamp,
    /// When the wait episode ended, or `None` while open.
    pub resolved_at: Option<Timestamp>,
}

/// What a task's agent asked a person to do (SPEC-134 CDT-004, column `tasks.needs_you`).
#[async_trait]
pub trait NeedsYouStore: Send + Sync {
    /// The open request of a task.
    async fn task_needs_you(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<crate::needs_you::NeedsYouRecord>>;
    /// Records (`Some`) or ends (`None`) the request at `at`; `true` when something changed.
    async fn set_task_needs_you(
        &self,
        task: &TaskId,
        request: Option<&crate::needs_you::NeedsYouRecord>,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Ends the request at `at` only while it still asks `action`, in one step: a request made
    /// meanwhile stays. `true` when it ended one.
    async fn end_task_needs_you_asking(
        &self,
        task: &TaskId,
        action: &str,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Ends the request asked at `asked_at` (milliseconds) at `at`, in one step, only while it is
    /// the open one: a request asked later stays (SPEC-512 ANS-004). `true` when it ended one.
    async fn end_task_needs_you_at(
        &self,
        task: &TaskId,
        asked_at: i64,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Replaces the open request `read` with `next` at `at`, in one step, only while it is still
    /// `read`: asked at the same time, the same action, naming the same people (SPEC-512
    /// ANS-005). `true` when it replaced it.
    async fn replace_task_needs_you(
        &self,
        task: &TaskId,
        read: &crate::needs_you::NeedsYouRecord,
        next: &crate::needs_you::NeedsYouRecord,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Tasks of a sandbox with an open request.
    async fn needs_you_tasks(&self, sandbox: &SandboxId) -> AppResult<Vec<TaskId>>;
    /// Records one wait-episode message idempotently (SPEC-578): resolves any other open wait
    /// message for the same task and keeps a single row per `(task_id, episode)`. Returns `true`
    /// when a new row was inserted.
    async fn record_wait_message(&self, entry: &WaitMessageRecord) -> AppResult<bool>;
    /// Marks every open wait message of `task` resolved at `at`. Returns `true` when any row
    /// changed.
    async fn resolve_wait_messages(&self, task: &TaskId, at: Timestamp) -> AppResult<bool>;
    /// Timestamped wait messages of `task`, newest first (up to `limit`).
    async fn task_wait_messages(
        &self,
        task: &TaskId,
        limit: u32,
    ) -> AppResult<Vec<WaitMessageRecord>>;
    /// Reads the stored `tasks.attention` payload regardless of current `activity`, for legacy
    /// browser-handoff fallback (SPEC-578).
    async fn stored_task_attention(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<(crate::attention::AttentionRecord, Timestamp)>>;
    /// Clears `tasks.attention` when a legacy browser handoff is resolved (SPEC-578).
    async fn clear_stored_task_attention(&self, task: &TaskId) -> AppResult<bool>;
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

/// Spans: the measured stages of tasks, CI runs and CD runs (SPEC-137, table `spans`). The
/// store's triggers write the spans that follow a state; these methods write the rest.
#[async_trait]
pub trait SpanStore: Send + Sync {
    /// Starts (or records ended) a span; `false` when the subject already has its key.
    async fn insert_span(&self, span: &crate::stages::NewSpan) -> AppResult<bool>;
    /// Ends an open span of a subject at `at` (not before its start); `outcome` when known.
    async fn end_span(
        &self,
        sandbox: &SandboxId,
        kind: yulius_domain::stages::SubjectKind,
        subject: &str,
        key: &str,
        at: Timestamp,
        outcome: Option<&str>,
    ) -> AppResult<bool>;
    /// Sets figures of a span, or adds them to what it has (`add`).
    async fn span_figures(
        &self,
        sandbox: &SandboxId,
        kind: yulius_domain::stages::SubjectKind,
        subject: &str,
        key: &str,
        figures: &[(String, f64)],
        add: bool,
    ) -> AppResult<()>;
    /// Retires an observed singleton only while the same decision is still current.
    async fn retire_singleton(
        &self,
        task: &TaskId,
        observed: &crate::stages::SpanRecord,
        now: Timestamp,
    ) -> AppResult<bool>;
    /// Latest merge wait for this task, including ordinary decisions that supersede a singleton.
    async fn latest_merge_wait(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<crate::stages::SpanRecord>>;
    /// A subject's newest `limit` spans, in start order.
    async fn subject_spans(
        &self,
        sandbox: &SandboxId,
        kind: yulius_domain::stages::SubjectKind,
        subject: &str,
        limit: u32,
    ) -> AppResult<Vec<crate::stages::SpanRecord>>;
    /// The queue and run spans of the CI runs of pull request `number` (alone or in a train).
    async fn ci_reference_spans(
        &self,
        sandbox: &SandboxId,
        number: u64,
    ) -> AppResult<Vec<crate::stages::SpanRecord>>;
    /// Spans that match `filter`, the newest `filter.limit`, in start order.
    async fn spans_between(
        &self,
        filter: &crate::stages::SpanFilter,
    ) -> AppResult<Vec<crate::stages::SpanRecord>>;
    /// Deletes spans that started before `before`; returns how many.
    async fn prune_spans(&self, before: Timestamp) -> AppResult<u64>;
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
    + crate::ports::vault::AgentStateStore
    + crate::ports::variables::ProjectVariableStore
    + crate::ports::guests::PlaygroundGuestStore
    + CiStore
    + CdStore
    + NeedsYouStore
    + DigestStore
    + HandoffStore
    + StagingStore
    + crate::ports::session_docs::SessionDocStore
    + NoteStore
    + SpanStore
    + crate::ports::notify::ChatLinkStore
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
        + crate::ports::vault::AgentStateStore
        + crate::ports::variables::ProjectVariableStore
        + crate::ports::guests::PlaygroundGuestStore
        + CiStore
        + CdStore
        + NeedsYouStore
        + DigestStore
        + HandoffStore
        + StagingStore
        + crate::ports::session_docs::SessionDocStore
        + NoteStore
        + SpanStore
        + crate::ports::notify::ChatLinkStore
{
}
