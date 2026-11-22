//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629, ADR-0461).
//!
//! Every [`REBALANCE_EVERY`] a pass ([`App::rebalance_now`]) looks at the running tasks whose
//! live run sits on a subscription past its harness's rebalance threshold and moves each idle one
//! to the best subscription below it (`rebalance::idle_move`), through a person's move
//! ([`App::move_task`]) and only when a restart loses nothing (`agent_cli::kept_idle`). A task
//! mid-turn, switching or paused is not looked at; it is looked at again in the next pass.
//!
//! A person's choice of a subscription pins the task to it ([`App::pin_task`]): the rebalance
//! never moves a task running on its pin, and a parked task resumes on it while it can take the
//! task ([`App::pinned_start`]).

use std::sync::Arc;
use std::time::Duration;

use yulius_domain::agent_cli::{IdleFacts, kept_idle};
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::rebalance::{IdleMove, idle_move};
use yulius_domain::rotation::{self, Decision, Trigger};
use yulius_domain::task::TaskStatus;

use crate::context::App;
use crate::model::TaskRecord;

/// How often the rebalance pass runs (SPEC-629 RBL-003): as often as the background usage round.
pub const REBALANCE_EVERY: Duration = Duration::from_secs(2 * 60);
/// The first pass after a start: once the tasks that went idle while Yulius was down are woken
/// (SPEC-358 MSB-007), as for parking.
pub const REBALANCE_FIRST: Duration = Duration::from_secs(3 * 60);
const _: () = assert!(REBALANCE_FIRST.as_secs() > crate::outage::WAKE_GRACE.as_secs());
/// How long a pass waits for a task's transcript to be read before it looks at its queue.
const LINES_READ_WAIT: Duration = Duration::from_secs(5);

/// What one rebalance pass did (SPEC-629 RBL-003, RBL-007).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RebalancePass {
    /// Idle tasks moved to a subscription below the threshold.
    pub moved: u32,
    /// Idle tasks that stay because no other subscription is below the threshold.
    pub no_room: u32,
    /// Idle tasks kept where they are because a restart would lose something, or the move was
    /// refused.
    pub kept: u32,
    /// Tasks on a subscription past the threshold that their person pinned there.
    pub pinned: u32,
}

/// What the pass did with one task.
enum Looked {
    Nothing,
    Moved,
    NoRoom,
    Kept,
    Pinned,
}

/// The switch reason of a rebalance move: `rebalance: <subscription> <how full it is>`, as the
/// task's info lists it. It names no reason a switch acts on (`manual`, `handover`, …).
fn move_reason(label: &str, said: &str) -> String {
    format!("rebalance: {label} {said}")
}

