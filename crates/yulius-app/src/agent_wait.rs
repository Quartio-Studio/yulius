//! Agents wait without polling (SPEC-011): `delivery_merge` called while the checks of the
//! reviewed head still run arms a merge that Yulius performs as soon as they pass, and
//! `yulius wait` (the sandbox CLI, `POST /internal/agent/wait`) blocks until the checks at the
//! agent's head finish, its delivery moves or an armed merge is decided, and says how it was
//! (SPEC-028 MSR-003).
//!
//! Everything here reads Yulius's own records (CI runs, deliveries, the armed merge and its
//! outcome): waiting never costs a GitHub request.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::delivery::MergeFailure;
use crate::error::AppResult;
use crate::model::TaskRecord;

/// How often a waiting request reads the state again.
const WAIT_POLL: Duration = Duration::from_secs(2);
/// Longest single wait request (the CLI asks again until its own limit).
pub const WAIT_MAX: Duration = Duration::from_secs(55);

/// The file in the data directory that keeps armed merges across restarts (SPEC-050 DLV-205): a
/// deploy dropped every armed merge, and agents waited for merges that no longer existed.
pub const ARMED_MERGES_FILE: &str = "armed-merges.json";

/// Serializes changes to [`ARMED_MERGES_FILE`].
static ARMED_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One armed merge as saved: task id, head, review summary, when it was armed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedArmed {
    task: String,
    head: String,
    summary: String,
    #[serde(default)]
    armed_at: i64,
    /// The task's owner who armed it themselves (SPEC-487 MRG-003); absent for an agent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by: Option<String>,
}

/// A merge armed by `delivery_merge` while the checks of the reviewed head still ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArmedMerge {
    /// The reviewed head: only this exact commit is merged.
    pub head: String,
    /// The review summary given with it.
    pub summary: String,
    /// When the agent armed it (ms): merge trains take armed merges in this order (SPEC-084).
    /// Kept when Yulius moves the head (DLV-203); 0 for merges saved before it was recorded.
    pub armed_at: i64,
    /// The login of the task's owner when they armed it themselves, through their Superagent or
    /// their own agent (SPEC-487 MRG-003): no independent review covers it, and its merge is
    /// theirs. `None` for an agent's `delivery_merge`.
    pub by: Option<String>,
}

impl ArmedMerge {
    /// Who merges this armed merge once it passes, for the audit (SPEC-487 MRG-003): the actor,
    /// the action and the words that say how it was armed.
    pub(crate) fn merge_audit(&self, number: u32) -> (String, &'static str, String) {
        match &self.by {
            Some(login) => (
                login.clone(),
                "delivery.person.merge",
                format!("armed by its owner (@{login}), no independent review asked for"),
            ),
            None => (
                format!("task:{number}"),
                "delivery.agent.merge",
                format!(
                    "reviewed independently ({})",
                    if self.summary.is_empty() {
                        "no summary"
                    } else {
                        self.summary.as_str()
                    }
                ),
            ),
        }
    }
}

/// A task's armed merge, Yulius's decisions of it in progress and how the last one ended: one
/// value under one lock (SPEC-064 WAI-003). `yulius wait` reads them together, so an outcome is
/// never seen next to a merge that still reads as armed; and a merge armed while an earlier one is
/// being decided supersedes it (WAI-002): that earlier decision changes nothing when it ends.
#[derive(Debug, Default)]
pub(crate) struct ArmedSlot {
    /// The armed merge; taken out while Yulius decides it.
    merge: Option<ArmedMerge>,
    /// Decisions in progress, each holding the merge it took.
    deciding: u32,
    /// How the last armed merge ended, until another one is armed.
    outcome: Option<MergeOutcome>,
    /// Raised by every arm and clear: a decision taken in an older generation is superseded.
    generation: u64,
    /// Where the armed merge stands, in one sentence, as its last decision left it (SPEC-441
    /// MQ-007).
    note: Option<String>,
    /// The sentence the decision in progress wrote, kept by [`Self::finish`] for its generation.
    pending_note: Option<String>,
}

/// A decision in progress: the merge taken out of the slot, and the generation it was armed in.
#[derive(Debug)]
pub(crate) struct Deciding {
    pub(crate) merge: ArmedMerge,
    generation: u64,
}

/// How Yulius decided an armed merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Still waiting, for this merge (the same, or moved to a new head by DLV-203).
    Keep(ArmedMerge),
    /// Merged, refused or dropped: one line for the agent.
    Decided(String),
}

impl ArmedSlot {
    /// Arms `merge`: it replaces any armed one and supersedes a decision in progress, and the
    /// last outcome is forgotten.
    fn arm(&mut self, merge: ArmedMerge) {
        self.generation += 1;
        self.merge = Some(merge);
        self.outcome = None;
        self.note = None;
    }

    /// Nothing is armed any more (merged at once by `delivery_merge`); a decision in progress is
    /// superseded.
    fn clear(&mut self) {
        self.generation += 1;
        self.merge = None;
        self.note = None;
    }

    /// Takes the armed merge out to decide it; it still reads as armed until [`Self::finish`].
    fn take(&mut self) -> Option<Deciding> {
        let merge = self.merge.take()?;
        self.deciding += 1;
        Some(Deciding {
            merge,
            generation: self.generation,
        })
    }

    /// Ends a decision in one step: the merge is put back or its outcome kept, unless a newer
    /// merge was armed (or cleared) meanwhile. Returns whether the armed merge changed (to save).
    fn finish(&mut self, deciding: &Deciding, verdict: Verdict) -> bool {
        self.deciding = self.deciding.saturating_sub(1);
        let note = self.pending_note.take();
        if self.generation != deciding.generation {
            return false;
        }
        match verdict {
            Verdict::Keep(merge) => {
                let changed = merge != deciding.merge;
                self.merge = Some(merge);
                self.note = note;
                changed
            }
            Verdict::Decided(text) => {
                self.note = None;
                self.outcome = Some(MergeOutcome {
                    head: deciding.merge.head.clone(),
                    text,
                    by: deciding.merge.by.clone(),
                });
                true
            }
        }
    }

    /// Whether a merge is armed (counting one being decided), how the last one ended, and where
    /// an armed one stands.
    fn view(&self) -> (bool, Option<MergeOutcome>, Option<String>) {
        let armed = self.merge.is_some() || self.deciding > 0;
        (
            armed,
            self.outcome.clone(),
            self.note.clone().filter(|_| armed),
        )
    }
}

/// An armed merge as a merge train sees it (SPEC-084).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArmedNow {
    /// Nothing is armed.
    None,
    /// Armed, and nobody is deciding it.
    Armed(ArmedMerge),
    /// Being decided right now (by the end of a CI run or the delivery loop).
    Deciding,
}

/// What keeping an armed merge current did (DLV-203).
enum Current {
    /// The branch contains the integration branch, or it is not Yulius's to touch now.
    Unchanged,
    /// Yulius merged the integration branch in and pushed this head: the merge stays armed.
    Moved(String),
    /// A real conflict, for the agent: the armed merge is dropped with this text.
    Conflict(String),
}

/// How an armed merge ended (merged, refused or dropped), kept until the next one is armed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeOutcome {
    /// The head that was armed.
    pub head: String,
    /// One line for the agent: what happened and what to do next.
    pub text: String,
    /// The task's owner who armed it, when they did (SPEC-487 MRG-001).
    pub by: Option<String>,
}

