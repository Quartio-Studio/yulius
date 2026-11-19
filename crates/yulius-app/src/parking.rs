//! Parked sessions (SPEC-512 part 1, #1412, ADR-0367): a task whose turn ended and that waits for
//! a person gives its idle session up after a grace period and resumes in that session when
//! someone writes to it.
//!
//! Contract:
//! - only a `running` task whose agent is idle (`waiting` or `done`), which the list shows under
//!   Needs you or Done and in which nothing goes on by itself or waits on a keyboard, is parked
//!   ([`ParkFacts::parkable`]); the grace period runs from the last change of its status or
//!   activity (`state_at`); 0 turns parking off;
//! - the decision is taken again under the launch lock ([`App::park_task_now`]), so a start, a
//!   turn or a message that came meanwhile wins; the run's end makes the task `parked`;
//! - a parked task holds no session: [`App::sessions_in_use`] counts live runs only;
//! - every park is audited `task.parked` by `yulius`, without the task's words.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::json;
use yulius_domain::ids::SandboxId;
use yulius_domain::task::{Activity, TaskGroup, TaskStatus};

use crate::context::App;
use crate::model::{TaskOrigin, TaskRecord};

/// The grace period an idle session keeps before it is parked unless the unit says otherwise
/// (`YULIUS_PARK_AFTER_MINUTES`): 30 minutes.
pub const DEFAULT_PARK_AFTER_MS: u64 = 30 * 60 * 1000;

/// A sandbox's pending requests for a person: all of them (`needs_you` too), which decide the
/// list's group, and those in its browser, which keep a session (the agent goes on after them).
#[derive(Debug, Clone, Default)]
struct Requests {
    all: HashSet<String>,
    browser: HashSet<String>,
}

/// Who parks a session, in the audit.
pub const PARKED_BY: &str = "yulius";

/// What decides whether a running task's session is parked now (SPEC-512 PRK-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParkFacts {
    /// The grace period; 0: parking is off.
    pub park_after_ms: u64,
    /// Since when its status and activity are what they are.
    pub quiet_ms: i64,
    /// Its status.
    pub status: TaskStatus,
    /// Its agent's activity.
    pub activity: Option<Activity>,
    /// The list's group.
    pub group: TaskGroup,
    /// A turn of its session is known to run.
    pub turn: bool,
    /// Background shells or subagents it left, or dynamic workflows not ended: it wakes by
    /// itself.
    pub background: bool,
    /// It waits in `yulius wait`, has a merge armed, or Yulius has news of its pull request due.
    pub yulius: bool,
    /// A prompt typed into it is held for a person's decision (TSK-013).
    pub held_prompt: bool,
    /// It asked a person to act in the sandbox browser.
    pub browser_request: bool,
    /// A person or an update paused it.
    pub paused: bool,
    /// A person typed into its terminal and sent nothing, or typed within the grace period.
    pub keyboard: bool,
    /// The onboarding task of its sandbox, whose run's end decides the onboarding.
    pub onboarding: bool,
}

impl ParkFacts {
    /// Whether the session is parked now.
    #[must_use]
    pub fn parkable(&self) -> bool {
        self.park_after_ms > 0
            && i64::try_from(self.park_after_ms).is_ok_and(|after| self.quiet_ms >= after)
            && self.status == TaskStatus::Running
            && self.activity.is_some_and(Activity::is_idle)
            && matches!(self.group, TaskGroup::NeedsYou | TaskGroup::Done)
            && !self.turn
            && !self.background
            && !self.yulius
            && !self.held_prompt
            && !self.browser_request
            && !self.paused
            && !self.keyboard
            && !self.onboarding
    }
}

