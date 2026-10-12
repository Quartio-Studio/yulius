//! The subscription pool at run time (SPEC-001 ROT-003/004/010/013, SPEC-002 ADR-17): pausing
//! tasks gently when no subscription has capacity, resuming them oldest first and staggered,
//! the provider readings behind the burn rate, and the capacity view with its forecast.
//!
//! A pause is a row in `capacity_pauses` plus the task status `waiting_capacity`; nothing is
//! ever typed into the agent to pause it. How the task is held depends on [`PauseKind`]:
//!
//! - `turn` / `prompt`: the agent sits at its turn boundary while the Stop or
//!   `UserPromptSubmit` hook answers "hold" (`202`) and the hook client keeps polling
//!   (`hooks.rs`); an agent without hooks is simply idle;
//! - `limit`: the CLI ended a turn with a usage limit and waits at its limit message; it always
//!   resumes by a native resume past the limit (SPEC-034), even on its own subscription;
//! - `start`: there is no process yet; the task starts when capacity returns.
//!
//! The resume loop ([`App::start_capacity_loop`]) wakes on new usage, a new or removed account,
//! a new pause, the earliest known reset, and every 5 minutes. It walks the open pauses oldest
//! first and resumes each task the policy places, waiting [`crate::context::AppConfig`]
//! `resume_stagger_ms` between two resumes: in place when the paused process's account has
//! capacity again (the held hook simply answers on its next poll), otherwise by a native resume
//! on the chosen account.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use yulius_contracts::api::admin::{CapacityAccount, CapacityView};
use yulius_contracts::hooks::{RateLimitWindow, RateLimits};
use yulius_domain::capacity::{ForecastInput, PoolMember, Span, WeeklySample};
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::rotation::{self, Decision, RotationInput, Trigger};
use yulius_domain::task::{Activity, TaskEvent, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, ModelFamily, UsageWindow};

use crate::accounts::{account_usage, in_use_count, usage_view};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::AppResult;
use crate::model::{
    AccountStatus, PauseKind, PauseRecord, TaskRecord, USAGE_FROM_SESSION, UsageSampleRecord,
};
use crate::supervisor::RunStart;

/// Default wait between two resumes of paused tasks ("resumed gently").
pub const RESUME_STAGGER_MS: u64 = 20_000;
/// The resume loop re-checks at least this often while nothing wakes it.
const RECHECK: Duration = Duration::from_secs(5 * 60);
/// Shortest sleep of the resume loop before the next pass.
const MIN_RECHECK: Duration = Duration::from_secs(1);
/// `Retry-After` of a held hook: how often the hook client asks again.
pub const HOLD_RETRY_SECS: u64 = 10;
/// Longest held prompt carried to another account as the resumed session's prompt (an argv
/// element; Linux limits one to 128 KiB).
pub const CONTINUATION_MAX: usize = 100 * 1024;
/// A held hook asks again every [`HOLD_RETRY_SECS`] (a restart: every 2 s); one nobody asked
/// about for this long was given up by the CLI (an interrupt, the CLI's own hook timeout — 15 s
/// for sessions started before ADR-17 — or a killed hook client): the agent is not held any more.
pub const HOLD_LOST_MS: i64 = 45_000;
/// While a hook is held, the resume loop looks at least this often whether it still is.
const HOLD_CHECK: Duration = Duration::from_secs(15);
/// Usage readings are kept this long (the forecast reads 7 days).
const SAMPLE_RETENTION_MS: i64 = 8 * 24 * 3_600_000;
const WEEK_MS: i64 = 7 * 24 * 3_600_000;

/// A session's usage reading is stored at once when a window moved by this many points (or
/// reset, or crossed the threshold); smaller moves at most every [`SESSION_WRITE_MS`].
const SESSION_STEP: f64 = 1.0;
/// Shortest time between two stored session readings of one subscription that moved less than
/// [`SESSION_STEP`].
const SESSION_WRITE_MS: i64 = 15_000;
/// Burn-rate samples from session readings: at most this often per subscription (and at every
/// window reset).
const SESSION_SAMPLE_MS: i64 = 5 * 60_000;
/// An unchanged session reading still refreshes when the usage was last seen, at most this
/// often, so a subscription in use never looks stale (F 2026-10-11).
const SESSION_TOUCH_MS: i64 = 60_000;
/// Latest reset time accepted from a session reading: a week and a day ahead.
const SESSION_RESET_MAX_MS: i64 = WEEK_MS + 24 * 3_600_000;

