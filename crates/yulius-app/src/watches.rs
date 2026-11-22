//! Script watches (SPEC-630, ADR-0465): a project's shell script on a schedule, run by Yulius in
//! the sandbox without an agent, whose output becomes a notice keyed `watch:<name>` for its owner.
//!
//! Contract:
//! - WCH-009: [`App::watch_tick`] runs every [`WatchSettings::tick`]. It first retries pending
//!   posts, then starts each watch that is due, not paused, not ended and not running, up to
//!   [`WatchSettings::max_runs`] runs at once on the instance. A run is claimed in the store
//!   before it starts (one per watch) and its end is recorded only by that run;
//! - WCH-005, WCH-007: a run's end is decided by [`yulius_domain::watch::outcome`]. The posts it
//!   asks for are written to the watch's pending posts in the same transaction as its state
//!   (an outbox), then delivered as notices ([`App::post_notice`]);
//! - WCH-008: a pending post leaves only once its notice is stored. A run's end the store refused
//!   is kept in memory and written again by the next tick;
//! - WCH-010: [`App::watch_startup`] records a run the previous process left running as lost (a
//!   failure), ends its process host and makes the watch due again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, watch};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::notice::{Expiry, NoticeKey, NoticeTone, REPORT_MS};
use yulius_domain::notify::NoticeKind;
use yulius_domain::time::Timestamp;
use yulius_domain::watch::{
    self as dom, FailureState, MAX_PER_PROJECT, POSTED_CHARS, RunBounds, RunEnd, WatchName,
    WatchSchedule, cut, outcome, post_title,
};

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::notices::NewNotice;
use crate::ports::store_ext::{WatchPending, WatchRecord, WatchRunEnd, WatchState};

/// The instance's watch settings (WCH-003, WCH-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchSettings {
    /// Each run's bounds.
    pub bounds: RunBounds,
    /// Runs going at once on the instance.
    pub max_runs: usize,
    /// The scheduler's period.
    pub tick: Duration,
}

impl Default for WatchSettings {
    fn default() -> Self {
        Self {
            bounds: RunBounds::default(),
            max_runs: 4,
            tick: Duration::from_secs(15),
        }
    }
}

/// Ends of runs the store refused, kept for the next tick (WCH-008); a bound, so a long outage
/// cannot grow memory: past it the oldest go (their watches are recovered as lost at a start).
const UNRECORDED_MAX: usize = 200;

/// What the scheduler holds between ticks.
pub struct WatchRuntime {
    permits: Arc<Semaphore>,
    /// The going run of each watch, to cancel it (a removal).
    cancels: Mutex<HashMap<String, watch::Sender<Option<String>>>>,
    /// Ends of runs the store refused, oldest first.
    unrecorded: Mutex<Vec<(String, String, WatchRunEnd)>>,
    /// One delivery pass at a time, so a post is never sent twice.
    delivering: tokio::sync::Mutex<()>,
}

impl WatchRuntime {
    /// A runtime allowing `max_runs` runs at once.
    #[must_use]
    pub fn new(max_runs: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_runs.max(1))),
            cancels: Mutex::new(HashMap::new()),
            unrecorded: Mutex::new(Vec::new()),
            delivering: tokio::sync::Mutex::new(()),
        }
    }

    fn cancels(&self) -> std::sync::MutexGuard<'_, HashMap<String, watch::Sender<Option<String>>>> {
        self.cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn unrecorded(&self) -> std::sync::MutexGuard<'_, Vec<(String, String, WatchRunEnd)>> {
        self.unrecorded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A new watch (WCH-001), as people and the Superagent give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWatch {
    /// Its project.
    pub sandbox: SandboxId,
    /// Its owner.
    pub owner: UserId,
    /// Its name.
    pub name: String,
    /// The bash script.
    pub script: String,
    /// An interval (`15m`); exactly one of this and `cron`.
    pub every: Option<String>,
    /// A cron expression.
    pub cron: Option<String>,
    /// The cron expression's UTC offset, minutes.
    pub utc_offset_minutes: i32,
    /// Its notices' expiry: `never` or a duration; `None`: a day.
    pub expiry: Option<String>,
    /// A run's timeout, seconds; `None`: 5 minutes.
    pub timeout_seconds: Option<u32>,
    /// Agent secret names its runs get through the egress proxy.
    pub secrets: Vec<String>,
    /// `ui` or `agent`.
    pub created_via: &'static str,
}

/// A watch's schedule as stored.
///
/// # Errors
/// A stored schedule that no longer parses (never written by this code).
pub fn schedule_of(watch: &WatchRecord) -> Result<WatchSchedule, String> {
    match (&watch.cron, watch.every_seconds) {
        (Some(cron), _) => WatchSchedule::cron(cron, watch.utc_offset_minutes),
        (None, Some(seconds)) => Ok(WatchSchedule::Every(seconds)),
        (None, None) => Err("the watch has no schedule".into()),
    }
}