/// What `yulius wait` watches for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WaitState {
    /// The commit waited on (the agent's HEAD).
    pub head: String,
    /// The Yulius CI run at that head: `none`, `queued`, `running`, `passed`, `failed`,
    /// `error` or `cancelled`.
    pub checks: String,
    /// Why a run ended without passing, when it says.
    #[serde(default)]
    pub checks_detail: Option<String>,
    /// The task's delivery stage (`none`, `pr_open`, `merged_integration`, …).
    pub stage: String,
    /// A merge is armed for when the checks pass.
    pub merge_armed: bool,
    /// How the last armed merge ended (MSR-003), until another one is armed.
    #[serde(default)]
    pub merge_outcome: Option<String>,
    /// The head that armed merge was for.
    #[serde(default)]
    pub merge_outcome_head: Option<String>,
    /// Why Yulius could not push this head, with what to do (DLV-201).
    #[serde(default)]
    pub push_problem: Option<String>,
    /// The merge train this task's armed merge is in, or why it was left out (SPEC-084 TRN-007).
    /// Informative: a change shows in the wait's progress but does not end it.
    #[serde(default)]
    pub merge_train: Option<String>,
    /// The latest CD run this task's agent started (SPEC-134 CDT-003): its id.
    #[serde(default)]
    pub cd_run: Option<String>,
    /// Its state (`running`, `passed`, `failed`, `error`, `cancelled`); the wait ends when it
    /// ends.
    #[serde(default)]
    pub cd_state: Option<String>,
    /// Its environment and why it ended without passing, when it says.
    #[serde(default)]
    pub cd_detail: Option<String>,
    /// It ended and no wait reported it yet (a run that ended before the wait began, review M4).
    #[serde(default)]
    pub cd_unreported: bool,
}

impl WaitState {
    /// Whether the checks at the head have ended (passed or not).
    #[must_use]
    pub fn checks_ended(&self) -> bool {
        matches!(
            self.checks.as_str(),
            "passed" | "failed" | "error" | "cancelled"
        )
    }
}

impl App {
    /// Arms a merge of `head` for when its checks pass (`delivery_merge` while they still run),
    /// the independent review having approved `reviewed` (`head`, or an ancestor of it through
    /// clean merges of the integration branch only). The same head armed already stays armed as
    /// it was, with its place in the queue (SPEC-441 MQ-006: arming it again took it out of its
    /// merge train); returns `false` then. A merge armed at `reviewed` follows `head` with its
    /// place.
    pub(crate) fn arm_merge(
        &self,
        task: &TaskRecord,
        head: &str,
        reviewed: &str,
        summary: &str,
    ) -> bool {
        self.arm_merge_as(task, head, reviewed, summary, None)
    }

    /// [`Self::arm_merge`] for the task's owner `login`, who arms `head` themselves (SPEC-487
    /// MRG-003): no review covers it, and the merge it makes is audited as theirs.
    pub(crate) fn arm_merge_for_person(&self, task: &TaskRecord, head: &str, login: &str) -> bool {
        let summary = format!("armed by @{login}, the task's owner");
        self.arm_merge_as(task, head, head, &summary, Some(login.to_owned()))
    }

    fn arm_merge_as(
        &self,
        task: &TaskRecord,
        head: &str,
        reviewed: &str,
        summary: &str,
        by: Option<String>,
    ) -> bool {
        let runtime = self.runtime(&task.id);
        let mut slot = lock_slot(&runtime);
        if slot.merge.as_ref().is_some_and(|armed| armed.head == head) {
            return false;
        }
        let reviewed = reviewed.to_ascii_lowercase();
        let armed_at = slot
            .merge
            .as_ref()
            .filter(|armed| reviewed.len() >= 7 && armed.head.starts_with(&reviewed))
            .map_or_else(|| self.now().0, |armed| armed.armed_at);
        let merge = ArmedMerge {
            head: head.to_owned(),
            summary: summary.to_owned(),
            armed_at,
            by,
        };
        self.save_armed(&task.id, Some(&merge));
        slot.arm(merge);
        true
    }

    /// Nothing is armed for `task` any more: `delivery_merge` merged it at once.
    pub(crate) fn clear_armed(&self, task: &TaskRecord) {
        let runtime = self.runtime(&task.id);
        let mut slot = lock_slot(&runtime);
        slot.clear();
        self.save_armed(&task.id, None);
    }

