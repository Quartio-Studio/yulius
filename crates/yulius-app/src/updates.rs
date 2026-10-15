//! Instance updates from `main` of Quartio-Studio/yulius (SPEC-001 OPS-006..OPS-009, SPEC-002
//! ADR-18).
//!
//! **Detect.** Every hour (and on "Check now" or opening the instance page, at most every 30 s) the
//! build's commit is compared with `main` through the GitHub API, read with the GitHub App's token
//! for that repository, else the configured read token, else anonymously. The result is cached;
//! superadmins are told.
//!
//! **Update.** Nothing updates by itself. A superadmin confirms the offered commit, then one run
//! moves through `building` (the root helper builds the commit; tasks keep working) →
//! `draining` (every task pauses at its turn boundary: the Stop and `UserPromptSubmit` hooks are
//! held as for a capacity pause, ADR-17, and new starts queue) → `installing` (the helper stops
//! the service, backs up, installs, starts and checks the new build, rolling back on failure)
//! → `resuming` (the held tasks are released one by one) → an outcome. The run is a database row,
//! so the server that starts after the restart carries it on; the agents are never told.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;
use yulius_contracts::api::updates::{
    AvailableUpdate, BuildView, CommitView, PauseProgress, ProgressView, StartUpdate,
    UpdateCheckView, UpdateRunView, UpdateView, WaitingTask,
};
use yulius_contracts::events::UiEvent;
use yulius_contracts::update::{HelperAction, HelperRequest, HelperState, HelperStatus};
use yulius_domain::ids::TaskId;
use yulius_domain::time::Timestamp;
use yulius_domain::update::{self as policy, RunState};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::Secret;
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
/// Tasks listed as still in a turn.
const MAX_WAITING_SHOWN: usize = 20;
/// Consecutive passes that must find every task at a pause point before installing.
const READY_PASSES: u32 = 2;

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
        "built" => "Built. Pausing tasks at their next step.",
        "stopping" => "Stopping the server. Agent sessions keep running.",
        "backing_up" => "Backing up the database.",
        "installing" => "Installing the new version.",
        "starting" => "Starting the new version.",
        "checking" => "Checking the new version.",
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

    /// The commit this server was built from.
    #[must_use]
    pub fn build_commit(&self) -> Option<&str> {
        self.update_services().and_then(|s| s.build.as_deref())
    }

    /// Whether starts queue because an update pauses the tasks.
    pub(crate) fn update_holds_starts(&self) -> bool {
        self.updates.hold.load(Ordering::SeqCst)
    }

    /// Whether an update holds a hook of `task` now (it was held and not released yet).
    pub(crate) fn held_for_update(&self, task: &TaskId) -> bool {
        self.updates.hold.load(Ordering::SeqCst)
            && !self
                .updates
                .released
                .lock()
                .is_ok_and(|released| released.contains(task))
            && self
                .updates
                .held_at
                .lock()
                .is_ok_and(|held| held.contains_key(task))
    }

    /// Whether the turn boundary of `task` is held for an update; records that it is.
    pub(crate) fn hold_for_update(&self, task: &TaskId) -> bool {
        if !self.updates.hold.load(Ordering::SeqCst) {
            return false;
        }
        if self
            .updates
            .released
            .lock()
            .is_ok_and(|released| released.contains(task))
        {
            return false;
        }
        let now = self.now().0;
        if let Ok(mut held) = self.updates.held_at.lock() {
            held.insert(task.clone(), now);
        }
        // The agent is still held: a capacity pause of the same task must not count its hook as
        // given up (ADR-17's 45 s rule) because the update answered the polls meanwhile.
        self.runtime(task)
            .hold_polled_ms
            .store(now, Ordering::SeqCst);
        true
    }

    fn set_hold(&self, on: bool) {
        let was = self.updates.hold.swap(on, Ordering::SeqCst);
        if was && !on {
            if let Ok(mut released) = self.updates.released.lock() {
                released.clear();
            }
            if let Ok(mut held) = self.updates.held_at.lock() {
                held.clear();
            }
        }
    }

    /// At start: an open run that pauses tasks keeps them paused from the first hook call on
    /// (before the loop runs), so a restart in the middle of an update never lets an agent go on.
    pub async fn restore_update_hold(&self) {
        if let Ok(Some(run)) = self.deps.store.open_update_run().await
            && run.state.holds_tasks()
        {
            self.set_hold(true);
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
        // No open run: nothing may stay held (a run ended while this server was down).
        if self.update_holds_starts() {
            self.set_hold(false);
            self.drain_queue().await;
        }
        let now = self.now().0;
        if let Some(every) = services.check_every {
            let due = self.updates.next_check_ms.load(Ordering::SeqCst);
            if now >= due {
                self.check_releases().await;
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
    async fn check_releases(&self) {
        let Some(services) = self.update_services() else {
            return;
        };
        let Some(source) = services.source.clone() else {
            return;
        };
        let _guard = self.updates.checking.lock().await;
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
    /// 30 s is reused).
    ///
    /// # Errors
    /// `Forbidden` for others.
    pub async fn check_updates_now(&self, ctx: &AuthContext) -> AppResult<UpdateView> {
        self.require_superadmin(ctx)?;
        let fresh = self.updates.check.lock().ok().and_then(|slot| {
            slot.as_ref()
                .map(|s| self.now().millis_since(s.at) < MANUAL_CHECK_GAP_MS)
        });
        if fresh != Some(true) {
            self.check_releases().await;
        }
        self.update_view(ctx).await
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
        if run.state.holds_tasks() {
            self.set_hold(true);
        }
        let age = self.now().millis_since(run.state_since);
        match run.state {
            RunState::Building => {
                match self.helper_status(&run, HelperAction::Build) {
                    None if self.await_ack(&run, HelperAction::Build).await => {
                        self.fail_run(&run, "The update helper did not respond. Is yulius-update.path enabled on the server?").await;
                    }
                    None => {}
                    Some(status) => match status.state {
                        HelperState::Running => {
                            let detail = format!(
                                "{}:{}",
                                status.step,
                                status.progress.map_or(0, |p| p.done)
                            );
                            self.publish_run(&run, &detail);
                        }
                        HelperState::Done => {
                            if let Some(helper) = &services.helper {
                                helper.clear_request();
                            }
                            self.updates.proceed.store(false, Ordering::SeqCst);
                            if self.move_run(&run, RunState::Draining, None, None).await {
                                self.set_hold(true);
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
                let progress = self.pause_progress().await;
                let proceed = self.updates.proceed.load(Ordering::SeqCst);
                let ready = proceed || progress.waiting.is_empty();
                let passes = if ready {
                    self.updates.ready_passes.fetch_add(1, Ordering::SeqCst) + 1
                } else {
                    self.updates.ready_passes.store(0, Ordering::SeqCst);
                    0
                };
                self.publish_run(&run, &format!("{}/{}", progress.paused, progress.total));
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
                    let message = format!(
                        "{} task(s) were still in a turn after 30 minutes; nothing was installed.",
                        progress.waiting.len()
                    );
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

    /// Ends the hold: every task goes on, queued tasks start, paused capacity is re-evaluated.
    async fn end_update_hold(&self) {
        self.set_hold(false);
        self.updates.proceed.store(false, Ordering::SeqCst);
        self.capacity_changed();
    }

    /// Releases the held tasks one by one, then ends the run with its outcome.
    async fn resume_after_update(
        self: &Arc<Self>,
        services: &UpdateServices,
        run: &UpdateRunRecord,
    ) {
        let mut live: Vec<(i64, TaskId)> = match self.deps.store.live_attempts().await {
            Ok(attempts) => {
                let held = self
                    .updates
                    .held_at
                    .lock()
                    .map(|h| h.clone())
                    .unwrap_or_default();
                attempts
                    .into_iter()
                    .map(|a| {
                        (
                            held.get(&a.task_id).copied().unwrap_or(a.started_at.0),
                            a.task_id,
                        )
                    })
                    .collect()
            }
            Err(_) => Vec::new(),
        };
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
            let Ok(Some(task)) = store.task(id).await else {
                paused += 1;
                continue;
            };
            let recently_held = held
                .get(id)
                .is_some_and(|at| now.saturating_sub(*at) <= HELD_FRESH_MS);
            if policy::at_pause_point(task.status, task.activity, recently_held) {
                paused += 1;
            } else if waiting.len() < MAX_WAITING_SHOWN {
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
        match run.state {
            RunState::Draining => {
                let mut pause = self.pause_progress().await;
                pause.deadline = Some(rfc3339(run.state_since.plus_millis(DRAIN_MAX_MS)));
                message = Some(if pause.waiting.is_empty() {
                    "Every task is paused. Restarting next.".to_owned()
                } else {
                    format!(
                        "Pausing tasks at their next step: {} of {} paused.",
                        pause.paused, pause.total
                    )
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
            error: run.error.clone(),
            log_tail,
            requested_by: run.requested_by.clone(),
            started_at: rfc3339(run.created_at),
            state_since: rfc3339(run.state_since),
            finished_at: run.finished_at.map(rfc3339),
            can_cancel: matches!(run.state, RunState::Building | RunState::Draining),
            can_proceed: run.state == RunState::Draining,
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
