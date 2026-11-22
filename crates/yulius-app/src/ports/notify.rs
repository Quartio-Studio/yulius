//! A person's own notification channel (SPEC-498 TGC-030, ADR-0351): the one thing a chat
//! service implements. Telegram is the first; the event side (which changes are worth a message,
//! for whom, redaction and the rate bound) never names it.

use std::sync::Arc;

use async_trait::async_trait;
use yulius_domain::ids::UserId;
use yulius_domain::notify::NotifyChoices;
use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::model::{Sealed, Secret};

/// Who the channel's bot is (Telegram: `getMe`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelIdentity {
    /// The bot's id: another id is another bot, whose chats are not ours.
    pub id: i64,
    /// Its username, for links (`t.me/<username>`).
    pub username: String,
}

/// A message a chat sent to the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    /// The chat's address (Telegram: the chat id).
    pub chat: String,
    /// A one-to-one chat with a person (never a group or channel).
    pub private: bool,
    /// Its text, or a file's caption (empty for a message without either).
    pub text: String,
    /// The bot's message it replies to (Telegram: its `message_id`), if any (SPEC-512 TGI-002).
    pub reply_to: Option<String>,
    /// The files it carries (TGI-003).
    pub files: Vec<InboundFile>,
    /// The album it belongs to (Telegram: `media_group_id`): an album's messages are one.
    pub group: Option<String>,
}

/// A file a chat sent (SPEC-512 TGI-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundFile {
    /// The service's id to fetch it by.
    pub id: String,
    /// Its name (the sender's, or one made from its kind).
    pub name: String,
    /// Its size in bytes, when the service says.
    pub size: Option<u64>,
}

/// What one receive returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Received {
    /// Where the next receive continues (Telegram: the next update offset); `None` keeps it.
    pub cursor: Option<String>,
    /// The messages, oldest first.
    pub messages: Vec<Inbound>,
}

/// Why a channel call failed. Never carries the token or a URL holding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// The service refused the token (revoked or wrong).
    Refused,
    /// The chat cannot be written to (it blocked the bot, or is gone).
    Blocked,
    /// The service is not reachable or answered with an error.
    Unavailable,
    /// Another client polls with the same token.
    Conflict,
    /// The answer was not in the expected form.
    Invalid,
    /// A file is over the bound asked for (SPEC-512 TGI-003).
    TooLarge,
}

impl ChannelError {
    /// Its wire name (the admin state's `last_error`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::Blocked => "blocked",
            Self::Unavailable => "unreachable",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::TooLarge => "too_large",
        }
    }
}

/// A chat service's bot, connected with its token.
#[async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Who the bot is; a refused token is [`ChannelError::Refused`].
    async fn check(&self) -> Result<ChannelIdentity, ChannelError>;
    /// Sends plain `text` to `chat`; the sent message's id, when the service gives one (a reply
    /// to it names it, SPEC-512 TGI-002).
    async fn send(&self, chat: &str, text: &str) -> Result<Option<String>, ChannelError>;
    /// Deletes the bot's own `message` in `chat` (a keyed notice replaced it, SPEC-624 NTC-007).
    /// The service may refuse an old one (Telegram: after 48 hours).
    async fn delete(&self, chat: &str, message: &str) -> Result<(), ChannelError>;
    /// Fetches a received file, at most `max` bytes ([`ChannelError::TooLarge`] beyond).
    async fn fetch_file(&self, id: &str, max: u64) -> Result<Vec<u8>, ChannelError>;
    /// Waits for messages after `cursor`, at most the channel's own long-poll bound.
    async fn receive(&self, cursor: Option<&str>) -> Result<Received, ChannelError>;
}

/// Builds a channel from a token (composition: the HTTP client and its base URL).
pub trait ChannelConnector: Send + Sync {
    /// The channel for `token`; `Err` for a token that cannot be one (a message without it).
    ///
    /// # Errors
    /// A malformed token.
    fn connect(&self, token: &Secret) -> Result<Arc<dyn NotificationChannel>, String>;
}

