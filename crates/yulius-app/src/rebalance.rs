//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629, ADR-0461).
//!
//! A person's choice of a subscription pins the task to it ([`App::pin_task`]): the rebalance
//! never moves a task running on its pin, and a parked task resumes on it while it can take the
//! task ([`App::pinned_start`]).

use std::sync::Arc;

use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::rotation::{self, Decision, Trigger};

use crate::context::App;
use crate::model::TaskRecord;

impl App {
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
