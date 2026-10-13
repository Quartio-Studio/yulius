//! A CI run waits for disk instead of erroring (SPEC-059 CI-301..CI-304, ADR-0040).
//!
//! A sandbox over its soft quota refuses new starts, CI jobs included. A queued run of such a
//! sandbox is therefore not started: the dispatcher asks [`App::ci_disk_room`] first. Over the
//! quota it starts one check per sandbox in the background, at most every [`CI_DISK_RECHECK`]:
//! upkeep under pressure (the caches of tasks that do not work, largest first, and the CI
//! checkouts while no run executes), then a fresh measurement. Still over: the sandbox's queued
//! runs say why and who holds the space, and their commit status stays `pending`, so an armed
//! merge keeps waiting. Under: the dispatcher is woken and starts them. Nothing times the wait
//! out; a supersede or a cancel ends it like any queued run.

use std::sync::Arc;
use std::time::Duration;

use yulius_domain::ci::RunState;
use yulius_domain::disk::human;
use yulius_domain::ids::SandboxId;

use super::CiRunUpdate;
use crate::context::App;
use crate::model::SandboxRecord;
use crate::ports::isolation::DiskUsage;

/// How often a sandbox over its quota is pruned and measured again while runs wait for it.
pub const CI_DISK_RECHECK: Duration = Duration::from_secs(60);

/// A sandbox whose runs wait for disk.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DiskWait {
    /// App time (ms) before which no new check starts.
    until_ms: i64,
    /// A check runs now.
    checking: bool,
}

/// What a waiting run says: the reading, the largest consumers and that it starts by itself.
fn wait_reason(usage: DiskUsage, consumers: &str) -> String {
    let largest = if consumers.is_empty() {
        String::new()
    } else {
        format!(" (largest: {consumers})")
    };
    format!(
        "waiting for disk space: the sandbox uses {} of its {} quota{largest}; the run starts by \
         itself once usage is under the quota",
        human(usage.used),
        human(usage.limit)
    )
}

impl App {
    /// Whether runs of `sandbox` may start as far as its disk goes. Over the quota it starts a
    /// check in the background when one is due, and answers `false` until a check finds room.
    pub(super) async fn ci_disk_room(self: &Arc<Self>, sandbox: &SandboxId) -> bool {
        let Some(isolation) = self.isolation.get() else {
            return true;
        };
        let now = self.now().0;
        let waiting = self
            .ci
            .disk_waits
            .lock()
            .ok()
            .and_then(|waits| waits.get(sandbox.as_str()).copied());
        if waiting.is_some_and(|wait| wait.checking || now < wait.until_ms) {
            return false;
        }
        let Ok(Some(record)) = self.deps.store.sandbox(sandbox).await else {
            return true;
        };
        let over = isolation
            .disk(&self.sandbox_ref(&record))
            .await
            .is_some_and(|usage| usage.used > usage.limit);
        let Ok(mut waits) = self.ci.disk_waits.lock() else {
            return true;
        };
        if !over {
            waits.remove(sandbox.as_str());
            return true;
        }
        waits.insert(
            sandbox.as_str().to_owned(),
            DiskWait {
                until_ms: 0,
                checking: true,
            },
        );
        drop(waits);
        let app = Arc::clone(self);
        tokio::spawn(async move { app.ci_disk_check(&record).await });
        false
    }

    /// Frees what upkeep may, measures again, and either lets the runs start or notes why they
    /// wait (CI-302).
    async fn ci_disk_check(self: &Arc<Self>, sandbox: &SandboxRecord) {
        let reference = self.sandbox_ref(sandbox);
        let freed = self
            .disk_upkeep(sandbox, true)
            .await
            .is_ok_and(|report| report.freed > 0 || report.worktrees > 0);
        let usage = match self.isolation.get() {
            // Upkeep measured again after it freed something.
            Some(isolation) if freed => isolation.disk(&reference).await,
            Some(isolation) => isolation.measure_disk(&reference).await,
            None => None,
        };
        let over = usage.filter(|usage| usage.used > usage.limit);
        if let Some(usage) = over {
            let reason = wait_reason(usage, &self.disk_consumers(sandbox).await);
            self.ci_note_disk_wait(&sandbox.id, &reason).await;
        }
        if let Ok(mut waits) = self.ci.disk_waits.lock() {
            if over.is_some() {
                let until_ms =
                    self.now().0 + i64::try_from(CI_DISK_RECHECK.as_millis()).unwrap_or(i64::MAX);
                waits.insert(
                    sandbox.id.as_str().to_owned(),
                    DiskWait {
                        until_ms,
                        checking: false,
                    },
                );
            } else {
                waits.remove(sandbox.id.as_str());
            }
        }
        self.ci_wake();
    }

    /// Writes `reason` on the sandbox's queued runs and publishes the ones it changed.
    async fn ci_note_disk_wait(&self, sandbox: &SandboxId, reason: &str) {
        let Ok(active) = self.deps.store.active_ci_runs().await else {
            return;
        };
        for run in active.into_iter().filter(|run| {
            run.sandbox_id == *sandbox
                && run.state == RunState::Queued
                && run.reason.as_deref() != Some(reason)
        }) {
            let note = CiRunUpdate {
                reason: Some(reason.to_owned()),
                ..CiRunUpdate::default()
            };
            if matches!(
                self.deps
                    .store
                    .transition_ci_run(&run.id, &[RunState::Queued], RunState::Queued, &note)
                    .await,
                Ok(true)
            ) {
                tracing::info!(run = %run.id, "CI run waits for disk space");
                self.ci_publish(&run.id).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_run_says_how_full_the_sandbox_is_and_who_holds_the_space() {
        let usage = DiskUsage {
            used: 126_000_000_000,
            limit: 100_000_000_000,
        };
        let reason = wait_reason(usage, "task #26 35.0 GB, CI job checkouts 14.0 GB");
        assert!(
            reason.starts_with("waiting for disk space: the sandbox uses 126.0 GB of its 100.0 GB"),
            "{reason}"
        );
        assert!(reason.contains("(largest: task #26 35.0 GB, CI job checkouts 14.0 GB)"));
        assert!(reason.ends_with("starts by itself once usage is under the quota"));
        assert!(!wait_reason(usage, "").contains("largest"));
    }
}
