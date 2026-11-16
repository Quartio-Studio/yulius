//! Telegram's Bot API as the personal [`NotificationChannel`] (SPEC-498, ADR-0351).
//!
//! `{base}/bot<token>/getMe`, `sendMessage` (plain text, no link previews) and `getUpdates`
//! (long polling, messages only). The base URL is `https://api.telegram.org` unless
//! `YULIUS_TELEGRAM_API_URL` says otherwise, HTTPS except on a loopback host (tests run a fake).
//! The token is part of every request's path, so no error, log line or debug string here ever
//! carries a URL: failures are typed [`ChannelError`]s. No redirects; every request has a
//! deadline and a bounded answer.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use yulius_app::model::Secret;
use yulius_app::ports::notify::{
    ChannelConnector, ChannelError, ChannelIdentity, Inbound, NotificationChannel, Received,
};

/// Telegram's Bot API.
pub const TELEGRAM_API: &str = "https://api.telegram.org";
/// How long `getUpdates` waits for a message (seconds, Telegram's long poll).
pub const POLL_SECONDS: u64 = 25;
/// Bound of a call other than the long poll.
const CALL_DEADLINE: Duration = Duration::from_secs(10);
/// Largest answer read.
const MAX_ANSWER_BYTES: usize = 1 << 20;
/// Updates asked for per `getUpdates` (Telegram's default is 100): a message is at most 4096
/// characters with at most 100 entities, so 20 fit [`MAX_ANSWER_BYTES`] with room to spare; an
/// answer over it anyway is asked again one update at a time (review round 1).
const POLL_LIMIT: u64 = 20;
/// Longest token accepted (Telegram's are about 46 characters).
const MAX_TOKEN_CHARS: usize = 200;

/// Checks a Bot API base URL (`YULIUS_TELEGRAM_API_URL`): HTTPS, or HTTP on a loopback host.
///
/// # Errors
/// A message naming what is wrong.
pub fn api_base(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("not a URL: {e}"))?;
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    match parsed.scheme() {
        "https" => Ok(url.to_owned()),
        "http" if loopback => Ok(url.to_owned()),
        _ => Err("the Telegram API URL must use HTTPS (HTTP only on a loopback host)".into()),
    }
}

