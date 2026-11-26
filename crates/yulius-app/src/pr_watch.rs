//! Tasks hear what happens to their pull request (SPEC-444, ADR-0287).
//!
//! A task used to hear about its pull request only through `yulius wait`, a background shell it
//! had to keep alive; Claude Code's goal check-ins pause after half an hour. Every
//! [`WATCH_EVERY`], this watch looks at each running task whose delivery has an open pull
//! request, from Yulius's own records and clone, and types what needs the task into its session
//! with the next step (PRN-001..PRN-006): a red run at its head or of a merge candidate holding
//! it, a conflict with the integration branch (of the run's head, or the branch's own where no
//! run exists), how its armed merge ended, its merge, and why a
//! green head is not merging, with "Hands off" while a candidate holds it. Each event is typed
//! once, by key, kept in [`PR_NOTICES_FILE`] (PRN-007). A session quiet for
//! [`CHECK_IN_AFTER_MS`] gets a check-in (PRN-008), and a pull request with no progress for
//! [`STALL_AFTER_MS`] while its CI is idle is stalled: told once and shown on the task list
//! (PRN-009). This amends SPEC-028 MSR-004 ("nothing is typed into an idle session") for a task
//! with an open pull request.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_domain::ci::{CiRef, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

use crate::agent_wait::{ArmedNow, ArmedSnapshot, QueueNote, Readiness};
use crate::ci::{CiJobRecord, CiRunRecord};
use crate::context::App;
use crate::delivery::{delivery_profile, past_merge};
use crate::error::{AppError, AppResult};
use crate::merge_train::{Hold, UnverifiedTrain, WatchedTrain};
use crate::model::{SandboxRecord, TaskRecord};
use crate::task_message::{Sender, Typed};

/// How often the watch runs: an event reaches the task within it.
pub const WATCH_EVERY: Duration = Duration::from_secs(20);
/// A session idle this long (ms) with an open pull request and nothing typed gets a check-in.
pub const CHECK_IN_AFTER_MS: i64 = 45 * 60 * 1000;
/// A pull request without progress this long (ms) while its CI is idle is stalled.
pub const STALL_AFTER_MS: i64 = 60 * 60 * 1000;
/// A stall waits this long (ms) after the agent's last turn closed: the push and the run that
/// turn's end starts take a moment (review round 2).
pub const STALL_QUIET_MS: i64 = 5 * 60 * 1000;
/// An armed green PR with no merge candidate running and no progress gets a check-in after this
/// long (ms) so it never waits silently (SPEC-580 AWM-004).
pub const ARMED_CHECK_IN_AFTER_MS: i64 = 10 * 60 * 1000;
/// If the armed-PR check-in does not move the PR within this long (ms), Yulius escalates to
/// Needs You using the SPEC-578 mechanism (SPEC-580 AWM-004).
pub const ARMED_NEEDS_YOU_AFTER_MS: i64 = 10 * 60 * 1000;
/// The file in the data directory that keeps what each task was told across restarts.
pub const PR_NOTICES_FILE: &str = "pr-notices.json";
/// Runs of a sandbox read per pass, newest first.
pub(crate) const RUNS_READ: u32 = 100;
/// Failing test names said per job.
const MAX_TESTS: usize = 5;

/// Serializes changes to [`PR_NOTICES_FILE`].
static NOTICES_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The watch's bounds: the constants above in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchBounds {
    /// Quiet time (ms) before a check-in.
    pub check_in_after_ms: i64,
    /// Time (ms) without progress before a stall.
    pub stall_after_ms: i64,
    /// Time (ms) after the agent's last turn closed before a stall.
    pub stall_quiet_ms: i64,
    /// Time (ms) an armed green PR may wait without testing or merging before a check-in.
    pub armed_check_in_after_ms: i64,
    /// Time (ms) after that check-in (or after the check-in bound if delivery is blocked) before
    /// escalating to Needs You.
    pub armed_needs_you_after_ms: i64,
}

impl Default for WatchBounds {
    fn default() -> Self {
        Self {
            check_in_after_ms: CHECK_IN_AFTER_MS,
            stall_after_ms: STALL_AFTER_MS,
            stall_quiet_ms: STALL_QUIET_MS,
            armed_check_in_after_ms: ARMED_CHECK_IN_AFTER_MS,
            armed_needs_you_after_ms: ARMED_NEEDS_YOU_AFTER_MS,
        }
    }
}

/// One thing that happened to the pull request, for the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// What kind (`run`, `candidate`, `conflict`, `outcome`, `green`, `merged`): one key each.
    pub kind: &'static str,
    /// Which one: typed once per key.
    pub key: String,
    /// What happened and what to do.
    pub text: String,
}

/// Durable activity and current attention from the same read that admits an armed notice.
pub struct NoticeRecipient {
    /// The agent began a turn, including one its transcript has yet to report.
    pub working: bool,
    /// The current reason the task needs attention, if any.
    pub attention: Option<crate::attention::AttentionRecord>,
}

/// What the watch knows of a task's pull request at one pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrFacts {
    /// Now (ms).
    pub now_ms: i64,
    /// The pull request.
    pub pr: u64,
    /// What happened (keys already told may be in it; [`decide`] skips them).
    pub events: Vec<Event>,
    /// Its runs, delivery and armed merge, in one string: a change is progress.
    pub progress: String,
    /// A run of its head, or of a merge candidate holding it, is queued or running.
    pub ci_busy: bool,
    /// Where it stands and what to do, in one or two sentences (check-in and stall).
    pub status: String,
    /// The session is idle: no turn open, no person typing.
    pub idle: bool,
    /// Since when (ms) the session is quiet: its last turn closed, or its run started.
    pub quiet_since_ms: i64,
    /// It may get a check-in: it does not wait for a person, its goal is not achieved.
    pub check_ins: bool,
    /// A person must act first (a permission prompt or question on screen, `needs_you`): nothing
    /// is typed, as the paste would answer it (review M1).
    pub person_must_act: bool,
    /// The agent's turn is open: it works, which is no stall.
    pub working: bool,
    /// The project has Yulius CI: without it no run ever comes, which is no stall (review M2).
    pub has_ci: bool,
    /// A newly observed ordinary rerun whose wait must survive the next pass and restart.
    pub singleton_wait: Option<SingletonWait>,
    /// The pull request's head, from its newest run (empty when none).
    pub head: String,
    /// An armed green PR with no active candidate in the sandbox that has not yet been tested or
    /// merged (SPEC-580 AWM-004).
    pub armed_green_waiting: bool,
    /// The armed head SHA while [`Self::armed_green_waiting`] is true.
    pub armed_head: String,
    /// The integration branch name for armed-PR stall messages.
    pub integration: String,
    /// Why the armed green PR has not yet entered a merge candidate or merged.
    pub armed_wait_reason: String,
    /// Pass deferred while an armed merge decision is in progress: leave stall clocks unchanged.
    pub deferred: bool,
}

/// First watcher observation of an ordinary rerun holding a recorded queue decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SingletonWait {
    request: String,
    run: String,
    since_ms: i64,
}

/// Evidence for a notice from a live merge request or the queue's last completed decision.
struct QueueNoticeSource {
    head: String,
    decision: Option<(crate::stages::SpanRecord, String)>,
    ordinary: Option<CiRunRecord>,
    candidate_id: Option<String>,
    candidate: Option<CiRunRecord>,
    train: Result<Option<WatchedTrain>, UnverifiedTrain>,
}

/// What the watch told one task, as saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    /// The pull request it is about: another one starts afresh.
    pub pr: u64,
    /// The ordinary rerun holding the last observed singleton and when its wait started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub singleton_wait: Option<SingletonWait>,
    /// The last key typed of each kind.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// Event keys an admitted Codex notice already failed to submit. A second
    /// refusal raises that exact notice to the owner. Older receipts start empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deferred_keys: BTreeMap<String, String>,
    /// The progress string last seen.
    #[serde(default)]
    pub progress: String,
    /// Since when (ms) it is the same.
    #[serde(default)]
    pub progress_ms: i64,
    /// When Yulius last typed anything to it (ms).
    #[serde(default)]
    pub typed_ms: i64,
    /// While stalled: the sentence the task list shows (PRN-009).
    #[serde(default)]
    pub stalled: Option<String>,
    /// The pull request's head as its newest run last showed it: its run is read by commit once
    /// it is out of the runs read per pass (review minor 2).
    #[serde(default)]
    pub head: String,
    /// `"{pr}:{armed_head}"` while an armed green PR is waiting without progress (SPEC-580).
    #[serde(default)]
    pub armed_green_key: String,
    /// When that armed green PR started waiting without progress (ms, 0 = none).
    #[serde(default)]
    pub armed_green_since_ms: i64,
    /// When Yulius delivered the armed-PR check-in to the agent (ms, 0 = not delivered yet).
    #[serde(default)]
    pub armed_check_in_ms: i64,
    /// Timestamp (`at`) of the Yulius-opened `NeedsYouRecord` for this armed-PR stall (0 = none).
    #[serde(default)]
    pub armed_needs_you_at: i64,
}

impl Told {
    /// Clears all armed-green PR stall tracking fields together so a deferred pass can never
    /// preserve a half-cleared stall clock (SPEC-580 merge review F2).
    fn clear_armed_stall(&mut self) {
        self.armed_green_key.clear();
        self.armed_green_since_ms = 0;
        self.armed_check_in_ms = 0;
        self.armed_needs_you_at = 0;
    }
}

/// Bounded action for a stalled armed green PR (SPEC-580 AWM-004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmedStallAction {
    /// No check-in or escalation is due on this pass.
    None,
    /// Send a check-in into the agent's session.
    CheckIn(String),
    /// Escalate to Needs You via SPEC-578 (`tasks.needs_you` + `NeedsYouNotice` + `wait_history`).
    Escalate {
        /// What happened (names PR, head, elapsed wait, and check-in outcome).
        happened: String,
        /// Current blocker or wait note.
        problem: String,
        /// Concrete next action for the person.
        action: String,
    },
}

/// Updates the armed-green PR wait clock on `next` and decides whether a check-in or Needs You
/// escalation is due (SPEC-580 AWM-004). Runs independently of `facts.working` so a stuck working
/// turn or background command cannot silence the bound.
#[must_use]
pub fn decide_armed_stall(
    facts: &PrFacts,
    next: &mut Told,
    bounds: &WatchBounds,
    task_number: u32,
) -> ArmedStallAction {
    if facts.deferred {
        return ArmedStallAction::None;
    }
    if !facts.armed_green_waiting || facts.armed_head.is_empty() {
        next.armed_green_key.clear();
        next.armed_green_since_ms = 0;
        next.armed_check_in_ms = 0;
        next.armed_needs_you_at = 0;
        return ArmedStallAction::None;
    }
    let key = format!("{}:{}", facts.pr, facts.armed_head);
    if next.armed_green_key != key {
        next.armed_green_key = key;
        next.armed_green_since_ms = facts.now_ms;
        next.armed_check_in_ms = 0;
        next.armed_needs_you_at = 0;
        return ArmedStallAction::None;
    }
    if next.armed_needs_you_at > 0 || facts.person_must_act {
        return ArmedStallAction::None;
    }
    let waited_ms = facts.now_ms.saturating_sub(next.armed_green_since_ms);
    let integration = if facts.integration.is_empty() {
        "the integration branch"
    } else {
        facts.integration.as_str()
    };
    let reason = if facts.armed_wait_reason.is_empty() {
        format!(
            "Yulius is waiting to test it on top of `{integration}` or merge it, but no progress \
             has been made."
        )
    } else {
        facts.armed_wait_reason.clone()
    };
    let escalate_due = if next.armed_check_in_ms > 0 {
        facts.now_ms.saturating_sub(next.armed_check_in_ms) >= bounds.armed_needs_you_after_ms
    } else {
        waited_ms
            >= bounds
                .armed_check_in_after_ms
                .saturating_add(bounds.armed_needs_you_after_ms)
    };
    if escalate_due {
        let total_min = (waited_ms / 60_000).max(1);
        let check_in_note = if next.armed_check_in_ms > 0 {
            "even after a check-in to the agent"
        } else {
            "and Yulius could not deliver a check-in to the session"
        };
        let happened = format!(
            "Armed green PR #{} at {} has not been tested on `{integration}` or merged for \
             {total_min} min ({check_in_note}).",
            facts.pr,
            short(&facts.armed_head),
        );
        let action = format!(
            "Open task #{task_number} to inspect PR #{}, check why it has not entered the merge \
             queue, or nudge the agent.",
            facts.pr
        );
        return ArmedStallAction::Escalate {
            happened,
            problem: reason,
            action,
        };
    }
    if next.armed_check_in_ms == 0 && waited_ms >= bounds.armed_check_in_after_ms {
        let minutes = (waited_ms / 60_000).max(1);
        let text = format!(
            "Your armed merge at {} has been green for {minutes} min without being tested on top \
             of `{integration}` or merged. {reason} Check `delivery_status` and `git status`, \
             commit or clean any uncommitted changes, and end your turn cleanly (without a \
             non-wait background command running) so the merge queue can pick it up.",
            short(&facts.armed_head),
        );
        return ArmedStallAction::CheckIn(message(facts.pr, &[&text]));
    }
    ArmedStallAction::None
}

/// Decides what to type to a task (one message, or nothing) and what it was told afterwards.
/// Events come first, then a stall, then a check-in; a message to the task is progress.
#[must_use]
pub fn decide(facts: &PrFacts, told: &Told, bounds: &WatchBounds) -> (Option<String>, Told) {
    let mut next = if told.pr == facts.pr {
        told.clone()
    } else {
        Told {
            pr: facts.pr,
            progress: facts.progress.clone(),
            progress_ms: facts.now_ms,
            ..Told::default()
        }
    };
    if let Some(wait) = &facts.singleton_wait {
        next.singleton_wait = Some(wait.clone());
    }
    if !facts.head.is_empty() {
        next.head.clone_from(&facts.head);
    }
    if next.progress != facts.progress {
        next.progress.clone_from(&facts.progress);
        next.progress_ms = facts.now_ms;
        next.stalled = None;
    }
    if facts.person_must_act {
        return (None, next);
    }
    let new: Vec<&Event> = facts
        .events
        .iter()
        .filter(|event| next.keys.get(event.kind) != Some(&event.key))
        .collect();
    if !new.is_empty() {
        for event in &new {
            next.keys.insert(event.kind.to_owned(), event.key.clone());
        }
        next.typed_ms = facts.now_ms;
        next.progress_ms = facts.now_ms;
        next.stalled = None;
        let texts: Vec<&str> = new.iter().map(|event| event.text.as_str()).collect();
        return (Some(message(facts.pr, &texts)), next);
    }
    let still = facts.now_ms.saturating_sub(next.progress_ms);
    if facts.has_ci
        && !facts.ci_busy
        && !facts.working
        && facts.now_ms.saturating_sub(facts.quiet_since_ms) >= bounds.stall_quiet_ms
        && next.stalled.is_none()
        && still >= bounds.stall_after_ms
    {
        let minutes = still / 60_000;
        let sentence = format!(
            "Pull request #{} has made no progress for {minutes} min: {}",
            facts.pr, facts.status
        );
        next.stalled = Some(sentence);
        next.typed_ms = facts.now_ms;
        let text = format!(
            "It has made no progress for {minutes} min (no run, no merge, no message), and \
             Yulius shows it as stalled on the task list. {} If `delivery_status` shows nothing \
             that blocks it, say so in your summary; never merge it by hand.",
            facts.status
        );
        return (Some(message(facts.pr, &[&text])), next);
    }
    let quiet_since = facts.quiet_since_ms.max(next.typed_ms);
    if facts.idle
        && facts.check_ins
        && facts.now_ms.saturating_sub(quiet_since) >= bounds.check_in_after_ms
    {
        next.typed_ms = facts.now_ms;
        let text = format!("Check-in while it is open. {}", facts.status);
        return (Some(message(facts.pr, &[&text])), next);
    }
    (None, next)
}

