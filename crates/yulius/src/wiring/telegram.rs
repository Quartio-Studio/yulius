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

use crate::wiring::http::{BodyError, read_bounded};
use async_trait::async_trait;
use serde_json::{Value, json};
use yulius_app::model::Secret;
use yulius_app::ports::notify::{
    ChannelConnector, ChannelError, ChannelIdentity, Inbound, InboundFile, NotificationChannel,
    Received,
};

/// Telegram's Bot API.
pub const TELEGRAM_API: &str = "https://api.telegram.org";
/// The bound of one file download (SPEC-512 TGI-003: at most 20 MB).
const FILE_DEADLINE: Duration = Duration::from_secs(120);
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
/// How much of an answer over the bound is kept, to read the first update's id.
const HEAD_BYTES: usize = 4096;
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
        let http = super::http::client_builder()
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
    /// The answer is longer than [`MAX_ANSWER_BYTES`]; its first bytes.
    TooLarge(Vec<u8>),
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
                CallFailure::TooLarge(_) => ChannelError::Invalid,
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
        let bytes = match read_bounded(&mut response, MAX_ANSWER_BYTES).await {
            Ok(bytes) => bytes,
            Err(BodyError::Read(_)) => return Err(Channel(ChannelError::Unavailable)),
            Err(BodyError::TooLarge(mut head)) => {
                head.truncate(HEAD_BYTES);
                return Err(CallFailure::TooLarge(head));
            }
        };
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

/// The first `update_id` in the head of an answer cut at the bound.
fn first_update_id(head: &[u8]) -> Option<i64> {
    const KEY: &[u8] = b"\"update_id\"";
    let at = head.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    let rest = std::str::from_utf8(&head[at..]).ok().or_else(|| {
        // A cut inside a multi-byte character ends the head: read what is whole.
        let valid = std::str::from_utf8(&head[at..]).err()?.valid_up_to();
        std::str::from_utf8(&head[at..at + valid]).ok()
    })?;
    let digits = rest.trim_start().strip_prefix(':')?.trim_start();
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse().ok()
}

/// The files of a message (SPEC-512 TGI-003): a photo's largest size, a document, a voice note,
/// an audio or a video file, each named by the sender or by its kind.
fn message_files(message: &Value) -> Vec<InboundFile> {
    let file = |value: &Value, kind: &str, ext: &str| -> Option<InboundFile> {
        let id = value.get("file_id").and_then(Value::as_str)?.to_owned();
        let unique = value
            .get("file_unique_id")
            .and_then(Value::as_str)
            .unwrap_or("file");
        let name = value
            .get("file_name")
            .and_then(Value::as_str)
            .filter(|n| !n.trim().is_empty())
            .map_or_else(|| format!("{kind}-{unique}.{ext}"), str::to_owned);
        Some(InboundFile {
            id,
            name,
            size: value.get("file_size").and_then(Value::as_u64),
        })
    };
    let mut files = Vec::new();
    if let Some(largest) = message
        .get("photo")
        .and_then(Value::as_array)
        .and_then(|sizes| sizes.last())
    {
        files.extend(file(largest, "photo", "jpg"));
    }
    for (key, ext) in [
        ("document", "bin"),
        ("voice", "ogg"),
        ("audio", "mp3"),
        ("video", "mp4"),
    ] {
        if let Some(value) = message.get(key) {
            files.extend(file(value, key, ext));
        }
    }
    files
}