/// The saved bot, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBot {
    /// The token, sealed with the master key.
    pub sealed: Sealed,
    /// The bot's id and username when it was set.
    pub identity: ChannelIdentity,
    /// When it was set.
    pub set_at: Timestamp,
    /// Who set it (login).
    pub set_by: Option<String>,
}

/// A person's linked chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLink {
    /// The person.
    pub user: UserId,
    /// The chat's address.
    pub chat: String,
    /// When it was linked.
    pub linked_at: Timestamp,
    /// What they want to receive.
    pub choices: NotifyChoices,
    /// Whether the chat may answer and start tasks (SPEC-512 TGI-001; off by default).
    pub intake: bool,
    /// The project a new message starts its task in.
    pub intake_sandbox: Option<String>,
}

/// What one of the bot's messages was about (SPEC-512 TGI-002): its task and, for a notice of
/// a request, that request's time, so a reply answers that request and no later one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentAbout {
    /// The task.
    pub task: yulius_domain::ids::TaskId,
    /// The request the message told of (`needs_you` `at`, ms).
    pub asked_at: Option<i64>,
}

/// The bot, link codes and linked chats (SPEC-498; migration 0063).
#[async_trait]
pub trait ChatLinkStore: Send + Sync {
    /// The saved bot.
    async fn chat_bot(&self) -> AppResult<Option<StoredBot>>;
    /// Saves (`Some`) or removes (`None`) the bot.
    async fn set_chat_bot(
        &self,
        bot: Option<(&Sealed, &ChannelIdentity, &str)>,
        at: Timestamp,
    ) -> AppResult<()>;
    /// Keeps `code_hash` as `user`'s one link code until `expires_at`, replacing an earlier one.
    async fn put_link_code(
        &self,
        user: &UserId,
        code_hash: &str,
        expires_at: Timestamp,
    ) -> AppResult<()>;
    /// Consumes the code with `code_hash` when it has not expired at `now`: its person. Expired
    /// codes are deleted on the way.
    async fn take_link_code(&self, code_hash: &str, now: Timestamp) -> AppResult<Option<UserId>>;
    /// Links `chat` to `user` with the default choices, replacing the person's earlier chat and
    /// the chat's earlier person; returns that earlier person when it was someone else.
    async fn link_chat(
        &self,
        user: &UserId,
        chat: &str,
        at: Timestamp,
    ) -> AppResult<Option<UserId>>;
    /// Unlinks `user`; the chat it had.
    async fn unlink_user(&self, user: &UserId) -> AppResult<Option<String>>;
    /// The link of `user`.
    async fn chat_link(&self, user: &UserId) -> AppResult<Option<ChatLink>>;
    /// The link of `chat`.
    async fn chat_link_of(&self, chat: &str) -> AppResult<Option<ChatLink>>;
    /// Every link.
    async fn chat_links(&self) -> AppResult<Vec<ChatLink>>;
    /// Sets `user`'s choices; `false` when they have no link.
    async fn set_chat_choices(&self, user: &UserId, choices: &NotifyChoices) -> AppResult<bool>;
    /// Sets the person's intake and its project (SPEC-512 TGI-001); `false` without a link.
    async fn set_chat_intake(
        &self,
        user: &UserId,
        enabled: bool,
        sandbox: Option<&str>,
    ) -> AppResult<bool>;
    /// Remembers that the bot's `message` in `chat` was about `task` (TGI-002); the chat's last
    /// 500 are kept.
    async fn record_sent(
        &self,
        chat: &str,
        message: &str,
        about: &SentAbout,
        at: Timestamp,
    ) -> AppResult<()>;
    /// What the bot's `message` in `chat` was about.
    async fn sent_about(&self, chat: &str, message: &str) -> AppResult<Option<SentAbout>>;
    /// Removes every link and code; how many links went.
    async fn clear_chat_links(&self) -> AppResult<u64>;
}
