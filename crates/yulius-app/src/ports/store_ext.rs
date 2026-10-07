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