/// Whether `token` has a bot token's shape: digits, a colon, then letters, digits, `_` or `-`.
fn token_shape(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else {
        return false;
    };
    token.len() <= MAX_TOKEN_CHARS
        && !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && !secret.is_empty()
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Builds [`TelegramBot`]s from tokens (composition).
#[derive(Debug, Clone)]
pub struct TelegramBots {
    base: String,
    poll_seconds: u64,
}

impl TelegramBots {
    /// Bots of the API at `base` (checked with [`api_base`]), long-polling `poll_seconds`.
    #[must_use]
    pub fn new(base: String, poll_seconds: u64) -> Self {
        Self { base, poll_seconds }
    }
}

impl ChannelConnector for TelegramBots {
    fn connect(&self, token: &Secret) -> Result<Arc<dyn NotificationChannel>, String> {
        if !token_shape(token.expose()) {
            return Err(
                "this is not a Telegram bot token (it looks like 123456789:ABC…, from @BotFather)"
                    .into(),
            );
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "the Telegram client could not be set up".to_owned())?;
        Ok(Arc::new(TelegramBot {
            http,
            base: self.base.clone(),
            token: token.expose().to_owned(),
            poll_seconds: self.poll_seconds,
        }))
    }
}

/// One bot, by its token.
pub struct TelegramBot {
    http: reqwest::Client,
    base: String,
    token: String,
    poll_seconds: u64,
}

impl std::fmt::Debug for TelegramBot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBot")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// Why a call gave no result.
enum CallFailure {
    /// The answer is longer than [`MAX_ANSWER_BYTES`].
    TooLarge,
    /// Any other failure.
    Channel(ChannelError),
}

impl TelegramBot {
    /// Calls `method` with `body`; the answer's `result`.
    async fn call(
        &self,
        method: &str,
        body: &Value,
        deadline: Duration,
    ) -> Result<Value, ChannelError> {
        self.call_bounded(method, body, deadline)
            .await
            .map_err(|failure| match failure {
                CallFailure::TooLarge => ChannelError::Invalid,
                CallFailure::Channel(err) => err,
            })
    }

    /// [`Self::call`], telling an answer over the bound apart.
    async fn call_bounded(
        &self,
        method: &str,
        body: &Value,
        deadline: Duration,
    ) -> Result<Value, CallFailure> {
        use CallFailure::Channel;
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let request = self.http.post(url).json(body).timeout(deadline).send();
        let mut response = request
            .await
            .map_err(|_| Channel(ChannelError::Unavailable))?;
        match response.status().as_u16() {
            200..=299 => {}
            // A wrong or revoked token: 401; a malformed one: 404.
            401 | 404 => return Err(Channel(ChannelError::Refused)),
            // The chat blocked the bot, or does not exist (any more).
            400 | 403 => return Err(Channel(ChannelError::Blocked)),
            409 => return Err(Channel(ChannelError::Conflict)),
            _ => return Err(Channel(ChannelError::Unavailable)),
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Channel(ChannelError::Unavailable))?
        {
            if bytes.len() + chunk.len() > MAX_ANSWER_BYTES {
                return Err(CallFailure::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let answer: Value =
            serde_json::from_slice(&bytes).map_err(|_| Channel(ChannelError::Invalid))?;
        if answer.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(Channel(ChannelError::Invalid));
        }
        answer
            .get("result")
            .cloned()
            .ok_or(Channel(ChannelError::Invalid))
    }

    /// One `getUpdates` from `offset`, at most `limit` updates, waiting `wait` seconds for one.
    async fn updates(
        &self,
        offset: Option<i64>,
        limit: u64,
        wait: u64,
    ) -> Result<Value, CallFailure> {
        let mut body = json!({
            "timeout": wait,
            "limit": limit,
            "allowed_updates": ["message"],
        });
        if let Some(offset) = offset {
            body["offset"] = json!(offset);
        }
        let deadline = Duration::from_secs(wait) + CALL_DEADLINE;
        self.call_bounded("getUpdates", &body, deadline).await
    }
}

/// The messages of a `getUpdates` result and the next offset.
fn parse_updates(result: &Value) -> Result<Received, ChannelError> {
    let updates = result.as_array().ok_or(ChannelError::Invalid)?;
    let mut next: Option<i64> = None;
    let mut messages = Vec::new();
    for update in updates {
        let id = update
            .get("update_id")
            .and_then(Value::as_i64)
            .ok_or(ChannelError::Invalid)?;
        next = Some(next.map_or(id + 1, |n| n.max(id + 1)));
        let Some(message) = update.get("message") else {
            continue;
        };
        let Some(chat) = message.get("chat") else {
            continue;
        };
        let Some(chat_id) = chat.get("id").and_then(Value::as_i64) else {
            continue;
        };
        messages.push(Inbound {
            chat: chat_id.to_string(),
            private: chat.get("type").and_then(Value::as_str) == Some("private"),
            text: message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    Ok(Received {
        cursor: next.map(|n| n.to_string()),
        messages,
    })
}

#[async_trait]
impl NotificationChannel for TelegramBot {
    async fn check(&self) -> Result<ChannelIdentity, ChannelError> {
        let me = self.call("getMe", &json!({}), CALL_DEADLINE).await?;
        let id = me
            .get("id")
            .and_then(Value::as_i64)
            .ok_or(ChannelError::Invalid)?;
        let username = me
            .get("username")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty() && u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .ok_or(ChannelError::Invalid)?;
        Ok(ChannelIdentity {
            id,
            username: username.to_owned(),
        })
    }

    async fn send(&self, chat: &str, text: &str) -> Result<(), ChannelError> {
        let chat: i64 = chat.parse().map_err(|_| ChannelError::Blocked)?;
        let body = json!({
            "chat_id": chat,
            "text": text,
            "link_preview_options": {"is_disabled": true},
        });
        self.call("sendMessage", &body, CALL_DEADLINE)
            .await
            .map(|_| ())
    }

    async fn receive(&self, cursor: Option<&str>) -> Result<Received, ChannelError> {
        let offset = cursor.and_then(|c| c.parse::<i64>().ok());
        let failed = |failure| match failure {
            CallFailure::TooLarge => ChannelError::Invalid,
            CallFailure::Channel(err) => err,
        };
        match self.updates(offset, POLL_LIMIT, self.poll_seconds).await {
            Err(CallFailure::TooLarge) => {}
            answer => return parse_updates(&answer.map_err(failed)?),
        }
        // Over the bound: the same updates one at a time (they wait already, so no long poll).
        match self.updates(offset, 1, 0).await {
            Err(CallFailure::TooLarge) => {}
            answer => return parse_updates(&answer.map_err(failed)?),
        }
        // One update alone is over it: stepped past (update ids follow each other), or the poll
        // would ask for it forever.
        let Some(offset) = offset else {
            return Err(ChannelError::Invalid);
        };
        tracing::warn!(
            update = offset,
            "a Telegram update over the answer bound was skipped"
        );
        Ok(Received {
            cursor: Some((offset + 1).to_string()),
            messages: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::Json;
    use axum::Router;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::routing::post;

    use super::*;

    // A synthetic token: digits, a colon, a body (never a real bot's).
    const TOKEN: &str = "1000:synthetic-bot-token_0001";
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[derive(Default)]
    struct Fake {
        bodies: Mutex<Vec<(String, Value)>>,
    }

    async fn answer(
        State(fake): State<Arc<Fake>>,
        Path((bot, method)): Path<(String, String)>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        if bot != format!("bot{TOKEN}") {
            return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false})));
        }
        let blocked = body["chat_id"] == json!(403);
        let (offset, limit) = (body["offset"].clone(), body["limit"].clone());
        if let Ok(mut bodies) = fake.bodies.lock() {
            bodies.push((method.clone(), body));
        }
        let result = match method.as_str() {
            "getMe" => json!({"id": 1000, "is_bot": true, "username": "yulius_test_bot"}),
            // From offset 100 the backlog is over the bound unless asked one at a time; at 200
            // one update alone is.
            "getUpdates" if offset == json!(100) || offset == json!(200) => {
                let alone = limit == json!(1) && offset == json!(100);
                if alone {
                    json!([{"update_id": 100, "message": {"chat": {"id": 42, "type": "private"}, "text": "/stop"}}])
                } else {
                    json!([{"update_id": 100, "message": {"text": "x".repeat(MAX_ANSWER_BYTES)}}])
                }
            }
            "getUpdates" => json!([
                {"update_id": 7, "message": {"chat": {"id": 42, "type": "private"}, "text": "/start abc"}},
                {"update_id": 8, "message": {"chat": {"id": -5, "type": "group"}, "text": "hi"}},
                {"update_id": 9, "edited_message": {}},
            ]),
            "sendMessage" if blocked => {
                return (StatusCode::FORBIDDEN, Json(json!({"ok": false})));
            }
            "sendMessage" => json!({"message_id": 1}),
            _ => return (StatusCode::NOT_FOUND, Json(json!({"ok": false}))),
        };
        (StatusCode::OK, Json(json!({"ok": true, "result": result})))
    }

    async fn fake() -> Result<(String, Arc<Fake>), Box<dyn std::error::Error>> {
        let fake = Arc::new(Fake::default());
        let app = Router::new()
            .route("/{bot}/{method}", post(answer))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move { axum::serve(listener, app).await });
        Ok((format!("http://{addr}"), fake))
    }

    #[test]
    fn the_api_url_is_https_except_on_loopback() {
        assert_eq!(api_base(TELEGRAM_API).as_deref(), Ok(TELEGRAM_API));
        assert_eq!(
            api_base("http://127.0.0.1:9/").as_deref(),
            Ok("http://127.0.0.1:9")
        );
        assert!(api_base("http://localhost:9").is_ok());
        assert!(api_base("http://telegram.example").is_err());
        assert!(api_base("ftp://127.0.0.1").is_err());
        assert!(api_base("not a url").is_err());
    }

    #[test]
    fn only_a_token_shape_connects() {
        let bots = TelegramBots::new(TELEGRAM_API.into(), 1);
        assert!(bots.connect(&Secret(TOKEN.into())).is_ok());
        for bad in ["", "abc", ":abc", "12:", "12:a b", "x1:abc"] {
            let err = bots.connect(&Secret(bad.into())).err().unwrap_or_default();
            assert!(err.contains("not a Telegram bot token"), "{bad}: {err}");
        }
    }

    #[tokio::test]
    async fn the_bot_checks_sends_and_receives() -> TestResult {
        let (base, fake) = fake().await?;
        let bot = TelegramBots::new(base, 1).connect(&Secret(TOKEN.into()))?;
        let me = bot.check().await.map_err(|e| e.as_str())?;
        assert_eq!(me.id, 1000);
        assert_eq!(me.username, "yulius_test_bot");
        bot.send("42", "hello").await.map_err(|e| e.as_str())?;
        assert_eq!(bot.send("403", "x").await, Err(ChannelError::Blocked));
        let received = bot.receive(Some("7")).await.map_err(|e| e.as_str())?;
        assert_eq!(received.cursor.as_deref(), Some("10"));
        assert_eq!(
            received.messages,
            vec![
                Inbound {
                    chat: "42".into(),
                    private: true,
                    text: "/start abc".into()
                },
                Inbound {
                    chat: "-5".into(),
                    private: false,
                    text: "hi".into()
                },
            ]
        );
        let bodies = fake.bodies.lock().map(|b| b.clone()).unwrap_or_default();
        let send = bodies
            .iter()
            .find(|(m, _)| m == "sendMessage")
            .ok_or("send")?;
        assert_eq!(send.1["chat_id"], json!(42));
        assert_eq!(send.1["text"], json!("hello"));
        assert!(send.1.get("parse_mode").is_none());
        let poll = bodies
            .iter()
            .find(|(m, _)| m == "getUpdates")
            .ok_or("poll")?;
        assert_eq!(poll.1["offset"], json!(7));
        assert_eq!(poll.1["limit"], json!(POLL_LIMIT));
        assert_eq!(poll.1["allowed_updates"], json!(["message"]));
        Ok(())
    }

    /// Review round 1: a batch over the answer bound is asked again one update at a time, and an
    /// update over it alone is stepped past, so the poll never asks for the same batch forever.
    #[tokio::test]
    async fn a_batch_over_the_bound_is_read_one_at_a_time_or_skipped() -> TestResult {
        let (base, fake) = fake().await?;
        let bot = TelegramBots::new(base, 1).connect(&Secret(TOKEN.into()))?;
        let received = bot.receive(Some("100")).await.map_err(|e| e.as_str())?;
        assert_eq!(received.cursor.as_deref(), Some("101"));
        assert_eq!(received.messages.len(), 1);
        assert_eq!(received.messages[0].text, "/stop");
        let skipped = bot.receive(Some("200")).await.map_err(|e| e.as_str())?;
        assert_eq!(skipped.cursor.as_deref(), Some("201"));
        assert!(skipped.messages.is_empty());
        let polls = fake.bodies.lock().map(|b| b.len()).unwrap_or(0);
        assert_eq!(polls, 4, "two tries each");
        Ok(())
    }

    #[tokio::test]
    async fn a_refused_token_and_an_unreachable_api_are_typed_without_the_token() -> TestResult {
        let (base, _) = fake().await?;
        let other = TelegramBots::new(base, 1).connect(&Secret("2000:another-token".into()))?;
        assert_eq!(other.check().await, Err(ChannelError::Refused));
        let gone =
            TelegramBots::new("http://127.0.0.1:9".into(), 1).connect(&Secret(TOKEN.into()))?;
        let err = gone.check().await.err().ok_or("an error")?;
        assert_eq!(err, ChannelError::Unavailable);
        assert!(!format!("{err:?} {}", err.as_str()).contains("synthetic"));
        let client = TelegramBot {
            http: reqwest::Client::new(),
            base: "http://127.0.0.1:9".into(),
            token: TOKEN.into(),
            poll_seconds: 1,
        };
        assert!(!format!("{client:?}").contains("synthetic"));
        Ok(())
    }

    #[test]
    fn malformed_updates_are_invalid() {
        assert_eq!(
            parse_updates(&json!({"not": "a list"})),
            Err(ChannelError::Invalid)
        );
        assert_eq!(
            parse_updates(&json!([{"message": {}}])),
            Err(ChannelError::Invalid)
        );
        assert_eq!(parse_updates(&json!([])), Ok(Received::default()));
    }
}