    /// Records `task`'s armed merge, or that it has none any more, in [`ARMED_MERGES_FILE`]
    /// (written whole, then renamed into place). Called with the task's [`ArmedSlot`] locked, so
    /// the file follows the slot's order of changes; only the startup restore forgets, without
    /// it, a task whose state keeps no armed merge. Failures are logged: the merge stays armed
    /// in memory.
    fn save_armed(&self, task: &TaskId, armed: Option<&ArmedMerge>) {
        let _guard = ARMED_FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.config.data_dir.join(ARMED_MERGES_FILE);
        let mut saved = read_armed(&path);
        saved.retain(|entry| entry.task != task.as_str());
        if let Some(armed) = armed {
            saved.push(SavedArmed {
                task: task.as_str().to_owned(),
                head: armed.head.clone(),
                summary: armed.summary.clone(),
                armed_at: armed.armed_at,
                by: armed.by.clone(),
            });
        }
        let written = serde_json::to_vec(&saved)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &path)
            });
        if let Err(err) = written {
            tracing::warn!(error = %err, "armed merges could not be saved");
        }
    }

    /// Puts back the armed merges after a restart (DLV-205) for the tasks whose run keeps one
    /// (`TaskStatus::keeps_an_armed_merge`, the rule every pass decides by); the others are
    /// forgotten.
    pub async fn restore_armed_merges(&self) {
        let path = self.config.data_dir.join(ARMED_MERGES_FILE);
        let saved = {
            let _guard = ARMED_FILE_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            read_armed(&path)
        };
        for entry in saved {
            let Ok(task_id) = TaskId::parse(&entry.task) else {
                continue;
            };
            let kept = matches!(
                self.deps.store.task(&task_id).await,
                Ok(Some(task)) if task.status.keeps_an_armed_merge()
            );
            if !kept {
                self.save_armed(&task_id, None);
                continue;
            }
            let runtime = self.runtime(&task_id);
            let mut slot = lock_slot(&runtime);
            // Only when nothing was armed or cleared in this process yet: what was armed or
            // decided since this file was read is newer than it.
            if slot.generation == 0 && slot.merge.is_none() && slot.deciding == 0 {
                slot.merge = Some(ArmedMerge {
                    head: entry.head,
                    summary: entry.summary,
                    armed_at: entry.armed_at,
                    by: entry.by,
                });
                tracing::info!(task = %task_id, "armed merge restored after a restart");
            }
        }
        self.armed_restored
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether `task` has a merge armed as far as this server knows: its slot, and, until the
    /// armed merges saved before this server started were put back, their file (a hook or a
    /// chain pass may be served before that; SPEC-487 CHN-001).
    pub(crate) fn merge_is_armed(&self, task: &TaskId) -> bool {
        if self.armed_view(task).0 {
            return true;
        }
        if self
            .armed_restored
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return false;
        }
        let _guard = ARMED_FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read_armed(&self.config.data_dir.join(ARMED_MERGES_FILE))
            .iter()
            .any(|entry| entry.task == task.as_str())
    }

    /// At a start: puts back the armed merges (DLV-205), then tries them once, as the delivery
    /// loop does, so one whose checks passed while Yulius was down merges now, not at the loop's
    /// first round (SPEC-358 MSB-008).
    pub async fn resume_armed_merges(self: &Arc<Self>) {
        self.restore_armed_merges().await;
        self.merge_armed_everywhere().await;
    }

    /// One merge pass over every sandbox's armed merges. A sandbox whose pass is held (a merge
    /// or a move to GitHub, which can push for minutes) is merged by whoever holds it, or on the
    /// next round: the other sandboxes never wait for it.
    pub(crate) async fn merge_armed_everywhere(self: &Arc<Self>) {
        let Ok(sandboxes) = self.deps.store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            if self.armed_pass(&sandbox.id).try_lock().is_err() {
                continue;
            }
            self.merge_armed(&sandbox.id).await;
        }
    }

    /// Where `task`'s armed merge stands (SPEC-441 MQ-007): its merge train's note, else the
    /// sentence its last decision left; `None` when nothing is armed.
    pub(crate) fn merge_queue_note(&self, task: &TaskRecord) -> Option<String> {
        let (armed, _, note) = lock_slot(&self.runtime(&task.id)).view();
        armed.then(|| self.train_note(task).or(note)).flatten()
    }

    /// Records where `task`'s armed merge stands, for the decision in progress (SPEC-441
    /// MQ-007): kept when that decision keeps the merge armed.
    fn note_armed(&self, task: &TaskId, text: String) {
        lock_slot(&self.runtime(task)).pending_note = Some(text);
    }

    /// The fast pass (SPEC-441 MQ-008): a merge pass over each sandbox with an armed merge that
    /// can be decided now, read from Yulius's own records only. A sandbox whose pass is held is
    /// decided by whoever holds it.
    pub async fn merge_decidable_everywhere(self: &Arc<Self>) {
        let Ok(sandboxes) = self.deps.store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            if !self.decidable_in(sandbox).await || self.armed_pass(&sandbox.id).try_lock().is_err()
            {
                continue;
            }
            self.merge_armed(&sandbox.id).await;
        }
    }

    /// Whether a pass could decide something in `sandbox` now, from Yulius's own records and its
    /// clone: a merge candidate whose run ended, or an armed head of an open pull request whose
    /// own run passed that is up to date (merge it) or, with no candidate under test, whose agent
    /// is idle with everything committed at that head (put it in one, or bring it up to date). A
    /// green head waiting for a candidate under test, for its busy agent or for a commit is not:
    /// a pass then would only fetch and ask GitHub again (review M1). Public for the fast pass's
    /// integration tests.
    pub async fn decidable_in(&self, sandbox: &crate::model::SandboxRecord) -> bool {
        use yulius_domain::ci::{GateResult, RunState};
        use yulius_domain::task::{Activity, TaskStatus};
        if self.train_run_ended(sandbox).await {
            return true;
        }
        let Ok(tasks) = self.deps.store.tasks(&sandbox.id, false).await else {
            return false;
        };
        let repo = self.sandbox_repo(&sandbox.id);
        let integration = format!(
            "origin/{}",
            crate::delivery::delivery_profile(sandbox).integration_branch
        );
        let candidate_running = self.train_running(sandbox);
        for task in tasks {
            let ArmedNow::Armed(armed) = self.armed_now(&task.id) else {
                continue;
            };
            if !matches!(
                self.ci_gate_result(sandbox, &armed.head).await,
                GateResult::Run(Some(RunState::Passed | RunState::Skipped))
            ) {
                continue;
            }
            let open = matches!(
                self.deps.store.delivery(&task.id).await,
                Ok(Some(record)) if record.stage == yulius_domain::delivery::DeliveryStage::PrOpen
            );
            if !open {
                continue;
            }
            let current = matches!(
                self.deps
                    .git
                    .is_ancestor(&repo, &integration, &armed.head)
                    .await,
                Ok(true)
            );
            if current && self.train_hold(&task) == crate::merge_train::Hold::Free {
                return true;
            }
            let idle = task.status == TaskStatus::Running
                && task.activity.is_some_and(Activity::is_idle)
                && self.background_shells_of(&task.id).unwrap_or(0) == 0;
            if !candidate_running && idle && self.at_clean_head(&task, &armed.head).await {
                return true;
            }
        }
        false
    }

    /// Whether `task`'s worktree is at `head` with nothing uncommitted.
    async fn at_clean_head(&self, task: &TaskRecord, head: &str) -> bool {
        let Some(worktree) = task.worktree_path.as_deref().map(std::path::Path::new) else {
            return false;
        };
        !self.deps.git.has_changes(worktree).await.unwrap_or(true)
            && matches!(
                self.deps.git.log(worktree, "HEAD", 1).await,
                Ok(log) if log.first().is_some_and(|commit| commit.sha == head)
            )
    }

    /// Whether `task` has a merge armed (or being decided), and how the last one ended: its
    /// head and the line for the agent (SPEC-444 PRN-005).
    pub(crate) fn armed_view(&self, task: &TaskId) -> (bool, Option<(String, String)>) {
        let (armed, outcome, _) = lock_slot(&self.runtime(task)).view();
        (armed, outcome.map(|o| (o.head, o.text)))
    }

    /// `task`'s armed merge as it is now, and whether Yulius is deciding one (SPEC-487 MRG-001):
    /// while a decision runs, its merge is out of the slot.
    pub(crate) fn armed_merge_view(&self, task: &TaskId) -> (Option<ArmedMerge>, bool) {
        let runtime = self.runtime(task);
        let slot = lock_slot(&runtime);
        (slot.merge.clone(), slot.deciding > 0)
    }

    /// How `task`'s last armed merge ended, with who armed it (SPEC-487 MRG-001).
    pub(crate) fn merge_outcome(&self, task: &TaskId) -> Option<MergeOutcome> {
        lock_slot(&self.runtime(task)).outcome.clone()
    }

    /// Forgets how an earlier armed merge ended (a new `delivery_merge` call answers for itself).
    pub(crate) fn forget_merge_outcome(&self, task: &TaskRecord) {
        lock_slot(&self.runtime(&task.id)).outcome = None;
    }

    /// `task`'s armed merge as a merge train sees it (SPEC-084).
    pub(crate) fn armed_now(&self, task: &TaskId) -> ArmedNow {
        let runtime = self.runtime(task);
        let slot = lock_slot(&runtime);
        match (&slot.merge, slot.deciding) {
            (_, 1..) => ArmedNow::Deciding,
            (Some(merge), 0) => ArmedNow::Armed(merge.clone()),
            (None, 0) => ArmedNow::None,
        }
    }

    /// Takes `task`'s armed merge out to decide it (a merge train's step); `None` when nothing
    /// is armed or it is being decided. End it with [`App::finish_armed`].
    pub(crate) fn take_armed(&self, task: &TaskId) -> Option<Deciding> {
        lock_slot(&self.runtime(task)).take()
    }

    /// Saves `merge` (its head just pushed by Yulius) as `task`'s armed merge while a decision of
    /// [`App::take_armed`] is still going, unless a newer arm superseded it: a restart before the
    /// decision ends then finds the head Yulius pushed (SPEC-084 TRN-008).
    pub(crate) fn save_armed_during(&self, task: &TaskId, deciding: &Deciding, merge: &ArmedMerge) {
        let runtime = self.runtime(task);
        let slot = lock_slot(&runtime);
        if slot.generation == deciding.generation {
            self.save_armed(task, Some(merge));
        }
    }

    /// Ends a decision of [`App::take_armed`] in one step, as [`App::merge_armed`] does.
    pub(crate) async fn finish_armed(&self, task: &TaskId, deciding: &Deciding, verdict: Verdict) {
        let decided = matches!(verdict, Verdict::Decided(_));
        {
            let runtime = self.runtime(task);
            let mut slot = lock_slot(&runtime);
            if slot.finish(deciding, verdict) {
                self.save_armed(task, slot.merge.as_ref());
            }
        }
        if decided {
            self.armed_wait_ended(task, &deciding.merge).await;
        }
    }

    /// Records an update of a task's branch onto the integration branch Yulius made (SPEC-137
    /// STG-104): `merged`, or `conflict` (aborted, left to the agent).
    pub(crate) async fn integration_update_span(
        &self,
        task: &TaskId,
        integration: &str,
        started: Timestamp,
        outcome: &str,
    ) {
        self.record_task_span(
            task,
            ("update", &format!("onto {integration}")),
            (started, self.now()),
            outcome,
            serde_json::json!({ "branch": integration }),
        )
        .await;
    }

    /// Records how long an armed merge waited for its checks, from arming to its decision
    /// (SPEC-137 STG-104): `merged` when the task's pull request is merged now, else `dropped`.
    async fn armed_wait_ended(&self, task: &TaskId, armed: &ArmedMerge) {
        use yulius_domain::delivery::DeliveryStage;
        let merged = matches!(
            self.deps.store.delivery(task).await,
            Ok(Some(record)) if !matches!(record.stage, DeliveryStage::None | DeliveryStage::PrOpen)
        );
        self.record_task_span(
            task,
            ("wait", "merge_armed"),
            (Timestamp(armed.armed_at), self.now()),
            if merged { "merged" } else { "dropped" },
            serde_json::json!({ "head": armed.head }),
        )
        .await;
    }

    /// Performs the armed merges of a sandbox whose checks passed (after every CI run and from
    /// the delivery loop). An armed merge is dropped when the branch moved on, the checks failed
    /// or the merge was refused; `yulius wait` says which. A merge train goes first (SPEC-084):
    /// it forms, or merges the pull requests it tested together.
    pub async fn merge_armed(self: &Arc<Self>, sandbox: &SandboxId) {
        // One pass per sandbox at a time (SPEC-084 TRN-010): a pass that read a branch as up to
        // date while another merged into its base had its merge refused as behind, and the
        // armed merge was dropped.
        let pass = self.armed_pass(sandbox);
        let _pass = pass.lock().await;
        self.run_merge_train(sandbox).await;
        let Ok(mut tasks) = self.deps.store.tasks(sandbox, false).await else {
            return;
        };
        // In the order they were armed (SPEC-441 MQ-002): of two that can merge now, the one
        // armed first goes first.
        tasks.sort_by_cached_key(|task| match self.armed_now(&task.id) {
            ArmedNow::Armed(merge) => (merge.armed_at, task.number),
            ArmedNow::None | ArmedNow::Deciding => (i64::MAX, task.number),
        });
        for task in tasks {
            // Taken while it is decided, so a CI end and the delivery loop never merge twice; it
            // reads as armed for `yulius wait` until the decision ends, in one step (WAI-003).
            let runtime = self.runtime(&task.id);
            let Some(deciding) = lock_slot(&runtime).take() else {
                continue;
            };
            let verdict = self.merge_armed_task(&task, deciding.merge.clone()).await;
            let decided = matches!(verdict, Verdict::Decided(_));
            {
                let mut slot = lock_slot(&runtime);
                if slot.finish(&deciding, verdict) {
                    self.save_armed(&task.id, slot.merge.as_ref());
                }
            }
            if decided {
                self.armed_wait_ended(&task.id, &deciding.merge).await;
            }
        }
    }

    /// The lock of `sandbox`'s armed-merge passes (SPEC-084 TRN-010), which a playground's merges
    /// also hold (SPEC-163): one decision of its merges at a time. Not reentrant.
    pub(crate) fn armed_pass(&self, sandbox: &SandboxId) -> Arc<tokio::sync::Mutex<()>> {
        let mut passes = self
            .ci
            .armed_passes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(passes.entry(sandbox.as_str().to_owned()).or_default())
    }

    /// Merges one armed task when its checks passed; says whether to keep waiting or how it
    /// ended.
    async fn merge_armed_task(self: &Arc<Self>, task: &TaskRecord, armed: ArmedMerge) -> Verdict {
        // An armed merge lives with the run that armed it (SPEC-483 D6): a task a person
        // stopped or closed is not merged behind their back, and one that could only wait for
        // an agent that is gone does not wait for ever. Said below, once it is known that the
        // pull request was not merged already.
        let ended = (!task.status.keeps_an_armed_merge()).then(|| {
            Verdict::Decided(format!(
                "The merge you armed was dropped: the task's run ended ({}) before the merge \
                 could be made. Yulius leaves its pull request as it is: continue the task and \
                 call `delivery_merge` again with the reviewed head, or merge it on GitHub.",
                task.status.as_str()
            ))
        });
        let head = match task.worktree_path.as_deref() {
            Some(worktree) => self
                .deps
                .git
                .log(std::path::Path::new(worktree), "HEAD", 1)
                .await
                .ok()
                .and_then(|log| log.into_iter().next())
                .map(|commit| commit.sha),
            None => None,
        };
        // (Of an ended run only what became of its pull request is asked, below.)
        if ended.is_none() && head.as_deref() != Some(armed.head.as_str()) {
            // The agent's own clean merges of the integration branch keep the review, so the
            // armed merge follows its head (SPEC-083 REV-001; 12 merges were dropped this way on
            // 2026-10-12/13). Its checks then decide at that head.
            if let (Some(worktree), Some(now)) = (task.worktree_path.as_deref(), head.as_deref())
                && self
                    .review_covers(task, std::path::Path::new(worktree), &armed.head, now)
                    .await
            {
                self.note_armed(
                    &task.id,
                    format!(
                        "Your branch moved from {} to {} only through clean merges of the \
                         integration branch: the merge stays armed, and the run at {} decides.",
                        short(&armed.head),
                        short(now),
                        short(now)
                    ),
                );
                return Verdict::Keep(ArmedMerge {
                    head: now.to_owned(),
                    ..armed
                });
            }
            return Verdict::Decided(format!(
                "The merge you armed was dropped: you armed {} with `delivery_merge`, but the \
                 branch is now at {}. Get the independent review of the new head and call \
                 `delivery_merge` again.",
                short(&armed.head),
                head.as_deref().map_or("an unknown commit", short)
            ));
        }
        let user = match self.deps.store.user(&task.owner).await {
            Ok(Some(user)) if user.disabled_at.is_none() => user,
            _ => {
                return Verdict::Decided(
                    "The merge you armed was dropped: the task's owner can no longer act. \
                     Open an issue (`issue_create`) and end with your summary."
                        .to_owned(),
                );
            }
        };
        let auth = AuthContext {
            user,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let id = task.id.to_string();
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            return Verdict::Decided(
                "The merge you armed was dropped: the project's sandbox could not be read. Check \
                 `delivery_status` and call `delivery_merge` again."
                    .to_owned(),
            );
        };
        // A playground has no pull request: its run of the head merged into the integration
        // branch decides (SPEC-163 PCI-003).
        if sandbox.is_playground() {
            if let Some(ended) = ended {
                return ended;
            }
            return self
                .merge_armed_playground(task, &auth, &sandbox, armed)
                .await;
        }
        // SPEC-084 TRN-004: a merge train that holds this pull request brings it up to date and
        // merges it itself (its note says where it stands); nothing to ask GitHub meanwhile.
        if ended.is_none() && self.train_hold(task) == crate::merge_train::Hold::Held {
            return Verdict::Keep(armed);
        }
        let Ok(view) = self.task_delivery(&auth, &id).await else {
            return ended.unwrap_or(Verdict::Keep(armed));
        };
        let Some(pr) = view.pr.as_ref() else {
            if let Some(ended) = ended {
                return ended;
            }
            self.note_armed(
                &task.id,
                "Yulius cannot read this task's pull request right now; it tries again on its \
                 next pass."
                    .to_owned(),
            );
            return Verdict::Keep(armed);
        };
        // Merged already, by anyone: what counts is whether its head passed (SPEC-441 MQ-009).
        if pr.state == "merged" {
            return Verdict::Decided(self.merged_note(task, &sandbox, pr.number, true).await);
        }
        if let Some(ended) = ended {
            return ended;
        }
        // A task's change merges into the integration branch only (SPEC-483 D3): a pull request
        // pointed elsewhere since it was armed (the agent's own token may do that) is dropped,
        // never brought up to date with that branch or merged into it.
        let profile = crate::delivery::delivery_profile(&sandbox);
        if pr.base != profile.integration_branch {
            return Verdict::Decided(format!(
                "The merge you armed was dropped: pull request #{} goes into `{}`, not `{}`. A \
                 task's change merges into `{}` only: point the pull request back at it (`gh pr \
                 edit {} --base {}`), then call `delivery_merge` again.",
                pr.number,
                pr.base,
                profile.integration_branch,
                profile.integration_branch,
                pr.number,
                profile.integration_branch,
            ));
        }
        let integration = profile.integration_branch.clone();
        let require_checks = profile.require_checks;
        // With Yulius CI, its run at the armed head decides: GitHub may still show an older head
        // whose run passed, and the base must not be merged into one whose run still goes.
        let checks =
            crate::delivery::ci_checks_word(self.ci_gate_result(&sandbox, &armed.head).await)
                .map_or_else(|| pr.checks.clone(), str::to_owned);
        let green = checks == "success" || (checks == "none" && !require_checks);
        if checks == "failure" {
            return Verdict::Decided(format!(
                "The merge you armed was dropped: the checks of pull request #{} failed at \
                 {}. Read why with `ci_status`, fix it test-first, get the review of the new \
                 head and call `delivery_merge` again.",
                pr.number,
                short(&armed.head)
            ));
        }
        // Its own run (the pre-check) finishes on its own head: moving the head now would only
        // supersede it (SPEC-441 MQ-001; every merge into the base restarted every armed pull
        // request's run, F 2026-11-07).
        if !green {
            let note = if checks == "error" {
                format!(
                    "The own Yulius CI run of pull request #{} at {} ended in error (it could not \
                     run: disk, a time limit, the host), which is not a failure of the code: \
                     rerun it with `ci_rerun`, and Yulius merges once it passes.",
                    pr.number,
                    short(&armed.head)
                )
            } else {
                format!(
                    "Pull request #{} waits for its own Yulius CI run at {} ({checks}). Yulius \
                     never restarts it when `{integration}` moves; once it passes, Yulius merges \
                     the pull request, or first tests it on top of the current `{integration}`.",
                    pr.number,
                    short(&armed.head)
                )
            };
            self.note_armed(&task.id, note);
            return Verdict::Keep(armed);
        }
        // Green but behind its base: the next merge candidate tests it on top of the base
        // (SPEC-441 MQ-003), unless it goes one by one.
        if self.behind_integration(task, &sandbox).await
            && !self.one_by_one(&sandbox, task, &armed).await
        {
            let note = self.candidate_wait_note(task, &armed, pr.number).await;
            self.note_armed(&task.id, note);
            return Verdict::Keep(armed);
        }
        // DLV-203, once the pre-check passed: a branch behind its base is brought up to date, so
        // the run that decides is on what would be merged and GitHub's up-to-date rule never
        // refuses it.
        match self.keep_armed_current(task, &sandbox).await {
            Current::Unchanged => {}
            Current::Moved(head) => {
                self.note_armed(
                    &task.id,
                    format!(
                        "Yulius merged `{integration}` into pull request #{} (now at {}) after \
                         its own run passed: the run at that head decides, and Yulius merges it \
                         once it passes. The merge stays armed.",
                        pr.number,
                        short(&head)
                    ),
                );
                return Verdict::Keep(ArmedMerge { head, ..armed });
            }
            Current::Conflict(text) => {
                return Verdict::Decided(text);
            }
        }
        // Still behind its base (its agent was busy, so it was not brought up to date): it waits
        // instead of being refused as behind and dropped (SPEC-084 TRN-011).
        if self.behind_integration(task, &sandbox).await {
            self.note_armed(
                &task.id,
                format!(
                    "Pull request #{} passed its own run but is behind `{integration}`: Yulius \
                     brings it up to date once your turn ends with everything committed.",
                    pr.number
                ),
            );
            return Verdict::Keep(armed);
        }
        let number = pr.number;
        let base = pr.base.clone();
        match self.merge_task_pr_for_task(&auth, &id).await {
            Ok(_) => {
                let (actor, action, how) = armed.merge_audit(task.number);
                let detail = format!(
                    "merged at head {} {how}; armed while the checks ran",
                    armed.head
                );
                if let Err(err) = self
                    .deps
                    .store
                    .audit(
                        self.now(),
                        &actor,
                        action,
                        Some(&format!("{}#{}", sandbox.slug, task.number)),
                        Some(&detail),
                    )
                    .await
                {
                    tracing::warn!(task = %task.id, error = %err, "audit of an armed merge failed");
                }
                let next = self.after_merge_of(task, PROFILE_ASKS).await;
                Verdict::Decided(format!(
                    "Yulius merged pull request #{number} into `{base}` at {}: its checks \
                     passed. {next}",
                    short(&armed.head)
                ))
            }
            // GitHub could not take the pull request's new title for now: armed until the next
            // pass (SPEC-069 BPL-008).
            Err(MergeFailure::Later(err)) => {
                self.note_armed(
                    &task.id,
                    format!(
                        "Pull request #{number} passed and is up to date, but GitHub could not \
                         take the merge just now ({}); Yulius tries again on its next pass.",
                        err.public_message()
                    ),
                );
                Verdict::Keep(armed)
            }
            Err(MergeFailure::Refused(err)) => Verdict::Decided(format!(
                "The merge you armed was refused: the checks of pull request #{number} \
                 passed at {}, but {}. Fix that (`delivery_status` says what is missing) \
                 and call `delivery_merge` again.",
                short(&armed.head),
                err.public_message()
            )),
        }
    }

    /// Decides a playground task's armed merge (SPEC-163 PCI-003), the reviewed head being its head:
    /// squashed once Yulius CI passed on it merged into the integration branch as it is now (a new
    /// merge is built and tested when that branch moved), kept while the run goes, dropped with
    /// why when it failed, conflicts or is refused.
    async fn merge_armed_playground(
        &self,
        task: &TaskRecord,
        auth: &AuthContext,
        sandbox: &crate::model::SandboxRecord,
        armed: ArmedMerge,
    ) -> Verdict {
        use crate::delivery::playground::PlaygroundMerge;
        use yulius_domain::delivery::DeliveryStage;
        let integration = crate::delivery::delivery_profile(sandbox).integration_branch;
        // Merged meanwhile (a person's merge): nothing is left to decide.
        if matches!(
            self.deps.store.delivery(&task.id).await,
            Ok(Some(record)) if !matches!(record.stage, DeliveryStage::None | DeliveryStage::PrOpen)
        ) {
            return Verdict::Decided(format!(
                "Your branch is already merged into `{integration}`: nothing is left armed. End \
                 with your summary."
            ));
        }
        match self
            .playground_merge(&auth.user, task, Some(&armed.head), true)
            .await
        {
            Ok(PlaygroundMerge::Testing(..)) => Verdict::Keep(armed),
            Ok(PlaygroundMerge::Merged(_)) => {
                let detail = format!(
                    "playground: squashed at head {} reviewed independently ({}); armed while the \
                     checks ran",
                    armed.head,
                    if armed.summary.is_empty() {
                        "no summary"
                    } else {
                        armed.summary.as_str()
                    }
                );
                if let Err(err) = self
                    .deps
                    .store
                    .audit(
                        self.now(),
                        &format!("task:{}", task.number),
                        "delivery.agent.merge",
                        Some(&format!("{}#{}", sandbox.slug, task.number)),
                        Some(&detail),
                    )
                    .await
                {
                    tracing::warn!(task = %task.id, error = %err, "audit of an armed merge failed");
                }
                // A playground squashes a task once, with its whole plan: nothing is ahead.
                Verdict::Decided(format!(
                    "Yulius squashed your branch into `{integration}` at {}: its checks passed on \
                     it merged into `{integration}`. End with your summary.",
                    short(&armed.head)
                ))
            }
            // A refusal (red checks, a conflict, an unready task) ends it; a failure of Git or the
            // store is tried again on the next pass, as on GitHub.
            Err(err @ crate::error::AppError::Conflict(_)) => Verdict::Decided(format!(
                "The merge you armed was refused: {}. Fix that and call `delivery_merge` again.",
                err.public_message()
            )),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "deciding a playground's armed merge failed; kept");
                Verdict::Keep(armed)
            }
        }
    }

    /// DLV-203: an armed pull request whose branch no longer contains the integration branch is
    /// brought up to date by Yulius, gently: it merges `origin/<integration>` into the task's
    /// worktree (the repository's merge drivers apply, unlike a host's server-side update) and
    /// pushes the result as a plain fast-forward. Only while the agent is idle with nothing
    /// running and nothing uncommitted; a real conflict is aborted and left to the agent. The
    /// review carries over: the merge brings in only commits already reviewed and merged.
    async fn keep_armed_current(
        self: &Arc<Self>,
        task: &TaskRecord,
        sandbox: &crate::model::SandboxRecord,
    ) -> Current {
        use crate::ports::git::MergeIntegration;
        use yulius_domain::task::{Activity, TaskStatus};
        let (Some(worktree), Some(branch)) =
            (task.worktree_path.as_deref(), task.branch.as_deref())
        else {
            return Current::Unchanged;
        };
        let worktree = std::path::Path::new(worktree);
        let idle = task.status == TaskStatus::Running
            && task.activity.is_some_and(Activity::is_idle)
            && self.background_shells_of(&task.id).unwrap_or(0) == 0;
        if !idle || self.deps.git.has_changes(worktree).await.unwrap_or(true) {
            return Current::Unchanged;
        }
        let integration = crate::delivery::delivery_profile(sandbox).integration_branch;
        let Ok(token) = self.task_github_token(task, sandbox).await else {
            return Current::Unchanged;
        };
        let updating = self.now();
        let merged = self
            .deps
            .git
            .merge_integration(worktree, &integration, &token)
            .await;
        match &merged {
            Ok(MergeIntegration::Merged(_)) => {
                self.integration_update_span(&task.id, &integration, updating, "merged")
                    .await;
            }
            Ok(MergeIntegration::Conflict(_)) => {
                self.integration_update_span(&task.id, &integration, updating, "conflict")
                    .await;
            }
            _ => {}
        }
        match merged {
            Ok(MergeIntegration::UpToDate) => Current::Unchanged,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "an armed pull request could not be brought up to date");
                Current::Unchanged
            }
            Ok(MergeIntegration::Conflict(files)) => {
                let listed: Vec<String> = files.iter().take(8).map(|f| format!("`{f}`")).collect();
                Current::Conflict(format!(
                    "The merge you armed is on hold: `{branch}` conflicts with `{integration}` in \
                     {}. Bring it in yourself: `git fetch origin && git merge \
                     origin/{integration}`, resolve those files, rerun the affected tests, \
                     commit and end your turn so Yulius pushes; then get the review of the new \
                     head and call `delivery_merge` again. Never rebase or force-push.",
                    listed.join(", ")
                ))
            }
            Ok(MergeIntegration::Merged(head)) => {
                match self
                    .dlv_push_head(&task.id, worktree, branch, &head, &token)
                    .await
                {
                    Ok(()) => {
                        let detail = format!(
                            "merged {integration} into {branch} at {head} for the armed merge"
                        );
                        let _ = self
                            .deps
                            .store
                            .audit(
                                self.now(),
                                "yulius",
                                "delivery.branch_updated",
                                Some(&format!("{}#{}", sandbox.slug, task.number)),
                                Some(&detail),
                            )
                            .await;
                        Current::Moved(head)
                    }
                    Err(err) => {
                        self.set_push_problem(&task.id, &head, branch, None, &err);
                        Current::Unchanged
                    }
                }
            }
        }
    }

    /// Whether `task`'s branch lacks the integration branch as Yulius's clone last fetched it
    /// (every CI round fetches it); `false` when that cannot be told.
    async fn behind_integration(
        &self,
        task: &TaskRecord,
        sandbox: &crate::model::SandboxRecord,
    ) -> bool {
        let Some(worktree) = task.worktree_path.as_deref() else {
            return false;
        };
        let integration = crate::delivery::delivery_profile(sandbox).integration_branch;
        matches!(
            self.deps
                .git
                .is_ancestor(
                    std::path::Path::new(worktree),
                    &format!("origin/{integration}"),
                    "HEAD"
                )
                .await,
            Ok(false)
        )
    }

    /// Why a green armed pull request behind its base is not in a merge candidate yet, in one
    /// sentence (SPEC-441 MQ-007).
    async fn candidate_wait_note(&self, task: &TaskRecord, armed: &ArmedMerge, pr: u64) -> String {
        use yulius_domain::task::{Activity, TaskStatus};
        let lead = format!(
            "Pull request #{pr} passed its own run at {}, but the integration branch moved since:",
            short(&armed.head)
        );
        let idle = task.status == TaskStatus::Running
            && task.activity.is_some_and(Activity::is_idle)
            && self.background_shells_of(&task.id).unwrap_or(0) == 0;
        let dirty = match task.worktree_path.as_deref() {
            Some(worktree) => self
                .deps
                .git
                .has_changes(std::path::Path::new(worktree))
                .await
                .unwrap_or(true),
            None => true,
        };
        if !idle || dirty {
            return format!(
                "{lead} Yulius tests it on top of the current integration branch as soon as your \
                 turn ends with everything committed, and merges that branch in for you then. \
                 Do not merge it yourself for this."
            );
        }
        match self.train_running_note(task) {
            Some(other) => format!(
                "{lead} it goes into the next merge candidate, after the one Yulius tests now \
                 ({other})."
            ),
            None => format!(
                "{lead} it goes into a merge candidate on Yulius's next pass, which tests it on \
                 top of the current integration branch."
            ),
        }
    }

    /// [`after_merge`] for `task` as its plan is now (a store failure reads as nothing ahead).
    pub(crate) async fn after_merge_of(&self, task: &TaskRecord, asks: &str) -> String {
        let ahead = match self.deps.store.steps(&task.id).await {
            Ok(steps) => crate::delivery::steps_ahead_note(&steps),
            Err(_) => String::new(),
        };
        after_merge(&ahead, asks)
    }

    /// What a merge of `task`'s pull request `number` counts as (SPEC-441 MQ-009): the merged
    /// head and whether its own Yulius CI run passed there, whoever merged it.
    pub(crate) async fn merged_note(
        &self,
        task: &TaskRecord,
        sandbox: &crate::model::SandboxRecord,
        number: u64,
        next: bool,
    ) -> String {
        let integration = crate::delivery::delivery_profile(sandbox).integration_branch;
        // A merge of one part of the plan is not the task's end (SPEC-484 PLN-003).
        let ahead = match self.deps.store.steps(&task.id).await {
            Ok(steps) if next => crate::delivery::steps_ahead_note(&steps),
            _ => String::new(),
        };
        let head = match self.task_github_token(task, sandbox).await {
            Ok(token) => self
                .deps
                .github
                .get_pull(&token, &sandbox.repo_owner, &sandbox.repo_name, number)
                .await
                .ok()
                .map(|pull| pull.head_sha),
            Err(_) => None,
        };
        let Some(head) = head else {
            return format!(
                "Pull request #{number} is merged into `{integration}`. Yulius could not read its \
                 merged head from GitHub, so whether its own Yulius CI run passed there is not \
                 known yet: `delivery_status` says it once GitHub answers."
            );
        };
        merged_text(
            number,
            &integration,
            &head,
            self.ci_gate_result(sandbox, &head).await,
            next,
            &ahead,
        )
    }

    /// The state `yulius wait` compares (local records only).
    ///
    /// # Errors
    /// `Unauthenticated` for an unknown task token; store failures.
    pub async fn agent_wait_state(&self, token: &str, head: Option<&str>) -> AppResult<WaitState> {
        let ctx = self.agent_context(token).await?;
        let task = &ctx.task;
        let store = &self.deps.store;
        let head = head
            .map(str::trim)
            .filter(|h| h.len() >= 7 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let (checks, checks_detail) = if head.is_empty() {
            ("none".to_owned(), None)
        } else {
            let runs = store.ci_runs(&task.sandbox_id, 50).await?;
            // A playground task's run tests its head merged into the integration branch (SPEC-163
            // PCI-006): the newest that tested it.
            let run = match runs.iter().find(|run| run.sha.starts_with(&head)) {
                Some(run) => Some(run),
                None => self.task_run_of_head(task, &runs, &head).await,
            };
            match run {
                Some(run) => (run.state.as_str().to_owned(), run.reason.clone()),
                None => ("none".to_owned(), None),
            }
        };
        let stage = store
            .delivery(&task.id)
            .await?
            .map_or_else(|| "none".to_owned(), |record| record.stage.encode());
        // One read of the slot: the outcome and whether a merge is armed agree (WAI-003).
        let (merge_armed, outcome, note) = lock_slot(&self.runtime(&task.id)).view();
        let push_problem = self
            .runtime(&task.id)
            .push_problem
            .lock()
            .ok()
            .and_then(|problem| problem.clone())
            .filter(|note| {
                !head.is_empty() && (note.head.starts_with(&head) || head.starts_with(&note.head))
            })
            .map(|note| note.text);
        let cd = self.cd_latest_run_of_task(task).await?;
        // A run that ended is reported once, recorded with the run (a restart keeps it): the
        // state that shows it marks it, and the wait that gets it ends on it.
        let cd_unreported = match &cd {
            Some(run) if run.state != yulius_domain::ci::RunState::Running => {
                self.deps.store.mark_cd_run_reported(&run.id).await?
            }
            _ => false,
        };
        Ok(WaitState {
            cd_unreported,
            cd_run: cd.as_ref().map(|run| run.id.clone()),
            cd_state: cd.as_ref().map(|run| run.state.as_str().to_owned()),
            cd_detail: cd.map(|run| match run.reason {
                Some(reason) => format!(
                    "{}: {reason}",
                    yulius_domain::cd::env_label(&run.environment)
                ),
                None => yulius_domain::cd::env_label(&run.environment).to_owned(),
            }),
            head,
            checks,
            checks_detail,
            stage,
            merge_armed,
            merge_outcome_head: outcome.as_ref().map(|o| o.head.clone()),
            merge_outcome: outcome.map(|o| o.text),
            push_problem,
            merge_train: if merge_armed {
                self.train_note(task).or(note)
            } else {
                None
            },
        })
    }

    /// `POST /internal/agent/wait`: the state now when `since` is absent or already differs,
    /// else the first state that differs from it within `timeout` (at most [`WAIT_MAX`]), else
    /// the unchanged state. A note that a push of `head` failed is dropped first when GitHub's
    /// branch is at `head`.
    ///
    /// # Errors
    /// As [`App::agent_wait_state`].
    pub async fn agent_wait(
        &self,
        token: &str,
        head: Option<&str>,
        since: Option<&WaitState>,
        timeout: Duration,
    ) -> AppResult<WaitState> {
        let deadline = tokio::time::Instant::now() + timeout.min(WAIT_MAX);
        // A failed push of this head that GitHub's branch has since reached is old news: the
        // wait goes on to the checks and the armed merge (SPEC-139 PSH-002, #366). Once per
        // request: a note written during the wait comes from a push that just asked GitHub.
        if let Some(head) = head
            .map(str::trim)
            .filter(|h| h.len() >= 7 && h.chars().all(|c| c.is_ascii_hexdigit()))
        {
            let ctx = self.agent_context(token).await?;
            self.forget_push_problem_if_pushed(&ctx.task, &head.to_ascii_lowercase())
                .await;
        }
        loop {
            let state = self.agent_wait_state(token, head).await?;
            if since.is_none_or(|since| *since != state) || tokio::time::Instant::now() >= deadline
            {
                return Ok(state);
            }
            tokio::time::sleep(WAIT_POLL).await;
        }
    }
}