/// The last session reading stored for a subscription.
#[derive(Debug, Clone, Copy)]
struct SessionReading {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
    written: Timestamp,
    sampled: Timestamp,
}

/// Runtime state of the pool controller (owned by [`App`]).
#[derive(Default)]
pub struct CapacityState {
    wake: Notify,
    /// The last session reading stored per subscription (ADR-17: usage from the sessions).
    sessions: Mutex<HashMap<AccountId, SessionReading>>,
    /// When each subscription's usage may be asked for again (ROT-009 back-off), shared by the
    /// background round and on-demand reads so neither hammers the provider (F 2026-10-11).
    pub(crate) usage_not_before: tokio::sync::Mutex<HashMap<AccountId, Timestamp>>,
    /// The last usage round asked for by opening the subscriptions list (debounced).
    pub(crate) usage_asked: Mutex<Option<Timestamp>>,
}

/// A window of Claude Code's status line input as a usage window of `minutes`; `None` without a
/// sane percentage. A reset time in the past or more than a week and a day ahead is unknown.
fn session_window(
    window: Option<RateLimitWindow>,
    minutes: u32,
    now: Timestamp,
) -> Option<UsageWindow> {
    let window = window?;
    let used = window
        .used_percentage
        .filter(|p| p.is_finite() && (0.0..=1_000.0).contains(p))?;
    let resets_at = window
        .resets_at
        .and_then(|s| s.checked_mul(1_000))
        .map(Timestamp)
        .filter(|at| *at > now && at.millis_since(now) <= SESSION_RESET_MAX_MS);
    Some(UsageWindow {
        used_percent: Some(used),
        window_minutes: Some(minutes),
        resets_at,
    })
}

/// Whether a new reading of a window must be stored at once: it appeared, reset, moved by
/// [`SESSION_STEP`] or more, or crossed the switch threshold.
fn moved_much(before: Option<UsageWindow>, after: Option<UsageWindow>) -> bool {
    match (before, after) {
        (None, None) => false,
        (Some(b), Some(a)) => {
            let (b_used, a_used) = (
                b.known_percent().unwrap_or(0.0),
                a.known_percent().unwrap_or(0.0),
            );
            b.resets_at != a.resets_at
                || (a_used - b_used).abs() >= SESSION_STEP
                || (b_used < rotation::THRESHOLD) != (a_used < rotation::THRESHOLD)
        }
        _ => true,
    }
}

/// What happened to one paused task in a resume pass.
enum Resume {
    /// It resumed (in place, by a switch, or by starting).
    Resumed,
    /// Still no capacity for it; the earliest known time that may change.
    Waiting(Option<Timestamp>),
    /// The pause no longer applies (task gone or not paused); it was closed.
    Gone,
}

/// The next weekly reset of a window whose reset time may have passed (weekly windows reset at
/// the same time every week).
fn next_weekly_reset(window: &UsageWindow, now: Timestamp) -> Option<Timestamp> {
    let at = window.resets_at?;
    if at > now {
        return Some(at);
    }
    let weeks = now.millis_since(at) / WEEK_MS + 1;
    Some(at.plus_millis(weeks.saturating_mul(WEEK_MS)))
}

fn healthy(status: AccountStatus) -> bool {
    matches!(status, AccountStatus::Ok | AccountStatus::Limited)
}

impl App {
    /// Wakes the resume loop: usage changed, an account was added or removed, a task paused.
    pub fn capacity_changed(&self) {
        self.capacity.wake.notify_one();
    }

    /// The policy input for `task` (its provider and model family) with `current` as the
    /// account it runs on.
    ///
    /// # Errors
    /// Store failures.
    pub async fn rotation_input(
        &self,
        task: &TaskRecord,
        current: Option<AccountId>,
    ) -> AppResult<RotationInput> {
        Ok(RotationInput {
            current,
            task_provider: task.harness.provider(),
            // A task without a model runs the default, Opus: its weekly cap binds.
            family: ModelFamily::of(Some(
                task.model
                    .as_deref()
                    .unwrap_or(yulius_domain::harness::DEFAULT_CLAUDE_MODEL),
            )),
            accounts: self.account_views().await?,
            now: self.now(),
            threshold: rotation::THRESHOLD,
        })
    }

