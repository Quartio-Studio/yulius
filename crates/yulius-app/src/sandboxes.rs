//! Sandbox (project) queries and views (SPEC-001 §5.2). Creation/import lives with the Git and
//! GitHub integration.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use yulius_contracts::api::sandboxes::{Profile, RepoRef, Sandbox, SandboxCounts};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::task::{AttentionFacts, PullSignal, TaskGroup, TaskStatus};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::AppResult;
use crate::model::{SandboxRecord, StepRecord, TaskRecord};
use tokio::sync::OnceCell;
use tokio::time::Instant;
use yulius_contracts::api::tasks::ParkedPr;

/// Maximum concurrent reads per batch and active native Git workers per application.
pub(crate) const PARKED_READS: usize = 8;

/// Time allowed for remote verification of one parked task.
pub(crate) fn parked_pr_deadline(budget_ms: u64) -> Instant {
    Instant::now() + std::time::Duration::from_millis(budget_ms)
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

/// Display result of one bounded read. An early timeout may precede the plan lookup.
pub(crate) struct ParkedStatus {
    pub(crate) facts: AttentionFacts,
    pub(crate) pr: Option<ParkedPr>,
    pub(crate) steps: Option<Vec<StepRecord>>,
}

impl ParkedStatus {
    pub(crate) fn new(task: &TaskRecord) -> Self {
        Self {
            facts: AttentionFacts::new(task.status, task.activity),
            pr: None,
            steps: None,
        }
    }
}

/// PR facts the Git overview supplied for a single task. A closed, unmerged PR
/// has a number and no attention signal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObservedPull {
    pub(crate) number: u64,
    pub(crate) signal: Option<PullSignal>,
}

impl ObservedPull {
    pub(crate) fn open(self) -> bool {
        self.signal
            .is_some_and(|signal| signal != PullSignal::Merged)
    }
}

/// Maximum number of overlapping responses held for start-order publication per sandbox.
const OVERVIEW_WINDOW: usize = 30;

type PullRows = HashMap<TaskId, Option<ObservedPull>>;

#[derive(Debug)]
struct Overview {
    complete: bool,
    rows: PullRows,
}

impl Overview {
    fn reading(&self, task: &TaskId, prior: Option<ObservedPull>) -> Option<ObservedPull> {
        let read = *self.rows.get(task)?;
        if !self.complete
            && let Some(open) = prior.filter(|pr| pr.open())
            && read.is_none_or(|pr| !pr.open() && pr.number != open.number)
        {
            return Some(open);
        }
        read
    }
}

#[derive(Debug)]
struct Slot {
    id: u64,
    published: Option<Overview>,
}

#[derive(Debug, Default)]
struct ReadOrder {
    next: u64,
    committed: Option<HashMap<TaskId, ObservedPull>>,
    slots: VecDeque<Slot>,
}

impl ReadOrder {
    fn finish(&mut self, id: u64, result: Option<Overview>) {
        let Some(index) = self.slots.iter().position(|slot| slot.id == id) else {
            return;
        };
        if result.is_some() {
            self.slots[index].published = result;
        } else {
            self.slots.remove(index);
        }
        while self
            .slots
            .front()
            .is_some_and(|slot| slot.published.is_some())
        {
            if let Some(read) = self.slots.pop_front().and_then(|slot| slot.published) {
                let prior = self.committed.get_or_insert_with(HashMap::new);
                *prior = read
                    .rows
                    .keys()
                    .filter_map(|task| {
                        read.reading(task, prior.get(task).copied())
                            .map(|pr| (task.clone(), pr))
                    })
                    .collect();
            }
        }
    }

    fn current(&self, task: &TaskId) -> Option<Option<ObservedPull>> {
        let mut known = self.committed.as_ref().map(|rows| rows.get(task).copied());
        for slot in &self.slots {
            if let Some(read) = &slot.published {
                known = Some(read.reading(task, known.flatten()));
            }
        }
        known
    }
}

/// A bounded per-sandbox publication window. Home/MCP can read a finished
/// response immediately, including when an earlier request is still in flight.
#[derive(Debug, Default)]
pub(crate) struct ObservedPulls(Mutex<HashMap<SandboxId, Arc<Mutex<ReadOrder>>>>);

pub(crate) struct OverviewRead {
    cache: Arc<Mutex<ReadOrder>>,
    id: Option<u64>,
}

impl OverviewRead {
    pub(crate) fn record(mut self, complete: bool, rows: PullRows) {
        if let Some(id) = self.id.take()
            && let Ok(mut order) = self.cache.lock()
        {
            order.finish(id, Some(Overview { complete, rows }));
        }
    }
}

impl Drop for OverviewRead {
    fn drop(&mut self) {
        if let Some(id) = self.id.take()
            && let Ok(mut order) = self.cache.lock()
        {
            order.finish(id, None);
        }
    }
}

