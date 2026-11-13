//! Tasks hear what happens to their pull request (SPEC-444, ADR-0287).
//!
//! A task used to hear about its pull request only through `yulius wait`, a background shell it
//! had to keep alive; Claude Code's goal check-ins pause after half an hour. Every
//! [`WATCH_EVERY`], this watch looks at each running task whose delivery has an open pull
//! request, from Yulius's own records and clone, and types what needs the task into its session
//! with the next step (PRN-001..PRN-006): a red run at its head or of a merge candidate holding
//! it, a conflict with the integration branch, how its armed merge ended, its merge, and why a
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

use crate::ci::{CiJobRecord, CiRunRecord};
use crate::context::App;
use crate::delivery::{delivery_profile, past_merge};
use crate::error::AppError;
use crate::merge_train::Hold;
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
/// The file in the data directory that keeps what each task was told across restarts.
pub const PR_NOTICES_FILE: &str = "pr-notices.json";
/// Runs of a sandbox read per pass, newest first.
const RUNS_READ: u32 = 100;
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
}

impl Default for WatchBounds {
    fn default() -> Self {
        Self {
            check_in_after_ms: CHECK_IN_AFTER_MS,
            stall_after_ms: STALL_AFTER_MS,
            stall_quiet_ms: STALL_QUIET_MS,
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
    /// The pull request's head, from its newest run (empty when none).
    pub head: String,
}

/// What the watch told one task, as saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    /// The pull request it is about: another one starts afresh.
    pub pr: u64,
    /// The last key typed of each kind.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
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

/// The words of a red run's jobs: each failed or errored job with its exit code and up to
/// [`MAX_TESTS`] failing test names (PRN-002).
fn failed_jobs(jobs: &[CiJobRecord]) -> String {
    let said: Vec<String> = jobs
        .iter()
        .filter(|job| matches!(job.state, RunState::Failed | RunState::Error))
        .map(|job| {
            let how = job.exit_code.map_or_else(
                || job.reason.clone().unwrap_or_else(|| "failed".to_owned()),
                |code| format!("exited {code}"),
            );
            let names = job
                .failures
                .as_deref()
                .map(yulius_domain::ci::failures::failed_test_names)
                .unwrap_or_default();
            let tests = match names.len() {
                0 => String::new(),
                n if n <= MAX_TESTS => format!(" (failing tests: {})", names.join(", ")),
                n => format!(
                    " (failing tests: {} and {} more)",
                    names[..MAX_TESTS].join(", "),
                    n - MAX_TESTS
                ),
            };
            format!("`{}` {how}{tests}", job.name)
        })
        .collect();
    if said.is_empty() {
        "a job failed".to_owned()
    } else {
        said.join("; ")
    }
}

/// The first failed job's name, for where to read the log.
fn first_failed(jobs: &[CiJobRecord]) -> Option<&str> {
    jobs.iter()
        .find(|job| matches!(job.state, RunState::Failed | RunState::Error))
        .map(|job| job.name.as_str())
}

/// PRN-002: a red run at the pull request's head.
fn red_run_text(run: &CiRunRecord, jobs: &[CiJobRecord]) -> String {
    let log = first_failed(jobs).map_or_else(
        || format!("`ci_status` with run_id {}", run.id),
        |job| format!("`ci_status` with run_id {} and job `{job}`", run.id),
    );
    format!(
        "Its Yulius CI run {} at {} failed: {}. Read the log with {log}, fix it test-first on \
         this branch, commit and end your turn so Yulius pushes; a failed run drops an armed \
         merge, so once the new head passed and its review approved it, call `delivery_merge` \
         again.",
        run.id,
        short(&run.sha),
        failed_jobs(jobs)
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
        "Merge candidate {} (run {}), which holds it, failed: {}. Yulius tests smaller candidates \
         and tells the pull request that breaks it. Do not re-arm, merge `{integration}` or push \
         to this branch meanwhile.",
        listed.join(", "),
        run.id,
        failed_jobs(jobs)
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
        "Its head {} passed its own Yulius CI run, but no merge is armed: once the independent \
         review approved {}, call `delivery_merge` with that reviewed_head. The queue does the \
         rest; never merge by hand.",
        short(head),
        short(head)
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

/// The saved notices; none when the file is missing or unreadable.
/// What the watch told each task, kept beside [`PR_NOTICES_FILE`] so that a question about one
/// task reads no file.
pub(crate) type ToldCache = std::sync::Mutex<Option<BTreeMap<String, Told>>>;

/// How long (ms) news the watch typed counts as due while the turn it starts has not begun: a
/// paste Claude Code swallowed must not hold a chain for ever (SPEC-487 CHN-002).
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

/// What [`App::pr_news_due`] reads about one running task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NewsFacts {
    /// A merge is armed or being decided.
    pub armed: bool,
    /// The watch types nothing into it now: a person paused it, or an update holds the runs.
    pub held: bool,
    /// News was typed and its turn has not begun, for less than [`NEWS_TURN_WAIT_MS`].
    pub typed: bool,
    /// Its pull request and whether it is merged, when it has one.
    pub pull: Option<(u64, bool)>,
    /// How its last armed merge ended (its head and the line for the agent), when it has one.
    pub outcome: Option<(String, String)>,
    /// Its sandbox is a playground: the watch tells its tasks nothing.
    pub playground: bool,
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
    if facts.held || facts.playground {
        return false;
    }
    if facts.typed {
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

/// What [`PR_NOTICES_FILE`] holds: what the watch told each task.
fn read_notices(path: &std::path::Path) -> BTreeMap<String, Told> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
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
    pub async fn watch_prs(self: &Arc<Self>, bounds: &WatchBounds) -> Vec<(TaskId, String)> {
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
            let told = saved.get(task.id.as_str()).cloned().unwrap_or_default();
            let Some(runtime) = self.existing_runtime(&task.id) else {
                continue;
            };
            let held =
                update_holds || !matches!(self.deps.store.open_pause(&task.id).await, Ok(None));
            let facts = if task.status == TaskStatus::Running && !held {
                self.pr_facts(&task, &told, &mut runs).await
            } else {
                None
            };
            let Some(facts) = facts else {
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
            let (text, next) = decide(&facts, &told, bounds);
            let next = match text {
                None => next,
                Some(text) => {
                    // Job names, reasons and test names come from the project: no control
                    // character may end the paste early (review minor 4).
                    let text = crate::task_message::clean_message(&text);
                    let paste = format!("\x1b[200~{text}\x1b[201~");
                    match self
                        .type_into_session(&task, paste.as_bytes(), Typed::Prompt, Sender::Yulius)
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(task = %task.id, pr = facts.pr, "told the task about its pull request");
                            // Until the turn this starts has begun, the task goes on by itself
                            // (SPEC-487 CHN-002).
                            runtime
                                .pr_news_typed_ms
                                .store(self.now().0, Ordering::SeqCst);
                            typed.push((task.id.clone(), text));
                            next
                        }
                        // Not ready for input, or a person is typing or may have a draft in
                        // the prompt box (SPEC-480): the next pass tries again. The pull request
                        // it saw open is kept, so its merge is still told.
                        Err(AppError::Conflict(why)) => {
                            tracing::debug!(task = %task.id, why, "a pull request notice waits");
                            untold(told, &next)
                        }
                        Err(err) => {
                            tracing::warn!(task = %task.id, error = %err, "could not tell the task about its pull request");
                            untold(told, &next)
                        }
                    }
                }
            };
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

    /// The turn of what the watch typed into `task`'s session has begun: the task reads
    /// Working (SPEC-487 CHN-002).
    pub(crate) fn pr_news_turn_began(&self, task: &TaskId) {
        if let Some(runtime) = self.existing_runtime(task) {
            runtime.pr_news_typed_ms.store(0, Ordering::SeqCst);
        }
    }

    /// Whether Yulius still has news of `task`'s pull request for its agent ([`news_due`];
    /// SPEC-487 CHN-001, CHN-002). Its agent goes on by itself once told, so the task is
    /// neither Done nor a person's turn until then.
    pub(crate) async fn pr_news_due(&self, task: &TaskRecord) -> bool {
        if task.status != TaskStatus::Running {
            return false;
        }
        if self.merge_is_armed(&task.id) {
            return true;
        }
        let store = &self.deps.store;
        // As the watch decides whom it types to.
        let held =
            self.update_holds_starts() || !matches!(store.open_pause(&task.id).await, Ok(None));
        let typed_ms = self
            .existing_runtime(&task.id)
            .map_or(0, |runtime| runtime.pr_news_typed_ms.load(Ordering::SeqCst));
        let typed = typed_ms > 0 && self.now().0.saturating_sub(typed_ms) < NEWS_TURN_WAIT_MS;
        let record = store.delivery(&task.id).await.ok().flatten();
        let pull = record.as_ref().and_then(|record| {
            let merged = past_merge(&record.stage);
            (merged || record.stage == DeliveryStage::PrOpen)
                .then_some(record.pr_number)
                .flatten()
                .map(|pr| (pr, merged))
        });
        let outcome = self.armed_view(&task.id).1;
        // Asked only where it decides: an armed merge that ended on an open pull request.
        let playground = matches!(pull, Some((_, false)))
            && outcome.is_some()
            && matches!(
                store.sandbox(&task.sandbox_id).await,
                Ok(Some(sandbox)) if sandbox.is_playground()
            );
        let facts = NewsFacts {
            armed: false,
            held,
            typed,
            pull,
            outcome,
            playground,
        };
        self.with_pr_notices(|told| match told.get(task.id.as_str()) {
            Some(told) => news_due(&facts, told),
            None => news_due(&facts, &Told::default()),
        })
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
        told: &Told,
        runs: &mut BTreeMap<String, Vec<CiRunRecord>>,
    ) -> Option<PrFacts> {
        // The slot before the delivery: a merge decided in between then reads as merged, never
        // as "no merge armed" on an open pull request (review minor 8).
        let (armed, outcome) = self.armed_view(&task.id);
        let record = self.deps.store.delivery(&task.id).await.ok()??;
        let pr = record.pr_number?;
        let sandbox = self.deps.store.sandbox(&task.sandbox_id).await.ok()??;
        if sandbox.is_playground() {
            return None;
        }
        let now_ms = self.now().0;
        // A person must act first (a prompt on screen, `needs_you`): nothing is typed, the merge
        // notice included (review M1, round 2).
        let needs_you = !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None));
        let person_must_act =
            needs_you || task.activity == Some(yulius_domain::task::Activity::NeedsAttention);
        if past_merge(&record.stage) {
            // Merged: said once, to a task the watch saw with it open (PRN-005, PRN-007).
            if !merge_untold(told, pr) {
                return None;
            }
            let text = self.merged_text_once(task, &sandbox, pr).await?;
            return Some(PrFacts {
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
            });
        }
        if record.stage != DeliveryStage::PrOpen {
            return None;
        }
        let integration = delivery_profile(&sandbox).integration_branch;
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
        let mut own = sandbox_runs
            .iter()
            .find(|run| run.reference == CiRef::PullRequest(pr))
            .cloned();
        if own.is_none() && !told.head.is_empty() {
            own = self
                .deps
                .store
                .ci_runs_for_sha(&sandbox.id, &told.head)
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|run| run.reference == CiRef::PullRequest(pr));
        }
        let own = own.as_ref();
        // A candidate older than the pull request's own newest run is about an earlier head.
        let candidate = sandbox_runs
            .iter()
            .find(|run| matches!(&run.reference, CiRef::Train(members) if members.contains(&pr)))
            .filter(|run| own.is_none_or(|own| run.queued_at >= own.queued_at));
        let hold = self.train_hold(task);
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
        if let Some(run) = candidate
            && let CiRef::Train(members) = &run.reference
            && run.state == RunState::Failed
            && members.len() > 1
            && !told_key("candidate", &run.id)
        {
            let jobs = self
                .deps
                .store
                .ci_jobs(&run.id, true)
                .await
                .unwrap_or_default();
            events.push(Event {
                kind: "candidate",
                key: run.id.clone(),
                text: red_candidate_text(run, members, &jobs, &integration),
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
        if let Some(head) = &head
            && hold == Hold::Free
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
        let queue_note = self.merge_queue_note(task);
        if let (true, Some(head)) = (green, &head) {
            let said = green_said(head, armed, hold, queue_note.as_deref(), &integration);
            if let Some((class, text)) = said {
                events.push(Event {
                    kind: "green",
                    key: format!("{head}:{class}"),
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
            status_text(own, armed, hold, queue_note.as_deref(), &integration)
        } else {
            no_ci_status(armed)
        };
        let runtime = self.existing_runtime(&task.id)?;
        let idle = !runtime.turn_in_progress() && self.typed_recently(&task.id).is_none();
        let quiet_since_ms = runtime
            .turn_closed_ms
            .load(Ordering::SeqCst)
            .max(runtime.spawned_ms.load(Ordering::SeqCst));
        let check_ins = !runtime.goal_achieved.load(Ordering::SeqCst) && !needs_you;
        Some(PrFacts {
            now_ms,
            pr,
            events,
            progress,
            ci_busy,
            status,
            idle,
            quiet_since_ms,
            check_ins,
            person_must_act,
            working: runtime.turn_in_progress(),
            has_ci,
            head: own.map(|run| run.sha.clone()).unwrap_or_default(),
        })
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

    /// Whether `head` conflicts with the integration branch as Yulius's clone last fetched it,
    /// with the repository's merge drivers (PRN-004); cached per head and integration commit.
    async fn conflicts(&self, task: &TaskRecord, sandbox: &SandboxRecord, head: &str) -> bool {
        // In the task's worktree, as delivery's own check: the merge drivers of the clone's old
        // checkout could be stale (review minor 11).
        let Some(repo) = task.worktree_path.as_deref().map(std::path::PathBuf::from) else {
            return false;
        };
        let integration = format!("origin/{}", delivery_profile(sandbox).integration_branch);
        let Ok(Some(base)) = self.deps.git.resolve_commit(&repo, &integration).await else {
            return false;
        };
        let pair = format!("{head}:{base}");
        let Some(runtime) = self.existing_runtime(&task.id) else {
            return false;
        };
        if let Some((checked, conflicts)) = runtime
            .pr_conflict
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            && checked == pair
        {
            return conflicts;
        }
        let conflicts = match self.deps.git.merge_base(&repo, &base, head).await {
            Ok(Some(since)) => matches!(
                self.deps.git.adds_to(&repo, &base, &since, head).await,
                Ok(None)
            ),
            _ => false,
        };
        *runtime
            .pr_conflict
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((pair, conflicts));
        conflicts
    }
}

/// What a task was told after a pass whose message could not be typed: as before, but about the
/// pull request that pass saw (so its merge is told even if nothing was typed yet).
fn untold(told: Told, next: &Told) -> Told {
    if told.pr == next.pr {
        told
    } else {
        Told {
            pr: next.pr,
            progress: next.progress.clone(),
            progress_ms: next.progress_ms,
            head: next.head.clone(),
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
    note: Option<&str>,
    integration: &str,
) -> Option<(&'static str, String)> {
    match (armed, hold, note) {
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
        "This project has no Yulius CI, so no run comes: once the independent review approved \
         its head, call `delivery_merge`; `delivery_status` says what else the merge needs."
            .to_owned()
    }
}

/// Where the pull request stands and what to do, for a check-in or a stall (PRN-008, PRN-009).
fn status_text(
    own: Option<&CiRunRecord>,
    armed: bool,
    hold: Hold,
    note: Option<&str>,
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
            "Its head {head} is green with no merge armed: once the independent review approved \
             it, call `delivery_merge`."
        ),
        RunState::Passed | RunState::Skipped => {
            let note = note.map_or_else(
                || "The merge is armed; Yulius decides it on its next pass.".to_owned(),
                upper_first,
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
             `websites` exited 1. Read the log with `ci_status` with run_id r1 and job \
             `quality`, fix it test-first on this branch, commit and end your turn so Yulius \
             pushes; a failed run drops an armed merge, so once the new head passed and its \
             review approved it, call `delivery_merge` again."
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
        let note = "pull request #7 is number 2 of 2 in merge train #5, #7: Yulius CI tests their combined result (run t1) and Yulius merges them in this order when it passes.";
        let held = status_text(Some(&green), true, Hold::Held, Some(note), "dev");
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
            green_said("0123456789", true, Hold::Held, Some(note), "dev").unwrap_or_default();
        assert_eq!(class, "held");
        assert!(text.ends_with("when the run passes."), "{text}");
        let (class, _) =
            green_said("0123456789", false, Hold::Free, None, "dev").unwrap_or_default();
        assert_eq!(class, "unarmed");
        let (class, text) = green_said(
            "0123456789",
            true,
            Hold::Free,
            Some("pull request #7 is behind `dev`."),
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
            Some("Pull request #7 passed its own run but is behind `dev`."),
            "dev",
        );
        assert!(behind.ends_with("never merge by hand."), "{behind}");
        assert!(unarmed_text("0123456789").contains("never merge by hand"));
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

    /// PRN-005: the merge is said once, then nothing more.
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
        // Typed, its turn not begun: still due, whatever was told.
        let typed = NewsFacts {
            typed: true,
            ..merged.clone()
        };
        assert!(news_due(&typed, &told_merged));
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
        // The watch types nothing into a paused or held task, nor into a playground's.
        for quiet in [
            NewsFacts {
                held: true,
                ..merged.clone()
            },
            NewsFacts {
                held: true,
                ..typed.clone()
            },
            NewsFacts {
                held: true,
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
            held: true,
            playground: true,
            ..NewsFacts::default()
        };
        assert!(news_due(&armed, &Told::default()));
    }

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
}
