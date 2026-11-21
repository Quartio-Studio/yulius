//! Merge trains (SPEC-084, ADR-0067), the merge candidates of the merge queue (SPEC-441,
//! ADR-0285): armed pull requests whose base moved since are tested together on top of the
//! integration branch and merged one after another on that one verdict (SPEC-493 MQC-001: the
//! candidate's run goes beside the members' own runs; SPEC-497 MQW-002: a passed candidate
//! merges even while a member's own run still goes, since it tested the exact merged result).
//!
//! Under a strict "up to date" rule a pull request merges only with its base in its head. Moving
//! the head of one whose own run still goes restarts that run (F 2026-11-07: with 45 to 60 minute
//! runs, most runs were thrown away), so Yulius never merges the base into a pull request before
//! its own run (its pre-check) passed. It joins a train while that run is still queued or running
//! or once it passed, and only when it is behind its base: a green one that is up to date merges
//! alone, at once. A member whose own run fails leaves (MQC-002); one whose own run errored or
//! was cancelled stays, decided by the candidate (SPEC-497 MQW-001). A candidate formed again on
//! the same base with the same members reuses the passed run of its tree. A candidate that
//! rebuilds a recently failed candidate tree takes the original failure (SPEC-542). A pull request that
//! changes derived files an earlier member also changes (`[merge_queue] apart`) waits for the
//! next candidate (MQW-003).
//!
//! A train (1 to [`MAX_MEMBERS`] members, in arm order) is built in Yulius's clone, exactly as the
//! merges will happen: the integration branch is merged into each armed head in arm order (the
//! repository's merge drivers apply), each result landing as the profile's merge method lands it,
//! so the train's last commit has the tree the integration branch has once every member merged. A
//! head that conflicts is left out. Yulius CI runs once on that commit (reference
//! `train/<numbers>`, never published on GitHub).
//!
//! Every member (and any left out) is held while the train lives: not brought up to date, not
//! merged on its own. When the run passes, the train merges them in order: it brings each up to
//! date as DLV-203 does, checks that the result has exactly the tree the train predicted, records
//! the train's verdict at that head (published as `yulius/ci`, so GitHub's own rules are met
//! without a new run), waits until GitHub shows the new head and merges it through the normal
//! merge path with all its gates. A member whose agent is busy at its turn is waited for, within
//! [`WAIT_LIMIT_MS`]. The integration branch must have the predicted tree before each step.
//!
//! A failed run is bisected: a train of two or more is formed again from its first half, and a
//! failed train of one ends that armed merge, saying why. Anything else unexpected dissolves the
//! train: an errored or cancelled run, or a member that cannot merge for a reason of its own,
//! sends those members one by one (DLV-203 after their own run passed); the integration branch
//! moving another way, a member re-armed or moved on, or GitHub not catching up within
//! [`WAIT_LIMIT_MS`] lets the train form again.
//!
//! State: `<data>/merge-trains.json` (one train per sandbox, written whole and renamed into
//! place), the CI run in the store and the commits in the clone; so a restart picks the train up
//! where it was (armed merges come back through DLV-205).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_domain::ci::{CiRef, GateResult, RunState};
use yulius_domain::delivery::{DeliveryStage, MergeMethod};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::vault::redact::{PlainMatcher, redact_text};

use crate::agent_wait::{ArmedMerge, ArmedNow, Deciding, QUEUE_BASE_GUIDANCE, QueueNote, Verdict};
use crate::auth::AuthContext;
use crate::ci::{CiJobRecord, CiRunRecord, PullHead, run_for_pull_head};
use crate::context::App;
use crate::delivery::{MergeFailure, delivery_profile, past_merge};
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
/// How long a train's run may stay queued or running (waiting for disk, a lane, its jobs)
/// before the train lets its members go one by one.
pub const TEST_LIMIT_MS: i64 = 2 * 60 * 60 * 1000;

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
    /// Most members of the next train while a red one is bisected (SPEC-441 MQ-004): the first
    /// half of the last red train's; none once a train passed.
    #[serde(default)]
    limit: Option<usize>,
    /// The one member a red train's bisection left out (SPEC-497 MQW-003): if the others pass
    /// without it on the same base, it is the one that breaks the combination, and it is told at
    /// once instead of failing a candidate of its own.
    #[serde(default)]
    suspect: Option<Suspect>,
}

/// A member a red train's bisection left out on its own (SPEC-497 MQW-003).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Suspect {
    task: String,
    pr: u64,
    head: String,
    armed_at: i64,
    /// The base the red train was built on.
    base: String,
    /// The members tested without it, in order, with the heads they were tested at.
    others: Vec<(u64, String)>,
    /// The red train's run.
    run: String,
}

impl Suspect {
    /// Whether `passed`, a train that passed, shows that this member breaks the combination: it
    /// is exactly the others, on the same base as the red train.
    fn proven_by(&self, passed: &Train) -> bool {
        self.base == passed.base
            && self.others
                == passed
                    .members
                    .iter()
                    .map(|m| (m.pr, m.head.clone()))
                    .collect::<Vec<_>>()
    }

    fn other_numbers(&self) -> Vec<u64> {
        self.others.iter().map(|(pr, _)| *pr).collect()
    }
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
    /// Since when a train waits for GitHub, a busy member or a run at a pushed head.
    #[serde(default)]
    waiting_since: Option<i64>,
    /// When it was formed (ms; 0 when unknown).
    #[serde(default)]
    formed_at: i64,
    /// The member whose own run its run ended waiting for, and since when (SPEC-493 MQC-001):
    /// the 20 s pass has nothing to decide meanwhile, and `yulius wait` says so.
    #[serde(default)]
    own_wait: Option<(u64, i64)>,
}

impl Train {
    /// Keep a red candidate while the member's own run is active, within the original deadline.
    fn wait_for_own(&mut self, pr: u64, now: i64) -> bool {
        let since = self.own_wait.map_or(now, |(_, since)| since);
        if now.saturating_sub(since) > WAIT_LIMIT_MS {
            return false;
        }
        self.own_wait = Some((pr, since));
        true
    }
}

/// A pull request in a train.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Member {
    task: String,
    /// Pull request number.
    pr: u64,
    /// The armed head it was built with.
    head: String,
    /// The head the passed train verified and pushed while waiting for GitHub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verified_head: Option<String>,
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
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    armed_at: Option<i64>,
    files: Vec<String>,
}

/// How a merge train holds an armed pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    /// Not in a train: it merges when green and up to date, or waits for the next train.
    Free,
    /// In a train (SPEC-441 MQ-003): the train brings it up to date and merges it. Or left out
    /// of one: the ones armed before it go first, and it waits until the train is over.
    Held,
}

/// The queue instructions and, for an included member, the run that holds its activation.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WatchedTrain {
    pub note: QueueNote,
    pub candidate: Option<(String, Vec<u64>)>,
}

/// The saved train and arm do not yet provide a verified queue position.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UnverifiedTrain;

