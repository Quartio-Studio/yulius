//! Store areas added by later checkpoints (workflows, automations, skills, MCP, delivery,
//! imports, analytics). Each area owns its trait here and its implementation in the store
//! adapter; `ExtensionStores` bundles them into [`super::store::Store`].

use async_trait::async_trait;

/// Workflow persistence (CP7).
#[async_trait]
pub trait WorkflowStore: Send + Sync {}

/// Automation persistence (CP7).
#[async_trait]
pub trait AutomationStore: Send + Sync {}

/// Skill settings persistence (CP7).
#[async_trait]
pub trait SkillStore: Send + Sync {}

/// MCP server configuration persistence (CP7).
#[async_trait]
pub trait McpStore: Send + Sync {}

/// Delivery and environment-run persistence (CP8).
#[async_trait]
pub trait DeliveryStore: Send + Sync {}

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
