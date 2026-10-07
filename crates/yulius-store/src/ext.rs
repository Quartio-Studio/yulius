//! Extension store areas implemented by later checkpoints (see `yulius_app::ports::store_ext`).

use yulius_app::ports::store_ext::{
    AutomationStore, DeliveryStore, ImportStore, McpStore, SkillStore, WorkflowStore,
};

use crate::SqliteStore;

impl WorkflowStore for SqliteStore {}
impl AutomationStore for SqliteStore {}
impl SkillStore for SqliteStore {}
impl McpStore for SqliteStore {}
impl DeliveryStore for SqliteStore {}
impl ImportStore for SqliteStore {}