/// When `watch` is next due, if it runs at all (paused and ended watches do not).
#[must_use]
pub fn next_due(watch: &WatchRecord) -> Option<Timestamp> {
    if watch.paused || watch.ended_reason.is_some() {
        return None;
    }
    schedule_of(watch)
        .ok()?
        .next_due(watch.state.last_started_at, watch.since)
}

/// A notice's expiry for a watch's reports: `never` or a duration (an absolute time would end
/// every later report).
///
/// # Errors
/// Anything else, in words.
pub fn watch_expiry(text: &str) -> Result<Expiry, String> {
    match Expiry::parse(text)? {
        Expiry::At(_) => {
            Err("a watch's expiry is `never` or a duration such as `2h` or `3d`, not a time".into())
        }
        expiry => Ok(expiry),
    }
}

/// A pending post's notice, as kept (WCH-008).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PostBody {
    category: String,
    tone: String,
    title: String,
    detail: Option<String>,
    /// `None`: the category's default.
    expiry: Option<String>,
}

fn random_id(app: &App, prefix: &str) -> String {
    let hex: String = app
        .deps
        .crypto
        .random_bytes()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{prefix}{hex}")
}

impl App {
    /// Creates a watch (WCH-001); it runs at the next tick (an interval) or slot (cron).
    ///
    /// # Errors
    /// `Invalid` for a bad name, script, schedule, expiry, timeout or secret name; `Conflict` for
    /// a name taken in the project or a full project; store failures.
    pub async fn watch_create(&self, new: NewWatch) -> AppResult<WatchRecord> {
        let name = WatchName::parse(&new.name).map_err(AppError::Invalid)?;
        dom::check_script(&new.script).map_err(AppError::Invalid)?;
        let schedule = match (new.every.as_deref(), new.cron.as_deref()) {
            (Some(every), None) => WatchSchedule::every(every),
            (None, Some(cron)) => WatchSchedule::cron(cron, new.utc_offset_minutes),
            _ => Err(
                "a watch runs either every interval (every) or on a cron schedule (cron)".into(),
            ),
        }
        .map_err(AppError::Invalid)?;
        if let Some(expiry) = &new.expiry {
            watch_expiry(expiry).map_err(AppError::Invalid)?;
        }
        let timeout = new.timeout_seconds.unwrap_or(dom::TIMEOUT_DEFAULT_SECONDS);
        dom::check_timeout(timeout).map_err(AppError::Invalid)?;
        for secret in &new.secrets {
            yulius_domain::vault::SecretName::parse(secret)
                .map_err(|e| AppError::Invalid(format!("{secret}: {e}")))?;
        }
        let now = self.now();
        let (every_seconds, cron, offset) = match schedule {
            WatchSchedule::Every(seconds) => (Some(seconds), None, 0),
            WatchSchedule::Cron(cron, offset) => (None, Some(cron.as_str().to_owned()), offset),
        };
        let record = WatchRecord {
            id: random_id(self, "wt_"),
            sandbox_id: new.sandbox,
            name: name.as_str().to_owned(),
            script: new.script,
            every_seconds,
            cron,
            utc_offset_minutes: offset,
            expiry: new.expiry.map(|e| e.trim().to_owned()),
            timeout_seconds: timeout,
            secrets: new.secrets,
            owner: new.owner,
            created_via: new.created_via.to_owned(),
            created_at: now,
            updated_at: now,
            paused: false,
            since: now,
            ended_reason: None,
            ended_at: None,
            state: WatchState::default(),
        };
        self.deps
            .store
            .insert_watch(&record, MAX_PER_PROJECT)
            .await?;
        Ok(record)
    }

    /// One scheduler pass (WCH-009): pending posts first, then every due watch, within the
    /// instance's run bound. Returns the runs it started, which go on by themselves.
    pub async fn watch_tick(self: &Arc<Self>) -> Vec<tokio::task::JoinHandle<()>> {
        self.watch_deliver().await;
        let watches = match self.deps.store.all_watches().await {
            Ok(watches) => watches,
            Err(error) => {
                tracing::warn!(error = %error, "watches could not be read");
                return Vec::new();
            }
        };
        let now = self.now();
        let mut started = Vec::new();
        for watch in watches {
            if watch.state.running.is_some() || next_due(&watch).is_none_or(|due| due > now) {
                continue;
            }
            let Ok(permit) = Arc::clone(&self.watches.permits).try_acquire_owned() else {
                break;
            };
            let app = Arc::clone(self);
            started.push(tokio::spawn(async move {
                let _permit = permit;
                app.watch_run(watch).await;
            }));
        }
        started
    }