/// One message about pull request `pr`: its texts in one paste, a list when several.
fn message(pr: u64, texts: &[&str]) -> String {
    let body = match texts {
        [one] => format!(" {one}"),
        many => many
            .iter()
            .map(|text| format!("\n- {text}"))
            .collect::<String>(),
    };
    format!("Yulius, about your pull request #{pr}:{body}")
}

/// Terminal failures and tests observed before fail-fast stopped a sibling (SPEC-566).
pub(crate) fn failed_jobs(jobs: &[CiJobRecord]) -> String {
    let said: Vec<String> = jobs
        .iter()
        .filter_map(|job| {
            let kept = job.failures.as_deref().unwrap_or_default();
            let (how, label, names) = match job.state {
                RunState::Failed | RunState::Error => {
                    let how = job.exit_code.map_or_else(
                        || job.reason.clone().unwrap_or_else(|| "failed".to_owned()),
                        |code| format!("exited {code}"),
                    );
                    (
                        how,
                        "failing tests",
                        yulius_domain::ci::failures::failed_test_names(kept),
                    )
                }
                RunState::Cancelled => {
                    let names = crate::ci::sibling_failure_names(job.reason.as_deref(), kept);
                    if names.is_empty() {
                        return None;
                    }
                    ("cancelled".to_owned(), "partial failing tests", names)
                }
                _ => return None,
            };
            let mut summary = format!("`{}` {how}", job.name);
            if !names.is_empty() {
                let shown = names.len().min(MAX_TESTS);
                let list = names[..shown].join(", ");
                summary.push_str(&format!(" ({label}: {list}"));
                if names.len() > shown {
                    summary.push_str(&format!(" and {} more", names.len() - shown));
                }
                summary.push(')');
            }
            let omitted = yulius_domain::ci::failures::omitted_tests(kept);
            if omitted > 0 {
                summary.push_str(&format!(" ({omitted} additional failure blocks omitted)"));
            }
            Some(summary)
        })
        .collect();
    if said.is_empty() {
        "a job failed".to_owned()
    } else {
        said.join("; ")
    }
}

/// PRN-002: a red run at the pull request's head.
fn red_run_text(run: &CiRunRecord, jobs: &[CiJobRecord]) -> String {
    format!(
        "Its Yulius CI run {} at {} failed: {}. Read `ci_status` with run_id {}. \
         Set job to a listed name for its full details. Fix it test-first on this branch, commit and end your turn \
         so Yulius pushes; a failed run drops an armed merge, so once the new head passed: {}.",
        run.id,
        short(&run.sha),
        failed_jobs(jobs),
        run.id,
        yulius_domain::review::KEEP_APPROVAL
    )
}

/// PRN-002: a run at the head that could not run.
fn errored_run_text(run: &CiRunRecord) -> String {
    format!(
        "Its Yulius CI run {} at {} could not run{}: that is not the code. Yulius reruns it once \
         by itself; if it errors again, rerun it with `ci_rerun`.",
        run.id,
        short(&run.sha),
        run.reason
            .as_deref()
            .map(|why| format!(" ({why})"))
            .unwrap_or_default()
    )
}

/// The words that keep a task's hands off a pull request a merge candidate holds (PRN-006).
fn hands_off(integration: &str) -> String {
    format!(
        "Hands off: do not re-arm, merge `{integration}` or push to this branch; Yulius merges it \
         in order when the run passes."
    )
}

/// PRN-003: a red merge candidate holding the pull request.
fn red_candidate_text(
    run: &CiRunRecord,
    members: &[u64],
    jobs: &[CiJobRecord],
    integration: &str,
) -> String {
    let listed: Vec<String> = members.iter().map(|n| format!("#{n}")).collect();
    format!(
        "Merge candidate {} (run {}), which holds it, failed: {}. Read `ci_status` with run_id {}. \
         Set job to a listed name for its full details. Yulius tests smaller candidates \
         and tells the pull request that breaks it. Do not re-arm, merge `{integration}` or push \
         to this branch meanwhile.",
        listed.join(", "),
        run.id,
        failed_jobs(jobs),
        run.id
    )
}

/// PRN-004: the head conflicts with the integration branch.
fn conflict_text(head: &str, integration: &str) -> String {
    format!(
        "Its head {} conflicts with `{integration}` as Yulius last fetched it, so it cannot \
         merge. Merge it in: `git fetch origin && git merge origin/{integration}`, resolve the \
         conflicts, rerun the affected tests, commit and end your turn so Yulius pushes; then \
         get the review of the new head and call `delivery_merge`. Never rebase or force-push.",
        short(head)
    )
}

/// PRN-006: a green head with no merge armed.
fn unarmed_text(head: &str) -> String {
    format!(
        "Its head {} passed its own Yulius CI run, but no merge is armed: {}. The queue does \
         the rest; never merge by hand.",
        short(head),
        yulius_domain::review::KEEP_APPROVAL
    )
}

/// A short, stable key for a text.
fn fingerprint(text: &str) -> String {
    // FNV-1a: stable across builds, unlike the std hasher.
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}

/// What the watch told each task, kept beside [`PR_NOTICES_FILE`] so that a question about one
/// task reads no file.
pub(crate) type ToldCache = std::sync::Mutex<Option<BTreeMap<String, Told>>>;

/// How long (ms) news the watch typed into an idle session counts as due while the turn it
/// starts has not begun: a paste Claude Code swallowed must not hold a chain for ever
/// (SPEC-487 CHN-002).
pub const NEWS_TURN_WAIT_MS: i64 = 2 * 60 * 1000;

/// Whether the watch still has to tell of pull request `pr`'s merge: it tells a merge only to a
/// task it saw with that pull request open, once (PRN-005, PRN-007).
fn merge_untold(told: &Told, pr: u64) -> bool {
    told.pr == pr && !told.keys.contains_key("merged")
}

/// The key under which the watch tells how an armed merge of `head` ended (`text`).
fn outcome_key(head: &str, text: &str) -> String {
    format!("{}:{}", short(head), fingerprint(text))
}

/// The key under which the watch tells an actionable failed publication of `head` (`text`)
/// (SPEC-553 REP-004).
fn publish_key(head: &str, text: &str) -> String {
    format!("{}:{}", short(head), fingerprint(text))
}

/// News the watch typed into a session whose turn has not begun (SPEC-487 CHN-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypedNews {
    /// When it was typed (ms).
    pub typed_ms: i64,
    /// Claude Code's queue handed something on, or was about to, at the typing or since: the
    /// news was typed while a turn ran or was coming, and waits in that queue for its own.
    pub in_queue: bool,
}

/// Since when a session is idle, for the bound of queued news (ms): its last clean Stop, its
/// interrupt, or the record that closed a turn whose Stop was never answered (`ended_ms`,
/// `TaskRuntime::last_stop_ms`: a Stop that Yulius's gate blocked is none, the turn goes on),
/// while the task reads idle and no turn began after it. Otherwise 0,
/// not known: while the task works; after a turn whose prompt came after that Stop and that
/// ended without one (an error, also after a blocked Stop), whose end is later than `ended_ms`
/// says (reviews of SPEC-487 slice 1, D-1, and slice 2, F13); and for a run without hooks.
///
/// Not covered: a session that went on from a clean Stop without a prompt (the goal's verdict
/// "not met") and that an error then ended still answers that earlier Stop, so for the
/// seconds until Claude Code hands a queued entry on, the bound may read as over.
fn idle_since(idle: bool, began_ms: i64, ended_ms: i64) -> i64 {
    if idle && ended_ms >= began_ms {
        ended_ms
    } else {
        0
    }
}

/// What [`App::pr_news_due`] reads about one running task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NewsFacts {
    /// A merge is armed or being decided.
    pub armed: bool,
    /// A person paused it: its process is frozen, so what is typed into it waits with it
    /// until they resume it, which may be never. (An update's hold and a pause for capacity
    /// end by themselves: news stays due.)
    pub paused: bool,
    /// When the watch typed news whose turn has not begun (ms, 0 = none).
    pub typed_ms: i64,
    /// That news was typed while a turn ran, and was read in Claude Code's queue since: the
    /// queue hands it on after that turn, and says whether it still waits.
    pub in_queue: bool,
    /// Claude Code's queue holds something a turn's end hands on, or handed it on to a turn
    /// not seen yet (SPEC-471, SPEC-479).
    pub prompt_waits: bool,
    /// Since when the session is idle ([`idle_since`], ms), or 0 when that is not known: no
    /// bound applies then, and the queue alone decides.
    pub idle_since_ms: i64,
    /// Now (ms).
    pub now_ms: i64,
    /// Its pull request and whether it is merged, when it has one.
    pub pull: Option<(u64, bool)>,
    /// How its last armed merge ended (its head and the line for the agent), when it has one.
    pub outcome: Option<(String, String)>,
    /// An actionable failed publication on an open pull request (head and text) not yet told
    /// (SPEC-553 REP-004).
    pub publish_problem: Option<(String, String)>,
    /// Its sandbox is a playground: the watch tells its tasks nothing.
    pub playground: bool,
}

impl NewsFacts {
    /// News was typed and its turn has not begun. News in Claude Code's queue (typed while a
    /// turn ran) waits as long as the queue says a prompt waits for its turn, however long
    /// the running turn takes and whichever entry gets its turn first; once the session is
    /// idle, for [`NEWS_TURN_WAIT_MS`] from then (a hand-over whose turn never shows must not
    /// hold a chain for ever). Other news (typed into an idle session, or queued and not read
    /// in the queue yet) waits [`NEWS_TURN_WAIT_MS`] from the typing; its mark is taken away
    /// when the task starts to work.
    fn typed_pending(&self) -> bool {
        if self.typed_ms == 0 {
            return false;
        }
        let since = |from_ms: i64| self.now_ms.saturating_sub(from_ms) < NEWS_TURN_WAIT_MS;
        if self.in_queue {
            return self.prompt_waits && (self.idle_since_ms == 0 || since(self.idle_since_ms));
        }
        since(self.typed_ms)
    }
}

/// Whether Yulius still has news of a running task's pull request for its agent (SPEC-487
/// CHN-001, CHN-002): a merge armed or being decided, whatever else holds (Yulius merges a
/// paused task's pull request too); otherwise, for a task the watch types into: news typed
/// whose turn has not begun, a merge the watch saw open and has not told, or an armed merge
/// that ended without merging and was not explained yet.
pub(crate) fn news_due(facts: &NewsFacts, told: &Told) -> bool {
    if facts.armed {
        return true;
    }
    if facts.paused || facts.playground {
        return false;
    }
    if facts.typed_pending() {
        return true;
    }
    if let (Some((_, false)), Some((head, text))) = (facts.pull, &facts.publish_problem)
        && told.keys.get("publish") != Some(&publish_key(head, text))
    {
        return true;
    }
    match (facts.pull, &facts.outcome) {
        (Some((pr, true)), _) => merge_untold(told, pr),
        (Some((_, false)), Some((head, text))) => {
            told.keys.get("outcome") != Some(&outcome_key(head, text))
        }
        _ => false,
    }
}

/// The saved notices ([`PR_NOTICES_FILE`]: what the watch told each task); none when the file
/// is missing or unreadable.
fn read_notices(path: &std::path::Path) -> BTreeMap<String, Told> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// A local capacity check can reuse a recent hosted observation of the exact
/// decision. Starting another read invalidates it, including if that read fails.
#[derive(Default)]
pub(crate) struct CandidateObservation {
    generation: u64,
    obsolete: Option<(
        crate::stages::SpanRecord,
        Option<String>,
        std::time::Instant,
    )>,
    /// What the views show (SPEC-621 FIN-004): the watch's last answered observation that the
    /// decision no longer applies, kept while a re-read is in flight or fails, without expiry.
    shown: Option<(crate::stages::SpanRecord, Option<String>)>,
}

/// A newer live outcome or a retired queue decision supersedes the saved failure.
pub(crate) fn candidate_from_wait(
    wait: Option<crate::stages::SpanRecord>,
    outcome: &mut Option<(String, String)>,
) -> AppResult<
    Option<(
        crate::stages::SpanRecord,
        crate::merge_train::SingletonDecision,
    )>,
> {
    let Some(span) = wait else {
        return Ok(None);
    };
    let Some(decision) = crate::merge_train::SingletonDecision::from_span(&span)? else {
        return Ok(None);
    };
    if outcome
        .as_ref()
        .is_some_and(|(head, text)| head != &decision.head || text != &decision.message)
    {
        return Ok(None);
    }
    *outcome = None;
    Ok(decision.superseded_at.is_none().then_some((span, decision)))
}

