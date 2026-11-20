//! Sandbox (project) queries and views (SPEC-001 §5.2). Creation/import lives with the Git and
//! GitHub integration.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tokio::time::Instant;
use yulius_contracts::api::tasks::ParkedPr;

use yulius_contracts::api::sandboxes::{Profile, RepoRef, Sandbox, SandboxCounts};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::task::{AttentionFacts, PullSignal, TaskGroup, TaskStatus};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::AppResult;
use crate::model::{SandboxRecord, TaskRecord};

/// Concurrent parked PR reads per list and active local Git workers per application.
pub(crate) const PARKED_READS: usize = 8;

/// All parked PRs rendered by one list share the same remote lookup deadline.
pub(crate) fn parked_pr_deadline() -> Instant {
    Instant::now() + std::time::Duration::from_millis(750)
}

/// Reserve a share of the remaining deadline for each unvisited batch.
pub(crate) fn fair_batches<T>(
    rows: &[T],
    deadline: Instant,
    concurrency: usize,
) -> impl Iterator<Item = (&[T], Instant)> {
    let mut batches = rows.chunks(concurrency);
    std::iter::from_fn(move || {
        let batch = batches.next()?;
        let now = Instant::now();
        let remaining = u32::try_from(batches.len() + 1).unwrap_or(u32::MAX);
        Some((
            batch,
            now + deadline.saturating_duration_since(now) / remaining,
        ))
    })
}

/// Run at most eight reads together and reserve time for later rows. A stalled
/// GitHub response can consume its batch's share, never the whole list's budget.
pub(crate) async fn fair_rows<'a, T, R, F>(
    rows: &'a [T],
    deadline: Instant,
    concurrency: usize,
    read: impl Fn(&'a T, Instant) -> F,
) -> AppResult<Vec<R>>
where
    F: std::future::Future<Output = AppResult<R>>,
{
    let mut results = Vec::with_capacity(rows.len());
    for (batch, until) in fair_batches(rows, deadline, concurrency) {
        results
            .extend(futures::future::try_join_all(batch.iter().map(|row| read(row, until))).await?);
    }
    Ok(results)
}

/// What each sandbox's Git overview last said about its tasks' pull requests (GIT-001): the same
/// facts the web UI's sidebar reads, so the "needs you" counts agree with it without calling
/// GitHub for every sandbox list. Replaced per sandbox on every overview read; bounded by the
/// sandboxes' tasks; empty after a restart until the next overview read.
#[derive(Debug, Default)]
pub(crate) struct ObservedPulls(Mutex<HashMap<SandboxId, HashMap<TaskId, ObservedPull>>>);

#[derive(Debug, Clone, Copy)]
pub(crate) struct ObservedPull {
    pub(crate) number: u64,
    pub(crate) signal: PullSignal,
}

impl ObservedPulls {
    /// Records one overview read of a sandbox.
    pub(crate) fn record(&self, sandbox: &SandboxId, pulls: HashMap<TaskId, ObservedPull>) {
        if let Ok(mut map) = self.0.lock() {
            map.insert(sandbox.clone(), pulls);
        }
    }

    /// The last observed pull request of a task: `Some(None)` when the overview listed the task
    /// without one, `None` when its sandbox was not read since the server started.
    pub(crate) fn get(&self, sandbox: &SandboxId, task: &TaskId) -> Option<Option<ObservedPull>> {
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
        self.sandbox_view_until(record, access, parked_pr_deadline())
            .await
    }

    async fn sandbox_view_until(
        &self,
        record: &SandboxRecord,
        access: AccessLevel,
        deadline: Instant,
    ) -> AppResult<Sandbox> {
        let tasks = self.deps.store.tasks(&record.id, false).await?;
        let requested = self.browser_requests(&record.id).await;
        let mut counts = SandboxCounts::default();
        let facts = fair_rows(&tasks, deadline, PARKED_READS, |task, until| {
            self.task_attention_facts_until(task, &requested, until)
        })
        .await?;
        for row in facts {
            match row.group() {
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
        self.task_attention_facts_until(task, requested, parked_pr_deadline())
            .await
    }

    pub(crate) async fn task_attention_facts_until(
        &self,
        task: &TaskRecord,
        requested: &HashSet<String>,
        deadline: Instant,
    ) -> AppResult<AttentionFacts> {
        if task.status == TaskStatus::Parked {
            return Ok(self.parked_attention(task, requested, deadline).await?.0);
        }
        self.local_attention_facts(task, requested).await
    }

    /// Check the recorded and branch PRs before reporting Done, even before
    /// anyone opens the sandbox Git overview. A missed deadline leaves work open.
    pub(crate) async fn parked_attention(
        &self,
        task: &TaskRecord,
        requested: &HashSet<String>,
        deadline: Instant,
    ) -> AppResult<(AttentionFacts, Option<ParkedPr>)> {
        use crate::delivery::{ParkedPullRead, PrVerification};
        let store = &self.deps.store;
        let mut facts = self.local_attention_facts(task, requested).await?;
        let record = store.delivery(&task.id).await?;
        let mut observed = ParkedPullRead::recorded(record.as_ref());
        if facts.plan_open
            || facts.spec_unfinished
            || facts.goal_unfinished
            || facts.held_prompt
            || facts.browser_request
        {
            return Ok((facts, observed.pr));
        }
        let check = async {
            let Some(prs) = self
                .verify_task_prs(
                    task,
                    record.as_ref(),
                    PrVerification::Display,
                    &mut observed,
                )
                .await?
            else {
                return Ok(false);
            };
            self.parked_evidence_current(task, record.as_ref(), &prs, PrVerification::Display)
                .await
        };
        facts.pull = match tokio::time::timeout_at(deadline, check).await {
            Ok(Ok(true)) => observed.pr.map(|pr| {
                if pr.open {
                    PullSignal::Tested
                } else {
                    PullSignal::Merged
                }
            }),
            _ => Some(observed.signal.unwrap_or(PullSignal::Errored)),
        };
        Ok((facts, observed.pr))
    }

    pub(crate) async fn local_attention_facts(
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
                Some(observed) => observed.map(|pr| pr.signal),
                // Not read since the server started: a merge recorded earlier is still known.
                None => store
                    .delivery(&task.id)
                    .await?
                    .filter(|record| crate::delivery::past_merge(&record.stage))
                    .map(|_| PullSignal::Merged),
            };
            let plan_open = plan_open(&store.steps(&task.id).await?);
            let spec_unfinished = if task.status == TaskStatus::Parked {
                self.read_spec_unfinished(&task.id).await.unwrap_or(true)
            } else {
                self.spec_unfinished(&task.id).await
            };
            (pull, plan_open, spec_unfinished)
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
        let visible = self.visible_sandboxes(&ctx.user).await?;
        fair_rows(
            &visible,
            parked_pr_deadline(),
            1,
            |(record, level), until| self.sandbox_view_until(record, *level, until),
        )
        .await
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
