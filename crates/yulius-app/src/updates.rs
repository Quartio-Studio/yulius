//! Instance updates from `main` of Quartio-Studio/yulius (SPEC-001 OPS-006..OPS-009, SPEC-002
//! ADR-18).
//!
//! **Detect.** Every hour (and on "Check now" or opening the instance page, at most every 30 s) the
//! build's commit is compared with `main` through the GitHub API, read with the GitHub App's token
//! for that repository, else the configured read token, else anonymously. The result is cached;
//! superadmins are told.
//!
//! **Update.** Nothing updates by itself. A superadmin confirms the offered commit, then one run
//! moves through `building` (the root helper builds the commit; tasks keep working; once built, a
//! deploy still running is waited for and new ones are refused until the install ended, SPEC-342)
//! → `draining` (every task's agent is frozen at once by the process host, mid-turn, SPEC-160; a
//! run that cannot be frozen pauses at its turn boundary: the Stop and `UserPromptSubmit` hooks
//! are held as for a capacity pause, ADR-17; new starts queue) → `installing` (the helper stops
//! the service, backs up, installs, starts and checks the new build, rolling back on failure)
//! → `resuming` (the tasks are released and thawed one by one) → an outcome. The run is a
//! database row, so the server that starts after the restart carries it on; the agents are never
//! told.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;
use yulius_contracts::api::updates::{
    AvailableUpdate, BuildView, CommitView, PauseProgress, ProgressView, StartUpdate,
    UpdateCheckView, UpdateRunView, UpdateView, WaitingDeploy, WaitingTask,
};
use yulius_contracts::events::UiEvent;
use yulius_contracts::update::{HelperAction, HelperRequest, HelperState, HelperStatus};
use yulius_domain::ids::TaskId;
use yulius_domain::time::Timestamp;
use yulius_domain::update::{self as policy, RunState};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::deploy_gate::{DEPLOY_REFUSED, DeployGate, DeployPermit, DeployWait};
use crate::error::{AppError, AppResult};
use crate::model::Secret;
use crate::ports::session::RunFreeze;
use crate::ports::updates::{ReleaseSource, ReleaseState, SourceError, UpdateHelper};

/// Automatic checks of `main`.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// The first automatic check after a start.
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(60);
/// "Check now" reuses a check younger than this.
pub const MANUAL_CHECK_GAP_MS: i64 = 30_000;
/// How often an open run is driven.
pub const TICK: Duration = Duration::from_secs(2);
/// Longest sleep of the loop without a run.
const IDLE_TICK: Duration = Duration::from_secs(60);
/// A request the helper has not acknowledged is written again after this.
pub const ACK_RETRY_MS: i64 = 20_000;
/// The helper never acknowledged: the run gives up.
pub const ACK_GIVE_UP_MS: i64 = 90_000;
/// Longest build (the helper's own bound is 2 hours).
pub const BUILD_MAX_MS: i64 = 3 * 3_600_000;
/// Longest wait for every task to reach a pause point; then the update is cancelled.
pub const DRAIN_MAX_MS: i64 = 30 * 60_000;
/// Longest install (stop, backup, start, check and a rollback).
pub const INSTALL_MAX_MS: i64 = 15 * 60_000;
/// `Retry-After` of a hook held for an update: released tasks go on within this.
pub const HOLD_RETRY_SECS: u64 = 3;
/// A task whose hook was held for the update within this counts as held.
const HELD_FRESH_MS: i64 = 15_000;
/// Default wait between two released tasks ("resumed staggered").
pub const RESUME_STAGGER: Duration = Duration::from_secs(2);
/// Commits listed in a view.
const MAX_COMMITS_SHOWN: usize = 50;
/// Tasks listed as still in a turn, and deploys listed as still running.
const MAX_WAITING_SHOWN: usize = 20;
/// The wait for running deploys after the build lasts at most the longest a deploy can last (a CD
/// run's agent-state wait and its deploy, each up to the CI job timeout; an environment run's
/// preparation and command) plus this (SPEC-342 DPU-004).
const DEPLOY_WAIT_SLACK_MS: i64 = 5 * 60_000;
/// Consecutive passes that must find every task at a pause point before installing.
const READY_PASSES: u32 = 2;
/// Passes a run may stay frozen without the kernel confirming it; then it is thawed and paused
/// at its turn boundary instead (SPEC-160 OPS-011).
const MAX_UNSETTLED_PASSES: u32 = 5;

/// One update run as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRunRecord {
    /// Id (a UUID).
    pub id: String,
    /// The commit it installs.
    pub target_commit: String,
    /// The build it started from.
    pub from_commit: Option<String>,
    /// Summary of the commits it brings.
    pub summary: String,
    /// State.
    pub state: RunState,
    /// While resuming: the state it ends in.
    pub outcome: Option<RunState>,
    /// Why it failed, was cancelled or rolled back.
    pub error: Option<String>,
    /// Login of the superadmin who started it.
    pub requested_by: String,
    /// When it started.
    pub created_at: Timestamp,
    /// When its state began.
    pub state_since: Timestamp,
    /// When it ended.
    pub finished_at: Option<Timestamp>,
}

/// What the instance updates with. Without a source nothing is checked; without a helper
/// nothing can be installed.
pub struct UpdateServices {
    /// The commit this binary was built from.
    pub build: Option<String>,
    /// Reads `main`.
    pub source: Option<Arc<dyn ReleaseSource>>,
    /// The root helper.
    pub helper: Option<Arc<dyn UpdateHelper>>,
    /// `YULIUS_UPDATE_TOKEN_FILE`: a token that may read the repository.
    pub read_token: Option<Secret>,
    /// Automatic checks every this long (`None`: only "Check now").
    pub check_every: Option<Duration>,
    /// Wait between two released tasks.
    pub resume_stagger: Duration,
}

impl UpdateServices {
    /// Services for a build, with hourly checks and the default stagger.
    #[must_use]
    pub fn new(build: Option<String>) -> Self {
        Self {
            build: build.filter(|c| policy::is_commit(c)),
            source: None,
            helper: None,
            read_token: None,
            check_every: Some(CHECK_EVERY),
            resume_stagger: RESUME_STAGGER,
        }
    }
}

/// The last check of `main`.
#[derive(Debug, Clone)]
struct CheckSnapshot {
    at: Timestamp,
    via: &'static str,
    result: Result<ReleaseState, String>,
}

/// Runtime state of updates (owned by [`App`]).
#[derive(Default)]
pub struct UpdateState {
    services: OnceLock<UpdateServices>,
    /// Hooks are held and starts queue (a run is draining, installing or resuming).
    hold: AtomicBool,
    /// Tasks released while resuming.
    released: Mutex<HashSet<TaskId>>,
    /// When each task's hook was last held for the update (ms).
    held_at: Mutex<HashMap<TaskId, i64>>,
    /// What freezing each task's run did while this server drained (SPEC-160).
    frozen: Mutex<HashMap<TaskId, Frozen>>,
    /// Held by a freeze pass and by every thaw of all runs, so that a cancel's thaw never runs
    /// between a pass's freeze and its record.
    freezing: tokio::sync::Mutex<()>,
    check: Mutex<Option<CheckSnapshot>>,
    checking: tokio::sync::Mutex<()>,
    /// "Continue without waiting" was chosen for the draining run.
    proceed: AtomicBool,
    /// Consecutive passes that found every task at a pause point.
    ready_passes: AtomicU32,
    /// Request writes in the current phase (`phase_key`).
    writes: AtomicU32,
    phase_key: Mutex<String>,
    /// Released tasks of the resuming run, and how many there are.
    resumed: AtomicU32,
    resume_total: AtomicU32,
    /// The last published (phase, step, progress), to publish only changes.
    published: Mutex<String>,
    next_check_ms: AtomicI64,
    wake: Notify,
    started: AtomicBool,
    /// No deploy starts from the build's end until the install ended (SPEC-342).
    deploys: DeployGate,
    /// When this server first found a run's build done with a deploy running.
    deploys_wait: DeployWait,
}