impl App {
    /// Starts the rebalance pass: the first after [`REBALANCE_FIRST`], then every
    /// [`REBALANCE_EVERY`] (SPEC-629 RBL-003).
    pub fn start_rebalance_loop(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(REBALANCE_FIRST).await;
            let mut every = tokio::time::interval(REBALANCE_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                let pass = app.rebalance_now().await;
                if pass != RebalancePass::default() {
                    tracing::info!(?pass, "rebalance pass");
                }
            }
        });
    }

    /// One rebalance pass over the running tasks, one move after the other (SPEC-629 RBL-003).
    /// Public for tests, which run it instead of waiting for the clock.
    pub async fn rebalance_now(self: &Arc<Self>) -> RebalancePass {
        let mut pass = RebalancePass::default();
        // An instance update holds every run: nothing is restarted under it.
        if self.update_holds_starts() {
            return pass;
        }
        let Ok(running) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Running])
            .await
        else {
            return pass;
        };
        for task in running {
            match self.rebalance_task(&task).await {
                Looked::Nothing => {}
                Looked::Moved => pass.moved += 1,
                Looked::NoRoom => pass.no_room += 1,
                Looked::Kept => pass.kept += 1,
                Looked::Pinned => pass.pinned += 1,
            }
        }
        pass
    }

    async fn rebalance_task(self: &Arc<Self>, task: &TaskRecord) -> Looked {
        // Cheap facts first: most running tasks work. Mid-turn (working, or a permission prompt
        // that needs attention) is never moved.
        if !task.activity.is_some_and(|a| a.is_idle()) {
            return Looked::Nothing;
        }
        let store = &self.deps.store;
        let Ok(Some(run)) = store.live_attempt(&task.id).await else {
            return Looked::Nothing;
        };
        let Some(current) = run.account_id.clone() else {
            return Looked::Nothing;
        };
        let Ok(input) = self.rotation_input(task, Some(current.clone())).await else {
            return Looked::Nothing;
        };
        let decision = idle_move(&input);
        if decision == IdleMove::Stay {
            return Looked::Nothing;
        }
        // A pin holds while the task runs on it; one that names another subscription ended
        // when the task left it (SPEC-629 RBL-005).
        match store.task_pin(&task.id).await {
            Ok(Some(pin)) if pin == current => return Looked::Pinned,
            Ok(Some(_)) => self.pin_task(&task.id, None).await,
            _ => {}
        }
        let (account, from) = match decision {
            IdleMove::Move { account, from } => (account, from),
            IdleMove::NoRoom { .. } => return Looked::NoRoom,
            IdleMove::Stay => return Looked::Nothing,
        };
        if let Some(why) = self.idle_kept_by(task).await {
            tracing::info!(task = %task.id, why, "rebalance: the idle task stays for now");
            return Looked::Kept;
        }
        // Still idle on the same run: no prompt arrived while this looked.
        let still = store.task(&task.id).await.ok().flatten().is_some_and(|t| {
            t.status == TaskStatus::Running && t.activity.is_some_and(|a| a.is_idle())
        }) && matches!(store.live_attempt(&task.id).await, Ok(Some(live)) if live.id == run.id);
        if !still {
            return Looked::Kept;
        }
        let label = match store.account(&current).await {
            Ok(Some(record)) => record.label,
            _ => current.as_str().to_owned(),
        };
        let reason = move_reason(&label, &from.said());
        match self.move_task(&task.id, Some(account), &reason).await {
            Ok(()) => Looked::Moved,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "rebalance: the move was refused");
                Looked::Kept
            }
        }
    }

    /// What keeps an idle task's session from a restart now (`agent_cli::kept_idle`), or its
    /// person's own goal in a prompt-mode session, which a restart could lose; `None` when a
    /// move loses nothing.
    async fn idle_kept_by(&self, task: &TaskRecord) -> Option<&'static str> {
        if self.keeps_own_goal(task) {
            return Some("its person's own goal is open in it");
        }
        let id = &task.id;
        // Its last lines are read first: they say what waits in Claude Code's queue.
        if tokio::time::timeout(LINES_READ_WAIT, self.await_transcript_read(id))
            .await
            .is_err()
        {
            return Some("its lines were not read yet");
        }
        let facts = IdleFacts {
            typed_ms_ago: self.typed_ms_ago(id),
            draft: self.has_draft(id),
            switch_refused: self.switch_cooling(id) || self.claude_move_cooling(id),
            paused: self.is_paused_by_person(id).await,
            update_holds: self.update_holds_starts(),
            background: self.background_or_wait_now(id).await,
            queued: self.claude_queue_holds(id),
            prompt_unread: self.prompt_just_went_on(id),
        };
        kept_idle(&facts)
    }

    /// Records a person's choice for `task`: pinned to `account`, or to none when they let the
    /// pool choose (SPEC-629 RBL-005). A failed write is logged; the move itself stands.
    pub(crate) async fn pin_task(&self, task: &TaskId, account: Option<&AccountId>) {
        if let Err(err) = self.deps.store.set_task_pin(task, account).await {
            tracing::warn!(task = %task, error = %err, "the task's pin could not be written");
        }
    }

    /// The subscription a parked `task` resumes on: its pin while the pin can take it (a
    /// person's choice is honoured below 100 %, ROT-007), else `None`, the pool's placement
    /// (SPEC-629 RBL-004, RBL-005).
    pub(crate) async fn pinned_start(self: &Arc<Self>, task: &TaskRecord) -> Option<AccountId> {
        let pin = self.deps.store.task_pin(&task.id).await.ok().flatten()?;
        let input = self.rotation_input(task, None).await.ok()?;
        let asked = Trigger::Manual {
            account: pin.clone(),
        };
        match rotation::decide(&input, asked).0 {
            Decision::Switch { .. } => Some(pin),
            Decision::Stay | Decision::Wait { .. } => None,
        }
    }
}
