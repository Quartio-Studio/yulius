//! The first start after a move to another host (SPEC-460 MIG-050). `yulius instance migrate`
//! marks the copy it installs; the start that finds the mark puts every task whose run was lost
//! with the old host back in the queue, with its session, owner and subscription, within the
//! session limit. An ordinary start never does (SPEC-002 §7.1): only a move takes every run at
//! once, from people who cannot continue each other's tasks.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, StopReason, TaskEvent, TaskStatus};

use crate::context::App;
use crate::needs_you::NeedsYouRecord;

/// The mark of a copy a move installed, at the data directory's top.
pub const MOVE_PENDING: &str = "move-pending.json";
/// What the first start did with the mark ([`MoveReport`]), read by the move's checklist.
pub const MOVED: &str = "moved.json";

/// What the first start after a move did with the tasks that were live.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveReport {
    /// 1.
    pub version: u32,
    /// The old public URL.
    #[serde(default)]
    pub from: Option<String>,
    /// The new one.
    #[serde(default)]
    pub to: Option<String>,
    /// Tasks put back in the queue.
    pub queued: u32,
    /// Tasks left as lost: a person had paused them or was stopping them.
    pub left: u32,
    /// Tasks that could not be queued; their owners find them under Needs you.
    pub failed: u32,
    /// When, in milliseconds since the epoch.
    pub at: i64,
}

impl MoveReport {
    /// When the first start recorded it.
    #[must_use]
    pub fn when(&self) -> yulius_domain::time::Timestamp {
        yulius_domain::time::Timestamp(self.at)
    }
}

/// The mark's facts; `None` without a mark. A mark that cannot be read, or is not JSON or not
/// even text, is still a mark (SPEC-483 MOV-002), as [`App::paused_before_a_move`] sees it.
fn read_mark(path: &Path) -> Option<MoveReport> {
    if !path.exists() {
        return None;
    }
    let bytes = std::fs::read(path).unwrap_or_default();
    let facts = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default();
    let url = |key: &str| facts.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    Some(MoveReport {
        version: 1,
        from: url("from"),
        to: url("to"),
        ..MoveReport::default()
    })
}

impl App {
    /// The tasks people had paused when a move's mark is there: read before the start reconciles
    /// its runs, which is what loses them.
    pub(crate) async fn paused_before_a_move(&self) -> HashSet<TaskId> {
        if !self.config.data_dir.join(MOVE_PENDING).exists() {
            return HashSet::new();
        }
        match self.deps.store.person_paused_tasks().await {
            Ok(paused) => paused.into_iter().map(|(task, _)| task).collect(),
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the tasks people paused");
                HashSet::new()
            }
        }
    }

    /// After a move: queues the tasks of `lost` (each with the status its run was lost in),
    /// records the result and removes the mark. Nothing without the mark.
    pub(crate) async fn revive_after_move(
        self: &Arc<Self>,
        lost: &[(TaskId, TaskStatus)],
        paused: &HashSet<TaskId>,
    ) {
        let data = &self.config.data_dir;
        let mark = data.join(MOVE_PENDING);
        let Some(mut report) = read_mark(&mark) else {
            return;
        };
        let store = &self.deps.store;
        let lost_status = TaskStatus::Stopped(StopReason::ServerLost);
        for (id, was) in lost {
            if *was == TaskStatus::Stopping || paused.contains(id) {
                report.left += 1;
                continue;
            }
            let Ok(Some(task)) = store.task(id).await else {
                continue;
            };
            // A finished task was closed, not lost; anything else changed meanwhile.
            if task.status != lost_status {
                continue;
            }
            let queued = match task.status.apply(TaskEvent::ContinueQueued) {
                Ok(next) => store
                    .transition_task(id, Some(task.status), next, None, self.now())
                    .await
                    .unwrap_or(false),
                Err(_) => false,
            };
            if queued {
                report.queued += 1;
                self.publish_task(id).await;
                continue;
            }
            report.failed += 1;
            let now = self.now();
            let record = NeedsYouRecord {
                action: "Continue this task".to_owned(),
                happened: Some(
                    "Yulius moved to another host and this task's session ended with the old one."
                        .to_owned(),
                ),
                problem: Some("It could not be put back in the queue.".to_owned()),
                decision: false,
                link: None,
                at: now.0,
            };
            if let Err(err) = store.set_task_needs_you(id, Some(&record), now).await {
                tracing::warn!(task = %id, error = %err, "could not flag a task the move left stopped");
            }
            if let Err(err) = self
                .set_task_activity(&task, Activity::NeedsAttention)
                .await
            {
                tracing::warn!(task = %id, error = %err, "could not flag a task the move left stopped");
            }
            self.publish_task(id).await;
        }
        report.at = self.now().0;
        tracing::info!(
            queued = report.queued,
            left = report.left,
            failed = report.failed,
            "the first start after a move"
        );
        let written = serde_json::to_vec(&report)
            .map_err(std::io::Error::other)
            .and_then(|json| std::fs::write(data.join(MOVED), json));
        if let Err(err) = written {
            tracing::warn!(error = %err, "could not record what the move's first start did");
        }
        // Removed last: a start that dies before this point does it again, and finds the queued
        // tasks no longer lost.
        if let Err(err) = std::fs::remove_file(&mark) {
            tracing::warn!(error = %err, "could not remove the move's mark");
        }
        self.drain_queue().await;
    }
}
