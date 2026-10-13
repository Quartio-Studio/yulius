//! Merge trains (SPEC-084, ADR-0067): when two or more armed pull requests wait, Yulius tests
//! them together and merges them one after another on that one verdict.
//!
//! Under a strict "up to date" rule only one pull request can merge per CI run: each merge puts
//! every other armed one behind, Yulius merges the integration branch into it (DLV-203) and CI
//! runs again (F 2026-10-13: 96 such updates in two days, 44% of the CI time kept).
//!
//! A train is built in Yulius's clone, exactly as the merges will happen: the integration branch
//! is merged into each armed head in arm order (the repository's merge drivers apply), each
//! result landing as the profile's merge method lands it, so the train's last commit has the
//! tree the integration branch has once every member merged. A head that conflicts is left out.
//! Yulius CI runs once on that commit (reference `train/<numbers>`, never published on GitHub).
//!
//! While it runs, the members after the first are held: not brought up to date (that would only
//! start runs the train makes useless) and not merged while behind. When it passes, the train
//! merges them in order: it brings each up to date as DLV-203 does, checks that the result has
//! exactly the tree the train predicted, records the train's verdict at that head (published as
//! `yulius/ci`, so GitHub's own rules are met without a new run), waits until GitHub shows the
//! new head and merges it through the normal merge path with all its gates. The integration
//! branch must have the predicted tree before each step.
//!
//! Anything unexpected dissolves the train and every member goes back to the one-by-one flow:
//! a red, errored or cancelled run (its members then merge one by one until armed again), the
//! integration branch moving another way, a member re-armed, busy, changed or refused, a tree
//! that differs, GitHub not catching up within [`WAIT_LIMIT_MS`]. A train can only make merging
//! faster, never stop it.
//!
//! State: `<data>/merge-trains.json` (one train per sandbox, written whole and renamed into
//! place), the CI run in the store and the commits in the clone; so a restart picks the train up
//! where it was (armed merges come back through DLV-205).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_domain::ci::{CiRef, Gate, GateResult, GateSource, RunState};
use yulius_domain::delivery::{DeliveryStage, MergeMethod};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};

use crate::agent_wait::{ArmedMerge, ArmedNow, Deciding, Verdict};
use crate::auth::AuthContext;
use crate::ci::{CiJobRecord, CiRunRecord};
use crate::context::App;
use crate::delivery::{delivery_profile, past_merge};
use crate::model::{SandboxRecord, TaskRecord};
use crate::ports::git::{MergeIntegration, TrainLanding, TrainStep};

/// The file in the data directory that keeps merge trains across restarts.
pub const MERGE_TRAINS_FILE: &str = "merge-trains.json";
/// Most pull requests in one train (bounded work per build and per merge round).
pub const MAX_MEMBERS: usize = 8;
/// Who asked for a verdict a train recorded at a pull request head that is not its last: a tree
/// only covered by the train's run as a step towards it, never a source for reusing a result.
pub const TRAIN_VERDICT_BY: &str = "yulius:merge-train";
/// The commit status note of a train's own run.
pub const TRAIN_STATUS_NOTE: &str =
    "not published: a merge train's commit exists only in Yulius's clone";
/// Reads of a pull request while GitHub has not shown a pushed head yet, in one round.
const HEAD_POLLS: u32 = 5;
/// Between those reads.
const HEAD_POLL_GAP: Duration = Duration::from_secs(1);
/// How long a train that passed may wait for GitHub (or a member's own run) before it is
/// dissolved and its members merge one by one.
pub const WAIT_LIMIT_MS: i64 = 15 * 60 * 1000;

/// Serializes changes to [`MERGE_TRAINS_FILE`].
static TRAINS_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A sandbox's merge train and what it remembers between trains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SandboxTrains {
    #[serde(default)]
    train: Option<Train>,
    /// Armed merges (task, when armed) whose train did not pass: they merge one by one until
    /// their agent arms again.
    #[serde(default)]
    held_back: Vec<(String, i64)>,
    /// The input (base and heads) of the last build that formed no train: not built again.
    #[serde(default)]
    unformed: Option<String>,
}

impl SandboxTrains {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One train.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Train {
    /// Its CI run.
    run: String,
    /// The commit that run tests: the integration branch once every member merged.
    commit: String,
    /// The integration branch commit it was built on, and its tree.
    base: String,
    base_tree: String,
    /// In merge order.
    members: Vec<Member>,
    /// Armed pull requests that conflicted on top of the members before them.
    #[serde(default)]
    left_out: Vec<LeftOut>,
    /// The run passed and the members are being merged.
    #[serde(default)]
    merging: bool,
    /// Since when a passed train waits (for GitHub or a member's own run).
    #[serde(default)]
    waiting_since: Option<i64>,
}

/// A pull request in a train.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Member {
    task: String,
    /// Pull request number.
    pr: u64,
    /// The armed head it was built with.
    head: String,
    /// When its merge was armed: a new arm takes it out of the train.
    armed_at: i64,
    /// The integration branch's tree once it merged.
    tree: String,
}