impl App {
    /// Runs [`Self::watch_prs`] every [`WATCH_EVERY`].
    pub fn start_pr_watch(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let bounds = WatchBounds::default();
            let mut tick = tokio::time::interval(WATCH_EVERY);
            loop {
                tick.tick().await;
                app.watch_prs(&bounds).await;
            }
        });
    }

    /// Why `task`'s pull request is stalled (PRN-009), while it is.
    #[must_use]
    pub fn pr_stalled(&self, task: &TaskId) -> Option<String> {
        self.existing_runtime(task).and_then(|runtime| {
            runtime
                .pr_stalled
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
    }

    /// One pass over the live tasks with an open pull request: types into each session what it
    /// was not told yet; returns the tasks and what was typed.
    ///
    /// Heap-boxed so callers awaiting multiple passes in one async frame stay well within the
    /// default 2 MiB thread stack (SPEC-580 merge review F1).
    pub async fn watch_prs(self: &Arc<Self>, bounds: &WatchBounds) -> Vec<(TaskId, String)> {
        Box::pin(self.watch_prs_pass(bounds)).await
    }

    async fn watch_prs_pass(self: &Arc<Self>, bounds: &WatchBounds) -> Vec<(TaskId, String)> {
        let path = self.config.data_dir.join(PR_NOTICES_FILE);
        let saved = self.with_pr_notices(Clone::clone);
        let attempts = match self.deps.store.live_attempts().await {
            Ok(attempts) => attempts,
            Err(err) => {
                tracing::warn!(error = %err, "the tasks whose pull requests to watch could not be read");
                return Vec::new();
            }
        };
        let update_holds = self.update_holds_starts();
        let mut runs: BTreeMap<String, Vec<CiRunRecord>> = BTreeMap::new();
        let mut kept: BTreeMap<String, Told> = BTreeMap::new();
        let mut typed = Vec::new();
        for attempt in attempts {
            let Ok(Some(task)) = self.deps.store.task(&attempt.task_id).await else {
                continue;
            };
            let mut told = saved.get(task.id.as_str()).cloned().unwrap_or_default();
            let Some(runtime) = self.existing_runtime(&task.id) else {
                continue;
            };
            // News typed whose turn is not waited for any longer (its bound ran out, or Claude
            // Code's queue handed it on) is due no longer: the list says so now, not at the
            // task's next event (SPEC-487 CHN-002).
            let stale = self.pr_news_typed(&task);
            if stale.typed_ms > 0 && !stale.typed_pending() {
                *runtime
                    .pr_news
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                self.publish_task(&task.id).await;
                // What waited for this task's news waits no longer: its turn's own end may
                // have looked while the news was still due.
                let (app, id) = (Arc::clone(self), task.id.clone());
                tokio::spawn(async move { app.advance_chains(&id).await });
            }
            let held =
                update_holds || !matches!(self.deps.store.open_pause(&task.id).await, Ok(None));
            let seen = self.armed_snapshot(&task.id);
            let facts = if task.status == TaskStatus::Running && !held {
                self.settle_codex_input_hold(&task, &attempt.id, None).await;
                Box::pin(self.pr_facts(&task, &mut told, &mut runs, &seen)).await
            } else {
                None
            };
            let Some((facts, source)) = facts else {
                // If an earlier armed-PR stall Needs You was opened and the task is no longer
                // watching an open PR (e.g. merged or closed), auto-clear it (SPEC-580 AWM-004).
                if task.status == TaskStatus::Running && !held && told.armed_needs_you_at > 0 {
                    self.end_needs_you_asked_at(&task.id, told.armed_needs_you_at, None)
                        .await;
                    told.clear_armed_stall();
                }
                // Not watched now (no open pull request, paused, not running): no stall shows
                // (review minor 3), and what it was told stays while its pull request may come
                // back.
                let cleared = runtime
                    .pr_stalled
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .is_some();
                if cleared {
                    self.publish_task(&task.id).await;
                }
                if told.pr > 0 {
                    kept.insert(task.id.as_str().to_owned(), told);
                }
                continue;
            };
            let has_new_event = facts
                .events
                .iter()
                .any(|event| told.keys.get(event.kind) != Some(&event.key));
            let (text, mut next) = decide(&facts, &told, bounds);
            let armed_action = decide_armed_stall(&facts, &mut next, bounds, task.number);
            let (to_type, is_armed_check_in) = if has_new_event {
                (text, false)
            } else {
                match &armed_action {
                    ArmedStallAction::CheckIn(armed_text) => {
                        // Prefer the actionable armed-PR check-in over a generic open-PR check-in
                        // when both become due on the same pass.
                        next.stalled.clone_from(&told.stalled);
                        (Some(armed_text.clone()), true)
                    }
                    ArmedStallAction::Escalate { .. } => {
                        // Escalating to Needs You: do not also type a generic check-in/stall.
                        next.typed_ms = told.typed_ms;
                        next.stalled.clone_from(&told.stalled);
                        (None, false)
                    }
                    ArmedStallAction::None => (text, false),
                }
            };
            let mut next = match to_type {
                None => next,
                Some(text) => {
                    // Job names, reasons and test names come from the project: no control
                    // character may end the paste early (review minor 4).
                    let prepared = text;
                    let text = crate::task_message::clean_message(&prepared);
                    let paste = format!("\x1b[200~{text}\x1b[201~");
                    // From here the task goes on by itself until the turn this starts has
                    // begun (SPEC-487 CHN-002). Stamped before it is typed, as its prompt hook
                    // may be served before the typing returns; when nothing was typed, what
                    // was there before is put back whole. When Claude Code's queue is handing
                    // something on, a turn runs or is coming, and the news waits behind it.
                    let mut before = None;
                    let mut stamp = || {
                        let pending = TypedNews {
                            typed_ms: self.now().0,
                            in_queue: runtime.queue().hands_on(),
                        };
                        before = Some(
                            runtime
                                .pr_news
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .replace(pending),
                        );
                    };

                    let result = if let Some(source) = &source {
                        let admit = async {
                            let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await
                            else {
                                return false;
                            };
                            let Some(recipient) = self
                                .armed_notice_recipient(&task, &sandbox, facts.pr, source)
                                .await
                            else {
                                return false;
                            };
                            // A changed free-slot note also changes this observation: only an
                            // arm, clear or completed queue decision publishes that note.
                            if self.update_holds_starts()
                                || !seen.same_arm(&self.armed_snapshot(&task.id))
                                || self.watched_train(&task, facts.pr) != source.train
                                || source.decision.is_some() && self.merge_is_armed(&task.id)
                            {
                                return false;
                            }
                            let mut current = facts.clone();
                            self.refresh_notice_activity(&task.id, &runtime, &mut current);
                            if recipient.working {
                                current.working = true;
                                current.idle = false;
                            }
                            if is_armed_check_in {
                                let mut check_told = told.clone();
                                if !matches!(
                                    decide_armed_stall(
                                        &current,
                                        &mut check_told,
                                        bounds,
                                        task.number,
                                    ),
                                    ArmedStallAction::CheckIn(ref msg) if msg == &prepared
                                ) {
                                    return false;
                                }
                            } else if decide(&current, &told, bounds).0.as_deref()
                                != Some(prepared.as_str())
                            {
                                return false;
                            }
                            stamp();
                            true
                        };
                        self.type_into_session_written(
                            &task,
                            paste.as_bytes(),
                            Typed::Notice(&attempt.id),
                            Sender::Yulius,
                            Some(admit),
                        )
                        .await
                    } else {
                        stamp();
                        self.type_into_session_written(
                            &task,
                            paste.as_bytes(),
                            Typed::Notice(&attempt.id),
                            Sender::Yulius,
                            None::<std::future::Ready<bool>>,
                        )
                        .await
                    };
                    // Only the writer's refusal counts towards raising the notice: a person's
                    // hold before the write is SPEC-480's to report (review round 1).
                    let (result, write_refused) = match result {
                        Ok(written) => {
                            let refused = written.is_err();
                            (written, refused)
                        }
                        Err(held) => (Err(held), false),
                    };
                    let put_back = || {
                        if let Some(previous) = before {
                            *runtime
                                .pr_news
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = previous;
                        }
                    };
                    match result {
                        Ok(()) => {
                            next.deferred_keys.clear();
                            tracing::info!(task = %task.id, pr = facts.pr, "told the task about its pull request");
                            if is_armed_check_in {
                                next.armed_check_in_ms = facts.now_ms;
                                next.typed_ms = facts.now_ms;
                            }
                            typed.push((task.id.clone(), text));
                            next
                        }
                        // Not ready for input, or a person is typing or may have a draft in
                        // the prompt box (SPEC-480): the next pass tries again. The pull request
                        // it saw open is kept, so its merge is still told.
                        Err(err) => {
                            put_back();
                            if matches!(err, AppError::Conflict(_)) {
                                tracing::debug!(task = %task.id, error = %err, "a pull request notice waits");
                            } else {
                                tracing::warn!(task = %task.id, error = %err, "could not tell the task about its pull request");
                            }
                            let mut pending = untold(told, &next);
                            if write_refused
                                && has_new_event
                                && task.harness == yulius_domain::harness::Harness::Codex
                            {
                                if pending.deferred_keys == next.keys {
                                    let action = "Inspect the terminal and resolve the reported delivery problem. Clear this request so Yulius can retry the pending pull request notice.";
                                    if let Err(record_err) = self
                                        .ask_for_blocked_input(
                                            &task,
                                            &attempt.id,
                                            &text,
                                            &err.public_message(),
                                            action,
                                        )
                                        .await
                                    {
                                        tracing::warn!(task = %task.id, error = %record_err, "could not raise undelivered PR news");
                                    }
                                    // Raised: once the request is cleared, the next refusal
                                    // counts afresh instead of raising it again at once.
                                    pending.deferred_keys.clear();
                                } else {
                                    pending.deferred_keys.clone_from(&next.keys);
                                }
                            }
                            pending
                        }
                    }
                }
            };
            if !has_new_event
                && let ArmedStallAction::Escalate {
                    happened,
                    problem,
                    action,
                } = armed_action
                && !self.update_holds_starts()
                && seen.same_arm(&self.armed_snapshot(&task.id))
                && source
                    .as_ref()
                    .is_none_or(|s| self.watched_train(&task, facts.pr) == s.train)
                && !Box::pin(self.hosted_arm_stale_or_moved(&task, facts.pr, &facts.armed_head))
                    .await
                && let Some(at) = Box::pin(
                    self.escalate_armed_stall(&task, facts.pr, &happened, &problem, &action),
                )
                .await
            {
                next.armed_needs_you_at = at;
            }
            // A merged pull request is no stall, also before its notice could be typed.
            let merged = facts.events.iter().any(|event| event.kind == "merged");
            let stalled = next
                .stalled
                .clone()
                .filter(|_| !merged && !next.keys.contains_key("merged"));
            let changed = {
                let mut shown = runtime
                    .pr_stalled
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let changed = *shown != stalled;
                *shown = stalled;
                changed
            };
            if changed {
                self.publish_task(&task.id).await;
            }
            kept.insert(task.id.as_str().to_owned(), next);
        }
        // A task not live this pass (a session switching, a capacity pause) keeps what it was
        // told while it is not resting: its next run is not told it all again.
        for (id, told) in &saved {
            if kept.contains_key(id) {
                continue;
            }
            let Ok(task_id) = TaskId::parse(id) else {
                continue;
            };
            if matches!(self.deps.store.task(&task_id).await, Ok(Some(task)) if !task.status.is_resting())
            {
                kept.insert(id.clone(), told.clone());
            }
        }
        if kept != saved {
            self.save_notices(&path, &kept);
        }
        typed
    }

    /// What the watch told each task, as saved: read from the file once per server (at its
    /// first pass or the first question about a task), then kept with every save.
    fn with_pr_notices<T>(&self, read: impl FnOnce(&BTreeMap<String, Told>) -> T) -> T {
        let mut cache = self
            .pr_told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read(cache.get_or_insert_with(|| {
            let _guard = NOTICES_FILE_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            read_notices(&self.config.data_dir.join(PR_NOTICES_FILE))
        }))
    }

    /// What is known of news the watch typed into `task`'s session whose turn has not begun
    /// (the typed fields of [`NewsFacts`]; the rest is its default).
    fn pr_news_typed(&self, task: &TaskRecord) -> NewsFacts {
        let idle = task
            .activity
            .is_some_and(yulius_domain::task::Activity::is_idle);
        let Some(runtime) = self.existing_runtime(&task.id) else {
            return NewsFacts::default();
        };
        let Some(news) = *runtime
            .pr_news
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        else {
            return NewsFacts::default();
        };
        NewsFacts {
            typed_ms: news.typed_ms,
            in_queue: news.in_queue,
            // Asked only for news that is known to be queued.
            prompt_waits: news.in_queue && runtime.queue_may_hand_on(),
            idle_since_ms: idle_since(
                idle,
                runtime.turn_began_ms.load(Ordering::SeqCst),
                runtime.last_stop_ms.load(Ordering::SeqCst),
            ),
            now_ms: self.now().0,
            ..NewsFacts::default()
        }
    }

    /// A turn of `task`'s session that began at `began_ms` (unknown: `None`) made the task
    /// Working. When it began after the watch typed into an idle session, it is the turn of
    /// that news, whose mark goes (SPEC-487 CHN-002). A turn that began before is another
    /// one: the transcript's watcher may read its prompt late, and a prompt hook sent before
    /// the typing may be answered after it. News in Claude Code's queue is not this turn's to
    /// end either: another entry may get its turn first (SPEC-479), so the queue decides
    /// ([`NewsFacts::typed_pending`]) and the watch's pass takes the spent mark away.
    pub(crate) fn pr_news_turn_began(&self, task: &TaskId, began_ms: Option<i64>) {
        if let Some(runtime) = self.existing_runtime(task) {
            let mut news = runtime
                .pr_news
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if news.is_some_and(|news| {
                !news.in_queue && began_ms.is_none_or(|began| began >= news.typed_ms)
            }) {
                *news = None;
            }
        }
    }

    /// Whether Yulius still has news of `task`'s pull request for its agent ([`news_due`];
    /// SPEC-487 CHN-001, CHN-002). Its agent goes on by itself once told, so the task is
    /// neither Done nor a person's turn until then.
    pub(crate) async fn pr_news_due(&self, task: &TaskRecord) -> bool {
        if task.status != TaskStatus::Running {
            return false;
        }
        crate::race::reached(
            crate::race::Point::LiveRowPrNews,
            std::path::Path::new(task.id.as_str()),
        );
        let record = self.deps.store.delivery(&task.id).await.ok().flatten();
        self.pr_news_due_of(task, record.as_ref(), false).await
    }

    /// Whether the watch already pasted run `run`'s failure of pull request `pr` into `task`'s
    /// session (SPEC-578).
    pub(crate) fn run_failure_told(&self, task: &TaskId, pr: u64, run: &str) -> bool {
        self.with_pr_notices(|told| {
            told.get(task.as_str())
                .is_some_and(|t| t.pr == pr && t.keys.get("run").map(String::as_str) == Some(run))
        })
    }

    /// Newest Yulius CI run for pull request `pr` in `sandbox`, if any (SPEC-578).
    pub(crate) async fn latest_pr_ci_run(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        pr: u64,
    ) -> Option<CiRunRecord> {
        let runs = self.deps.store.ci_runs(sandbox, RUNS_READ).await.ok()?;
        runs.into_iter()
            .find(|run| run.reference == CiRef::PullRequest(pr))
    }

    /// [`Self::pr_news_due`] for a decision that already read the task's delivery `record`, so
    /// that one decision rests on one reading of it (review round 2); `merged_elsewhere` when
    /// GitHub says its pull request is merged and the record does not say so yet.
    pub(crate) async fn pr_news_due_of(
        &self,
        task: &TaskRecord,
        record: Option<&crate::delivery::DeliveryRecord>,
        merged_elsewhere: bool,
    ) -> bool {
        if task.status != TaskStatus::Running
            || !self.accepts_machine_notice(&task.id).await.unwrap_or(false)
        {
            return false;
        }
        if self.merge_is_armed(&task.id) {
            return true;
        }
        let store = &self.deps.store;
        let typed = self.pr_news_typed(task);
        let pull = record.and_then(|record| {
            let merged = merged_elsewhere || past_merge(&record.stage);
            (merged || record.stage == DeliveryStage::PrOpen)
                .then_some(record.pr_number)
                .flatten()
                .map(|pr| (pr, merged))
        });
        let outcome = self.armed_view(&task.id).1;
        let publish_problem = if !self.agent_waits_on_yulius(&task.id) {
            self.current_actionable_push_problem(task, None).await
        } else {
            None
        };
        // Asked only where it decides: an armed merge or failed publication on an open PR.
        let playground = matches!(pull, Some((_, false)))
            && (outcome.is_some() || publish_problem.is_some())
            && matches!(
                store.sandbox(&task.sandbox_id).await,
                Ok(Some(sandbox)) if sandbox.is_playground()
            );
        let paused = matches!(store.person_pause(&task.id).await, Ok(Some(_)));
        let facts = NewsFacts {
            armed: false,
            // A pause that cannot be read counts as none: the safe side for Done.
            paused,
            pull,
            outcome,
            publish_problem,
            playground,
            ..typed
        };
        let due = self.with_pr_notices(|told| match told.get(task.id.as_str()) {
            Some(told) => news_due(&facts, told),
            None => news_due(&facts, &Told::default()),
        });
        if due {
            return true;
        }
        // An untold failed/errored Yulius CI run on a live, unpaused, non-playground session
        // with no open human request will be pasted into the agent by `watch_prs` so the agent
        // can recover it without escalating to Needs You (SPEC-578). Check `store.live_attempt`
        // (what `watch_prs` itself iterates) rather than `existing_runtime`, because `armed_view`
        // above lazily allocates a `TaskRuntime`.
        if !paused
            && matches!(store.live_attempt(&task.id).await, Ok(Some(_)))
            && let Some((pr, false)) = pull
            && task.activity != Some(yulius_domain::task::Activity::NeedsAttention)
            && matches!(store.task_needs_you(&task.id).await, Ok(None))
            && self
                .browser_tab_request_for(&task.sandbox_id, &task.id)
                .await
                .is_none()
            && let Some(run) = self.latest_pr_ci_run(&task.sandbox_id, pr).await
            && matches!(run.state, RunState::Failed | RunState::Error)
            && !(run.state == RunState::Failed
                && self.run_superseded_by_pushed_head(task, &run.sha).await)
            && !matches!(
                store.sandbox(&task.sandbox_id).await,
                Ok(Some(sandbox)) if sandbox.is_playground()
            )
        {
            return !self.run_failure_told(&task.id, pr, &run.id);
        }
        false
    }

    /// Writes what each task was told (whole, then renamed into place); failures are logged.
    fn save_notices(&self, path: &std::path::Path, notices: &BTreeMap<String, Told>) {
        *self
            .pr_told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notices.clone());
        let _guard = NOTICES_FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let written = serde_json::to_vec(notices)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, path)
            });
        if let Err(err) = written {
            tracing::warn!(error = %err, "pull request notices could not be saved");
        }
    }

    /// What the watch knows of `task`'s pull request, or `None` when it has no open one (nor
    /// one the watch saw open that just merged). `runs` caches each sandbox's runs for the pass.
    async fn pr_facts(
        &self,
        task: &TaskRecord,
        told: &mut Told,
        runs: &mut BTreeMap<String, Vec<CiRunRecord>>,
        seen: &ArmedSnapshot,
    ) -> Option<(PrFacts, Option<QueueNoticeSource>)> {
        // The slot before the delivery: a merge decided in between then reads as merged, never
        // as "no merge armed" on an open pull request (review minor 8).
        let (armed, mut outcome) = self.armed_view(&task.id);
        // A saved arm may still await restoration (SPEC-487, review round 2).
        let armed = armed || self.merge_is_armed(&task.id);
        let record = self.deps.store.delivery(&task.id).await.ok()??;
        let pr = record.pr_number?;
        let sandbox = self.deps.store.sandbox(&task.sandbox_id).await.ok()??;
        if sandbox.is_playground() {
            return None;
        }
        let now_ms = self.now().0;
        if past_merge(&record.stage) {
            if told.armed_needs_you_at > 0 {
                self.end_needs_you_asked_at(&task.id, told.armed_needs_you_at, None)
                    .await;
                told.clear_armed_stall();
            }
            // Merged: said once, to a task the watch saw with it open (PRN-005, PRN-007).
            if !merge_untold(told, pr) || !self.accepts_machine_notice(&task.id).await.ok()? {
                return None;
            }
            let needs_you = !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None));
            let browser_req = self
                .browser_tab_request_for(&task.sandbox_id, &task.id)
                .await
                .is_some();
            let flagged = task.activity == Some(yulius_domain::task::Activity::NeedsAttention);
            let person_must_act = needs_you
                || browser_req
                || (flagged && !self.held_message_flag_outlived(task).await);
            let text = self.merged_text_once(task, &sandbox, pr).await?;
            return Some((
                PrFacts {
                    now_ms,
                    pr,
                    events: vec![Event {
                        kind: "merged",
                        key: pr.to_string(),
                        text,
                    }],
                    progress: told.progress.clone(),
                    person_must_act,
                    ..PrFacts::default()
                },
                None,
            ));
        }
        if record.stage != DeliveryStage::PrOpen {
            return None;
        }
        // A person must act first (a prompt on screen, `needs_you`): nothing is typed, the merge
        // notice included (review M1, round 2).
        let mut needs_you = !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None));
        let browser_req = self
            .browser_tab_request_for(&task.sandbox_id, &task.id)
            .await
            .is_some();
        let flagged = task.activity == Some(yulius_domain::task::Activity::NeedsAttention);
        let flagged_active = flagged && !self.held_message_flag_outlived(task).await;
        let mut person_must_act = needs_you || browser_req || flagged_active;
        // A deferred pass remembers the open PR and leaves its existing stall clock alone.
        let prior_progress = if told.pr == pr {
            told.progress.clone()
        } else {
            String::new()
        };
        let observed = || PrFacts {
            pr,
            now_ms,
            progress: prior_progress.clone(),
            ..PrFacts::default()
        };
        let observed_only = || {
            Some((
                PrFacts {
                    deferred: true,
                    ..observed()
                },
                None,
            ))
        };
        if seen.observation.deciding {
            return observed_only();
        }
        let arm_head = match &seen.now {
            ArmedNow::Armed(arm) => Some(arm.head.as_str()),
            _ if armed => return observed_only(),
            _ => None,
        };
        let integration = delivery_profile(&sandbox).integration_branch;
        // A dropped singleton belongs to the task whose wait ended, even when CI reused
        // a different train's result. Verify the hosted inputs before choosing ordinary CI.
        let mut receipt = None;
        if !armed {
            // Disarmed PR leaves armed-green waiting immediately: reset all stall-clock fields
            // (and clear any earlier armed-stall Needs You) before any deferred `observed_only()`
            // return so re-arming the same head always starts a fresh bound (SPEC-580 merge F2).
            let armed_needs_you_at = told.armed_needs_you_at;
            told.clear_armed_stall();
            if armed_needs_you_at > 0 {
                self.end_needs_you_asked_at(&task.id, armed_needs_you_at, None)
                    .await;
                needs_you = !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None));
                person_must_act = needs_you || browser_req || flagged_active;
            }
            let Ok(latest) = self
                .deps
                .store
                .latest_merge_wait(&task.id)
                .await
                .and_then(|wait| candidate_from_wait(wait, &mut outcome))
            else {
                return observed_only();
            };
            if let Some((span, decision)) = latest
                && decision.singleton.pr == pr
            {
                if person_must_act {
                    return observed_only();
                }
                let Ok(pull) = self
                    .observe_candidate_pull(task, &sandbox, &span, &decision)
                    .await
                else {
                    return observed_only();
                };
                if !self.notice_pull_owned(task, &sandbox, pr, &pull) || pull.base_sha.is_empty() {
                    return observed_only();
                }
                receipt = Some((span, decision, pull));
            }
        }
        let verified_head =
            arm_head.or_else(|| receipt.as_ref().map(|(_, _, pull)| pull.head_sha.as_str()));
        let mut own = if let Some(head) = verified_head {
            // Every armed owner follows the queue's ordinary run at that head, including
            // a run shared by the scheduler or one outside the recent sandbox window.
            match self.ordinary_pull_run(&sandbox, pr, head).await {
                Ok(run) => run,
                Err(_) => return observed_only(),
            }
        } else {
            if !runs.contains_key(sandbox.id.as_str()) {
                let read = self
                    .deps
                    .store
                    .ci_runs(&sandbox.id, RUNS_READ)
                    .await
                    .unwrap_or_default();
                runs.insert(sandbox.id.as_str().to_owned(), read);
            }
            let sandbox_runs = runs.get(sandbox.id.as_str()).map_or(&[][..], Vec::as_slice);
            let mut recent = sandbox_runs
                .iter()
                .find(|run| run.reference == CiRef::PullRequest(pr))
                .cloned();
            if recent.is_none() && !told.head.is_empty() {
                recent = self
                    .deps
                    .store
                    .ci_runs_for_sha(&sandbox.id, &told.head)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find(|run| run.reference == CiRef::PullRequest(pr));
            }
            recent
        };
        // SPEC-553 REP-004: recover an idle task whose clean committed CI repair on a failing
        // open PR missed turn-end publication (for example across a server restart). Reads only
        // local HEAD unless it differs from the failing PR run's SHA.
        if !armed
            && !needs_you
            && !flagged
            && let Some(failed_sha) = own
                .as_ref()
                .filter(|run| run.state == RunState::Failed)
                .map(|run| run.sha.clone())
            && self.maybe_publish_repair_for_watch(task, &failed_sha).await
        {
            if receipt.is_some() {
                // The watcher just pushed a repair. Read the new hosted head on the next pass.
                return observed_only();
            }
            let read = self
                .deps
                .store
                .ci_runs(&sandbox.id, RUNS_READ)
                .await
                .unwrap_or_default();
            runs.insert(sandbox.id.as_str().to_owned(), read);
            let refreshed = runs.get(sandbox.id.as_str()).map_or(&[][..], Vec::as_slice);
            own = refreshed
                .iter()
                .find(|run| run.reference == CiRef::PullRequest(pr))
                .cloned();
        }
        let train_read = self.watched_train(task, pr);
        let train = train_read.as_ref().ok().and_then(Option::as_ref);
        let holding = train.and_then(|train| train.candidate.as_ref());
        // A narrowed train may reuse its larger predecessor's run. Its verification
        // notice is a separate event; larger candidates retain their persisted key.
        let candidate_kind = if holding.is_some_and(|(_, members)| members.len() == 1) {
            "singleton"
        } else {
            "candidate"
        };
        // An ordinary failure is actionable even when the holding CI record cannot be decoded.
        let ordinary_takes_precedence = own.as_ref().is_some_and(|run| {
            run.state == RunState::Failed
                || (run.state == RunState::Error && told.keys.get("run") != Some(&run.id))
        });
        let candidate_id = holding
            .filter(|_| !ordinary_takes_precedence)
            .map(|(id, _)| id.as_str());
        let Ok(candidate) = self.holding_run(&sandbox, candidate_id).await else {
            return observed_only();
        };
        let candidate = candidate.as_ref();
        if let Some(run) = own.as_ref()
            && run.state == RunState::Failed
            && self.run_superseded_by_pushed_head(task, &run.sha).await
        {
            if arm_head.is_some() {
                return observed_only();
            }
            own = None;
        }
        let own = own.as_ref();
        let notice_hold = if holding.is_some() {
            Hold::Held
        } else {
            Hold::Free
        };
        let mut events = Vec::new();
        let told_key = |kind: &str, key: &str| told.keys.get(kind).is_some_and(|k| k == key);
        if let Some(run) = own {
            match run.state {
                RunState::Failed if !told_key("run", &run.id) => {
                    let jobs = self
                        .deps
                        .store
                        .ci_jobs(&run.id, true)
                        .await
                        .unwrap_or_default();
                    events.push(Event {
                        kind: "run",
                        key: run.id.clone(),
                        text: red_run_text(run, &jobs),
                    });
                }
                RunState::Error if !told_key("run", &run.id) => events.push(Event {
                    kind: "run",
                    key: run.id.clone(),
                    text: errored_run_text(run),
                }),
                _ => {}
            }
        }
        if train_read.is_err() && events.is_empty() {
            return observed_only();
        }
        // Dissolving or already reported trains have no new instructions for this member.
        if events.is_empty()
            && holding.is_some_and(|_| match candidate.map(|run| run.state) {
                Some(RunState::Queued | RunState::Running | RunState::Passed) => false,
                Some(RunState::Failed) => {
                    candidate.is_some_and(|run| told_key(candidate_kind, &run.id))
                }
                Some(RunState::Error | RunState::Cancelled | RunState::Skipped) | None => true,
            })
        {
            return observed_only();
        }
        if let (Some(run), Some((_, members))) = (candidate, holding)
            && run.state == RunState::Failed
            && !own.is_some_and(|run| run.state == RunState::Failed)
            && !told_key(candidate_kind, &run.id)
        {
            let text = if let [pr] = members.as_slice() {
                format!(
                    "Merge candidate #{pr} (run {}) failed. Yulius is verifying this pull request's head and checks before reporting the failure. Do not re-arm, merge `{integration}` or push to this branch meanwhile.",
                    run.id
                )
            } else {
                let jobs = self
                    .deps
                    .store
                    .ci_jobs(&run.id, true)
                    .await
                    .unwrap_or_default();
                red_candidate_text(run, members, &jobs, &integration)
            };
            events.push(Event {
                kind: candidate_kind,
                key: run.id.clone(),
                text,
            });
        }
        if !armed && let Some((head, text)) = outcome {
            let key = outcome_key(&head, &text);
            if !told_key("outcome", &key) {
                events.push(Event {
                    kind: "outcome",
                    key,
                    text,
                });
            }
        }
        let head = own.map(|run| run.sha.clone());
        // Without a run at its head (a project without Yulius CI) the branch's pushed head is
        // asked: a conflicting pull request is not tested (SPEC-491 DON-003), so its agent has
        // to hear of it there too.
        let conflict_head = match verified_head.or(head.as_deref()) {
            Some(head) => Some(head.to_owned()),
            None => self.branch_head(task).await,
        };
        // SPEC-553 REP-004: deliver an actionable failed publication once when `yulius wait` is
        // not already actively waking the agent on it and the failed head is still current.
        if !armed
            && !self.agent_waits_on_yulius(&task.id)
            && let Some((note_head, text)) = self
                .current_actionable_push_problem(task, conflict_head.as_deref())
                .await
        {
            let key = publish_key(&note_head, &text);
            if !told_key("publish", &key) {
                events.push(Event {
                    kind: "publish",
                    key,
                    text,
                });
            }
        }
        if let Some(head) = &conflict_head
            && matches!(train_read, Ok(None))
            && self.conflicts(task, &sandbox, head).await
        {
            events.push(Event {
                kind: "conflict",
                key: head.clone(),
                text: conflict_text(head, &integration),
            });
        }
        let green =
            own.is_some_and(|run| matches!(run.state, RunState::Passed | RunState::Skipped));
        let arm_note = self.armed_snapshot(&task.id).note;
        let queue_note = train.map(|train| &train.note).or(arm_note.as_ref());
        if let (true, Some(head)) = (green, &head)
            && candidate.is_none_or(|run| run.state != RunState::Failed)
        {
            let said = green_said(head, armed, notice_hold, queue_note, &integration);
            if let Some((class, text)) = said {
                let key = if train.is_some() && holding.is_none() {
                    format!("{head}:left-out:{}", fingerprint(&text))
                } else {
                    format!("{head}:{class}")
                };
                events.push(Event {
                    kind: "green",
                    key,
                    text,
                });
            }
        }
        let ci_busy = own.is_some_and(|run| run.state.is_active())
            || candidate.is_some_and(|run| run.state.is_active());
        let progress = format!(
            "{}|{}|{}|{}",
            own.map_or_else(String::new, |run| format!(
                "{}:{}",
                run.id,
                run.state.as_str()
            )),
            candidate.map_or_else(String::new, |run| format!(
                "{}:{}",
                run.id,
                run.state.as_str()
            )),
            record.stage.encode(),
            if armed { "armed" } else { "" }
        );
        let has_ci = self.config.ci.enabled
            && (own.is_some() || self.ci_gate(&sandbox).await.gate.is_some());
        let status = if has_ci {
            status_text(own, armed, notice_hold, queue_note, &integration)
        } else {
            no_ci_status(armed)
        };
        let armed_head = match &seen.now {
            ArmedNow::Armed(merge) => merge.head.clone(),
            ArmedNow::None | ArmedNow::Deciding => String::new(),
        };
        let armed_green = if armed_head.is_empty() {
            false
        } else {
            match self.ci_gate_for_pull(&sandbox, pr, &armed_head).await {
                yulius_domain::ci::GateResult::Run(Some(RunState::Passed | RunState::Skipped)) => {
                    true
                }
                yulius_domain::ci::GateResult::NotConfigured => {
                    !delivery_profile(&sandbox).require_checks
                }
                _ => false,
            }
        };
        let armed_green_waiting =
            armed_green && !ci_busy && notice_hold == Hold::Free && !self.train_running(&sandbox);
        // Auto-clear an earlier armed-PR stall Needs You once the PR moves (enters a train,
        // head changes, disarms, or checks stop being green; SPEC-580 AWM-004).
        let armed_key = format!("{pr}:{armed_head}");
        if told.armed_needs_you_at > 0
            && (!armed_green_waiting || told.armed_green_key != armed_key)
        {
            self.end_needs_you_asked_at(&task.id, told.armed_needs_you_at, None)
                .await;
            told.clear_armed_stall();
            needs_you = !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None));
            person_must_act = needs_you || browser_req || flagged_active;
        }
        let armed_wait_reason = if armed_green_waiting {
            self.armed_wait_reason(task, queue_note, &integration).await
        } else {
            String::new()
        };
        let runtime = self.existing_runtime(&task.id)?;
        let mut facts = PrFacts {
            now_ms,
            pr,
            events,
            progress,
            ci_busy,
            status,
            check_ins: !needs_you,
            person_must_act,
            has_ci,
            head: own.map(|run| run.sha.clone()).unwrap_or_default(),
            armed_green_waiting,
            armed_head,
            integration,
            armed_wait_reason,
            deferred: false,
            ..PrFacts::default()
        };
        if let Some((span, decision, pull)) = &receipt {
            facts.head.clone_from(&pull.head_sha);
            if pull.head_sha == decision.head && pull.base_sha == decision.singleton.base {
                if let Some(run) = own
                    && run.state.is_active()
                    && decision.expired_ordinary_run.as_ref() != Some(&run.id)
                {
                    // queued_at is a mutable queue position. Persist the first observation
                    // of this run and request, as the active train does for its ordinary wait.
                    let prior = told
                        .singleton_wait
                        .as_ref()
                        .filter(|wait| wait.request == span.key && wait.run == run.id);
                    if now_ms.saturating_sub(prior.map_or(now_ms, |wait| wait.since_ms))
                        <= crate::merge_train::WAIT_LIMIT_MS
                    {
                        return Some((
                            PrFacts {
                                singleton_wait: prior.is_none().then(|| SingletonWait {
                                    request: span.key.clone(),
                                    run: run.id.clone(),
                                    since_ms: now_ms,
                                }),
                                ..observed()
                            },
                            None,
                        ));
                    }
                }
                facts.events.retain(|event| event.kind != "green");
                if !own.is_some_and(|run| run.state == RunState::Failed) {
                    facts
                        .progress
                        .push_str(&format!("|{}", decision.singleton.run));
                    facts.status =
                        format!("The integration candidate failed for pull request #{pr}.");
                    facts.ci_busy = false;
                    facts.events.push(Event {
                        kind: "outcome",
                        key: outcome_key(&decision.head, &decision.message),
                        text: decision.message.clone(),
                    });
                }
            }
        }
        self.refresh_notice_activity(&task.id, &runtime, &mut facts);
        crate::race::reached(
            crate::race::Point::LivePrFactsRead,
            std::path::Path::new(task.id.as_str()),
        );
        if self.update_holds_starts()
            || !seen.same_arm(&self.armed_snapshot(&task.id))
            || self.merge_is_armed(&task.id) != armed
        {
            return observed_only();
        }
        let candidate_id = candidate_id.map(str::to_owned);
        let source = verified_head
            .map(|head| head.to_owned())
            .map(|head| QueueNoticeSource {
                head,
                decision: receipt.map(|(span, _, pull)| (span, pull.base_sha)),
                ordinary: own.cloned(),
                candidate_id,
                candidate: candidate.cloned(),
                train: train_read,
            });
        Some((facts, source))
    }

    fn refresh_notice_activity(
        &self,
        task: &TaskId,
        runtime: &crate::supervisor::TaskRuntime,
        facts: &mut PrFacts,
    ) {
        facts.working = runtime.turn_in_progress();
        facts.idle = !facts.working && self.typed_recently(task).is_none();
        facts.quiet_since_ms = runtime
            .turn_closed_ms
            .load(Ordering::SeqCst)
            .max(runtime.spawned_ms.load(Ordering::SeqCst));
        facts.check_ins &= !runtime.goal_achieved.load(Ordering::SeqCst);
    }

    /// Read the CI run named by the holding train. A PR left out of it has no candidate run.
    async fn holding_run(
        &self,
        sandbox: &SandboxRecord,
        id: Option<&str>,
    ) -> AppResult<Option<CiRunRecord>> {
        let Some(id) = id else { return Ok(None) };
        Ok(self
            .deps
            .store
            .ci_run(id)
            .await?
            .filter(|run| run.sandbox_id == sandbox.id))
    }

    async fn candidate_receipt(
        &self,
        task: &TaskRecord,
        pr: u64,
    ) -> AppResult<
        Option<(
            crate::stages::SpanRecord,
            crate::merge_train::SingletonDecision,
        )>,
    > {
        if self.merge_is_armed(&task.id) {
            return Ok(None);
        }
        let (_, mut outcome) = self.armed_view(&task.id);
        let wait = self.deps.store.latest_merge_wait(&task.id).await?;
        Ok(candidate_from_wait(wait, &mut outcome)?
            .filter(|(_, decision)| decision.singleton.pr == pr))
    }

    /// Capacity reads never contact GitHub. A cold or expired observation holds the owner.
    pub(crate) async fn local_candidate_blocks(
        &self,
        task: &TaskRecord,
        record: Option<&crate::delivery::DeliveryRecord>,
    ) -> AppResult<bool> {
        let Some(record) = record.filter(|r| r.stage == DeliveryStage::PrOpen) else {
            return Ok(false);
        };
        let Some(pr) = record.pr_number else {
            return Ok(true);
        };
        let Some((span, _)) = self.candidate_receipt(task, pr).await? else {
            return Ok(false);
        };
        let runtime = self.runtime(&task.id);
        let observation = runtime
            .candidate_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(!observation
            .obsolete
            .as_ref()
            .is_some_and(|(observed, branch, at)| {
                observed == &span && branch == &task.branch && at.elapsed() < WATCH_EVERY * 2
            }))
    }

    /// Whether the views show the owner's recorded failure as pending: as capacity, but from
    /// the last answered observation, so a re-read in flight or a failed one changes nothing.
    pub(crate) async fn local_candidate_shown(
        &self,
        task: &TaskRecord,
        record: Option<&crate::delivery::DeliveryRecord>,
    ) -> AppResult<bool> {
        let Some(record) = record.filter(|r| r.stage == DeliveryStage::PrOpen) else {
            return Ok(false);
        };
        let Some(pr) = record.pr_number else {
            return Ok(true);
        };
        let Some((span, _)) = self.candidate_receipt(task, pr).await? else {
            return Ok(false);
        };
        let runtime = self.runtime(&task.id);
        let observation = runtime
            .candidate_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(!observation
            .shown
            .as_ref()
            .is_some_and(|(observed, branch)| observed == &span && branch == &task.branch))
    }

    async fn observe_candidate_pull(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        span: &crate::stages::SpanRecord,
        decision: &crate::merge_train::SingletonDecision,
    ) -> AppResult<crate::ports::github::GhPull> {
        let runtime = self.runtime(&task.id);
        let lock = || {
            runtime
                .candidate_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        let change_observation = || {
            runtime
                .arm_writer
                .try_write()
                .map_err(|_| AppError::Conflict("The session’s delivery is being checked.".into()))
        };
        let generation = {
            let _writer = change_observation()?;
            let mut observed = lock();
            observed.generation = observed.generation.wrapping_add(1);
            observed.obsolete = None;
            observed.generation
        };
        crate::race::reached(
            crate::race::Point::CandidateObservationRead,
            std::path::Path::new(task.id.as_str()),
        );
        let pull = self
            .read_notice_pull(sandbox, decision.singleton.pr)
            .await?;
        let obsolete = self
            .candidate_pull_signal(task, sandbox, decision, &pull)
            .is_none();
        let shown_changed = {
            let _writer = change_observation()?;
            let mut observed = lock();
            if observed.generation != generation {
                // A newer observation started meanwhile: it answers for both.
                false
            } else {
                if obsolete {
                    observed.obsolete =
                        Some((span.clone(), task.branch.clone(), std::time::Instant::now()));
                }
                let shown = obsolete.then(|| (span.clone(), task.branch.clone()));
                let changed = observed.shown != shown;
                observed.shown = shown;
                changed
            }
        };
        // What the task shows follows the answered observation (SPEC-621 FIN-004), including a
        // new failed decision replacing an older cleared one: say so when it changes.
        if shown_changed {
            self.publish_task(&task.id).await;
        }
        Ok(pull)
    }

    pub(crate) fn candidate_pull_signal(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        decision: &crate::merge_train::SingletonDecision,
        pull: &crate::ports::github::GhPull,
    ) -> Option<yulius_domain::task::PullSignal> {
        use yulius_domain::task::PullSignal;
        let pr = decision.singleton.pr;
        if pull.number != pr || !matches!(pull.state.as_str(), "open" | "closed") {
            return Some(PullSignal::Errored);
        }
        if pull.merged || pull.state == "closed" || pull.from_fork {
            return None;
        }
        if task.branch.as_deref().is_none_or(str::is_empty)
            || pull.head.is_empty()
            || pull.base.is_empty()
            || pull.head_sha.is_empty()
        {
            return Some(PullSignal::Errored);
        }
        if !self.notice_pull_owned(task, sandbox, pr, pull) || pull.head_sha != decision.head {
            return None;
        }
        if pull.base_sha.is_empty() {
            return Some(PullSignal::Errored);
        }
        (pull.base_sha == decision.singleton.base).then_some(PullSignal::Failing)
    }

    async fn read_notice_pull(
        &self,
        sandbox: &SandboxRecord,
        pr: u64,
    ) -> AppResult<crate::ports::github::GhPull> {
        let read = async {
            let token = self.dlv_installation_token(sandbox).await?;
            self.deps
                .github
                .get_pull(&token, &sandbox.repo_owner, &sandbox.repo_name, pr)
                .await
        };
        tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .map_err(|_| AppError::Upstream("Timed out verifying the pull request".into()))?
    }

    fn notice_pull_owned(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        pr: u64,
        pull: &crate::ports::github::GhPull,
    ) -> bool {
        pull.number == pr
            && pull.state == "open"
            && !pull.merged
            && !pull.from_fork
            && pull.base == delivery_profile(sandbox).integration_branch
            && task.branch.as_deref() == Some(pull.head.as_str())
            && !pull.head_sha.is_empty()
    }

    /// Recheck the recipient after the hosted read, before recording anything as heard.
    async fn armed_notice_recipient(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        pr: u64,
        source: &QueueNoticeSource,
    ) -> Option<NoticeRecipient> {
        let head = &source.head;
        if self.run_superseded_by_pushed_head(task, head).await {
            return None;
        }
        crate::race::reached(
            crate::race::Point::ArmedNoticePublicationRead,
            std::path::Path::new(task.id.as_str()),
        );
        // The last hosted read also decides whether the live owner still occupies its slot.
        let pull = if let Some((span, _)) = &source.decision {
            let decision = crate::merge_train::SingletonDecision::from_span(span).ok()??;
            self.observe_candidate_pull(task, sandbox, span, &decision)
                .await
                .ok()?
        } else {
            self.read_notice_pull(sandbox, pr).await.ok()?
        };
        if !self.notice_pull_owned(task, sandbox, pr, &pull)
            || !pull.head_sha.eq_ignore_ascii_case(head)
        {
            return None;
        }
        if let Some((span, base)) = &source.decision
            && (&pull.base_sha != base
                || self
                    .deps
                    .store
                    .latest_merge_wait(&task.id)
                    .await
                    .ok()?
                    .as_ref()
                    != Some(span))
        {
            return None;
        }
        let (ordinary, candidate) = tokio::try_join!(
            self.ordinary_pull_run(sandbox, pr, head),
            self.holding_run(sandbox, source.candidate_id.as_deref()),
        )
        .ok()?;
        if ordinary != source.ordinary || candidate != source.candidate {
            return None;
        }
        let recipient = self
            .deps
            .store
            .armed_notice_recipient(&task.id, pr, task.branch.as_deref()?)
            .await
            .ok()??;
        if let Some(reason) = &recipient.attention
            && !(self.held_draft_box_cleared(task) && crate::task_message::draft_flag(reason))
        {
            return None;
        }
        // Chromium can hold its slot across a restart. A pending lookup leaves the notice
        // unheard so that the next pass verifies the published head again.
        tokio::select! {
            biased;
            request = self.browser_tab_request_for(&task.sandbox_id, &task.id) => request.is_none().then_some(recipient),
            () = std::future::ready(()) => None,
        }
    }

    /// Checks GitHub (bounded to 5 s) before escalating a stalled armed green PR to Needs You:
    /// returns `true` ONLY when GitHub positively confirms the PR is no longer open/unmerged at
    /// `head` on the integration branch (so an already-merged or externally moved PR never pages a
    /// person). Errors and timeouts return `false` so a GitHub outage cannot hide a genuine stall
    /// (SPEC-580 review F4).
    async fn hosted_arm_stale_or_moved(&self, task: &TaskRecord, pr: u64, head: &str) -> bool {
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            return false;
        };
        let read = async {
            let token = self.dlv_installation_token(&sandbox).await?;
            self.deps
                .github
                .get_pull(&token, &sandbox.repo_owner, &sandbox.repo_name, pr)
                .await
        };
        match tokio::time::timeout(Duration::from_secs(5), read).await {
            Ok(Ok(pull)) => {
                !(pull.number == pr
                    && pull.state == "open"
                    && !pull.merged
                    && !pull.from_fork
                    && pull.base == delivery_profile(&sandbox).integration_branch
                    && task.branch.as_deref() == Some(pull.head.as_str())
                    && pull.head_sha.eq_ignore_ascii_case(head))
            }
            _ => false,
        }
    }

    /// Concrete reason why an armed green PR has not entered a merge candidate or merged yet
    /// (SPEC-580 AWM-004).
    async fn armed_wait_reason(
        &self,
        task: &TaskRecord,
        queue_note: Option<&QueueNote>,
        integration: &str,
    ) -> String {
        if let Some(note) = queue_note {
            return note.text.clone();
        }
        match self.queue_readiness(task, None).await {
            Readiness::Working => format!(
                "Yulius still sees the agent's turn as working, so the merge queue waits for its \
                 turn to end before testing on `{integration}`."
            ),
            Readiness::Shells => format!(
                "A background command is still running in the task, so the merge queue waits for \
                 it to finish before updating `{integration}`."
            ),
            Readiness::Dirty => format!(
                "Uncommitted changes remain in the task worktree, so the merge queue cannot test \
                 it on top of `{integration}`."
            ),
            Readiness::NotRunning | Readiness::MovedOn | Readiness::Ready => format!(
                "The armed green pull request has not entered a merge candidate or merged into \
                 `{integration}`."
            ),
        }
    }

    /// Persists an actionable Needs You request via SPEC-578 (`tasks.needs_you` + `publish_task`
    /// -> `task_wait_messages`) when an armed green PR remains stalled past its bound. Returns
    /// `Some(at_ms)` when recorded so it can be auto-cleared once the PR makes progress.
    async fn escalate_armed_stall(
        &self,
        task: &TaskRecord,
        pr: u64,
        happened: &str,
        problem: &str,
        action: &str,
    ) -> Option<i64> {
        let store = &self.deps.store;
        if !matches!(store.task_needs_you(&task.id).await, Ok(None)) {
            return None;
        }
        let now = self.now();
        let (_, link) = self.task_page_link(task).await;
        let clamp =
            |text: &str, max: usize| crate::attention::shown_line(text, max).unwrap_or_default();
        let record = crate::needs_you::NeedsYouRecord {
            action: clamp(action, crate::needs_you::MAX_ACTION_CHARS),
            happened: Some(clamp(happened, crate::needs_you::MAX_HAPPENED_CHARS)),
            problem: Some(clamp(problem, crate::needs_you::MAX_PROBLEM_CHARS)),
            decision: false,
            link: Some(link),
            at: now.0,
            cause: None,
            named: crate::needs_you::Named::default(),
        };
        if let Err(err) = store.set_task_needs_you(&task.id, Some(&record), now).await {
            tracing::warn!(task = %task.id, pr, error = %err, "could not record armed-PR stall Needs You");
            return None;
        }
        self.publish_task(&task.id).await;
        if let Err(err) = store
            .audit(
                now,
                "system",
                "task.needs_you.armed_stalled",
                Some(task.id.as_str()),
                Some(happened),
            )
            .await
        {
            tracing::warn!(task = %task.id, pr, error = %err, "auditing armed-PR stall Needs You failed");
        }
        Some(now.0)
    }

    /// The merge notice of `task`'s pull request `pr` (`merged_text`), read from GitHub once and
    /// kept until it is typed (review minor 7: not a GitHub call every pass).
    async fn merged_text_once(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        pr: u64,
    ) -> Option<String> {
        let runtime = self.existing_runtime(&task.id)?;
        let kept = runtime
            .pr_merged_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .filter(|(number, _)| *number == pr);
        if let Some((_, text)) = kept {
            return Some(text);
        }
        let text = self.merged_note(task, sandbox, pr, true).await;
        *runtime
            .pr_merged_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((pr, text.clone()));
        Some(text)
    }

    /// Whether `task`'s actionable failed publication still applies to its current unpublished
    /// worktree `HEAD` (SPEC-553 REP-004). Returns `None` once the branch or worktree has moved
    /// past `note_head`, preventing stale notices or parked wakes after a clean follow-up commit
    /// while preserving SPEC-139 PSH-001 push-note semantics.
    async fn current_actionable_push_problem(
        &self,
        task: &TaskRecord,
        remote_head: Option<&str>,
    ) -> Option<(String, String)> {
        let (note_head, text) = self.actionable_push_problem(&task.id)?;
        if remote_head.is_some_and(|h| h.eq_ignore_ascii_case(&note_head)) {
            return None;
        }
        if let Some(worktree) = task.worktree_path.as_deref()
            && let Some(local_head) = self
                .deps
                .git
                .log(std::path::Path::new(worktree), "HEAD", 1)
                .await
                .ok()
                .and_then(|log| log.into_iter().next())
                .map(|c| c.sha)
            && !local_head.eq_ignore_ascii_case(&note_head)
        {
            return None;
        }
        if remote_head.is_none()
            && self
                .branch_head(task)
                .await
                .is_some_and(|h| h.eq_ignore_ascii_case(&note_head))
        {
            return None;
        }
        Some((note_head, text))
    }

    /// Whether a failed CI run at `failed_sha` has already been superseded by a newer commit
    /// pushed to `task`'s feature branch (SPEC-553 REP-002). Keeps stale failed-run notices
    /// suppressed across repeated `watch_prs` passes (and restarts) when no new CI run exists yet.
    /// The Stop gate asks it too before continuing a Codex goal on that failure (ADR-0464).
    /// A Git error counts as not superseded, for both.
    pub(crate) async fn run_superseded_by_pushed_head(
        &self,
        task: &TaskRecord,
        failed_sha: &str,
    ) -> bool {
        let Some(worktree) = task.worktree_path.as_deref() else {
            return false;
        };
        let worktree = std::path::Path::new(worktree);
        if let Some(pushed) = self
            .existing_runtime(&task.id)
            .and_then(|rt| rt.auto_pr_head.lock().ok().and_then(|g| g.clone()))
            && !pushed.eq_ignore_ascii_case(failed_sha)
            && matches!(
                self.deps
                    .git
                    .is_ancestor(worktree, failed_sha, &pushed)
                    .await,
                Ok(true)
            )
        {
            return true;
        }
        if let Some(remote) = self.branch_head(task).await
            && !remote.eq_ignore_ascii_case(failed_sha)
            && matches!(
                self.deps
                    .git
                    .is_ancestor(worktree, failed_sha, &remote)
                    .await,
                Ok(true)
            )
        {
            return true;
        }
        false
    }

    /// Local check + one-shot repair publication for an idle running task on a failing open PR
    /// (SPEC-553 REP-004). Returns `true` when it pushed a repair commit.
    async fn maybe_publish_repair_for_watch(&self, task: &TaskRecord, failed_sha: &str) -> bool {
        let Some(runtime) = self.existing_runtime(&task.id) else {
            return false;
        };
        if runtime.turn_in_progress()
            || !task
                .activity
                .is_some_and(yulius_domain::task::Activity::is_idle)
        {
            return false;
        }
        let Some(worktree) = task.worktree_path.as_deref().map(std::path::Path::new) else {
            return false;
        };
        let Some(head) = self
            .deps
            .git
            .log(worktree, "HEAD", 1)
            .await
            .ok()
            .and_then(|log| log.into_iter().next())
            .map(|c| c.sha)
        else {
            return false;
        };
        if head.eq_ignore_ascii_case(failed_sha) || self.has_push_problem_for(&task.id, &head) {
            return false;
        }
        {
            let mut checked = runtime
                .repair_checked_head
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if checked
                .as_deref()
                .is_some_and(|h| h.eq_ignore_ascii_case(&head))
            {
                return false;
            }
            *checked = Some(head.clone());
        }
        self.try_publish_open_pr_repair(&task.id, Some(&head)).await
    }

    /// The head of the task's branch as last pushed (the clone's remote-tracking branch), which
    /// is the pull request's head: the one the Git overview and `task_is_done` ask about, so the
    /// watch tells the conflict they read (review of SPEC-491, nit 5). Local commits not pushed
    /// yet are not the pull request's.
    async fn branch_head(&self, task: &TaskRecord) -> Option<String> {
        let worktree = task.worktree_path.as_deref().map(std::path::Path::new)?;
        let branch = task.branch.as_deref()?;
        self.deps
            .git
            .resolve_commit(worktree, &format!("refs/remotes/origin/{branch}"))
            .await
            .ok()
            .flatten()
    }

    /// Whether `head` conflicts with the integration branch as Yulius's clone last fetched it,
    /// with the repository's merge drivers (PRN-004); cached per head and integration commit.
    /// No answer counts as no conflict: [`Self::local_conflict`] says when there is none.
    async fn conflicts(&self, task: &TaskRecord, sandbox: &SandboxRecord, head: &str) -> bool {
        self.local_conflict(task, sandbox, head)
            .await
            .unwrap_or(false)
    }

    /// [`Self::conflicts`], `None` when the clone cannot answer: the task has no worktree (left
    /// or never made), the integration branch or `head` is not in it, or Git failed.
    pub(crate) async fn local_conflict(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
        head: &str,
    ) -> Option<bool> {
        // In the task's worktree, as delivery's own check: the merge drivers of the clone's old
        // checkout could be stale (review minor 11).
        let repo = task
            .worktree_path
            .as_deref()
            .map(std::path::PathBuf::from)?;
        let integration = format!("origin/{}", delivery_profile(sandbox).integration_branch);
        let base = self
            .deps
            .git
            .resolve_commit(&repo, &integration)
            .await
            .ok()
            .flatten()?;
        let pair = format!("{head}:{base}");
        // Kept with the task's run; a task this server has no run of is asked each time.
        let runtime = self.existing_runtime(&task.id);
        if let Some(runtime) = &runtime
            && let Some((checked, conflicts)) = runtime
                .pr_conflict
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            && checked == pair
        {
            return Some(conflicts);
        }
        // A missing commit, missing common history or failed lookup cannot establish
        // mergeability. Let the caller fall back to GitHub and keep the cache empty.
        let since = self.deps.git.merge_base(&repo, &base, head).await.ok()??;
        let conflicts = self
            .deps
            .git
            .adds_to(&repo, &base, &since, head)
            .await
            .ok()?
            .is_none();
        if let Some(runtime) = runtime {
            *runtime
                .pr_conflict
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((pair, conflicts));
        }
        Some(conflicts)
    }
}

