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
    }

    /// Nothing is armed any more (merged at once by `delivery_merge`); a decision in progress is
    /// superseded.
    fn clear(&mut self) {
        self.generation += 1;
        self.merge = None;
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
        if self.generation != deciding.generation {
            return false;
        }
        match verdict {
            Verdict::Keep(merge) => {
                let changed = merge != deciding.merge;
                self.merge = Some(merge);
                changed
            }
            Verdict::Decided(text) => {
                self.outcome = Some(MergeOutcome {
                    head: deciding.merge.head.clone(),
                    text,
                });
                true
            }
        }
    }

    /// Whether a merge is armed (counting one being decided), and how the last one ended.
    fn view(&self) -> (bool, Option<MergeOutcome>) {
        (
            self.merge.is_some() || self.deciding > 0,
            self.outcome.clone(),
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
    /// Arms a merge of `head` for when its checks pass (`delivery_merge` while they still run).
    pub(crate) fn arm_merge(&self, task: &TaskRecord, head: &str, summary: &str) {
        let merge = ArmedMerge {
            head: head.to_owned(),
            summary: summary.to_owned(),
            armed_at: self.now().0,
        };
        let runtime = self.runtime(&task.id);
        let mut slot = lock_slot(&runtime);
        self.save_armed(&task.id, Some(&merge));
        slot.arm(merge);
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
    /// the file follows the slot's order of changes; only the startup restore forgets a task that
    /// is not running without it. Failures are logged: the merge stays armed in memory.
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

    /// Puts back the armed merges of running tasks after a restart (DLV-205); the others are
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
            let running = matches!(
                self.deps.store.task(&task_id).await,
                Ok(Some(task)) if task.status == yulius_domain::task::TaskStatus::Running
            );
            if !running {
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
                });
                tracing::info!(task = %task_id, "armed merge restored after a restart");
            }
        }
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
        let pass = {
            let mut passes = self
                .ci
                .armed_passes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(passes.entry(sandbox.as_str().to_owned()).or_default())
        };
        let _pass = pass.lock().await;
        self.run_merge_train(sandbox).await;
        let Ok(tasks) = self.deps.store.tasks(sandbox, false).await else {
            return;
        };
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

    /// Merges one armed task when its checks passed; says whether to keep waiting or how it
    /// ended.
    async fn merge_armed_task(self: &Arc<Self>, task: &TaskRecord, armed: ArmedMerge) -> Verdict {
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
        if head.as_deref() != Some(armed.head.as_str()) {
            // The agent's own clean merges of the integration branch keep the review, so the
            // armed merge follows its head (SPEC-083 REV-001; 12 merges were dropped this way on
            // 2026-10-12/13). Its checks then decide at that head.
            if let (Some(worktree), Some(now)) = (task.worktree_path.as_deref(), head.as_deref())
                && self
                    .review_covers(task, std::path::Path::new(worktree), &armed.head, now)
                    .await
            {
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
        };
        let id = task.id.to_string();
        let Ok(view) = self.task_delivery(&auth, &id).await else {
            return Verdict::Keep(armed);
        };
        let Some(pr) = view.pr.as_ref() else {
            return Verdict::Keep(armed);
        };
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            return Verdict::Decided(
                "The merge you armed was dropped: the project's sandbox could not be read. Check \
                 `delivery_status` and call `delivery_merge` again."
                    .to_owned(),
            );
        };
        // SPEC-084 TRN-004: a merge train that holds this pull request brings it up to date and
        // merges it itself.
        if self.train_hold(task) == crate::merge_train::Hold::Held {
            return Verdict::Keep(armed);
        }
        // DLV-203: a branch behind its base is brought up to date first, so the run that decides
        // is on what would be merged and GitHub's up-to-date rule never refuses it.
        match self.keep_armed_current(task, &sandbox).await {
            Current::Unchanged => {}
            Current::Moved(head) => {
                return Verdict::Keep(ArmedMerge {
                    head,
                    summary: armed.summary,
                    armed_at: armed.armed_at,
                });
            }
            Current::Conflict(text) => {
                return Verdict::Decided(text);
            }
        }
        let require_checks = crate::delivery::delivery_profile(&sandbox).require_checks;
        let green = pr.checks == "success" || (pr.checks == "none" && !require_checks);
        if pr.checks == "failure" {
            return Verdict::Decided(format!(
                "The merge you armed was dropped: the checks of pull request #{} failed at \
                 {}. Read why with `ci_status`, fix it test-first, get the review of the new \
                 head and call `delivery_merge` again.",
                pr.number,
                short(&armed.head)
            ));
        }
        if !green {
            return Verdict::Keep(armed);
        }
        // Still behind its base (its agent was busy, so it was not brought up to date): it waits
        // instead of being refused as behind and dropped (SPEC-084 TRN-011).
        if self.behind_integration(task, &sandbox).await {
            return Verdict::Keep(armed);
        }
        let number = pr.number;
        let base = pr.base.clone();
        match self.merge_task_pr_for_task(&auth, &id).await {
            Ok(_) => {
                let detail = format!(
                    "merged at head {} reviewed independently ({}); armed while the checks ran",
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
                Verdict::Decided(format!(
                    "Yulius merged pull request #{number} into `{base}` at {}: its checks \
                     passed. Finish what the delivery profile still asks for this stage \
                     (`delivery_status`; deploy and verify an integration environment with \
                     `delivery_deploy`), then end with your summary. Do not promote unless \
                     the task asks for it.",
                    short(&armed.head)
                ))
            }
            // GitHub could not take the pull request's new title for now: armed until the next
            // pass (SPEC-069 BPL-008).
            Err(MergeFailure::Later(_)) => Verdict::Keep(armed),
            Err(MergeFailure::Refused(err)) => Verdict::Decided(format!(
                "The merge you armed was refused: the checks of pull request #{number} \
                 passed at {}, but {}. Fix that (`delivery_status` says what is missing) \
                 and call `delivery_merge` again.",
                short(&armed.head),
                err.public_message()
            )),
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
            match runs.iter().find(|run| run.sha.starts_with(&head)) {
                Some(run) => (run.state.as_str().to_owned(), run.reason.clone()),
                None => ("none".to_owned(), None),
            }
        };
        let stage = store
            .delivery(&task.id)
            .await?
            .map_or_else(|| "none".to_owned(), |record| record.stage.encode());
        // One read of the slot: the outcome and whether a merge is armed agree (WAI-003).
        let (merge_armed, outcome) = lock_slot(&self.runtime(&task.id)).view();
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
                Some(reason) => format!("{}: {reason}", run.environment.label()),
                None => run.environment.label().to_owned(),
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
                self.train_note(task)
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
        }
    }

    /// WAI-003 (#255): a decision ends in one step, so the wait never reads an outcome next to a
    /// merge that still reads as armed, and always reads armed until that step.
    #[test]
    fn a_decision_ends_in_one_step() -> Result<(), &'static str> {
        let mut slot = ArmedSlot::default();
        slot.arm(merge("aaa"));
        assert_eq!(slot.view(), (true, None));
        let deciding = slot.take().ok_or("armed")?;
        assert_eq!(slot.view(), (true, None), "armed while it is decided");
        assert!(slot.take().is_none(), "decided once at a time");
        assert!(slot.finish(&deciding, Verdict::Decided("dropped".to_owned())));
        let outcome = MergeOutcome {
            head: "aaa".to_owned(),
            text: "dropped".to_owned(),
        };
        assert_eq!(slot.view(), (false, Some(outcome)));
        // Arming again forgets it.
        slot.arm(merge("bbb"));
        assert_eq!(slot.view(), (true, None));
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
        assert_eq!(slot.view(), (true, None));
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
            assert_eq!(slot.view(), (true, None));

            let mut slot = ArmedSlot::default();
            slot.arm(merge("aaa"));
            let deciding = slot.take().ok_or("armed")?;
            slot.clear();
            assert!(!slot.finish(&deciding, verdict));
            assert_eq!(slot.view(), (false, None));
        }
        Ok(())
    }
}
