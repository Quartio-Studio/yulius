//! The personal Telegram channel (SPEC-498, ADR-0351): the instance's bot, set and removed by an
//! instance administrator; each person's chat, linked with a single-use code a browser session
//! asks for; and the long poll that receives `/start <code>` and `/stop`.
//!
//! The token is checked with `getMe`, sealed with the master key like the Jev and Vidoc keys, in
//! effect at once, and never logged, audited or returned. Everything here talks to the bot through
//! the [`NotificationChannel`] port; what is worth a message is `notify.rs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use yulius_contracts::api::telegram::{
    MyTelegramView, TelegramBotView, TelegramChoices, TelegramIntake, TelegramLinkCode,
};
use yulius_domain::ids::UserId;
use yulius_domain::notify::{ChatCommand, LINK_CODE_LEN, LINK_CODE_TTL_MS, NotifyChoices};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::Secret;
use crate::ports::notify::{
    ChannelConnector, ChannelError, ChannelIdentity, ChatLink, Inbound, NotificationChannel,
};

/// The one reply of a chat that is not linked (TGC-014).
pub const UNLINKED_REPLY: &str = "This bot sends Yulius notifications to linked accounts only. \
     To link yours, open Yulius, then Settings → Notifications → Link Telegram.";
/// The reply of a linked chat to anything but a command while its intake is off (SPEC-512
/// TGI-001).
pub const LINKED_HELP: &str = "This chat gets your Yulius notifications. To answer tasks and \
     start new ones from here, turn on Telegram intake in Yulius: Settings → Notifications. Send \
     /stop to unlink this chat.";
/// The reply to a code that is not valid (any more).
pub const BAD_CODE_REPLY: &str =
    "That link code is not valid any more. Ask for a new one in Yulius: Settings → Notifications.";
/// The audit target of the bot's changes; a link's is its person's id.
pub const AUDIT_TARGET: &str = "telegram";
/// How often an unlinked chat gets its reply at most (TGC-014).
const QUIET_MS: i64 = 10 * 60 * 1000;
/// Chats remembered for that (TGC-014).
const QUIET_CHATS: usize = 1024;
/// A chat's intake messages waiting behind the one being handled, at most: a whole poll answer
/// (SPEC-512 TGI-002).
const INTAKE_WAITING: usize = 20;
/// The answer to a chat whose messages did not all fit its lane (at most every `QUIET_MS`).
pub const INTAKE_FULL_REPLY: &str = "Too many messages at once: Yulius is still working through \
     your earlier ones, so some of the latest were not handled. Send them again once it has \
     answered.";
/// Messages waiting to be sent (TGC-024).
const OUTBOX: usize = 256;
/// Bound of one `getMe` or `sendMessage` (the client's own deadline is shorter).
const CALL_DEADLINE: Duration = Duration::from_secs(15);
/// Letters of a link code: no look-alikes, 32 of them, so 16 carry 80 bits.
const CODE_ALPHABET: &[u8; 32] = b"abcdefghjkmnpqrstuvwxyz23456789A";

/// Timings of the channel, shorter in tests.
#[derive(Debug, Clone, Copy)]
pub struct ChannelTuning {
    /// The first wait after a failed poll (TGC-003).
    pub backoff_min: Duration,
    /// The longest wait.
    pub backoff_max: Duration,
    /// The rate bound's window in milliseconds of the app's clock (TGC-024).
    pub window_ms: i64,
    /// How often held notifications are checked for their summary.
    pub flush_every: Duration,
}

impl Default for ChannelTuning {
    fn default() -> Self {
        Self {
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            window_ms: yulius_domain::notify::WINDOW_MS,
            flush_every: Duration::from_secs(2),
        }
    }
}

/// The bot in use.
#[derive(Clone)]
pub(crate) struct ActiveBot {
    pub(crate) channel: Arc<dyn NotificationChannel>,
    pub(crate) identity: ChannelIdentity,
}

#[derive(Debug, Default)]
struct PollState {
    polling: bool,
    last_error: Option<ChannelError>,
}

/// A message for a chat, and the task it is about (a reply to it reaches that task, SPEC-512
/// TGI-002).
pub(crate) struct Outgoing {
    pub(crate) chat: String,
    pub(crate) text: String,
    pub(crate) about: Option<crate::ports::notify::SentAbout>,
    /// A keyed notice's scope (SPEC-624 NTC-007): once sent, it replaces the bot's earlier
    /// message with this scope in the chat.
    pub(crate) scope: Option<String>,
}

/// A linked chat's message for intake, and the link it came through.
type Intake = (ChatLink, Inbound);