/// What a merge of pull request `number` into `integration` at `head` counts as, from Yulius
/// CI's result there (SPEC-441 MQ-009): merged on green when its own run passed, whoever merged
/// it. With `next`, what the agent does now follows: the end of the task, or, with steps of its
/// plan still ahead (`ahead`, [`crate::delivery::steps_ahead_note`]), the work that goes on
/// (SPEC-484 PLN-003).
pub(crate) fn merged_text(
    number: u64,
    integration: &str,
    head: &str,
    gate: yulius_domain::ci::GateResult,
    next: bool,
    ahead: &str,
) -> String {
    use yulius_domain::ci::{GateResult, RunState};
    let at = format!(
        "Pull request #{number} is merged into `{integration}` at {}",
        short(head)
    );
    let ahead = ahead.trim();
    let finish = after_merge(
        ahead,
        "Finish what the delivery profile still asks for this stage (`delivery_status`)",
    );
    let (fact, then) = match gate {
        GateResult::Run(Some(RunState::Passed | RunState::Skipped)) => (
            format!(
                "{at}, whose own Yulius CI run passed: it counts as merged on green Yulius CI, \
                 whoever merged it."
            ),
            finish,
        ),
        GateResult::NotConfigured => (
            format!("{at}; the project has no Yulius CI, so no run had to pass."),
            finish,
        ),
        GateResult::Run(Some(state @ (RunState::Queued | RunState::Running))) => (
            format!(
                "{at}, whose own Yulius CI run is still {}: it counts as merged on green once \
                 that run passes.",
                state.as_str()
            ),
            if ahead.is_empty() {
                "Wait for that run with `yulius wait` (run_in_background), then end with your \
                 summary."
                    .to_owned()
            } else {
                format!(
                    "Wait for that run with `yulius wait` (run_in_background), then go on with \
                     the next step. {ahead}"
                )
            },
        ),
        GateResult::Run(state) => (
            format!(
                "{at}, but Yulius CI did not pass at that head ({}): it does not count as merged \
                 on green.",
                state.map_or("no run", RunState::as_str)
            ),
            format!(
                "Read `ci_status` for `{integration}`, and fix what fails test-first on this \
                 branch: Yulius opens a new pull request for it."
            ),
        ),
    };
    if next { format!("{fact} {then}") } else { fact }
}