impl ObservedPulls {
    pub(crate) fn begin(&self, sandbox: &SandboxId) -> AppResult<OverviewRead> {
        let cache = self
            .0
            .lock()
            .map_err(crate::error::AppError::internal)?
            .entry(sandbox.clone())
            .or_default()
            .clone();
        let id = {
            let mut order = cache.lock().map_err(crate::error::AppError::internal)?;
            if order.slots.len() >= OVERVIEW_WINDOW {
                return Err(crate::error::AppError::Capacity(
                    "too many overlapping Git overview reads in this sandbox".into(),
                ));
            }
            let id = order.next;
            order.next = order.next.wrapping_add(1);
            order.slots.push_back(Slot {
                id,
                published: None,
            });
            id
        };
        Ok(OverviewRead {
            cache,
            id: Some(id),
        })
    }

    pub(crate) fn known_pull(
        &self,
        sandbox: &SandboxId,
        task: &TaskId,
    ) -> Option<Option<ObservedPull>> {
        let cache = self.0.lock().ok()?.get(sandbox)?.clone();
        cache.lock().ok()?.current(task)
    }

    fn get(&self, sandbox: &SandboxId, task: &TaskId) -> Option<Option<PullSignal>> {
        self.known_pull(sandbox, task)
            .map(|pr| pr.and_then(|pr| pr.signal))
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
        let groups = self.list_groups(&tasks).await?;
        Ok(Self::sandbox_with_groups(record, access, groups))
    }