/// What a task was told after a pass whose message could not be typed: as before, but about the
/// pull request that pass saw (so its merge is told even if nothing was typed yet), preserving
/// the armed-green PR wait clock so blocked typing never resets the stall bound (SPEC-580).
fn untold(told: Told, next: &Told) -> Told {
    if told.pr == next.pr {
        Told {
            armed_green_key: next.armed_green_key.clone(),
            armed_green_since_ms: next.armed_green_since_ms,
            armed_check_in_ms: next.armed_check_in_ms,
            armed_needs_you_at: next.armed_needs_you_at,
            ..told
        }
    } else {
        Told {
            pr: next.pr,
            progress: next.progress.clone(),
            progress_ms: next.progress_ms,
            head: next.head.clone(),
            armed_green_key: next.armed_green_key.clone(),
            armed_green_since_ms: next.armed_green_since_ms,
            armed_check_in_ms: 0,
            armed_needs_you_at: next.armed_needs_you_at,
            ..Told::default()
        }
    }
}

/// What a green head that is not merging says (PRN-006): the kind of reason and the words; none
/// for an armed, free one no merge pass kept yet (the next pass, within 20 s, may merge it).
fn green_said(
    head: &str,
    armed: bool,
    hold: Hold,
    note: Option<&QueueNote>,
    integration: &str,
) -> Option<(&'static str, String)> {
    if armed && note.is_some_and(|note| note.awaits_own_run) {
        return None;
    }
    match (armed, hold, note.map(|note| note.text.as_str())) {
        (false, _, _) => Some(("unarmed", unarmed_text(head))),
        (true, Hold::Held, note) => Some((
            "held",
            format!(
                "{} {}",
                note.map_or_else(|| "A merge candidate holds it.".to_owned(), upper_first),
                hands_off(integration)
            ),
        )),
        (true, Hold::Free, Some(note)) => Some((
            "armed",
            format!(
                "Its head {} passed and the merge is armed, but it has not merged yet: {} \
                 Nothing to do unless that says so; Yulius tells you when it moves.",
                short(head),
                upper_first(note)
            ),
        )),
        (true, Hold::Free, None) => None,
    }
}

