//! Server-lifetime background work: crash recovery, queue draining, the subscription pool's
//! resume loop (ROT-010) and account usage refresh (ROT-009) (SPEC-002 §6.1, §7.5, §10). Nothing
//! is typed into an idle task's session (SPEC-028 MSR-004).

use std::sync::Arc;
use std::time::Duration;

use crate::context::App;

/// Queue drain interval.
const TICK: Duration = Duration::from_secs(60);
/// First background usage round after start: lets the server settle first (ROT-009).
const USAGE_FIRST: Duration = Duration::from_secs(2 * 60);
/// Background usage rounds: every subscription in its back-off or read moments ago is skipped,
/// so this asks the provider often without hammering it (F 2026-10-11).
const USAGE_EVERY: Duration = Duration::from_secs(2 * 60);
/// Delivery reconciliation with GitHub (pull requests merged or closed outside Yulius, #35).
const DELIVERY_EVERY: Duration = Duration::from_secs(3 * 60);
impl App {
    /// Starts recovery and periodic work; call once after the listeners are bound.
    pub fn start_background(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = app.reconcile().await {
                tracing::error!(error = %err, "session reconciliation failed");
            }
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