/// A task's run as the drain froze it (SPEC-160).
#[derive(Debug, Clone)]
struct Frozen {
    run_dir: String,
    state: FreezeState,
}

/// Where freezing a task's run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreezeState {
    /// The kernel confirmed it: the task counts as paused.
    Frozen,
    /// Frozen, not confirmed yet after this many passes.
    Freezing(u32),
    /// It cannot be frozen in this drain: it pauses at its turn boundary.
    Refused,
}

fn build_view(commit: Option<&str>) -> BuildView {
    BuildView {
        commit: commit.map(str::to_owned),
        short: commit.map(|c| policy::short(c).to_owned()),
    }
}

fn request_id(run: &UpdateRunRecord, action: HelperAction) -> String {
    format!("{}-{}", run.id, action.as_str())
}

/// A calm sentence for the helper's step.
fn step_message(step: &str) -> Option<&'static str> {
    Some(match step {
        "fetching" => "Fetching the new version from GitHub.",
        "verifying" => "Checking that the commit is on main.",
        "compiling" => "Building the new version. Tasks keep working meanwhile.",
        "built" => "Built. Pausing tasks.",
        "stopping" => "Stopping the server. Agent sessions keep running.",
        "backing_up" => "Backing up the database.",
        "installing" => "Installing the new version.",
        "starting" => "Starting the new version.",
        "checking" => "Checking the new version.",
        "checking_host" => "Checking that sandboxes still get Docker.",
        "succeeded" => "The new version runs.",
        "rolling_back" => "The new version failed its check. Restoring the previous one.",
        _ => return None,
    })
}

impl App {
    /// Installs the update services (once; later calls are ignored and return `false`).
    pub fn set_updates(&self, services: UpdateServices) -> bool {
        self.updates.services.set(services).is_ok()
    }

    fn update_services(&self) -> Option<&UpdateServices> {
        self.updates.services.get()
    }

    /// The root update helper, when installed in the update services.
    pub(crate) fn updates_helper(&self) -> Option<&Arc<dyn UpdateHelper>> {
        self.update_services()?.helper.as_ref()
    }

    /// The commit this server was built from.
    #[must_use]
    pub fn build_commit(&self) -> Option<&str> {
        self.update_services().and_then(|s| s.build.as_deref())
    }

    /// Whether starts queue because an update pauses the tasks.
    pub(crate) fn update_holds_starts(&self) -> bool {
        self.updates.hold.load(Ordering::SeqCst)
    }

    /// Whether an update pauses `task` now: the hold is on and the task was not released yet.
    /// Its agent may be frozen (SPEC-160), which makes no hook call, or held at its boundary;
    /// either way time spent so does not count against it (the launch goal's watch).
    #[must_use]
    pub fn held_for_update(&self, task: &TaskId) -> bool {
        self.updates.hold.load(Ordering::SeqCst)
            && !self
                .updates
                .released
                .lock()
                .is_ok_and(|released| released.contains(task))
    }

    /// Whether an update's pause of `task` took hold (SPEC-191 PAU-001): its run is frozen, or
    /// its turn boundary was held in the last 15 s. Meaningful while [`Self::held_for_update`].
    #[must_use]
    pub fn update_pause_holds(&self, task: &TaskId) -> bool {
        let frozen = self.updates.frozen.lock().is_ok_and(|f| {
            f.get(task)
                .is_some_and(|run| run.state == FreezeState::Frozen)
        });
        frozen || self.recently_held(task)
    }

    fn recently_held(&self, task: &TaskId) -> bool {
        let now = self.now().0;
        self.updates.held_at.lock().is_ok_and(|h| {
            h.get(task)
                .is_some_and(|at| now.saturating_sub(*at) <= HELD_FRESH_MS)
        })
    }

    /// Whether the turn boundary of `task` is held for an update (`Some`), and then whether the
    /// hold just took it (`Some(true)`: its pause took hold and is worth publishing); records it.
    pub(crate) fn hold_for_update(&self, task: &TaskId) -> Option<bool> {
        if !self.updates.hold.load(Ordering::SeqCst) {
            return None;
        }
        if self
            .updates
            .released
            .lock()
            .is_ok_and(|released| released.contains(task))
        {
            return None;
        }
        let newly = !self.update_pause_holds(task);
        let now = self.now().0;
        if let Ok(mut held) = self.updates.held_at.lock() {
            held.insert(task.clone(), now);
        }
        // The agent is still held: a capacity pause of the same task must not count its hook as
        // given up (ADR-17's 45 s rule) because the update answered the polls meanwhile.
        self.runtime(task)
            .hold_polled_ms
            .store(now, Ordering::SeqCst);
        Some(newly)
    }

    /// Turns the hold on or off; the lists hear of every live task's pause when it changed
    /// (SPEC-191 PAU-002).
    async fn switch_hold(&self, on: bool) {
        if self.set_hold(on) {
            self.publish_live_tasks().await;
        }
    }

