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
    MyTelegramView, TelegramBotView, TelegramChoices, TelegramLinkCode,
};
use yulius_domain::ids::UserId;
use yulius_domain::notify::{ChatCommand, LINK_CODE_LEN, LINK_CODE_TTL_MS, NotifyChoices};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::Secret;
use crate::ports::notify::{
    ChannelConnector, ChannelError, ChannelIdentity, Inbound, NotificationChannel,
};

/// The one reply of a chat that is not linked (TGC-014).
pub const UNLINKED_REPLY: &str = "This bot sends Yulius notifications to linked accounts only. \
     To link yours, open Yulius, then Settings → Notifications → Link Telegram.";
/// The reply of a linked chat to anything but a command (part 1 sends, it does not take work).
pub const LINKED_HELP: &str = "This chat gets your Yulius notifications. Replies and new tasks \
     are not supported yet. Send /stop to unlink this chat.";
/// The reply to a code that is not valid (any more).
pub const BAD_CODE_REPLY: &str =
    "That link code is not valid any more. Ask for a new one in Yulius: Settings → Notifications.";
/// The audit target of the bot's changes; a link's is its person's id.
pub const AUDIT_TARGET: &str = "telegram";
/// How often an unlinked chat gets its reply at most (TGC-014).
const QUIET_MS: i64 = 10 * 60 * 1000;
/// Chats remembered for that (TGC-014).
const QUIET_CHATS: usize = 1024;
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

/// A message for a chat.
pub(crate) struct Outgoing {
    pub(crate) chat: String,
    pub(crate) text: String,
}

/// The channel services an instance runs with.
pub struct ChannelServices {
    connector: Arc<dyn ChannelConnector>,
    pub(crate) tuning: ChannelTuning,
    active: RwLock<Option<ActiveBot>>,
    poller: Mutex<Option<JoinHandle<()>>>,
    poll: Mutex<PollState>,
    quiet: Mutex<HashMap<String, i64>>,
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
            .map(Secret);
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
        let Some(outbox) = self.channel.get().and_then(|s| s.outbox.get()) else {
            return;
        };
        if outbox.try_send(Outgoing { chat, text }).is_err() {
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
            if let Err(err) = sent {
                tracing::warn!(error = err.as_str(), "a Telegram message could not be sent");
            }
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
                    for message in received.messages {
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
    async fn on_inbound(&self, message: &Inbound) -> AppResult<()> {
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
                } else {
                    LINKED_HELP.to_owned()
                }
            }
        };
        self.enqueue(message.chat.clone(), reply);
        Ok(())
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
        let token = Secret(token.expose().trim().to_owned());
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
            choices: choices_view(link.map_or_else(NotifyChoices::default, |l| l.choices)),
            account_choice: ctx.user.is_superadmin || ctx.user.is_instance_admin,
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
        let admin = ctx.user.is_superadmin || ctx.user.is_instance_admin;
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
}
