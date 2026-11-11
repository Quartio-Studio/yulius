//! What reaches a person's own chat (SPEC-444 TGC-020..TGC-024, ADR-0287): the notifier reads
//! the UI event bus, keeps what the notification center would tell (a task entering Needs you,
//! a new `needs_you` request, Done, an abnormal end, a subscription problem), sends it only to
//! the linked people whose event stream carries that event and who chose that kind, redacted
//! like transcripts, and folds a burst under the per-person rate bound. It speaks to the channel
//! only through the send queue (`telegram.rs`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::broadcast::error::RecvError;
use yulius_contracts::api::admin::AccountView;
use yulius_contracts::api::tasks::TaskSummary;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::notify::{
    AskedOf, NoticeKind, TaskNotice, Throttle, account_problem, account_text, task_notice,
    task_signature,
};
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::error::AppResult;
use crate::events::{Envelope, Reach};

/// Subjects whose last notification is remembered; beyond it the memory starts again.
const TOLD_MAX: usize = 20_000;

/// The notifier's memory, in this process only.
#[derive(Debug, Default)]
pub(crate) struct NotifierState {
    /// When the notifier started: a task first seen later is told only of a state entered since.
    started_at: OnceLock<Timestamp>,
    /// The last signature told (or seen) per subject (`task:<id>`, `account:<id>`).
    told: Mutex<HashMap<String, String>>,
    /// Each person's chat and rate bound.
    throttles: Mutex<HashMap<UserId, (String, Throttle)>>,
}

/// One notification, redacted.
struct Notice {
    kind: NoticeKind,
    headline: String,
    text: String,
}

impl NotifierState {
    /// Records `signature` for `subject`; the earlier one (`None` when first seen).
    fn remember(&self, subject: String, signature: String) -> Option<String> {
        let Ok(mut told) = self.told.lock() else {
            return None;
        };
        if told.len() >= TOLD_MAX && !told.contains_key(&subject) {
            told.clear();
        }
        told.insert(subject, signature)
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
        let _ = services.notifier.started_at.set(self.now());
        let flush_every = services.tuning.flush_every;
        let mut rx = self.events.subscribe();
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut flush = tokio::time::interval(flush_every);
            flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            flush.tick().await;
            loop {
                tokio::select! {
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
            UiEvent::TaskUpdated { .. } | UiEvent::AccountUpdated { .. }
        ) {
            return;
        }
        // Nobody linked: nothing to read or remember (keeps the notifier ahead of the bus).
        if self
            .deps
            .store
            .chat_links()
            .await
            .map_or(true, |links| links.is_empty())
        {
            return;
        }
        let notice = match &envelope.event {
            UiEvent::TaskUpdated { task } => self.task_event_notice(task).await,
            UiEvent::AccountUpdated { account } => Ok(self.account_notice(account)),
            _ => Ok(None),
        };
        match notice {
            Ok(Some(notice)) => {
                if let Err(err) = self.deliver(envelope, &notice).await {
                    tracing::warn!(error = %err, "a notification could not be delivered");
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(error = %err, "a task's notification could not be read"),
        }
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
        let requested = self.browser_requests(&task.sandbox_id).await;
        let group = self.task_group(&task, &requested).await?;
        let kind = task_notice(group, task.status);
        let asked = match kind {
            Some(NoticeKind::NeedsYou) => store.task_needs_you(&id).await?,
            _ => None,
        };
        let signature = kind.map(|kind| {
            task_signature(
                kind,
                task.status,
                asked.as_ref().map(|a| a.at.to_string()).as_deref(),
            )
        });
        let previous = services
            .notifier
            .remember(format!("task:{id}"), signature.clone().unwrap_or_default());
        let (Some(kind), Some(signature)) = (kind, signature) else {
            return Ok(None);
        };
        if previous.as_deref() == Some(signature.as_str()) {
            return Ok(None);
        }
        if previous.is_none() {
            // First seen since the start: told only of a state entered after it.
            let state_at = store.task_times(&id).await?.map_or(0, |t| t.state_at.0);
            let since = state_at.max(asked.as_ref().map_or(0, |a| a.at));
            let started = services.notifier.started_at.get().map_or(0, |t| t.0);
            if since < started {
                return Ok(None);
            }
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
        let notice = TaskNotice {
            kind,
            status: task.status,
            sandbox: &sandbox.repo_name,
            number: task.number,
            title: &task.title,
            asked: asked.as_ref().map(|a| AskedOf {
                action: &a.action,
                problem: a.problem.as_deref(),
                decision: a.decision,
            }),
            link: &link,
        };
        // The same redaction as the sandbox's transcripts (TGC-023).
        Ok(Some(Notice {
            kind,
            headline: self
                .redact_for_sandbox(&task.sandbox_id, &notice.headline())
                .await,
            text: self
                .redact_for_sandbox(&task.sandbox_id, &notice.text())
                .await,
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
        })
    }

    /// Sends `notice` to every linked person who chose its kind and whose event stream carries
    /// `envelope` (TGC-020, TGC-021), under each one's rate bound (TGC-024).
    async fn deliver(&self, envelope: &Envelope, notice: &Notice) -> AppResult<()> {
        let services = self.channel_services()?;
        let store = &self.deps.store;
        for link in store.chat_links().await? {
            if !link.choices.wants(notice.kind) {
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
                self.enqueue(link.chat.clone(), notice.text.clone());
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
