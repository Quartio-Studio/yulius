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

/// PR facts the Git overview supplied for a single task. A closed, unmerged PR
/// has a number and no attention signal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObservedPull {
    pub(crate) number: u64,
    pub(crate) signal: Option<PullSignal>,
}

impl ObservedPull {
    fn open(self) -> bool {
        self.signal
            .is_some_and(|signal| signal != PullSignal::Merged)
    }
}

#[derive(Debug, Default)]
struct SandboxPulls {
    revision: u64,
    // Keep confirmed absence as evidence too: an older request may still be reading an open PR.
    rows: HashMap<TaskId, RowObservation>,
}

#[derive(Debug, Default)]
struct RowObservation {
    revision: u64,
    pull: Option<ObservedPull>,
}

/// Bounded by the tasks in the latest overview. The readers publish evidence
/// under one lock after their I/O. Missing rows in a partial read say nothing.
#[derive(Debug, Default)]
pub(crate) struct ObservedPulls {
    sequence: std::sync::atomic::AtomicU64,
    sandboxes: Mutex<HashMap<SandboxId, SandboxPulls>>,
}

impl ObservedPulls {
    pub(crate) fn begin(&self) -> u64 {
        self.sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    pub(crate) fn record(
        &self,
        sandbox: &SandboxId,
        revision: u64,
        complete: bool,
        pulls: HashMap<TaskId, Option<ObservedPull>>,
    ) {
        let Ok(mut sandboxes) = self.sandboxes.lock() else {
            return;
        };
        let cached = sandboxes.entry(sandbox.clone()).or_default();
        let latest = revision >= cached.revision;
        if latest {
            cached.rows.retain(|task, _| pulls.contains_key(task));
            cached.revision = revision;
        }
        for (task, read) in pulls {
            let row = if latest {
                cached.rows.entry(task).or_default()
            } else if let Some(row) = cached.rows.get_mut(&task) {
                row
            } else {
                continue;
            };
            if revision < row.revision {
                continue;
            }
            if !complete
                && (read.is_none()
                    || row.pull.is_some_and(|prior| {
                        prior.open()
                            && read.is_some_and(|pr| !pr.open() && pr.number != prior.number)
                    }))
            {
                continue;
            }
            *row = RowObservation {
                revision,
                pull: read,
            };
        }
    }

    /// Outer None means no overview since startup. Inner None means no PR signal.
    fn get(&self, sandbox: &SandboxId, task: &TaskId) -> Option<Option<PullSignal>> {
        let sandboxes = self.sandboxes.lock().ok()?;
        sandboxes.get(sandbox).map(|cached| {
            cached
                .rows
                .get(task)
                .and_then(|row| row.pull)
                .and_then(|pr| pr.signal)
        })
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
        let mut profile: yulius_contracts::api::sandboxes::Profile =
            serde_json::from_str(&record.profile_json).unwrap_or_else(|_| standard_profile());
        // Always a value (SPEC-338 VOF-001), read on its own like the scans read it.
        profile.vidoc = Some(crate::delivery::vidoc_switched_on(record));
        profile.protect_production = Some(crate::delivery::production_protection_required(record));
        Ok(Sandbox {
            id: record.id.to_string(),
            slug: record.slug.clone(),
            name: record.repo_name.clone(),
            repo: RepoRef {
                owner: record.repo_owner.clone(),
                name: record.repo_name.clone(),
                url: if record.is_playground() {
                    String::new()
                } else {
                    format!(
                        "https://github.com/{}/{}",
                        record.repo_owner, record.repo_name
                    )
                },
            },
            playground: record.is_playground(),
            access: access.as_str().to_owned(),
            default_branch: record.default_branch.clone(),
            profile,
            onboarding: record.onboarding_state.clone(),
            port_base: record.port_base,
            port_count: record.port_count,
            counts,
        })
    }

    /// Whether the sidebar shows `task` under Done: an achieved goal, a tested pull request with
    /// its plan complete, or an agent that answered with nothing open (SPEC-429 ANS-007). Archive
    /// and delete close such a task's session, as the UI offers them.
    pub(crate) async fn shows_done(&self, task: &TaskRecord) -> bool {
        let requested = self.browser_requests(&task.sandbox_id).await;
        matches!(self.task_group(task, &requested).await, Ok(TaskGroup::Done))
    }

    /// A task's sidebar group (`AttentionFacts::group`), so Home's cards and the sandbox tabs
    /// count what the sidebar lists (#147, #170). Of an ended task only whether it is the
    /// onboarding task its sandbox waits for is read.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn task_group(
        &self,
        task: &TaskRecord,
        requested: &HashSet<String>,
    ) -> AppResult<TaskGroup> {
        Ok(self.task_attention_facts(task, requested).await?.group())
    }