    async fn publish_live_tasks(&self) {
        let mut tasks: Vec<TaskId> = self
            .deps
            .store
            .live_attempts()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.task_id)
            .collect();
        tasks.sort();
        tasks.dedup();
        for task in &tasks {
            self.publish_task(task).await;
        }
    }

    /// Whether the hold changed.
    fn set_hold(&self, on: bool) -> bool {
        let was = self.updates.hold.swap(on, Ordering::SeqCst);
        if was && !on {
            if let Ok(mut released) = self.updates.released.lock() {
                released.clear();
            }
            if let Ok(mut held) = self.updates.held_at.lock() {
                held.clear();
            }
        }
        was != on
    }

    /// At start: an open run that pauses tasks keeps them paused from the first hook call on
    /// (before the loop runs), so a restart in the middle of an update never lets an agent go on.
    ///
    /// With no update holding tasks, every run left frozen (a server that stopped between freeze
    /// and thaw) is thawed (SPEC-160 OPS-013); one that installs or resumes keeps them frozen
    /// until it resumes them.
    pub async fn restore_update_hold(&self) {
        match self.deps.store.open_update_run().await {
            Ok(Some(run)) if run.state.holds_tasks() => {
                self.set_hold(true);
            }
            Ok(_) => self.sweep_frozen_runs().await,
            Err(err) => tracing::warn!(error = %err, "cannot read the open update run"),
        }
    }

    /// One freeze pass of the draining run `run_id` (SPEC-160 OPS-010/011): freezes the run of
    /// every live task not confirmed frozen yet, unless a run that cannot be frozen is still in a
    /// turn: then nothing stays frozen (the others would sit frozen while it works, for up to
    /// the drain's bound) and they freeze once it is at its turn boundary.
    async fn freeze_pass(&self, run_id: &str) {
        let _pass = self.updates.freezing.lock().await;
        let attempts = self.deps.store.live_attempts().await.unwrap_or_default();
        // "Continue without waiting" installs now: every run that can be frozen is, whatever the
        // others do.
        let proceed = self.updates.proceed.load(Ordering::SeqCst);
        if !proceed && self.unfreezable_in_turn(&attempts).await {
            self.thaw_while_one_works().await;
            return;
        }
        for attempt in &attempts {
            let known = self
                .updates
                .frozen
                .lock()
                .ok()
                .and_then(|f| f.get(&attempt.task_id).map(|run| run.state));
            let passes = match known {
                Some(FreezeState::Frozen | FreezeState::Refused) => continue,
                Some(FreezeState::Freezing(passes)) => passes,
                None if !self.can_freeze(&attempt.run_dir).await => continue,
                None => 0,
            };
            // A cancel may have ended the drain while this pass ran.
            if !self.still_draining(run_id).await {
                return;
            }
            let host = Arc::clone(&self.deps.session_host);
            let dir = PathBuf::from(&attempt.run_dir);
            let answer = tokio::task::spawn_blocking(move || host.freeze(&dir))
                .await
                .unwrap_or(RunFreeze::Unsupported);
            let state = match answer {
                RunFreeze::Frozen => FreezeState::Frozen,
                RunFreeze::Freezing if passes + 1 < MAX_UNSETTLED_PASSES => {
                    FreezeState::Freezing(passes + 1)
                }
                RunFreeze::Freezing => {
                    tracing::warn!(task = %attempt.task_id, "the run did not freeze; it pauses at its turn boundary");
                    let kept = self.person_frozen_runs().await.unwrap_or_default();
                    self.thaw_run(&attempt.run_dir, &kept).await;
                    FreezeState::Refused
                }
                RunFreeze::Unsupported => FreezeState::Refused,
            };
            if let Ok(mut frozen) = self.updates.frozen.lock() {
                frozen.insert(
                    attempt.task_id.clone(),
                    Frozen {
                        run_dir: attempt.run_dir.clone(),
                        state,
                    },
                );
            }
            // Its pause took hold: the lists say so (SPEC-191 PAU-002).
            if state == FreezeState::Frozen {
                self.publish_task(&attempt.task_id).await;
            }
        }
        // A run refused in this pass may be in a turn.
        if !proceed && self.unfreezable_in_turn(&attempts).await {
            self.thaw_while_one_works().await;
        }
    }

    /// Nothing stays frozen while a run that cannot be frozen works: the runs this drain froze,
    /// and those a server that restarted mid-drain froze (swept).
    async fn thaw_while_one_works(&self) {
        self.thaw_unconfirmed_and_frozen().await;
        self.sweep_frozen_runs().await;
    }

    async fn still_draining(&self, run_id: &str) -> bool {
        matches!(
            self.deps.store.open_update_run().await,
            Ok(Some(run)) if run.id == run_id && run.state == RunState::Draining
        )
    }

    async fn can_freeze(&self, run_dir: &str) -> bool {
        let host = Arc::clone(&self.deps.session_host);
        let dir = PathBuf::from(run_dir);
        tokio::task::spawn_blocking(move || host.can_freeze(&dir))
            .await
            .unwrap_or(false)
    }

    /// Whether a live task whose run cannot be frozen is still in a turn.
    async fn unfreezable_in_turn(&self, attempts: &[crate::model::AttemptRecord]) -> bool {
        for attempt in attempts {
            let known = self
                .updates
                .frozen
                .lock()
                .ok()
                .and_then(|f| f.get(&attempt.task_id).map(|run| run.state));
            let unfreezable = match known {
                Some(FreezeState::Refused) => true,
                Some(_) => false,
                None => !self.can_freeze(&attempt.run_dir).await,
            };
            if unfreezable && !self.at_turn_pause(&attempt.task_id).await {
                return true;
            }
        }
        false
    }

    /// Whether `task` is at a pause point of the turn-boundary hold (or gone).
    async fn at_turn_pause(&self, task: &TaskId) -> bool {
        let Ok(Some(record)) = self.deps.store.task(task).await else {
            return true;
        };
        policy::at_pause_point(record.status, record.activity, self.recently_held(task))
    }

    /// Thaws every run frozen in this drain and forgets it (refused runs are kept: they stay
    /// paused at their turn boundary).
    async fn thaw_unconfirmed_and_frozen(&self) {
        let runs: Vec<String> = self
            .updates
            .frozen
            .lock()
            .map(|mut f| {
                let thawed: Vec<TaskId> = f
                    .iter()
                    .filter(|(_, run)| run.state != FreezeState::Refused)
                    .map(|(task, _)| task.clone())
                    .collect();
                thawed
                    .into_iter()
                    .filter_map(|task| f.remove(&task).map(|run| run.run_dir))
                    .collect()
            })
            .unwrap_or_default();
        let kept = self.person_frozen_runs().await.unwrap_or_default();
        for run in runs {
            self.thaw_run(&run, &kept).await;
        }
    }

    /// The lock that serializes freezing and thawing runs: the update's freeze passes and thaws,
    /// and a person's pause and resume (SPEC-191), each with the check that decides it.
    pub(crate) async fn freeze_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.updates.freezing.lock().await
    }

    /// Freezes one run (blocking work off the runtime); the caller holds [`Self::freeze_lock`].
    pub(crate) async fn host_freeze(&self, run_dir: &str) -> RunFreeze {
        let host = Arc::clone(&self.deps.session_host);
        let dir = PathBuf::from(run_dir);
        tokio::task::spawn_blocking(move || host.freeze(&dir))
            .await
            .unwrap_or(RunFreeze::Unsupported)
    }

    /// Thaws one run for an update, never one a person paused (`kept`, from
    /// [`Self::person_frozen_runs`], SPEC-191 PER-003); the caller holds [`Self::freeze_lock`].
    async fn thaw_run(&self, run_dir: &str, kept: &[String]) {
        if kept.iter().any(|run| run == run_dir) {
            return;
        }
        self.host_thaw(run_dir).await;
    }

    /// Thaws one run (blocking work off the runtime).
    pub(crate) async fn host_thaw(&self, run_dir: &str) {
        let host = Arc::clone(&self.deps.session_host);
        let dir = PathBuf::from(run_dir);
        if tokio::task::spawn_blocking(move || host.thaw(&dir))
            .await
            .is_err()
        {
            tracing::warn!(run = run_dir, "thawing the run panicked");
        }
    }

    /// Thaws every run this server froze, then every run group left frozen (OPS-013).
    async fn thaw_frozen_runs(&self) {
        let _pass = self.updates.freezing.lock().await;
        let runs: Vec<String> = self
            .updates
            .frozen
            .lock()
            .map(|mut f| {
                f.drain()
                    .filter(|(_, run)| run.state != FreezeState::Refused)
                    .map(|(_, run)| run.run_dir)
                    .collect()
            })
            .unwrap_or_default();
        let kept = self.person_frozen_runs().await.unwrap_or_default();
        for run in runs {
            self.thaw_run(&run, &kept).await;
        }
        self.sweep_frozen_runs().await;
    }

    /// Thaws every run group the process host finds frozen, but those of runs a person paused;
    /// nothing when it cannot tell which those are (SPEC-191 PER-003).
    async fn sweep_frozen_runs(&self) {
        let keep: Vec<PathBuf> = match self.person_frozen_runs().await {
            Ok(runs) => runs.into_iter().map(PathBuf::from).collect(),
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the tasks people paused; nothing is thawed");
                return;
            }
        };
        let host = Arc::clone(&self.deps.session_host);
        match tokio::task::spawn_blocking(move || host.thaw_all(&keep)).await {
            Ok(0) => {}
            Ok(thawed) => tracing::info!(thawed, "thawed runs left frozen by an update"),
            Err(_) => tracing::warn!("thawing left-over runs panicked"),
        }
    }

    /// Starts the update loop (once): checks `main` and drives an open run.
    pub fn start_update_loop(self: &Arc<Self>) {
        if self.update_services().is_none() || self.updates.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let first = self
            .now()
            .plus_millis(i64::try_from(FIRST_CHECK_AFTER.as_millis()).unwrap_or(60_000));
        self.updates.next_check_ms.store(first.0, Ordering::SeqCst);
        let app = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let wait = app.update_round().await;
                tokio::select! {
                    () = app.updates.wake.notified() => {}
                    () = tokio::time::sleep(wait) => {}
                }
            }
        });
    }

    fn wake_updates(&self) {
        self.updates.wake.notify_one();
    }

    /// One pass: drives the open run, else checks `main` when due. Returns how long to sleep.
    pub async fn update_round(self: &Arc<Self>) -> Duration {
        let Some(services) = self.update_services() else {
            return IDLE_TICK;
        };
        let run = match self.deps.store.open_update_run().await {
            Ok(run) => run,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the update run");
                return TICK;
            }
        };
        if let Some(run) = run {
            self.drive_update(services, run).await;
            return TICK;
        }
        // No open run: nothing may stay held (a run ended while this server was down) or frozen,
        // and deploys may start (also after a pass that closed the gate as a cancel landed).
        self.open_deploys();
        if self.update_holds_starts() {
            self.switch_hold(false).await;
            self.drain_queue().await;
        }
        if self.updates.frozen.lock().is_ok_and(|f| !f.is_empty()) {
            self.thaw_frozen_runs().await;
        }
        let now = self.now().0;
        if let Some(every) = services.check_every {
            let due = self.updates.next_check_ms.load(Ordering::SeqCst);
            if now >= due {
                self.check_releases(None).await;
                let next = now.saturating_add(i64::try_from(every.as_millis()).unwrap_or(i64::MAX));
                self.updates.next_check_ms.store(next, Ordering::SeqCst);
            }
            let left = self.updates.next_check_ms.load(Ordering::SeqCst) - now;
            return Duration::from_millis(u64::try_from(left.max(1_000)).unwrap_or(1_000))
                .min(IDLE_TICK);
        }
        IDLE_TICK
    }

    /// The GitHub App's token for Quartio-Studio/yulius (read).
    async fn app_release_token(&self) -> AppResult<Secret> {
        let app = self.github_app().await?;
        let instance = self
            .deps
            .store
            .instance()
            .await?
            .ok_or_else(|| AppError::Conflict("instance not initialized".into()))?;
        let own = instance
            .owner_login
            .as_deref()
            .is_some_and(|o| o.eq_ignore_ascii_case(policy::REPO_OWNER));
        let installation = match instance.installation_id {
            Some(id) if own => id,
            _ => self
                .deps
                .github
                .find_installation(&app, policy::REPO_OWNER)
                .await?
                .ok_or_else(|| {
                    AppError::Conflict(format!(
                        "the GitHub App is not installed on {}",
                        policy::REPO_OWNER
                    ))
                })?,
        };
        self.deps
            .github
            .repository_token(&app, installation, policy::REPO_NAME)
            .await
    }

    /// The credentials to try, best first.
    async fn release_tokens(
        &self,
        services: &UpdateServices,
    ) -> Vec<(Option<Secret>, &'static str)> {
        let mut tokens = Vec::new();
        if let Ok(token) = self.app_release_token().await {
            tokens.push((Some(token), "app"));
        }
        if let Some(token) = &services.read_token {
            tokens.push((Some(token.clone()), "token"));
        }
        if tokens.is_empty() {
            tokens.push((None, "anonymous"));
        }
        tokens
    }

    /// Reads `main` now and caches the result; tells viewers when the offered update changed.
    /// With `reuse_within`, a result stored meanwhile (by the check this one waited for) that is
    /// younger than that many milliseconds is kept instead.
    async fn check_releases(&self, reuse_within: Option<i64>) {
        let Some(services) = self.update_services() else {
            return;
        };
        let Some(source) = services.source.clone() else {
            return;
        };
        let _guard = self.updates.checking.lock().await;
        if let Some(gap) = reuse_within
            && self.last_check_within(gap)
        {
            return;
        }
        let mut outcome: Option<(Result<ReleaseState, SourceError>, &'static str)> = None;
        for (token, via) in self.release_tokens(services).await {
            let result = source
                .compare(token.as_ref(), services.build.as_deref())
                .await;
            let retry = matches!(result, Err(SourceError::NoAccess));
            outcome = Some((result, via));
            if !retry {
                break;
            }
        }
        let Some((result, via)) = outcome else {
            return;
        };
        let result = result.map_err(|err| match err {
            SourceError::NoAccess => format!(
                "Cannot read {}/{}: install the GitHub App on it (Contents: read) or set \
                 YULIUS_UPDATE_TOKEN_FILE.",
                policy::REPO_OWNER,
                policy::REPO_NAME
            ),
            other => format!("{other}."),
        });
        if let Err(err) = &result {
            tracing::info!(error = %err, "update check failed");
        }
        let offered = |r: &Result<ReleaseState, String>| {
            r.as_ref()
                .ok()
                .filter(|s| s.relation.offers_update())
                .map(|s| s.head.clone())
        };
        let changed = {
            let Ok(mut slot) = self.updates.check.lock() else {
                return;
            };
            let before = slot.as_ref().and_then(|s| offered(&s.result));
            let after = offered(&result);
            *slot = Some(CheckSnapshot {
                at: self.now(),
                via,
                result,
            });
            before != after
        };
        if changed {
            self.publish_update(None);
        }
    }

    fn publish_update(&self, phase: Option<&str>) {
        self.events.publish(
            None,
            false,
            UiEvent::UpdateChanged {
                phase: phase.map(str::to_owned),
            },
        );
    }

    /// Publishes a run's progress when something visible changed.
    fn publish_run(&self, run: &UpdateRunRecord, detail: &str) {
        let key = format!("{}:{}:{detail}", run.id, run.state.as_str());
        if let Ok(mut last) = self.updates.published.lock() {
            if *last == key {
                return;
            }
            *last = key;
        }
        let phase = (!run.state.is_terminal()).then(|| run.state.phase());
        self.publish_update(phase);
    }

    /// `POST /api/admin/update/check` (superadmin): reads `main` now (a check younger than
    /// 30 s is reused, also one that ended while this one waited for it).
    ///
    /// # Errors
    /// `Forbidden` for others.
    pub async fn check_updates_now(&self, ctx: &AuthContext) -> AppResult<UpdateView> {
        self.require_superadmin(ctx)?;
        if !self.last_check_within(MANUAL_CHECK_GAP_MS) {
            self.check_releases(Some(MANUAL_CHECK_GAP_MS)).await;
        }
        self.update_view(ctx).await
    }

    /// Whether the last check is younger than `gap` milliseconds.
    fn last_check_within(&self, gap: i64) -> bool {
        self.updates.check.lock().is_ok_and(|slot| {
            slot.as_ref()
                .is_some_and(|s| self.now().millis_since(s.at) < gap)
        })
    }

    /// `POST /api/admin/update` (superadmin): installs the offered commit. Tasks keep working
    /// while it builds; they pause only for the restart.
    ///
    /// # Errors
    /// `Forbidden` for others; `Invalid` without confirmation or for a malformed commit;
    /// `Conflict` without a helper, while a run is open, or for anything but the offered head of
    /// `main`.
    pub async fn start_update(
        &self,
        ctx: &AuthContext,
        req: &StartUpdate,
    ) -> AppResult<UpdateView> {
        self.require_superadmin(ctx)?;
        if !req.confirm {
            return Err(AppError::Invalid(
                "confirm the update: tasks pause while the server restarts".into(),
            ));
        }
        let commit = req.commit.trim();
        if !policy::is_commit(commit) {
            return Err(AppError::Invalid(
                "the commit must be a full 40-character id".into(),
            ));
        }
        let services = self
            .update_services()
            .ok_or_else(|| AppError::Conflict("updates are not available on this server".into()))?;
        let helper = services
            .helper
            .clone()
            .filter(|h| h.installed())
            .ok_or_else(|| {
                AppError::Conflict(
                    "the update helper is not installed on this server (see the operations guide)"
                        .into(),
                )
            })?;
        let (summary, via) = {
            let slot = self.updates.check.lock().map_err(AppError::internal)?;
            let snapshot = slot.as_ref().ok_or_else(|| {
                AppError::Conflict("no update is on offer yet; check for updates first".into())
            })?;
            let state = snapshot
                .result
                .as_ref()
                .ok()
                .filter(|s| s.relation.offers_update() && s.head == commit)
                .ok_or_else(|| {
                    AppError::Conflict(format!(
                        "{} is not the update on offer; check for updates again",
                        policy::short(commit)
                    ))
                })?;
            let summary = policy::summary(state.commits.iter().map(|c| c.message.as_str()))
                .unwrap_or_else(|| format!("Update to {}", policy::short(commit)));
            (summary, snapshot.via)
        };
        if services.build.as_deref() == Some(commit) {
            return Err(AppError::Conflict(
                "this server already runs that commit".into(),
            ));
        }
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        let run = UpdateRunRecord {
            id: self.deps.crypto.new_id(),
            target_commit: commit.to_owned(),
            from_commit: services.build.clone(),
            summary,
            state: RunState::Building,
            outcome: None,
            error: None,
            requested_by: login.clone(),
            created_at: now,
            state_since: now,
            finished_at: None,
        };
        if !self.deps.store.insert_update_run(&run).await? {
            return Err(AppError::Conflict("an update is already running".into()));
        }
        self.reset_phase(&run);
        let token = self.helper_token(services, via).await;
        if let Err(err) = self.write_request(helper.as_ref(), &run, HelperAction::Build, token) {
            let message = format!("Could not ask the update helper: {err}");
            let _ = self
                .deps
                .store
                .move_update_run(
                    &run.id,
                    RunState::Building,
                    RunState::Failed,
                    None,
                    Some(&message),
                    now,
                )
                .await;
            self.publish_update(None);
            return Err(AppError::Upstream(message));
        }
        self.deps
            .store
            .audit(now, &login, "update.started", Some(commit), Some(&run.id))
            .await?;
        self.publish_run(&run, "requested");
        self.wake_updates();
        self.update_view(ctx).await
    }

    /// `POST /api/admin/update/cancel` (superadmin): stops an update that is building or pausing
    /// tasks; nothing is installed and paused tasks go on at once.
    ///
    /// # Errors
    /// `Forbidden` for others; `Conflict` when no run can be cancelled now.
    pub async fn cancel_update(&self, ctx: &AuthContext) -> AppResult<UpdateView> {
        self.require_superadmin(ctx)?;
        let run = self
            .deps
            .store
            .open_update_run()
            .await?
            .filter(|r| matches!(r.state, RunState::Building | RunState::Draining))
            .ok_or_else(|| {
                AppError::Conflict(
                    "no update can be cancelled now (it is restarting or done)".into(),
                )
            })?;
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        let message = format!("Cancelled by {login}; nothing was installed.");
        if !self
            .deps
            .store
            .move_update_run(
                &run.id,
                run.state,
                RunState::Cancelled,
                None,
                Some(&message),
                now,
            )
            .await?
        {
            return Err(AppError::Conflict("the update moved on; try again".into()));
        }
        self.end_update_hold().await;
        if let Some(helper) = self.update_services().and_then(|s| s.helper.as_ref()) {
            helper.clear_request();
        }
        self.deps
            .store
            .audit(
                now,
                &login,
                "update.cancelled",
                Some(&run.target_commit),
                Some(&run.id),
            )
            .await?;
        self.publish_update(None);
        self.update_view(ctx).await
    }

    /// `POST /api/admin/update/proceed` (superadmin): stops waiting for tasks still in a turn
    /// and restarts now (they may see a short interruption).
    ///
    /// # Errors
    /// `Forbidden` for others; `Conflict` unless the run is pausing tasks.
    pub async fn proceed_update(&self, ctx: &AuthContext) -> AppResult<UpdateView> {
        self.require_superadmin(ctx)?;
        let run = self
            .deps
            .store
            .open_update_run()
            .await?
            .filter(|r| r.state == RunState::Draining)
            .ok_or_else(|| AppError::Conflict("the update is not pausing tasks".into()))?;
        self.updates.proceed.store(true, Ordering::SeqCst);
        let login = self.login_of(&ctx.user.id).await;
        self.deps
            .store
            .audit(
                self.now(),
                &login,
                "update.proceeded",
                Some(&run.target_commit),
                Some(&run.id),
            )
            .await?;
        self.wake_updates();
        self.update_view(ctx).await
    }

    fn reset_phase(&self, run: &UpdateRunRecord) {
        let key = format!("{}:{}", run.id, run.state.as_str());
        if let Ok(mut phase) = self.updates.phase_key.lock()
            && *phase != key
        {
            *phase = key;
            self.updates.writes.store(0, Ordering::SeqCst);
            self.updates.ready_passes.store(0, Ordering::SeqCst);
        }
    }

    fn write_request(
        &self,
        helper: &dyn UpdateHelper,
        run: &UpdateRunRecord,
        action: HelperAction,
        token: Option<Secret>,
    ) -> Result<(), String> {
        self.updates.writes.fetch_add(1, Ordering::SeqCst);
        helper.request(&HelperRequest {
            version: yulius_contracts::update::VERSION,
            id: request_id(run, action),
            action,
            commit: run.target_commit.clone(),
            token: token.map(|t| t.0),
            requested_at: self.now().0,
        })
    }

    /// The helper's status for this run's request, if it has acknowledged it.
    fn helper_status(&self, run: &UpdateRunRecord, action: HelperAction) -> Option<HelperStatus> {
        let helper = self.update_services()?.helper.as_ref()?;
        helper
            .status()
            .filter(|s| s.id == request_id(run, action) && s.commit == run.target_commit)
    }

    /// No acknowledgement yet: writes the request again every [`ACK_RETRY_MS`]; `true` once the
    /// helper has not answered for [`ACK_GIVE_UP_MS`].
    async fn await_ack(&self, run: &UpdateRunRecord, action: HelperAction) -> bool {
        let Some(helper) = self.update_services().and_then(|s| s.helper.clone()) else {
            return true;
        };
        let age = self.now().millis_since(run.state_since);
        let writes = i64::from(self.updates.writes.load(Ordering::SeqCst));
        if age >= writes.saturating_mul(ACK_RETRY_MS) {
            let token = match (action, self.update_services()) {
                (HelperAction::Build, Some(services)) => {
                    let via = self
                        .updates
                        .check
                        .lock()
                        .ok()
                        .and_then(|s| s.as_ref().map(|s| s.via))
                        .unwrap_or("app");
                    self.helper_token(services, via).await
                }
                _ => None,
            };
            if let Err(err) = self.write_request(helper.as_ref(), run, action, token) {
                tracing::warn!(error = %err, "could not write the update request");
            }
        }
        // A helper still busy with an earlier request is not unresponsive.
        let busy = helper.status().is_some_and(|s| {
            s.state == HelperState::Running
                && self.now().0.saturating_sub(s.updated_at) < ACK_RETRY_MS * 3
        });
        age > ACK_GIVE_UP_MS && !busy
    }

    /// The token the helper fetches with: what read `main` at the last check.
    async fn helper_token(&self, services: &UpdateServices, via: &str) -> Option<Secret> {
        match via {
            "app" => match self.app_release_token().await {
                Ok(token) => Some(token),
                Err(_) => services.read_token.clone(),
            },
            "token" => services.read_token.clone(),
            _ => None,
        }
    }

    async fn move_run(
        &self,
        run: &UpdateRunRecord,
        to: RunState,
        outcome: Option<RunState>,
        error: Option<&str>,
    ) -> bool {
        match self
            .deps
            .store
            .move_update_run(&run.id, run.state, to, outcome, error, self.now())
            .await
        {
            Ok(moved) => moved,
            Err(err) => {
                tracing::warn!(error = %err, "could not move the update run");
                false
            }
        }
    }

    async fn drive_update(self: &Arc<Self>, services: &UpdateServices, run: UpdateRunRecord) {
        self.reset_phase(&run);
        // The building run closes the gate itself once built (SPEC-342 DPU-007).
        match run.state {
            RunState::Draining | RunState::Installing => {
                self.updates.deploys.set_closed(true);
            }
            RunState::Resuming => {
                self.updates.deploys.set_closed(false);
            }
            _ => {}
        }
        if run.state.holds_tasks() {
            self.switch_hold(true).await;
        }
        let age = self.now().millis_since(run.state_since);
        match run.state {
            RunState::Building => {
                match self.helper_status(&run, HelperAction::Build) {
                    None if self.await_ack(&run, HelperAction::Build).await => {
                        self.fail_run(&run, "The update helper did not respond. Is yulius-update.path enabled on the server?").await;
                    }
                    // Deploys go on while it builds (SPEC-342 DPU-007).
                    None => self.open_deploys(),
                    Some(status) => match status.state {
                        HelperState::Running => {
                            self.open_deploys();
                            let detail = format!(
                                "{}:{}",
                                status.step,
                                status.progress.map_or(0, |p| p.done)
                            );
                            self.publish_run(&run, &detail);
                        }
                        HelperState::Done => {
                            // Deploys running now finish before anything pauses (SPEC-342).
                            let in_flight = self.deploys_in_flight().await;
                            if !matches!(in_flight, Ok((0, _))) {
                                self.wait_for_deploys(&run, &in_flight).await;
                                return;
                            }
                            self.updates.deploys_wait.reset();
                            if let Some(helper) = &services.helper {
                                helper.clear_request();
                            }
                            self.updates.proceed.store(false, Ordering::SeqCst);
                            if self.move_run(&run, RunState::Draining, None, None).await {
                                self.switch_hold(true).await;
                                let _ = self
                                    .deps
                                    .store
                                    .audit(
                                        self.now(),
                                        "system",
                                        "update.built",
                                        Some(&run.target_commit),
                                        Some(&run.id),
                                    )
                                    .await;
                                self.publish_update(Some(RunState::Draining.phase()));
                            }
                        }
                        HelperState::Failed | HelperState::RolledBack => {
                            let error = status
                                .error
                                .unwrap_or_else(|| "The build failed.".to_owned());
                            self.fail_run(&run, &error).await;
                        }
                    },
                }
                if age > BUILD_MAX_MS {
                    self.fail_run(&run, "The build did not finish within 3 hours.")
                        .await;
                }
            }
            RunState::Draining => {
                self.freeze_pass(&run.id).await;
                let progress = self.pause_progress().await;
                // A deploy is never cut, not even on "Continue without waiting" (DPU-005).
                let deploys = match self.deploys_in_flight().await {
                    Ok((count, _)) => Some(count),
                    Err(err) => {
                        tracing::warn!(error = %err, "could not read the running deploys");
                        None
                    }
                };
                let no_deploy = deploys == Some(0);
                let proceed = self.updates.proceed.load(Ordering::SeqCst) && no_deploy;
                let ready = no_deploy && (proceed || progress.paused >= progress.total);
                let passes = if ready {
                    self.updates.ready_passes.fetch_add(1, Ordering::SeqCst) + 1
                } else {
                    self.updates.ready_passes.store(0, Ordering::SeqCst);
                    0
                };
                self.publish_run(
                    &run,
                    &format!("{}/{}/{deploys:?}", progress.paused, progress.total),
                );
                if passes >= READY_PASSES || proceed {
                    let Some(helper) = services.helper.clone() else {
                        self.cancel_run(&run, "The update helper is gone.").await;
                        return;
                    };
                    if self.move_run(&run, RunState::Installing, None, None).await {
                        let moved = UpdateRunRecord {
                            state: RunState::Installing,
                            state_since: self.now(),
                            ..run.clone()
                        };
                        self.reset_phase(&moved);
                        if let Err(err) =
                            self.write_request(helper.as_ref(), &moved, HelperAction::Install, None)
                        {
                            tracing::warn!(error = %err, "could not write the install request");
                        }
                        let _ = self
                            .deps
                            .store
                            .audit(
                                self.now(),
                                "system",
                                "update.installing",
                                Some(&run.target_commit),
                                Some(&run.id),
                            )
                            .await;
                        self.publish_update(Some(RunState::Installing.phase()));
                    }
                } else if age > DRAIN_MAX_MS {
                    // Counted as readiness is: `waiting` is the page's capped list (#533).
                    let message = if let Some(running @ 1..) = deploys {
                        format!(
                            "{running} deploy(s) were still running after 30 minutes; nothing \
                             was installed."
                        )
                    } else if deploys.is_none() {
                        "The running deploys could not be read for 30 minutes; nothing was \
                         installed."
                            .to_owned()
                    } else {
                        format!(
                            "{} task(s) were still in a turn after 30 minutes; nothing was \
                             installed.",
                            progress.total.saturating_sub(progress.paused)
                        )
                    };
                    self.cancel_run(&run, &message).await;
                }
            }
            RunState::Installing => {
                let finished = match self.helper_status(&run, HelperAction::Install) {
                    None if self.await_ack(&run, HelperAction::Install).await => Some((
                        RunState::Failed,
                        Some(
                            "The update helper did not respond; nothing was installed.".to_owned(),
                        ),
                    )),
                    None => None,
                    Some(status) => match status.state {
                        HelperState::Running => {
                            self.publish_run(&run, &status.step);
                            None
                        }
                        HelperState::Done
                            if services.build.as_deref() == Some(run.target_commit.as_str()) =>
                        {
                            Some((RunState::Succeeded, None))
                        }
                        HelperState::Done => Some((
                            RunState::Failed,
                            Some(format!(
                                "The helper reported success, but this server still runs {}.",
                                services
                                    .build
                                    .as_deref()
                                    .map_or("an unknown build", policy::short)
                            )),
                        )),
                        HelperState::RolledBack => Some((
                            RunState::RolledBack,
                            Some(
                                status
                                    .error
                                    .unwrap_or_else(|| "The new version failed its check.".into()),
                            ),
                        )),
                        HelperState::Failed => Some((
                            RunState::Failed,
                            Some(status.error.unwrap_or_else(|| "Installing failed.".into())),
                        )),
                    },
                };
                let finished = finished.or_else(|| {
                    (age > INSTALL_MAX_MS).then(|| {
                        (
                            RunState::Failed,
                            Some("No result from the update helper within 15 minutes.".to_owned()),
                        )
                    })
                });
                if let Some((outcome, error)) = finished
                    && self
                        .move_run(&run, RunState::Resuming, Some(outcome), error.as_deref())
                        .await
                {
                    self.publish_update(Some(RunState::Resuming.phase()));
                    self.wake_updates();
                }
            }
            RunState::Resuming => self.resume_after_update(services, &run).await,
            RunState::Succeeded | RunState::Failed | RunState::RolledBack | RunState::Cancelled => {
            }
        }
    }

    async fn fail_run(&self, run: &UpdateRunRecord, error: &str) {
        if self
            .move_run(run, RunState::Failed, None, Some(error))
            .await
        {
            self.open_deploys();
            if let Some(helper) = self.update_services().and_then(|s| s.helper.as_ref()) {
                helper.clear_request();
            }
            let _ = self
                .deps
                .store
                .audit(
                    self.now(),
                    "system",
                    "update.failed",
                    Some(&run.target_commit),
                    Some(&run.id),
                )
                .await;
            self.publish_update(None);
        }
    }

    async fn cancel_run(&self, run: &UpdateRunRecord, error: &str) {
        if self
            .move_run(run, RunState::Cancelled, None, Some(error))
            .await
        {
            self.end_update_hold().await;
            if let Some(helper) = self.update_services().and_then(|s| s.helper.as_ref()) {
                helper.clear_request();
            }
            let _ = self
                .deps
                .store
                .audit(
                    self.now(),
                    "system",
                    "update.cancelled",
                    Some(&run.target_commit),
                    Some(&run.id),
                )
                .await;
            self.publish_update(None);
        }
    }

    /// Ends the hold: every task goes on (frozen runs are thawed), queued tasks start, paused
    /// capacity is re-evaluated.
    async fn end_update_hold(&self) {
        self.open_deploys();
        self.thaw_frozen_runs().await;
        self.switch_hold(false).await;
        self.updates.proceed.store(false, Ordering::SeqCst);
        self.capacity_changed();
    }

    /// A permit to start a deploy (SPEC-342 DPU-001/002), held until its run is recorded as
    /// running: refused from the end of an update's build until its install ended.
    ///
    /// # Errors
    /// `Conflict` while an update is about to restart the server.
    pub fn deploy_permit(&self) -> AppResult<DeployPermit> {
        self.updates
            .deploys
            .permit()
            .ok_or_else(|| AppError::Conflict(DEPLOY_REFUSED.into()))
    }

    /// How many deploys hold a permit and are not recorded yet (tests, diagnostics).
    #[must_use]
    pub fn deploys_starting(&self) -> u32 {
        self.updates.deploys.starting()
    }

    fn open_deploys(&self) {
        self.updates.deploys.set_closed(false);
        self.updates.deploys_wait.reset();
    }

    /// Closes the deploy gate, then counts the deploys still running or starting and lists the
    /// running ones (SPEC-342 DPU-002): closing first, nothing that passed the gate is missed.
    async fn deploys_in_flight(&self) -> AppResult<(usize, Vec<WaitingDeploy>)> {
        self.updates.deploys.set_closed(true);
        self.listed_deploys().await
    }

    /// The deploys running or starting, and the running ones as the page lists them.
    async fn listed_deploys(&self) -> AppResult<(usize, Vec<WaitingDeploy>)> {
        let starting = self.updates.deploys.starting() as usize;
        let store = &self.deps.store;
        let mut runs: Vec<(yulius_domain::ids::SandboxId, String, &str, String)> = store
            .running_cd_runs()
            .await?
            .into_iter()
            .map(|r| (r.sandbox_id, r.environment, "cd", r.id))
            .collect();
        runs.extend(
            store
                .running_environment_runs()
                .await?
                .into_iter()
                .map(|r| (r.sandbox_id, r.environment, "environment", r.id)),
        );
        let total = runs.len() + starting;
        let mut shown = Vec::new();
        for (sandbox, environment, kind, run) in runs.into_iter().take(MAX_WAITING_SHOWN) {
            let sandbox = store
                .sandbox(&sandbox)
                .await
                .ok()
                .flatten()
                .map_or_else(|| sandbox.to_string(), |s| s.slug);
            shown.push(WaitingDeploy {
                sandbox,
                environment,
                kind: kind.to_owned(),
                run,
            });
        }
        Ok((total, shown))
    }

    /// The build is done and deploys still run (or could not be read: counted as one): the run
    /// stays `building`, tasks keep working, and past the bound the update is cancelled
    /// (SPEC-342 DPU-003/004).
    async fn wait_for_deploys(
        &self,
        run: &UpdateRunRecord,
        in_flight: &AppResult<(usize, Vec<WaitingDeploy>)>,
    ) {
        let count = match in_flight {
            Ok((count, _)) => Some(*count),
            Err(err) => {
                tracing::warn!(error = %err, "could not read the running deploys");
                None
            }
        };
        let now = self.now().0;
        let since = self.updates.deploys_wait.since(&run.id, now);
        self.publish_run(run, &format!("built:deploys:{count:?}"));
        let bound = self.deploy_wait_max_ms();
        if now.saturating_sub(since) > bound {
            let minutes = bound / 60_000;
            let message = match count {
                Some(count) => format!(
                    "{count} deploy(s) were still running {minutes} minutes after the build; \
                     nothing was installed."
                ),
                None => format!(
                    "The running deploys could not be read for {minutes} minutes after the \
                     build; nothing was installed."
                ),
            };
            self.cancel_run(run, &message).await;
        }
    }

    /// The longest wait for running deploys after the build (SPEC-342 DPU-004): a CD run waits
    /// up to the job timeout for its agent states and then deploys for up to the job timeout; an
    /// environment run prepares for up to 35 minutes and then runs for up to the job timeout.
    fn deploy_wait_max_ms(&self) -> i64 {
        let timeout = self.config.ci.job_timeout;
        let longest = (timeout * 2).max(timeout + crate::delivery::PREPARE_ALLOWANCE);
        i64::try_from(longest.as_millis())
            .unwrap_or(i64::MAX)
            .saturating_add(DEPLOY_WAIT_SLACK_MS)
    }

    /// Releases the held tasks one by one, then ends the run with its outcome.
    async fn resume_after_update(
        self: &Arc<Self>,
        services: &UpdateServices,
        run: &UpdateRunRecord,
    ) {
        let attempts = self.deps.store.live_attempts().await.unwrap_or_default();
        let held = self
            .updates
            .held_at
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default();
        let mut live: Vec<(i64, TaskId)> = attempts
            .iter()
            .map(|a| {
                (
                    held.get(&a.task_id).copied().unwrap_or(a.started_at.0),
                    a.task_id.clone(),
                )
            })
            .collect();
        live.sort();
        live.dedup_by(|a, b| a.1 == b.1);
        let total = u32::try_from(live.len()).unwrap_or(u32::MAX);
        self.updates.resume_total.store(total, Ordering::SeqCst);
        self.updates.resumed.store(0, Ordering::SeqCst);
        for (index, (_, task)) in live.iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(services.resume_stagger).await;
            }
            if let Ok(mut released) = self.updates.released.lock() {
                released.insert(task.clone());
            }
            // Released first: its first hook after the thaw is not held. Under the lock, so a
            // person's pause cannot land between its check and the thaw.
            {
                let _pass = self.freeze_lock().await;
                let kept = self.person_frozen_runs().await.unwrap_or_default();
                for attempt in attempts.iter().filter(|a| a.task_id == *task) {
                    self.thaw_run(&attempt.run_dir, &kept).await;
                }
            }
            if let Ok(mut frozen) = self.updates.frozen.lock() {
                frozen.remove(task);
            }
            self.publish_task(task).await;
            let done = u32::try_from(index + 1).unwrap_or(u32::MAX);
            self.updates.resumed.store(done, Ordering::SeqCst);
            self.publish_run(run, &format!("{done}/{total}"));
        }
        let outcome = run.outcome.unwrap_or(RunState::Succeeded);
        if self.move_run(run, outcome, None, None).await {
            self.end_update_hold().await;
            if let Some(helper) = &services.helper {
                helper.clear_request();
            }
            let _ = self
                .deps
                .store
                .audit(
                    self.now(),
                    "system",
                    "update.finished",
                    Some(&run.target_commit),
                    Some(outcome.as_str()),
                )
                .await;
            self.publish_update(None);
            // New starts and resumes that waited for the update.
            self.drain_queue().await;
        }
    }

    /// Live tasks and which of them are at a pause point.
    async fn pause_progress(&self) -> PauseProgress {
        let store = &self.deps.store;
        let now = self.now().0;
        let held = self
            .updates
            .held_at
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default();
        let frozen: HashSet<TaskId> = self
            .updates
            .frozen
            .lock()
            .map(|f| {
                f.iter()
                    .filter(|(_, run)| run.state == FreezeState::Frozen)
                    .map(|(task, _)| task.clone())
                    .collect()
            })
            .unwrap_or_default();
        let settling: HashSet<TaskId> = self
            .updates
            .frozen
            .lock()
            .map(|f| {
                f.iter()
                    .filter(|(_, run)| matches!(run.state, FreezeState::Freezing(_)))
                    .map(|(task, _)| task.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut tasks: Vec<TaskId> = store
            .live_attempts()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.task_id)
            .collect();
        tasks.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        tasks.dedup();
        let mut paused = 0_u32;
        let mut waiting = Vec::new();
        for id in &tasks {
            if frozen.contains(id) {
                paused += 1;
                continue;
            }
            let Ok(Some(task)) = store.task(id).await else {
                paused += 1;
                continue;
            };
            let recently_held = held
                .get(id)
                .is_some_and(|at| now.saturating_sub(*at) <= HELD_FRESH_MS);
            if policy::at_pause_point(task.status, task.activity, recently_held) {
                paused += 1;
            } else if !settling.contains(id) && waiting.len() < MAX_WAITING_SHOWN {
                // A run being frozen is not "still in a turn" (OPS-014).
                let sandbox = store
                    .sandbox(&task.sandbox_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|s| s.slug)
                    .unwrap_or_default();
                waiting.push(WaitingTask {
                    id: task.id.to_string(),
                    number: task.number,
                    title: task.title.clone(),
                    sandbox,
                });
            }
        }
        let total = u32::try_from(tasks.len()).unwrap_or(u32::MAX);
        PauseProgress {
            paused,
            total,
            waiting,
            deadline: None,
        }
    }

    /// `GET /api/update`: the build and a running update's phase for everyone; the offered
    /// update, the last check and the run for superadmins.
    ///
    /// # Errors
    /// Store failures.
    pub async fn update_view(&self, ctx: &AuthContext) -> AppResult<UpdateView> {
        let services = self.update_services();
        let build = services.and_then(|s| s.build.as_deref());
        let open = self.deps.store.open_update_run().await?;
        let mut view = UpdateView {
            current: build_view(build),
            phase: open.as_ref().map(|r| r.state.phase().to_owned()),
            ..UpdateView::default()
        };
        if !ctx.user.is_superadmin {
            return Ok(view);
        }
        view.helper_installed = Some(
            services
                .and_then(|s| s.helper.as_ref())
                .is_some_and(|h| h.installed()),
        );
        let snapshot = self.updates.check.lock().ok().and_then(|s| s.clone());
        view.check = Some(match &snapshot {
            None => UpdateCheckView::default(),
            Some(s) => UpdateCheckView {
                checked_at: Some(rfc3339(s.at)),
                error: s.result.as_ref().err().cloned(),
                via: Some(s.via.to_owned()),
                relation: s
                    .result
                    .as_ref()
                    .ok()
                    .map(|r| r.relation.as_str().to_owned()),
                head: s.result.as_ref().ok().map(|r| r.head.clone()),
            },
        });
        view.available = snapshot
            .as_ref()
            .and_then(|s| s.result.as_ref().ok())
            .filter(|r| r.relation.offers_update() && build != Some(r.head.as_str()))
            .map(available_update);
        let run = match open {
            Some(run) => Some(run),
            None => self.deps.store.last_update_run().await?,
        };
        if let Some(run) = run {
            view.run = Some(self.run_view(&run).await);
        }
        Ok(view)
    }

    async fn run_view(&self, run: &UpdateRunRecord) -> UpdateRunView {
        let action = match run.state {
            RunState::Building => Some(HelperAction::Build),
            RunState::Installing | RunState::Resuming => Some(HelperAction::Install),
            _ => match run.outcome.or(Some(run.state)) {
                Some(RunState::RolledBack | RunState::Succeeded) => Some(HelperAction::Install),
                _ => None,
            },
        };
        let status = action.and_then(|a| self.helper_status(run, a));
        let mut step = status.as_ref().map(|s| s.step.clone());
        let mut progress = status
            .as_ref()
            .and_then(|s| s.progress)
            .filter(|_| run.state == RunState::Building)
            .map(|p| ProgressView {
                done: p.done,
                total: p.total,
            });
        let mut tasks = None;
        let mut message = step.as_deref().and_then(step_message).map(str::to_owned);
        // The gate is closed once built: the deploys it waits for (SPEC-342 DPU-006).
        let (running, deploys) = if self.updates.deploys.is_closed()
            && matches!(run.state, RunState::Building | RunState::Draining)
        {
            match self.listed_deploys().await {
                Ok((count, shown)) => (Some(count), shown),
                Err(_) => (None, Vec::new()),
            }
        } else {
            (Some(0), Vec::new())
        };
        if run.state == RunState::Building {
            match running {
                Some(0) => {}
                Some(count) => {
                    message = Some(format!(
                        "Built. Waiting for {count} deploy(s) to finish before pausing tasks; new \
                         deploys are refused until the update installed."
                    ));
                }
                None => {
                    message = Some(
                        "Built. The running deploys could not be read; waiting before pausing \
                         tasks."
                            .into(),
                    );
                }
            }
        }
        match run.state {
            RunState::Draining => {
                let mut pause = self.pause_progress().await;
                pause.deadline = Some(rfc3339(run.state_since.plus_millis(DRAIN_MAX_MS)));
                // A deploy holds the install (SPEC-342 DPU-005).
                message = Some(match running {
                    Some(0) if pause.paused >= pause.total => {
                        "Every task is paused. Restarting next.".to_owned()
                    }
                    Some(0) => {
                        format!("Pausing tasks: {} of {} paused.", pause.paused, pause.total)
                    }
                    Some(count) => {
                        format!("Waiting for {count} deploy(s) to finish before restarting.")
                    }
                    None => "The running deploys could not be read; waiting before restarting."
                        .to_owned(),
                });
                step = Some("pausing".into());
                tasks = Some(pause);
            }
            RunState::Resuming => {
                let total = self.updates.resume_total.load(Ordering::SeqCst);
                let done = self.updates.resumed.load(Ordering::SeqCst);
                progress = Some(ProgressView { done, total });
                message = Some(format!("Resuming tasks: {done} of {total}."));
                step = Some("resuming".into());
            }
            RunState::Succeeded => {
                message = Some(format!("Updated to {}.", policy::short(&run.target_commit)));
            }
            RunState::RolledBack => {
                message = Some("The update was rolled back; the previous version runs.".into());
            }
            RunState::Failed => message = Some("The update failed.".into()),
            RunState::Cancelled => message = Some("Cancelled; nothing was installed.".into()),
            RunState::Building | RunState::Installing => {}
        }
        let docker = status.as_ref().and_then(|s| s.host_check.clone());
        let ended_badly = matches!(run.state, RunState::Failed | RunState::RolledBack)
            || matches!(run.outcome, Some(RunState::Failed | RunState::RolledBack));
        let log_tail = if ended_badly {
            status.map(|s| s.log_tail).unwrap_or_default()
        } else {
            Vec::new()
        };
        UpdateRunView {
            id: run.id.clone(),
            target: build_view(Some(&run.target_commit)),
            from: build_view(run.from_commit.as_deref()),
            summary: run.summary.clone(),
            state: run.state.phase().to_owned(),
            step,
            message,
            progress,
            tasks,
            deploys,
            error: run.error.clone(),
            log_tail,
            docker,
            requested_by: run.requested_by.clone(),
            started_at: rfc3339(run.created_at),
            state_since: rfc3339(run.state_since),
            finished_at: run.finished_at.map(rfc3339),
            can_cancel: matches!(run.state, RunState::Building | RunState::Draining),
            can_proceed: run.state == RunState::Draining && running == Some(0),
        }
    }
}

fn available_update(state: &ReleaseState) -> AvailableUpdate {
    let commits: Vec<CommitView> = state
        .commits
        .iter()
        .filter_map(|c| {
            policy::commit_title(&c.message).map(|title| CommitView {
                sha: c.sha.clone(),
                short: policy::short(&c.sha).to_owned(),
                title,
                author: c.author.clone(),
                date: c.date.clone(),
            })
        })
        .collect();
    let count = u32::try_from(commits.len()).unwrap_or(u32::MAX);
    let summary = policy::summary(state.commits.iter().map(|c| c.message.as_str()))
        .unwrap_or_else(|| format!("Update to {}", policy::short(&state.head)));
    AvailableUpdate {
        commit: state.head.clone(),
        short: policy::short(&state.head).to_owned(),
        summary,
        count,
        commits: commits.into_iter().take(MAX_COMMITS_SHOWN).collect(),
        relation: state.relation.as_str().to_owned(),
    }
}
