//! What reaches a person's own chat (SPEC-498 TGC-020..TGC-024, ADR-0351): the notifier reads
//! the UI event bus, keeps what the notification center would tell (a task entering Needs you,
//! a new `needs_you` request, Done, an abnormal end, a subscription problem), sends it only to
//! the linked people whose event stream carries that event and who chose that kind, redacted
//! like transcripts, and folds a burst under the per-person rate bound. It speaks to the channel
//! only through the send queue (`telegram.rs`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast::error::RecvError;
use yulius_contracts::api::admin::AccountView;
use yulius_contracts::api::tasks::TaskSummary;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::notice::NoticeKey;
use yulius_domain::notify::{
    AskedOf, NoticeKind, TaskNotice, Throttle, account_problem, account_text, control_text,
    posted_text, shared_cause, taken_text, task_notice, task_signature,
};

use crate::context::App;
use crate::error::AppResult;
use crate::events::{Envelope, Reach};
use crate::model::TaskRecord;
use crate::needs_you::NeedsYouRecord;

/// Episodes and requests for control remembered as told; beyond it that memory starts again (a
/// still-open one may then be told once more). Tasks and subscriptions are bounded by the store.
const ONCE_MAX: usize = 20_000;

/// The notifier's memory, in this process only.
#[derive(Debug, Default)]
pub(crate) struct NotifierState {
    /// The last signature told (or seen) per task (`task:<id>`) and subscription (`account:<id>`),
    /// replaced by the seed when a bot becomes active (TGC-022); a removed task is forgotten.
    told: Mutex<HashMap<String, String>>,
    /// The sandbox-wide episodes (`shared:…`) and requests for control (`control:…`) told.
    once: Mutex<HashSet<String>>,
    /// Asks the notifier's own task to seed and then make a bot the one in use, answering when
    /// it did.
    seed_tx: std::sync::OnceLock<tokio::sync::mpsc::Sender<SeedRequest>>,
    /// Each person's chat and rate bound.
    throttles: Mutex<HashMap<UserId, (String, Throttle)>>,
}

/// A bot to make the one in use once the memory is seeded, and who waits for it.
type SeedRequest = (crate::telegram::ActiveBot, tokio::sync::oneshot::Sender<()>);

/// What a task reads as for the chat ([`App::task_reading`]).
struct TaskReading {
    /// The notification its state calls for, if any.
    kind: Option<NoticeKind>,
    /// What makes it new; empty when it calls for none.
    signature: String,
    /// Its open `needs_you` request while it needs you.
    asked: Option<NeedsYouRecord>,
    /// Actionable Needs You explanation for any Needs You state (SPEC-578).
    notice: Option<yulius_contracts::api::tasks::NeedsYouNoticeView>,
}

/// The memory's subject of a sandbox-wide problem's episode (TGC-026).
fn shared_subject(task: &TaskRecord, cause: &str, episode: i64) -> String {
    format!("shared:{}:{cause}:{episode}", task.sandbox_id)
}

/// The memory's subject of one request for control (TGC-027).
fn control_subject(task: &TaskId, request: &crate::model::ControlRequest) -> String {
    format!("control:{task}:{}:{}", request.by, request.at.0)
}

/// One notification, redacted, and the task it is about (a reply to it reaches the task,
/// SPEC-512 TGI-002).
struct Notice {
    kind: NoticeKind,
    headline: String,
    text: String,
    about: Option<crate::ports::notify::SentAbout>,
    /// Its project and key (`<sandbox or ->:<key>`): the bot's next message with them replaces
    /// this one in the chat (SPEC-624 NTC-007, NTC-008).
    scope: Option<String>,
}

/// A notice's scope in a chat: its project and recipient (`-` for none) and key, so a notice
/// for one person and a project-wide one with the same key keep their own messages (review
/// round 1).
fn chat_scope(
    sandbox: Option<&yulius_domain::ids::SandboxId>,
    recipient: Option<&UserId>,
    key: &NoticeKey,
) -> String {
    format!(
        "{}:{}:{key}",
        sandbox.map_or("-", |s| s.as_str()),
        recipient.map_or("-", |u| u.as_str())
    )
}

impl NotifierState {
    /// Records `signature` for `subject`; the earlier one (`None` when first seen).
    fn remember(&self, subject: String, signature: String) -> Option<String> {
        self.told.lock().ok()?.insert(subject, signature)
    }