    /// The subscription a run outside any task (a split proposal, a Cockpit) runs on: the best
    /// placement of the pool for a run of `family`, whose weekly cap binds too.
    ///
    /// # Errors
    /// `Conflict` without any Claude subscription, `Capacity` while none has capacity.
    pub(crate) async fn pool_account(
        &self,
        provider: yulius_domain::harness::Provider,
        family: ModelFamily,
    ) -> AppResult<crate::model::AccountRecord> {
        let accounts = self.account_views().await?;
        if !accounts.iter().any(|a| a.provider == provider) {
            return Err(crate::error::AppError::Conflict(
                "no Claude account is connected; an instance administrator can add one".into(),
            ));
        }
        let input = RotationInput {
            current: None,
            task_provider: provider,
            family,
            accounts,
            now: self.now(),
            threshold: rotation::THRESHOLD,
        };
        match rotation::decide(&input, Trigger::Start).0 {
            Decision::Switch { account } => self
                .deps
                .store
                .account(&account)
                .await?
                .ok_or(crate::error::AppError::NotFound),
            Decision::Stay | Decision::Wait { .. } => Err(crate::error::AppError::Capacity(
                "no subscription has capacity right now".into(),
            )),
        }
    }

    /// Pauses a task for lack of capacity (ROT-010): records the pause, moves the task to
    /// `waiting_capacity` and tells viewers (never the agent). A task that is already paused
    /// keeps its pause, except that a usage limit the CLI reported turns it into a `limit`
    /// pause (the cut turn resumes past the limit, SPEC-034).
    ///
    /// # Errors
    /// Store failures.
    pub async fn pause_task(
        &self,
        task_id: &TaskId,
        kind: PauseKind,
        account: Option<AccountId>,
        continuation: Option<String>,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let Some(task) = store.task(task_id).await? else {
            return Ok(());
        };
        let now = self.now();
        let inserted = store
            .insert_pause(&PauseRecord {
                id: self.deps.crypto.new_id(),
                task_id: task_id.clone(),
                kind,
                account_id: account,
                continuation: continuation.filter(|c| c.len() <= CONTINUATION_MAX),
                started_at: now,
                ended_at: None,
                outcome: None,
            })
            .await?;
        if !inserted && kind == PauseKind::Limit {
            store.update_open_pause(task_id, kind, None).await?;
        }
        if task.status != TaskStatus::WaitingCapacity
            && let Ok(next) = task.status.apply(TaskEvent::CapacityExhausted)
        {
            store
                .transition_task(task_id, Some(task.status), next, None, now)
                .await?;
        }
        if inserted {
            store
                .audit(
                    now,
                    "system",
                    "task.paused",
                    Some(task_id.as_str()),
                    Some(kind.as_str()),
                )
                .await?;
            self.runtime(task_id)
                .hub
                .notice(
                "Yulius paused this task: no subscription has capacity left. It continues by itself.",
            );
        }
        self.publish_task(task_id).await;
        self.capacity_changed();
        Ok(())
    }

    /// Closes the open pause of a task, if any (the task resumed, ended or was stopped).
    pub(crate) async fn end_pause(&self, task_id: &TaskId, outcome: &str) {
        if let Err(err) = self
            .deps
            .store
            .end_pause(task_id, self.now(), outcome)
            .await
        {
            tracing::warn!(error = %err, task = %task_id, "could not close a capacity pause");
        }
    }

    /// A running session's reading of its subscription's plan usage (Claude Code's status line:
    /// the rate-limit headers of its own API responses, ADR-17). The only usage a
    /// `claude setup-token` sign-in can see: the usage endpoint refuses it. The five-hour and
    /// weekly windows replace the stored ones (per-model caps and a reported limit stay); stored
    /// when they changed — at once for a move of a point or more, a reset or the threshold, else
    /// at most every 15 s — and sampled for the burn rate at most every 5 minutes.
    pub(crate) async fn session_usage(&self, account: &AccountId, limits: &RateLimits) {
        let now = self.now();
        let five_hour = session_window(limits.five_hour, 5 * 60, now);
        let seven_day = session_window(limits.seven_day, 7 * 24 * 60, now);
        if five_hour.is_none() && seven_day.is_none() {
            return;
        }
        let Some(sample) = self.session_reading_due(account, five_hour, seven_day, now) else {
            return;
        };
        let Ok(Some(record)) = self.deps.store.account(account).await else {
            return;
        };
        let mut usage = account_usage(&record);
        usage.primary = five_hour.or(usage.primary);
        usage.secondary = seven_day.or(usage.secondary);
        usage.observed_at = Some(now);
        self.record_account_usage(account, &usage).await;
        if sample {
            let windows = AccountUsage {
                primary: five_hour,
                secondary: seven_day,
                observed_at: Some(now),
                ..AccountUsage::default()
            };
            self.record_usage_samples(account, &windows, USAGE_FROM_SESSION)
                .await;
        }
    }

