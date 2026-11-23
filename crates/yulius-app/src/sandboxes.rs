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
    /// The recorded merge is displayable, but follow-up discovery was unavailable for a start rule.
    pub(crate) followup_unverified: bool,
}

impl ParkedStatus {
    pub(crate) fn new(task: &TaskRecord) -> Self {
        Self {
            facts: AttentionFacts::new(task.status, task.activity),
            pr: None,
            steps: None,
            followup_unverified: false,
        }
    }
}

/// PR facts the Git overview supplied for a single task. A closed, unmerged PR
/// has a number and no attention signal.
#[derive(Debug, Clone)]
pub(crate) struct ObservedPull {
    pub(crate) head: Option<Arc<str>>,
    pub(crate) number: u64,
    pub(crate) open: bool,
    pub(crate) conflicts: bool,
    pub(crate) signal: Option<PullSignal>,
}

impl ObservedPull {
    /// Resolve missing GitHub facts against the preceding observation in request order.
    pub(crate) fn fallback(
        prior: Option<Self>,
        read: Option<&Self>,
        complete: bool,
        unavailable: Option<u64>,
    ) -> Option<Self> {
        let mut prior = prior?;
        let keep = match read {
            None => !complete || unavailable == Some(prior.number),
            Some(pr) if pr.open => {
                unavailable == Some(pr.number)
                    && prior.open
                    && prior.number == pr.number
                    && pr.head.is_some()
                    && pr.head == prior.head
            }
            Some(pr) => !complete && prior.open && pr.number != prior.number,
        };
        if keep
            && let Some(read) = read.filter(|pr| {
                pr.open && pr.number == prior.number && pr.head == prior.head && pr.conflicts
            })
        {
            prior.conflicts = read.conflicts;
            if prior
                .signal
                .is_none_or(|signal| signal == PullSignal::Tested)
            {
                prior.signal = Some(PullSignal::Conflicting);
            }
        }
        keep.then_some(prior)
    }

    /// A branch listing confirms identity. Keep the checks already observed on that open PR.
    fn after_branch_read(self, prior: Option<Self>) -> Self {
        prior
            .filter(|old| {
                old.number == self.number
                    && old.open
                    && self.open
                    && (self.head.is_none() || old.head == self.head)
            })
            .unwrap_or(self)
    }
}

/// Maximum overlapping responses from a sandbox overview or one task branch.
const OVERVIEW_WINDOW: usize = 30;

type PullRows = HashMap<TaskId, Option<ObservedPull>>;

#[derive(Debug)]
struct Overview {
    complete: bool,
    rows: PullRows,
    unavailable: HashMap<TaskId, u64>,
}

impl Overview {
    fn reading(&self, task: &TaskId, prior: Option<ObservedPull>) -> Option<ObservedPull> {
        let read = self.rows.get(task)?.clone();
        ObservedPull::fallback(
            prior,
            read.as_ref(),
            self.complete,
            self.unavailable.get(task).copied(),
        )
        .or(read)
    }
}

#[derive(Debug)]
enum PullObservation {
    Sandbox(Overview),
    Branch(TaskId, ObservedPull),
    Verified(TaskId, Option<ObservedPull>),
}

#[derive(Debug)]
struct Slot {
    id: u64,
    branch_task: Option<TaskId>,
    published: Option<PullObservation>,
}

#[derive(Debug, Default)]
struct ReadOrder {
    next: u64,
    committed: HashMap<TaskId, ObservedPull>,
    sandbox_read: bool,
    slots: VecDeque<Slot>,
}