/// Where a pull request stands in a project without Yulius CI: no run ever comes (review M2).
fn no_ci_status(armed: bool) -> String {
    if armed {
        "This project has no Yulius CI, so no run comes: the merge is armed and Yulius decides it \
         on its next pass; `delivery_status` says what it waits for. Never merge by hand."
            .to_owned()
    } else {
        format!(
            "This project has no Yulius CI, so no run comes: {}; `delivery_status` says what \
             else the merge needs.",
            yulius_domain::review::KEEP_APPROVAL
        )
    }
}

/// Where the pull request stands and what to do, for a check-in or a stall (PRN-008, PRN-009).
fn status_text(
    own: Option<&CiRunRecord>,
    armed: bool,
    hold: Hold,
    note: Option<&QueueNote>,
    integration: &str,
) -> String {
    let Some(run) = own else {
        return "No Yulius CI run at its head yet: if you committed, end your turn so Yulius \
                pushes; `delivery_status` says what is missing."
            .to_owned();
    };
    let head = short(&run.sha);
    match run.state {
        RunState::Queued | RunState::Running => format!(
            "Its own run {} at {head} is {}: nothing to do, Yulius tells you when it ends. End \
             your turn.",
            run.id,
            run.state.as_str()
        ),
        RunState::Failed => format!(
            "Its run {} at {head} failed: read it with `ci_status` and fix it on this branch.",
            run.id
        ),
        RunState::Error | RunState::Cancelled => format!(
            "Its run {} at {head} ended {} without testing the code: rerun it with `ci_rerun`.",
            run.id,
            run.state.as_str()
        ),
        RunState::Passed | RunState::Skipped if !armed => format!(
            "Its head {head} is green with no merge armed: {}.",
            yulius_domain::review::KEEP_APPROVAL
        ),
        RunState::Passed | RunState::Skipped => {
            let note = note.filter(|note| !note.awaits_own_run).map_or_else(
                || "The merge is armed; Yulius decides it on its next pass.".to_owned(),
                |note| upper_first(&note.text),
            );
            if hold == Hold::Held {
                format!("{note} {}", hands_off(integration))
            } else {
                format!("{note} Nothing to do unless that says so; never merge by hand.")
            }
        }
    }
}

fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_domain::ci::GateSource;
    use yulius_domain::ids::SandboxId;
    use yulius_domain::resources::Need;
    use yulius_domain::time::Timestamp;

    const MIN: i64 = 60 * 1000;

    fn run(id: &str, state: RunState) -> CiRunRecord {
        CiRunRecord {
            id: id.to_owned(),
            sandbox_id: SandboxId::parse("s-one").unwrap_or_else(|_| unreachable!()),
            reference: CiRef::PullRequest(7),
            sha: "0123456789abcdef".to_owned(),
            source: GateSource::ProjectConfig,
            state,
            reason: None,
            requested_by: "yulius".to_owned(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(0),
            started_at: None,
            ended_at: None,
        }
    }

    fn job(name: &str, state: RunState, exit: Option<i32>, failures: Option<&str>) -> CiJobRecord {
        CiJobRecord {
            id: format!("j-{name}"),
            run_id: "r1".to_owned(),
            ordinal: 0,
            name: name.to_owned(),
            key: name.to_owned(),
            command: String::new(),
            matrix: None,
            state,
            reason: None,
            exit_code: exit,
            output: String::new(),
            failures: failures.map(str::to_owned),
            started_at: None,
            ended_at: None,
            need: Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        }
    }

    fn event(kind: &'static str, key: &str) -> Event {
        Event {
            kind,
            key: key.to_owned(),
            text: format!("{kind} {key}."),
        }
    }

    /// An open pull request at 10 minutes, its session idle since then.
    fn facts(events: Vec<Event>) -> PrFacts {
        PrFacts {
            now_ms: 10 * MIN,
            pr: 7,
            events,
            progress: "r1:running|||armed".to_owned(),
            ci_busy: true,
            status: "Its own run r1 is running.".to_owned(),
            idle: true,
            quiet_since_ms: 10 * MIN,
            check_ins: true,
            has_ci: true,
            ..PrFacts::default()
        }
    }

    /// PRN-002: the failed jobs with their exit codes, at most five test names each, and where
    /// to read the log; an errored run is not the code.
    #[test]
    fn a_red_run_names_jobs_tests_and_the_log() {
        let failures: String = (1..=7)
            .map(|i| format!("FAIL [   0.1s] (1/9) app tests::t{i}\n  stderr ───\n    boom\n\n"))
            .collect();
        let jobs = [
            job("lint", RunState::Passed, Some(0), None),
            job("quality", RunState::Failed, Some(100), Some(&failures)),
            job("websites", RunState::Failed, Some(1), None),
        ];
        let text = red_run_text(&run("r1", RunState::Failed), &jobs);
        assert_eq!(
            text,
            "Its Yulius CI run r1 at 012345678 failed: `quality` exited 100 (failing tests: app \
             tests::t1, app tests::t2, app tests::t3, app tests::t4, app tests::t5 and 2 more); \
             `websites` exited 1. Read `ci_status` with run_id r1. Set job to a listed name \
             for its full details. Fix it test-first on this branch, commit and end your turn so Yulius \
             pushes; a failed run drops an armed merge, so once the new head passed: if a review \
             approved this head or an earlier one of this pull request, call `delivery_merge` \
             with that approved head as `reviewed_head`: the approval carries across at most 20 \
             product lines in all since it (tests and documents do not count), and the call says \
             when the change needs a new round; with no approval yet, a review of the head comes \
             first."
        );
        let errored = RunState::Error;
        let text = errored_run_text(&CiRunRecord {
            reason: Some("timed out after 30 min".to_owned()),
            ..run("r2", errored)
        });
        assert!(
            text.contains("could not run (timed out after 30 min): that is not the code")
                && text.contains("`ci_rerun`"),
            "{text}"
        );
    }

    #[test]
    fn stopped_sibling_observations_are_partial_and_bounded() {
        let mut kept: String = (1..=38)
            .map(|n| format!("---- case-{n} stdout ----\nprivate assertion\n\n"))
            .collect();
        kept.push_str("---- case-1 stdout ----\n\n");
        let mut sibling = job("shard-2", RunState::Cancelled, None, Some(&kept));
        sibling.reason = Some("stopped early: job shard-1 failed".into());
        let primary = job("shard-1", RunState::Failed, Some(1), None);
        let jobs = [primary.clone(), sibling.clone()];
        let red = run("r1", RunState::Failed);
        for notice in [
            red_run_text(&red, &jobs),
            red_candidate_text(&red, &[5, 7], &jobs, "dev"),
        ] {
            assert!(notice.contains("`shard-2` cancelled (partial failing tests: case-1, case-2, case-3, case-4, case-5 and 33 more)"), "{notice}");
            assert!(
                !notice.contains("private assertion") && !notice.contains("case-6"),
                "{notice}"
            );
            assert!(
                notice.contains("ci_status") && notice.contains("run_id r1"),
                "{notice}"
            );
        }
        kept.push_str("[yulius] 7 more failing tests' output not kept\n");
        sibling.failures = Some(kept);
        let notice = red_run_text(&red, &[primary, sibling]);
        assert!(
            notice.contains("33 more") && notice.contains("7 additional failure blocks omitted"),
            "{notice}"
        );
    }

    #[test]
    fn unrelated_or_empty_cancellations_add_nothing_to_the_red_notice() {
        let primary = job("quality", RunState::Failed, Some(1), None);
        let red = run("r1", RunState::Failed);
        let ordinary = red_run_text(&red, std::slice::from_ref(&primary));
        for (state, reason, kept) in [
            (RunState::Cancelled, "superseded", "---- stale stdout ----"),
            (
                RunState::Cancelled,
                "stopped early: job  failed",
                "---- stale stdout ----",
            ),
            (
                RunState::Cancelled,
                "stopped early: the sandbox is over its disk quota",
                "---- stale stdout ----",
            ),
            (RunState::Cancelled, "stopped early: job quality failed", ""),
            (
                RunState::Cancelled,
                "stopped early: job quality failed",
                "ordinary output",
            ),
            (
                RunState::Passed,
                "stopped early: job quality failed",
                "---- stale stdout ----",
            ),
        ] {
            let mut sibling = job("shard", state, None, Some(kept));
            sibling.reason = Some(reason.into());
            sibling.output = "---- only-in-tail stdout ----".into();
            assert_eq!(red_run_text(&red, &[primary.clone(), sibling]), ordinary);
        }
    }

    #[test]
    fn a_red_browser_job_names_its_test_in_the_notice() {
        let mut scanner = yulius_domain::ci::failures::FailureScanner::default();
        scanner.push(b" FAIL  |chromium| tests/editor.e2e.ts > opens a folder\n");
        let kept = scanner.finish();
        let jobs = [job("e2e", RunState::Failed, Some(1), kept.as_deref())];
        let text = red_run_text(&run("r1", RunState::Failed), &jobs);
        assert!(
            text.contains("failing tests: tests/editor.e2e.ts › opens a folder"),
            "{text}"
        );
    }

    #[test]
    fn green_waits_for_the_queue_to_observe_the_pass() {
        for (hold, reason) in [
            (Hold::Free, "The own CI run is pending."),
            (
                Hold::Free,
                "The own CI run ended in error: rerun it with `ci_rerun`.",
            ),
            (Hold::Held, "The train waits for this member's own run."),
        ] {
            let stale = QueueNote {
                text: reason.to_owned(),
                awaits_own_run: true,
            };
            assert_eq!(
                green_said("0123456789", true, hold, Some(&stale), "dev"),
                None
            );
            let status = status_text(
                Some(&run("r1", RunState::Passed)),
                true,
                hold,
                Some(&stale),
                "dev",
            );
            assert!(!status.contains(reason), "{status}");
        }
        let other = QueueNote::current("The train waits for pull request #8's own run.".into());
        let (_, held) =
            green_said("0123456789", true, Hold::Held, Some(&other), "dev").unwrap_or_default();
        assert!(held.contains("waits for pull request #8"), "{held}");
        let fresh =
            QueueNote::current("Pull request #7 passed its own run but is behind `dev`.".into());
        let (_, notice) =
            green_said("0123456789", true, Hold::Free, Some(&fresh), "dev").unwrap_or_default();
        assert!(notice.contains("is behind `dev`"), "{notice}");
    }

    /// PRN-003, PRN-006: a red candidate and a held green head both keep the task's hands off.
    #[test]
    fn a_red_candidate_says_hands_off() {
        let candidate = CiRunRecord {
            reference: CiRef::Train(vec![5, 7]),
            ..run("t1", RunState::Failed)
        };
        let jobs = [job("quality", RunState::Failed, Some(101), None)];
        let text = red_candidate_text(&candidate, &[5, 7], &jobs, "dev");
        assert!(
            text.starts_with(
                "Merge candidate #5, #7 (run t1), which holds it, failed: `quality` exited 101."
            ) && text.contains("tells the pull request that breaks it")
                && text.contains("Do not re-arm, merge `dev` or push to this branch"),
            "{text}"
        );
    }

    /// PRN-006: a green head says why it is not merging, and to keep its hands off while a
    /// candidate holds it.
    #[test]
    fn green_says_why_and_hands_off_in_a_candidate() {
        let green = run("r1", RunState::Passed);
        let unarmed = status_text(Some(&green), false, Hold::Free, None, "dev");
        assert!(
            unarmed.contains("no merge armed") && unarmed.contains("`delivery_merge`"),
            "{unarmed}"
        );
        let note = QueueNote::current("pull request #7 is number 2 of 2 in merge train #5, #7: Yulius CI tests their combined result (run t1) and Yulius merges them in this order when it passes.".into());
        let held = status_text(Some(&green), true, Hold::Held, Some(&note), "dev");
        assert!(
            held.starts_with("Pull request #7 is number 2 of 2 in merge train #5, #7")
                && held.ends_with(
                    "Hands off: do not re-arm, merge `dev` or push to this branch; Yulius merges \
                     it in order when the run passes."
                ),
            "{held}"
        );
        // An armed, free head no pass kept yet says nothing: the next pass may merge it.
        assert_eq!(
            green_said("0123456789", true, Hold::Free, None, "dev"),
            None
        );
        let (class, text) =
            green_said("0123456789", true, Hold::Held, Some(&note), "dev").unwrap_or_default();
        assert_eq!(class, "held");
        assert!(text.ends_with("when the run passes."), "{text}");
        let (class, _) =
            green_said("0123456789", false, Hold::Free, None, "dev").unwrap_or_default();
        assert_eq!(class, "unarmed");
        let (class, text) = green_said(
            "0123456789",
            true,
            Hold::Free,
            Some(&QueueNote::current(
                "pull request #7 is behind `dev`.".into(),
            )),
            "dev",
        )
        .unwrap_or_default();
        assert_eq!(class, "armed");
        assert!(
            text.contains("not merged yet: Pull request #7 is behind `dev`."),
            "{text}"
        );
        let behind = status_text(
            Some(&green),
            true,
            Hold::Free,
            Some(&QueueNote::current(
                "Pull request #7 passed its own run but is behind `dev`.".into(),
            )),
            "dev",
        );
        assert!(behind.ends_with("never merge by hand."), "{behind}");
        assert!(unarmed_text("0123456789").contains("never merge by hand"));
        // SPEC-506 FST-001 (review round 1): a green head with no merge armed is told both cases,
        // an approval already given and none yet.
        for text in [
            unarmed_text("0123456789"),
            status_text(
                Some(&run("r1", RunState::Passed)),
                false,
                Hold::Free,
                None,
                "dev",
            ),
            no_ci_status(false),
        ] {
            assert!(
                text.contains(yulius_domain::review::KEEP_APPROVAL)
                    && text.contains("with no approval yet, a review of the head comes first"),
                "{text}"
            );
        }
        let queued = status_text(
            Some(&run("r1", RunState::Queued)),
            true,
            Hold::Free,
            None,
            "dev",
        );
        assert!(queued.contains("is queued: nothing to do"), "{queued}");
        assert!(status_text(None, false, Hold::Free, None, "dev").starts_with("No Yulius CI run"));
        // Review round 2: without Yulius CI no run comes, and the words say so.
        assert!(no_ci_status(false).contains("has no Yulius CI"));
        assert!(no_ci_status(false).contains("call `delivery_merge`"));
        assert!(no_ci_status(true).contains("Yulius decides it"));
    }

    /// PRN-001, PRN-007: new events go in one message; each key is typed once, a new key of the
    /// same kind again, and another pull request starts afresh.
    #[test]
    fn each_event_is_typed_once() {
        let bounds = WatchBounds::default();
        let (text, told) = decide(
            &facts(vec![event("run", "r1"), event("conflict", "abc")]),
            &Told::default(),
            &bounds,
        );
        assert_eq!(
            text.as_deref(),
            Some("Yulius, about your pull request #7:\n- run r1.\n- conflict abc.")
        );
        let (again, same) = decide(
            &facts(vec![event("run", "r1"), event("conflict", "abc")]),
            &told,
            &bounds,
        );
        assert_eq!(again, None);
        assert_eq!(same, told);
        let (text, told) = decide(&facts(vec![event("run", "r2")]), &told, &bounds);
        assert_eq!(
            text.as_deref(),
            Some("Yulius, about your pull request #7: run r2.")
        );
        let other = PrFacts {
            pr: 8,
            ..facts(vec![event("run", "r2")])
        };
        let (text, fresh) = decide(&other, &told, &bounds);
        assert!(text.is_some(), "another pull request");
        assert_eq!(fresh.pr, 8);
    }

    /// SPEC-487 CHN-001, CHN-002: when Yulius still has news for a running task's agent.
    #[test]
    fn news_is_due_until_the_agent_heard_it() {
        let told_open = Told {
            pr: 7,
            ..Told::default()
        };
        let mut told_merged = told_open.clone();
        told_merged.keys.insert("merged".into(), "7".into());
        let merged = NewsFacts {
            pull: Some((7, true)),
            ..NewsFacts::default()
        };
        // Merged: due to a task the watch saw with it open, until it said so.
        assert!(news_due(&merged, &told_open));
        assert!(!news_due(&merged, &told_merged));
        assert!(!news_due(&merged, &Told::default()), "never seen open");
        let other = Told {
            pr: 6,
            ..Told::default()
        };
        assert!(!news_due(&merged, &other), "it saw another pull request");
        // An armed merge that ended on an open pull request: due until the watch said how.
        let dropped = NewsFacts {
            pull: Some((7, false)),
            outcome: Some(("abcdef1234".into(), "The merge was dropped: red.".into())),
            ..NewsFacts::default()
        };
        assert!(news_due(&dropped, &told_open));
        assert!(
            news_due(&dropped, &Told::default()),
            "the watch sees it next"
        );
        let mut told_outcome = told_open.clone();
        told_outcome.keys.insert(
            "outcome".into(),
            outcome_key("abcdef1234", "The merge was dropped: red."),
        );
        assert!(!news_due(&dropped, &told_outcome));
        let again = NewsFacts {
            outcome: Some((
                "abcdef1234".into(),
                "The merge was dropped: a conflict.".into(),
            )),
            ..dropped.clone()
        };
        assert!(news_due(&again, &told_outcome), "another end is other news");
        // An open pull request with nothing armed and nothing ended: no news.
        let open = NewsFacts {
            pull: Some((7, false)),
            ..NewsFacts::default()
        };
        assert!(!news_due(&open, &told_open));
        assert!(
            !news_due(&NewsFacts::default(), &Told::default()),
            "no pull request"
        );
        // A person's pause may never end, and nothing is typed into a playground's tasks.
        for quiet in [
            NewsFacts {
                paused: true,
                ..merged.clone()
            },
            NewsFacts {
                paused: true,
                ..dropped.clone()
            },
            NewsFacts {
                playground: true,
                ..dropped.clone()
            },
        ] {
            assert!(!news_due(&quiet, &told_open), "{quiet:?}");
        }
        // An armed merge is Yulius's to make, paused task or not.
        let armed = NewsFacts {
            armed: true,
            paused: true,
            playground: true,
            ..NewsFacts::default()
        };
        assert!(news_due(&armed, &Told::default()));
    }

    /// SPEC-487 CHN-002 (review of slice 1, D-1): the idle time that bounds queued news counts
    /// from a turn's end only when that end is the session's last word.
    #[test]
    fn a_session_is_idle_since_its_last_turns_end_only() {
        // Idle after a Stop (or a person's interrupt, which is recorded as one): since then.
        assert_eq!(idle_since(true, 100, 200), 200);
        // The task works: a Stop Yulius's gate blocked moved nothing, and the turn goes on.
        assert_eq!(idle_since(false, 100, 200), 0);
        // Idle after a turn that began after the last clean Stop and ended without one (an
        // error, also when Yulius's gate had blocked a Stop of that turn): the last clean Stop
        // is an earlier turn's, so nothing is known, and no bound may run out at once.
        assert_eq!(idle_since(true, 300, 200), 0);
        // A run without hooks knows no end.
        assert_eq!(idle_since(true, 0, 0), 0);
    }

    /// SPEC-487 CHN-002: news that was typed is due until its turn begins. Typed into an idle
    /// session: until the task starts to work (the mark is taken away then), for
    /// [`NEWS_TURN_WAIT_MS`] at most. Typed while a turn ran: as long as Claude Code's queue
    /// says a prompt waits for its turn, however long the running turn takes.
    #[test]
    fn typed_news_is_due_until_its_turn() {
        let minute = 60_000;
        let mut told = Told {
            pr: 7,
            ..Told::default()
        };
        told.keys.insert("merged".into(), "7".into());
        let idle = |typed_ms, now_ms| NewsFacts {
            typed_ms,
            now_ms,
            pull: Some((7, true)),
            ..NewsFacts::default()
        };
        assert!(news_due(&idle(10 * minute, 10 * minute), &told));
        assert!(news_due(&idle(10 * minute, 12 * minute - 1), &told));
        assert!(
            !news_due(&idle(10 * minute, 12 * minute), &told),
            "a paste nobody took holds nothing for ever"
        );
        assert!(!news_due(&idle(0, 10 * minute), &told), "nothing typed");
        // Queued behind a turn that runs for half an hour: due while the queue says so.
        let queued = |prompt_waits| NewsFacts {
            in_queue: true,
            prompt_waits,
            ..idle(10 * minute, 40 * minute)
        };
        assert!(news_due(&queued(true), &told));
        assert!(!news_due(&queued(false), &told), "handed on and seen");
        // Once the task reads idle, a hand-over whose turn never shows waits two minutes from
        // its last turn's end. While it works, however long ago a Stop came (one that
        // Yulius's gate blocked moves the turn's end and the turn goes on), and for a run
        // whose turn ends are not known, the queue alone decides (`idle_since_ms` 0).
        let idle_since = |idle_since_ms| NewsFacts {
            idle_since_ms,
            ..queued(true)
        };
        assert!(news_due(&idle_since(38 * minute + 1), &told));
        assert!(!news_due(&idle_since(38 * minute), &told));
        assert!(news_due(&idle_since(0), &told));
        // What Claude Code's queue holds says nothing about news not read in it: news typed
        // into an idle session, or queued news whose record was not read yet (time decides).
        let unread = |now_ms| NewsFacts {
            prompt_waits: true,
            ..idle(10 * minute, now_ms)
        };
        assert!(news_due(&unread(11 * minute), &told));
        assert!(!news_due(&unread(12 * minute), &told));
        // A person's pause mutes it too.
        let paused = NewsFacts {
            paused: true,
            ..idle(10 * minute, 10 * minute)
        };
        assert!(!news_due(&paused, &told));
    }

    /// PRN-005: the merge is said once, then nothing more.
    #[test]
    fn merged_ends_the_watch() {
        let bounds = WatchBounds::default();
        let merged = facts(vec![event("merged", "7")]);
        let (text, told) = decide(
            &merged,
            &Told {
                pr: 7,
                ..Told::default()
            },
            &bounds,
        );
        assert_eq!(
            text.as_deref(),
            Some("Yulius, about your pull request #7: merged 7.")
        );
        assert_eq!(decide(&merged, &told, &bounds).0, None);
    }

    /// PRN-007: what was told is saved and read back, so a restart repeats nothing.
    #[test]
    fn state_survives_a_restart() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(PR_NOTICES_FILE);
        assert!(read_notices(&path).is_empty(), "no file");
        let (_, told) = decide(
            &facts(vec![event("run", "r1")]),
            &Told::default(),
            &WatchBounds::default(),
        );
        let saved = BTreeMap::from([("t1".to_owned(), told.clone())]);
        std::fs::write(&path, serde_json::to_vec(&saved)?)?;
        let read = read_notices(&path);
        assert_eq!(read.get("t1"), Some(&told));
        assert_eq!(
            decide(
                &facts(vec![event("run", "r1")]),
                &told,
                &WatchBounds::default()
            )
            .0,
            None
        );
        std::fs::write(&path, "not json")?;
        assert!(read_notices(&path).is_empty(), "unreadable");
        Ok(())
    }

    /// PRN-008: an idle session quiet for 45 minutes gets a check-in, then again 45 minutes after
    /// it; not while busy, waiting for a person or done.
    #[test]
    fn a_quiet_session_gets_a_check_in() {
        let bounds = WatchBounds::default();
        let (_, told) = decide(&facts(Vec::new()), &Told::default(), &bounds);
        let at = |minutes: i64| PrFacts {
            now_ms: minutes * MIN,
            ..facts(Vec::new())
        };
        assert_eq!(decide(&at(54), &told, &bounds).0, None, "44 quiet minutes");
        let (text, told) = decide(&at(55), &told, &bounds);
        assert_eq!(
            text.as_deref(),
            Some(
                "Yulius, about your pull request #7: Check-in while it is open. Its own run r1 is running."
            )
        );
        assert_eq!(
            decide(&at(99), &told, &bounds).0,
            None,
            "44 minutes after it"
        );
        assert!(decide(&at(100), &told, &bounds).0.is_some());
        for quiet in [
            PrFacts {
                idle: false,
                ..at(100)
            },
            PrFacts {
                check_ins: false,
                ..at(100)
            },
            PrFacts {
                quiet_since_ms: 60 * MIN,
                ..at(100)
            },
        ] {
            assert_eq!(decide(&quiet, &told, &bounds).0, None, "{quiet:?}");
        }
    }

    /// Review M1: nothing is typed while a person must act (a permission prompt, a dialog,
    /// `needs_you`): the paste would answer it. The event waits, untold, until they did.
    #[test]
    fn nothing_is_typed_while_a_person_must_act() {
        let bounds = WatchBounds::default();
        let blocked = PrFacts {
            person_must_act: true,
            ..facts(vec![event("run", "r1")])
        };
        let (text, told) = decide(&blocked, &Told::default(), &bounds);
        assert_eq!(text, None);
        assert!(told.keys.is_empty(), "still to tell");
        let stalled = PrFacts {
            now_ms: 500 * MIN,
            ci_busy: false,
            ..blocked.clone()
        };
        assert_eq!(
            decide(&stalled, &told, &bounds).0,
            None,
            "no stall typed either"
        );
        assert!(
            decide(&facts(vec![event("run", "r1")]), &told, &bounds)
                .0
                .is_some()
        );
    }

    /// Review M2 and minor 6: without Yulius CI no run ever comes, so nothing stalls for lack of
    /// one; and an agent working on it (a turn open) is no stall.
    #[test]
    fn no_stall_without_ci_or_while_the_agent_works() {
        let bounds = WatchBounds::default();
        let quiet = |minutes: i64| PrFacts {
            now_ms: minutes * MIN,
            ci_busy: false,
            idle: false,
            ..facts(Vec::new())
        };
        let (_, told) = decide(&quiet(10), &Told::default(), &bounds);
        assert_eq!(
            decide(
                &PrFacts {
                    has_ci: false,
                    ..quiet(200)
                },
                &told,
                &bounds
            )
            .0,
            None
        );
        assert_eq!(
            decide(
                &PrFacts {
                    working: true,
                    ..quiet(200)
                },
                &told,
                &bounds
            )
            .0,
            None
        );
        // Review round 2: a turn that just closed (its push and run still to come) is no stall.
        let just_closed = PrFacts {
            quiet_since_ms: 199 * MIN,
            ..quiet(200)
        };
        assert_eq!(decide(&just_closed, &told, &bounds).0, None);
        assert!(decide(&quiet(200), &told, &bounds).0.is_some());
    }

    /// PRN-009: an hour without progress while CI is idle stalls it, once, until progress; a run
    /// under way or a message to the task is no stall.
    #[test]
    fn a_pull_request_without_progress_stalls() {
        let bounds = WatchBounds::default();
        let idle = |minutes: i64| PrFacts {
            now_ms: minutes * MIN,
            ci_busy: false,
            idle: false,
            progress: "r1:passed|||armed".to_owned(),
            status: "The merge is armed.".to_owned(),
            ..facts(Vec::new())
        };
        let (_, told) = decide(&idle(10), &Told::default(), &bounds);
        assert_eq!(decide(&idle(69), &told, &bounds).0, None);
        assert_eq!(
            decide(
                &PrFacts {
                    ci_busy: true,
                    ..idle(200)
                },
                &told,
                &bounds
            )
            .0,
            None,
            "its run is under way"
        );
        let (text, stalled) = decide(&idle(70), &told, &bounds);
        let text = text.unwrap_or_default();
        assert!(
            text.starts_with(
                "Yulius, about your pull request #7: It has made no progress for 60 min"
            ) && text.contains("never merge it by hand"),
            "{text}"
        );
        assert_eq!(
            stalled.stalled.as_deref(),
            Some("Pull request #7 has made no progress for 60 min: The merge is armed.")
        );
        assert_eq!(decide(&idle(200), &stalled, &bounds).0, None, "told once");
        let moved = PrFacts {
            progress: "r2:queued|||armed".to_owned(),
            ..idle(201)
        };
        assert_eq!(
            decide(&moved, &stalled, &bounds).1.stalled,
            None,
            "progress ends it"
        );
        let (_, told) = decide(&facts(vec![event("run", "r1")]), &stalled, &bounds);
        assert_eq!(told.stalled, None, "a message to the task ends it");
    }

    /// SPEC-553 REP-004: an actionable failed publication is told once per `(head, message)`,
    /// survives a restart through `Told`, and counts as news due only until told.
    #[test]
    fn failed_publication_notice_is_told_once_and_counts_as_news_due_until_told() {
        let bounds = WatchBounds::default();
        let text = "Yulius could not push 3a3b0b620 to pull request #7: diverged".to_owned();
        let key = publish_key("3a3b0b6201111111", &text);
        let pub_event = Event {
            kind: "publish",
            key: key.clone(),
            text: text.clone(),
        };
        let news = NewsFacts {
            pull: Some((7, false)),
            publish_problem: Some(("3a3b0b6201111111".to_owned(), text.clone())),
            ..NewsFacts::default()
        };
        assert!(news_due(&news, &Told::default()), "untold failure is due");

        let (said, told) = decide(&facts(vec![pub_event.clone()]), &Told::default(), &bounds);
        assert!(said.is_some_and(|s| s.contains("3a3b0b620")));
        assert_eq!(told.keys.get("publish"), Some(&key));
        assert!(!news_due(&news, &told), "told failure is no longer due");

        // Second pass and after a restart (reloaded `told`): no duplicate notice.
        let reloaded: Told =
            serde_json::from_str(&serde_json::to_string(&told).unwrap_or_else(|_| String::new()))
                .unwrap_or_default();
        assert_eq!(decide(&facts(vec![pub_event]), &reloaded, &bounds).0, None);

        // A new head's failure gets its own one-time notice.
        let next_key = publish_key("c367788262222222", &text);
        let next_event = Event {
            kind: "publish",
            key: next_key,
            text,
        };
        assert!(
            decide(&facts(vec![next_event]), &reloaded, &bounds)
                .0
                .is_some()
        );
    }

    /// SPEC-580 AWM-004: an armed green PR waiting without progress checks in after the bound
    /// (even when `working == true`), then escalates to Needs You if unmoved (including when
    /// check-in typing was blocked), ignores deferred passes, and resets when progress occurs.
    #[test]
    fn armed_green_stall_checks_in_then_escalates_and_resets_on_progress() {
        let bounds = WatchBounds::default();
        let at = |min: i64| PrFacts {
            now_ms: min * 60_000,
            pr: 1498,
            working: true,
            armed_green_waiting: true,
            armed_head: "74dfc55c9abcdef".to_owned(),
            integration: "dev".to_owned(),
            armed_wait_reason: "Uncommitted changes remain.".to_owned(),
            ..PrFacts::default()
        };
        let mut told = Told {
            pr: 1498,
            ..Told::default()
        };
        // T=1 starts the armed-green clock cleanly.
        assert_eq!(
            decide_armed_stall(&at(1), &mut told, &bounds, 338),
            ArmedStallAction::None
        );
        assert_eq!(told.armed_green_since_ms, 60_000);
        // Deferred pass leaves the clock alone.
        let deferred = PrFacts {
            deferred: true,
            armed_green_waiting: false,
            ..at(5)
        };
        assert_eq!(
            decide_armed_stall(&deferred, &mut told, &bounds, 338),
            ArmedStallAction::None
        );
        assert_eq!(told.armed_green_since_ms, 60_000);
        // At T=10 (9 min elapsed): still before 10 min bound.
        assert_eq!(
            decide_armed_stall(&at(10), &mut told, &bounds, 338),
            ArmedStallAction::None
        );
        // At T=11 (10 min elapsed): fires check-in even though `working == true`.
        let action = decide_armed_stall(&at(11), &mut told, &bounds, 338);
        assert!(
            matches!(action, ArmedStallAction::CheckIn(ref msg) if msg.contains("#1498") && msg.contains("74dfc55c9")),
            "{action:?}"
        );
        // Simulate delivered check-in at T=11.
        told.armed_check_in_ms = 11 * 60_000;
        // At T=20 (9 min after check-in): not escalated yet.
        assert_eq!(
            decide_armed_stall(&at(20), &mut told, &bounds, 338),
            ArmedStallAction::None
        );
        // At T=21 (10 min after check-in): escalates to Needs You.
        let escalated = decide_armed_stall(&at(21), &mut told, &bounds, 338);
        assert!(
            matches!(
                escalated,
                ArmedStallAction::Escalate { ref happened, ref problem, ref action }
                if happened.contains("#1498")
                    && happened.contains("even after a check-in")
                    && problem.contains("Uncommitted changes")
                    && action.contains("#338")
            ),
            "{escalated:?}"
        );
        // If check-in could never be delivered (armed_check_in_ms == 0), escalation still fires
        // once check_in + needs_you has elapsed.
        let mut blocked = Told {
            pr: 1498,
            armed_green_key: "1498:74dfc55c9abcdef".to_owned(),
            armed_green_since_ms: 60_000,
            armed_check_in_ms: 0,
            ..Told::default()
        };
        let escalated_blocked = decide_armed_stall(&at(21), &mut blocked, &bounds, 338);
        assert!(
            matches!(
                escalated_blocked,
                ArmedStallAction::Escalate { ref happened, .. }
                if happened.contains("could not deliver a check-in")
            ),
            "{escalated_blocked:?}"
        );
        // Leaving armed-green waiting resets all fields.
        let progressed = PrFacts {
            armed_green_waiting: false,
            ..at(22)
        };
        assert_eq!(
            decide_armed_stall(&progressed, &mut told, &bounds, 338),
            ArmedStallAction::None
        );
        assert_eq!(told.armed_green_since_ms, 0);
        assert_eq!(told.armed_check_in_ms, 0);
        assert!(told.armed_green_key.is_empty());
    }

    #[test]
    fn untold_resets_armed_check_in_on_new_head_and_preserves_on_same_head() {
        let told_head_a = Told {
            pr: 10,
            armed_green_key: "10:aaaaaaa".to_owned(),
            armed_green_since_ms: 60_000,
            armed_check_in_ms: 660_000,
            ..Told::default()
        };
        let next_same = Told {
            pr: 10,
            armed_green_key: "10:aaaaaaa".to_owned(),
            armed_green_since_ms: 60_000,
            armed_check_in_ms: 660_000,
            ..Told::default()
        };
        let kept_same = untold(told_head_a.clone(), &next_same);
        assert_eq!(kept_same.armed_green_key, "10:aaaaaaa");
        assert_eq!(kept_same.armed_check_in_ms, 660_000);

        // New head B: `decide_armed_stall` reset `next.armed_check_in_ms = 0`; a failed paste on
        // the transition pass must NOT leak head A's check-in timestamp onto head B (SPEC-580 F5).
        let next_head_b = Told {
            pr: 10,
            armed_green_key: "10:bbbbbbb".to_owned(),
            armed_green_since_ms: 1_320_000,
            armed_check_in_ms: 0,
            ..Told::default()
        };
        let kept_b = untold(told_head_a, &next_head_b);
        assert_eq!(kept_b.armed_green_key, "10:bbbbbbb");
        assert_eq!(kept_b.armed_green_since_ms, 1_320_000);
        assert_eq!(kept_b.armed_check_in_ms, 0);
    }
}