    /// The facts that place the task in its sidebar group and explain a derived wait.
    pub(crate) async fn task_attention_facts(
        &self,
        task: &TaskRecord,
        requested: &HashSet<String>,
    ) -> AppResult<AttentionFacts> {
        let store = &self.deps.store;
        if task.status.is_resting() || task.status == TaskStatus::Stopping {
            let mut facts = AttentionFacts::new(task.status, task.activity);
            facts.awaits_start_onboarding =
                self.is_awaited_onboarding(task).await? && !store.has_attempts(&task.id).await?;
            return Ok(facts);
        }
        let held_prompt = store.pending_hold(&task.id).await?.is_some();
        let chain_blocked = task.status == TaskStatus::Chained
            && store
                .chain(&task.id)
                .await?
                .is_some_and(|chain| chain.blocked_reason.is_some());
        // A parked task is idle without its session (SPEC-512 PRK-004): its pull request, plan
        // and goal speak as an idle running task's do; the live overlays (shells, `yulius wait`,
        // an armed merge) need a session, so `live` keeps them off.
        let idle = task
            .activity
            .is_some_and(yulius_domain::task::Activity::is_idle);
        let finished = matches!(task.status, TaskStatus::Running | TaskStatus::Parked) && idle;
        let live = task.status == TaskStatus::Running && idle;
        // The pull request, the plan and the goal matter only after a finished turn: with no step
        // open and no unfinished goal, an idle agent answered (SPEC-429).
        let (pull, plan_open, spec_unfinished) = if finished {
            let pull = match self.observed_pulls.get(&task.sandbox_id, &task.id) {
                Some(observed) => observed,
                // Not read since the server started: a merge recorded earlier is still known.
                None => store
                    .delivery(&task.id)
                    .await?
                    .filter(|record| crate::delivery::past_merge(&record.stage))
                    .map(|_| PullSignal::Merged),
            };
            let plan_open = plan_open(&store.steps(&task.id).await?);
            (pull, plan_open, self.spec_unfinished(&task.id).await)
        } else {
            (None, false, false)
        };
        Ok(AttentionFacts {
            status: task.status,
            activity: task.activity,
            held_prompt,
            browser_request: requested.contains(task.id.as_str()),
            chain_blocked,
            plan_open,
            spec_unfinished,
            goal_unfinished: finished && self.goal_unfinished_of(task).await,
            background_shells: live
                && self
                    .background_shells_of(&task.id)
                    .is_some_and(|count| count > 0),
            yulius_wait: live
                && (self.agent_waits_on_yulius(&task.id) || self.pr_news_due(task).await),
            pull,
            paused: self.is_paused(task).await,
            awaits_start_onboarding: false,
            merge_armed: live && self.merge_is_armed(&task.id),
        })
    }

    /// Tasks with a pending request for a person to act: in the sandbox's browser (BRW-004),
    /// read from its tabs (none while the browser does not run: its requests went with it), or
    /// through `needs_you` (SPEC-134 CDT-004).
    pub(crate) async fn browser_requests(&self, sandbox: &SandboxId) -> HashSet<String> {
        let mut requested = self.browser_tab_requests(sandbox).await;
        match self.deps.store.needs_you_tasks(sandbox).await {
            Ok(tasks) => requested.extend(tasks.into_iter().map(|id| id.to_string())),
            Err(err) => tracing::warn!(error = %err, "reading needs_you requests failed"),
        }
        requested
    }

    /// Tasks with a pending request for a person to act in the sandbox's browser (BRW-004), read
    /// from its tabs: none while the browser does not run.
    pub(crate) async fn browser_tab_requests(&self, sandbox: &SandboxId) -> HashSet<String> {
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
        let (record, level) = self.require_for(ctx, &id, Action::View).await?;
        self.sandbox_view(&record, level).await
    }
}

