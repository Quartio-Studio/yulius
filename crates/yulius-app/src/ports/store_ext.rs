//! Store areas added by later checkpoints (workflows, automations, skills, MCP, delivery,
//! imports, analytics). Each area owns its trait here and its implementation in the store
//! adapter; `ExtensionStores` bundles them into [`super::store::Store`].

use async_trait::async_trait;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::automations::{AuditEntry, AutomationRecord, AutomationRunRecord, RunState};
use crate::error::AppResult;
use crate::mcp_config::McpServerRecord;
use crate::workflows::WorkflowRecord;

/// Workflow persistence (CP7).
#[async_trait]
pub trait WorkflowStore: Send + Sync {
    /// Instance workflows plus those of `sandbox`: instance first, then by name.
    async fn workflows(&self, sandbox: &SandboxId) -> AppResult<Vec<WorkflowRecord>>;
    /// Workflow by id.
    async fn workflow(&self, id: &str) -> AppResult<Option<WorkflowRecord>>;
    /// Inserts `workflows` atomically only when no instance workflow exists; returns whether
    /// anything was inserted.
    async fn seed_instance_workflows(&self, workflows: &[WorkflowRecord]) -> AppResult<bool>;
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

/// Delivery and environment-run persistence (CP8).
#[async_trait]
pub trait DeliveryStore: Send + Sync {}

/// Import job persistence (CP10).
#[async_trait]
pub trait ImportStore: Send + Sync {}

/// Analytics queries (CP11).
#[async_trait]
pub trait AnalyticsStore: Send + Sync {}

/// All extension areas.
pub trait ExtensionStores:
    WorkflowStore
    + AutomationStore
    + SkillStore
    + McpStore
    + DeliveryStore
    + ImportStore
    + AnalyticsStore
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
{
}
