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
//! - `limit`: the CLI ended a turn with a usage limit and waits at its limit message;
//! - `start`: there is no process yet; the task starts when capacity returns.
//!
//! The resume loop ([`App::start_capacity_loop`]) wakes on new usage, a new or removed account,
//! a new pause, the earliest known reset, and every 5 minutes. It walks the open pauses oldest
//! first and resumes each task the policy places, waiting [`crate::context::AppConfig`]
//! `resume_stagger_ms` between two resumes: in place when the paused process's account has
//! capacity again (the held hook simply answers on its next poll), otherwise by a native resume
//! on the chosen account.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use yulius_contracts::api::admin::{CapacityAccount, CapacityView};
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
use crate::model::{AccountStatus, PauseKind, PauseRecord, TaskRecord, UsageSampleRecord};
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
/// The only text the harness ever sends on its own after a pause: a turn the CLI itself
/// interrupted with a usage limit needs a user message to go on. It says nothing about pauses,
/// accounts or limits.
pub const CONTINUE_TEXT: &str = "Continue.";
/// Usage readings are kept this long (the forecast reads 7 days).
const SAMPLE_RETENTION_MS: i64 = 8 * 24 * 3_600_000;
const WEEK_MS: i64 = 7 * 24 * 3_600_000;

/// Runtime state of the pool controller (owned by [`App`]).
#[derive(Default)]
pub struct CapacityState {
    wake: Notify,
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
            family: ModelFamily::of(task.model.as_deref()),
            accounts: self.account_views().await?,
            now: self.now(),
            threshold: rotation::THRESHOLD,
        })
    }

    /// The subscription a one-off request outside any task (a split proposal) runs on: the best
    /// placement of the pool.
    ///
    /// # Errors
    /// `Conflict` without any Claude subscription, `Capacity` while none has capacity.
    pub(crate) async fn pool_account(
        &self,
        provider: yulius_domain::harness::Provider,
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
            family: ModelFamily::Any,
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
    /// pause (the interrupted turn needs [`CONTINUE_TEXT`] to go on).
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
                .notice("Yulius paused this task: waiting for subscription capacity.");
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

    /// Stores the provider's readings of an account's windows for the burn rate (ROT-013).
    pub(crate) async fn record_usage_samples(&self, account: &AccountId, usage: &AccountUsage) {
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
        let Ok(pauses) = self.deps.store.open_pauses().await else {
            return RECHECK;
        };
        let mut earliest: Option<Timestamp> = None;
        let mut resumed = 0_u32;
        for pause in pauses {
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
        earliest.map_or(RECHECK, |at| {
            let ms = u64::try_from(at.millis_since(self.now()).max(0)).unwrap_or(0);
            // Wake a moment after the reset so the window reads as reset.
            Duration::from_millis(ms.saturating_add(1_000)).clamp(MIN_RECHECK, RECHECK)
        })
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
            Decision::Wait { until } => Resume::Waiting(until),
            Decision::Stay if has_process => {
                self.release_in_place(&task, pause).await;
                Resume::Resumed
            }
            Decision::Stay | Decision::Switch { .. } => {
                let account = match decision {
                    Decision::Switch { account } => Some(account),
                    _ => None,
                };
                if has_process {
                    let continuation = match pause.kind {
                        PauseKind::Prompt => pause.continuation.clone(),
                        PauseKind::Limit => Some(CONTINUE_TEXT.to_owned()),
                        PauseKind::Turn => self.stop_continuation(&task).await,
                        PauseKind::Start => None,
                    };
                    if let Err(err) = self
                        .switch_task_now(&task.id, account, &reason, continuation)
                        .await
                    {
                        tracing::warn!(error = %err, task = %task.id, "resume on another subscription failed");
                    }
                } else {
                    self.end_pause(&task.id, "started").await;
                    let start = match store.provider_sessions(&task.id).await {
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
    /// runs again. A held hook answers on its next poll; only a turn the CLI interrupted with
    /// a limit gets [`CONTINUE_TEXT`].
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
            .notice("Subscription capacity is back: the task continues.");
        if pause.kind == PauseKind::Limit {
            if let Err(err) = self.send_input(&task.id, CONTINUE_TEXT.as_bytes()).await {
                tracing::warn!(error = %err, task = %task.id, "could not continue after a limit");
            } else {
                // A separate Enter, so the TUI sees typed text first and then a submit.
                tokio::time::sleep(Duration::from_millis(150)).await;
                let _ = self.send_input(&task.id, b"\r").await;
            }
        }
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
