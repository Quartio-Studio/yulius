//! Server-lifetime background work: crash recovery, queue draining, the subscription pool's
//! resume loop (ROT-010), account usage refresh (ROT-009) and waking idle tasks when messages
//! arrive (SPEC-002 §6.1, §7.5, §10).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
use crate::coordination::MessageWaker;

/// Default idle time (agent waiting and nobody typing) before a message wakes a task (COM-005).
pub const WAKE_IDLE_MS: i64 = 30_000;
/// Queue drain interval.
const TICK: Duration = Duration::from_secs(60);
/// First background usage round after start: lets the server settle first (ROT-009).
const USAGE_FIRST: Duration = Duration::from_secs(2 * 60);
/// Background usage rounds: every subscription in its back-off or read moments ago is skipped,
/// so this asks the provider often without hammering it (F 2026-10-11).
const USAGE_EVERY: Duration = Duration::from_secs(2 * 60);
/// Delivery reconciliation with GitHub (pull requests merged or closed outside Yulius, #35).
const DELIVERY_EVERY: Duration = Duration::from_secs(3 * 60);
/// Text typed into an idle agent session.
const WAKE_TEXT: &str =
    "Yulius: you have new messages. Read them with the messages_inbox tool and act on them.";
/// Pause between the wake-up text and its Enter, so the session reads them as typing.
const WAKE_SUBMIT_PAUSE: Duration = Duration::from_millis(300);
/// A wake-up typed this recently and never submitted is still in the input box: Enter alone.
const WAKE_UNSENT_MS: i64 = 10 * 60_000;

struct IdleWaker(Weak<App>);

#[async_trait]
impl MessageWaker for IdleWaker {
    async fn message_arrived(&self, task: &TaskId) {
        if let Some(app) = self.0.upgrade() {
            app.schedule_wake(task.clone(), 0);
        }
    }
}

impl App {
    /// Starts recovery and periodic work; call once after the listeners are bound.
    pub fn start_background(self: &Arc<Self>) {
        let _ = self
            .message_waker
            .set(Arc::new(IdleWaker(Arc::downgrade(self))));
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

    fn schedule_wake(self: &Arc<Self>, task: TaskId, delay_ms: i64) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(u64::try_from(delay_ms).unwrap_or(0)))
                    .await;
            }
            app.wake_if_idle(&task).await;
        });
    }

    /// Types a short notice into a waiting agent once it and its viewers have been idle for
    /// `config.wake_idle_ms`; retries once later when the task is not idle long enough yet.
    async fn wake_if_idle(self: &Arc<Self>, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running
            || !task.activity.is_some_and(Activity::is_idle)
            || !task.wake_enabled
        {
            return;
        }
        let pending = self
            .deps
            .store
            .inbox(task_id, true, 50)
            .await
            .map(|entries| entries.iter().any(|e| !e.delivered))
            .unwrap_or(false);
        if !pending {
            return;
        }
        let now = self.now().0;
        let last_input = self.runtime(task_id).last_input_ms.load(Ordering::SeqCst);
        let idle_since = task.updated_at.0.max(last_input);
        let remaining = idle_since + self.config.wake_idle_ms - now;
        if remaining > 0 {
            self.schedule_wake(task_id.clone(), remaining + 500);
            return;
        }
        // F 2026-10-12: text and Enter written together read as a paste with a newline, so the
        // prompt never submitted, the agent never woke, and every later wake piled onto the input
        // box. The text goes first and Enter on its own; a wake still unsent needs Enter only.
        let runtime = self.runtime(task_id);
        let typed = runtime.wake_typed_ms.load(Ordering::SeqCst);
        if typed == 0 || now - typed > WAKE_UNSENT_MS {
            if let Err(err) = self.send_input(task_id, WAKE_TEXT.as_bytes()).await {
                tracing::debug!(error = %err, task = %task_id, "wake failed");
                return;
            }
            runtime.wake_typed_ms.store(now, Ordering::SeqCst);
            tokio::time::sleep(WAKE_SUBMIT_PAUSE).await;
        }
        if let Err(err) = self.send_input(task_id, b"\r").await {
            tracing::debug!(error = %err, task = %task_id, "wake failed");
        }
    }
}
