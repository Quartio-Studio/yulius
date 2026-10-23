//! Contained runs in isolated sandboxes (SPEC-002 ADR-22): asking the isolation adapter for the
//! sandbox's holder and holding a lease until the run's process has joined it.

use std::sync::Arc;

use yulius_domain::ids::SandboxId;

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::isolation::{LimitCounters, SandboxIsolation, SandboxJoin};

/// `PATH` of runs in an isolated sandbox (root's, with `sbin` for package tools).
pub const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// `PATH` for a process whose home is `home`: the per-user tool directories first — where
/// rustup, pipx, `npm --prefix ~/.local` and similar installers put what the onboarding (or a
/// person) installed, so it stays found by every later run without a login shell — then `rest`
/// (F 2026-10-11).
#[must_use]
pub fn with_home_tools(home: &str, rest: &str) -> String {
    let home = home.trim_end_matches('/');
    format!("{home}/.cargo/bin:{home}/.local/bin:{rest}")
}

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
    /// `sandbox`'s limit counters, where an isolation adapter counts them (SPEC-268 PID-004).
    pub(crate) async fn limit_counters(&self, sandbox: &SandboxId) -> Option<LimitCounters> {
        self.isolation.get()?.limit_counters(sandbox).await
    }

    /// The holder a contained run of `sandbox` joins, with its lease; `None` when the run uses
    /// the previous containment (no adapter, no base yet).
    ///
    /// # Errors
    /// The adapter's refusal (disk quota) or failure (the holder cannot start).
    pub(crate) async fn isolation_join(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Option<(SandboxJoin, IsolationLease)>> {
        // An adapter is installed only on a contained server, so none means the previous
        // containment.
        let Some(isolation) = self.isolation.get() else {
            return Ok(None);
        };
        let request = self.sandbox_ref(sandbox);
        // Over the quota: free what can be freed (caches of tasks that do not work) and ask
        // once more before refusing (NFR-009).
        let prepared = match isolation.prepare(&request).await {
            Err(AppError::Conflict(reason)) => {
                let over = isolation
                    .disk(&request)
                    .await
                    .is_some_and(|usage| usage.used > usage.limit);
                let freed = over
                    && self
                        .disk_upkeep(sandbox, true)
                        .await
                        .is_ok_and(|report| report.freed > 0 || report.worktrees > 0);
                if !freed {
                    return Err(AppError::Conflict(
                        self.naming_consumers(sandbox, reason).await,
                    ));
                }
                match isolation.prepare(&request).await {
                    Err(AppError::Conflict(reason)) => {
                        return Err(AppError::Conflict(
                            self.naming_consumers(sandbox, reason).await,
                        ));
                    }
                    other => other?,
                }
            }
            other => other?,
        };
        Ok(prepared.map(|join| {
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

impl App {
    /// A quota refusal with the sandbox's largest consumers, so people see whose space it is
    /// (SPEC-059 DSK-202).
    async fn naming_consumers(&self, sandbox: &SandboxRecord, reason: String) -> String {
        let consumers = self.disk_consumers(sandbox).await;
        if consumers.is_empty() {
            reason
        } else {
            format!("{reason} (largest: {consumers})")
        }
    }
}

#[cfg(test)]
mod home_tools_tests {
    use super::*;

    #[test]
    fn home_tool_directories_come_first() {
        assert_eq!(
            with_home_tools("/home/ana/", SANDBOX_PATH),
            format!("/home/ana/.cargo/bin:/home/ana/.local/bin:{SANDBOX_PATH}")
        );
    }
}