    /// Whether a session reading is to be stored now (`Some(sample)`, with whether it is also a
    /// burn-rate sample), remembering it when it is.
    fn session_reading_due(
        &self,
        account: &AccountId,
        five_hour: Option<UsageWindow>,
        seven_day: Option<UsageWindow>,
        now: Timestamp,
    ) -> Option<bool> {
        let mut sessions = self.capacity.sessions.lock().ok()?;
        let (write, sample, sampled) = match sessions.get(account) {
            None => (true, true, now),
            Some(last) if last.five_hour == five_hour && last.seven_day == seven_day => (
                now.millis_since(last.written) >= SESSION_TOUCH_MS,
                false,
                last.sampled,
            ),
            Some(last) => {
                let much =
                    moved_much(last.five_hour, five_hour) || moved_much(last.seven_day, seven_day);
                let write = much || now.millis_since(last.written) >= SESSION_WRITE_MS;
                let reset = last.five_hour.map(|w| w.resets_at) != five_hour.map(|w| w.resets_at)
                    || last.seven_day.map(|w| w.resets_at) != seven_day.map(|w| w.resets_at);
                let sample =
                    write && (reset || now.millis_since(last.sampled) >= SESSION_SAMPLE_MS);
                (write, sample, if sample { now } else { last.sampled })
            }
        };
        if !write {
            return None;
        }
        sessions.insert(
            account.clone(),
            SessionReading {
                five_hour,
                seven_day,
                written: now,
                sampled,
            },
        );
        Some(sample)
    }

    /// Stores readings of an account's windows for the burn rate (ROT-013); `source` says who
    /// read them ([`crate::model::USAGE_FROM_PROBE`] or [`USAGE_FROM_SESSION`]).
    pub(crate) async fn record_usage_samples(
        &self,
        account: &AccountId,
        usage: &AccountUsage,
        source: &str,
    ) {
        let at = usage.observed_at.unwrap_or_else(|| self.now());
        let samples: Vec<UsageSampleRecord> = [
            ("five_hour", usage.primary),
            ("seven_day", usage.secondary),
            ("seven_day_opus", usage.weekly_opus),
            ("seven_day_sonnet", usage.weekly_sonnet),
        ]
        .into_iter()
        .filter_map(|(window, w)| {
            let w = w?;
            Some(UsageSampleRecord {
                account_id: account.clone(),
                at,
                window: window.to_owned(),
                used_percent: w.known_percent()?,
                resets_at: w.resets_at,
                source: source.to_owned(),
            })
        })
        .collect();
        if samples.is_empty() {
            return;
        }
        if let Err(err) = self
            .deps
            .store
            .record_usage_samples(&samples, at.plus_millis(-SAMPLE_RETENTION_MS))
            .await
        {
            tracing::warn!(error = %err, "could not store usage readings");
        }
    }