/// Whether Telegram's `file_path` is a plain relative path under the bot's file root: no
/// parent step, no escape, no absolute path (SPEC-512 TGI-003).
fn file_path_ok(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains("..")
        && !path.contains('%')
        && !path.contains('\\')
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.'))
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
        let after = id.checked_add(1).ok_or(ChannelError::Invalid)?;
        next = Some(next.map_or(after, |n| n.max(after)));
        let Some(message) = update.get("message") else {
            continue;
        };
        let Some(chat) = message.get("chat") else {
            continue;
        };
        let Some(chat_id) = chat.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let text = message
            .get("text")
            .or_else(|| message.get("caption"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        messages.push(Inbound {
            chat: chat_id.to_string(),
            private: chat.get("type").and_then(Value::as_str) == Some("private"),
            text,
            reply_to: message
                .get("reply_to_message")
                .and_then(|r| r.get("message_id"))
                .and_then(Value::as_i64)
                .map(|id| id.to_string()),
            files: message_files(message),
            group: message
                .get("media_group_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
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

    async fn send(&self, chat: &str, text: &str) -> Result<Option<String>, ChannelError> {
        let chat: i64 = chat.parse().map_err(|_| ChannelError::Blocked)?;
        let body = json!({
            "chat_id": chat,
            "text": text,
            "link_preview_options": {"is_disabled": true},
        });
        let sent = self.call("sendMessage", &body, CALL_DEADLINE).await?;
        Ok(sent
            .get("message_id")
            .and_then(Value::as_i64)
            .map(|id| id.to_string()))
    }

    async fn delete(&self, chat: &str, message: &str) -> Result<(), ChannelError> {
        let chat: i64 = chat.parse().map_err(|_| ChannelError::Blocked)?;
        let message: i64 = message.parse().map_err(|_| ChannelError::Invalid)?;
        let body = json!({"chat_id": chat, "message_id": message});
        self.call("deleteMessage", &body, CALL_DEADLINE).await?;
        Ok(())
    }

    async fn fetch_file(&self, id: &str, max: u64) -> Result<Vec<u8>, ChannelError> {
        let file = self
            .call("getFile", &json!({ "file_id": id }), CALL_DEADLINE)
            .await?;
        if file
            .get("file_size")
            .and_then(Value::as_u64)
            .is_some_and(|size| size > max)
        {
            return Err(ChannelError::TooLarge);
        }
        let path = file
            .get("file_path")
            .and_then(Value::as_str)
            .filter(|p| file_path_ok(p))
            .ok_or(ChannelError::Invalid)?;
        let url = format!("{}/file/bot{}/{path}", self.base, self.token);
        let mut response = self
            .http
            .get(url)
            .timeout(FILE_DEADLINE)
            .send()
            .await
            .map_err(|_| ChannelError::Unavailable)?;
        if !response.status().is_success() {
            return Err(ChannelError::Unavailable);
        }
        read_bounded(&mut response, usize::try_from(max).unwrap_or(usize::MAX))
            .await
            .map_err(|e| match e {
                BodyError::Read(_) => ChannelError::Unavailable,
                BodyError::TooLarge(_) => ChannelError::TooLarge,
            })
    }

    async fn receive(&self, cursor: Option<&str>) -> Result<Received, ChannelError> {
        let offset = cursor.and_then(|c| c.parse::<i64>().ok());
        let failed = |failure| match failure {
            CallFailure::TooLarge(_) => ChannelError::Invalid,
            CallFailure::Channel(err) => err,
        };
        match self.updates(offset, POLL_LIMIT, self.poll_seconds).await {
            Err(CallFailure::TooLarge(_)) => {}
            answer => return parse_updates(&answer.map_err(failed)?),
        }
        // Over the bound: the same updates one at a time (they wait already, so no long poll).
        let head = match self.updates(offset, 1, 0).await {
            Err(CallFailure::TooLarge(head)) => head,
            answer => return parse_updates(&answer.map_err(failed)?),
        };
        // One update alone is over it: stepped past by the id its answer starts with (ids may
        // jump, and a first poll has no offset), or the poll would ask for it forever. Without a
        // readable id it is an error, so the poll backs off (review round 2).
        let id = first_update_id(&head).ok_or(ChannelError::Invalid)?;
        tracing::warn!(
            update = id,
            "a Telegram update over the answer bound was skipped"
        );
        let after = id.checked_add(1).ok_or(ChannelError::Invalid)?;
        Ok(Received {
            cursor: Some(after.to_string()),
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

    /// An answer of one update over the bound, its fields after `{` starting with `lead`.
    fn oversized(lead: &str) -> String {
        let text = "x".repeat(MAX_ANSWER_BYTES);
        format!(r#"{{"ok":true,"result":[{{{lead}"message":{{"text":"{text}"}}}}]}}"#)
    }

    async fn answer(
        State(fake): State<Arc<Fake>>,
        Path((bot, method)): Path<(String, String)>,
        Json(body): Json<Value>,
    ) -> (StatusCode, String) {
        if bot != format!("bot{TOKEN}") {
            return (StatusCode::UNAUTHORIZED, json!({"ok": false}).to_string());
        }
        let blocked = body["chat_id"] == json!(403);
        let (offset, limit) = (body["offset"].clone(), body["limit"].clone());
        let file_id = body["file_id"].clone();
        if let Ok(mut bodies) = fake.bodies.lock() {
            bodies.push((method.clone(), body));
        }
        let result = match method.as_str() {
            "getMe" => json!({"id": 1000, "is_bot": true, "username": "yulius_test_bot"}),
            // From offset 100 the backlog is over the bound unless asked one at a time; at 200
            // one update alone is, and its id is 205 (ids may jump); at 300 no id can be read.
            "getUpdates" if offset == json!(100) && limit == json!(1) => {
                json!([{"update_id": 100, "message": {"chat": {"id": 42, "type": "private"}, "text": "/stop"}}])
            }
            // Written as Telegram orders it (`update_id` first); `json!` would sort the keys.
            "getUpdates" if offset == json!(100) || offset == json!(200) => {
                let id = if offset == json!(100) { 100 } else { 205 };
                return (StatusCode::OK, oversized(&format!("\"update_id\":{id},")));
            }
            "getUpdates" if offset == json!(300) => return (StatusCode::OK, oversized("")),
            "getUpdates" => json!([
                {"update_id": 7, "message": {"chat": {"id": 42, "type": "private"}, "text": "/start abc"}},
                {"update_id": 8, "message": {"chat": {"id": -5, "type": "group"}, "text": "hi"}},
                {"update_id": 9, "edited_message": {}},
            ]),
            "sendMessage" if blocked => {
                return (StatusCode::FORBIDDEN, json!({"ok": false}).to_string());
            }
            "sendMessage" => json!({"message_id": 1}),
            "getFile" if file_id == json!("big") => {
                json!({"file_id": "big", "file_size": 30_000_000, "file_path": "documents/big.bin"})
            }
            "getFile" => json!({"file_id": file_id, "file_path": "photos/file_7.jpg"}),
            _ => return (StatusCode::NOT_FOUND, json!({"ok": false}).to_string()),
        };
        (
            StatusCode::OK,
            json!({"ok": true, "result": result}).to_string(),
        )
    }

    async fn fake() -> Result<(String, Arc<Fake>), Box<dyn std::error::Error>> {
        let fake = Arc::new(Fake::default());
        let app = Router::new()
            .route("/{bot}/{method}", post(answer))
            .route(
                "/file/{bot}/{*path}",
                axum::routing::get(|Path((bot, path)): Path<(String, String)>| async move {
                    if bot != format!("bot{TOKEN}") || path != "photos/file_7.jpg" {
                        return (StatusCode::NOT_FOUND, Vec::new());
                    }
                    (StatusCode::OK, vec![7_u8; 64])
                }),
            )
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
        let sent = bot.send("42", "hello").await.map_err(|e| e.as_str())?;
        assert_eq!(sent.as_deref(), Some("1"), "the sent message's id");
        assert_eq!(bot.send("403", "x").await, Err(ChannelError::Blocked));
        let received = bot.receive(Some("7")).await.map_err(|e| e.as_str())?;
        assert_eq!(received.cursor.as_deref(), Some("10"));
        assert_eq!(
            received.messages,
            vec![
                Inbound {
                    chat: "42".into(),
                    private: true,
                    text: "/start abc".into(),
                    reply_to: None,
                    files: Vec::new(),
                    group: None,
                },
                Inbound {
                    chat: "-5".into(),
                    private: false,
                    text: "hi".into(),
                    reply_to: None,
                    files: Vec::new(),
                    group: None,
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
        // Stepped past by the id the answer starts with (review round 2), not the offset.
        let skipped = bot.receive(Some("200")).await.map_err(|e| e.as_str())?;
        assert_eq!(skipped.cursor.as_deref(), Some("206"));
        assert!(skipped.messages.is_empty());
        // No id to read: an error, so the poll backs off rather than asking at once again.
        assert_eq!(bot.receive(Some("300")).await, Err(ChannelError::Invalid));
        let polls = fake.bodies.lock().map(|b| b.len()).unwrap_or(0);
        assert_eq!(polls, 6, "two tries each");
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
    fn the_last_update_id_is_invalid() {
        let last = json!([{"update_id": i64::MAX, "message": {"chat": {"id": 1}, "text": "x"}}]);
        assert!(matches!(parse_updates(&last), Err(ChannelError::Invalid)));
    }

    #[test]
    fn the_first_update_id_is_read_from_a_cut_head() {
        assert_eq!(
            first_update_id(br#"{"ok":true,"result":[{"update_id": 42,"message":{"text":"x"#),
            Some(42)
        );
        assert_eq!(first_update_id(br#"{"ok":true,"result":[{"message""#), None);
        assert_eq!(first_update_id(br#"{"update_id":"#), None);
        let mut cut = br#"{"update_id":7,"message":{"text":""#.to_vec();
        cut.extend_from_slice(&"\u{6f22}".as_bytes()[..2]);
        assert_eq!(
            first_update_id(&cut),
            Some(7),
            "a cut character ends the head"
        );
    }

    /// TGI-002, TGI-003: a reply names the bot's message; a caption is the text; a photo's
    /// largest size, a document and a voice note are files; an album is named.
    #[test]
    fn replies_captions_files_and_albums_are_read() {
        let received = parse_updates(&json!([{"update_id": 1, "message": {
            "chat": {"id": 42, "type": "private"},
            "caption": "Fix this screen",
            "media_group_id": "g1",
            "reply_to_message": {"message_id": 77},
            "photo": [
                {"file_id": "small", "file_unique_id": "u1", "file_size": 10},
                {"file_id": "large", "file_unique_id": "u2", "file_size": 900}
            ],
            "document": {"file_id": "doc", "file_unique_id": "u3", "file_name": "log.txt"},
            "voice": {"file_id": "v", "file_unique_id": "u4"}
        }}]));
        let message = &received
            .map_err(ChannelError::as_str)
            .unwrap_or_default()
            .messages[0];
        assert_eq!(message.text, "Fix this screen");
        assert_eq!(message.reply_to.as_deref(), Some("77"));
        assert_eq!(message.group.as_deref(), Some("g1"));
        let files: Vec<(&str, &str, Option<u64>)> = message
            .files
            .iter()
            .map(|f| (f.id.as_str(), f.name.as_str(), f.size))
            .collect();
        assert_eq!(
            files,
            vec![
                ("large", "photo-u2.jpg", Some(900)),
                ("doc", "log.txt", None),
                ("v", "voice-u4.ogg", None),
            ]
        );
    }

    /// TGI-003: a file is fetched within its bound; one Telegram says is larger is refused
    /// before any byte, as is one whose bytes go past it.
    #[tokio::test]
    async fn a_file_is_fetched_within_its_bound() -> TestResult {
        let (base, _fake) = fake().await?;
        let bot = TelegramBots::new(base, 1).connect(&Secret(TOKEN.into()))?;
        let bytes = bot.fetch_file("p7", 1_000).await.map_err(|e| e.as_str())?;
        assert_eq!(bytes, vec![7_u8; 64]);
        assert_eq!(
            bot.fetch_file("big", 20_000_000).await,
            Err(ChannelError::TooLarge)
        );
        assert_eq!(bot.fetch_file("p7", 63).await, Err(ChannelError::TooLarge));
        Ok(())
    }

    /// TGI-003: only a plain relative `file_path` is fetched.
    #[test]
    fn only_a_plain_file_path_is_fetched() {
        assert!(file_path_ok("photos/file_7.jpg"));
        assert!(file_path_ok("documents/report-1_v2.pdf"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../b",
            "%2e%2e/x",
            "a\\b",
            "a b",
            "a?x=1",
        ] {
            assert!(!file_path_ok(bad), "{bad}");
        }
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