/// An armed pull request left out of a train.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LeftOut {
    task: String,
    pr: u64,
    files: Vec<String>,
}

/// How a merge train holds an armed pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    /// Not in a train, or first in one under test: the one-by-one flow as before.
    Free,
    /// In a train under test, not first: not brought up to date, not merged while behind.
    Testing,
    /// In a train that passed (the train merges it), or left out of one: the ones armed before
    /// it go first, and it waits until the train is over.
    Held,
}

/// What one step of a passed train did.
enum Step {
    /// The member merged; go on with the next.
    Merged,
    /// Not yet (GitHub has not shown the head, the member's own run still runs): next pass.
    Wait,
    /// The train is dissolved, for this reason.
    Stop(String),
}

/// An armed pull request that could join a train.
struct Candidate {
    task: TaskRecord,
    armed: ArmedMerge,
    pr: u64,
}

fn hold_of(train: Option<&Train>, task: &str) -> Hold {
    let Some(train) = train else {
        return Hold::Free;
    };
    if train.left_out.iter().any(|left| left.task == task) {
        return Hold::Held;
    }
    match train.members.iter().position(|m| m.task == task) {
        None => Hold::Free,
        Some(_) if train.merging => Hold::Held,
        Some(0) => Hold::Free,
        Some(_) => Hold::Testing,
    }
}

/// The integration branch's tree once the first `merged` members merged.
fn tree_after(train: &Train, merged: usize) -> &str {
    match merged.checked_sub(1).and_then(|i| train.members.get(i)) {
        Some(member) => &member.tree,
        None => &train.base_tree,
    }
}

fn numbers(members: &[Member]) -> Vec<u64> {
    members.iter().map(|m| m.pr).collect()
}

fn listed(numbers: &[u64]) -> String {
    numbers
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}

/// What `yulius wait` says about `task`'s place in `train`.
fn note_of(train: &Train, task: &str) -> Option<String> {
    let prs = numbers(&train.members);
    if let Some(index) = train.members.iter().position(|m| m.task == task) {
        let place = format!(
            "pull request #{} is number {} of {} in merge train {}",
            train.members[index].pr,
            index + 1,
            prs.len(),
            listed(&prs)
        );
        return Some(if train.merging {
            format!("{place}: their combined result passed; Yulius merges them in this order.")
        } else {
            format!(
                "{place}: Yulius CI tests their combined result (run {}) and Yulius merges them \
                 in this order when it passes.",
                train.run
            )
        });
    }
    train.left_out.iter().find(|l| l.task == task).map(|left| {
        let files: Vec<String> = left
            .files
            .iter()
            .take(8)
            .map(|f| format!("`{f}`"))
            .collect();
        format!(
            "pull request #{} was left out of merge train {}: merged on top of them it conflicts \
             in {}. It merges one by one after them; if it then conflicts with the integration \
             branch, Yulius says what to resolve.",
            left.pr,
            listed(&prs),
            files.join(", ")
        )
    })
}

/// Items in arm order (`key`: when armed, then task number), at most [`MAX_MEMBERS`].
fn in_arm_order<T>(mut items: Vec<T>, key: impl Fn(&T) -> (i64, u32)) -> Vec<T> {
    items.sort_by_key(|item| key(item));
    items.truncate(MAX_MEMBERS);
    items
}

impl App {
    fn trains_path(&self) -> std::path::PathBuf {
        self.config.data_dir.join(MERGE_TRAINS_FILE)
    }

    fn read_trains(&self, sandbox: &SandboxId) -> SandboxTrains {
        let _guard = TRAINS_FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read_all(&self.trains_path())
            .remove(sandbox.as_str())
            .unwrap_or_default()
    }

