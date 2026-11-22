//! Server-lifetime background work: crash recovery and the metrics recount, queue draining, the
//! subscription pool's resume loop (ROT-010) and account usage refresh (ROT-009) (SPEC-002 §6.1,
//! §7.5, §10). Nothing is typed into an idle task's session (SPEC-028 MSR-004), except "Yulius is
//! back; continue" into a task that went idle while Yulius was down (SPEC-358 MSB-007), and
//! `/mcp reconnect yulius` into one whose Yulius MCP went silent (SPEC-432 MCR-003).

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::context::App;
use crate::error::AppResult;
use crate::metrics_recount::RecountTally;

/// Queue drain interval, also the device grant purge interval (#169).
const TICK: Duration = Duration::from_secs(60);
/// No idle session is parked this soon after a start: the tasks that went idle while Yulius was
/// down are woken first ([`crate::outage::WAKE_GRACE`] after the start, SPEC-358 MSB-007).
const PARK_AFTER_START: Duration = Duration::from_secs(3 * 60);
const _: () = assert!(PARK_AFTER_START.as_secs() > crate::outage::WAKE_GRACE.as_secs());
/// First background usage round after start: lets the server settle first (ROT-009).
const USAGE_FIRST: Duration = Duration::from_secs(2 * 60);
/// Background usage rounds: every subscription in its back-off or read moments ago is skipped,
/// so this asks the provider often without hammering it (F 2026-10-11).
const USAGE_EVERY: Duration = Duration::from_secs(2 * 60);
/// Delivery reconciliation with GitHub (pull requests merged or closed outside Yulius, #35).
const DELIVERY_EVERY: Duration = Duration::from_secs(3 * 60);
/// How often armed merges that can be decided now are decided (SPEC-441 MQ-008): an armed,
/// green, mergeable pull request merges within a minute.
const DECIDABLE_EVERY: Duration = Duration::from_secs(20);
// At least twice a minute, and more often than the delivery round (checked when compiling).
const _: () = assert!(
    DECIDABLE_EVERY.as_secs() <= 30 && DECIDABLE_EVERY.as_secs() < DELIVERY_EVERY.as_secs()
);
impl App {
    /// Reconciles the runs recorded live before this start, then starts the metrics recount
    /// (SPEC-091 RCT-004, SPEC-143 RDM-003): once the runs that did not survive the restart have
    /// ended, the runs recorded before SPEC-087 get their transcripts' figures. Without
    /// reconciliation a lost run would read as live, so nothing is recounted until a start that
    /// reconciles. Returns the recount, running in its own task, when it started.
    pub async fn recover(self: &Arc<Self>) -> Option<JoinHandle<AppResult<RecountTally>>> {
        // A move took every run with the old host: the copy's mark brings their tasks back
        // (SPEC-460 MIG-050).
        let paused = self.paused_before_a_move().await;
        match self.reconcile_lost().await {
            Ok(lost) => self.revive_after_move(&lost, &paused).await,
            Err(err) => {
                tracing::error!(error = %err, "session reconciliation failed");
                return None;
            }
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
        // When the previous server was last alive (SPEC-358 MSB-006).
        let outage = self.start_outage_watch();
        // No Superagent request is written until the saved ones are back (SPEC-487 XRQ-002).
        self.begin_cross_restore();
        // The Superagents' checks, read before anything can save them (SPEC-487 CHK-002).
        self.restore_superagent_checks();
        let checks = Arc::clone(self);
        tokio::spawn(async move {
            // A Superagent that is not running is started for a due check (CHK-004).
            let mut every = tokio::time::interval(std::time::Duration::from_secs(60));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                checks.start_superagents_for_due_checks().await;
            }
        });
        let app = Arc::clone(self);
        tokio::spawn(async move {
            // The Superagents' requests that waited, and their outcomes (SPEC-487 XRQ-002).
            app.restore_cross_requests().await;
            // Nothing waits for the recount (SPEC-091 RCT-005).
            drop(app.recover().await);
            // The runs are reattached: the tasks that went idle while Yulius was down are woken
            // once their transcripts replayed (MSB-007).
            app.schedule_outage_wakes(outage);
            // Sessions whose Yulius MCP Claude Code dropped are reconnected (SPEC-432 MCR-003).
            app.start_mcp_watch(outage.started);
            // Tasks hear what happens to their pull request (SPEC-444 PRN-001).
            app.start_pr_watch();
            // Tasks hear once that their sandbox's Docker is full, and when it no longer is
            // (SPEC-494 DFJ-007).
            app.start_docker_watch();
            // Onboarded sandboxes take the current Infra bootstrap revision (SPEC-575 MFS-012).
            // On its own task: GitHub access checks must not hold the queue (review round 1).
            if app.config.bootstrap_upgrades {
                let upgrades = Arc::clone(&app);
                tokio::spawn(async move {
                    upgrades.create_bootstrap_upgrades().await;
                });
            }
            // A handoff a stopped server left mid-creation is settled (SPEC-512 HOF-004).
            if let Err(err) = app.deps.store.settle_creating_handoffs(app.now()).await {
                tracing::warn!(error = %err, "handoffs left creating could not be settled");
            }
            // Merges armed before the restart stay armed (DLV-205), and one whose checks passed
            // meanwhile merges now (SPEC-358 MSB-008).
            app.resume_armed_merges().await;
            app.drain_queue().await;
            // Paused tasks (also those paused before a restart) resume from here.
            app.start_capacity_loop();
            let mut tick = tokio::time::interval(TICK);
            tick.tick().await;
            let started = tokio::time::Instant::now();
            loop {
                tick.tick().await;
                app.drain_queue().await;
                // Chained tasks start once their predecessor is Done (TSK-012).
                app.advance_all_chains().await;
                // Merge-first staging queues move on (SPEC-575 MFS-003 to MFS-008), on their own
                // task: GitHub and Git reads must not hold the tick.
                let staging = Arc::clone(&app);
                tokio::spawn(async move {
                    staging.staging_tick().await;
                });
                // Prepared tasks start at their time (SPEC-105 PRP-003).
                app.release_due_prepared().await;
                // Idle sessions past their grace period are parked (SPEC-512 PRK-002), once the
                // tasks that went idle while Yulius was down have been woken (MSB-007).
                if started.elapsed() >= PARK_AFTER_START {
                    app.park_idle_sessions_now().await;
                    // A handed-off task's outcome reaches its source (SPEC-512 HOF-005).
                    app.tell_handoff_outcomes().await;
                    // The day's digest, once its time passed (SPEC-512 DIG-002), on its own
                    // task: reading transcripts must not hold the queue (review round 1); the
                    // run's claim keeps a later tick from starting a second one.
                    let digest = Arc::clone(&app);
                    tokio::spawn(async move {
                        digest.digest_tick().await;
                    });
                }
                // Expired notices (SPEC-624 NTC-004): reads already leave them out.
                app.prune_notices_now().await;
                // Expired and consumed CLI device grants (#169).
                if let Err(err) = app.purge_device_grants().await {
                    tracing::warn!(error = %err, "device grants could not be purged");
                }
            }
        });
        // What the guests of revoked or expired playground links still run (SPEC-410 GST-005), on
        // its own loop: ending terminals waits, and the tick above must not.
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(TICK);
            tick.tick().await;
            loop {
                tick.tick().await;
                app.end_expired_guest_links().await;
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
                // Armed merges whose checks passed while no CI run ended here.
                app.merge_armed_everywhere().await;
            }
        });
        // Armed merges that can be decided now, between CI run ends and delivery rounds: a
        // passed train waiting for GitHub, or a green head (SPEC-441 MQ-008).
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(DECIDABLE_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            every.tick().await;
            loop {
                every.tick().await;
                app.merge_decidable_everywhere().await;
            }
        });
        // Yulius-run CI and repository hygiene (SPEC-002 ADR-21).
        self.start_ci();
        self.start_disk_upkeep();
        // Leftovers at every start (so after every update) and hourly (SPEC-215 LFT-009).
        self.start_leftovers_rounds();
        // Whether GitHub accepts this address for sign-in: after a move it may not (SPEC-466).
        self.start_github_callback_check();
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
