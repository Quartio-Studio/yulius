//! The personal Telegram channel (SPEC-444): the instance's bot and each person's linked chat.
//! No response ever carries the bot token.

use serde::{Deserialize, Serialize};

/// `GET /api/admin/telegram` (instance administrators): the bot's state, never its token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TelegramBotView {
    /// Whether a bot token is saved.
    pub configured: bool,
    /// The bot's username (`t.me/<username>`), from `getMe` when it was set.
    pub bot_username: Option<String>,
    /// When the token was set (RFC 3339).
    pub set_at: Option<String>,
    /// Who set it (login).
    pub set_by: Option<String>,
    /// `off` (no bot), `polling` (receiving) or `retrying` (after an error, with backoff).
    pub state: String,
    /// The last error's kind while `retrying`: `refused`, `unreachable`, `conflict`, `invalid`.
    pub last_error: Option<String>,
    /// How many people linked a chat.
    pub linked: u64,
}

/// `PUT /api/admin/telegram`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetTelegramBot {
    /// The bot token from @BotFather.
    pub token: String,
}

/// What a person receives in their chat (TGC-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct TelegramChoices {
    /// Tasks that need them (on by default).
    pub needs_you: bool,
    /// Tasks that are Done.
    pub done: bool,
    /// Tasks that failed or stopped abnormally.
    pub failed: bool,
    /// Subscription problems (instance administrators only).
    pub account: bool,
}

/// `GET /api/me/telegram`: the person's own link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyTelegramView {
    /// Whether the instance has a bot (otherwise nothing can be linked).
    pub available: bool,
    /// The bot's username.
    pub bot_username: Option<String>,
    /// Whether a chat is linked.
    pub linked: bool,
    /// When it was linked (RFC 3339).
    pub linked_at: Option<String>,
    /// What they receive (the defaults while not linked).
    pub choices: TelegramChoices,
    /// Whether subscription problems can be chosen (instance administrators).
    pub account_choice: bool,
}

/// `POST /api/me/telegram/link` (browser sessions only): a single-use code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramLinkCode {
    /// The code, to send as `/start <code>`.
    pub code: String,
    /// `https://t.me/<bot>?start=<code>`.
    pub link: String,
    /// When it stops working (RFC 3339), 10 minutes from now.
    pub expires_at: String,
}
