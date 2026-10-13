//! Supertasks (SPEC-071, ADR-0054): a task that waits for the running work of its project, then
//! runs alone while every other task of the project waits for it.
//!
//! The rule is applied where every run starts ([`App::start_run`]): a task it holds stays (or
//! becomes) `queued`, and the queue offers it a slot again on its next pass. Live runs are never
//! touched. A supertask is **pending** from the moment it waits to start until it is Done or its
//! run ended; a task is **working** while it has a live run and is not Done (SES-101).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

use crate::context::App;
use crate::error::AppResult;
use crate::model::TaskRecord;

/// One other task of the project, as the rule sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Member {
    /// Task number.
    pub number: u32,
    /// Whether it is a supertask.
    pub supertask: bool,
    /// A supertask that waits to start or runs and is not Done.
    pub pending: bool,
    /// A live run that is not Done.
    pub working: bool,
}

/// The task numbers that keep a task from starting a run now, ascending; empty when nothing
/// holds it. Any task waits for the project's pending supertasks — a supertask only for those
/// with a lower number — and a supertask also waits for every working task (SUP-002, SUP-003).
/// `others` excludes the task itself.
pub(crate) fn holders(number: u32, supertask: bool, others: &[Member]) -> Vec<u32> {
    let mut found: Vec<u32> = others
        .iter()
        .filter(|m| {
            let first = m.supertask && m.pending && (!supertask || m.number < number);
            first || (supertask && m.working)
        })
        .map(|m| m.number)
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// The order in which the queue offers free slots (SUP-004): supertasks first, then the rest,
/// each in the queue's own order.
pub(crate) fn start_order(mut queued: Vec<TaskRecord>) -> Vec<TaskRecord> {
    queued.sort_by_key(|task| !task.supertask);
    queued
}

/// Statuses in which a supertask is pending (unless it is Done); a stopping one until its
/// process is reaped.
fn waits_or_runs(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Queued
            | TaskStatus::Starting
            | TaskStatus::Running
            | TaskStatus::Switching
            | TaskStatus::WaitingCapacity
            | TaskStatus::Stopping
    )
}

/// The holders last seen per held task, so a task is published when what it waits for changes
/// (SUP-006) and not on every pass of the queue (bounded by the queued tasks); and the queue's
/// single flight: one pass at a time, and one more when asked during it.
#[derive(Default)]
pub(crate) struct Holds {
    last: Mutex<HashMap<TaskId, Vec<u32>>>,
    draining: AtomicBool,
    again: AtomicBool,
    capacity_held: AtomicBool,
}

impl Holds {
    /// Claims the queue pass; false when this caller runs it. When a pass already runs, it is
    /// asked to go once more and this returns true.
    pub(crate) fn begin_drain(&self) -> bool {
        if self.draining.swap(true, Ordering::SeqCst) {
            self.again.store(true, Ordering::SeqCst);
            return true;
        }
        self.again.store(false, Ordering::SeqCst);
        false
    }

    /// Ends a pass; true when the caller must run another one (asked during the pass).
    pub(crate) fn end_drain(&self) -> bool {
        if self.again.swap(false, Ordering::SeqCst) {
            return true;
        }
        self.draining.store(false, Ordering::SeqCst);
        // Asked between the check and the release, and nobody claimed the pass since: go on.
        self.again.swap(false, Ordering::SeqCst) && !self.draining.swap(true, Ordering::SeqCst)
    }

    /// Records that a capacity resume was held, so the next queue pass wakes the resume loop.
    pub(crate) fn capacity_held(&self) {
        self.capacity_held.store(true, Ordering::SeqCst);
    }

    /// Whether a capacity resume was held since the last call.
    pub(crate) fn take_capacity_held(&self) -> bool {
        self.capacity_held.swap(false, Ordering::SeqCst)
    }

    /// Forgets the tasks that are no longer queued.
    pub(crate) fn keep(&self, queued: &HashSet<TaskId>) {
        self.last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|task, _| queued.contains(task));
    }

    /// Records `holders` for `task`; true when they differ from the last ones.
    fn record(&self, task: &TaskId, holders: &[u32]) -> bool {
        let mut map = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if holders.is_empty() {
            return map.remove(task).is_some();
        }
        if map.get(task).is_some_and(|last| last == holders) {
            return false;
        }
        map.insert(task.clone(), holders.to_vec());
        true
    }
}