impl ReadOrder {
    fn finish(&mut self, id: u64, result: Option<PullObservation>) {
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
                let prior = &mut self.committed;
                match &read {
                    PullObservation::Sandbox(overview) => {
                        self.sandbox_read = true;
                        *prior = overview
                            .rows
                            .keys()
                            .filter_map(|task| {
                                overview
                                    .reading(task, prior.get(task).cloned())
                                    .map(|pr| (task.clone(), pr))
                            })
                            .collect();
                    }
                    PullObservation::Verified(task, Some(pr)) => {
                        prior.insert(task.clone(), pr.clone());
                    }
                    PullObservation::Verified(task, None) => {
                        prior.remove(task);
                    }
                    PullObservation::Branch(task, pr) => {
                        let pr = pr.clone().after_branch_read(prior.get(task).cloned());
                        prior.insert(task.clone(), pr);
                    }
                }
            }
        }
    }

    fn current(&self, task: &TaskId) -> Option<Option<ObservedPull>> {
        let mut known = self
            .committed
            .get(task)
            .cloned()
            .map(Some)
            .or(self.sandbox_read.then_some(None));
        for read in self.slots.iter().filter_map(|slot| slot.published.as_ref()) {
            match read {
                PullObservation::Sandbox(overview) => {
                    known = Some(overview.reading(task, known.flatten()))
                }
                PullObservation::Branch(id, pr) if id == task => {
                    known = Some(Some(pr.clone().after_branch_read(known.flatten())));
                }
                PullObservation::Verified(id, pr) if id == task => {
                    known = Some(pr.clone());
                }
                PullObservation::Branch(_, _) | PullObservation::Verified(_, _) => {}
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
    #[cfg(test)]
    fn record(self, complete: bool, rows: PullRows) {
        self.record_overview(complete, rows, HashMap::new());
    }

    pub(crate) fn record_overview(
        self,
        complete: bool,
        rows: PullRows,
        unavailable: HashMap<TaskId, u64>,
    ) {
        self.publish(PullObservation::Sandbox(Overview {
            complete,
            rows,
            unavailable,
        }));
    }

    pub(crate) fn record_branch(self, task: &TaskId, merged: u64, replacement: Option<u64>) {
        let pr = ObservedPull {
            head: None,
            number: replacement.unwrap_or(merged),
            open: replacement.is_some(),
            conflicts: false,
            signal: Some(if replacement.is_some() {
                PullSignal::InReview
            } else {
                PullSignal::Merged
            }),
        };
        self.record_identity(task, pr);
    }

    pub(crate) fn record_identity(self, task: &TaskId, pr: ObservedPull) {
        self.publish(PullObservation::Branch(task.clone(), pr));
    }

    pub(crate) fn record_terminal(self, task: &TaskId, pull: &crate::ports::github::GhPull) {
        self.record_identity(
            task,
            ObservedPull {
                head: Some(Arc::from(pull.head_sha.as_str())),
                number: pull.number,
                open: false,
                conflicts: false,
                signal: Some(if pull.merged {
                    PullSignal::Merged
                } else {
                    PullSignal::InReview
                }),
            },
        );
    }

    pub(crate) fn verified(self, task: &TaskId, pr: Option<ObservedPull>) {
        self.publish(PullObservation::Verified(task.clone(), pr));
    }

    fn publish(mut self, observation: PullObservation) {
        if let Some(id) = self.id.take()
            && let Ok(mut order) = self.cache.lock()
        {
            order.finish(id, Some(observation));
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
        self.begin_read(sandbox, None)
    }

    pub(crate) fn begin_branch(
        &self,
        sandbox: &SandboxId,
        task: &TaskId,
    ) -> AppResult<OverviewRead> {
        self.begin_read(sandbox, Some(task))
    }

    fn begin_read(
        &self,
        sandbox: &SandboxId,
        branch_task: Option<&TaskId>,
    ) -> AppResult<OverviewRead> {
        let cache = self
            .0
            .lock()
            .map_err(crate::error::AppError::internal)?
            .entry(sandbox.clone())
            .or_default()
            .clone();
        let id = {
            let mut order = cache.lock().map_err(crate::error::AppError::internal)?;
            if order
                .slots
                .iter()
                .filter(|slot| slot.branch_task.as_ref() == branch_task)
                .count()
                >= OVERVIEW_WINDOW
            {
                return Err(crate::error::AppError::Capacity(
                    "too many overlapping reads of the same PR source".into(),
                ));
            }
            let id = order.next;
            order.next = order.next.wrapping_add(1);
            order.slots.push_back(Slot {
                id,
                branch_task: branch_task.cloned(),
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

    #[cfg(test)]
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
        let groups = self.list_groups(&tasks, false).await?;
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

    pub(crate) async fn list_groups(
        &self,
        tasks: &[TaskRecord],
        for_start_rule: bool,
    ) -> AppResult<Vec<TaskGroup>> {
        let requests = OnceCell::new();
        let purpose = if for_start_rule {
            crate::ports::github::GitHubPurpose::Other
        } else {
            crate::ports::github::GitHubPurpose::Display
        };
        let parked = crate::ports::github::for_purpose(
            purpose,
            self.parked_list_statuses(tasks.iter().map(|task| (task, &requests)).collect()),
        )
        .await;
        self.groups_with_parked(tasks, &requests, &parked, for_start_rule)
            .await
    }

    async fn groups_with_parked(
        &self,
        tasks: &[TaskRecord],
        requests: &OnceCell<HashSet<String>>,
        parked: &HashMap<TaskId, ParkedStatus>,
        for_start_rule: bool,
    ) -> AppResult<Vec<TaskGroup>> {
        let mut groups = Vec::with_capacity(tasks.len());
        for task in tasks {
            let group = if let Some(status) = parked.get(&task.id) {
                if for_start_rule && status.followup_unverified {
                    TaskGroup::NeedsYou
                } else {
                    status.facts.group()
                }
            } else {
                let requested = requests
                    .get_or_init(|| self.browser_requests(&task.sandbox_id))
                    .await;
                let group = self.task_group(task, requested).await?;
                if for_start_rule && group == TaskGroup::Done && task.status == TaskStatus::Running
                {
                    let settled = match self.deps.store.delivery(&task.id).await {
                        Ok(record) => self
                            .hold_live_delivery(task, record.as_ref())
                            .await
                            .is_some(),
                        Err(_) => false,
                    };
                    if settled { group } else { TaskGroup::Working }
                } else {
                    group
                }
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
        crate::ports::github::for_purpose(
            crate::ports::github::GitHubPurpose::Display,
            self.parked_status_inner(task, requests, deadline),
        )
        .await
    }

    async fn parked_status_inner(
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
        let mut discovery = None;
        let mut publication = self
            .observed_pulls
            .begin_branch(&task.sandbox_id, &task.id)
            .ok();
        let mut verified = false;
        let mut local_ci = None;
        let check = async {
            let record = self.deps.store.delivery(&task.id).await?;
            observed = self.known_parked_pull(task, record.as_ref());
            self.restore_parked_followup(task, &mut observed).await?;
            let requested = requests
                .get_or_init(|| self.browser_requests(&task.sandbox_id))
                .await;
            self.read_parked_local_work(task, Some(requested), &mut status)
                .await?;
            if status.facts.parked_has_local_work_or_request() {
                return Ok(());
            }
            if crate::ports::github::current_purpose()
                == crate::ports::github::GitHubPurpose::Display
                && let Some(pull) = self.display_terminal(task, record.as_ref()).await?
                && observed
                    .pr
                    .is_none_or(|known| !known.open || known.number == pull.number)
            {
                observed.pr = Some(yulius_contracts::api::tasks::ParkedPr {
                    number: pull.number,
                    open: false,
                });
                observed.note_current(&pull);
                status.facts.pull = observed.signal;
                verified = true;
                return Ok(());
            }
            if let Some(pr) = observed.pr.filter(|pr| pr.open)
                && let Some(head) = observed.head.clone()
                && let Some(signal) = self.parking_ci_signal(task, pr.number, &head, None).await?
            {
                local_ci = Some((pr.number, head, signal));
            }
            if let Some(merged) = record
                .as_ref()
                .filter(|r| crate::delivery::past_merge(&r.stage))
                .and_then(|r| r.pr_number)
            {
                discovery = Some(crate::delivery::FollowupDiscovery::new(
                    publication.take(),
                    merged,
                    deadline,
                ));
            }
            // An open delivery can close and reveal a different PR in this very read.
            // Publish that replacement in request order so the terminal shortcut sees it.
            let tracking = record
                .as_ref()
                .filter(|r| r.stage == yulius_domain::delivery::DeliveryStage::PrOpen)
                .and_then(|r| r.pr_number)
                .and_then(|number| {
                    self.observed_pulls
                        .begin_branch(&task.sandbox_id, &task.id)
                        .ok()
                        .map(|read| (number, read))
                });
            let prs = self
                .verify_task_prs(
                    task,
                    record.as_ref(),
                    PrVerification::Display,
                    &mut observed,
                    discovery.as_mut(),
                )
                .await?;
            if let Some((number, read)) = tracking
                && let Some(replacement) = observed.pr.filter(|pr| pr.open && pr.number != number)
            {
                read.record_branch(&task.id, number, Some(replacement.number));
            }
            let current = if let Some(prs) = prs {
                status.followup_unverified = prs.followup_unverified;
                let current = self
                    .parked_evidence_current(
                        task,
                        record.as_ref(),
                        &prs,
                        PrVerification::Display,
                        &mut observed,
                        &mut status,
                    )
                    .await?;
                if status.followup_unverified {
                    // Another reader can discover an open replacement while we are rechecking local work.
                    observed = self.known_parked_pull(task, record.as_ref());
                    self.restore_parked_followup(task, &mut observed).await?;
                }
                current && !(status.followup_unverified && observed.pr.is_some_and(|pr| pr.open))
            } else {
                // A person or a plan change can arrive while even a failing check is in flight.
                self.read_parked_local_work(task, None, &mut status).await?;
                false
            };
            if !status.facts.parked_has_local_work_or_request() {
                verified = !status.followup_unverified
                    && !observed.unavailable
                    && (current
                        || (observed.pr.is_some_and(|pr| pr.open) && observed.signal.is_some()));
                status.facts.pull = if current {
                    observed.pr.map(|pr| {
                        if pr.open {
                            PullSignal::Tested
                        } else {
                            PullSignal::Merged
                        }
                    })
                } else if observed.unavailable || status.followup_unverified {
                    status.followup_unverified = true;
                    self.last_pull_signal(task, &observed)
                } else {
                    Some(
                        observed
                            .signal
                            .filter(|s| *s != PullSignal::Merged)
                            .unwrap_or(PullSignal::InReview),
                    )
                };
            }
            Ok::<_, crate::error::AppError>(())
        };
        match tokio::time::timeout_at(deadline, check).await {
            Ok(Ok(())) => {}
            Ok(Err(crate::error::AppError::Internal(_))) => {
                status.followup_unverified = true;
                status.facts.pull = Some(PullSignal::InReview);
            }
            Err(_) | Ok(Err(_)) => {
                status.followup_unverified = true;
                status.facts.pull = self.last_pull_signal(task, &observed);
            }
        }
        if status.followup_unverified
            && let Some((number, head, signal)) = local_ci
            && observed.pr.is_some_and(|pr| pr.open && pr.number == number)
            && observed.head.as_ref() == Some(&head)
            && observed.signal.is_none()
        {
            status.facts.pull = Some(signal);
        }
        crate::race::reached(
            crate::race::Point::ParkedStatusRead,
            std::path::Path::new(task.id.as_str()),
        );
        let confirmed = verified.then_some(status.facts.pull).flatten();
        if let Some(mut discovery) = discovery {
            discovery.publish(&task.id, &observed, confirmed);
        } else if let Some(read) = publication {
            let pull = observed.pr.map(|pr| ObservedPull {
                number: pr.number,
                open: pr.open,
                conflicts: status.facts.pull == Some(PullSignal::Conflicting),
                head: observed.head.clone(),
                signal: status.facts.pull,
            });
            if verified {
                read.verified(&task.id, pull);
            } else if observed.fresh_head
                && let Some(pull) = pull
            {
                read.record_identity(&task.id, pull);
            }
        }
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
        let (pull, plan_open, spec_unfinished, candidate_pending) = if finished {
            let record = store.delivery(&task.id).await?;
            // An achieved goal whose recorded integration failure the watch has not cleared is
            // Yulius's to settle, as capacity holds it (SPEC-621 FIN-004).
            let candidate_pending = live
                && task.activity == Some(yulius_domain::task::Activity::Done)
                && self
                    .local_candidate_shown(task, record.as_ref())
                    .await
                    .unwrap_or_else(|err| {
                        tracing::warn!(task = %task.id, error = %err, "the recorded integration failure could not be read; it is shown as pending");
                        true
                    });
            let observed = self.known_parked_pull(task, record.as_ref());
            let mut pull = self.last_pull_signal(task, &observed);
            if let Some(pr) = observed.pr.filter(|pr| pr.open)
                && let Some(head) = observed.head.as_deref()
                && let Some(signal) = self.parking_ci_signal(task, pr.number, head, None).await?
            {
                pull = Some(signal);
            }
            // A newer Yulius CI run already queued or running supersedes a stale failing
            // pull-request observation on a live task (SPEC-578).
            if live && pull == Some(PullSignal::Failing) && self.pr_ci_progressing(task).await {
                pull = Some(PullSignal::InReview);
            }
            let plan_open = plan_open(&store.steps(&task.id).await?);
            (
                pull,
                plan_open,
                self.spec_unfinished(&task.id).await,
                candidate_pending,
            )
        } else {
            (None, false, false, false)
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
            staging_wait: if finished {
                store.task_staging_wait(&task.id).await?
            } else {
                None
            },
            pull,
            paused: self.is_paused(task).await,
            awaits_start_onboarding: false,
            merge_armed: live && self.merge_is_armed(&task.id),
            candidate_pending,
        })
    }

    /// Whether `task`'s open pull request currently has a Yulius CI run queued or running
    /// (SPEC-578), so a stale cached failure does not keep a live task in Needs You.
    pub(crate) async fn pr_ci_progressing(&self, task: &TaskRecord) -> bool {
        let pr = self
            .observed_pulls
            .known_pull(&task.sandbox_id, &task.id)
            .flatten()
            .map(|obs| obs.number)
            .or(match self.deps.store.delivery(&task.id).await {
                Ok(Some(rec)) => rec.pr_number,
                _ => None,
            });
        let Some(pr) = pr else {
            return false;
        };
        self.latest_pr_ci_run(&task.sandbox_id, pr)
            .await
            .is_some_and(|run| {
                matches!(
                    run.state,
                    yulius_domain::ci::RunState::Queued | yulius_domain::ci::RunState::Running
                )
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
        let read = self.deps.browser.running(sandbox);
        // Stop may publish several task views while synchronizing the spec and plan.
        // They share its observation deadline. Parking and other decision paths keep
        // their own browser read contracts.
        let session = match crate::hooks::STOP_BROWSER_DEADLINE.try_with(|deadline| *deadline) {
            Ok(deadline) => tokio::time::timeout_at(deadline, read).await.ok().flatten(),
            Err(_) => read.await,
        };
        match session {
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
            let groups = self
                .groups_with_parked(&tasks, &requests, &parked, false)
                .await?;
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
            head: None,
            number: 2,
            open: true,
            conflicts: false,
            signal: Some(PullSignal::Failing),
        });
        let landed = Some(ObservedPull {
            head: None,
            number: 1,
            open: false,
            conflicts: false,
            signal: Some(PullSignal::Merged),
        });
        for (complete, later, expected) in [
            (false, None, Some(PullSignal::Failing)),
            (false, landed.clone(), Some(PullSignal::Failing)),
            (
                false,
                Some(ObservedPull {
                    head: None,
                    number: 2,
                    open: false,
                    conflicts: false,
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
                    newer.record(complete, rows(later.clone()));
                    earlier.record(true, rows(red.clone()));
                } else {
                    earlier.record(true, rows(red.clone()));
                    newer.record(complete, rows(later.clone()));
                }
                assert_eq!(cache.get(&sandbox, &task), Some(expected));
                cache.begin(&sandbox)?.record(true, HashMap::new());
            }
        }

        let older = cache.begin(&sandbox)?;
        cache.begin(&sandbox)?.record(true, HashMap::new());
        older.record(false, rows(red.clone()));
        assert_eq!(cache.get(&sandbox, &task), Some(None));
        Ok(())
    }

    #[test]
    fn branch_discovery_leaves_other_tasks_cold_and_preserves_their_observations() -> AppResult<()>
    {
        let cache = ObservedPulls::default();
        let sandbox = SandboxId::parse("sandbox").map_err(crate::error::AppError::internal)?;
        let task = TaskId::parse("task").map_err(crate::error::AppError::internal)?;
        let other = TaskId::parse("other").map_err(crate::error::AppError::internal)?;
        let older = cache.begin(&sandbox)?;
        cache
            .begin_branch(&sandbox, &task)?
            .record_branch(&task, 1, Some(2));
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::InReview)));
        assert!(cache.known_pull(&sandbox, &other).is_none());
        drop(older);
        assert!(cache.known_pull(&sandbox, &other).is_none());
        let red = Some(ObservedPull {
            head: None,
            number: 2,
            open: true,
            conflicts: false,
            signal: Some(PullSignal::Failing),
        });
        let rows = HashMap::from([(task.clone(), red.clone()), (other.clone(), red)]);
        cache.begin(&sandbox)?.record(true, rows);
        cache
            .begin_branch(&sandbox, &task)?
            .record_branch(&task, 1, Some(2));
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::Failing)));
        cache
            .begin_branch(&sandbox, &task)?
            .record_branch(&task, 1, Some(3));
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::InReview)));
        cache
            .begin_branch(&sandbox, &task)?
            .record_branch(&task, 1, None);
        assert_eq!(cache.get(&sandbox, &task), Some(Some(PullSignal::Merged)));
        assert_eq!(cache.get(&sandbox, &other), Some(Some(PullSignal::Failing)));
        Ok(())
    }

    #[test]
    fn parked_branch_results_leave_room_for_an_overview_of_a_busy_sandbox() -> AppResult<()> {
        let cache = ObservedPulls::default();
        let sandbox = SandboxId::parse("sandbox").map_err(crate::error::AppError::internal)?;
        let slow_overview = cache.begin(&sandbox)?;
        for number in 1..=OVERVIEW_WINDOW {
            let task = TaskId::parse(&format!("parked-{number}"))
                .map_err(crate::error::AppError::internal)?;
            cache
                .begin_branch(&sandbox, &task)?
                .record_branch(&task, 1, None);
        }
        drop(cache.begin(&sandbox)?);
        let repeat = TaskId::parse("repeat").map_err(crate::error::AppError::internal)?;
        for _ in 0..OVERVIEW_WINDOW {
            cache
                .begin_branch(&sandbox, &repeat)?
                .record_branch(&repeat, 1, None);
        }
        assert!(matches!(
            cache.begin_branch(&sandbox, &repeat),
            Err(crate::error::AppError::Capacity(_))
        ));
        drop(slow_overview);
        drop(cache.begin_branch(&sandbox, &repeat)?);
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
            head: None,
            number: 2,
            open: true,
            conflicts: false,
            signal: Some(PullSignal::Failing),
        });
        for _ in 1..OVERVIEW_WINDOW {
            cache
                .begin(&sandbox)?
                .record(false, HashMap::from([(task.clone(), red.clone())]));
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