/// What became of a message handed to its chat's lane.
enum Lane {
    /// It waits behind the chat's running worker.
    Queued,
    /// No worker ran: start one with this queue.
    Start(mpsc::Receiver<Intake>),
    /// The lane is full.
    Full,
}

/// The channel services an instance runs with.
pub struct ChannelServices {
    connector: Arc<dyn ChannelConnector>,
    pub(crate) tuning: ChannelTuning,
    active: RwLock<Option<ActiveBot>>,
    poller: Mutex<Option<JoinHandle<()>>>,
    poll: Mutex<PollState>,
    quiet: Mutex<HashMap<String, i64>>,
    /// One lane per chat with messages in hand: its worker handles them one at a time, in
    /// order, [`INTAKE_WAITING`] waiting at most (TGI-002). The worker removes it when empty.
    lanes: Mutex<HashMap<String, mpsc::Sender<Intake>>>,
    outbox: OnceLock<mpsc::Sender<Outgoing>>,
    /// Serializes set and remove, so the stored and the active bot agree.
    switching: tokio::sync::Mutex<()>,
    pub(crate) notifier: crate::notify::NotifierState,
    /// Read by every seed; only a test writes it, to hold seeds ([`ChannelServices::seed_hold`]).
    pub(crate) seeds: Arc<tokio::sync::RwLock<()>>,
}

impl ChannelServices {
    /// Services connecting bots with `connector`.
    #[must_use]
    pub fn new(connector: Arc<dyn ChannelConnector>, tuning: ChannelTuning) -> Self {
        Self {
            connector,
            tuning,
            active: RwLock::new(None),
            poller: Mutex::new(None),
            poll: Mutex::new(PollState::default()),
            quiet: Mutex::new(HashMap::new()),
            lanes: Mutex::new(HashMap::new()),
            outbox: OnceLock::new(),
            switching: tokio::sync::Mutex::new(()),
            notifier: crate::notify::NotifierState::default(),
            seeds: Arc::new(tokio::sync::RwLock::new(())),
        }
    }

    /// For tests: while its write guard is held, no seed of the notifier runs (a hold point, so
    /// a test can act while a bot's activation waits for its seed). The server never writes it.
    #[must_use]
    pub fn seed_hold(&self) -> Arc<tokio::sync::RwLock<()>> {
        Arc::clone(&self.seeds)
    }

    pub(crate) fn active(&self) -> Option<ActiveBot> {
        self.active.read().ok()?.clone()
    }

    fn set_poll(&self, polling: bool, last_error: Option<ChannelError>) {
        if let Ok(mut poll) = self.poll.lock() {
            poll.polling = polling;
            poll.last_error = last_error;
        }
    }