    /// Records `subject` as told; whether it was already.
    fn told_once(&self, subject: String) -> bool {
        let Ok(mut once) = self.once.lock() else {
            return true;
        };
        if once.len() >= ONCE_MAX && !once.contains(&subject) {
            once.clear();
        }
        !once.insert(subject)
    }

    /// Forgets what was told of `subject` when it was a failure (`failed:…`).
    fn forget_failure(&self, subject: &str) {
        if let Ok(mut told) = self.told.lock()
            && told
                .get(subject)
                .is_some_and(|said| said.starts_with("failed:"))
        {
            told.remove(subject);
        }
    }

    fn forget(&self, subject: &str) {
        if let Ok(mut told) = self.told.lock() {
            told.remove(subject);
        }
    }
}

impl App {
    /// Starts the notifier: it subscribes before returning, so no later event is missed.
    pub(crate) fn start_notifier(self: &Arc<Self>) {
        let Ok(services) = self.channel_services() else {
            return;
        };
        let flush_every = services.tuning.flush_every;
        let mut rx = self.events.subscribe();
        let (seed_tx, mut seed_rx) = tokio::sync::mpsc::channel(4);
        if services.notifier.seed_tx.set(seed_tx).is_err() {
            return;
        }
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut flush = tokio::time::interval(flush_every);
            flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            flush.tick().await;
            loop {
                tokio::select! {
                    // A seed goes before the events waiting behind it. Until its bot is active no
                    // event is handled, and the events published while it reads are handled after
                    // it, against what it read (review rounds 2 and 3).
                    biased;
                    Some((bot, done)) = seed_rx.recv() => {
                        if let Ok(services) = app.channel_services() {
                            let _hold = services.seeds.read().await;
                        }
                        app.seed_notices().await;
                        app.activate(Some(bot));
                        let _ = done.send(());
                    }
                    event = rx.recv() => match event {
                        Ok(envelope) => app.notify_envelope(&envelope).await,
                        Err(RecvError::Lagged(missed)) => {
                            tracing::warn!(missed, "the notifier fell behind the event bus");
                        }
                        Err(RecvError::Closed) => break,
                    },
                    _ = flush.tick() => app.flush_notices().await,
                }
            }
        });
    }

    async fn notify_envelope(&self, envelope: &Envelope) {
        let Ok(services) = self.channel_services() else {
            return;
        };
        if services.active().is_none() {
            return;
        }
        // Only tasks and subscriptions are told; every other event costs nothing.
        if let UiEvent::TaskRemoved { task_id } = &envelope.event {
            services.notifier.forget(&format!("task:{task_id}"));
            return;
        }
        if !matches!(
            envelope.event,
            UiEvent::TaskUpdated { .. }
                | UiEvent::AccountUpdated { .. }
                | UiEvent::ControlTaken { .. }
                | UiEvent::ControlTakenBack { .. }
                | UiEvent::NoticePosted { .. }
        ) {
            return;
        }
        // Read and remembered while nobody is linked too, so the first person to link is not told
        // of older states as new (review round 1).
        let mut notices = Vec::new();
        match &envelope.event {
            UiEvent::TaskUpdated { task } => {
                notices.push(
                    self.task_event_notice(task)
                        .await
                        .map(|n| n.map(|n| (n, None))),
                );
                notices.push(
                    self.control_notice(task)
                        .await
                        .map(|n| n.map(|(owner, n)| (n, Some(owner)))),
                );
            }
            UiEvent::AccountUpdated { account } => {
                notices.push(Ok(self.account_notice(account).map(|n| (n, None))));
            }
            // Sent only to the person it was taken from, as the event is (SPEC-510 TKC-002).
            UiEvent::ControlTaken { task_id, by, .. } => {
                notices.push(
                    self.taken_notice(task_id, by, false)
                        .await
                        .map(|n| n.and_then(|n| envelope.user.clone().map(|only| (n, Some(only))))),
                );
            }
            // A posted notice (SPEC-624): to whom its envelope reaches, its recipient only when
            // it has one.
            UiEvent::NoticePosted { notice } => {
                notices.push(Ok(self
                    .posted_notice(notice, envelope.user.as_ref())
                    .await
                    .map(|n| (n, envelope.user.clone()))));
            }
            UiEvent::ControlTakenBack { task_id, by, .. } => {
                notices.push(
                    self.taken_notice(task_id, by, true)
                        .await
                        .map(|n| n.and_then(|n| envelope.user.clone().map(|only| (n, Some(only))))),
                );
            }
            _ => {}
        }
        for notice in notices {
            match notice {
                Ok(Some((notice, only))) => {
                    if let Err(err) = self.deliver(envelope, &notice, only.as_ref()).await {
                        tracing::warn!(error = %err, "a notification could not be delivered");
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "a task's notification could not be read");
                }
            }
        }
    }

    /// A posted notice for the chat (SPEC-624 NTC-007): its project's name, title, detail and
    /// link, already redacted when it was posted; keyed, so the next one replaces it.
    async fn posted_notice(
        &self,
        notice: &yulius_contracts::api::notices::NoticeView,
        recipient: Option<&UserId>,
    ) -> Option<Notice> {
        let kind = NoticeKind::parse(&notice.category)?;
        let key = NoticeKey::parse(&notice.key).ok()?;
        let sandbox = match notice.sandbox_id.as_deref() {
            Some(id) => {
                let id = yulius_domain::ids::SandboxId::parse(id).ok()?;
                Some(self.deps.store.sandbox(&id).await.ok().flatten()?)
            }
            None => None,
        };
        let base = self.config.public_url.trim_end_matches('/');
        let link = notice.link.as_ref().map(|path| format!("{base}{path}"));
        let (headline, text) = posted_text(
            sandbox.as_ref().map(|s| s.repo_name.as_str()),
            &notice.title,
            notice.detail.as_deref(),
            link.as_deref(),
        );
        Some(Notice {
            kind,
            headline,
            text,
            about: None,
            scope: Some(chat_scope(sandbox.as_ref().map(|s| &s.id), recipient, &key)),
        })
    }

    /// A take of a task by `by` (`back`: its giver took it back), for whoever it was taken from
    /// (SPEC-510 TKC-002), under the Needs you choice: they may want it back.
    async fn taken_notice(&self, task_id: &str, by: &str, back: bool) -> AppResult<Option<Notice>> {
        let Ok(id) = TaskId::parse(task_id) else {
            return Ok(None);
        };
        let store = &self.deps.store;
        let Some(task) = store.task(&id).await? else {
            return Ok(None);
        };
        let Some(sandbox) = store.sandbox(&task.sandbox_id).await? else {
            return Ok(None);
        };
        let link = format!(
            "{}/s/{}/t/{}",
            self.config.public_url.trim_end_matches('/'),
            sandbox.slug,
            task.number
        );
        let (headline, text) = taken_text(
            by,
            task.number,
            &task.title,
            &sandbox.repo_name,
            &link,
            back,
        );
        Ok(Some(Notice {
            kind: NoticeKind::NeedsYou,
            headline: self.redact_for_sandbox(&task.sandbox_id, &headline).await,
            text: self.redact_for_sandbox(&task.sandbox_id, &text).await,
            about: Some(crate::ports::notify::SentAbout {
                task: task.id.clone(),
                asked_at: None,
            }),
            scope: Some(chat_scope(
                Some(&task.sandbox_id),
                None,
                &NoticeKey::lossy("control:", task.id.as_str()),
            )),
        }))
    }

    /// A person's request for control of a task (SPEC-490 OWN-008), for the task's owner only,
    /// once per request (TGC-027): the notification center's entry, wherever the owner is.
    async fn control_notice(&self, summary: &TaskSummary) -> AppResult<Option<(UserId, Notice)>> {
        if summary.control_request.is_none() {
            return Ok(None);
        }
        let services = self.channel_services()?;
        let Ok(id) = TaskId::parse(&summary.id) else {
            return Ok(None);
        };
        let store = &self.deps.store;
        let (Some(task), Some(request)) =
            (store.task(&id).await?, store.control_request(&id).await?)
        else {
            return Ok(None);
        };
        if services.notifier.told_once(control_subject(&id, &request)) {
            return Ok(None);
        }
        let Some(sandbox) = store.sandbox(&task.sandbox_id).await? else {
            return Ok(None);
        };
        let link = format!(
            "{}/s/{}/t/{}",
            self.config.public_url.trim_end_matches('/'),
            sandbox.slug,
            task.number
        );
        let by = self.login_of(&request.by).await;
        let (headline, text) =
            control_text(&by, task.number, &task.title, &sandbox.repo_name, &link);
        Ok(Some((
            task.owner.clone(),
            Notice {
                kind: NoticeKind::NeedsYou,
                headline: self.redact_for_sandbox(&task.sandbox_id, &headline).await,
                text: self.redact_for_sandbox(&task.sandbox_id, &text).await,
                about: Some(crate::ports::notify::SentAbout {
                    task: task.id.clone(),
                    asked_at: None,
                }),
                scope: Some(chat_scope(
                    Some(&task.sandbox_id),
                    None,
                    &NoticeKey::lossy("control:", task.id.as_str()),
                )),
            },
        )))
    }

    /// What `task` reads as for the chat now (TGC-022); `None` while it switches: a switch is under
    /// the hood (TGC-025, as the notification center: SPEC-468 CCF-008), it sends nothing and keeps
    /// what was told, so a task that reads afterwards as it did before is no news.
    async fn task_reading(
        &self,
        task: &TaskRecord,
        requested: &HashSet<String>,
    ) -> AppResult<Option<TaskReading>> {
        // A switch is under the hood (TGC-025), and so is a session starting: a parked task's
        // resume reads idle again as it did, which is no news (SPEC-512 PRK-004). What the task
        // reads once it runs is compared with what was told before.
        if matches!(
            task.status,
            yulius_domain::task::TaskStatus::Switching | yulius_domain::task::TaskStatus::Starting
        ) {
            return Ok(None);
        }
        let facts = self.task_attention_facts(task, requested).await?;
        let group = facts.group();
        let kind = task_notice(group, task.status);
        let asked = match kind {
            Some(NoticeKind::NeedsYou) => self.deps.store.task_needs_you(&task.id).await?,
            _ => None,
        };
        let notice = match kind {
            Some(NoticeKind::NeedsYou) => self.resolve_needs_you_notice(task, &facts).await,
            _ => None,
        };
        let episode_key = asked
            .as_ref()
            .map(|a| a.at.to_string())
            .or_else(|| notice.as_ref().map(|n| n.episode.clone()));
        let signature = kind.map_or_else(String::new, |kind| {
            task_signature(kind, task.status, episode_key.as_deref())
        });
        Ok(Some(TaskReading {
            kind,
            signature,
            asked,
            notice,
        }))
    }

    /// Makes `bot` the one in use from what is the case now (TGC-022): the notifier's own task
    /// seeds its memory and then activates the bot, and this returns once it did (a bounded store
    /// read). Without a notifier (or one gone meanwhile) the bot is activated here, and nothing is
    /// told.
    pub(crate) async fn activate_seeded(self: &Arc<Self>, bot: crate::telegram::ActiveBot) {
        let Ok(services) = self.channel_services() else {
            return;
        };
        let (done, seeded) = tokio::sync::oneshot::channel();
        let Some(tx) = services.notifier.seed_tx.get() else {
            self.activate(Some(bot));
            return;
        };
        // Kept for a notifier that is gone before it took or answered the request.
        let fallback = bot.clone();
        if tx.send((bot, done)).await.is_err() || seeded.await.is_err() {
            self.activate(Some(fallback));
        }
    }

    /// Seeds the notifier's memory with what is the case now: every task's reading, its
    /// sandbox-wide episode and waiting request for control, and every subscription's problem,
    /// replacing what it held in one step, so only what changes after it is told (TGC-022; review
    /// round 1: judging "first seen" by the task's state time dropped a chain blocked later, for
    /// good). Run by the notifier's own task, in order with the events (review round 2).
    async fn seed_notices(&self) {
        match self.try_seed_notices().await {
            Ok(tasks) => tracing::info!(tasks, "the notifier knows what is the case now"),
            Err(err) => {
                tracing::warn!(error = %err, "the notifier could not read what is the case")
            }
        }
    }

    async fn try_seed_notices(&self) -> AppResult<usize> {
        let services = self.channel_services()?;
        let store = &self.deps.store;
        let mut told = HashMap::new();
        let mut once = HashSet::new();
        let mut seeded = 0;
        for sandbox in store.sandboxes().await? {
            let requested = self.browser_requests(&sandbox.id).await;
            for archived in [false, true] {
                for task in store.tasks(&sandbox.id, archived).await? {
                    if let Some(reading) = self.task_reading(&task, &requested).await? {
                        let shared = reading.asked.as_ref().and_then(|a| {
                            a.cause
                                .as_deref()
                                .filter(|cause| shared_cause(cause).is_some())
                                .map(|cause| (cause.to_owned(), a.at))
                        });
                        if let Some((cause, at)) = shared {
                            let episode = self.shared_episode(&task.id, &cause, at).await;
                            once.insert(shared_subject(&task, &cause, episode));
                        }
                        told.insert(format!("task:{}", task.id), reading.signature);
                    }
                    if let Some(request) = store.control_request(&task.id).await? {
                        once.insert(control_subject(&task.id, &request));
                    }
                    seeded += 1;
                }
            }
        }
        for account in store.accounts().await? {
            let status = account.status.as_str();
            let seen = account_problem(status).map_or_else(String::new, |_| status.to_owned());
            told.insert(format!("account:{}", account.id), seen);
        }
        if let Ok(mut held) = services.notifier.told.lock() {
            *held = told;
        }
        if let Ok(mut held) = services.notifier.once.lock() {
            *held = once;
        }
        Ok(seeded)
    }

    /// The notification a task's change calls for (TGC-022), once per new state.
    async fn task_event_notice(&self, summary: &TaskSummary) -> AppResult<Option<Notice>> {
        let services = self.channel_services()?;
        let Ok(id) = TaskId::parse(&summary.id) else {
            return Ok(None);
        };
        let store = &self.deps.store;
        let Some(task) = store.task(&id).await? else {
            return Ok(None);
        };
        // A start ends what was told of the run before when that was its end: the next end is
        // news again. What a parked task asked is kept, so its resume is no news (SPEC-512).
        if task.status == yulius_domain::task::TaskStatus::Starting {
            services.notifier.forget_failure(&format!("task:{id}"));
        }
        let requested = self.browser_requests(&task.sandbox_id).await;
        let Some(TaskReading {
            kind,
            signature,
            asked,
            notice: resolved_notice,
        }) = self.task_reading(&task, &requested).await?
        else {
            return Ok(None);
        };
        // What was the case when the bot became active is in the memory (seeded), so a task first
        // seen here is new (TGC-022).
        let previous = services
            .notifier
            .remember(format!("task:{id}"), signature.clone());
        let Some(kind) = kind else {
            return Ok(None);
        };
        if previous.as_deref() == Some(signature.as_str()) {
            return Ok(None);
        }
        // A request about a problem of the whole sandbox (SPEC-494) is told once per episode, the
        // one the notification center names by its oldest open request, whichever task asked
        // (TGC-026).
        let shared = asked
            .as_ref()
            .and_then(|a| a.cause.as_deref().map(|cause| (cause, a.at)))
            .and_then(|(cause, at)| shared_cause(cause).map(|words| (cause, at, words)));
        if let Some((cause, at, _)) = shared {
            let episode = self.shared_episode(&id, cause, at).await;
            if services
                .notifier
                .told_once(shared_subject(&task, cause, episode))
            {
                return Ok(None);
            }
        }
        let Some(sandbox) = store.sandbox(&task.sandbox_id).await? else {
            return Ok(None);
        };
        let task_link = format!(
            "{}/s/{}/t/{}",
            self.config.public_url.trim_end_matches('/'),
            sandbox.slug,
            task.number
        );
        let asked_of = asked
            .as_ref()
            .map(|a| AskedOf {
                happened: a.happened.as_deref(),
                action: &a.action,
                problem: a.problem.as_deref(),
                decision: a.decision,
            })
            .or_else(|| {
                resolved_notice.as_ref().map(|n| AskedOf {
                    happened: Some(&n.happened),
                    action: &n.action,
                    problem: n.problem.as_deref(),
                    decision: n.decision,
                })
            });
        let link = asked
            .as_ref()
            .and_then(|a| a.link.as_deref())
            .or_else(|| resolved_notice.as_ref().map(|n| n.link.as_str()))
            .unwrap_or(&task_link);
        let notice = TaskNotice {
            kind,
            status: task.status,
            harness: task.harness,
            sandbox: &sandbox.repo_name,
            number: task.number,
            title: &task.title,
            asked: asked_of,
            shared: shared.map(|(_, _, words)| words),
            link,
        };
        // The same redaction as the sandbox's transcripts and notices (TGC-023, SPEC-578):
        // vault values plus generic credential shapes.
        Ok(Some(Notice {
            kind,
            headline: self
                .redact_notice_text(&task.sandbox_id, &notice.headline())
                .await,
            text: self
                .redact_notice_text(&task.sandbox_id, &notice.text())
                .await,
            about: Some(crate::ports::notify::SentAbout {
                task: task.id.clone(),
                asked_at: asked.as_ref().map(|a| a.at),
            }),
            // One message per task, or per sandbox-wide problem (the spec's table).
            scope: Some(chat_scope(
                Some(&task.sandbox_id),
                None,
                &shared.map_or_else(
                    || NoticeKey::task(task.id.as_str()),
                    |(cause, _, _)| NoticeKey::lossy("needs-you:", cause),
                ),
            )),
        }))
    }

    /// A subscription's problem (TGC-020), once per status.
    fn account_notice(&self, account: &AccountView) -> Option<Notice> {
        let services = self.channel_services().ok()?;
        let problem = account_problem(&account.status);
        let previous = services.notifier.remember(
            format!("account:{}", account.id),
            problem.map(|_| account.status.clone()).unwrap_or_default(),
        );
        let problem = problem?;
        if previous.as_deref() == Some(account.status.as_str()) {
            return None;
        }
        let link = format!(
            "{}/admin/accounts",
            self.config.public_url.trim_end_matches('/')
        );
        let (headline, text) = account_text(
            &account.label,
            problem,
            account.last_error.as_deref(),
            &link,
        );
        // No sandbox: the instance-wide redaction of credential shapes (TGC-023).
        let redact = |text: String| yulius_domain::redact::redact(&text).map_or(text, |r| r.text);
        Some(Notice {
            kind: NoticeKind::Account,
            headline: redact(headline),
            text: redact(text),
            about: None,
            scope: Some(chat_scope(
                None,
                None,
                &NoticeKey::lossy("account:", &account.id),
            )),
        })
    }

    /// Sends `notice` to every linked person (or `only` that one) who chose its kind and whose
    /// event stream carries `envelope` (TGC-020, TGC-021), under each one's rate bound (TGC-024).
    async fn deliver(
        &self,
        envelope: &Envelope,
        notice: &Notice,
        only: Option<&UserId>,
    ) -> AppResult<()> {
        let services = self.channel_services()?;
        let store = &self.deps.store;
        for link in store.chat_links().await? {
            if only.is_some_and(|only| *only != link.user) || !link.choices.wants(notice.kind) {
                continue;
            }
            let Some(person) = store.user(&link.user).await? else {
                continue;
            };
            if person.disabled_at.is_some() {
                continue;
            }
            if !self
                .envelope_reaches(&person, envelope, &mut Reach::default())
                .await
            {
                continue;
            }
            let now = self.now().0;
            let send = services.notifier.throttles.lock().is_ok_and(|mut all| {
                let (chat, throttle) = all.entry(link.user.clone()).or_insert_with(|| {
                    (link.chat.clone(), Throttle::new(services.tuning.window_ms))
                });
                chat.clone_from(&link.chat);
                throttle.offer(now, notice.headline.clone())
            });
            if send {
                self.enqueue_keyed(
                    link.chat.clone(),
                    notice.text.clone(),
                    notice.about.clone(),
                    notice.scope.clone(),
                );
            }
        }
        Ok(())
    }

    /// Sends each person's summary of what their bound held, once their window ended, only to the
    /// chat still linked to them while they are enabled: an unlinked, relinked or disabled
    /// person's held notices are dropped (review round 1).
    async fn flush_notices(&self) {
        let Ok(services) = self.channel_services() else {
            return;
        };
        let now = self.now().0;
        let link = self.config.public_url.trim_end_matches('/').to_owned();
        let mut due = Vec::new();
        if let Ok(mut all) = services.notifier.throttles.lock() {
            for (user, (chat, throttle)) in all.iter_mut() {
                if let Some(summary) = throttle.flush(now, &link) {
                    due.push((user.clone(), chat.clone(), summary));
                }
            }
            all.retain(|_, (_, throttle)| !throttle.idle(now));
        }
        let store = &self.deps.store;
        for (user, chat, summary) in due {
            let linked = store
                .chat_link(&user)
                .await
                .ok()
                .flatten()
                .is_some_and(|l| l.chat == chat);
            let enabled = store
                .user(&user)
                .await
                .ok()
                .flatten()
                .is_some_and(|u| u.disabled_at.is_none());
            if linked && enabled {
                self.enqueue(chat, summary);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The episodes and requests for control told are bounded: a full memory starts again.
    #[test]
    fn the_once_memory_is_bounded() {
        let state = NotifierState::default();
        for n in 0..ONCE_MAX {
            assert!(!state.told_once(format!("control:{n}")));
        }
        assert!(state.told_once("control:0".into()), "a known subject fits");
        assert!(!state.told_once("control:new".into()));
        assert_eq!(state.once.lock().map(|o| o.len()).unwrap_or(0), 1);
        assert!(state.told_once("control:new".into()));
    }
}