/// What the delivery profile may still ask after a merge Yulius made, for [`after_merge`].
pub(crate) const PROFILE_ASKS: &str = "Finish what the delivery profile still asks for this stage \
                                       (`delivery_status`; deploy and verify an integration \
                                       environment with `delivery_deploy`)";

/// What an agent does after a merge of its pull request: what the delivery profile still asks
/// for the merged part (`asks`, a clause), and then the end of the task, or, with steps of its
/// plan still ahead (`ahead`, [`crate::delivery::steps_ahead_note`]), the work that goes on: a
/// merge of one part of the plan is not the task's end (SPEC-484 PLN-003).
pub(crate) fn after_merge(ahead: &str, asks: &str) -> String {
    let ahead = ahead.trim();
    if ahead.is_empty() {
        format!("{asks}, then end with your summary. Do not promote unless the task asks for it.")
    } else {
        format!(
            "{asks}, then go on with the next step: the finished part of your plan is delivered. \
             {ahead} Do not promote unless the task asks for it."
        )
    }
}

/// The task's [`ArmedSlot`], also after a panic elsewhere held it (its values stay consistent:
/// each change is one assignment or a whole method).
fn lock_slot(runtime: &crate::supervisor::TaskRuntime) -> std::sync::MutexGuard<'_, ArmedSlot> {
    runtime
        .armed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The saved armed merges; none when the file is missing or unreadable.