    /// Starts the resume loop; call once (from `start_background`).
    pub fn start_capacity_loop(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let wait = app.resume_round().await;
                tokio::select! {
                    () = app.capacity.wake.notified() => {}
                    () = tokio::time::sleep(wait) => {}
                }
            }
        });
    }

    /// One pass over the open pauses, oldest first: each task the policy can place resumes,
    /// with the configured stagger between two resumes. Returns how long to sleep before the
    /// next pass (until the earliest known reset, at most 5 minutes).
    pub async fn resume_round(self: &Arc<Self>) -> Duration {
        // An instance update holds every task; the pool resumes them after it (ADR-18).
        if self.update_holds_starts() {
            return Duration::from_secs(5);
        }
        let Ok(pauses) = self.deps.store.open_pauses().await else {
            return RECHECK;
        };
        let mut earliest: Option<Timestamp> = None;
        let mut resumed = 0_u32;
        let mut holding = false;
        for pause in pauses {
            holding |= self.hook_held(&pause);
            if resumed > 0 && self.would_resume(&pause).await {
                tokio::time::sleep(Duration::from_millis(self.config.resume_stagger_ms)).await;
            }
            match self.resume_paused(&pause).await {
                Resume::Resumed => resumed += 1,
                Resume::Waiting(until) => {
                    if let Some(at) = until {
                        earliest = Some(earliest.map_or(at, |e| e.min(at)));
                    }
                }
                Resume::Gone => {}
            }
        }
        let wait = earliest.map_or(RECHECK, |at| {
            let ms = u64::try_from(at.millis_since(self.now()).max(0)).unwrap_or(0);
            // Wake a moment after the reset so the window reads as reset.
            Duration::from_millis(ms.saturating_add(1_000)).clamp(MIN_RECHECK, RECHECK)
        });
        if holding { wait.min(HOLD_CHECK) } else { wait }
    }

    /// Whether `pause` holds an agent at a hook of a run that reports through hooks (a `turn`
    /// or `prompt` pause with a process): only such a hold is asked about again and again.
    fn hook_held(&self, pause: &PauseRecord) -> bool {
        matches!(pause.kind, PauseKind::Turn | PauseKind::Prompt)
            && pause.account_id.is_some()
            && self
                .existing_runtime(&pause.task_id)
                .is_some_and(|rt| rt.hooks_seen.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Whether the CLI gave up the hook that `pause` holds: nobody asked about it for
    /// [`HOLD_LOST_MS`]. The clock starts with the first look when no poll was seen since this
    /// server started.
    fn hold_given_up(&self, pause: &PauseRecord) -> bool {
        if !self.hook_held(pause) {
            return false;
        }
        let now = self.now();
        let polled = &self.runtime(&pause.task_id).hold_polled_ms;
        let last = polled.load(std::sync::atomic::Ordering::SeqCst);
        if last <= 0 {
            polled.store(now.0, std::sync::atomic::Ordering::SeqCst);
            return false;
        }
        now.millis_since(Timestamp(last)) > HOLD_LOST_MS
    }

    /// A paused task whose hold ended without the resume loop — the person typed or interrupted
    /// the held turn, or the CLI gave the hold up — runs again on its own process: its pause
    /// closes with `outcome` and the task is `running` with `activity` (F 2026-10-10: the task
    /// follows its session; a pause that holds nothing must not read "Paused").
    pub(crate) async fn hold_ended(&self, task_id: &TaskId, outcome: &str, activity: Activity) {
        self.end_pause(task_id, outcome).await;
        let store = &self.deps.store;
        if let Ok(Some(task)) = store.task(task_id).await
            && task.status == TaskStatus::WaitingCapacity
            && store.live_attempt(task_id).await.ok().flatten().is_some()
            && let Ok(next) = task.status.apply(TaskEvent::CapacityReturned)
        {
            let _ = store
                .transition_task(task_id, Some(task.status), next, Some(activity), self.now())
                .await;
            tracing::info!(task = %task_id, %outcome, "a held turn ended without the pool: the task runs again");
            self.publish_task(task_id).await;
        }
    }

    /// The policy decision for a paused task now: `Stay` resumes in place, `Switch` on another
    /// account, `Wait` keeps it paused.
    async fn paused_decision(&self, task: &TaskRecord) -> AppResult<(Decision, String, bool)> {
        let live = self.deps.store.live_attempt(&task.id).await?;
        let has_process = live.is_some();
        let current = live.and_then(|a| a.account_id);
        let trigger = if has_process {
            Trigger::Proactive
        } else {
            Trigger::Start
        };
        let input = self.rotation_input(task, current).await?;
        let (decision, reason) = rotation::decide(&input, trigger);
        Ok((decision, format!("{reason:?}"), has_process))
    }

    async fn would_resume(&self, pause: &PauseRecord) -> bool {
        match self.deps.store.task(&pause.task_id).await {
            Ok(Some(task)) if task.status == TaskStatus::WaitingCapacity => matches!(
                self.paused_decision(&task).await,
                Ok((Decision::Stay | Decision::Switch { .. }, _, _))
            ),
            _ => false,
        }
    }

    async fn resume_paused(self: &Arc<Self>, pause: &PauseRecord) -> Resume {
        let store = &self.deps.store;
        let task = match store.task(&pause.task_id).await {
            Ok(Some(task)) if task.status == TaskStatus::WaitingCapacity => task,
            Ok(_) => {
                self.end_pause(&pause.task_id, "ended").await;
                return Resume::Gone;
            }
            Err(_) => return Resume::Waiting(None),
        };
        let (decision, reason, has_process) = match self.paused_decision(&task).await {
            Ok(d) => d,
            Err(_) => return Resume::Waiting(None),
        };
        match decision {
            // Still no capacity, but nobody is held any more: the CLI gave the hook up.
            Decision::Wait { .. } if self.hold_given_up(pause) => {
                self.hold_ended(&task.id, "abandoned", Activity::Waiting)
                    .await;
                Resume::Gone
            }
            Decision::Wait { until } => Resume::Waiting(until),
            // A turn the limit cut cannot go on in place without typing into it: it resumes past
            // the limit like on another subscription (SPEC-034).
            Decision::Stay if has_process && pause.kind != PauseKind::Limit => {
                self.release_in_place(&task, pause).await;
                Resume::Resumed
            }
            Decision::Stay | Decision::Switch { .. } => {
                let account = match decision {
                    Decision::Switch { account } => Some(account),
                    _ => None,
                };
                if has_process {
                    let switched = match pause.kind {
                        PauseKind::Limit => {
                            self.switch_after_limit(&task.id, account, &reason).await
                        }
                        kind => {
                            let continuation = match kind {
                                PauseKind::Prompt => pause.continuation.clone(),
                                PauseKind::Turn => self.stop_continuation(&task).await,
                                PauseKind::Limit | PauseKind::Start => None,
                            };
                            self.switch_task_now(&task.id, account, &reason, continuation)
                                .await
                        }
                    };
                    if let Err(err) = switched {
                        tracing::warn!(error = %err, task = %task.id, "resume on another subscription failed");
                    }
                } else {
                    self.end_pause(&task.id, "started").await;
                    let start = match store.provider_sessions(&task.id).await {
                        Ok(lineage) if !lineage.is_empty() && pause.kind == PauseKind::Limit => {
                            RunStart::AfterLimit
                        }
                        Ok(lineage) if !lineage.is_empty() => RunStart::Resume {
                            prompt: pause.continuation.clone(),
                        },
                        _ => RunStart::Fresh {
                            prompt: pause.continuation.clone(),
                        },
                    };
                    if let Err(err) = self.start_run(&task.id, start, account).await {
                        tracing::warn!(error = %err, task = %task.id, "paused task failed to start");
                    }
                }
                Resume::Resumed
            }
        }
    }

    /// Capacity is back on the account the paused process uses: the pause ends and the task
    /// runs again; a held hook answers on its next poll. Nothing is typed (a `limit` pause never
    /// comes here, SPEC-034).
    async fn release_in_place(&self, task: &TaskRecord, pause: &PauseRecord) {
        let store = &self.deps.store;
        self.end_pause(&task.id, "released").await;
        if let Ok(next) = task.status.apply(TaskEvent::CapacityReturned) {
            let activity = match pause.kind {
                PauseKind::Turn => Activity::Waiting,
                _ => Activity::Working,
            };
            let _ = store
                .transition_task(
                    &task.id,
                    Some(task.status),
                    next,
                    Some(activity),
                    self.now(),
                )
                .await;
        }
        self.runtime(&task.id)
            .hub
            .notice("A subscription has capacity again: the task continues.");
        self.publish_task(&task.id).await;
    }

    /// The subscription pool, its burn rate and forecast for the caller (`GET /api/capacity`):
    /// everyone signed in sees labels, status and usage; ids and live task counts are for
    /// instance administrators (like the Status panel).
    ///
    /// # Errors
    /// Store failures.
    pub async fn capacity_view(&self, ctx: &AuthContext) -> AppResult<CapacityView> {
        let store = &self.deps.store;
        let admin = ctx.user.is_superadmin || ctx.user.is_instance_admin;
        let now = self.now();
        let threshold = rotation::THRESHOLD;
        let records = store.accounts().await?;
        let live = store.live_attempts().await?;
        let views = self.account_views().await?;
        let mut accounts = Vec::with_capacity(records.len());
        let mut members = Vec::new();
        let mut five_hour_left: Option<f64> = None;
        let mut available = 0_u32;
        for record in &records {
            let usage = account_usage(record);
            let is_available = views.iter().any(|v| {
                v.id == record.id && rotation::assess(v, now, threshold, ModelFamily::Any).is_some()
            });
            if is_available {
                available += 1;
            }
            if record.credential.is_some() && healthy(record.status) {
                members.push(PoolMember {
                    weekly_used: usage.secondary.and_then(|w| w.current_percent(now)),
                    weekly_resets_at: usage.secondary.and_then(|w| next_weekly_reset(&w, now)),
                });
                if let Some(p) = usage.primary.and_then(|w| w.current_percent(now)) {
                    five_hour_left = Some(five_hour_left.unwrap_or(0.0) + (threshold - p).max(0.0));
                }
            }
            accounts.push(CapacityAccount {
                label: record.label.clone(),
                status: record.status.as_str().to_owned(),
                usage: usage_view(&usage),
                available: is_available,
                id: admin.then(|| record.id.to_string()),
                in_use_by: admin.then(|| in_use_count(&record.id, &live)),
            });
        }
        let since = now.plus_millis(-WEEK_MS);
        let samples: Vec<WeeklySample> = store
            .usage_samples(since)
            .await?
            .into_iter()
            .filter(|s| s.window == "seven_day")
            .map(|s| WeeklySample {
                account: s.account_id.to_string(),
                at: s.at,
                used_percent: s.used_percent,
                resets_at: s.resets_at,
            })
            .collect();
        let runs: Vec<Span> = store
            .attempt_spans(since)
            .await?
            .into_iter()
            .map(|(start, end)| Span { start, end })
            .collect();
        let pause_records = store.pauses_since(since).await?;
        let span = |p: &PauseRecord| Span {
            start: p.started_at,
            end: p.ended_at,
        };
        let pauses: Vec<Span> = pause_records.iter().map(span).collect();
        let held: Vec<Span> = pause_records
            .iter()
            .filter(|p| p.kind.holds_process())
            .map(span)
            .collect();
        let forecast = yulius_domain::capacity::forecast(&ForecastInput {
            now,
            threshold,
            members: &members,
            samples: &samples,
            runs: &runs,
            pauses: &pauses,
            held: &held,
        });
        let paused_tasks = u32::try_from(
            pause_records
                .iter()
                .filter(|p| p.ended_at.is_none())
                .count(),
        )
        .unwrap_or(u32::MAX);
        let resumes_at = (available == 0)
            .then(|| {
                rotation::available_again(&RotationInput {
                    current: None,
                    task_provider: yulius_domain::harness::Provider::Anthropic,
                    family: ModelFamily::Any,
                    accounts: views.clone(),
                    now,
                    threshold,
                })
            })
            .flatten();
        Ok(CapacityView {
            threshold,
            subscriptions: forecast.subscriptions,
            unknown_usage: forecast.unknown_usage,
            available,
            weekly_left: forecast.weekly_headroom,
            five_hour_left,
            burn_24h: forecast.burn_24h,
            burn_7d: forecast.burn_7d,
            runs_out_at: forecast.runs_out_at.map(rfc3339),
            paused_hours_7d: forecast.paused_hours_7d,
            active_hours_7d: forecast.active_hours_7d,
            needed: forecast.needed,
            add: forecast.add,
            recommendation: forecast.recommendation(),
            paused_tasks,
            resumes_at: resumes_at.map(rfc3339),
            accounts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_resets_roll_forward_by_whole_weeks() {
        let now = Timestamp(100 * WEEK_MS);
        let window = |at: i64| UsageWindow {
            used_percent: Some(1.0),
            window_minutes: None,
            resets_at: Some(Timestamp(at)),
        };
        assert_eq!(
            next_weekly_reset(&window(now.0 + 5), now),
            Some(Timestamp(now.0 + 5))
        );
        assert_eq!(
            next_weekly_reset(&window(now.0 - 1), now),
            Some(Timestamp(now.0 - 1 + WEEK_MS))
        );
        assert_eq!(
            next_weekly_reset(&window(now.0 - WEEK_MS - 1), now),
            Some(Timestamp(now.0 - 1 + WEEK_MS))
        );
        assert_eq!(
            next_weekly_reset(
                &UsageWindow {
                    resets_at: None,
                    ..window(0)
                },
                now
            ),
            None
        );
    }
}