    fn sandbox_with_groups(
        record: &SandboxRecord,
        access: AccessLevel,
        groups: Vec<TaskGroup>,
    ) -> Sandbox {
        let mut counts = SandboxCounts::default();
        for group in groups {
            match group {
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
        Sandbox {
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
        }
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

    pub(crate) fn status_deadline(&self) -> Instant {
        parked_pr_deadline(self.config.parked_pr_read_ms)
    }

    /// Verify just the parked rows, before unrelated local work can spend their budget.
    pub(crate) async fn parked_list_statuses(
        &self,
        mut parked: Vec<(&TaskRecord, &OnceCell<HashSet<String>>)>,
    ) -> HashMap<TaskId, ParkedStatus> {
        parked.retain(|(task, _)| task.status == TaskStatus::Parked);
        let mut statuses = HashMap::with_capacity(parked.len());
        for (batch, until) in fair_batches(&parked, self.status_deadline(), PARKED_READS) {
            statuses.extend(self.read_parked_batch(batch, until).await);
        }
        statuses
    }

    pub(crate) async fn read_parked_batch(
        &self,
        batch: &[(&TaskRecord, &OnceCell<HashSet<String>>)],
        deadline: Instant,
    ) -> Vec<(TaskId, ParkedStatus)> {
        let reads = batch.iter().map(|&(task, requests)| async move {
            (
                task.id.clone(),
                self.parked_status(task, requests, deadline).await,
            )
        });
        futures::future::join_all(reads).await
    }

    pub(crate) async fn list_groups(&self, tasks: &[TaskRecord]) -> AppResult<Vec<TaskGroup>> {
        let requests = OnceCell::new();
        let parked = self
            .parked_list_statuses(tasks.iter().map(|task| (task, &requests)).collect())
            .await;
        self.groups_with_parked(tasks, &requests, &parked).await
    }

    async fn groups_with_parked(
        &self,
        tasks: &[TaskRecord],
        requests: &OnceCell<HashSet<String>>,
        parked: &HashMap<TaskId, ParkedStatus>,
    ) -> AppResult<Vec<TaskGroup>> {
        let mut groups = Vec::with_capacity(tasks.len());
        for task in tasks {
            let group = if let Some(status) = parked.get(&task.id) {
                status.facts.group()
            } else {
                let requested = requests
                    .get_or_init(|| self.browser_requests(&task.sandbox_id))
                    .await;
                self.task_group(task, requested).await?
            };
            groups.push(group);
        }
        Ok(groups)
    }

    /// Check the recorded and branch PRs before reporting Done, even before
    /// anyone opens the sandbox Git overview. A missed deadline leaves work open.
    pub(crate) async fn parked_status(
        &self,
        task: &TaskRecord,
        requests: &OnceCell<HashSet<String>>,
        deadline: Instant,
    ) -> ParkedStatus {
        use crate::delivery::PrVerification;
        // Seed from memory before waiting on any store or browser lock. Once
        // the record is read, a timed-out lookup can still name its delivery.
        let mut observed = self.known_parked_pull(task, None);
        let mut status = ParkedStatus::new(task);
        let check = async {
            let record = self.deps.store.delivery(&task.id).await?;
            observed = self.known_parked_pull(task, record.as_ref());
            let requested = requests
                .get_or_init(|| self.browser_requests(&task.sandbox_id))
                .await;
            self.read_parked_local_work(task, Some(requested), &mut status)
                .await?;
            if status.facts.parked_has_local_work_or_request() {
                return Ok(());
            }
            let prs = self
                .verify_task_prs(
                    task,
                    record.as_ref(),
                    PrVerification::Display,
                    &mut observed,
                )
                .await?;
            let current = if let Some(prs) = prs {
                self.parked_evidence_current(
                    task,
                    record.as_ref(),
                    &prs,
                    PrVerification::Display,
                    &mut observed,
                    &mut status,
                )
                .await?
            } else {
                // A person or a plan change can arrive while even a failing check is in flight.
                self.read_parked_local_work(task, None, &mut status).await?;
                false
            };
            if !status.facts.parked_has_local_work_or_request() {
                status.facts.pull = if current {
                    observed.pr.map(|pr| {
                        if pr.open {
                            PullSignal::Tested
                        } else {
                            PullSignal::Merged
                        }
                    })
                } else {
                    Some(observed.signal.unwrap_or(PullSignal::Errored))
                };
            }
            Ok::<_, crate::error::AppError>(())
        };
        if !matches!(tokio::time::timeout_at(deadline, check).await, Ok(Ok(()))) {
            status.facts.pull = Some(PullSignal::Errored);
        }
        crate::race::reached(
            crate::race::Point::ParkedStatusRead,
            std::path::Path::new(task.id.as_str()),
        );
        status.pr = observed.pr;
        status
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
        let mut projects = Vec::new();
        for (record, level) in self.visible_sandboxes(&ctx.user).await? {
            let tasks = self.deps.store.tasks(&record.id, false).await?;
            projects.push((record, level, tasks, OnceCell::new()));
        }
        let parked = self
            .parked_list_statuses(
                projects
                    .iter()
                    .flat_map(|(_, _, tasks, requests)| {
                        tasks.iter().map(move |task| (task, requests))
                    })
                    .collect(),
            )
            .await;
        let mut out = Vec::with_capacity(projects.len());
        for (record, level, tasks, requests) in projects {
            let groups = self.groups_with_parked(&tasks, &requests, &parked).await?;
            out.push(Self::sandbox_with_groups(&record, level, groups));
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
        let rows = |pull| HashMap::from([(task.clone(), pull)]);
        let red = Some(ObservedPull {
            number: 2,
            signal: Some(PullSignal::Failing),
        });
        let landed = Some(ObservedPull {
            number: 1,
            signal: Some(PullSignal::Merged),
        });
        for (complete, later, expected) in [
            (false, None, Some(PullSignal::Failing)),
            (false, landed, Some(PullSignal::Failing)),
            (
                false,
                Some(ObservedPull {
                    number: 2,
                    signal: None,
                }),
                None,
            ),
            (true, None, None),
            (true, landed, Some(PullSignal::Merged)),
        ] {
            for reverse in [false, true] {
                let earlier = cache.begin(&sandbox)?;
                let newer = cache.begin(&sandbox)?;
                if reverse {
                    newer.record(complete, rows(later));
                    earlier.record(true, rows(red));
                } else {
                    earlier.record(true, rows(red));
                    newer.record(complete, rows(later));
                }
                assert_eq!(cache.get(&sandbox, &task), Some(expected));
                cache.begin(&sandbox)?.record(true, HashMap::new());
            }
        }

        let older = cache.begin(&sandbox)?;
        cache.begin(&sandbox)?.record(true, HashMap::new());
        older.record(false, rows(red));
        assert_eq!(cache.get(&sandbox, &task), Some(None));
        Ok(())
    }

    #[test]
    fn cancelling_an_older_overview_releases_its_publication_window() -> AppResult<()> {
        let cache = ObservedPulls::default();
        let sandbox = SandboxId::parse("one").map_err(crate::error::AppError::internal)?;
        let other = SandboxId::parse("two").map_err(crate::error::AppError::internal)?;
        let task = TaskId::parse("task").map_err(crate::error::AppError::internal)?;
        let old = cache.begin(&sandbox)?;
        let red = Some(ObservedPull {
            number: 2,
            signal: Some(PullSignal::Failing),
        });
        for _ in 1..OVERVIEW_WINDOW {
            cache
                .begin(&sandbox)?
                .record(false, HashMap::from([(task.clone(), red)]));
        }
        assert!(matches!(
            cache.begin(&sandbox),
            Err(crate::error::AppError::Capacity(_))
        ));
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::Failing)));
        cache.begin(&other)?.record(true, HashMap::new());
        drop(old);
        cache.begin(&sandbox)?.record(true, HashMap::new());
        assert_eq!(cache.get(&sandbox, &task), Some(None));
        Ok(())
    }
    #[test]
    fn cancelling_later_overviews_returns_capacity_while_the_first_read_is_pending() -> AppResult<()>
    {
        let cache = ObservedPulls::default();
        let sandbox = SandboxId::parse("one").map_err(crate::error::AppError::internal)?;
        let _old = cache.begin(&sandbox)?;
        for _ in 1..OVERVIEW_WINDOW {
            drop(cache.begin(&sandbox)?);
        }
        drop(cache.begin(&sandbox)?);
        Ok(())
    }
}