    fn lanes(&self) -> std::sync::MutexGuard<'_, HashMap<String, mpsc::Sender<Intake>>> {
        self.lanes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Hands `item` to its chat's lane.
    fn to_lane(&self, item: &Intake) -> Lane {
        let mut lanes = self.lanes();
        if let Some(lane) = lanes.get(&item.1.chat) {
            match lane.try_send(item.clone()) {
                Ok(()) => return Lane::Queued,
                Err(mpsc::error::TrySendError::Full(_)) => return Lane::Full,
                // Its worker is gone (it panicked): a new one takes over.
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
        let (tx, rx) = mpsc::channel(INTAKE_WAITING);
        lanes.insert(item.1.chat.clone(), tx);
        Lane::Start(rx)
    }

    /// The next message waiting in `chat`'s lane; with none, the lane is removed in the same
    /// step, so a message handed over meanwhile starts a new worker instead of being lost.
    fn next_in_lane(&self, chat: &str, rx: &mut mpsc::Receiver<Intake>) -> Option<Intake> {
        let mut lanes = self.lanes();
        let next = rx.try_recv().ok();
        if next.is_none() {
            lanes.remove(chat);
        }
        next
    }

    /// Whether `chat` gets the unlinked reply now (at most every [`QUIET_MS`]).
    fn quiet_reply_due(&self, chat: &str, now: i64) -> bool {
        let Ok(mut quiet) = self.quiet.lock() else {
            return false;
        };
        if quiet
            .get(chat)
            .is_some_and(|at| now.saturating_sub(*at) < QUIET_MS)
        {
            return false;
        }
        if quiet.len() >= QUIET_CHATS {
            quiet.retain(|_, at| now.saturating_sub(*at) < QUIET_MS);
            if quiet.len() >= QUIET_CHATS {
                return false;
            }
        }
        quiet.insert(chat.to_owned(), now);
        true
    }
}

fn choices_view(choices: NotifyChoices) -> TelegramChoices {
    TelegramChoices {
        needs_you: choices.needs_you,
        done: choices.done,
        failed: choices.failed,
        account: choices.account,
        digest: choices.digest,
    }
}

impl App {
    /// Installs the channel services (composition); `false` when already set.
    pub fn set_channel(&self, services: ChannelServices) -> bool {
        self.channel.set(services).is_ok()
    }

    pub(crate) fn channel_services(&self) -> AppResult<&ChannelServices> {
        self.channel
            .get()
            .ok_or_else(|| AppError::Conflict("Telegram is not available on this server".into()))
    }

    /// Starts the channel: the sender, the notifier and, with a saved bot, its long poll. The
    /// saved token is not checked here, so an unreachable Telegram does not hold the start.
    pub async fn start_channel(self: &Arc<Self>) {
        let Ok(services) = self.channel_services() else {
            return;
        };
        let (tx, rx) = mpsc::channel(OUTBOX);
        if services.outbox.set(tx).is_err() {
            return;
        }
        tokio::spawn(Arc::clone(self).send_loop(rx));
        self.start_notifier();
        let saved = match self.deps.store.chat_bot().await {
            Ok(saved) => saved,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the saved Telegram bot");
                None
            }
        };
        let Some(stored) = saved else {
            tracing::info!("the Telegram channel is off: no bot");
            return;
        };
        let token = self
            .deps
            .crypto
            .open(&stored.sealed)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(Secret::new);
        let Some(token) = token else {
            tracing::warn!("the saved Telegram bot token cannot be opened with this master key");
            return;
        };
        match services.connector.connect(&token) {
            Ok(channel) => {
                tracing::info!(bot = %stored.identity.username, "the Telegram channel is on");
                // What is the case at the start is not news (TGC-022).
                self.activate_seeded(ActiveBot {
                    channel,
                    identity: stored.identity,
                })
                .await;
            }
            Err(err) => tracing::warn!("the saved Telegram bot token is not usable: {err}"),
        }
    }

    /// Makes `bot` the one in use: the earlier poll ends at once, a new one starts.
    pub(crate) fn activate(self: &Arc<Self>, bot: Option<ActiveBot>) {
        let Ok(services) = self.channel_services() else {
            return;
        };
        if let Ok(mut poller) = services.poller.lock() {
            if let Some(handle) = poller.take() {
                handle.abort();
            }
            if let Ok(mut active) = services.active.write() {
                active.clone_from(&bot);
            }
            services.set_poll(false, None);
            if let Some(bot) = bot {
                *poller = Some(tokio::spawn(Arc::clone(self).poll_loop(bot)));
            }
        }
    }

    /// Queues `text` for `chat`; a full queue drops it (TGC-024).
    pub(crate) fn enqueue(&self, chat: String, text: String) {
        self.enqueue_about(chat, text, None);
    }

    /// [`App::enqueue`] for a message about `task`: once sent, a reply to it reaches the task
    /// (SPEC-512 TGI-002).
    pub(crate) fn enqueue_about(
        &self,
        chat: String,
        text: String,
        about: Option<crate::ports::notify::SentAbout>,
    ) {
        self.enqueue_keyed(chat, text, about, None);
    }

    /// [`App::enqueue_about`] for a keyed notice (SPEC-624 NTC-007): once sent, the bot's earlier
    /// message with `scope` in the chat is deleted, best effort.
    pub(crate) fn enqueue_keyed(
        &self,
        chat: String,
        text: String,
        about: Option<crate::ports::notify::SentAbout>,
        scope: Option<String>,
    ) {
        let Some(outbox) = self.channel.get().and_then(|s| s.outbox.get()) else {
            return;
        };
        let message = Outgoing {
            chat,
            text,
            about,
            scope,
        };
        if outbox.try_send(message).is_err() {
            tracing::warn!("a Telegram notification was dropped: the send queue is full");
        }
    }

    async fn send_loop(self: Arc<Self>, mut rx: mpsc::Receiver<Outgoing>) {
        while let Some(message) = rx.recv().await {
            let Some(bot) = self.channel.get().and_then(ChannelServices::active) else {
                continue;
            };
            let sent = tokio::time::timeout(
                CALL_DEADLINE,
                bot.channel.send(&message.chat, &message.text),
            )
            .await
            .unwrap_or(Err(ChannelError::Unavailable));
            match sent {
                Ok(Some(id)) => {
                    if let Some(about) = &message.about
                        && let Err(err) = self
                            .deps
                            .store
                            .record_sent(&message.chat, &id, about, self.now())
                            .await
                    {
                        tracing::warn!(error = %err, "a sent Telegram message was not remembered");
                    }
                    if let Some(scope) = &message.scope {
                        self.replace_keyed(&bot, &message.chat, scope, &id).await;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(error = err.as_str(), "a Telegram message could not be sent");
                }
            }
        }
    }

    /// Remembers `id` as the bot's message for `scope` in `chat` and deletes the one it replaces
    /// (SPEC-624 NTC-007). A delete the service refuses (an old message) leaves it in the chat.
    async fn replace_keyed(&self, bot: &ActiveBot, chat: &str, scope: &str, id: &str) {
        let earlier = match self
            .deps
            .store
            .swap_keyed_message(chat, scope, id, self.now())
            .await
        {
            Ok(earlier) => earlier,
            Err(err) => {
                tracing::warn!(error = %err, "a keyed Telegram message was not remembered");
                return;
            }
        };
        let Some(earlier) = earlier else {
            return;
        };
        let deleted = tokio::time::timeout(CALL_DEADLINE, bot.channel.delete(chat, &earlier))
            .await
            .unwrap_or(Err(ChannelError::Unavailable));
        if let Err(err) = deleted {
            tracing::debug!(error = err.as_str(), "a replaced Telegram message stays");
        }
    }

    /// Receives with exponential backoff after errors (TGC-003), until aborted.
    async fn poll_loop(self: Arc<Self>, bot: ActiveBot) {
        let Ok(services) = self.channel_services() else {
            return;
        };
        let tuning = services.tuning;
        let mut cursor: Option<String> = None;
        let mut delay = tuning.backoff_min;
        loop {
            match bot.channel.receive(cursor.as_deref()).await {
                Ok(received) => {
                    services.set_poll(true, None);
                    delay = tuning.backoff_min;
                    if received.cursor.is_some() {
                        cursor = received.cursor;
                    }
                    for message in albums(received.messages) {
                        if let Err(err) = self.on_inbound(&message).await {
                            tracing::warn!(error = %err, "a Telegram message could not be handled");
                        }
                    }
                }
                Err(err) => {
                    services.set_poll(false, Some(err));
                    tracing::warn!(
                        error = err.as_str(),
                        retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "the Telegram poll failed"
                    );
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(tuning.backoff_max);
                }
            }
        }
    }

    /// One received message (TGC-012..014): `/start <code>` links, `/stop` unlinks, a linked chat
    /// gets help, any other chat the one fixed reply.
    async fn on_inbound(self: &Arc<Self>, message: &Inbound) -> AppResult<()> {
        let store = &self.deps.store;
        let now = self.now();
        let command = yulius_domain::notify::chat_command(&message.text);
        if message.private
            && let ChatCommand::Start(Some(code)) = command
            && yulius_domain::notify::is_link_code(code)
        {
            let hash = self.deps.crypto.sha256_hex(code.as_bytes());
            if let Some(user) = store.take_link_code(&hash, now).await? {
                return self.link_chat(&user, &message.chat).await;
            }
        }
        let link = if message.private {
            store.chat_link_of(&message.chat).await?
        } else {
            None
        };
        let Some(link) = link else {
            let services = self.channel_services()?;
            if services.quiet_reply_due(&message.chat, now.0) {
                self.enqueue(message.chat.clone(), UNLINKED_REPLY.into());
            }
            return Ok(());
        };
        let reply = match command {
            ChatCommand::Stop => {
                store.unlink_user(&link.user).await?;
                let login = self.login_of(&link.user).await;
                store
                    .audit(
                        now,
                        &login,
                        "telegram.unlinked",
                        Some(link.user.as_str()),
                        Some("telegram"),
                    )
                    .await?;
                "Unlinked: this chat gets no more Yulius notifications.".to_owned()
            }
            // With intake on, the message is the person's work (SPEC-512 TGI-002..TGI-004), on
            // its own task: fetching files must not hold the poll (and with it every chat's
            // `/stop` and link codes), and the next poll confirms the update at once, so a
            // restart does not hand the same message over twice (review round 1).
            ChatCommand::Other if link.intake => {
                let services = self.channel_services()?;
                let item = (link, message.clone());
                match services.to_lane(&item) {
                    Lane::Queued => {}
                    Lane::Start(rx) => {
                        tokio::spawn(Arc::clone(self).intake_lane(item, rx));
                    }
                    Lane::Full => {
                        tracing::warn!(
                            waiting = INTAKE_WAITING,
                            "a Telegram message was not handled: its chat's lane is full"
                        );
                        if services.quiet_reply_due(&message.chat, now.0) {
                            self.enqueue(message.chat.clone(), INTAKE_FULL_REPLY.into());
                        }
                    }
                }
                return Ok(());
            }
            // Anything else gets its answer at most every 10 minutes, so no chat fills the
            // shared send queue (review round 1).
            ChatCommand::Start(_) | ChatCommand::Other => {
                if !self
                    .channel_services()?
                    .quiet_reply_due(&message.chat, now.0)
                {
                    return Ok(());
                }
                if matches!(command, ChatCommand::Start(Some(_))) {
                    BAD_CODE_REPLY.to_owned()
                } else if link.intake {
                    crate::telegram_intake::INTAKE_HELP.to_owned()
                } else {
                    LINKED_HELP.to_owned()
                }
            }
        };
        self.enqueue(message.chat.clone(), reply);
        Ok(())
    }

    /// A chat's lane worker: handles `first`, then each message waiting, in order (TGI-002).
    async fn intake_lane(self: Arc<Self>, first: Intake, mut rx: mpsc::Receiver<Intake>) {
        let mut item = first;
        loop {
            let (link, message) = &item;
            match self.intake_in_turn(link, message).await {
                Ok(Some(answer)) => {
                    self.enqueue_about(message.chat.clone(), answer.text, answer.about);
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "a Telegram message could not be handled");
                    self.enqueue(
                        message.chat.clone(),
                        "Yulius could not handle that message; try again.".into(),
                    );
                }
            }
            let Ok(services) = self.channel_services() else {
                return;
            };
            match services.next_in_lane(&message.chat, &mut rx) {
                Some(next) => item = next,
                None => return,
            }
        }
    }

    /// A message whose turn in its lane came: handled only while the link it came through still
    /// holds with intake on (TGI-001: turning intake off or `/stop` ends what waits too). Else it
    /// gets the answer it would get now, at most every `QUIET_MS`: the help with intake off, the
    /// unlinked reply without a link; a message of an earlier link of the chat gets nothing.
    async fn intake_in_turn(
        self: &Arc<Self>,
        received: &ChatLink,
        message: &Inbound,
    ) -> AppResult<Option<crate::telegram_intake::IntakeAnswer>> {
        let now = self.deps.store.chat_link_of(&message.chat).await?;
        let reply = match now {
            Some(link) if link.user == received.user && link.linked_at == received.linked_at => {
                if link.intake {
                    return self.telegram_intake(&link, message).await.map(Some);
                }
                LINKED_HELP
            }
            Some(_) => return Ok(None),
            None => UNLINKED_REPLY,
        };
        let due = self
            .channel_services()?
            .quiet_reply_due(&message.chat, self.now().0);
        Ok(due.then(|| reply.into()))
    }

    async fn link_chat(&self, user: &UserId, chat: &str) -> AppResult<()> {
        let store = &self.deps.store;
        let now = self.now();
        let Some(person) = store.user(user).await?.filter(|u| u.disabled_at.is_none()) else {
            self.enqueue(chat.to_owned(), BAD_CODE_REPLY.into());
            return Ok(());
        };
        if let Some(earlier) = store.link_chat(user, chat, now).await? {
            let login = self.login_of(&earlier).await;
            store
                .audit(
                    now,
                    &login,
                    "telegram.unlinked",
                    Some(earlier.as_str()),
                    Some("relinked"),
                )
                .await?;
        }
        let login = person.label();
        store
            .audit(
                now,
                &login,
                "telegram.linked",
                Some(user.as_str()),
                Some("telegram"),
            )
            .await?;
        self.enqueue(
            chat.to_owned(),
            format!(
                "Linked: this chat now gets Yulius notifications for {login}. Choose which in \
                 Settings → Notifications. Send /stop to unlink."
            ),
        );
        Ok(())
    }

    /// Whether the bot's long poll runs now (it ends at once when the bot is removed).
    #[must_use]
    pub fn telegram_polling(&self) -> bool {
        self.channel
            .get()
            .and_then(|s| {
                s.poller
                    .lock()
                    .ok()
                    .map(|p| p.as_ref().is_some_and(|h| !h.is_finished()))
            })
            .unwrap_or(false)
    }

    /// `GET /api/admin/telegram` (instance administrators): the bot's state, never its token.
    ///
    /// # Errors
    /// `Forbidden` for others, `Conflict` without the channel; store failures.
    pub async fn telegram_bot_view(&self, ctx: &AuthContext) -> AppResult<TelegramBotView> {
        self.require_instance_admin(ctx)?;
        let services = self.channel_services()?;
        let saved = self.deps.store.chat_bot().await?;
        let linked = self.deps.store.chat_links().await?.len();
        let (polling, last_error) = services
            .poll
            .lock()
            .map(|p| (p.polling, p.last_error))
            .unwrap_or((false, None));
        let active = services.active().is_some();
        // `polling` once a poll was answered; before that `starting` (review round 1).
        let state = match (active, polling, last_error) {
            (false, _, _) => "off",
            (true, _, Some(_)) => "retrying",
            (true, true, None) => "polling",
            (true, false, None) => "starting",
        };
        Ok(TelegramBotView {
            configured: saved.is_some(),
            bot_username: saved.as_ref().map(|s| s.identity.username.clone()),
            set_at: saved.as_ref().map(|s| rfc3339(s.set_at)),
            set_by: saved.and_then(|s| s.set_by),
            state: state.into(),
            last_error: last_error.filter(|_| active).map(|e| e.as_str().to_owned()),
            linked: u64::try_from(linked).unwrap_or(u64::MAX),
        })
    }

    /// `PUT /api/admin/telegram` (instance administrators): checks `token` with `getMe`, saves it
    /// sealed and uses it at once (TGC-001). Another bot's token unlinks every chat (TGC-002).
    ///
    /// # Errors
    /// `Forbidden` for others, `Invalid` for a token Telegram refuses or that cannot be one,
    /// `Upstream` when Telegram cannot be reached.
    pub async fn set_telegram_bot(
        self: &Arc<Self>,
        ctx: &AuthContext,
        token: &Secret,
    ) -> AppResult<TelegramBotView> {
        self.require_instance_admin(ctx)?;
        let services = self.channel_services()?;
        let token = Secret::new(token.expose().trim().to_owned());
        if token.expose().is_empty() {
            return Err(AppError::Invalid(
                "paste the bot token from @BotFather".into(),
            ));
        }
        let channel = services
            .connector
            .connect(&token)
            .map_err(AppError::Invalid)?;
        let identity = match tokio::time::timeout(CALL_DEADLINE, channel.check()).await {
            Ok(Ok(identity)) => identity,
            Ok(Err(ChannelError::Refused)) => {
                return Err(AppError::Invalid(
                    "Telegram refused this token. Copy it again from @BotFather.".into(),
                ));
            }
            Ok(Err(_)) | Err(_) => {
                return Err(AppError::Upstream(
                    "Telegram could not be reached. Nothing was saved; try again.".into(),
                ));
            }
        };
        let sealed = self.deps.crypto.seal(token.expose().as_bytes())?;
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        // The switch runs on its own task, so a request that goes away (hyper drops its handler)
        // never leaves it half done: the lock is held until the bot is active and audited, and
        // a remove after it is never overtaken (review round 4).
        let app = Arc::clone(self);
        let bot = ActiveBot { channel, identity };
        tokio::spawn(async move { app.switch_bot(bot, sealed, login, now).await })
            .await
            .map_err(|_| AppError::Internal("the Telegram bot could not be switched".into()))??;
        self.telegram_bot_view(ctx).await
    }

    /// Saves `bot` and makes it the one in use, under the switch lock (TGC-001, TGC-002).
    async fn switch_bot(
        self: Arc<Self>,
        bot: ActiveBot,
        sealed: crate::model::Sealed,
        login: String,
        now: yulius_domain::time::Timestamp,
    ) -> AppResult<()> {
        let services = self.channel_services()?;
        let identity = bot.identity.clone();
        let _switching = services.switching.lock().await;
        let earlier = self.deps.store.chat_bot().await?;
        let unlinked = if earlier.is_some_and(|e| e.identity.id == identity.id) {
            0
        } else {
            // The earlier bot's poll ends first, so no `/start` it handles links a chat after.
            self.activate(None);
            self.deps.store.clear_chat_links().await?
        };
        self.deps
            .store
            .set_chat_bot(Some((&sealed, &identity, &login)), now)
            .await?;
        let detail = format!("@{} (unlinked {unlinked})", identity.username);
        // A bot that becomes active starts from what is the case now (TGC-022); the same bot set
        // again keeps the memory it has.
        if services.active().is_none() {
            self.activate_seeded(bot).await;
        } else {
            self.activate(Some(bot));
        }
        self.deps
            .store
            .audit(
                now,
                &login,
                "telegram.bot_set",
                Some(AUDIT_TARGET),
                Some(&detail),
            )
            .await?;
        tracing::info!("the Telegram channel is on (bot set in settings)");
        Ok(())
    }

    /// `DELETE /api/admin/telegram` (instance administrators): stops the channel at once and
    /// unlinks every chat (TGC-002).
    ///
    /// # Errors
    /// `Forbidden` for others; store failures.
    pub async fn remove_telegram_bot(
        self: &Arc<Self>,
        ctx: &AuthContext,
    ) -> AppResult<TelegramBotView> {
        self.require_instance_admin(ctx)?;
        let now = self.now();
        let login = self.login_of(&ctx.user.id).await;
        // On its own task, like a set: a request that goes away still stops the bot, unlinks
        // and is audited.
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let services = app.channel_services()?;
            let _switching = services.switching.lock().await;
            let had = app.deps.store.chat_bot().await?.is_some();
            app.activate(None);
            app.deps.store.set_chat_bot(None, now).await?;
            let unlinked = app.deps.store.clear_chat_links().await?;
            if had {
                app.deps
                    .store
                    .audit(
                        now,
                        &login,
                        "telegram.bot_removed",
                        Some(AUDIT_TARGET),
                        Some(&format!("unlinked {unlinked}")),
                    )
                    .await?;
                tracing::info!("the Telegram channel is off (bot removed)");
            }
            AppResult::Ok(())
        })
        .await
        .map_err(|_| AppError::Internal("the Telegram bot could not be removed".into()))??;
        self.telegram_bot_view(ctx).await
    }

    /// `GET /api/me/telegram`: the person's own link (TGC-010).
    ///
    /// # Errors
    /// Store failures.
    pub async fn my_telegram(&self, ctx: &AuthContext) -> AppResult<MyTelegramView> {
        let bot = self
            .channel
            .get()
            .and_then(ChannelServices::active)
            .map(|b| b.identity.username);
        let link = self.deps.store.chat_link(&ctx.user.id).await?;
        Ok(MyTelegramView {
            available: bot.is_some(),
            bot_username: bot,
            linked: link.is_some(),
            linked_at: link.as_ref().map(|l| rfc3339(l.linked_at)),
            intake: link
                .as_ref()
                .map_or_else(TelegramIntake::default, |l| TelegramIntake {
                    enabled: l.intake,
                    sandbox_id: l.intake_sandbox.clone(),
                }),
            choices: choices_view(link.map_or_else(NotifyChoices::default, |l| l.choices)),
            account_choice: ctx.user.is_admin(),
        })
    }

    /// `POST /api/me/telegram/link`: a single-use code valid for 10 minutes and its `t.me` link
    /// (TGC-011). Only a browser session may ask: no API token links a chat.
    ///
    /// # Errors
    /// `Forbidden` for an API token, `Conflict` without a bot.
    pub async fn telegram_link_code(&self, ctx: &AuthContext) -> AppResult<TelegramLinkCode> {
        if !ctx.is_browser() {
            return Err(AppError::Forbidden(
                "a Telegram chat is linked from Yulius in a browser, not with an API token".into(),
            ));
        }
        let bot = self
            .channel
            .get()
            .and_then(ChannelServices::active)
            .ok_or_else(|| {
                AppError::Conflict("no Telegram bot is set up on this instance".into())
            })?;
        let bytes = self.deps.crypto.random_bytes();
        let code: String = bytes
            .iter()
            .take(LINK_CODE_LEN)
            .map(|b| char::from(CODE_ALPHABET[usize::from(b & 31)]))
            .collect();
        let expires_at = self.now().plus_millis(LINK_CODE_TTL_MS);
        self.deps
            .store
            .put_link_code(
                &ctx.user.id,
                &self.deps.crypto.sha256_hex(code.as_bytes()),
                expires_at,
            )
            .await?;
        Ok(TelegramLinkCode {
            link: format!("https://t.me/{}?start={code}", bot.identity.username),
            code,
            expires_at: rfc3339(expires_at),
        })
    }

    /// `DELETE /api/me/telegram`: unlinks the person's chat, which is told (TGC-013).
    ///
    /// # Errors
    /// Store failures.
    pub async fn unlink_telegram(&self, ctx: &AuthContext) -> AppResult<MyTelegramView> {
        if let Some(chat) = self.deps.store.unlink_user(&ctx.user.id).await? {
            let login = self.login_of(&ctx.user.id).await;
            self.deps
                .store
                .audit(
                    self.now(),
                    &login,
                    "telegram.unlinked",
                    Some(ctx.user.id.as_str()),
                    Some("settings"),
                )
                .await?;
            self.enqueue(
                chat,
                "Unlinked in Yulius Settings: this chat gets no more notifications.".into(),
            );
        }
        self.my_telegram(ctx).await
    }

    /// `PUT /api/me/telegram/intake` (SPEC-512 TGI-001): whether the person's chat answers and
    /// starts tasks, and in which project; only from a browser session, as linking is, and only
    /// a project they may work in. Audited `telegram.intake`.
    ///
    /// # Errors
    /// `Forbidden` for an API token or a project they may not work in, `Invalid` when turned on
    /// without a project, `Conflict` without a linked chat.
    pub async fn set_telegram_intake(
        &self,
        ctx: &AuthContext,
        intake: &TelegramIntake,
    ) -> AppResult<MyTelegramView> {
        if !ctx.is_browser() {
            return Err(AppError::Forbidden(
                "a Telegram chat's intake is set in Yulius in a browser, not with an API token"
                    .into(),
            ));
        }
        let sandbox = match intake
            .sandbox_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            Some(id) => {
                let sid =
                    yulius_domain::ids::SandboxId::parse(id).map_err(|_| AppError::NotFound)?;
                self.require(&ctx.user, &sid, yulius_domain::permission::Action::Work)
                    .await?;
                Some(sid)
            }
            None => None,
        };
        if intake.enabled && sandbox.is_none() {
            return Err(AppError::Invalid(
                "choose the project a new message starts its task in".into(),
            ));
        }
        let sandbox = sandbox.as_ref().map(yulius_domain::ids::SandboxId::as_str);
        if !self
            .deps
            .store
            .set_chat_intake(&ctx.user.id, intake.enabled, sandbox)
            .await?
        {
            return Err(AppError::Conflict("link a Telegram chat first".into()));
        }
        let login = self.login_of(&ctx.user.id).await;
        let detail = if intake.enabled {
            format!("on {}", sandbox.unwrap_or_default())
        } else {
            "off".to_owned()
        };
        self.deps
            .store
            .audit(
                self.now(),
                &login,
                "telegram.intake",
                Some(ctx.user.id.as_str()),
                Some(&detail),
            )
            .await?;
        self.my_telegram(ctx).await
    }

    /// `PUT /api/me/telegram/choices`: what reaches the person's chat (TGC-020). Subscription
    /// problems only for instance administrators.
    ///
    /// # Errors
    /// `Conflict` without a linked chat.
    pub async fn set_telegram_choices(
        &self,
        ctx: &AuthContext,
        choices: &TelegramChoices,
    ) -> AppResult<MyTelegramView> {
        let admin = ctx.user.is_admin();
        let choices = NotifyChoices {
            needs_you: choices.needs_you,
            done: choices.done,
            failed: choices.failed,
            account: choices.account && admin,
            digest: choices.digest,
        };
        if !self
            .deps
            .store
            .set_chat_choices(&ctx.user.id, &choices)
            .await?
        {
            return Err(AppError::Conflict("link a Telegram chat first".into()));
        }
        self.my_telegram(ctx).await
    }
}