/// What one step of a passed train did.
enum Step {
    /// The member merged; go on with the next.
    Merged,
    /// Not yet (GitHub has not shown the head, or a run at the head pushed still runs): next
    /// pass.
    Wait,
    /// GitHub has yet to accept the head checked against this member’s tested tree.
    WaitAtHead(String),
    /// The train is dissolved, for this reason.
    Stop(String),
    /// The train is dissolved, for a reason of this member's own: it goes one by one, where it
    /// is told what stops it, instead of being tested again in the same train (SPEC-441 MQ-005).
    OneByOne(String),
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
    // Every member waits for the train: none can merge alone (it is behind its base), and
    // bringing one up to date would only move the head the train tested (SPEC-441 MQ-003).
    if train.left_out.iter().any(|left| left.task == task)
        || train.members.iter().any(|m| m.task == task)
    {
        Hold::Held
    } else {
        Hold::Free
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

/// An accepted queue decision and the redacted instructions its original owner received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SingletonDecision {
    pub head: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_at: Option<i64>,
    /// The ordinary run still active when the queue exhausted its original wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired_ordinary_run: Option<String>,
    pub singleton: TestedSingleton,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TestedSingleton {
    pub pr: u64,
    pub base: String,
    pub tree: String,
    pub run: String,
}

impl SingletonDecision {
    /// An ordinary dropped arm recorded only its head. A malformed decision
    /// cannot establish whether a saved activation was already consumed.
    pub(crate) fn from_span(
        span: &crate::stages::SpanRecord,
    ) -> crate::error::AppResult<Option<Self>> {
        use crate::error::AppError;
        if span.outcome.as_deref() != Some("dropped") {
            return Ok(None);
        }
        let unreadable = || AppError::internal("unreadable merge wait decision");
        let fields: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(span.detail.as_deref().ok_or_else(unreadable)?)
                .map_err(|_| unreadable())?;
        if fields.len() == 1
            && fields
                .get("head")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|head| !head.is_empty())
        {
            return Ok(None);
        }
        let decision: Self =
            serde_json::from_value(serde_json::Value::Object(fields)).map_err(|_| unreadable())?;
        if [
            &decision.head,
            &decision.message,
            &decision.singleton.base,
            &decision.singleton.tree,
            &decision.singleton.run,
        ]
        .iter()
        .any(|value| value.is_empty())
        {
            return Err(unreadable());
        }
        Ok(Some(decision))
    }
}

/// Result of observing a singleton's hosted inputs and ordinary CI at its exact head.
enum SingletonObservation {
    Changed,
    Current(Option<(RunState, String)>),
}

/// Fold redacted CI text to a bounded line that can be typed into the owner's terminal.
fn diagnostic_excerpt(text: &str, limit: usize, matcher: &PlainMatcher) -> String {
    let normalized: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    // Formatting can join fragments into a secret. Check the complete line before clipping it.
    redact_text(matcher, &normalized)
        .0
        .chars()
        .take(limit)
        .collect()
}

/// The instructions an owner gets when their head failed on top of the integration base.
fn singleton_failure(pr: u64, head: &str, integration: &str, run: &str) -> String {
    format!(
        "The merge you armed was dropped: pull request #{pr} was tested at {}. When \
         merged into the current `{integration}` it fails (merge candidate run {run}). Read \
         why with `ci_status`, merge `origin/{integration}` into your branch, fix it \
         test-first, commit, then {}.",
        short(head),
        yulius_domain::review::KEEP_APPROVAL
    )
}

/// What `yulius wait` says about `task`'s place in `train`.
fn note_of(train: &Train, task: &str, arm: Option<(&str, i64)>) -> Option<QueueNote> {
    let prs = numbers(&train.members);
    if let Some(index) = train.members.iter().position(|m| m.task == task) {
        let member = &train.members[index];
        let head = member.verified_head.as_deref().unwrap_or(&member.head);
        if arm != Some((head, member.armed_at)) {
            return Some(QueueNote::current(
                "The queue verifies its current head before giving this request a train position."
                    .into(),
            ));
        }
        let place = format!(
            "pull request #{} is number {} of {} in merge train {}",
            train.members[index].pr,
            index + 1,
            prs.len(),
            listed(&prs)
        );
        let text = if let Some((pr, _)) = train.own_wait {
            format!(
                "{place}: their combined result was tested (run {}); Yulius waits for pull \
                 request #{pr}'s own Yulius CI run before it merges or tests them further.",
                train.run
            )
        } else if train.merging {
            format!("{place}: their combined result passed; Yulius merges them in this order.")
        } else {
            format!(
                "{place}: Yulius CI tests their combined result (run {}) and Yulius merges them \
                 in this order when it passes.",
                train.run
            )
        };
        return Some(QueueNote {
            text: format!("{text} {QUEUE_BASE_GUIDANCE}"),
            awaits_own_run: train
                .own_wait
                .is_some_and(|(pr, _)| pr == train.members[index].pr),
        });
    }
    train.left_out.iter().find(|l| l.task == task).map(|left| {
        if arm.is_none() || left.head.as_deref().zip(left.armed_at) != arm {
            return QueueNote::current(format!(
                "pull request #{} waits behind merge train {} and is considered after it ends.",
                left.pr,
                listed(&prs)
            ));
        }
        let files: Vec<String> = left
            .files
            .iter()
            .take(8)
            .map(|f| format!("`{f}`"))
            .collect();
        QueueNote::current(format!(
            "pull request #{} was left out of merge train {}: merged on top of them it conflicts \
             in {}. It merges one by one after them; if it then conflicts with the integration \
             branch, Yulius says what to resolve.",
            left.pr,
            listed(&prs),
            files.join(", ")
        ))
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
        self.try_read_trains(sandbox).unwrap_or_default()
    }

    fn try_read_trains(&self, sandbox: &SandboxId) -> std::io::Result<SandboxTrains> {
        let _guard = TRAINS_FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(read_all(&self.trains_path())?
            .remove(sandbox.as_str())
            .unwrap_or_default())
    }

    /// Saves `state` for `sandbox` (written whole, then renamed into place); failures are
    /// logged, and the train is then only as current as the file.
    fn write_trains(&self, sandbox: &SandboxId, state: &SandboxTrains) {
        let _guard = TRAINS_FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.trains_path();
        let mut all = read_all(&path).unwrap_or_default();
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
        // No CI on this instance: no train advances, so none may hold anything.
        if !self.config.ci.enabled {
            return Hold::Free;
        }
        hold_of(
            self.read_trains(&task.sandbox_id).train.as_ref(),
            task.id.as_str(),
        )
    }

    /// Which run and member list belong to this PR's currently armed activation.
    /// A reusable CI run can have been admitted for a different set of members.
    pub(crate) fn watched_train(
        &self,
        task: &TaskRecord,
        pr: u64,
    ) -> Result<Option<WatchedTrain>, UnverifiedTrain> {
        if !self.config.ci.enabled {
            return Ok(None);
        }
        let crate::agent_wait::ArmedNow::Armed(arm) = self.armed_now(&task.id) else {
            return Ok(None);
        };
        let Some(train) = self
            .try_read_trains(&task.sandbox_id)
            .map_err(|_| UnverifiedTrain)?
            .train
        else {
            return Ok(None);
        };
        let Some(note) = note_of(&train, task.id.as_str(), Some((&arm.head, arm.armed_at))) else {
            return Ok(None);
        };
        let candidate =
            if let Some(member) = train.members.iter().find(|m| m.task == task.id.as_str()) {
                if member.pr != pr || member.armed_at != arm.armed_at {
                    return Ok(None);
                }
                // The queue saves the pushed arm before checkpointing the verified train head.
                // During that interval, including after restart, a free-slot check-in is stale.
                if member.verified_head.as_deref().unwrap_or(&member.head) != arm.head {
                    return Err(UnverifiedTrain);
                }
                Some((train.run, numbers(&train.members)))
            } else {
                // Left-out PRs wait behind the train without sharing its failure.
                if !train
                    .left_out
                    .iter()
                    .any(|left| left.task == task.id.as_str() && left.pr == pr)
                {
                    return Ok(None);
                }
                None
            };
        Ok(Some(WatchedTrain { note, candidate }))
    }

    /// Whether `task` is a member of its sandbox's live merge train, merged or not: the train
    /// counts on its recorded stage until it ends (SPEC-425, review finding 4).
    pub(crate) fn in_train(&self, task: &TaskRecord) -> bool {
        // No CI on this instance: no train advances or ends, so none may hold anything.
        if !self.config.ci.enabled {
            return false;
        }
        self.read_trains(&task.sandbox_id)
            .train
            .is_some_and(|train| train.members.iter().any(|m| m.task == task.id.as_str()))
    }

    /// What `yulius wait` says about `task`'s merge train, if it is in one or was left out.
    pub(crate) fn train_note(&self, task: &TaskRecord) -> Option<QueueNote> {
        let state = self.read_trains(&task.sandbox_id);
        let current = self.armed_now(&task.id);
        let arm = match &current {
            ArmedNow::Armed(arm) => Some((arm.head.as_str(), arm.armed_at)),
            _ => None,
        };
        note_of(state.train.as_ref()?, task.id.as_str(), arm)
    }

    /// `sandbox`'s merge train, for the person's merge queue (SPEC-487 MRG-002): its CI run, its
    /// members in merge order (task id, pull request), whether it passed and is merging, and the
    /// pull requests left out of it for conflicts.
    pub(crate) fn train_summary(&self, sandbox: &SandboxId) -> Option<serde_json::Value> {
        let train = self.read_trains(sandbox).train?;
        Some(serde_json::json!({
            "run": train.run,
            "members": train.members.iter().map(|member| serde_json::json!({
                "task": member.task, "pull_request": member.pr, "head": member.head,
            })).collect::<Vec<_>>(),
            "merging": train.merging,
            "left_out": train.left_out.iter().map(|left| serde_json::json!({
                "task": left.task, "pull_request": left.pr, "files": left.files,
            })).collect::<Vec<_>>(),
        }))
    }

    /// Whether `sandbox` has a merge train whose run is still queued or running.
    pub(crate) fn train_running(&self, sandbox: &SandboxRecord) -> bool {
        self.read_trains(&sandbox.id)
            .train
            .is_some_and(|train| !train.merging)
    }

    /// Whether `sandbox`'s merge train has a run that is no longer queued or running: the next
    /// pass merges its members or dissolves it.
    /// A red train waiting for a member's own run within its bound has nothing to decide: the
    /// end of that run starts a pass by itself.
    pub(crate) async fn train_run_ended(&self, sandbox: &SandboxRecord) -> bool {
        let Some(train) = self.read_trains(&sandbox.id).train else {
            return false;
        };
        if let Some((_, since)) = train.own_wait
            && self.now().0 <= since.saturating_add(WAIT_LIMIT_MS)
        {
            return false;
        }
        !matches!(
            self.deps.store.ci_run(&train.run).await,
            Ok(Some(run)) if run.state.is_active()
        )
    }

    /// The merge candidate `task`'s sandbox tests now, when `task` is not in it: "merge candidate
    /// #a, #b (run R)" (SPEC-441 MQ-007).
    pub(crate) fn train_running_note(&self, task: &TaskRecord) -> Option<String> {
        let train = self.read_trains(&task.sandbox_id).train?;
        (hold_of(Some(&train), task.id.as_str()) == Hold::Free).then(|| {
            format!(
                "merge candidate {} (run {})",
                listed(&numbers(&train.members)),
                train.run
            )
        })
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
        // A playground's merges go one by one (SPEC-163): it has no pull request to train.
        if sandbox.removed_at.is_some() || sandbox.is_playground() {
            return;
        }
        // Decided on the integration branch as it is now: a merge a moment ago (by this pass or
        // a person) leaves the others behind it (SPEC-441 MQ-003).
        if self.armed_in(sandbox_id).await
            && let Err(err) = self.dlv_refresh_clone(&sandbox).await
        {
            tracing::warn!(sandbox = %sandbox.slug, error = %err, "a merge pass could not fetch");
        }
        let mut state = self.read_trains(sandbox_id);
        // During recovery a saved arm can be known before its history is readable.
        // A fast CI completion must preserve that train and its original wait deadline.
        if state.train.as_ref().is_some_and(|train| {
            train.members.iter().any(|member| {
                TaskId::parse(&member.task).is_ok_and(|task| {
                    self.armed_now(&task) == ArmedNow::None && self.merge_is_armed(&task)
                })
            })
        }) {
            return;
        }
        let before = state.clone();
        state.train = match state.train.take() {
            Some(train) => self.advance_train(&sandbox, &mut state, train).await,
            None => self.form_train(&sandbox, &mut state).await,
        };
        if state != before {
            self.write_trains(sandbox_id, &state);
        }
    }

    /// Whether a task of `sandbox` has an armed merge (being decided or not).
    pub(crate) async fn armed_in(&self, sandbox: &SandboxId) -> bool {
        let Ok(tasks) = self.deps.store.tasks(sandbox, false).await else {
            return false;
        };
        tasks
            .iter()
            .any(|task| self.armed_now(&task.id) != ArmedNow::None)
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
        crate::race::reached(crate::race::Point::TrainDissolving, Path::new(&train.run));
        let _ = self
            .ci_cancel(
                &train.run,
                &format!("the merge train was dissolved: {reason}"),
            )
            .await;
        tracing::info!(sandbox = %sandbox.slug, run = %train.run, reason, "merge train dissolved");
        self.train_audit(
            sandbox,
            yulius_domain::merge_queue::DISSOLVED,
            &format!(
                "merge train {} (run {}): {reason}; its pull requests merge one by one (why: {})",
                listed(&numbers(&train.members)),
                train.run,
                dissolved_why(reason).as_str()
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
        // A member whose own run did not pass leaves, whatever the candidate's run says: the
        // others form a candidate again without it (SPEC-493 MQC-002).
        if let Some(reason) = self.member_left(sandbox, &train).await {
            self.dissolve(sandbox, &train, &reason).await;
            return None;
        }
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
                if train.formed_at > 0 && self.now().0 - train.formed_at > TEST_LIMIT_MS {
                    self.dissolve(sandbox, &train, "its CI run took too long")
                        .await;
                    return None;
                }
                if let Some(reason) = self.train_broken(sandbox, &train).await {
                    self.dissolve(sandbox, &train, &reason).await;
                    return None;
                }
                Some(train)
            }
            RunState::Passed => {
                state.limit = None;
                if let Some(suspect) = state.suspect.take()
                    && suspect.proven_by(&train)
                {
                    self.tell_suspect(sandbox, &suspect, &train).await;
                }
                self.drive_train(sandbox, state, train).await
            }
            RunState::Failed => {
                crate::race::reached(
                    crate::race::Point::FailedTrainDeciding,
                    Path::new(&train.run),
                );
                if train.members.len() == 1 {
                    return self.decide_failed_singleton(sandbox, state, train).await;
                }
                // A member's own run that still goes may say why: it is waited for (within the
                // test limit), and a member whose own run fails then leaves instead of the
                // candidate being halved (SPEC-493 MQC-002).
                let mut going = None;
                for index in 0..train.members.len() {
                    if going.is_none() && self.own_run_going(sandbox, &train, index).await {
                        going = Some(train.members[index].pr);
                    }
                }
                let mut train = train;
                if let Some(pr) = going
                    && train.wait_for_own(pr, self.now().0)
                {
                    return Some(train);
                }
                // A member may have failed since the check at the start of this pass:
                // its run then no longer counts as going. Read its verdict before splitting.
                if let Some(reason) = self.member_left(sandbox, &train).await {
                    self.dissolve(sandbox, &train, &reason).await;
                } else {
                    self.bisect(sandbox, state, &train).await;
                }
                None
            }
            other => {
                // Errored or cancelled, which says nothing about the members: they merge one by
                // one, each brought up to date after its own run passed, until armed again.
                state.limit = None;
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

    /// Why a member leaves `train` (SPEC-493 MQC-002, SPEC-497 MQW-001): its own run at the head
    /// it was armed with failed, its change's verdict. An errored or cancelled own run says
    /// nothing about the change (the infrastructure twice, a time limit, the head brought up to
    /// date): the candidate's run, which tests its exact merged result, still decides it. A
    /// merged member stays.
    async fn member_left(&self, sandbox: &SandboxRecord, train: &Train) -> Option<String> {
        for member in &train.members {
            let Ok(task) = TaskId::parse(&member.task) else {
                continue;
            };
            if matches!(
                self.deps.store.delivery(&task).await,
                Ok(Some(record)) if past_merge(&record.stage)
            ) {
                continue;
            }
            if let Some(reason) = self.own_run_failed(sandbox, member).await {
                return Some(reason);
            }
        }
        None
    }

    /// Why `member` leaves for its own run, if its own run failed: the run at its armed head now
    /// (one its agent's clean merge of the integration branch moved, armed at the same time, or
    /// one the train brought up to date and recorded its verdict at), else at the head it joined
    /// with (SPEC-497 MQW-001, review round 1). Asked at each pass's start and before each
    /// member's merge, so a failure that ends meanwhile is never merged on the candidate.
    async fn own_run_failed(&self, sandbox: &SandboxRecord, member: &Member) -> Option<String> {
        let head = TaskId::parse(&member.task)
            .ok()
            .and_then(|task| match self.armed_now(&task) {
                ArmedNow::Armed(armed) if armed.armed_at == member.armed_at => Some(armed.head),
                _ => None,
            })
            .unwrap_or_else(|| member.head.clone());
        matches!(
            self.ci_gate_for_pull(sandbox, member.pr, &head).await,
            GateResult::Run(Some(RunState::Failed))
        )
        .then(|| {
            format!(
                "#{}'s own run at {} ended failed: it leaves the candidate",
                member.pr,
                short(&head),
            )
        })
    }

    /// Whether member `index`'s own run at its armed head still goes: a red candidate waits for
    /// it before it is bisected, since it may say why (SPEC-493 MQC-002).
    async fn own_run_going(&self, sandbox: &SandboxRecord, train: &Train, index: usize) -> bool {
        matches!(
            self.ci_gate_for_pull(sandbox, train.members[index].pr, &train.members[index].head)
                .await,
            GateResult::Run(Some(RunState::Queued | RunState::Running))
        )
    }

    /// A failed candidate of multiple members tests its first half without the rest.
    async fn bisect(&self, sandbox: &SandboxRecord, state: &mut SandboxTrains, train: &Train) {
        state.suspect = None;
        let half = train.members.len().div_ceil(2);
        state.limit = Some(half);
        // One member left out: if the rest pass without it on this base, it is the one that
        // breaks the combination (SPEC-497 MQW-003).
        if let [last] = &train.members[half..] {
            state.suspect = Some(Suspect {
                task: last.task.clone(),
                pr: last.pr,
                head: last.head.clone(),
                armed_at: last.armed_at,
                base: train.base.clone(),
                others: train.members[..half]
                    .iter()
                    .map(|m| (m.pr, m.head.clone()))
                    .collect(),
                run: train.run.clone(),
            });
        }
        self.dissolve(
            sandbox,
            train,
            &format!("its CI run failed; the first {half} are tested again without the rest"),
        )
        .await;
    }

    /// A red singleton is attributed to the current PR only after its hosted inputs
    /// and its own CI run have been read. A new own run keeps the original wait budget.
    async fn decide_failed_singleton(
        &self,
        sandbox: &SandboxRecord,
        state: &mut SandboxTrains,
        mut train: Train,
    ) -> Option<Train> {
        let member = train.members.first()?;
        let task = TaskId::parse(&member.task).ok()?;
        let integration = delivery_profile(sandbox).integration_branch;
        let text = tokio::time::timeout(
            Duration::from_secs(5),
            self.singleton_failure_text(sandbox, member, &integration, &train.run),
        )
        .await
        .ok()
        .flatten();
        let pr = member.pr;
        let unavailable_since = train.waiting_since;
        let mut expired_ordinary_run = None;
        let text = match self
            .singleton_observation(sandbox, &task, member, &integration, &train.base)
            .await
        {
            Some(SingletonObservation::Changed) => {
                self.dissolve(sandbox, &train, "its tested head or base changed")
                    .await;
                return None;
            }
            Some(SingletonObservation::Current(Some((RunState::Failed, _)))) => {
                state.limit = None;
                state.suspect = None;
                self.dissolve(
                    sandbox,
                    &train,
                    &format!(
                        "#{pr}'s own run at {} ended failed: it leaves the candidate",
                        short(&member.head)
                    ),
                )
                .await;
                return None;
            }
            Some(SingletonObservation::Current(ordinary)) => {
                if text.is_some() {
                    train.waiting_since = None;
                }
                if let Some((RunState::Queued | RunState::Running, id)) = ordinary {
                    if train.wait_for_own(pr, self.now().0) && text.is_some() {
                        return Some(train);
                    }
                    expired_ordinary_run = Some(id);
                }
                text
            }
            None => None,
        };
        if let Some(text) = text {
            state.limit = None;
            state.suspect = None;
            let member = train.members.first()?;
            let decision = SingletonDecision {
                head: member.head.clone(),
                message: text,
                superseded_at: None,
                expired_ordinary_run,
                singleton: TestedSingleton {
                    pr,
                    base: train.base.clone(),
                    tree: member.tree.clone(),
                    run: train.run.clone(),
                },
            };
            if let Some(deciding) = self.take_armed(&task) {
                if deciding.merge.armed_at != member.armed_at || deciding.merge.head != member.head
                {
                    self.finish_armed(&task, &deciding, Verdict::Keep(deciding.merge.clone()))
                        .await;
                } else if !self
                    .finish_singleton(&task, &sandbox.id, &deciding, &decision)
                    .await
                {
                    train.waiting_since = unavailable_since;
                    return self
                        .wait_for_singleton_evidence(sandbox, state, train)
                        .await;
                }
            }
        } else {
            return self
                .wait_for_singleton_evidence(sandbox, state, train)
                .await;
        }
        self.dissolve(sandbox, &train, "its CI run failed").await;
        None
    }

    async fn wait_for_singleton_evidence(
        &self,
        sandbox: &SandboxRecord,
        state: &mut SandboxTrains,
        mut train: Train,
    ) -> Option<Train> {
        let now = self.now().0;
        let since = *train.waiting_since.get_or_insert(now);
        if now.saturating_sub(since) <= WAIT_LIMIT_MS {
            return Some(train);
        }
        let member = train.members.first()?;
        state.held_back.push((member.task.clone(), member.armed_at));
        self.dissolve(
            sandbox,
            &train,
            "its tested inputs, failure or decision record stayed unavailable",
        )
        .await;
        None
    }

    /// Attach a bounded, redacted diagnostic from the actual source run to the repair instructions.
    async fn singleton_failure_text(
        &self,
        sandbox: &SandboxRecord,
        member: &Member,
        integration: &str,
        run: &str,
    ) -> Option<String> {
        let mut text = singleton_failure(member.pr, &member.head, integration, run);
        let jobs = self.deps.store.ci_jobs(run, true).await.ok()?;
        let mut failed = jobs.iter().filter(|job| job.state == RunState::Failed);
        let has_text = |s: &str| !s.trim().is_empty();
        let job = failed
            .clone()
            .find(|job| job.failures.as_deref().is_some_and(has_text))
            .or_else(|| {
                failed
                    .clone()
                    .find(|job| job.reason.as_deref().is_some_and(has_text))
            })
            .or_else(|| failed.next())?;
        let failures = job.failures.as_deref().filter(|s| has_text(s));
        let label = if failures.is_none() && job.exit_code.is_none() {
            format!("`{}` failed", job.name)
        } else {
            crate::pr_watch::failed_jobs(std::slice::from_ref(job))
        };
        let matcher = self.agent_secret_matcher(&sandbox.id).await.ok()?;
        let retained = redact_text(
            &matcher,
            failures.or(job.reason.as_deref()).unwrap_or_default(),
        )
        .0;
        // The scanner's first line names the test already in the label. A parser's
        // structured refusal has no such header, so keep its first line.
        let body = if failures.is_some() {
            retained
                .split_once('\n')
                .map_or(retained.as_str(), |(_, body)| body)
        } else {
            retained.as_str()
        };
        let label = redact_text(&matcher, &label).0;
        text.push(' ');
        text.push_str(&diagnostic_excerpt(&label, 128, &matcher));
        let excerpt = diagnostic_excerpt(body, 383, &matcher);
        if !excerpt.is_empty() {
            text.push(' ');
            text.push_str(&excerpt);
        }
        Some(redact_text(&matcher, &text).0)
    }

    /// Before dropping an arm, confirm the PR still has the inputs the singleton tested.
    /// A changed input invalidates the attribution. An unavailable read leaves it undecided.
    async fn singleton_observation(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskId,
        member: &Member,
        integration: &str,
        base: &str,
    ) -> Option<SingletonObservation> {
        let confirm = async {
            let token = self.dlv_installation_token(sandbox).await.ok()?;
            let pull = self
                .deps
                .github
                .get_pull(&token, &sandbox.repo_owner, &sandbox.repo_name, member.pr)
                .await
                .ok()?;
            let delivery = self.deps.store.delivery(task).await.ok()?;
            let current = delivery.is_some_and(|record| {
                record.stage == DeliveryStage::PrOpen && record.pr_number == Some(member.pr)
            }) && pull.number == member.pr
                && pull.state == "open"
                && !pull.merged
                && !pull.from_fork
                && pull.base == integration;
            if current && (pull.head_sha.is_empty() || pull.base_sha.is_empty()) {
                return None;
            }
            if !current || pull.head_sha != member.head || pull.base_sha != base {
                return Some(SingletonObservation::Changed);
            }
            // A failed read cannot establish that the owner's earlier running check ended.
            let runs = self
                .deps
                .store
                .ci_runs_for_pull_head(&sandbox.id, PullHead::Exact(&member.head))
                .await
                .ok()?;
            Some(SingletonObservation::Current(
                run_for_pull_head(&runs, member.pr).map(|run| (run.state, run.id.clone())),
            ))
        };
        tokio::time::timeout(Duration::from_secs(5), confirm)
            .await
            .ok()
            .flatten()
    }

    /// Ends the armed merge of `suspect`, the member a red train's bisection left out, once the
    /// others passed without it on the same base (`passed`): it failed with them and they pass
    /// without it, often a combination in a derived file (formatting, a generated list), and it is
    /// told now instead of failing a candidate of its own on top of them (SPEC-497 MQW-003). The
    /// message says what was seen, not why; an armed merge armed again since is kept.
    async fn tell_suspect(&self, sandbox: &SandboxRecord, suspect: &Suspect, passed: &Train) {
        let Ok(task) = TaskId::parse(&suspect.task) else {
            return;
        };
        let Some(deciding) = self.take_armed(&task) else {
            return;
        };
        if deciding.merge.armed_at != suspect.armed_at {
            let verdict = Verdict::Keep(deciding.merge.clone());
            self.finish_armed(&task, &deciding, verdict).await;
            return;
        }
        let integration = delivery_profile(sandbox).integration_branch;
        let own = match self
            .ci_gate_for_pull(sandbox, suspect.pr, &suspect.head)
            .await
        {
            GateResult::Run(Some(state)) => {
                format!("its own run at {} {}", short(&suspect.head), {
                    match state {
                        RunState::Passed => "passed".to_owned(),
                        RunState::Queued | RunState::Running => "still goes".to_owned(),
                        other => format!("ended {}", other.as_str()),
                    }
                })
            }
            _ => format!("no own run at {} was read", short(&suspect.head)),
        };
        let others = listed(&suspect.other_numbers());
        let verdict = Verdict::Decided(format!(
            "The merge you armed was dropped: pull request #{pr} merged into the current \
             `{integration}` together with {others} failed (merge candidate run {red}), and \
             {others} passed without it on the same `{integration}` (run {green}); {own}. Its \
             change fails with theirs, often in a derived file (formatting, a generated list), \
             or a test fails only there. Read why with `ci_status`, merge `origin/{integration}` \
             once they merged, fix it test-first, commit, then {keep}.",
            pr = suspect.pr,
            red = suspect.run,
            green = passed.run,
            keep = yulius_domain::review::KEEP_APPROVAL,
        ));
        self.finish_armed(&task, &deciding, verdict).await;
        self.train_audit(
            sandbox,
            "delivery.train.combined",
            &format!(
                "#{} failed with {others} (run {}), which passed without it (run {}): told at once",
                suspect.pr, suspect.run, passed.run
            ),
        )
        .await;
    }

    /// Whether merge candidates (trains) decide `sandbox`'s armed merges (SPEC-441 MQ-005): it
    /// has Yulius CI, lands by squash or merge commit and does not promote automatically.
    async fn candidates_apply(&self, sandbox: &SandboxRecord) -> bool {
        let profile = delivery_profile(sandbox);
        self.config.ci.enabled
            && sandbox.removed_at.is_none()
            && !sandbox.is_playground()
            // A promotion after each merge would ship states between the members (never tested).
            && !profile.auto_promote
            && profile.merge != MergeMethod::Rebase
            && self.ci_gate(sandbox).await.gate.is_some()
    }

    /// Whether `task`'s armed merge, green and behind its base, goes one by one (Yulius brings it
    /// up to date, DLV-203, and that head's own run decides) instead of into a merge candidate
    /// (SPEC-441 MQ-005).
    pub(crate) async fn one_by_one(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        armed: &ArmedMerge,
    ) -> bool {
        !self.candidates_apply(sandbox).await
            || self
                .read_trains(&sandbox.id)
                .held_back
                .contains(&(task.id.to_string(), armed.armed_at))
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
        state: &mut SandboxTrains,
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
            // Its own run may still go: the candidate's run tested the exact merged result, which
            // is enough to merge (SPEC-497 MQW-002); one that failed, at this pass's start or
            // since, makes it leave (review round 1).
            crate::race::reached(
                crate::race::Point::TrainMemberMerging,
                Path::new(&train.members[index].task),
            );
            if let Some(reason) = self.own_run_failed(sandbox, &train.members[index]).await {
                self.dissolve(sandbox, &train, &reason).await;
                return None;
            }
            train.own_wait = None;
            match self.train_step(sandbox, &train, index).await {
                Step::Merged => train.waiting_since = None,
                Step::WaitAtHead(head) => {
                    train.members[index].verified_head = Some(head);
                    return self.train_waits(sandbox, train).await;
                }
                Step::Wait => return self.train_waits(sandbox, train).await,
                Step::Stop(reason) => {
                    self.dissolve(sandbox, &train, &reason).await;
                    return None;
                }
                Step::OneByOne(reason) => {
                    let member = &train.members[index];
                    state.held_back.push((member.task.clone(), member.armed_at));
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
        self.train_audit(sandbox, yulius_domain::merge_queue::MERGED, &detail)
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
                self.finish_armed(&task_id, &deciding, verdict).await;
                return Step::Stop(format!("#{} cannot be read", member.pr));
            }
        };
        let (verdict, step) = self
            .train_member(sandbox, train, index, &task, &deciding)
            .await;
        self.finish_armed(&task_id, &deciding, verdict).await;
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
        // Most stops are this member's own: it goes one by one, where it is told why.
        let keep =
            |armed: ArmedMerge, reason: String| (Verdict::Keep(armed), Step::OneByOne(reason));
        if armed.armed_at != member.armed_at {
            return (
                Verdict::Keep(armed),
                Step::Stop(format!("#{} was armed again", member.pr)),
            );
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
        // A busy agent's turn ends soon: the train waits for it (bounded, TRN-006) instead of
        // throwing its passed run away (SPEC-441 MQ-003).
        if !idle || self.deps.git.has_changes(worktree).await.unwrap_or(true) {
            return (Verdict::Keep(armed), Step::Wait);
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
            return (
                Verdict::Keep(armed),
                Step::Stop(format!("#{} moved on", member.pr)),
            );
        }
        let profile = delivery_profile(sandbox);
        let integration = profile.integration_branch.clone();
        let Ok(token) = self.task_github_token(task, sandbox).await else {
            return keep(armed, format!("#{} has no GitHub token", member.pr));
        };
        let updating = self.now();
        let (head, moved) = match self
            .deps
            .git
            .merge_integration(worktree, &integration, &token)
            .await
        {
            Ok(MergeIntegration::UpToDate) => (armed.head.clone(), false),
            Ok(MergeIntegration::Merged(head)) => {
                self.integration_update_span(&task.id, &integration, updating, "merged")
                    .await;
                (head, true)
            }
            // Aborted: the one-by-one flow tells the agent what to resolve.
            Ok(MergeIntegration::Conflict(_)) => {
                self.integration_update_span(&task.id, &integration, updating, "conflict")
                    .await;
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
            match self
                .dlv_push_head(&task.id, worktree, branch, &head, &token)
                .await
            {
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
            if let Err(err) = self
                .dlv_push_head(&task.id, worktree, branch, &head, &token)
                .await
            {
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
            return (Verdict::Keep(moved_armed), Step::WaitAtHead(head));
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
            token: None,
        };
        match self
            .merge_task_pr_for_task(&auth, &task.id.to_string())
            .await
        {
            Ok(merged) => {
                let others: Vec<u64> = train
                    .members
                    .iter()
                    .map(|m| m.pr)
                    .filter(|n| *n != member.pr)
                    .collect();
                let (actor, action, how) = armed.merge_audit(task.number);
                let detail = format!(
                    "merged at head {head} {how}; armed while the checks ran; in merge train {} \
                     tested at {} (run {})",
                    listed(&numbers(&train.members)),
                    train.commit,
                    train.run
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
                    tracing::warn!(task = %task.id, error = %err, "audit of a train merge failed");
                }
                let next = self.after_merge_of(task, sandbox, merged.number).await;
                let peers = if others.is_empty() {
                    String::new()
                } else {
                    format!(" with {}", listed(&others))
                };
                let text = format!(
                    "{} in a merge train{peers}. Yulius CI passed on the candidate at {}. {next}",
                    merged.receipt,
                    short(&train.commit)
                );
                (Verdict::Decided(text), Step::Merged)
            }
            // GitHub could not take the pull request's new title for now: the train waits for
            // the next pass, as for a head GitHub does not show yet (SPEC-069 BPL-008).
            Err(MergeFailure::Later(_)) => (Verdict::Keep(moved_armed), Step::WaitAtHead(head)),
            Err(MergeFailure::Refused(err)) => keep(
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
            .ci_runs_for_pull_head(&sandbox.id, crate::ci::PullHead::Exact(head))
            .await
            .map_err(|_| Step::Wait)?;
        if let Some(latest) = crate::ci::run_for_pull_head(&runs, member.pr) {
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
            .map(|job| {
                // A job the train skipped stays skipped in the verdict (SPEC-162).
                let (state, exit_code) = yulius_domain::ci::copied_job_verdict(Some(job.state));
                CiJobRecord {
                    id: self.deps.crypto.new_id(),
                    run_id: run.id.clone(),
                    ordinal: job.ordinal,
                    name: job.name.clone(),
                    key: job.key.clone(),
                    command: job.command.clone(),
                    matrix: job.matrix.clone(),
                    state,
                    // A full-suite skip stays one through the copy (SPEC-575 MFS-002).
                    reason: Some(yulius_domain::ci::copied_job_reason(
                        state,
                        job.reason.as_deref(),
                        &note,
                    )),
                    exit_code: Some(exit_code),
                    output: format!("{note}.\n"),
                    failures: None,
                    started_at: Some(now),
                    ended_at: Some(now),
                    need: job.need,
                    timeout_minutes: job.timeout_minutes,
                    exclusive: job.exclusive,
                    waited_ms: None,
                }
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

    /// Forms a train from the armed pull requests waiting now whose own run passed and whose base
    /// moved since (SPEC-441 MQ-003), if any, at most the bisection's limit of them.
    async fn form_train(
        &self,
        sandbox: &SandboxRecord,
        state: &mut SandboxTrains,
    ) -> Option<Train> {
        let profile = delivery_profile(sandbox);
        if !self.candidates_apply(sandbox).await {
            return None;
        }
        let landing = match profile.merge {
            MergeMethod::Squash => TrainLanding::Squash,
            MergeMethod::Merge => TrainLanding::MergeCommit,
            MergeMethod::Rebase => return None,
        };
        let view = self.ci_gate(sandbox).await;
        let gate = view.gate.clone()?;
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
            let checks = self.ci_gate_for_pull(sandbox, pr, &armed.head).await;
            let up_to_date = matches!(
                self.deps
                    .git
                    .is_ancestor(&repo, &integration, &armed.head)
                    .await,
                Ok(true)
            );
            let passed = checks == GateResult::Run(Some(RunState::Passed));
            // One that can merge right now goes first, on its own: a train waits for it.
            if up_to_date && passed {
                return None;
            }
            // One whose own run passed or still goes (SPEC-493 MQC-001: the candidate's run goes
            // beside it; SPEC-497 MQW-002: it merges once the candidate passed), behind its base.
            let going = matches!(
                checks,
                GateResult::Run(Some(RunState::Queued | RunState::Running))
            );
            if !(passed || going)
                || up_to_date
                || state
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
        // Derived files the project names are changed by one member at a time: a later one waits
        // for the next candidate (SPEC-497 MQW-003).
        let (mut candidates, held) = self.hold_apart(&repo, &integration, candidates).await;
        candidates.truncate(state.limit.unwrap_or(MAX_MEMBERS).max(1));
        if candidates.is_empty() {
            return None;
        }
        let heads: Vec<String> = candidates.iter().map(|c| c.armed.head.clone()).collect();
        let key = format!("{base}:{}", heads.join(","));
        if state.unformed.as_deref() == Some(key.as_str()) {
            return None;
        }
        // One merged meanwhile (by hand) is not tested again: GitHub says so before a build, and
        // its own decision then says what the merge counts as (SPEC-441 MQ-009).
        let mut open = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if self.pull_open(sandbox, candidate.pr).await {
                open.push(candidate);
            }
        }
        let candidates = open;
        if candidates.is_empty() {
            return None;
        }
        let heads: Vec<String> = candidates.iter().map(|c| c.armed.head.clone()).collect();
        let key = format!("{base}:{}", heads.join(","));
        let ci_dir = self.ci_dir(&sandbox.id);
        // Readied through handles: Git checks out in it by its path (SPEC-367).
        // A train that cannot be built leaves its candidates to the one-by-one flow.
        let one_by_one = |state: &mut SandboxTrains, key: String| {
            state.unformed = Some(key);
            state.held_back.extend(
                candidates
                    .iter()
                    .map(|c| (c.task.id.to_string(), c.armed.armed_at)),
            );
        };
        if let Err(err) = self.ci_checkout_ready(&sandbox.id, &[], "train").await {
            tracing::warn!(sandbox = %sandbox.slug, error = %err, "a merge train could not be built");
            one_by_one(state, key);
            return None;
        }
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
                one_by_one(state, key);
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
                    verified_head: None,
                    armed_at: candidate.armed.armed_at,
                    tree,
                }),
                TrainStep::Conflict(files) => left_out.push(LeftOut {
                    task: candidate.task.id.to_string(),
                    pr: candidate.pr,
                    head: Some(candidate.armed.head.clone()),
                    armed_at: Some(candidate.armed.armed_at),
                    files,
                }),
            }
        }
        // Every one conflicts with the integration branch itself: one by one, where Yulius
        // names the files to resolve (DLV-203).
        let Some(last) = members.last() else {
            one_by_one(state, key);
            return None;
        };
        // The integration branch's jobs, with the members' own changed ones beside them, as for a
        // pull request (`ci_gate_for_head`, SPEC-504 CFG-001).
        let gate = self
            .ci_gate_for_head(sandbox, &view, &built.head)
            .await
            .unwrap_or(gate);
        let prs = numbers(&members);
        // Ahead of the sandbox's queued runs: it decides every member's merge at once.
        let place = self
            .deps
            .store
            .ci_runs(&sandbox.id, 50)
            .await
            .ok()
            .and_then(|runs| {
                runs.iter()
                    .filter(|r| r.state == RunState::Queued)
                    .map(|r| r.queued_at.0)
                    .min()
            })
            .map_or_else(|| self.now(), |first| Timestamp(first - 1));
        let reused = self
            .ci_reuse_for_candidate(
                sandbox,
                CiRef::Train(prs.clone()),
                &built.head,
                &last.tree,
                &gate,
            )
            .await;
        let queued = match reused {
            Err(err) => Err(err),
            Ok(Some(run)) => Ok(run),
            Ok(None) => {
                self.ci_queue_placed(
                    sandbox,
                    CiRef::Train(prs.clone()),
                    &built.head,
                    &gate,
                    "yulius",
                    place,
                )
                .await
            }
        };
        let run = match queued {
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
            yulius_domain::merge_queue::FORMED,
            &format!(
                "merge train {} on {} at {}: run {} decides {}{}{}",
                listed(&prs),
                profile.integration_branch,
                built.base,
                run.id,
                built.head,
                if left.is_empty() {
                    String::new()
                } else {
                    format!("; left out (conflicts): {}", listed(&left))
                },
                if held.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; held apart for the next candidate (derived files): {}",
                        held.iter()
                            .map(|h| format!("#{} (with #{}, {})", h.pr, h.with, h.pattern))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
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
            formed_at: self.now().0,
            own_wait: None,
        })
    }

    /// `candidates` (in arm order) without those `[merge_queue] apart` of the integration branch's
    /// `yulius.toml` holds out of this candidate, and those held (SPEC-497 MQW-003): two that
    /// change files under one of its patterns never share a candidate. A table that does not parse
    /// holds nothing apart, as no table does.
    async fn hold_apart(
        &self,
        repo: &Path,
        integration: &str,
        candidates: Vec<Candidate>,
    ) -> (Vec<Candidate>, Vec<yulius_domain::merge_queue::HeldApart>) {
        let apart = match self
            .deps
            .git
            .show_file(repo, integration, "yulius.toml")
            .await
        {
            Ok(Some(text)) => {
                yulius_domain::spec_format::parse_yulius_toml_merge_queue(&text).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        if apart.is_empty() {
            return (candidates, Vec::new());
        }
        let mut changed = Vec::with_capacity(candidates.len());
        for candidate in &candidates {
            let paths = self
                .deps
                .git
                .changed_paths(repo, integration, &candidate.armed.head)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            changed.push((candidate.pr, paths));
        }
        let (kept, held) = yulius_domain::merge_queue::held_apart(&apart, &changed);
        let candidates = candidates
            .into_iter()
            .filter(|c| kept.contains(&c.pr))
            .collect();
        (candidates, held)
    }

    /// Whether GitHub shows pull request `pr` open; `true` when it cannot be read (the merge
    /// path asks again).
    async fn pull_open(&self, sandbox: &SandboxRecord, pr: u64) -> bool {
        let Ok(token) = self.dlv_installation_token(sandbox).await else {
            return true;
        };
        match self
            .deps
            .github
            .get_pull(&token, &sandbox.repo_owner, &sandbox.repo_name, pr)
            .await
        {
            Ok(pull) => !pull.merged && pull.state == "open",
            Err(_) => true,
        }
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

/// Why a train was dissolved, from the reason this module gives (SPEC-493 MQC-005).
fn dissolved_why(reason: &str) -> yulius_domain::merge_queue::Dissolved {
    use yulius_domain::merge_queue::Dissolved;
    let has = |words: &[&str]| words.iter().any(|w| reason.contains(w));
    if has(&["too long"]) {
        Dissolved::TooLong
    } else if has(&["ended failed: it leaves the candidate"])
        || (reason.starts_with("the own run of #") && reason.ends_with("ended failed"))
    {
        Dissolved::MemberFailed
    } else if has(&[
        "leaves the candidate",
        "was armed again",
        "is no longer armed",
        "merged before the ones ahead",
        "cannot be read",
        "has no worktree",
        "moved on",
        "the own run of #",
    ]) {
        Dissolved::MemberLeft
    } else if has(&["its CI run failed"]) {
        Dissolved::Bisected
    } else if has(&["did not predict", "the train predicted"]) {
        Dissolved::BaseMoved
    } else {
        Dissolved::Errored
    }
}

/// Read the train registry. A missing file means no sandbox has a train.
fn read_all(path: &Path) -> std::io::Result<BTreeMap<String, SandboxTrains>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(task: &str, pr: u64, tree: &str) -> Member {
        Member {
            task: task.to_owned(),
            pr,
            head: format!("head-{task}"),
            verified_head: None,
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
                head: Some("head-d".into()),
                armed_at: Some(1),
                files: vec!["shared.txt".to_owned()],
            }],
            merging: false,
            waiting_since: None,
            formed_at: 1,
            own_wait: None,
        }
    }

    /// SPEC-497 MQW-003: a left-out member is blamed only by the others passing without it on the
    /// red train's base; another set of members or another base proves nothing.
    #[test]
    fn a_suspect_is_proven_only_by_the_others_on_the_same_base() {
        let suspect = Suspect {
            task: "c".to_owned(),
            pr: 13,
            head: "head-c".to_owned(),
            armed_at: 1,
            base: "base".to_owned(),
            others: vec![(11, "head-a".to_owned()), (12, "head-b".to_owned())],
            run: "red".to_owned(),
        };
        let mut passed = train();
        passed.members.truncate(2);
        assert!(suspect.proven_by(&passed));
        let mut moved = passed.clone();
        moved.base = "later".to_owned();
        assert!(!suspect.proven_by(&moved), "another base");
        let mut fewer = passed.clone();
        fewer.members.truncate(1);
        assert!(!suspect.proven_by(&fewer), "#12 untested");
        let mut rearmed = passed.clone();
        rearmed.members[1].head = "head-b2".to_owned();
        assert!(
            !suspect.proven_by(&rearmed),
            "#12 armed again with another head (review round 1)"
        );
        let mut itself = passed;
        itself.members = vec![member("c", 13, "t3")];
        assert!(!suspect.proven_by(&itself), "it passed alone");
    }

    /// SPEC-493 MQC-005: each reason this module dissolves a train for names its why.
    #[test]
    fn the_why_of_each_dissolve() {
        use yulius_domain::merge_queue::Dissolved;
        for (reason, why) in [
            (
                "#2's own run at abc ended failed: it leaves the candidate",
                Dissolved::MemberFailed,
            ),
            ("#3 was armed again", Dissolved::MemberLeft),
            ("#3 is no longer armed", Dissolved::MemberLeft),
            ("its CI run failed", Dissolved::Bisected),
            (
                "`dev` moved to a state the train did not predict",
                Dissolved::BaseMoved,
            ),
            (
                "`dev` is not in the state the train predicted before #4",
                Dissolved::BaseMoved,
            ),
            ("its CI run took too long", Dissolved::TooLong),
            ("it waited too long to merge", Dissolved::TooLong),
            ("its CI run ended cancelled", Dissolved::Errored),
            ("its CI run is gone", Dissolved::Errored),
            ("#1's own run took too long", Dissolved::TooLong),
            ("#4 moved on", Dissolved::MemberLeft),
            (
                "the own run of #2 at abc ended failed",
                Dissolved::MemberFailed,
            ),
            (
                "the own run of #2 at abc ended error",
                Dissolved::MemberLeft,
            ),
        ] {
            assert_eq!(dissolved_why(reason), why, "{reason}");
        }
    }

    /// SPEC-441 MQ-003 (was TRN-004): under test and once it passed, the train holds every member,
    /// the first too; others are free.
    #[test]
    fn a_train_holds_every_member() {
        let mut train = train();
        assert_eq!(hold_of(None, "a"), Hold::Free);
        assert_eq!(hold_of(Some(&train), "a"), Hold::Held);
        assert_eq!(hold_of(Some(&train), "b"), Hold::Held);
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

    #[test]
    fn a_train_member_whose_activation_is_unavailable_waits_for_verification() {
        let train = train();
        let text = note_of(&train, "a", None)
            .map(|note| note.text)
            .unwrap_or_default();
        assert!(text.contains("verifies its current head"), "{text}");
        assert!(!text.contains("run1"));
    }

    /// TRN-007: a member hears its place; one left out hears why.
    #[test]
    fn the_wait_says_where_a_pull_request_stands() {
        let mut train = train();
        let note = note_of(&train, "b", Some(("head-b", 1)))
            .map(|note| note.text)
            .unwrap_or_default();
        assert!(
            note.contains("#12 is number 2 of 3 in merge train #11, #12, #13"),
            "{note}"
        );
        assert!(note.contains("run run1"), "{note}");
        let left = note_of(&train, "d", Some(("head-d", 1)))
            .map(|note| note.text)
            .unwrap_or_default();
        assert!(
            left.contains("#14 was left out") && left.contains("`shared.txt`"),
            "{left}"
        );
        assert_eq!(note_of(&train, "x", None), None);
        train.own_wait = Some((12, 0));
        assert!(note_of(&train, "b", Some(("head-b", 1))).is_some_and(|note| note.awaits_own_run));
        assert!(
            note_of(&train, "a", Some(("head-a", 1)))
                .is_some_and(|note| !note.awaits_own_run && note.text.contains("#12's own"))
        );
        train.own_wait = None;
        train.merging = true;
        let merging = note_of(&train, "a", Some(("head-a", 1)))
            .map(|note| note.text)
            .unwrap_or_default();
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
            limit: Some(2),
            suspect: Some(Suspect {
                task: "c".to_owned(),
                pr: 13,
                head: "head-c".to_owned(),
                armed_at: 1,
                base: "base".to_owned(),
                others: vec![(11, "head-a".to_owned()), (12, "head-b".to_owned())],
                run: "red".to_owned(),
            }),
        };
        let text = serde_json::to_string(&state)?;
        assert_eq!(serde_json::from_str::<SandboxTrains>(&text)?, state);
        assert!(SandboxTrains::default().is_empty());
        let old: SandboxTrains = serde_json::from_str("{}")?;
        assert!(old.is_empty());
        Ok(())
    }
}
