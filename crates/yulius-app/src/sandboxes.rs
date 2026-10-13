//! Sandbox (project) queries and views (SPEC-001 §5.2). Creation/import lives with the Git and
//! GitHub integration.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use yulius_contracts::api::sandboxes::{Profile, RepoRef, Sandbox, SandboxCounts};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::task::{AttentionFacts, PullSignal, TaskGroup, TaskStatus};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::AppResult;
use crate::model::{SandboxRecord, TaskRecord};

/// What each sandbox's Git overview last said about its tasks' pull requests (GIT-001): the same
/// facts the web UI's sidebar reads, so the "needs you" counts agree with it without calling
/// GitHub for every sandbox list. Replaced per sandbox on every overview read; bounded by the
/// sandboxes' tasks; empty after a restart until the next overview read.
#[derive(Debug, Default)]
pub(crate) struct ObservedPulls(Mutex<HashMap<SandboxId, HashMap<TaskId, PullSignal>>>);

impl ObservedPulls {
    /// Records one overview read of a sandbox.
    pub(crate) fn record(&self, sandbox: &SandboxId, pulls: HashMap<TaskId, PullSignal>) {
        if let Ok(mut map) = self.0.lock() {
            map.insert(sandbox.clone(), pulls);
        }
    }

    /// The last observed pull request of a task: `Some(None)` when the overview listed the task
    /// without one, `None` when its sandbox was not read since the server started.
    fn get(&self, sandbox: &SandboxId, task: &TaskId) -> Option<Option<PullSignal>> {
        let map = self.0.lock().ok()?;
        map.get(sandbox).map(|pulls| pulls.get(task).copied())
    }
}

/// The default profile for new projects (SPEC-001 DLV-002).
#[must_use]
pub fn standard_profile() -> Profile {
    crate::delivery::profile_to_contract(&yulius_domain::delivery::DeliveryProfile::standard())
}

impl App {
    /// API view of a sandbox for a caller with `access`.
    ///
    /// # Errors
    /// Store failures.
    pub async fn sandbox_view(
        &self,
        record: &SandboxRecord,
        access: AccessLevel,
    ) -> AppResult<Sandbox> {
        let tasks = self.deps.store.tasks(&record.id, false).await?;
        let requested = self.browser_requests(&record.id).await;
        let mut counts = SandboxCounts::default();
        for task in &tasks {
            match self.task_group(task, &requested).await? {
                TaskGroup::NeedsYou => counts.needs_attention += 1,
                TaskGroup::Working => counts.running += 1,
                TaskGroup::Queued => counts.queued += 1,
                TaskGroup::Done | TaskGroup::Ended => {}
            }
        }
        Ok(Sandbox {
            id: record.id.to_string(),
            slug: record.slug.clone(),
            name: record.repo_name.clone(),
            repo: RepoRef {
                owner: record.repo_owner.clone(),
                name: record.repo_name.clone(),
                url: format!(
                    "https://github.com/{}/{}",
                    record.repo_owner, record.repo_name
                ),
            },
            access: access.as_str().to_owned(),
            default_branch: record.default_branch.clone(),
            profile: serde_json::from_str(&record.profile_json)
                .unwrap_or_else(|_| standard_profile()),
            onboarding: record.onboarding_state.clone(),
            port_base: record.port_base,
            port_count: record.port_count,
            counts,
        })
    }

    /// A task's sidebar group (`AttentionFacts::group`), so Home's cards and the sandbox tabs
    /// count what the sidebar lists (#147, #170). Ended tasks are answered without reading the
    /// store.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn task_group(
        &self,
        task: &TaskRecord,
        requested: &HashSet<String>,
    ) -> AppResult<TaskGroup> {
        if task.status.is_resting() || task.status == TaskStatus::Stopping {
            return Ok(TaskGroup::Ended);
        }
        let store = &self.deps.store;
        let held_prompt = store.pending_hold(&task.id).await?.is_some();
        let chain_blocked = task.status == TaskStatus::Chained
            && store
                .chain(&task.id)
                .await?
                .is_some_and(|chain| chain.blocked_reason.is_some());
        let finished = task.status == TaskStatus::Running
            && task
                .activity
                .is_some_and(yulius_domain::task::Activity::is_idle);
        // The pull request and the plan matter only after a finished turn.
        let (pull, plan_open) = if finished {
            let pull = match self.observed_pulls.get(&task.sandbox_id, &task.id) {
                Some(observed) => observed,
                // Not read since the server started: a merge recorded earlier is still known.
                None => store
                    .delivery(&task.id)
                    .await?
                    .filter(|record| crate::delivery::past_merge(&record.stage))
                    .map(|_| PullSignal::Merged),
            };
            let plan_open = pull.is_some() && plan_open(&store.steps(&task.id).await?);
            (pull, plan_open)
        } else {
            (None, false)
        };
        Ok(AttentionFacts {
            status: task.status,
            activity: task.activity,
            held_prompt,
            browser_request: requested.contains(task.id.as_str()),
            chain_blocked,
            plan_open,
            background_shells: finished
                && self
                    .background_shells_of(&task.id)
                    .is_some_and(|count| count > 0),
            pull,
        }
        .group())
    }

    /// Tasks with a pending request to act in the sandbox's browser (BRW-004), read from its
    /// tabs; none while the browser does not run (its requests went with it).
    async fn browser_requests(&self, sandbox: &SandboxId) -> HashSet<String> {
        match self.deps.browser.running(sandbox).await {
            Some(session) => session
                .tabs()
                .into_iter()
                .filter_map(|tab| tab.action_requested.and_then(|request| request.task_id))
                .collect(),
            None => HashSet::new(),
        }
    }

    /// Sandboxes visible to the caller.
    ///
    /// # Errors
    /// Store failures.
    pub async fn list_sandboxes(&self, ctx: &AuthContext) -> AppResult<Vec<Sandbox>> {
        let mut out = Vec::new();
        for (record, level) in self.visible_sandboxes(&ctx.user).await? {
            out.push(self.sandbox_view(&record, level).await?);
        }
        Ok(out)
    }

    /// One sandbox, if visible.
    ///
    /// # Errors
    /// `NotFound` when invisible.
    pub async fn get_sandbox(&self, ctx: &AuthContext, id: &str) -> AppResult<Sandbox> {
        let id = yulius_domain::ids::SandboxId::parse(id)
            .map_err(|_| crate::error::AppError::NotFound)?;
        let (record, level) = self.require(&ctx.user, &id, Action::View).await?;
        self.sandbox_view(&record, level).await
    }
}

/// Whether a plan has steps not done or skipped yet (STP-001, STP-005).
pub(crate) fn plan_open(steps: &[crate::model::StepRecord]) -> bool {
    let progress = crate::tasks::progress(steps);
    progress.total > 0 && progress.done < progress.total
}
