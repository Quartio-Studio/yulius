//! Server-lifetime background work: crash recovery and the metrics recount, queue draining, the
//! subscription pool's resume loop (ROT-010) and account usage refresh (ROT-009) (SPEC-002 §6.1,
//! §7.5, §10). Nothing is typed into an idle task's session (SPEC-028 MSR-004).

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::context::App;
use crate::error::AppResult;
use crate::metrics_recount::RecountTally;

/// Queue drain interval, also the device grant purge interval (#169).
const TICK: Duration = Duration::from_secs(60);
/// First background usage round after start: lets the server settle first (ROT-009).
const USAGE_FIRST: Duration = Duration::from_secs(2 * 60);
/// Background usage rounds: every subscription in its back-off or read moments ago is skipped,
/// so this asks the provider often without hammering it (F 2026-10-11).
const USAGE_EVERY: Duration = Duration::from_secs(2 * 60);
/// Delivery reconciliation with GitHub (pull requests merged or closed outside Yulius, #35).
const DELIVERY_EVERY: Duration = Duration::from_secs(3 * 60);
impl App {
    /// Reconciles the runs recorded live before this start, then starts the metrics recount
    /// (SPEC-091 RCT-004, SPEC-143 RDM-003): once the runs that did not survive the restart have
    /// ended, the runs recorded before SPEC-087 get their transcripts' figures. Without
    /// reconciliation a lost run would read as live, so nothing is recounted until a start that
    /// reconciles. Returns the recount, running in its own task, when it started.
    pub async fn recover(self: &Arc<Self>) -> Option<JoinHandle<AppResult<RecountTally>>> {
        if let Err(err) = self.reconcile().await {
            tracing::error!(error = %err, "session reconciliation failed");
            return None;
        }
        let recount = Arc::clone(self);
        Some(tokio::spawn(async move {
            let tally = recount.recount_old_metrics().await;
            if let Err(err) = &tally {
                tracing::warn!(error = %err, "the metrics recount could not start");
            }
            tally
        }))
    }

    /// Starts recovery and periodic work; call once after the listeners are bound.
    pub fn start_background(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            // Nothing waits for the recount (SPEC-091 RCT-005).
            drop(app.recover().await);
            // Merges armed before the restart stay armed (DLV-205).
            app.restore_armed_merges().await;
            app.drain_queue().await;
            // Paused tasks (also those paused before a restart) resume from here.
            app.start_capacity_loop();
            let mut tick = tokio::time::interval(TICK);
            tick.tick().await;
            loop {
                tick.tick().await;
                app.drain_queue().await;
                // Chained tasks start once their predecessor is Done (TSK-012).
                app.advance_all_chains().await;
                // Prepared tasks start at their time (SPEC-105 PRP-003).
                app.release_due_prepared().await;
                // Expired and consumed CLI device grants (#169).
                if let Err(err) = app.purge_device_grants().await {
                    tracing::warn!(error = %err, "device grants could not be purged");
                }
            }
        });
        // Deliveries in progress follow GitHub, so the Git board moves without anyone looking.
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(DELIVERY_EVERY);
            every.tick().await;
            loop {
                every.tick().await;
                app.reconcile_deliveries().await;
                // Armed merges whose checks passed while no CI run ended here (a restart).
                if let Ok(sandboxes) = app.deps.store.sandboxes().await {
                    for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
                        // A sandbox whose pass is held (a merge or a move to GitHub, which can
                        // push for minutes) is merged by whoever holds it, or on the next round:
                        // the other sandboxes never wait for it.
                        if app.armed_pass(&sandbox.id).try_lock().is_err() {
                            continue;
                        }
                        app.merge_armed(&sandbox.id).await;
                    }
                }
            }
        });
        // Yulius-run CI and repository hygiene (SPEC-002 ADR-21).
        self.start_ci();
        self.start_disk_upkeep();
        // Account usage in its own loop: a slow provider never delays queue draining.
        let app = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(USAGE_FIRST).await;
            let mut every = tokio::time::interval(USAGE_EVERY);
            loop {
                every.tick().await;
                app.refresh_usage_shared().await;
            }
        });
    }
}
