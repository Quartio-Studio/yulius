//! Why a task's live run is paused, as people see it (SPEC-191): an instance update holds it, or
//! a person paused it. A capacity pause is its own status (`waiting_capacity`, ROT-010).

use yulius_contracts::api::tasks::TaskPause;
use yulius_domain::task::TaskStatus;

use crate::context::App;
use crate::model::TaskRecord;

impl App {
    /// The pause of `task` for its summary (PAU-001): only a live run is paused.
    pub(crate) fn pause_view(&self, task: &TaskRecord) -> Option<TaskPause> {
        if !matches!(task.status, TaskStatus::Running | TaskStatus::Switching) {
            return None;
        }
        self.held_for_update(&task.id).then(|| TaskPause {
            by: "update".to_owned(),
            user: None,
            since: None,
            pending: !self.update_pause_holds(&task.id),
        })
    }
}