/// One message per album (SPEC-512 TGI-003): consecutive messages of a chat with the same
/// `group` are one, its text the first non-empty one and its files all of theirs.
fn albums(messages: Vec<Inbound>) -> Vec<Inbound> {
    let mut out: Vec<Inbound> = Vec::new();
    for message in messages {
        if let Some(last) = out.last_mut()
            && message.group.is_some()
            && last.group == message.group
            && last.chat == message.chat
        {
            if last.text.trim().is_empty() {
                last.text = message.text;
            }
            last.files.extend(message.files);
            continue;
        }
        out.push(message);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_alphabet_has_32_distinct_letters_and_digits() {
        let mut letters = CODE_ALPHABET.to_vec();
        letters.sort_unstable();
        letters.dedup();
        assert_eq!(letters.len(), 32);
        assert!(CODE_ALPHABET.iter().all(u8::is_ascii_alphanumeric));
    }

    fn inbound(chat: &str, text: &str, group: Option<&str>, file: Option<&str>) -> Inbound {
        Inbound {
            chat: chat.into(),
            private: true,
            text: text.into(),
            reply_to: None,
            files: file
                .map(|id| crate::ports::notify::InboundFile {
                    id: id.into(),
                    name: format!("{id}.jpg"),
                    size: None,
                })
                .into_iter()
                .collect(),
            group: group.map(str::to_owned),
        }
    }

    /// SPEC-512 TGI-003: an album's messages are one task's: its caption and all its files.
    #[test]
    fn an_album_is_one_message() {
        let merged = albums(vec![
            inbound("1", "", Some("g"), Some("a")),
            inbound("1", "Fix these", Some("g"), Some("b")),
            inbound("2", "", Some("g"), Some("c")),
            inbound("1", "next", None, None),
        ]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "Fix these");
        assert_eq!(merged[0].files.len(), 2);
        assert_eq!(merged[1].chat, "2", "another chat's album is its own");
        assert_eq!(merged[2].text, "next");
    }
}