    /// Saves `state` for `sandbox` (written whole, then renamed into place); failures are
    /// logged, and the train is then only as current as the file.
    fn write_trains(&self, sandbox: &SandboxId, state: &SandboxTrains) {
        let _guard = TRAINS_FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.trains_path();
        let mut all = read_all(&path);
        if state.is_empty() {
            all.remove(sandbox.as_str());
        } else {
            all.insert(sandbox.as_str().to_owned(), state.clone());
        }
        let written = serde_json::to_vec(&all)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &path)
            });
        if let Err(err) = written {
            tracing::warn!(error = %err, "merge trains could not be saved");
        }
    }

    /// How a merge train holds `task`'s armed merge (SPEC-084 TRN-004).
    pub(crate) fn train_hold(&self, task: &TaskRecord) -> Hold {
        hold_of(
            self.read_trains(&task.sandbox_id).train.as_ref(),
            task.id.as_str(),
        )
    }

    /// What `yulius wait` says about `task`'s merge train, if it is in one or was left out.
    pub(crate) fn train_note(&self, task: &TaskRecord) -> Option<String> {
        let state = self.read_trains(&task.sandbox_id);
        note_of(state.train.as_ref()?, task.id.as_str())
    }

    /// Whether a held task's branch lacks the integration branch (or that cannot be read).
    pub(crate) async fn train_member_behind(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
    ) -> bool {
        let Some(worktree) = task.worktree_path.as_deref() else {
            return true;
        };
        let integration = delivery_profile(sandbox).integration_branch;
        !matches!(
            self.deps
                .git
                .is_ancestor(
                    Path::new(worktree),
                    &format!("origin/{integration}"),
                    "HEAD"
                )
                .await,
            Ok(true)
        )
    }

    /// Forms `sandbox`'s merge train or advances it: the start of every [`App::merge_armed`]
    /// pass, which holds the sandbox's pass lock (one at a time).
    pub(crate) async fn run_merge_train(self: &Arc<Self>, sandbox_id: &SandboxId) {
        if !self.config.ci.enabled {
            return;
        }
        let Ok(Some(sandbox)) = self.deps.store.sandbox(sandbox_id).await else {
            return;
        };
        if sandbox.removed_at.is_some() {
            return;
        }
        let mut state = self.read_trains(sandbox_id);
        let before = state.clone();
        state.train = match state.train.take() {
            Some(train) => self.advance_train(&sandbox, &mut state, train).await,
            None => self.form_train(&sandbox, &mut state).await,
        };
        if state != before {
            self.write_trains(sandbox_id, &state);
        }
    }

    async fn train_audit(&self, sandbox: &SandboxRecord, action: &str, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                "yulius",
                action,
                Some(&sandbox.slug),
                Some(detail),
            )
            .await
        {
            tracing::warn!(sandbox = %sandbox.slug, error = %err, "audit of a merge train failed");
        }
    }

    /// Ends `train` for `reason`: its run is cancelled while it is still going, and every
    /// member goes back to the one-by-one flow.
    async fn dissolve(&self, sandbox: &SandboxRecord, train: &Train, reason: &str) {
        let _ = self
            .ci_cancel(
                &train.run,
                &format!("the merge train was dissolved: {reason}"),
            )
            .await;
        tracing::info!(sandbox = %sandbox.slug, run = %train.run, reason, "merge train dissolved");
        self.train_audit(
            sandbox,
            "delivery.train.dissolved",
            &format!(
                "merge train {} (run {}): {reason}; its pull requests merge one by one",
                listed(&numbers(&train.members)),
                train.run
            ),
        )
        .await;
    }

    async fn advance_train(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        state: &mut SandboxTrains,
        train: Train,
    ) -> Option<Train> {
        let run = match self.deps.store.ci_run(&train.run).await {
            Ok(Some(run)) => run,
            Ok(None) => {
                self.dissolve(sandbox, &train, "its CI run is gone").await;
                return None;
            }
            Err(_) => return Some(train),
        };
        match run.state {
            RunState::Queued | RunState::Running => {
                if let Some(reason) = self.train_broken(sandbox, &train).await {
                    self.dissolve(sandbox, &train, &reason).await;
                    return None;
                }
                Some(train)
            }
            RunState::Passed => self.drive_train(sandbox, train).await,
            other => {
                // Red: its members merge one by one, tested on their own, until armed again.
                state
                    .held_back
                    .extend(train.members.iter().map(|m| (m.task.clone(), m.armed_at)));
                self.dissolve(
                    sandbox,
                    &train,
                    &format!("its CI run ended {}", other.as_str()),
                )
                .await;
                None
            }
        }
    }

    /// Why a train under test no longer predicts what will merge, if it does not.
    async fn train_broken(&self, sandbox: &SandboxRecord, train: &Train) -> Option<String> {
        let mut merged = 0;
        for (index, member) in train.members.iter().enumerate() {
            let Ok(task) = TaskId::parse(&member.task) else {
                return Some("a member is unknown".to_owned());
            };
            let landed = matches!(
                self.deps.store.delivery(&task).await,
                Ok(Some(record)) if past_merge(&record.stage)
            );
            if landed {
                if merged != index {
                    return Some(format!("#{} merged before the ones ahead of it", member.pr));
                }
                merged += 1;
                continue;
            }
            match self.armed_now(&task) {
                ArmedNow::Armed(armed) if armed.armed_at != member.armed_at => {
                    return Some(format!("#{} was armed again", member.pr));
                }
                ArmedNow::None => return Some(format!("#{} is no longer armed", member.pr)),
                ArmedNow::Armed(_) | ArmedNow::Deciding => {}
            }
        }
        let integration = delivery_profile(sandbox).integration_branch;
        let tree = self
            .deps
            .git
            .tree_of(
                &self.sandbox_repo(&sandbox.id),
                &format!("origin/{integration}"),
            )
            .await
            .ok()
            .flatten();
        (tree.as_deref() != Some(tree_after(train, merged)))
            .then(|| format!("`{integration}` moved to a state the train did not predict"))
    }

    /// Merges a passed train's members in order, as far as it can now.
    async fn drive_train(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        mut train: Train,
    ) -> Option<Train> {
        train.merging = true;
        let integration = delivery_profile(sandbox).integration_branch;
        let repo = self.sandbox_repo(&sandbox.id);
        if let Err(err) = self.dlv_refresh_clone(sandbox).await {
            tracing::warn!(sandbox = %sandbox.slug, error = %err, "a merge train could not fetch");
            return self.train_waits(sandbox, train).await;
        }
        for index in 0..train.members.len() {
            let member = &train.members[index];
            let Ok(task_id) = TaskId::parse(&member.task) else {
                self.dissolve(sandbox, &train, "a member is unknown").await;
                return None;
            };
            if matches!(
                self.deps.store.delivery(&task_id).await,
                Ok(Some(record)) if past_merge(&record.stage)
            ) {
                continue;
            }
            let tree = self
                .deps
                .git
                .tree_of(&repo, &format!("origin/{integration}"))
                .await
                .ok()
                .flatten();
            if tree.as_deref() != Some(tree_after(&train, index)) {
                let reason = format!(
                    "`{integration}` is not in the state the train predicted before #{}",
                    member.pr
                );
                self.dissolve(sandbox, &train, &reason).await;
                return None;
            }
            match self.train_step(sandbox, &train, index).await {
                Step::Merged => train.waiting_since = None,
                Step::Wait => return self.train_waits(sandbox, train).await,
                Step::Stop(reason) => {
                    self.dissolve(sandbox, &train, &reason).await;
                    return None;
                }
            }
        }
        let landed = self
            .deps
            .git
            .tree_of(&repo, &format!("origin/{integration}"))
            .await
            .ok()
            .flatten();
        let detail = if landed.as_deref() == Some(tree_after(&train, train.members.len())) {
            format!(
                "merge train {} merged into {integration}; its tree is the one run {} tested",
                listed(&numbers(&train.members)),
                train.run
            )
        } else {
            format!(
                "merge train {} merged into {integration}, but its tree differs from the one run \
                 {} tested: its own CI decides",
                listed(&numbers(&train.members)),
                train.run
            )
        };
        self.train_audit(sandbox, "delivery.train.merged", &detail)
            .await;
        None
    }

    /// A passed train that cannot go on now: kept for the next pass, unless it has waited
    /// [`WAIT_LIMIT_MS`] already.
    async fn train_waits(&self, sandbox: &SandboxRecord, mut train: Train) -> Option<Train> {
        let since = *train.waiting_since.get_or_insert(self.now().0);
        if self.now().0 - since > WAIT_LIMIT_MS {
            self.dissolve(sandbox, &train, "it waited too long to merge")
                .await;
            return None;
        }
        Some(train)
    }

    /// Decides member `index` of a passed train (its armed merge taken while it is decided).
    async fn train_step(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        train: &Train,
        index: usize,
    ) -> Step {
        let member = &train.members[index];
        let Ok(task_id) = TaskId::parse(&member.task) else {
            return Step::Stop("a member is unknown".to_owned());
        };
        let Some(deciding) = self.take_armed(&task_id) else {
            return match self.armed_now(&task_id) {
                ArmedNow::Deciding => Step::Wait,
                _ => Step::Stop(format!("#{} is no longer armed", member.pr)),
            };
        };
        let task = match self.deps.store.task(&task_id).await {
            Ok(Some(task)) => task,
            _ => {
                let verdict = Verdict::Keep(deciding.merge.clone());
                self.finish_armed(&task_id, &deciding, verdict);
                return Step::Stop(format!("#{} cannot be read", member.pr));
            }
        };
        let (verdict, step) = self
            .train_member(sandbox, train, index, &task, &deciding)
            .await;
        self.finish_armed(&task_id, &deciding, verdict);
        step
    }

    /// Brings member `index` up to date as DLV-203 does, checks the predicted tree, records the
    /// train's verdict there and merges it once GitHub shows that head.
    async fn train_member(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        train: &Train,
        index: usize,
        task: &TaskRecord,
        deciding: &Deciding,
    ) -> (Verdict, Step) {
        let member = &train.members[index];
        let armed = deciding.merge.clone();
        let keep = |armed: ArmedMerge, reason: String| (Verdict::Keep(armed), Step::Stop(reason));
        if armed.armed_at != member.armed_at {
            return keep(armed, format!("#{} was armed again", member.pr));
        }
        let (Some(worktree), Some(branch)) =
            (task.worktree_path.as_deref(), task.branch.as_deref())
        else {
            return keep(armed, format!("#{} has no worktree", member.pr));
        };
        let worktree = Path::new(worktree);
        let idle = task.status == TaskStatus::Running
            && task.activity.is_some_and(Activity::is_idle)
            && self.background_shells_of(&task.id).unwrap_or(0) == 0;
        if !idle || self.deps.git.has_changes(worktree).await.unwrap_or(true) {
            return keep(armed, format!("the agent of #{} is busy", member.pr));
        }
        let at = self
            .deps
            .git
            .log(worktree, "HEAD", 1)
            .await
            .ok()
            .and_then(|log| log.into_iter().next())
            .map(|commit| commit.sha);
        if at.as_deref() != Some(armed.head.as_str()) {
            return keep(armed, format!("#{} moved on", member.pr));
        }
        let profile = delivery_profile(sandbox);
        let integration = profile.integration_branch.clone();
        let Ok(token) = self.task_github_token(task, sandbox).await else {
            return keep(armed, format!("#{} has no GitHub token", member.pr));
        };
        let (head, moved) = match self
            .deps
            .git
            .merge_integration(worktree, &integration, &token)
            .await
        {
            Ok(MergeIntegration::UpToDate) => (armed.head.clone(), false),
            Ok(MergeIntegration::Merged(head)) => (head, true),
            // Aborted: the one-by-one flow tells the agent what to resolve.
            Ok(MergeIntegration::Conflict(_)) => {
                return keep(armed, format!("#{} conflicts after all", member.pr));
            }
            Err(err) => {
                return keep(
                    armed,
                    format!(
                        "#{} could not be brought up to date: {}",
                        member.pr,
                        err.public_message()
                    ),
                );
            }
        };
        let tree = self.deps.git.tree_of(worktree, &head).await.ok().flatten();
        let moved_armed = ArmedMerge {
            head: head.clone(),
            ..armed.clone()
        };
        // A merge made here is pushed as DLV-203 would push it; the armed merge follows the
        // pushed head, or stays where it was when the push failed.
        let push = |reason: String| async {
            if !moved {
                return keep(armed.clone(), reason);
            }
            match self.deps.git.push(worktree, branch, branch, &token).await {
                Ok(()) => {
                    self.save_armed_during(&task.id, deciding, &moved_armed);
                    self.train_branch_updated(sandbox, task, branch, &head, &integration)
                        .await;
                    keep(moved_armed.clone(), reason)
                }
                Err(err) => {
                    self.set_push_problem(&task.id, &head, branch, None, &err);
                    keep(armed.clone(), reason)
                }
            }
        };
        if tree.as_deref() != Some(member.tree.as_str()) {
            // Not what was tested: its own run decides it.
            return push(format!(
                "#{} does not have the tree the train tested",
                member.pr
            ))
            .await;
        }
        // The train's verdict at this head, recorded before the head is pushed so the CI loop
        // never queues a run for it.
        let recorded = match self.train_verdict(sandbox, train, index, &head).await {
            Ok(recorded) => recorded,
            Err(Step::Wait) if !moved => return (Verdict::Keep(armed), Step::Wait),
            Err(Step::Stop(reason)) => return push(reason).await,
            Err(_) => return push(format!("#{}'s verdict could not be read", member.pr)).await,
        };
        if moved {
            if let Err(err) = self.deps.git.push(worktree, branch, branch, &token).await {
                self.set_push_problem(&task.id, &head, branch, None, &err);
                return keep(armed, format!("#{} could not be pushed", member.pr));
            }
            // A restart from here on finds the pushed head armed (TRN-008).
            self.save_armed_during(&task.id, deciding, &moved_armed);
            self.train_branch_updated(sandbox, task, branch, &head, &integration)
                .await;
        }
        if let Some(run) = recorded {
            self.ci_publish(&run).await;
        }
        if !self.github_shows(sandbox, member.pr, &head).await {
            return (Verdict::Keep(moved_armed), Step::Wait);
        }
        let Ok(Some(user)) = self.deps.store.user(&task.owner).await else {
            return keep(
                moved_armed,
                format!("the owner of #{} cannot be read", member.pr),
            );
        };
        if user.disabled_at.is_some() {
            return keep(
                moved_armed,
                format!("the owner of #{} can no longer act", member.pr),
            );
        }
        let auth = AuthContext {
            user,
            csrf_token: None,
            session_hash: None,
        };
        match self
            .merge_task_pr_for_task(&auth, &task.id.to_string())
            .await
        {
            Ok(_) => {
                let others: Vec<u64> = train
                    .members
                    .iter()
                    .map(|m| m.pr)
                    .filter(|n| *n != member.pr)
                    .collect();
                let detail = format!(
                    "merged at head {head} reviewed independently ({}); armed while the checks \
                     ran; in merge train {} tested at {} (run {})",
                    if armed.summary.is_empty() {
                        "no summary"
                    } else {
                        armed.summary.as_str()
                    },
                    listed(&numbers(&train.members)),
                    train.commit,
                    train.run
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
                    tracing::warn!(task = %task.id, error = %err, "audit of a train merge failed");
                }
                let text = format!(
                    "Yulius merged pull request #{} into `{integration}` at {} in a merge train \
                     with {}: Yulius CI passed on their combined result. Finish what the delivery \
                     profile still asks for this stage (`delivery_status`; deploy and verify an \
                     integration environment with `delivery_deploy`), then end with your \
                     summary. Do not promote unless the task asks for it.",
                    member.pr,
                    short(&head),
                    listed(&others)
                );
                (Verdict::Decided(text), Step::Merged)
            }
            Err(err) => keep(
                moved_armed,
                format!(
                    "merging #{} was refused: {}",
                    member.pr,
                    err.public_message()
                ),
            ),
        }
    }

    async fn train_branch_updated(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        branch: &str,
        head: &str,
        integration: &str,
    ) {
        let _ = self
            .deps
            .store
            .audit(
                self.now(),
                "yulius",
                "delivery.branch_updated",
                Some(&format!("{}#{}", sandbox.slug, task.number)),
                Some(&format!(
                    "merged {integration} into {branch} at {head} for the merge train"
                )),
            )
            .await;
    }

    /// Makes sure `head` (member `index`'s, with the tree the train predicted) has the train's
    /// verdict: `Ok(Some(run))` for a verdict recorded now (to publish once pushed), `Ok(None)`
    /// when a passed run is already there; `Err(Wait)` while its own run still runs, and
    /// `Err(Stop)` when that run did not pass or nothing could be recorded.
    async fn train_verdict(
        &self,
        sandbox: &SandboxRecord,
        train: &Train,
        index: usize,
        head: &str,
    ) -> Result<Option<String>, Step> {
        let member = &train.members[index];
        let store = &self.deps.store;
        let runs = store
            .ci_runs_for_sha(&sandbox.id, head)
            .await
            .map_err(|_| Step::Wait)?;
        if let Some(latest) = runs.first() {
            return match latest.state {
                RunState::Passed => Ok(None),
                RunState::Queued | RunState::Running => Err(Step::Wait),
                other => Err(Step::Stop(format!(
                    "the own run of #{} at {} ended {}",
                    member.pr,
                    short(head),
                    other.as_str()
                ))),
            };
        }
        let Ok(Some(tested)) = store.ci_run(&train.run).await else {
            return Err(Step::Stop("its CI run is gone".to_owned()));
        };
        let jobs = store
            .ci_jobs(&train.run, false)
            .await
            .map_err(|_| Step::Wait)?;
        let last = index + 1 == train.members.len();
        let now = self.now();
        let note = format!(
            "not run again: merge train {} passed at {} (run {}); this is step {} of {}",
            listed(&numbers(&train.members)),
            short(&train.commit),
            train.run,
            index + 1,
            train.members.len()
        );
        let run = CiRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            reference: CiRef::PullRequest(member.pr),
            sha: head.to_owned(),
            source: tested.source,
            state: RunState::Passed,
            reason: Some(note.clone()),
            // The last step has exactly the tested tree; an earlier one is only covered by it.
            requested_by: if last { "yulius" } else { TRAIN_VERDICT_BY }.to_owned(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: now,
            started_at: Some(now),
            ended_at: Some(now),
        };
        let copies: Vec<CiJobRecord> = jobs
            .iter()
            .map(|job| CiJobRecord {
                id: self.deps.crypto.new_id(),
                run_id: run.id.clone(),
                ordinal: job.ordinal,
                name: job.name.clone(),
                key: job.key.clone(),
                command: job.command.clone(),
                matrix: job.matrix.clone(),
                state: RunState::Passed,
                reason: Some(note.clone()),
                exit_code: Some(0),
                output: format!("{note}.\n"),
                started_at: Some(now),
                ended_at: Some(now),
            })
            .collect();
        store
            .insert_ci_run(&run, &copies)
            .await
            .map_err(|err| Step::Stop(format!("its verdict could not be recorded: {err}")))?;
        Ok(Some(run.id))
    }

    /// Whether GitHub shows pull request `pr` at `head` and not behind its base: a merge right
    /// after a push is otherwise refused as a stale head (bounded: [`HEAD_POLLS`] reads).
    async fn github_shows(&self, sandbox: &SandboxRecord, pr: u64, head: &str) -> bool {
        let Ok(token) = self.dlv_installation_token(sandbox).await else {
            return false;
        };
        for attempt in 0..HEAD_POLLS {
            if attempt > 0 {
                tokio::time::sleep(HEAD_POLL_GAP).await;
            }
            if let Ok(pull) = self
                .deps
                .github
                .get_pull(&token, &sandbox.repo_owner, &sandbox.repo_name, pr)
                .await
                && pull.head_sha == head
                && !pull.behind
            {
                return true;
            }
        }
        false
    }

    /// Forms a train from the armed pull requests waiting now, if two or more can go together.
    async fn form_train(
        &self,
        sandbox: &SandboxRecord,
        state: &mut SandboxTrains,
    ) -> Option<Train> {
        let profile = delivery_profile(sandbox);
        // A promotion after each merge would ship states between the members (never tested).
        if profile.auto_promote {
            return None;
        }
        let landing = match profile.merge {
            MergeMethod::Squash => TrainLanding::Squash,
            MergeMethod::Merge => TrainLanding::MergeCommit,
            MergeMethod::Rebase => return None,
        };
        let gate = self.ci_gate(sandbox).await.gate?;
        let repo = self.sandbox_repo(&sandbox.id);
        let integration = format!("origin/{}", profile.integration_branch);
        let base = self
            .deps
            .git
            .resolve_commit(&repo, &integration)
            .await
            .ok()
            .flatten()?;
        self.cancel_stray_train_runs(sandbox).await;
        let tasks = self.deps.store.tasks(&sandbox.id, false).await.ok()?;
        let mut candidates = Vec::new();
        let mut still_armed = Vec::new();
        for task in tasks {
            let ArmedNow::Armed(armed) = self.armed_now(&task.id) else {
                continue;
            };
            still_armed.push((task.id.to_string(), armed.armed_at));
            let Some(pr) = self
                .deps
                .store
                .delivery(&task.id)
                .await
                .ok()
                .flatten()
                .filter(|record| record.stage == DeliveryStage::PrOpen)
                .and_then(|record| record.pr_number)
            else {
                continue;
            };
            let checks = self.ci_gate_result(sandbox, &armed.head).await;
            let up_to_date = matches!(
                self.deps
                    .git
                    .is_ancestor(&repo, &integration, &armed.head)
                    .await,
                Ok(true)
            );
            // One that can merge right now goes first, on its own: a train waits for it.
            if up_to_date && checks == GateResult::Run(Some(RunState::Passed)) {
                return None;
            }
            if matches!(
                checks,
                GateResult::Run(Some(
                    RunState::Failed | RunState::Error | RunState::Cancelled
                ))
            ) || state
                .held_back
                .contains(&(task.id.to_string(), armed.armed_at))
                || !self.train_ready(&task, &armed).await
            {
                continue;
            }
            candidates.push(Candidate { task, armed, pr });
        }
        state.held_back.retain(|entry| still_armed.contains(entry));
        let candidates = in_arm_order(candidates, |c| (c.armed.armed_at, c.task.number));
        if candidates.len() < 2 {
            return None;
        }
        let heads: Vec<String> = candidates.iter().map(|c| c.armed.head.clone()).collect();
        let key = format!("{base}:{}", heads.join(","));
        if state.unformed.as_deref() == Some(key.as_str()) {
            return None;
        }
        let ci_dir = self.ci_dir(&sandbox.id);
        let built = match self
            .deps
            .git
            .build_train(
                &repo,
                &ci_dir.join("train"),
                &ci_dir,
                &base,
                &heads,
                landing,
            )
            .await
        {
            Ok(built) => built,
            Err(err) => {
                tracing::warn!(sandbox = %sandbox.slug, error = %err, "a merge train could not be built");
                state.unformed = Some(key);
                return None;
            }
        };
        let mut members = Vec::new();
        let mut left_out = Vec::new();
        for (candidate, step) in candidates.iter().zip(built.steps) {
            match step {
                TrainStep::Clean { tree } => members.push(Member {
                    task: candidate.task.id.to_string(),
                    pr: candidate.pr,
                    head: candidate.armed.head.clone(),
                    armed_at: candidate.armed.armed_at,
                    tree,
                }),
                TrainStep::Conflict(files) => left_out.push(LeftOut {
                    task: candidate.task.id.to_string(),
                    pr: candidate.pr,
                    files,
                }),
            }
        }
        if members.len() < 2 {
            state.unformed = Some(key);
            return None;
        }
        // Its own CI when the members changed it, as for a pull request (CI-110).
        let own = match self.ci_jobs_at(&repo, &built.head).await {
            Ok(jobs) if !jobs.is_empty() => Some(jobs),
            _ => None,
        };
        let at_base = self.ci_jobs_at(&repo, &built.base).await.ok();
        let gate = match own {
            Some(jobs) if crate::ci::own_jobs_apply(&jobs, at_base.as_deref(), &gate.jobs) => {
                Gate {
                    jobs,
                    source: GateSource::PullRequest,
                }
            }
            _ => gate,
        };
        let prs = numbers(&members);
        let run = match self
            .ci_queue_placed(
                sandbox,
                CiRef::Train(prs.clone()),
                &built.head,
                &gate,
                "yulius",
                self.now(),
            )
            .await
        {
            Ok(run) => run,
            Err(err) => {
                tracing::warn!(sandbox = %sandbox.slug, error = %err, "a merge train's run could not be queued");
                return None;
            }
        };
        state.unformed = None;
        let left: Vec<u64> = left_out.iter().map(|l| l.pr).collect();
        self.train_audit(
            sandbox,
            "delivery.train.formed",
            &format!(
                "merge train {} on {} at {}: run {} tests {}{}",
                listed(&prs),
                profile.integration_branch,
                built.base,
                run.id,
                built.head,
                if left.is_empty() {
                    String::new()
                } else {
                    format!("; left out (conflicts): {}", listed(&left))
                }
            ),
        )
        .await;
        tracing::info!(sandbox = %sandbox.slug, run = %run.id, members = prs.len(), "merge train formed");
        Some(Train {
            run: run.id,
            commit: built.head,
            base: built.base,
            base_tree: built.base_tree,
            members,
            left_out,
            merging: false,
            waiting_since: None,
        })
    }

    /// Whether `task` could be brought up to date now, as DLV-203 requires: running and idle
    /// with nothing running in the background, nothing uncommitted, at the armed head.
    async fn train_ready(&self, task: &TaskRecord, armed: &ArmedMerge) -> bool {
        let Some(worktree) = task.worktree_path.as_deref().map(Path::new) else {
            return false;
        };
        let idle = task.status == TaskStatus::Running
            && task.activity.is_some_and(Activity::is_idle)
            && self.background_shells_of(&task.id).unwrap_or(0) == 0;
        if !idle || task.branch.is_none() {
            return false;
        }
        if self.deps.git.has_changes(worktree).await.unwrap_or(true) {
            return false;
        }
        matches!(
            self.deps.git.log(worktree, "HEAD", 1).await,
            Ok(log) if log.first().is_some_and(|c| c.sha == armed.head)
        )
    }

    /// Train runs left going without a train (a lost or rewritten file) are cancelled.
    async fn cancel_stray_train_runs(&self, sandbox: &SandboxRecord) {
        let Ok(runs) = self.deps.store.ci_runs(&sandbox.id, 50).await else {
            return;
        };
        for run in runs
            .iter()
            .filter(|r| r.state.is_active() && matches!(r.reference, CiRef::Train(_)))
        {
            let _ = self
                .ci_cancel(&run.id, "no merge train holds this run any more")
                .await;
        }
    }
}

