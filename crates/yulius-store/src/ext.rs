//! Extension store areas implemented by later checkpoints (see `yulius_app::ports::store_ext`).

use yulius_app::ports::store_ext::{DeliveryStore, ImportStore};

use crate::SqliteStore;

impl DeliveryStore for SqliteStore {}
impl ImportStore for SqliteStore {}