impl App {
    /// Parks every running task whose session [`ParkFacts::parkable`] says may go (SPEC-512
    /// PRK-002); the background tick calls it every minute. Returns how many parks began.
    pub async fn park_idle_sessions_now(self: &Arc<Self>) -> usize {
        if self.config.park_after_ms == 0 {
            return 0;
        }
        let Ok(running) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Running])
            .await
        else {
            return 0;
        };
        let mut requests: HashMap<SandboxId, Requests> = HashMap::new();
        let mut parked = 0;
        for task in running {
            // Cheap facts first: most running tasks work.
            if !task.activity.is_some_and(Activity::is_idle) {
                continue;
            }
            if !requests.contains_key(&task.sandbox_id) {
                let read = self.park_requests(&task.sandbox_id).await;
                requests.insert(task.sandbox_id.clone(), read);
            }
            let requested = requests.get(&task.sandbox_id).cloned().unwrap_or_default();
            if !self.park_facts(&task, &requested).await.parkable() {
                continue;
            }
            match self.park_task_now(&task.id).await {
                Ok(true) => parked += 1,
                Ok(false) => {}
                Err(err) => tracing::warn!(task = %task.id, error = %err, "a park failed"),
            }
        }
        parked
    }

    /// [`ParkFacts::parkable`] for `task` as stored now: what [`App::park_task_now`] asks again
    /// under the launch lock.
    pub(crate) async fn parkable_now(&self, task: &TaskRecord) -> bool {
        let requested = self.park_requests(&task.sandbox_id).await;
        self.park_facts(task, &requested).await.parkable()
    }

    async fn park_requests(&self, sandbox: &SandboxId) -> Requests {
        Requests {
            all: self.browser_requests(sandbox).await,
            browser: self.browser_tab_requests(sandbox).await,
        }
    }

    async fn park_facts(&self, task: &TaskRecord, requested: &Requests) -> ParkFacts {
        let now = self.now().0;
        let park_after_ms = self.config.park_after_ms;
        let quiet_ms = match self.deps.store.task_times(&task.id).await {
            Ok(Some(times)) => now.saturating_sub(times.state_at.0),
            _ => 0,
        };
        let runtime = self.existing_runtime(&task.id);
        let keyboard = runtime.as_ref().is_some_and(|runtime| {
            let typed = runtime.typed_at_ms.load(Ordering::SeqCst);
            let emptied = runtime.box_emptied_ms.load(Ordering::SeqCst);
            yulius_domain::intake::box_held(now, typed, emptied).is_some()
                || (typed > 0
                    && i64::try_from(park_after_ms)
                        .is_ok_and(|after| now.saturating_sub(typed) < after))
        });
        let background = match &runtime {
            // A session Yulius does not hold is nothing it can end.
            None => true,
            // Shells not reported (a session resumed with nothing typed has run no turn) count
            // as none, as the list counts them.
            Some(runtime) => {
                self.background_shells_of(&task.id)
                    .is_some_and(|count| count > 0)
                    || !runtime.with_workflows(|workflows| workflows.runs().is_empty())
            }
        };
        let group = self
            .task_group(task, &requested.all)
            .await
            .unwrap_or(TaskGroup::Working);
        ParkFacts {
            park_after_ms,
            quiet_ms,
            status: task.status,
            activity: task.activity,
            group,
            turn: runtime
                .as_ref()
                .is_some_and(|runtime| runtime.turn_in_progress()),
            background,
            yulius: self.agent_waits_on_yulius(&task.id)
                || self.merge_is_armed(&task.id)
                || self.pr_news_due(task).await,
            held_prompt: self
                .deps
                .store
                .pending_hold(&task.id)
                .await
                .map_or(true, |hold| hold.is_some()),
            browser_request: requested.browser.contains(task.id.as_str()),
            paused: self.is_paused(task).await,
            keyboard,
            onboarding: task.origin == TaskOrigin::Onboarding,
        }
    }
}

/// Audits the park of `task` (`task.parked`, by [`PARKED_BY`]); a failed write is logged.
pub(crate) async fn audit_parked(app: &App, task: &TaskRecord) {
    let detail = json!({"number": task.number, "activity": task.activity.map(Activity::as_str)});
    if let Err(err) = app
        .deps
        .store
        .audit(
            app.now(),
            PARKED_BY,
            "task.parked",
            Some(task.id.as_str()),
            Some(&detail.to_string()),
        )
        .await
    {
        tracing::warn!(task = %task.id, error = %err, "a park was not audited");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> ParkFacts {
        ParkFacts {
            park_after_ms: 60_000,
            quiet_ms: 60_000,
            status: TaskStatus::Running,
            activity: Some(Activity::Waiting),
            group: TaskGroup::NeedsYou,
            turn: false,
            background: false,
            yulius: false,
            held_prompt: false,
            browser_request: false,
            paused: false,
            keyboard: false,
            onboarding: false,
        }
    }

    /// PRK-002: an idle task that waits for a person, or is Done, parks once the grace period is
    /// over.
    #[test]
    fn an_idle_task_that_waits_for_a_person_parks_after_the_grace_period() {
        assert!(idle().parkable());
        assert!(
            ParkFacts {
                activity: Some(Activity::Done),
                group: TaskGroup::Done,
                ..idle()
            }
            .parkable()
        );
        assert!(
            !ParkFacts {
                quiet_ms: 59_999,
                ..idle()
            }
            .parkable()
        );
        assert!(
            !ParkFacts {
                park_after_ms: 0,
                quiet_ms: i64::MAX,
                ..idle()
            }
            .parkable(),
            "0 turns parking off"
        );
    }

    /// PRK-002: whatever goes on by itself, asks in its terminal or waits on a keyboard keeps
    /// the session.
    #[test]
    fn what_goes_on_or_waits_on_a_keyboard_keeps_the_session() {
        let kept = [
            ParkFacts {
                activity: Some(Activity::Working),
                ..idle()
            },
            ParkFacts {
                activity: Some(Activity::NeedsAttention),
                ..idle()
            },
            ParkFacts {
                activity: None,
                ..idle()
            },
            ParkFacts {
                group: TaskGroup::Working,
                ..idle()
            },
            ParkFacts {
                status: TaskStatus::WaitingCapacity,
                ..idle()
            },
            ParkFacts {
                status: TaskStatus::Switching,
                ..idle()
            },
            ParkFacts {
                turn: true,
                ..idle()
            },
            ParkFacts {
                background: true,
                ..idle()
            },
            ParkFacts {
                yulius: true,
                ..idle()
            },
            ParkFacts {
                held_prompt: true,
                ..idle()
            },
            ParkFacts {
                browser_request: true,
                ..idle()
            },
            ParkFacts {
                paused: true,
                ..idle()
            },
            ParkFacts {
                keyboard: true,
                ..idle()
            },
            ParkFacts {
                onboarding: true,
                ..idle()
            },
        ];
        for facts in kept {
            assert!(!facts.parkable(), "{facts:?}");
        }
    }
}