/// Every sandbox's trains; none when the file is missing or unreadable.
fn read_all(path: &Path) -> BTreeMap<String, SandboxTrains> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(task: &str, pr: u64, tree: &str) -> Member {
        Member {
            task: task.to_owned(),
            pr,
            head: format!("head-{task}"),
            armed_at: 1,
            tree: tree.to_owned(),
        }
    }

    fn train() -> Train {
        Train {
            run: "run1".to_owned(),
            commit: "c0ffee".to_owned(),
            base: "base".to_owned(),
            base_tree: "t0".to_owned(),
            members: vec![
                member("a", 11, "t1"),
                member("b", 12, "t2"),
                member("c", 13, "t3"),
            ],
            left_out: vec![LeftOut {
                task: "d".to_owned(),
                pr: 14,
                files: vec!["shared.txt".to_owned()],
            }],
            merging: false,
            waiting_since: None,
        }
    }

    /// TRN-004: under test only the first member is free; once it passed the train merges all.
    #[test]
    fn a_train_holds_the_members_after_the_first_while_it_runs() {
        let mut train = train();
        assert_eq!(hold_of(None, "a"), Hold::Free);
        assert_eq!(hold_of(Some(&train), "a"), Hold::Free);
        assert_eq!(hold_of(Some(&train), "b"), Hold::Testing);
        assert_eq!(
            hold_of(Some(&train), "d"),
            Hold::Held,
            "left out: after the train"
        );
        assert_eq!(hold_of(Some(&train), "x"), Hold::Free);
        train.merging = true;
        assert_eq!(hold_of(Some(&train), "a"), Hold::Held);
        assert_eq!(hold_of(Some(&train), "c"), Hold::Held);
        assert_eq!(hold_of(Some(&train), "d"), Hold::Held);
        assert_eq!(hold_of(Some(&train), "x"), Hold::Free);
    }

    /// TRN-005: each step expects the tree the members before it leave.
    #[test]
    fn each_step_expects_the_tree_of_the_ones_before_it() {
        let train = train();
        assert_eq!(tree_after(&train, 0), "t0");
        assert_eq!(tree_after(&train, 1), "t1");
        assert_eq!(tree_after(&train, 3), "t3");
    }

    /// TRN-007: a member hears its place; one left out hears why.
    #[test]
    fn the_wait_says_where_a_pull_request_stands() {
        let mut train = train();
        let note = note_of(&train, "b").unwrap_or_default();
        assert!(
            note.contains("#12 is number 2 of 3 in merge train #11, #12, #13"),
            "{note}"
        );
        assert!(note.contains("run run1"), "{note}");
        let left = note_of(&train, "d").unwrap_or_default();
        assert!(
            left.contains("#14 was left out") && left.contains("`shared.txt`"),
            "{left}"
        );
        assert_eq!(note_of(&train, "x"), None);
        train.merging = true;
        let merging = note_of(&train, "a").unwrap_or_default();
        assert!(merging.contains("passed; Yulius merges them"), "{merging}");
    }

    /// TRN-001: armed merges go in arm order, then by task number, at most MAX_MEMBERS.
    #[test]
    fn candidates_go_in_arm_order() {
        // (task number, armed at)
        let mut many: Vec<(u32, i64)> = (0..12).map(|n| (100 - n, 50)).collect();
        many.push((1, 70));
        many.push((2, 10));
        let ordered = in_arm_order(many, |&(number, at)| (at, number));
        let numbers: Vec<u32> = ordered.iter().map(|&(number, _)| number).collect();
        assert_eq!(numbers.len(), MAX_MEMBERS);
        assert_eq!(numbers[0], 2, "armed first");
        assert_eq!(
            &numbers[1..3],
            &[89, 90],
            "armed at the same moment: by number"
        );
        assert!(!numbers.contains(&1), "armed last, beyond the limit");
    }

    #[test]
    fn the_file_round_trips_and_forgets_empty_sandboxes() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = SandboxTrains {
            train: Some(train()),
            held_back: vec![("a".to_owned(), 1)],
            unformed: None,
        };
        let text = serde_json::to_string(&state)?;
        assert_eq!(serde_json::from_str::<SandboxTrains>(&text)?, state);
        assert!(SandboxTrains::default().is_empty());
        let old: SandboxTrains = serde_json::from_str("{}")?;
        assert!(old.is_empty());
        Ok(())
    }
}