impl App {
    /// The task numbers that keep `task` from starting a run now (see [`holders`]).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn supertask_holders(&self, task: &TaskRecord) -> AppResult<Vec<u32>> {
        let store = &self.deps.store;
        let project = store.tasks(&task.sandbox_id, false).await?;
        // Most projects have no supertask: nothing to read further.
        if !task.supertask && !project.iter().any(|t| t.supertask && t.id != task.id) {
            return Ok(Vec::new());
        }
        let live: HashSet<TaskId> = if task.supertask {
            store
                .live_attempts()
                .await?
                .into_iter()
                .map(|a| a.task_id)
                .collect()
        } else {
            HashSet::new()
        };
        let mut others = Vec::new();
        for other in project.iter().filter(|t| t.id != task.id) {
            let pending = other.supertask && waits_or_runs(other.status);
            let working = live.contains(&other.id);
            if !pending && !working {
                continue;
            }
            let done = self.task_is_done_locally(other).await;
            others.push(Member {
                number: other.number,
                supertask: other.supertask,
                pending: pending && !done,
                working: working && !done,
            });
        }
        Ok(holders(task.number, task.supertask, &others))
    }

    /// Whether a supertask keeps `task` from starting a run now (SUP-002, SUP-003). A queued
    /// task is published when what it waits for changed, so its label follows (SUP-006).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn held_by_supertask(&self, task: &TaskRecord) -> AppResult<bool> {
        let holders = self.supertask_holders(task).await?;
        if self.supertask_holds.record(&task.id, &holders) && task.status == TaskStatus::Queued {
            self.publish_task(&task.id).await;
        }
        Ok(!holders.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(number: u32, supertask: bool, pending: bool, working: bool) -> Member {
        Member {
            number,
            supertask,
            pending,
            working,
        }
    }

    #[test]
    fn a_plain_task_waits_only_for_pending_supertasks() {
        let others = [
            member(1, false, false, true),
            member(2, true, true, false),
            member(3, true, false, false),
            member(4, true, true, true),
        ];
        assert_eq!(holders(9, false, &others), vec![2, 4]);
        assert!(holders(9, false, &[member(1, false, false, true)]).is_empty());
    }

    #[test]
    fn a_supertask_waits_for_working_tasks_and_earlier_supertasks() {
        let others = [
            member(1, false, false, true),
            member(2, false, false, false),
            member(3, true, true, false),
            member(8, true, true, false),
            member(9, true, true, true),
        ];
        // #8 is later and not working: it waits for #5, not the other way round.
        assert_eq!(holders(5, true, &others), vec![1, 3, 9]);
        assert!(holders(5, true, &[member(2, false, false, false)]).is_empty());
    }

    #[test]
    fn done_or_ended_tasks_hold_nothing() {
        // A Done or ended task is neither pending nor working.
        let others = [
            member(1, true, false, false),
            member(2, false, false, false),
        ];
        assert!(holders(3, true, &others).is_empty());
        assert!(holders(3, false, &others).is_empty());
    }

    fn id(text: &str) -> TaskId {
        TaskId::parse(text).unwrap_or_else(|_| unreachable!())
    }

    fn task(text: &str, supertask: bool) -> TaskRecord {
        use yulius_domain::ids::{SandboxId, UserId};
        use yulius_domain::time::Timestamp;
        TaskRecord {
            id: id(text),
            sandbox_id: SandboxId::parse("s1").unwrap_or_else(|_| unreachable!()),
            number: 1,
            title: text.into(),
            prompt: text.into(),
            harness: yulius_domain::harness::Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Queued,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: crate::model::TaskOrigin::Ui,
            automation_run: None,
            created_by: UserId::parse("u1").unwrap_or_else(|_| unreachable!()),
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: crate::model::TaskMode::Goal,
            supertask,
        }
    }

    #[test]
    fn supertasks_are_offered_slots_first() {
        let order: Vec<String> = start_order(vec![
            task("t-a", false),
            task("t-b", true),
            task("t-c", false),
            task("t-d", true),
        ])
        .into_iter()
        .map(|t| t.id.to_string())
        .collect();
        assert_eq!(order, ["t-b", "t-d", "t-a", "t-c"]);
    }

    #[test]
    fn holds_report_changes_only() {
        let holds = Holds::default();
        let a = id("t-a");
        assert!(holds.record(&a, &[1]));
        assert!(!holds.record(&a, &[1]));
        assert!(holds.record(&a, &[1, 2]));
        assert!(holds.record(&a, &[]));
        assert!(!holds.record(&a, &[]));
    }

    #[test]
    fn one_queue_pass_at_a_time_and_one_more_when_asked() {
        let holds = Holds::default();
        assert!(!holds.begin_drain(), "the first caller runs the pass");
        assert!(holds.begin_drain(), "a second caller asks for another pass");
        assert!(holds.begin_drain());
        assert!(holds.end_drain(), "one more pass, however many asked");
        assert!(!holds.end_drain(), "then the pass is released");
        assert!(!holds.begin_drain(), "and the next caller runs it");
        assert!(!holds.end_drain());
    }
}