impl App {
    /// Whether the task's spec, as Yulius last read it, says that its work is not finished
    /// (SPEC-491 DON-004): the task's own record that work it declared is left, however
    /// complete its plan reads. A task without a spec, or whose spec was never read, declares
    /// its work by its steps alone; a status that cannot be read counts as none.
    pub(crate) async fn spec_unfinished(&self, task: &yulius_domain::ids::TaskId) -> bool {
        self.read_spec_unfinished(task).await.unwrap_or(false)
    }

    /// Parking must distinguish a missing spec from a status it could not read.
    pub(crate) async fn read_spec_unfinished(
        &self,
        task: &yulius_domain::ids::TaskId,
    ) -> AppResult<bool> {
        use yulius_domain::spec_format::SpecStatus;
        let Some(status) = self.deps.store.spec_status(task).await? else {
            return Ok(false);
        };
        SpecStatus::parse(&status)
            .map(SpecStatus::work_left)
            .ok_or_else(|| crate::error::AppError::internal("unrecognized stored spec status"))
    }

    /// Keeps what the task's spec says of its work, whenever Yulius reads the spec (DON-004);
    /// `None` when the task has no spec (any more). A change is published to the pages that show
    /// the task. A failed write is logged and changes nothing.
    pub(crate) async fn note_spec_status(
        &self,
        task: &yulius_domain::ids::TaskId,
        status: Option<yulius_domain::spec_format::SpecStatus>,
    ) {
        let status = status.map(yulius_domain::spec_format::SpecStatus::as_str);
        match self.deps.store.set_spec_status(task, status).await {
            Ok(true) => self.publish_task(task).await,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(task = %task, error = %err, "keeping the spec's status failed");
            }
        }
    }
}

/// Whether a plan has steps not done or skipped yet (STP-001, STP-005).
pub(crate) fn plan_open(steps: &[crate::model::StepRecord]) -> bool {
    let progress = crate::tasks::progress(steps);
    progress.total > 0 && progress.done < progress.total
}

#[cfg(test)]
mod overview_tests {
    use super::*;

    #[test]
    fn overlapping_overviews_update_each_tasks_last_authoritative_pr_read() -> AppResult<()> {
        let cache = ObservedPulls::default();
        let sandbox = SandboxId::parse("sandbox").map_err(crate::error::AppError::internal)?;
        let task = TaskId::parse("task").map_err(crate::error::AppError::internal)?;
        let old = cache.begin();
        let next = cache.begin();
        let red = Some(ObservedPull {
            number: 2,
            signal: Some(PullSignal::Failing),
        });
        let record = |revision, complete, pull| {
            cache.record(
                &sandbox,
                revision,
                complete,
                HashMap::from([(task.clone(), pull)]),
            )
        };
        // An incomplete read with no information cannot suppress a slower complete observation.
        record(next, false, None);
        record(old, true, red);
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::Failing)));
        let landed = Some(ObservedPull {
            number: 1,
            signal: Some(PullSignal::Merged),
        });
        record(cache.begin(), false, landed);
        assert_eq!(
            cache.get(&sandbox, &task),
            Some(Some(PullSignal::Failing)),
            "an older recorded merge cannot clear the replacement"
        );
        // The owner observed this PR's closure even though the whole list was unavailable.
        record(
            cache.begin(),
            false,
            Some(ObservedPull {
                number: 2,
                signal: None,
            }),
        );
        assert_eq!(cache.get(&sandbox, &task), Some(None));
        record(next, true, red);
        assert_eq!(
            cache.get(&sandbox, &task),
            Some(None),
            "a late red report cannot reopen the confirmed closure"
        );
        record(cache.begin(), false, red);
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::Failing)));
        record(cache.begin(), true, landed);
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::Merged)));
        cache.record(&sandbox, cache.begin(), true, HashMap::new());
        record(next, false, red);
        assert_eq!(
            cache.get(&sandbox, &task),
            Some(None),
            "a late response cannot reinsert a removed task"
        );
        Ok(())
    }
}
