//! Contained runs in isolated sandboxes (SPEC-002 ADR-22): asking the isolation adapter for the
//! sandbox's holder and holding a lease until the run's process has joined it.

use std::sync::Arc;

use yulius_domain::ids::SandboxId;

use crate::context::App;
use crate::error::AppResult;
use crate::identity::sandbox_hostname;
use crate::model::SandboxRecord;
use crate::ports::isolation::{SandboxIsolation, SandboxJoin, SandboxRef};

/// `PATH` of runs in an isolated sandbox (root's, with `sbin` for package tools).
pub const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// A lease on a sandbox's holder: while it lives the holder is not stopped as idle. Dropped
/// once the run's process has joined (or failed to start).
pub struct IsolationLease {
    isolation: Arc<dyn SandboxIsolation>,
    sandbox: SandboxId,
}

impl Drop for IsolationLease {
    fn drop(&mut self) {
        self.isolation.release(&self.sandbox);
    }
}

impl App {
    /// The holder a contained run of `sandbox` joins, with its lease; `None` when the run uses
    /// the previous containment (not contained, no adapter, no base yet).
    ///
    /// # Errors
    /// The adapter's refusal (disk quota) or failure (the holder cannot start).
    pub(crate) async fn isolation_join(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Option<(SandboxJoin, IsolationLease)>> {
        if !self.config.contained {
            return Ok(None);
        }
        let Some(isolation) = self.isolation.get() else {
            return Ok(None);
        };
        let first = sandbox.port_base;
        let last = first.saturating_add(sandbox.port_count.saturating_sub(1));
        let request = SandboxRef {
            id: sandbox.id.clone(),
            hostname: sandbox_hostname(&sandbox.slug),
            dir: self.sandbox_dir(&sandbox.id),
            port_first: first,
            port_last: last,
        };
        Ok(isolation.prepare(&request).await?.map(|join| {
            (
                join,
                IsolationLease {
                    isolation: Arc::clone(isolation),
                    sandbox: sandbox.id.clone(),
                },
            )
        }))
    }
}
