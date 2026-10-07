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

/// Import job persistence (CP10).
#[async_trait]
pub trait ImportStore: Send + Sync {}

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