    /// Runs `watch` once now and records its end (one run per watch: `false` when one goes).
    pub async fn watch_run(&self, watch: WatchRecord) -> bool {
        let run_id = random_id(self, "wr_");
        let started = self.now();
        match self
            .deps
            .store
            .watch_run_started(&watch.id, &run_id, started)
            .await
        {
            Ok(true) => {}
            Ok(false) => return false,
            Err(error) => {
                tracing::warn!(watch = %watch.id, error = %error, "a watch's run could not be claimed");
                return false;
            }
        }
        let (cancel, mut cancelled) = watch::channel(None);
        self.watches.cancels().insert(watch.id.clone(), cancel);
        let bounds = self.config.watches.bounds;
        let result = match self.deps.store.sandbox(&watch.sandbox_id).await {
            Ok(Some(sandbox)) => {
                self.watch_execute(&sandbox, &watch, (&run_id, bounds), &mut cancelled)
                    .await
            }
            Ok(None) => crate::ci::WatchRunOutput {
                end: RunEnd::Failed("its project is gone".into()),
                output: String::new(),
                errors: String::new(),
            },
            Err(error) => crate::ci::WatchRunOutput {
                end: RunEnd::Failed(format!(
                    "its project could not be read: {}",
                    error.public_message()
                )),
                output: String::new(),
                errors: String::new(),
            },
        };
        self.watches.cancels().remove(&watch.id);
        self.watch_record_end(&watch, &run_id, started, &result)
            .await;
        true
    }

    /// Records a run's end and its posts (WCH-005 to WCH-008), then delivers them.
    async fn watch_record_end(
        &self,
        watch: &WatchRecord,
        run_id: &str,
        started: Timestamp,
        result: &crate::ci::WatchRunOutput,
    ) {
        let name = WatchName::parse(&watch.name)
            .unwrap_or_else(|_| WatchName::parse("watch").unwrap_or_else(|_| unreachable!()));
        let before = FailureState {
            failures: watch.state.failures,
            warned: watch.state.warned,
        };
        let decided = outcome(
            &name,
            &result.end,
            (&result.output, &result.errors),
            before,
            (self.config.watches.bounds, watch.timeout_seconds),
        );
        let now = self.now();
        let output = result.output.trim();
        let errors = result.errors.trim();
        let state = WatchState {
            running: None,
            last_started_at: Some(started),
            last_ended_at: Some(now),
            last_exit: result.end.code(),
            last_failure: decided.failure.clone(),
            last_output: (!output.is_empty()).then(|| cut(output, POSTED_CHARS)),
            last_errors: (!errors.is_empty()).then(|| errors.to_owned()),
            failures: decided.state.failures,
            warned: decided.state.warned,
            runs: watch.state.runs.saturating_add(1),
            ..watch.state.clone()
        };
        let mut pending = Vec::new();
        let queue = |key: String, body: Option<PostBody>| WatchPending {
            watch_id: watch.id.clone(),
            key,
            body: body.and_then(|b| serde_json::to_string(&b).ok()),
            queued_at: now,
        };
        if let Some(text) = &decided.post {
            let title = post_title(&name, text);
            pending.push(queue(
                name.report_key(),
                Some(PostBody {
                    category: NoticeKind::Done.as_str().into(),
                    tone: NoticeTone::Info.as_str().into(),
                    title,
                    detail: text.contains('\n').then(|| text.clone()),
                    expiry: Some(watch.expiry.clone().unwrap_or_else(|| "1d".into())),
                }),
            ));
        }
        if let Some(text) = &decided.warn {
            pending.push(queue(
                name.warning_key(),
                Some(PostBody {
                    category: NoticeKind::Failed.as_str().into(),
                    tone: NoticeTone::Error.as_str().into(),
                    title: format!("{name}: failing, {} runs in a row", decided.state.failures),
                    detail: Some(text.clone()),
                    expiry: None,
                }),
            ));
        }
        if decided.clear_warning {
            pending.push(queue(name.warning_key(), None));
        }
        let end = WatchRunEnd {
            state,
            ended: decided.end,
            pending,
        };
        if let Err(error) = self
            .deps
            .store
            .watch_run_ended(&watch.id, run_id, &end)
            .await
        {
            tracing::warn!(watch = %watch.id, error = %error, "a watch's run end is kept for the next tick");
            let mut kept = self.watches.unrecorded();
            if kept.len() >= UNRECORDED_MAX {
                kept.remove(0);
            }
            kept.push((watch.id.clone(), run_id.to_owned(), end));
            return;
        }
        self.watch_deliver().await;
    }

