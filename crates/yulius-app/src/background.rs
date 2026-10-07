//! Server-lifetime background work: crash recovery, queue draining, capacity re-evaluation and
//! waking idle tasks when messages arrive (SPEC-002 §6.1, §7.5, §10).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
use crate::coordination::MessageWaker;

/// Idle time (agent waiting and nobody typing) before a message wakes a task (COM-005).
pub const WAKE_IDLE_MS: i64 = 30_000;
/// Queue drain interval.
const TICK: Duration = Duration::from_secs(60);
/// Capacity re-evaluation interval, in ticks (5 min).
const CAPACITY_TICKS: u64 = 5;
/// Text typed into an idle agent session.
const WAKE_TEXT: &str =
    "Yulius: you have new messages. Read them with the messages_inbox tool and act on them.";

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
            let mut tick = tokio::time::interval(TICK);
            tick.tick().await;
            let mut n: u64 = 0;
            loop {
                tick.tick().await;
                n = n.wrapping_add(1);
                app.drain_queue().await;
                if n.is_multiple_of(CAPACITY_TICKS)
                    && let Ok(waiting) = app
                        .deps
                        .store
                        .tasks_with_status(&[TaskStatus::WaitingCapacity])
                        .await
                {
                    for task in waiting {
                        app.retry_waiting(&task.id).await;
                    }
                }
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
    /// [`WAKE_IDLE_MS`]; retries once later when the task is not idle long enough yet.
    async fn wake_if_idle(self: &Arc<Self>, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running
            || task.activity != Some(Activity::Waiting)
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
        let remaining = idle_since + WAKE_IDLE_MS - now;
        if remaining > 0 {
            self.schedule_wake(task_id.clone(), remaining + 500);
            return;
        }
        let mut text = WAKE_TEXT.as_bytes().to_vec();
        text.push(b'\r');
        if let Err(err) = self.send_input(task_id, &text).await {
            tracing::debug!(error = %err, task = %task_id, "wake failed");
        }
    }
}