fn read_armed(path: &std::path::Path) -> Vec<SavedArmed> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merge(head: &str) -> ArmedMerge {
        ArmedMerge {
            head: head.to_owned(),
            summary: "approve".to_owned(),
            armed_at: 0,
            by: None,
        }
    }

    /// WAI-003 (#255): a decision ends in one step, so the wait never reads an outcome next to a
    /// merge that still reads as armed, and always reads armed until that step.
    #[test]
    fn a_decision_ends_in_one_step() -> Result<(), &'static str> {
        let mut slot = ArmedSlot::default();
        slot.arm(merge("aaa"));
        assert_eq!(slot.view(), (true, None, None));
        let deciding = slot.take().ok_or("armed")?;
        assert_eq!(slot.view(), (true, None, None), "armed while it is decided");
        assert!(slot.take().is_none(), "decided once at a time");
        assert!(slot.finish(&deciding, Verdict::Decided("dropped".to_owned())));
        let outcome = MergeOutcome {
            head: "aaa".to_owned(),
            text: "dropped".to_owned(),
            by: None,
        };
        assert_eq!(slot.view(), (false, Some(outcome), None));
        // Arming again forgets it.
        slot.arm(merge("bbb"));
        assert_eq!(slot.view(), (true, None, None));
        Ok(())
    }

    /// A merge kept (checks pending) goes back as it was; one moved to a new head is saved.
    #[test]
    fn a_kept_merge_goes_back() -> Result<(), &'static str> {
        let mut slot = ArmedSlot::default();
        slot.arm(merge("aaa"));
        let deciding = slot.take().ok_or("armed")?;
        assert!(
            !slot.finish(&deciding, Verdict::Keep(merge("aaa"))),
            "unchanged"
        );
        assert_eq!(slot.view(), (true, None, None));
        let deciding = slot.take().ok_or("armed")?;
        assert!(slot.finish(&deciding, Verdict::Keep(merge("ccc"))), "moved");
        assert_eq!(slot.merge, Some(merge("ccc")));
        Ok(())
    }

    /// WAI-002 (#248): a merge armed (or merged at once) while an earlier one is decided
    /// supersedes it; that decision neither puts its merge back nor leaves its outcome.
    #[test]
    fn a_newer_arm_supersedes_a_decision_in_progress() -> Result<(), &'static str> {
        for verdict in [
            Verdict::Keep(merge("aaa")),
            Verdict::Decided("dropped".to_owned()),
        ] {
            let mut slot = ArmedSlot::default();
            slot.arm(merge("aaa"));
            let deciding = slot.take().ok_or("armed")?;
            slot.arm(merge("bbb"));
            assert!(!slot.finish(&deciding, verdict.clone()), "nothing to save");
            assert_eq!(slot.merge, Some(merge("bbb")));
            assert_eq!(slot.view(), (true, None, None));

            let mut slot = ArmedSlot::default();
            slot.arm(merge("aaa"));
            let deciding = slot.take().ok_or("armed")?;
            slot.clear();
            assert!(!slot.finish(&deciding, verdict));
            assert_eq!(slot.view(), (false, None, None));
        }
        Ok(())
    }

    /// SPEC-441 MQ-007: the sentence a decision wrote stays while that decision keeps the merge
    /// armed, goes with its outcome or a newer arm, and a superseded decision's is dropped.
    #[test]
    fn a_decision_leaves_where_the_merge_stands() -> Result<(), &'static str> {
        let mut slot = ArmedSlot::default();
        slot.arm(merge("aaa"));
        let deciding = slot.take().ok_or("armed")?;
        slot.pending_note = Some("waits for its own run".to_owned());
        slot.finish(&deciding, Verdict::Keep(merge("aaa")));
        assert_eq!(slot.view().2.as_deref(), Some("waits for its own run"));
        // The next decision writes nothing: the old sentence is not kept.
        let deciding = slot.take().ok_or("armed")?;
        slot.finish(&deciding, Verdict::Keep(merge("aaa")));
        assert_eq!(slot.view().2, None);

        let deciding = slot.take().ok_or("armed")?;
        slot.pending_note = Some("in a candidate".to_owned());
        slot.finish(&deciding, Verdict::Decided("merged".to_owned()));
        assert_eq!(slot.view().2, None, "decided");

        slot.arm(merge("bbb"));
        let deciding = slot.take().ok_or("armed")?;
        slot.pending_note = Some("stale".to_owned());
        slot.arm(merge("ccc"));
        slot.finish(&deciding, Verdict::Keep(merge("bbb")));
        assert_eq!(slot.view().2, None, "superseded");
        Ok(())
    }

    /// SPEC-441 MQ-009: a merge counts as on green by its head's own run, whoever made it.
    #[test]
    fn a_merge_counts_by_its_heads_run() {
        use yulius_domain::ci::{GateResult, RunState};
        let green = merged_text(
            7,
            "dev",
            "abcdef0123",
            GateResult::Run(Some(RunState::Passed)),
            true,
            "",
        );
        assert!(
            green.starts_with("Pull request #7 is merged into `dev` at abcdef012, whose own Yulius CI run passed: it counts as merged on green Yulius CI, whoever merged it."),
            "{green}"
        );
        assert!(green.contains("end with your summary"), "{green}");
        let red = merged_text(
            7,
            "dev",
            "abcdef0123",
            GateResult::Run(Some(RunState::Failed)),
            false,
            "",
        );
        assert_eq!(
            red,
            "Pull request #7 is merged into `dev` at abcdef012, but Yulius CI did not pass at \
             that head (failed): it does not count as merged on green."
        );
        let none = merged_text(7, "dev", "abcdef0123", GateResult::Run(None), false, "");
        assert!(none.contains("(no run)"), "{none}");
    }

    /// SPEC-484 PLN-003: a merge of one part of the plan is not the task's end.
    #[test]
    fn a_merge_with_steps_ahead_says_the_task_goes_on() {
        use yulius_domain::ci::{GateResult, RunState};
        let ahead = " 2 step(s) of your plan are still ahead (next: \"Slice 2\"): they stay \
                     open, and the work you commit for them on this branch is delivered next, \
                     by a `delivery_merge` of its own.";
        for gate in [
            GateResult::Run(Some(RunState::Passed)),
            GateResult::NotConfigured,
            GateResult::Run(Some(RunState::Running)),
        ] {
            let text = merged_text(7, "dev", "abcdef0123", gate, true, ahead);
            assert!(
                text.contains("2 step(s) of your plan are still ahead (next: \"Slice 2\")"),
                "{text}"
            );
            assert!(text.contains("the next step"), "{text}");
            assert!(!text.contains("end with your summary"), "{text}");
            assert!(!text.contains("  "), "{text}");
        }
        // What the merge counts as is said without a next step, steps ahead or not.
        let fact = merged_text(
            7,
            "dev",
            "abcdef0123",
            GateResult::Run(Some(RunState::Passed)),
            false,
            ahead,
        );
        assert!(!fact.contains("ahead"), "{fact}");
        // What the profile asks for the merged part stays; only the task's end gives way.
        assert_eq!(
            after_merge(ahead, PROFILE_ASKS),
            "Finish what the delivery profile still asks for this stage (`delivery_status`; \
             deploy and verify an integration environment with `delivery_deploy`), then go on \
             with the next step: the finished part of your plan is delivered. 2 step(s) of your \
             plan are still ahead (next: \"Slice 2\"): they stay open, and the work you commit \
             for them on this branch is delivered next, by a `delivery_merge` of its own. Do not \
             promote unless the task asks for it."
        );
        assert_eq!(
            after_merge("", PROFILE_ASKS),
            "Finish what the delivery profile still asks for this stage (`delivery_status`; \
             deploy and verify an integration environment with `delivery_deploy`), then end with \
             your summary. Do not promote unless the task asks for it."
        );
    }
}