    /// Writes the run ends the store refused, then delivers every pending post, oldest first
    /// (WCH-008). A post leaves only once its notice is stored; one whose watch is gone, or
    /// that can never be stored (refused as invalid), is dropped.
    pub async fn watch_deliver(&self) {
        let _one = self.watches.delivering.lock().await;
        let kept = std::mem::take(&mut *self.watches.unrecorded());
        for (index, (watch, run, end)) in kept.iter().enumerate() {
            if let Err(error) = self.deps.store.watch_run_ended(watch, run, end).await {
                tracing::warn!(error = %error, "a watch's run end could not be recorded yet");
                let mut unrecorded = self.watches.unrecorded();
                let rest = kept[index..].iter().cloned();
                let newer = std::mem::take(&mut *unrecorded);
                unrecorded.extend(rest.chain(newer));
                return;
            }
        }
        let pending = match self.deps.store.watch_pending().await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::warn!(error = %error, "a watch's pending posts could not be read");
                return;
            }
        };
        for post in pending {
            match self.watch_deliver_one(&post).await {
                Ok(()) => {
                    if let Err(error) = self
                        .deps
                        .store
                        .drop_watch_pending(&post.watch_id, &post.key, post.queued_at)
                        .await
                    {
                        tracing::warn!(error = %error, "a delivered watch post stays pending");
                    }
                }
                Err(error) => {
                    tracing::warn!(watch = %post.watch_id, key = %post.key, error = %error, "a watch's post stays pending");
                }
            }
        }
    }

    /// Stores one pending post as its notice, or removes the notice of its key; `Ok` also when
    /// it can be dropped (its watch is gone, or it can never be stored).
    async fn watch_deliver_one(&self, post: &WatchPending) -> AppResult<()> {
        let Some(watch) = self.deps.store.watch(&post.watch_id).await? else {
            return Ok(());
        };
        let Ok(key) = NoticeKey::parse(&post.key) else {
            return Ok(());
        };
        let Some(body) = &post.body else {
            self.remove_notice(Some(&watch.sandbox_id), Some(&watch.owner), &key)
                .await?;
            return Ok(());
        };
        let Ok(body) = serde_json::from_str::<PostBody>(body) else {
            return Ok(());
        };
        let slug = self
            .deps
            .store
            .sandbox(&watch.sandbox_id)
            .await?
            .map(|s| s.slug);
        let expiry = match body.expiry.as_deref().map(watch_expiry) {
            Some(Ok(expiry)) => Some(expiry),
            Some(Err(_)) => Some(Expiry::After(REPORT_MS)),
            None => None,
        };
        let report = post.key == format!("{}{}", dom::KEY_PREFIX, watch.name);
        let notice = NewNotice {
            to: Some(watch.owner.clone()),
            detail: body.detail,
            link: slug.map(|slug| format!("/s/{}/watches", urlencoding_path(&slug))),
            expiry,
            ..NewNotice::new(
                Some(watch.sandbox_id.clone()),
                key,
                NoticeKind::parse(&body.category).unwrap_or(NoticeKind::Done),
                NoticeTone::parse(&body.tone),
                body.title,
            )
        };
        match self.post_notice(notice).await {
            Ok(_) => {
                if report {
                    let _ = self.deps.store.watch_posted(&watch.id, self.now()).await;
                }
                Ok(())
            }
            Err(AppError::Invalid(reason)) => {
                tracing::warn!(watch = %watch.id, reason = %reason, "a watch's post was refused and dropped");
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// At a start (WCH-010): each run the previous process left going is recorded as lost, its
    /// process host and files end, and its watch is due again from now.
    pub async fn watch_startup(&self) {
        let watches = match self.deps.store.all_watches().await {
            Ok(watches) => watches,
            Err(error) => {
                tracing::warn!(error = %error, "watches could not be read at the start");
                return;
            }
        };
        for watch in watches {
            let Some((run_id, started)) = watch.state.running.clone() else {
                continue;
            };
            self.watch_end_lost(&watch.sandbox_id, &run_id);
            let lost = crate::ci::WatchRunOutput {
                end: RunEnd::Failed("lost: the server stopped while it ran".into()),
                output: String::new(),
                errors: String::new(),
            };
            self.watch_record_end(&watch, &run_id, started, &lost).await;
            if !watch.paused && watch.ended_reason.is_none() {
                let _ = self
                    .deps
                    .store
                    .set_watch_paused(&watch.id, false, self.now())
                    .await;
            }
        }
        self.watch_deliver().await;
    }

    /// Starts the watch scheduler (WCH-009): the start's recovery, then a tick every period.
    pub fn start_watch_scheduler(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            app.watch_startup().await;
            let mut tick = tokio::time::interval(app.config.watches.tick);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let _runs = app.watch_tick().await;
            }
        });
    }
}

/// A slug as a path segment (slugs are already URL-safe; this keeps a stray `/` out).
fn urlencoding_path(slug: &str) -> String {
    slug.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_string()
            } else {
                format!("%{:02X}", u32::from(c) & 0xff)
            }
        })
        .collect()
}
