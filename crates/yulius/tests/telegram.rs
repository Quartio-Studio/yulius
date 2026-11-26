//! The personal Telegram channel against a fake Bot API on 127.0.0.1: the bot set and
//! removed by an instance administrator, chats linked with single-use codes and unlinked, and
//! notifications filtered by the event stream's rule, redacted and rate bound. The real Telegram
//! is never called.

use crate::support;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::routing::post;
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness, perms,
    seed_sandbox, seed_task, setup_superadmin_creds,
};
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::notify::ChatLinkStore as _;
use yulius_app::ports::store::{
    AccountStore as _, AuditStore as _, CoordinationStore as _, TaskStore as _, UserStore as _,
};
use yulius_app::ports::store_ext::NeedsYouStore as _;
use yulius_app::telegram::{ChannelServices, ChannelTuning, UNLINKED_REPLY};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};

/// Synthetic bot tokens of the fake (never a real bot's).
const TOKEN_A: &str = "1000:synthetic-bot-token_a";
const TOKEN_B: &str = "2000:synthetic-bot-token_b";
/// An agent secret of sandbox `s1` that must never reach a chat.
const SECRET: &str = "s3cret-telegram-value-9f8e7d6c5b4a";
const MINUTE_MS: i64 = 60_000;

/// The fake Bot API: two bots, a queue of updates per poll, and every message sent.
#[derive(Default)]
struct FakeTelegram {
    updates: Mutex<VecDeque<Value>>,
    next_update: AtomicI64,
    arrived: tokio::sync::Notify,
    sent: Mutex<Vec<(String, String)>>,
    sent_changed: tokio::sync::Notify,
    polls: AtomicUsize,
    fail_polls: AtomicUsize,
    /// While set, no poll is answered (released with `release`).
    hold_polls: std::sync::atomic::AtomicBool,
    released: tokio::sync::Notify,
    /// While set, no file download is answered (released with `release_files`).
    hold_files: std::sync::atomic::AtomicBool,
    files_released: tokio::sync::Notify,
    /// File downloads asked for, and a wake-up for each.
    files_asked: AtomicUsize,
    file_asked: tokio::sync::Notify,
    /// Woken by every poll, once it has dropped the updates it confirms.
    polled: tokio::sync::Notify,
    /// Messages deleted, as `(chat, message id)`, and a wake-up for each.
    deleted: Mutex<Vec<(String, String)>>,
    deleted_changed: tokio::sync::Notify,
    /// While set, a delete is refused as Telegram refuses an old message's.
    fail_deletes: std::sync::atomic::AtomicBool,
}

impl FakeTelegram {
    fn bot(token: &str) -> Option<Value> {
        match token {
            TOKEN_A => Some(json!({"id": 1000, "is_bot": true, "username": "yulius_a_bot"})),
            TOKEN_B => Some(json!({"id": 2000, "is_bot": true, "username": "yulius_b_bot"})),
            _ => None,
        }
    }

    /// A message from `chat` (`private` or a group).
    fn message(&self, chat: i64, private: bool, text: &str) {
        let id = self.next_update.fetch_add(1, Ordering::SeqCst) + 1;
        let kind = if private { "private" } else { "group" };
        if let Ok(mut updates) = self.updates.lock() {
            updates.push_back(json!({"update_id": id, "message": {
                "chat": {"id": chat, "type": kind}, "from": {"id": chat}, "text": text}}));
        }
        self.arrived.notify_waiters();
    }

    /// Any message `body` from the private chat `chat` (a reply, a photo; SPEC-512 TGI-002).
    fn raw(&self, chat: i64, body: Value) {
        let id = self.next_update.fetch_add(1, Ordering::SeqCst) + 1;
        let mut message = json!({"chat": {"id": chat, "type": "private"}, "from": {"id": chat}});
        if let (Some(m), Some(extra)) = (message.as_object_mut(), body.as_object()) {
            m.extend(extra.clone());
        }
        if let Ok(mut updates) = self.updates.lock() {
            updates.push_back(json!({"update_id": id, "message": message}));
        }
        self.arrived.notify_waiters();
    }

    /// Messages `bodies` from `chat`, all in the queue before the poll wakes: one batch.
    fn raw_batch(&self, chat: i64, bodies: &[Value]) {
        if let Ok(mut updates) = self.updates.lock() {
            for body in bodies {
                let id = self.next_update.fetch_add(1, Ordering::SeqCst) + 1;
                let mut message =
                    json!({"chat": {"id": chat, "type": "private"}, "from": {"id": chat}});
                if let (Some(m), Some(extra)) = (message.as_object_mut(), body.as_object()) {
                    m.extend(extra.clone());
                }
                updates.push_back(json!({"update_id": id, "message": message}));
            }
        }
        self.arrived.notify_waiters();
    }

    fn release_files(&self) {
        self.hold_files.store(false, Ordering::SeqCst);
        self.files_released.notify_waiters();
    }

    /// Waits until a poll confirmed every update sent so far (10 s at most).
    async fn wait_updates_taken(&self) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let polled = self.polled.notified();
            if self.updates.lock().map(|u| u.is_empty()).unwrap_or(false) {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, polled).await.is_err() {
                return Err("the updates were not taken".into());
            }
        }
    }

    /// Waits until a file download was asked for (10 s at most).
    async fn wait_file_asked(&self) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let asked = self.file_asked.notified();
            if self.files_asked.load(Ordering::SeqCst) > 0 {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, asked).await.is_err() {
                return Err("no file download was asked for".into());
            }
        }
    }

    /// The id of the last message sent to `chat` that starts with `lead`.
    fn sent_id(&self, chat: i64, lead: &str) -> Option<usize> {
        let chat = chat.to_string();
        self.sent.lock().ok().and_then(|sent| {
            sent.iter()
                .enumerate()
                .rev()
                .find(|(_, (c, t))| *c == chat && t.starts_with(lead))
                .map(|(i, _)| i + 1)
        })
    }

    fn sent_to(&self, chat: i64) -> Vec<String> {
        let chat = chat.to_string();
        self.sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter(|(c, _)| *c == chat)
                    .map(|(_, t)| t.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The id the fake gave the `nth` message (from 0) sent to `chat`: its place among all sent.
    fn id_of(&self, chat: i64, nth: usize) -> Option<String> {
        let chat = chat.to_string();
        let sent = self.sent.lock().ok()?;
        sent.iter()
            .enumerate()
            .filter(|(_, (c, _))| *c == chat)
            .nth(nth)
            .map(|(place, _)| (place + 1).to_string())
    }

    fn deleted(&self) -> Vec<(String, String)> {
        self.deleted.lock().map(|d| d.clone()).unwrap_or_default()
    }

    /// Waits until `count` messages were deleted (10 s at most); they all.
    async fn wait_deleted(&self, count: usize) -> Result<Vec<(String, String)>, String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let changed = self.deleted_changed.notified();
            let deleted = self.deleted();
            if deleted.len() >= count {
                return Ok(deleted);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(format!("{} of {count} deleted: {deleted:?}", deleted.len()));
            }
        }
    }

    /// Waits until `chat` was sent `count` messages (10 s at most); they all.
    async fn wait_sent(&self, chat: i64, count: usize) -> Result<Vec<String>, String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let changed = self.sent_changed.notified();
            let sent = self.sent_to(chat);
            if sent.len() >= count {
                return Ok(sent);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(format!(
                    "chat {chat} got {} of {count}: {sent:?}",
                    sent.len()
                ));
            }
        }
    }
}

async fn bot_api(
    State(fake): State<Arc<FakeTelegram>>,
    Path((bot, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let Some(me) = bot.strip_prefix("bot").and_then(FakeTelegram::bot) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false})));
    };
    let result = match method.as_str() {
        "getMe" => me,
        "getFile" => {
            json!({"file_id": body["file_id"], "file_size": 3, "file_path": "photos/p.jpg"})
        }
        "sendMessage" => {
            let chat = body["chat_id"].as_i64().unwrap_or_default().to_string();
            let text = body["text"].as_str().unwrap_or_default().to_owned();
            // Each sent message's id is its place among all sent.
            let id = fake.sent.lock().map_or(0, |mut sent| {
                sent.push((chat, text));
                sent.len()
            });
            fake.sent_changed.notify_waiters();
            json!({"message_id": id})
        }
        "deleteMessage" => {
            if fake.fail_deletes.load(Ordering::SeqCst) {
                return (StatusCode::BAD_REQUEST, Json(json!({"ok": false})));
            }
            let chat = body["chat_id"].as_i64().unwrap_or_default().to_string();
            let message = body["message_id"].as_i64().unwrap_or_default().to_string();
            if let Ok(mut deleted) = fake.deleted.lock() {
                deleted.push((chat, message));
            }
            fake.deleted_changed.notify_waiters();
            json!(true)
        }
        "getUpdates" => {
            loop {
                let released = fake.released.notified();
                if !fake.hold_polls.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
            if fake
                .fail_polls
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return (StatusCode::BAD_GATEWAY, Json(json!({"ok": false})));
            }
            fake.polls.fetch_add(1, Ordering::SeqCst);
            let offset = body["offset"].as_i64().unwrap_or(0);
            let wait = Duration::from_secs(body["timeout"].as_u64().unwrap_or(0));
            let pending = |fake: &FakeTelegram| -> Vec<Value> {
                let Ok(mut updates) = fake.updates.lock() else {
                    return Vec::new();
                };
                updates.retain(|u| u["update_id"].as_i64().unwrap_or(0) >= offset);
                updates.iter().cloned().collect()
            };
            let arrived = fake.arrived.notified();
            let mut ready = pending(&fake);
            fake.polled.notify_waiters();
            if ready.is_empty() {
                let _ = tokio::time::timeout(wait, arrived).await;
                ready = pending(&fake);
            }
            // Telegram answers at most `limit` updates, 100 when none is asked for.
            let limit = body["limit"].as_u64().unwrap_or(100);
            ready.truncate(usize::try_from(limit).unwrap_or(100));
            Value::Array(ready)
        }
        _ => return (StatusCode::NOT_FOUND, Json(json!({"ok": false}))),
    };
    (StatusCode::OK, Json(json!({"ok": true, "result": result})))
}

/// A file's bytes, held while `hold_files` is set.
async fn file_api(State(fake): State<Arc<FakeTelegram>>) -> (StatusCode, Vec<u8>) {
    fake.files_asked.fetch_add(1, Ordering::SeqCst);
    fake.file_asked.notify_waiters();
    loop {
        let released = fake.files_released.notified();
        if !fake.hold_files.load(Ordering::SeqCst) {
            break;
        }
        released.await;
    }
    (StatusCode::OK, b"png".to_vec())
}

struct World {
    h: Harness,
    /// Holds the notifier's seeds while written ([`ChannelServices::seed_hold`]).
    seeds: Arc<tokio::sync::RwLock<()>>,
    fake: Arc<FakeTelegram>,
    admin: Creds,
    alice: Creds,
    bob: Creds,
    carol: Creds,
    _served: support::Served,
}

/// An instance with the channel wired to the fake; alice (an instance administrator) may write
/// in `s1` (`acme/app`), bob sees nothing, carol reads `s2` (`acme/other`) only.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let w = world_unstarted().await?;
    w.h.app.start_channel().await;
    // Subscription problems are posted notices, as the server starts them.
    w.h.app.start_notice_producers();
    Ok(w)
}

/// [`world`] before its channel starts, as a server is before `start_channel`.
async fn world_unstarted() -> Result<World, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 12, false).await?;
    let carol = admit_and_sign_in(&h, &admin, "carol", 13, false).await?;
    let alice_id = h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_sandbox(&h, "s1", "app", alice_id.as_str()).await?;
    seed_sandbox(&h, "s2", "other", alice_id.as_str()).await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("carol", "acme/other", perms("read"));
    let fake = Arc::new(FakeTelegram::default());
    let served = support::serve(
        Router::new()
            .route("/{bot}/{method}", post(bot_api))
            .route("/file/{bot}/{*path}", axum::routing::get(file_api))
            .with_state(fake.clone()),
    )
    .await?;
    let bots = yulius::wiring::telegram::TelegramBots::new(format!("http://{}", served.addr), 1);
    let services = ChannelServices::new(
        Arc::new(bots),
        ChannelTuning {
            backoff_min: Duration::from_millis(20),
            backoff_max: Duration::from_millis(200),
            window_ms: MINUTE_MS,
            flush_every: Duration::from_millis(50),
        },
    );
    let seeds = services.seed_hold();
    h.app.set_channel(services);
    Ok(World {
        h,
        seeds,
        fake,
        admin,
        alice,
        bob,
        carol,
        _served: served,
    })
}

async fn api(
    w: &World,
    method: Method,
    path: &str,
    body: Option<Value>,
    who: &Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    let r = call(&w.h.router, method, path, body, who).await?;
    let text = r.body.to_string();
    assert!(
        !text.contains("synthetic-bot-token"),
        "a response carried the token: {text}"
    );
    Ok(r)
}

async fn set_bot(w: &World, token: &str) -> Result<support::Resp, Box<dyn std::error::Error>> {
    api(
        w,
        Method::PUT,
        "/api/admin/telegram",
        Some(json!({ "token": token })),
        &w.admin,
    )
    .await
}

/// Waits until the admin view's state is `state` (10 s at most).
async fn wait_state(w: &World, state: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = api(w, Method::GET, "/api/admin/telegram", None, &w.admin).await?;
        if view.body["state"] == state {
            return Ok(view.body);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("state never became {state}: {}", view.body).into());
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Links `who`'s chat `chat` through a code and `/start`; the code.
async fn link(w: &World, who: &Creds, chat: i64) -> Result<String, Box<dyn std::error::Error>> {
    let before = w.fake.sent_to(chat).len();
    let r = api(w, Method::POST, "/api/me/telegram/link", None, who).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let code = r.body["code"].as_str().ok_or("code")?.to_owned();
    w.fake.message(chat, true, &format!("/start {code}"));
    let sent = w.fake.wait_sent(chat, before + 1).await?;
    assert!(sent[before].starts_with("Linked"), "{sent:?}");
    Ok(code)
}

async fn audits(
    w: &World,
    target: &str,
    action: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(w.h.store.audit_details(target, action, 20).await?)
}

async fn user_id(w: &World, login: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(w.h
        .store
        .user_by_login(login)
        .await?
        .ok_or("user")?
        .id
        .as_str()
        .to_owned())
}

/// The task's agent asks a person (`needs_you`), and the change is published.
async fn ask(w: &World, task: &str, problem: &str) -> TestResult {
    ask_about(w, task, problem, None).await
}

/// [`ask`] naming a problem of the whole sandbox (`cause`).
async fn ask_about(w: &World, task: &str, problem: &str, cause: Option<&str>) -> TestResult {
    let id = TaskId::parse(task)?;
    let request = NeedsYouRecord {
        action: "Add the payment key".into(),
        happened: None,
        problem: Some(problem.into()),
        decision: false,
        link: None,
        cause: cause.map(str::to_owned),
        at: w.h.app.now().0,
        named: Default::default(),
    };
    w.h.store
        .set_task_needs_you(&id, Some(&request), w.h.app.now())
        .await?;
    w.h.app.publish_task(&id).await;
    Ok(())
}

/// Every event published before this has been handled by the notifier: carol's chat gets the
/// notification of a new task of `s2` (only she sees `s2`), sent after whatever came before.
async fn barrier(w: &World, task: &str) -> TestResult {
    let before = w.fake.sent_to(300).len();
    let carol = user_id(w, "carol").await?;
    seed_task(&w.h, task, "s2", &carol, TaskStatus::Running, None).await?;
    ask(w, task, "barrier").await?;
    w.fake.wait_sent(300, before + 1).await?;
    Ok(())
}

/// An instance administrator sets the bot (checked, sealed, audited, never
/// returned) and removing it stops polling and unlinks; others are refused.
#[tokio::test]
async fn an_instance_administrator_sets_and_removes_the_bot() -> TestResult {
    let w = world().await?;
    let off = api(&w, Method::GET, "/api/admin/telegram", None, &w.admin).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(off.body["configured"], false);
    assert_eq!(off.body["state"], "off");
    let member = api(&w, Method::GET, "/api/admin/telegram", None, &w.bob).await?;
    assert_eq!(member.status, StatusCode::FORBIDDEN);
    let member = call(
        &w.h.router,
        Method::PUT,
        "/api/admin/telegram",
        Some(json!({"token": TOKEN_A})),
        &w.bob,
    )
    .await?;
    assert_eq!(member.status, StatusCode::FORBIDDEN);
    for bad in ["", "not-a-token", "3000:refused-by-telegram"] {
        let r = set_bot(&w, bad).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad:?}: {}",
            r.body
        );
    }
    assert!(w.h.store.chat_bot().await?.is_none(), "nothing saved");

    // An instance administrator who is not the superadmin sets it.
    w.fake.hold_polls.store(true, Ordering::SeqCst);
    let r = api(
        &w,
        Method::PUT,
        "/api/admin/telegram",
        Some(json!({ "token": format!(" {TOKEN_A}\n") })),
        &w.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["configured"], true);
    assert_eq!(r.body["bot_username"], "yulius_a_bot");
    assert_eq!(r.body["set_by"], "alice");
    // No poll answered yet (the fake holds them): it is starting (review round 1: "polling"
    // claimed it was receiving before any poll succeeded).
    assert_eq!(r.body["state"], "starting", "{}", r.body);
    assert_eq!(wait_state(&w, "starting").await?["state"], "starting");
    w.fake.hold_polls.store(false, Ordering::SeqCst);
    w.fake.released.notify_waiters();
    let stored = w.h.store.chat_bot().await?.ok_or("stored")?;
    assert!(
        !String::from_utf8_lossy(&stored.sealed.0).contains("synthetic"),
        "the token is sealed at rest"
    );
    assert_eq!(
        audits(&w, "telegram", "telegram.bot_set").await?,
        vec!["@yulius_a_bot (unlinked 0)".to_owned()]
    );
    wait_state(&w, "polling").await?;

    link(&w, &w.alice, 100).await?;
    // The same bot again keeps the link; another bot unlinks every chat.
    assert_eq!(set_bot(&w, TOKEN_A).await?.body["linked"], 1);
    assert_eq!(set_bot(&w, TOKEN_B).await?.body["linked"], 0);
    link(&w, &w.alice, 100).await?;

    let r = api(&w, Method::DELETE, "/api/admin/telegram", None, &w.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["configured"], false);
    assert_eq!(r.body["state"], "off");
    assert_eq!(r.body["linked"], 0);
    assert!(w.h.store.chat_bot().await?.is_none());
    assert_eq!(
        audits(&w, "telegram", "telegram.bot_removed").await?,
        vec!["unlinked 1".to_owned()]
    );
    // Polling stopped (the poller task is gone), and nobody can link.
    assert!(!w.h.app.telegram_polling(), "the poll ended with the bot");
    let r = api(&w, Method::POST, "/api/me/telegram/link", None, &w.alice).await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    let mine = api(&w, Method::GET, "/api/me/telegram", None, &w.alice).await?;
    assert_eq!(mine.body["available"], false);
    Ok(())
}

/// A single-use 10-minute code from a browser session links a private chat;
/// a used, expired or group code does not; `/stop` and Settings unlink; all audited.
#[tokio::test]
async fn a_person_links_with_a_single_use_code_and_unlinks() -> TestResult {
    let w = world().await?;
    let mine = api(&w, Method::GET, "/api/me/telegram", None, &w.alice).await?;
    assert_eq!(mine.body["available"], false);
    assert_eq!(set_bot(&w, TOKEN_A).await?.status, StatusCode::OK);
    let alice = user_id(&w, "alice").await?;

    // No API token links a chat.
    let user = w.h.store.user_by_login("alice").await?.ok_or("alice")?;
    let token =
        w.h.app
            .issue_api_token(&user, "cli", yulius_domain::permission::TokenScope::Operate)
            .await?;
    let bearer = Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    };
    let r = api(&w, Method::POST, "/api/me/telegram/link", None, &bearer).await?;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);

    let r = api(&w, Method::POST, "/api/me/telegram/link", None, &w.alice).await?;
    let code = r.body["code"].as_str().ok_or("code")?.to_owned();
    assert_eq!(code.len(), 16);
    assert_eq!(
        r.body["link"],
        format!("https://t.me/yulius_a_bot?start={code}")
    );
    // A group chat never links, even with a valid code.
    w.fake.message(-500, false, &format!("/start {code}"));
    assert_eq!(w.fake.wait_sent(-500, 1).await?, vec![UNLINKED_REPLY]);
    assert!(w.h.store.chat_link_of("-500").await?.is_none());
    // The code is still unused: a private chat links with it, once.
    w.fake.message(100, true, &format!("/start {code}"));
    let sent = w.fake.wait_sent(100, 1).await?;
    assert!(sent[0].contains("for alice"), "{sent:?}");
    let mine = api(&w, Method::GET, "/api/me/telegram", None, &w.alice).await?;
    assert_eq!(mine.body["linked"], true);
    assert_eq!(mine.body["choices"]["needs_you"], true);
    assert_eq!(mine.body["choices"]["done"], false);
    assert_eq!(
        audits(&w, &alice, "telegram.linked").await?,
        vec!["telegram"]
    );
    w.fake.message(101, true, &format!("/start {code}"));
    assert_eq!(w.fake.wait_sent(101, 1).await?, vec![UNLINKED_REPLY]);
    assert!(w.h.store.chat_link_of("101").await?.is_none());

    // An expired code does not link.
    let r = api(&w, Method::POST, "/api/me/telegram/link", None, &w.bob).await?;
    let late = r.body["code"].as_str().ok_or("code")?.to_owned();
    w.h.clock.0.fetch_add(10 * MINUTE_MS + 1, Ordering::SeqCst);
    w.fake.message(200, true, &format!("/start {late}"));
    assert_eq!(w.fake.wait_sent(200, 1).await?, vec![UNLINKED_REPLY]);
    assert!(w.h.store.chat_link_of("200").await?.is_none());

    // /stop in the chat unlinks.
    w.fake.message(100, true, "/stop");
    let sent = w.fake.wait_sent(100, 2).await?;
    assert!(sent[1].starts_with("Unlinked"), "{sent:?}");
    assert!(w.h.store.chat_link_of("100").await?.is_none());
    assert_eq!(
        audits(&w, &alice, "telegram.unlinked").await?,
        vec!["telegram"]
    );

    // Settings unlinks, and tells the chat.
    link(&w, &w.alice, 100).await?;
    let r = api(&w, Method::DELETE, "/api/me/telegram", None, &w.alice).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["linked"], false);
    let sent = w.fake.wait_sent(100, 4).await?;
    assert!(sent[3].contains("Unlinked in Yulius Settings"), "{sent:?}");
    let unlinked = audits(&w, &alice, "telegram.unlinked").await?;
    assert_eq!(unlinked, vec!["settings", "telegram"]);
    Ok(())
}

/// An unlinked chat gets one fixed reply, at most every 10 minutes, and nothing else.
#[tokio::test]
async fn an_unlinked_chat_gets_one_fixed_reply() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    for text in ["hello", "/start", "/start AAAAAAAAAAAAAAAA", "/stop"] {
        w.fake.message(777, true, text);
    }
    w.fake.wait_sent(777, 1).await?;
    // A linked chat's reply proves the four messages before it were handled.
    link(&w, &w.carol, 300).await?;
    assert_eq!(w.fake.sent_to(777), vec![UNLINKED_REPLY]);
    assert!(w.h.store.chat_link_of("777").await?.is_none());
    w.h.clock.0.fetch_add(10 * MINUTE_MS, Ordering::SeqCst);
    w.fake.message(777, true, "again");
    assert_eq!(w.fake.wait_sent(777, 2).await?.len(), 2);
    // A linked chat's help is bounded the same way, so no chat fills the send queue.
    for text in ["hi", "hello", "/start"] {
        w.fake.message(300, true, text);
    }
    w.fake.message(777, true, "a third time");
    w.h.clock.0.fetch_add(10 * MINUTE_MS, Ordering::SeqCst);
    w.fake.message(777, true, "and a fourth");
    w.fake.wait_sent(777, 3).await?;
    let carol = w.fake.sent_to(300);
    assert_eq!(carol.len(), 2, "{carol:?}");
    assert!(
        carol[1].starts_with("This chat gets your Yulius notifications"),
        "{carol:?}"
    );
    Ok(())
}

/// A task entering Needs you reaches the linked people who see it, with its
/// link and its sandbox's agent secrets redacted; others and unwanted kinds get nothing.
#[tokio::test]
async fn needs_you_reaches_only_its_viewers_redacted() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "PAYMENT_KEY", "value": SECRET})),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    link(&w, &w.alice, 100).await?;
    link(&w, &w.bob, 200).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let task = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;

    ask(&w, "t1", &format!("Checkout fails with {SECRET}")).await?;
    let sent = w.fake.wait_sent(100, 2).await?;
    let text = &sent[1];
    assert!(text.starts_with("Needs you · app\n"), "{text}");
    assert!(
        text.contains(&format!("#{} task t1", task.number)),
        "{text}"
    );
    assert!(text.contains("To do: Add the payment key"), "{text}");
    assert!(
        text.ends_with(&format!("https://yulius.test/s/app/t/{}", task.number)),
        "{text}"
    );
    assert!(!text.contains(SECRET), "redacted: {text}");
    assert!(text.contains("Checkout fails with"), "{text}");
    // The same request again is not news.
    w.h.app.publish_task(&TaskId::parse("t1")?).await;
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2);
    assert_eq!(w.fake.sent_to(200).len(), 1, "bob cannot see s1");

    // Done reaches alice only once she chose it.
    let id = TaskId::parse("t1")?;
    w.h.store
        .set_task_needs_you(&id, None, w.h.app.now())
        .await?;
    w.h.store
        .set_activity(&id, Some(Activity::Done), w.h.app.now())
        .await?;
    w.h.app.publish_task(&id).await;
    barrier(&w, "c2").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2, "done is off by default");
    let r = api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(json!({"needs_you": true, "done": true, "failed": false, "account": false})),
        &w.alice,
    )
    .await?;
    assert_eq!(r.body["choices"]["done"], true, "{}", r.body);
    w.h.store
        .set_activity(&id, Some(Activity::Working), w.h.app.now())
        .await?;
    w.h.app.publish_task(&id).await;
    // The notifier reads the task when it handles the event: it sees Working before Done.
    barrier(&w, "c3").await?;
    w.h.store
        .set_activity(&id, Some(Activity::Done), w.h.app.now())
        .await?;
    w.h.app.publish_task(&id).await;
    let sent = w.fake.wait_sent(100, 3).await?;
    assert!(sent[2].starts_with("Done · app\n"), "{sent:?}");

    // A crash is told to those who chose failures; a person's own stop is not.
    api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(json!({"needs_you": true, "done": true, "failed": true, "account": true})),
        &w.alice,
    )
    .await?;
    w.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Stopped(yulius_domain::task::StopReason::Crashed),
            None,
            w.h.app.now(),
        )
        .await?;
    w.h.app.publish_task(&id).await;
    let sent = w.fake.wait_sent(100, 4).await?;
    assert!(sent[3].starts_with("Stopped (crashed) · app\n"), "{sent:?}");
    // Carol, who cannot choose subscription problems, is not given that choice.
    let r = api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(json!({"needs_you": true, "done": false, "failed": false, "account": true})),
        &w.carol,
    )
    .await?;
    assert_eq!(r.body["choices"]["account"], false);
    assert_eq!(r.body["account_choice"], false);
    Ok(())
}

/// Seven notifications in one minute: five messages, then one summary of the other two.
#[tokio::test]
async fn a_burst_is_folded_into_one_summary() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    for n in 1..=7 {
        let task = format!("t{n}");
        seed_task(&w.h, &task, "s1", &alice, TaskStatus::Running, None).await?;
        ask(&w, &task, "blocked").await?;
    }
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.sent_to(100).len(), 1 + 5);
    w.h.clock.0.fetch_add(MINUTE_MS, Ordering::SeqCst);
    let sent = w.fake.wait_sent(100, 1 + 6).await?;
    let summary = &sent[6];
    assert!(
        summary.starts_with("2 more notifications in the last minute:\n• Needs you · #"),
        "{summary}"
    );
    assert!(summary.ends_with("https://yulius.test"), "{summary}");
    Ok(())
}

/// After errors the poll retries with backoff, says so, and recovers.
#[tokio::test]
async fn polling_retries_after_errors_and_recovers() -> TestResult {
    let w = world().await?;
    w.fake.fail_polls.store(1_000, Ordering::SeqCst);
    set_bot(&w, TOKEN_A).await?;
    let view = wait_state(&w, "retrying").await?;
    assert_eq!(view["last_error"], "unreachable");
    w.fake.fail_polls.store(0, Ordering::SeqCst);
    wait_state(&w, "polling").await?;
    link(&w, &w.alice, 100).await?;
    Ok(())
}

/// What was already the case when the bot became active (a start of the server, or a
/// bot set in Settings) is not told; a change after it is, even one that moves no status or
/// activity (review round 1: a chain blocked later was dropped for good).
#[tokio::test]
async fn a_state_entered_before_the_start_is_not_told() -> TestResult {
    use yulius_app::ports::store_ext::IntakeStore as _;
    let w = world().await?;
    let alice = user_id(&w, "alice").await?;
    // Before the bot: t1 needs you, t2 waits for t1, a subscription must sign in again.
    let first = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    let id = TaskId::parse("t1")?;
    let request = NeedsYouRecord {
        action: "old".into(),
        happened: None,
        problem: None,
        decision: false,
        link: None,
        cause: None,
        at: w.h.app.now().0,
        named: Default::default(),
    };
    w.h.store
        .set_task_needs_you(&id, Some(&request), w.h.app.now())
        .await?;
    seed_task(&w.h, "t2", "s1", &alice, TaskStatus::Chained, None).await?;
    let t2 = TaskId::parse("t2")?;
    w.h.store
        .insert_chain(&yulius_app::intake::ChainRecord {
            task_id: t2.clone(),
            after_task_id: Some(id.clone()),
            after_number: first.number,
            blocked_reason: None,
            created_at: w.h.app.now(),
            rule: None,
        })
        .await?;
    subscription(&w, AccountStatus::Relogin).await?;
    // The change as an event (the tick would reconcile it too): its notice is posted before any
    // bot is active.
    let mut events = w.h.app.events.subscribe();
    account_event(&w, "relogin");
    loop {
        if let yulius_contracts::events::UiEvent::NoticePosted { notice } =
            tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await??
                .event
            && notice.key == "account:acc1"
        {
            break;
        }
    }
    w.h.clock.0.fetch_add(MINUTE_MS, Ordering::SeqCst);

    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let all = json!({"needs_you": true, "done": true, "failed": true, "account": true});
    api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(all),
        &w.alice,
    )
    .await?;
    w.h.app.publish_task(&id).await;
    w.h.app.publish_task(&t2).await;
    account_event(&w, "relogin");
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.sent_to(100).len(), 1, "only the link's reply");

    // After it: the chain is blocked (no status or activity moves) and the subscription is
    // limited: both are news.
    w.h.store
        .set_chain_blocked(&t2, Some("#1 crashed. Start this task now or cancel it."))
        .await?;
    w.h.app.publish_task(&t2).await;
    let sent = w.fake.wait_sent(100, 2).await?;
    assert!(
        sent[1].starts_with(&format!("Needs you · app\n#{} task t2\n", first.number + 1)),
        "{sent:?}"
    );
    // Stored first, as the accounts code does before it publishes the view.
    w.h.store
        .set_account_state(
            &AccountId::parse("acc1")?,
            AccountStatus::Limited,
            None,
            None,
            w.h.app.now(),
        )
        .await?;
    account_event(&w, "limited");
    let sent = w.fake.wait_sent(100, 3).await?;
    assert!(
        sent[2].starts_with("Team Max reached its usage limit\n"),
        "{sent:?}"
    );
    Ok(())
}

/// Review round 1: while the bot is active, what happens before anyone links is remembered, so
/// the first person to link is not told of hours-old states as new.
#[tokio::test]
async fn a_state_entered_before_anyone_linked_is_not_told_later() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    ask(&w, "t1", "nobody is linked yet").await?;
    // The notifier handled it (the bot's poll and the bus run on): carol links and her barrier
    // passes after it.
    link(&w, &w.carol, 300).await?;
    barrier(&w, "c0").await?;
    link(&w, &w.alice, 100).await?;
    w.h.app.publish_task(&TaskId::parse("t1")?).await;
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.sent_to(100).len(), 1, "{:?}", w.fake.sent_to(100));
    Ok(())
}

/// A subscription `acc1` ("Team Max") with `status`, stored.
async fn subscription(w: &World, status: AccountStatus) -> TestResult {
    w.h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse("acc1")?,
            provider: Provider::Anthropic,
            label: "Team Max".into(),
            credential: None,
            status,
            usage_json: None,
            last_error: None,
            created_at: w.h.app.now(),
            updated_at: w.h.app.now(),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    Ok(())
}

/// Publishes `acc1`'s view with `status`, as the accounts code does after an edit or a sign-in
/// (a status written without one is reconciled by the tick).
fn account_event(w: &World, status: &str) {
    use yulius_contracts::api::admin::{AccountUsageView, AccountView};
    use yulius_contracts::events::UiEvent;
    w.h.app.events.publish(
        None,
        true,
        UiEvent::AccountUpdated {
            account: Box::new(AccountView {
                id: "acc1".into(),
                provider: "anthropic".into(),
                label: "Team Max".into(),
                status: status.into(),
                usage: AccountUsageView::default(),
                last_error: None,
                email: None,
                email_typed: false,
                in_use_by: 0,
                tokens_24h: None,
                tokens_7d: None,
                billing: None,
                usage_read: None,
            }),
        },
    );
}

/// Review round 1: a summary held for a chat is never sent after that chat was unlinked.
#[tokio::test]
async fn a_summary_never_reaches_a_chat_unlinked_meanwhile() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let carol = user_id(&w, "carol").await?;
    for n in 1..=7 {
        for (who, sandbox, prefix) in [(&alice, "s1", "a"), (&carol, "s2", "c")] {
            let task = format!("{prefix}{n}");
            seed_task(&w.h, &task, sandbox, who, TaskStatus::Running, None).await?;
            ask(&w, &task, "blocked").await?;
        }
    }
    // Bob alone sees `s3`: his notice is handled after all fourteen above, so the clock moves on
    // only once each person's bound holds its two.
    seed_sandbox(&w.h, "s3", "third", &alice).await?;
    w.h.github.set_perm("bob", "acme/third", perms("read"));
    link(&w, &w.bob, 200).await?;
    seed_task(&w.h, "b1", "s3", &alice, TaskStatus::Running, None).await?;
    ask(&w, "b1", "barrier").await?;
    w.fake.wait_sent(200, 2).await?;
    assert_eq!(w.fake.sent_to(100).len(), 1 + 5);
    assert_eq!(w.fake.sent_to(300).len(), 1 + 5);
    w.fake.message(100, true, "/stop");
    let sent = w.fake.wait_sent(100, 1 + 5 + 1).await?;
    assert!(sent[6].starts_with("Unlinked"), "{sent:?}");
    // The minute ends: carol's summary goes, then a new notice of hers, sent after any summary.
    w.h.clock.0.fetch_add(MINUTE_MS, Ordering::SeqCst);
    let sent = w.fake.wait_sent(300, 1 + 5 + 1).await?;
    assert!(sent[6].starts_with("2 more notifications"), "{sent:?}");
    barrier(&w, "after").await?;
    let alice_chat = w.fake.sent_to(100);
    assert_eq!(
        alice_chat.len(),
        7,
        "no summary after /stop: {alice_chat:?}"
    );
    Ok(())
}

/// Review round 1: subscription problems reach instance administrators who chose them, redacted;
/// a disabled person gets nothing; a chat taken by another person is audited for the first.
#[tokio::test]
async fn subscription_problems_reach_administrators_and_disabled_people_get_nothing() -> TestResult
{
    use yulius_contracts::api::admin::{AccountUsageView, AccountView};
    use yulius_contracts::events::UiEvent;
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.bob, 200).await?;
    link(&w, &w.carol, 300).await?;
    let all = json!({"needs_you": true, "done": false, "failed": false, "account": true});
    api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(all),
        &w.alice,
    )
    .await?;
    // Bob is no administrator: even a stored choice gives him no subscription notices.
    let bob = w.h.store.user_by_login("bob").await?.ok_or("bob")?.id;
    let wants_all = yulius_domain::notify::NotifyChoices {
        needs_you: true,
        done: true,
        failed: true,
        account: true,
        digest: true,
    };
    assert!(w.h.store.set_chat_choices(&bob, &wants_all).await?);
    let secret = concat!("ghp_", "Zx9Telegram0Account1Fixture2Body3456");
    w.h.app.events.publish(
        None,
        true,
        UiEvent::AccountUpdated {
            account: Box::new(AccountView {
                id: "acc1".into(),
                provider: "anthropic".into(),
                label: "Team Max".into(),
                status: "relogin".into(),
                usage: AccountUsageView::default(),
                last_error: Some(format!("token {secret} expired")),
                email: None,
                email_typed: false,
                in_use_by: 0,
                tokens_24h: None,
                tokens_7d: None,
                billing: None,
                usage_read: None,
            }),
        },
    );
    let sent = w.fake.wait_sent(100, 2).await?;
    assert!(
        sent[1].starts_with("Team Max needs to sign in again\n"),
        "{sent:?}"
    );
    assert!(
        sent[1].ends_with("https://yulius.test/admin/accounts"),
        "{sent:?}"
    );
    assert!(
        !sent[1].contains("Zx9Telegram0Account"),
        "redacted: {sent:?}"
    );

    // Carol is disabled: her sandbox's news no longer reaches her.
    let carol = w.h.store.user_by_login("carol").await?.ok_or("carol")?.id;
    w.h.store
        .update_user_flags(&carol, Some(Some(w.h.app.now())), None)
        .await?;
    seed_task(&w.h, "c1", "s2", carol.as_str(), TaskStatus::Running, None).await?;
    ask(&w, "c1", "blocked").await?;
    // Alice's own task is handled after carol's: her message proves carol's was decided.
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "a1", "s1", &alice, TaskStatus::Running, None).await?;
    ask(&w, "a1", "blocked").await?;
    w.fake.wait_sent(100, 3).await?;
    assert_eq!(w.fake.sent_to(300).len(), 1, "only the link's reply");
    assert_eq!(w.fake.sent_to(200).len(), 1, "bob: nothing");

    // Bob links alice's chat: alice is unlinked, audited as relinked.
    link(&w, &w.bob, 100).await?;
    assert!(
        w.h.store
            .chat_link(&yulius_domain::ids::UserId::parse(&alice)?)
            .await?
            .is_none()
    );
    assert_eq!(
        audits(&w, &alice, "telegram.unlinked").await?,
        vec!["relinked"]
    );
    Ok(())
}

/// Moves task `id` to `status` with `activity` and publishes it.
async fn move_to(
    w: &World,
    id: &str,
    status: TaskStatus,
    activity: Option<Activity>,
) -> TestResult {
    let id = TaskId::parse(id)?;
    w.h.store
        .transition_task(&id, None, status, activity, w.h.app.now())
        .await?;
    w.h.app.publish_task(&id).await;
    Ok(())
}

/// A subscription switch ("Continuing") is under the hood, as in the notification center:
/// it sends nothing, and a task that reads afterwards as it did before (its
/// turn ended with a step open: "Your turn") is not told again; one that reads differently is.
#[tokio::test]
async fn a_subscription_switch_is_no_news() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let task = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    let now = w.h.app.now();
    w.h.store
        .replace_steps(
            &task.id,
            &[yulius_app::model::StepRecord {
                id: yulius_domain::ids::StepId::parse("st-1")?,
                task_id: task.id.clone(),
                ordinal: 1,
                title: "Ship it".into(),
                status: "pending".into(),
                note: None,
                updated_by: Some("system".into()),
                updated_at: Some(now),
                effort: 1,
            }],
            "system",
            now,
        )
        .await?;
    move_to(&w, "t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    let sent = w.fake.wait_sent(100, 2).await?;
    assert!(sent[1].starts_with("Needs you · app\n"), "{sent:?}");

    // The session moves to another subscription and its turn ends as before: no news.
    move_to(&w, "t1", TaskStatus::Switching, None).await?;
    barrier(&w, "c1").await?;
    move_to(&w, "t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    barrier(&w, "c2").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2, "{:?}", w.fake.sent_to(100));

    // After another switch it asks a person: that reads differently, so it is told.
    move_to(&w, "t1", TaskStatus::Switching, None).await?;
    barrier(&w, "c3").await?;
    move_to(&w, "t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    ask(&w, "t1", "the key is missing").await?;
    let sent = w.fake.wait_sent(100, 3).await?;
    assert!(sent[2].contains("the key is missing"), "{sent:?}");
    assert_eq!(w.fake.sent_to(100).len(), 3, "{sent:?}");
    Ok(())
}

/// A parked task's Resume is no news: a "Your turn" told
/// before the park is not told again when the resumed session reads idle as it did.
#[tokio::test]
async fn a_parked_tasks_resume_is_no_news() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let task = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    let now = w.h.app.now();
    w.h.store
        .replace_steps(
            &task.id,
            &[yulius_app::model::StepRecord {
                id: yulius_domain::ids::StepId::parse("st-1")?,
                task_id: task.id.clone(),
                ordinal: 1,
                title: "Ship it".into(),
                status: "pending".into(),
                note: None,
                updated_by: Some("system".into()),
                updated_at: Some(now),
                effort: 1,
            }],
            "system",
            now,
        )
        .await?;
    move_to(&w, "t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    let sent = w.fake.wait_sent(100, 2).await?;
    assert!(sent[1].starts_with("Needs you · app\n"), "{sent:?}");
    // Parked, then resumed with nothing typed: starting, then idle again as it was.
    move_to(&w, "t1", TaskStatus::Parked, Some(Activity::Waiting)).await?;
    barrier(&w, "p1").await?;
    move_to(&w, "t1", TaskStatus::Starting, None).await?;
    barrier(&w, "p2").await?;
    move_to(&w, "t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    barrier(&w, "p3").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2, "{:?}", w.fake.sent_to(100));
    Ok(())
}

/// A task told it stopped abnormally, continued and stopped the
/// same way again, is told again: a start ends what was told of the run before.
#[tokio::test]
async fn a_failure_after_a_restart_is_told_again() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let chose = api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(json!({"needs_you": true, "done": false, "failed": true, "account": false})),
        &w.alice,
    )
    .await?;
    assert_eq!(chose.body["choices"]["failed"], true, "{}", chose.body);
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    let crashed = TaskStatus::Stopped(yulius_domain::task::StopReason::Crashed);
    move_to(&w, "t1", crashed, None).await?;
    w.fake.wait_sent(100, 2).await?;
    move_to(&w, "t1", TaskStatus::Starting, None).await?;
    barrier(&w, "c1").await?;
    move_to(&w, "t1", crashed, None).await?;
    let sent = w.fake.wait_sent(100, 3).await?;
    assert!(sent[2].contains("crashed"), "{sent:?}");
    Ok(())
}

/// A task's newer state replaces its earlier message in the chat
/// (the bot deletes it once the new one is sent); a delete Telegram refuses still lets the new
/// message through; a message that is no notice deletes nothing.
#[tokio::test]
async fn a_tasks_newer_state_replaces_its_message() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let chose = api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(json!({"needs_you": true, "done": true, "failed": true, "account": false})),
        &w.alice,
    )
    .await?;
    assert_eq!(chose.status, StatusCode::OK, "{}", chose.body);
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    ask(&w, "t1", "The checkout fails").await?;
    let asked = w.fake.wait_sent(100, 2).await?;
    assert!(asked[1].starts_with("Needs you"), "{asked:?}");
    let asked_id = w.fake.id_of(100, 1).ok_or("asked id")?;
    assert!(w.fake.deleted().is_empty(), "the link reply is no notice");

    let id = TaskId::parse("t1")?;
    w.h.store
        .set_task_needs_you(&id, None, w.h.app.now())
        .await?;
    w.h.store
        .set_activity(&id, Some(Activity::Done), w.h.app.now())
        .await?;
    w.h.app.publish_task(&id).await;
    let done = w.fake.wait_sent(100, 3).await?;
    assert!(done[2].starts_with("Done"), "{done:?}");
    assert_eq!(
        w.fake.wait_deleted(1).await?,
        vec![("100".to_owned(), asked_id)],
        "the Needs you message is replaced"
    );

    // An old message Telegram will not delete stays; the new one is sent all the same.
    w.fake.fail_deletes.store(true, Ordering::SeqCst);
    w.h.store.set_activity(&id, None, w.h.app.now()).await?;
    w.h.clock.0.fetch_add(MINUTE_MS, Ordering::SeqCst);
    ask(&w, "t1", "The checkout fails again").await?;
    let again = w.fake.wait_sent(100, 4).await?;
    assert!(again[3].starts_with("Needs you"), "{again:?}");
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.deleted().len(), 1);
    Ok(())
}

/// A posted notice reaches the chats of the people who see its project and
/// chose its kind; the next one with its key replaces it, another key stands beside it.
#[tokio::test]
async fn a_posted_notice_replaces_its_message_by_key() -> TestResult {
    use yulius_app::notices::NewNotice;
    use yulius_domain::notice::{NoticeKey, NoticeTone};
    use yulius_domain::notify::NoticeKind;
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let notice = |key: &str, title: &str| -> Result<NewNotice, Box<dyn std::error::Error>> {
        Ok(NewNotice {
            link: Some("/s/app/ci".into()),
            ..NewNotice::new(
                Some(yulius_domain::ids::SandboxId::parse("s1")?),
                NoticeKey::parse(key)?,
                NoticeKind::NeedsYou,
                NoticeTone::Error,
                title,
            )
        })
    };
    w.h.app
        .post_notice(notice("gate:quick::a", "quick::a fails")?)
        .await?;
    let first = w.fake.wait_sent(100, 2).await?;
    assert_eq!(
        first[1], "app: quick::a fails\nhttps://yulius.test/s/app/ci",
        "{first:?}"
    );
    let first_id = w.fake.id_of(100, 1).ok_or("first id")?;
    w.h.app
        .post_notice(notice("gate:quick::a", "quick::a fails again")?)
        .await?;
    w.fake.wait_sent(100, 3).await?;
    assert_eq!(
        w.fake.wait_deleted(1).await?,
        vec![("100".to_owned(), first_id)]
    );
    w.h.app
        .post_notice(notice("gate:quick::b", "quick::b fails")?)
        .await?;
    w.fake.wait_sent(100, 4).await?;
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.deleted().len(), 1, "another key stands beside it");
    assert_eq!(
        w.fake.sent_to(300).len(),
        2,
        "carol cannot see s1: her link reply and the barrier only"
    );
    // A notice for alice alone with the same key is another notice: its message stands beside
    // the project-wide one (review round 1).
    let alice = w.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    w.h.app
        .post_notice(NewNotice {
            to: Some(alice),
            ..notice("gate:quick::b", "quick::b is yours")?
        })
        .await?;
    w.fake.wait_sent(100, 5).await?;
    barrier(&w, "c2").await?;
    assert_eq!(w.fake.deleted().len(), 1, "the project-wide message stays");
    Ok(())
}

/// Tasks asking about their sandbox's Docker send one message per episode,
/// as the notification center shows one entry, not one per task; a new episode is told again.
#[tokio::test]
async fn a_sandbox_wide_docker_needs_you_is_told_once_per_episode() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let first = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    seed_task(&w.h, "t2", "s1", &alice, TaskStatus::Running, None).await?;
    ask_about(&w, "t1", "Docker does not answer", Some("docker")).await?;
    let sent = w.fake.wait_sent(100, 2).await?;
    let text = &sent[1];
    assert!(
        text.starts_with("Needs you · app: its Docker needs you\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("Asked by #{} task t1", first.number)),
        "{text}"
    );
    assert!(text.contains("To do: Add the payment key"), "{text}");
    w.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    ask_about(&w, "t2", "Docker does not answer", Some("docker")).await?;
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2, "{:?}", w.fake.sent_to(100));

    // Both are answered; Docker fails again later: a new episode.
    for task in ["t1", "t2"] {
        let id = TaskId::parse(task)?;
        w.h.store
            .set_task_needs_you(&id, None, w.h.app.now())
            .await?;
        w.h.app.publish_task(&id).await;
    }
    barrier(&w, "c2").await?;
    w.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    ask_about(&w, "t2", "Docker does not answer", Some("docker")).await?;
    let sent = w.fake.wait_sent(100, 3).await?;
    assert!(
        sent[2].starts_with("Needs you · app: its Docker needs you\n"),
        "{sent:?}"
    );
    Ok(())
}

/// A take reaches the chat of the person it was taken from,
/// under the Needs you choice, and no one else's; so does a take-back.
#[tokio::test]
async fn a_take_reaches_the_chat_of_whoever_it_was_taken_from() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    w.h.github.set_perm("bob", "acme/app", perms("write"));
    link(&w, &w.alice, 100).await?;
    link(&w, &w.bob, 200).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let task = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    let r = api(&w, Method::POST, "/api/tasks/t1/control/take", None, &w.bob).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let sent = w.fake.wait_sent(100, 2).await?;
    assert_eq!(
        sent[1],
        format!(
            "@bob took control of #{n} task t1\nIt acts on GitHub as them now; you can take it \
             back · app\nhttps://yulius.test/s/app/t/{n}",
            n = task.number
        )
    );
    let r = api(
        &w,
        Method::POST,
        "/api/tasks/t1/control/take-back",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let sent = w.fake.wait_sent(200, 2).await?;
    assert_eq!(
        sent[1],
        format!(
            "@alice took back control of #{n} task t1\nIt acts on GitHub as them again · app\n\
             https://yulius.test/s/app/t/{n}",
            n = task.number
        )
    );
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2, "{:?}", w.fake.sent_to(100));
    assert_eq!(w.fake.sent_to(200).len(), 2, "{:?}", w.fake.sent_to(200));
    Ok(())
}

/// A person's request for control of a task reaches the task's
/// owner's chat once, under the Needs you choice, and no other viewer's.
#[tokio::test]
async fn a_request_for_control_reaches_the_owner_only() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    w.h.github.set_perm("bob", "acme/app", perms("write"));
    link(&w, &w.alice, 100).await?;
    link(&w, &w.bob, 200).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    let task = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    let r = api(
        &w,
        Method::POST,
        "/api/tasks/t1/control-request",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let sent = w.fake.wait_sent(100, 2).await?;
    let text = &sent[1];
    assert!(
        text.starts_with(&format!(
            "@bob asks for control of #{} task t1\n",
            task.number
        )),
        "{text}"
    );
    assert!(
        text.contains("It would act on GitHub as them · app"),
        "{text}"
    );
    assert!(
        text.ends_with(&format!("https://yulius.test/s/app/t/{}", task.number)),
        "{text}"
    );
    // The same request seen again is no news; the asker and others hear nothing of it.
    w.h.app.publish_task(&task.id).await;
    barrier(&w, "c1").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2, "{:?}", w.fake.sent_to(100));
    assert_eq!(w.fake.sent_to(200).len(), 1, "{:?}", w.fake.sent_to(200));

    // An owner who turned Needs you off does not get it.
    api(
        &w,
        Method::DELETE,
        "/api/tasks/t1/control-request",
        None,
        &w.bob,
    )
    .await?;
    api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(json!({"needs_you": false, "done": false, "failed": false, "account": false})),
        &w.alice,
    )
    .await?;
    w.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    let r = api(
        &w,
        Method::POST,
        "/api/tasks/t1/control-request",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    barrier(&w, "c2").await?;
    assert_eq!(w.fake.sent_to(100).len(), 2, "{:?}", w.fake.sent_to(100));
    Ok(())
}

/// A backlog of long messages (here 100 of 4096 CJK characters, about
/// 1.2 MB, over the client's 1 MiB answer bound) does not stop the poll: a `/start` behind it
/// still links.
#[tokio::test]
async fn a_backlog_of_long_messages_does_not_stop_linking() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    let r = api(&w, Method::POST, "/api/me/telegram/link", None, &w.alice).await?;
    let code = r.body["code"].as_str().ok_or("code")?.to_owned();
    let long = "\u{6f22}".repeat(4096);
    for _ in 0..100 {
        w.fake.message(-900, false, &long);
    }
    w.fake.message(100, true, &format!("/start {code}"));
    let sent = w.fake.wait_sent(100, 1).await?;
    assert!(sent[0].starts_with("Linked"), "{sent:?}");
    Ok(())
}

/// At a start with a saved bot, what is already the case is read before
/// anything is told: older states are not news, a later change is.
#[tokio::test]
async fn a_start_with_a_saved_bot_tells_only_later_changes() -> TestResult {
    use yulius_app::ports::notify::ChannelIdentity;
    use yulius_app::ports::store_ext::IntakeStore as _;
    let w = world_unstarted().await?;
    let alice = user_id(&w, "alice").await?;
    let carol = user_id(&w, "carol").await?;
    // Saved before the start: the bot, alice's and carol's chats, t1 needing alice, t2 after t1.
    let sealed = w.h.app.deps.crypto.seal(TOKEN_A.as_bytes())?;
    let identity = ChannelIdentity {
        id: 1000,
        username: "yulius_a_bot".into(),
    };
    w.h.store
        .set_chat_bot(Some((&sealed, &identity, "admin")), w.h.app.now())
        .await?;
    w.h.store
        .link_chat(
            &yulius_domain::ids::UserId::parse(&alice)?,
            "100",
            w.h.app.now(),
        )
        .await?;
    w.h.store
        .link_chat(
            &yulius_domain::ids::UserId::parse(&carol)?,
            "300",
            w.h.app.now(),
        )
        .await?;
    let first = seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    ask(&w, "t1", "asked before the start").await?;
    seed_task(&w.h, "t2", "s1", &alice, TaskStatus::Chained, None).await?;
    let t2 = TaskId::parse("t2")?;
    w.h.store
        .insert_chain(&yulius_app::intake::ChainRecord {
            task_id: t2.clone(),
            after_task_id: Some(first.id.clone()),
            after_number: first.number,
            blocked_reason: None,
            created_at: w.h.app.now(),
            rule: None,
        })
        .await?;

    w.h.app.start_channel().await;
    w.h.app.publish_task(&first.id).await;
    barrier(&w, "c1").await?;
    assert!(w.fake.sent_to(100).is_empty(), "{:?}", w.fake.sent_to(100));

    w.h.store
        .set_chain_blocked(&t2, Some("#1 crashed. Start this task now or cancel it."))
        .await?;
    w.h.app.publish_task(&t2).await;
    let sent = w.fake.wait_sent(100, 1).await?;
    assert!(
        sent[0].starts_with(&format!("Needs you · app\n#{} task t2\n", first.number + 1)),
        "{sent:?}"
    );
    Ok(())
}

/// A Save whose request goes away while its bot waits for the seed
/// still finishes under the switch lock, so a Remove after it is never overtaken by that bot's
/// activation (a dropped request let the notifier activate a removed bot).
#[tokio::test]
async fn a_dropped_save_never_brings_a_removed_bot_back() -> TestResult {
    let w = world().await?;
    let hold = Arc::clone(&w.seeds).write_owned().await;
    // The bot is saved and its activation waits for the held seed: then the request goes away
    // (its future is dropped, as hyper drops a handler whose client left).
    {
        let save = set_bot(&w, TOKEN_A);
        tokio::pin!(save);
        let saved = async {
            while w.h.store.chat_bot().await.ok().flatten().is_none() {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! {
            done = &mut save => return Err(format!("the save ended under a held seed: {:?}", done.map(|r| r.body)).into()),
            () = saved => {}
            () = tokio::time::sleep(Duration::from_secs(10)) => return Err("the bot was never saved".into()),
        }
    }
    // A remove meanwhile waits for the switch the save began (it must not overtake it): with the
    // seed held it does not finish.
    let remove = api(&w, Method::DELETE, "/api/admin/telegram", None, &w.admin);
    tokio::pin!(remove);
    let early = tokio::time::timeout(Duration::from_millis(500), &mut remove).await;
    assert!(early.is_err(), "the remove overtook the save's switch");
    drop(hold);
    let removed = remove.await?;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    assert_eq!(removed.body["configured"], false, "{}", removed.body);
    assert_eq!(removed.body["state"], "off", "{}", removed.body);
    assert!(w.h.store.chat_bot().await?.is_none());
    assert!(!w.h.app.telegram_polling(), "the removed bot polls again");
    assert_eq!(
        audits(&w, "telegram", "telegram.bot_set").await?.len(),
        1,
        "the dropped save is audited"
    );
    Ok(())
}

/// A Remove whose request goes away while it waits for the switch lock
/// still stops the bot and is audited.
#[tokio::test]
async fn a_dropped_remove_is_still_audited() -> TestResult {
    let w = world().await?;
    let hold = Arc::clone(&w.seeds).write_owned().await;
    {
        let save = set_bot(&w, TOKEN_A);
        tokio::pin!(save);
        let saved = async {
            while w.h.store.chat_bot().await.ok().flatten().is_none() {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! {
            done = &mut save => return Err(format!("the save ended under a held seed: {:?}", done.map(|r| r.body)).into()),
            () = saved => {}
            () = tokio::time::sleep(Duration::from_secs(10)) => return Err("the bot was never saved".into()),
        }
    }
    {
        // The remove waits behind the save's switch, then its request goes away.
        let remove = api(&w, Method::DELETE, "/api/admin/telegram", None, &w.admin);
        tokio::pin!(remove);
        let early = tokio::time::timeout(Duration::from_millis(500), &mut remove).await;
        assert!(early.is_err(), "the remove overtook the save's switch");
    }
    drop(hold);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while audits(&w, "telegram", "telegram.bot_removed")
        .await?
        .is_empty()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the dropped remove was never audited"
        );
        tokio::task::yield_now().await;
    }
    assert!(w.h.store.chat_bot().await?.is_none());
    assert!(!w.h.app.telegram_polling(), "the removed bot polls");
    Ok(())
}

async fn set_digest(
    w: &World,
    body: Value,
    who: &Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    api(w, Method::PUT, "/api/admin/digest", Some(body), who).await
}

/// The digest is off by default, set by an instance administrator only, its
/// time `HH:MM` and its threshold 1 to 720 hours; a change is audited.
#[tokio::test]
async fn the_digest_is_off_by_default_and_set_by_an_instance_administrator() -> TestResult {
    let w = world().await?;
    let view = api(&w, Method::GET, "/api/admin/digest", None, &w.alice).await?;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert_eq!(
        view.body,
        json!({"enabled": false, "at": "18:00", "park_idle_hours": null})
    );
    let bob = api(&w, Method::GET, "/api/admin/digest", None, &w.bob).await?;
    assert_eq!(bob.status, StatusCode::FORBIDDEN);
    let refused = set_digest(&w, json!({"enabled": true, "at": "18:00"}), &w.bob).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    for bad in [
        json!({"enabled": true, "at": "25:00"}),
        json!({"enabled": true, "at": "18:00", "park_idle_hours": 0}),
        json!({"enabled": true, "at": "18:00", "park_idle_hours": 721}),
    ] {
        let r = set_digest(&w, bad.clone(), &w.alice).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            r.body
        );
    }
    let r = set_digest(
        &w,
        json!({"enabled": true, "at": "7:05", "park_idle_hours": 24}),
        &w.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.body,
        json!({"enabled": true, "at": "07:05", "park_idle_hours": 24})
    );
    let view = api(&w, Method::GET, "/api/admin/digest", None, &w.admin).await?;
    assert_eq!(view.body["at"], "07:05");
    assert_eq!(audits(&w, "digest", "instance.digest.set").await?.len(), 1);
    // Nothing was sent: the first digest is the next 07:05.
    let mine = api(&w, Method::GET, "/api/me/digest", None, &w.alice).await?;
    assert_eq!(mine.body, Value::Null);
    Ok(())
}

/// SPEC-512 DIG-004 to DIG-006: one digest per person with something to report: what waits for
/// them first, what was parked since the last one, how many still work; its texts redacted as
/// the sandbox's transcripts are; in the browser and the linked chat, unless the person turned
/// digests off there. A person with nothing gets none.
#[tokio::test]
async fn a_digest_lists_what_waits_first_redacted_and_reaches_the_chat() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "PAYMENT_KEY", "value": SECRET})),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    link(&w, &w.alice, 100).await?;
    link(&w, &w.bob, 200).await?;
    link(&w, &w.carol, 300).await?;
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    seed_task(&w.h, "t2", "s1", &alice, TaskStatus::Running, None).await?;
    seed_task(&w.h, "t3", "s1", &alice, TaskStatus::Running, None).await?;
    move_to(&w, "t2", TaskStatus::Running, Some(Activity::Working)).await?;
    move_to(&w, "t3", TaskStatus::Parked, Some(Activity::Done)).await?;
    let request = NeedsYouRecord {
        action: format!("Rotate {SECRET} on the server"),
        happened: None,
        problem: None,
        decision: false,
        link: None,
        at: 1,
        cause: None,
        named: Default::default(),
    };
    w.h.store
        .set_task_needs_you(&TaskId::parse("t1")?, Some(&request), w.h.app.now())
        .await?;
    // A secret in a title is redacted too (review round 1).
    w.h.store
        .update_task(
            &TaskId::parse("t2")?,
            &yulius_app::ports::store::TaskFieldUpdate {
                title: Some(format!("Pay with {SECRET}")),
                ..Default::default()
            },
            w.h.app.now(),
        )
        .await?;
    // Saving the setting before the first digest keeps its 24-hour look back (review round 1:
    // a save shortened the window to the save).
    let saved = set_digest(&w, json!({"enabled": true, "at": "18:00"}), &w.alice).await?;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    let before = w.fake.sent_to(100).len();

    assert_eq!(
        w.h.app.send_digests_now(None).await.sent,
        1,
        "bob has nothing"
    );
    let mine = api(&w, Method::GET, "/api/me/digest", None, &w.alice).await?;
    let digest = &mine.body;
    assert_eq!(digest["waiting"][0]["number"], 1, "{digest}");
    let asked = digest["waiting"][0]["detail"].as_str().ok_or("detail")?;
    assert!(
        asked.contains("Rotate") && !asked.contains(SECRET),
        "{asked}"
    );
    assert_eq!(digest["parked"][0]["number"], 3, "{digest}");
    assert_eq!(digest["parked"][0]["detail"], "Finished", "{digest}");
    assert_eq!(digest["working_count"], 1, "{digest}");
    assert_eq!(digest["working"][0]["number"], 2, "{digest}");
    let title = digest["working"][0]["title"].as_str().ok_or("title")?;
    assert!(
        title.starts_with("Pay with") && !title.contains(SECRET),
        "{title}"
    );
    assert_eq!(
        digest["waiting"][0]["link"],
        "https://yulius.test/s/app/t/1"
    );
    let bobs = api(&w, Method::GET, "/api/me/digest", None, &w.bob).await?;
    assert_eq!(bobs.body, Value::Null);

    let sent = w.fake.wait_sent(100, before + 1).await?;
    let text = sent
        .iter()
        .rev()
        .find(|t| t.starts_with("Your Yulius digest"))
        .ok_or("no digest sent")?;
    assert!(text.contains("Waiting for you (1):"), "{text}");
    assert!(text.contains("Parked (1):"), "{text}");
    assert!(text.contains("Still working: 1"), "{text}");
    assert!(!text.contains(SECRET), "redacted: {text}");
    assert_eq!(
        w.fake.sent_to(200).len(),
        1,
        "bob got only his link's answer"
    );

    // Turned off in the chat: the browser still gets it, the chat does not.
    let r = api(
        &w,
        Method::PUT,
        "/api/me/telegram/choices",
        Some(json!({"needs_you": true, "done": false, "failed": false, "account": false, "digest": false})),
        &w.alice,
    )
    .await?;
    assert_eq!(r.body["choices"]["digest"], false, "{}", r.body);
    let told = w.fake.sent_to(100).len();
    assert_eq!(w.h.app.send_digests_now(None).await.sent, 1);
    barrier(&w, "d1").await?;
    assert_eq!(w.fake.sent_to(100).len(), told, "{:?}", w.fake.sent_to(100));
    let again = api(&w, Method::GET, "/api/me/digest", None, &w.alice).await?;
    assert_ne!(again.body["id"], digest["id"]);
    // What the first digest listed as parked is not listed again (review round 1).
    assert_eq!(again.body["parked"], json!([]), "{}", again.body);
    Ok(())
}

/// The digest runs once its time passed, once a day, and never while off.
#[tokio::test]
async fn the_digest_runs_once_a_day_after_its_time() -> TestResult {
    let w = world().await?;
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    move_to(&w, "t1", TaskStatus::Running, Some(Activity::Working)).await?;
    // The harness's day, so the session stays signed in.
    let day = w.h.app.now().0.div_euclid(24 * 60 * MINUTE_MS) * 24 * 60 * MINUTE_MS;
    w.h.clock
        .0
        .store(day + 10 * 60 * MINUTE_MS, Ordering::SeqCst);
    assert_eq!(w.h.app.digest_tick().await.sent, 0, "off by default");
    let r = api(
        &w,
        Method::PUT,
        "/api/admin/digest",
        Some(json!({"enabled": true, "at": "18:00"})),
        &w.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(w.h.app.digest_tick().await.sent, 0, "before its time");
    w.h.clock
        .0
        .store(day + 18 * 60 * MINUTE_MS + 1, Ordering::SeqCst);
    assert_eq!(w.h.app.digest_tick().await.sent, 1);
    assert_eq!(w.h.app.digest_tick().await.sent, 0, "once a day");
    w.h.clock
        .0
        .store(day + 42 * 60 * MINUTE_MS + 1, Ordering::SeqCst);
    assert_eq!(w.h.app.digest_tick().await.sent, 1, "the next day");
    Ok(())
}

/// A request on `task` naming `who` (and `withdrawn`), changed in the store.
async fn ask_named(w: &World, task: &str, who: &[&str], withdrawn: &[&str]) -> TestResult {
    let request = NeedsYouRecord {
        action: "Pick the release date".into(),
        happened: None,
        problem: None,
        decision: true,
        link: None,
        at: 1,
        cause: None,
        named: yulius_app::needs_you::Named {
            who: who.iter().map(|s| (*s).to_owned()).collect(),
            withdrawn: withdrawn.iter().map(|s| (*s).to_owned()).collect(),
        },
    };
    w.h.store
        .set_task_needs_you(&TaskId::parse(task)?, Some(&request), w.h.app.now())
        .await?;
    Ok(())
}

async fn digest_of(w: &World, who: &Creds) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(api(w, Method::GET, "/api/me/digest", None, who).await?.body)
}

/// A digest reaches only people who can see the project: not
/// an owner who lost access, not a named person without access; a named person who can see it
/// gets the request, a withdrawn one does not, and a disabled owner gets nothing while the named
/// person still does.
#[tokio::test]
async fn a_digest_reaches_only_people_who_can_see_the_project() -> TestResult {
    let w = world().await?;
    let alice = user_id(&w, "alice").await?;
    let bob = user_id(&w, "bob").await?;
    // Bob owns a task of s1, which he cannot see; it names carol, who reads s2 only.
    seed_task(&w.h, "t1", "s1", &bob, TaskStatus::Running, None).await?;
    ask_named(&w, "t1", &["carol"], &[]).await?;
    assert_eq!(w.h.app.send_digests_now(None).await.sent, 0);
    assert_eq!(digest_of(&w, &w.bob).await?, Value::Null);
    assert_eq!(digest_of(&w, &w.carol).await?, Value::Null);

    // Alice's task of s2 names carol, who reads s2.
    w.h.github.set_perm("alice", "acme/other", perms("write"));
    w.h.store.delete_task(&TaskId::parse("t1")?).await?;
    seed_task(&w.h, "t2", "s2", &alice, TaskStatus::Running, None).await?;
    ask_named(&w, "t2", &["carol"], &[]).await?;
    assert_eq!(w.h.app.send_digests_now(None).await.sent, 2);
    let carols = digest_of(&w, &w.carol).await?;
    assert_eq!(
        carols["waiting"][0]["detail"], "Pick the release date",
        "{carols}"
    );

    // Withdrawn, carol gets nothing more.
    ask_named(&w, "t2", &[], &["carol"]).await?;
    let before = digest_of(&w, &w.carol).await?;
    assert_eq!(w.h.app.send_digests_now(None).await.sent, 1, "alice only");
    assert_eq!(digest_of(&w, &w.carol).await?["id"], before["id"]);

    // Alice disabled: carol, named again, still gets it; alice gets no new one.
    use yulius_app::ports::store_ext::DigestStore as _;
    ask_named(&w, "t2", &["carol"], &[]).await?;
    let alice_id = yulius_domain::ids::UserId::parse(&alice)?;
    let alices_last =
        w.h.store
            .latest_digest(&alice_id)
            .await?
            .ok_or("alice had one")?
            .id;
    w.h.store
        .update_user_flags(&alice_id, Some(Some(w.h.app.now())), None)
        .await?;
    let carol_before = digest_of(&w, &w.carol).await?;
    assert_eq!(w.h.app.send_digests_now(None).await.sent, 1, "carol only");
    assert_ne!(digest_of(&w, &w.carol).await?["id"], carol_before["id"]);
    assert_eq!(
        w.h.store.latest_digest(&alice_id).await?.ok_or("alice")?.id,
        alices_last
    );
    Ok(())
}

/// The same-day rule reads the run's claim, not where its
/// listing ended: a digest claimed at 23:59:30 whose listing ended after midnight does not cost
/// the next day's.
#[tokio::test]
async fn a_digest_whose_listing_ends_after_midnight_keeps_the_next_days() -> TestResult {
    use yulius_app::ports::store_ext::DigestStore as _;
    let w = world().await?;
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    move_to(&w, "t1", TaskStatus::Running, Some(Activity::Working)).await?;
    let day_ms = 24 * 60 * MINUTE_MS;
    let day = w.h.app.now().0.div_euclid(day_ms) * day_ms;
    w.h.clock
        .0
        .store(day + 10 * 60 * MINUTE_MS, Ordering::SeqCst);
    let r = set_digest(&w, json!({"enabled": true, "at": "23:59"}), &w.alice).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let claimed = day + day_ms - 30_000;
    w.h.clock.0.store(claimed, Ordering::SeqCst);
    assert_eq!(w.h.app.digest_tick().await.sent, 1);
    // Its listing ended at 00:00:05 the next day (a slow park).
    w.h.store
        .set_digest_listed_at(yulius_domain::time::Timestamp(day + day_ms + 5_000))
        .await?;
    w.h.clock.0.store(claimed + day_ms, Ordering::SeqCst);
    assert_eq!(w.h.app.digest_tick().await.sent, 1, "the next day's digest");
    Ok(())
}

/// Turns `who`'s intake on with `sandbox` as its project.
async fn intake_on(
    w: &World,
    who: &Creds,
    sandbox: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    api(
        w,
        Method::PUT,
        "/api/me/telegram/intake",
        Some(json!({"enabled": true, "sandbox_id": sandbox})),
        who,
    )
    .await
}

/// The reply chat `chat` got after `before` messages (10 s at most).
async fn next_reply(
    w: &World,
    chat: i64,
    before: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    Ok(w.fake.wait_sent(chat, before + 1).await?[before].clone())
}

/// A signed-in Claude subscription, so tasks may start.
async fn signed_in_subscription(w: &World) -> TestResult {
    w.h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse("acc-ok")?,
            provider: Provider::Anthropic,
            label: "Team Max".into(),
            credential: Some(
                w.h.app
                    .deps
                    .crypto
                    .seal(b"synthetic-subscription-credential")?,
            ),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: w.h.app.now(),
            updated_at: w.h.app.now(),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    Ok(())
}

async fn tasks_of(w: &World, sandbox: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let r = api(
        w,
        Method::GET,
        &format!("/api/sandboxes/{sandbox}/tasks"),
        None,
        &w.admin,
    )
    .await?;
    Ok(r.body.as_array().cloned().unwrap_or_default())
}

/// Intake is off by default: a linked chat's message creates nothing and its
/// answer says how to turn it on; it is turned on from a browser session with a project the
/// person may work in, audited.
#[tokio::test]
async fn telegram_intake_is_off_by_default_and_set_with_a_project() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    let me = api(&w, Method::GET, "/api/me/telegram", None, &w.alice).await?;
    assert_eq!(
        me.body["intake"],
        json!({"enabled": false, "sandbox_id": null}),
        "{}",
        me.body
    );
    let before = w.fake.sent_to(100).len();
    w.fake.message(100, true, "Make the header blue");
    let reply = next_reply(&w, 100, before).await?;
    assert!(reply.contains("turn on Telegram intake"), "{reply}");
    assert!(tasks_of(&w, "s1").await?.is_empty(), "nothing started");

    let none = api(
        &w,
        Method::PUT,
        "/api/me/telegram/intake",
        Some(json!({"enabled": true, "sandbox_id": null})),
        &w.alice,
    )
    .await?;
    assert_eq!(
        none.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        none.body
    );
    let elsewhere = intake_on(&w, &w.alice, "s2").await?;
    assert!(
        matches!(
            elsewhere.status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ),
        "alice cannot work in s2: {}",
        elsewhere.body
    );
    // No API token turns intake on: only a browser session (review round 1).
    let user = w.h.store.user_by_login("alice").await?.ok_or("alice")?;
    let token =
        w.h.app
            .issue_api_token(&user, "cli", yulius_domain::permission::TokenScope::Operate)
            .await?;
    let bearer = Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    };
    let by_token = intake_on(&w, &bearer, "s1").await?;
    assert_eq!(by_token.status, StatusCode::FORBIDDEN, "{}", by_token.body);
    let alice_id = user_id(&w, "alice").await?;
    assert!(audits(&w, &alice_id, "telegram.intake").await?.is_empty());
    let me = api(&w, Method::GET, "/api/me/telegram", None, &w.alice).await?;
    assert_eq!(me.body["intake"]["enabled"], false, "nothing stored");
    let on = intake_on(&w, &w.alice, "s1").await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(
        on.body["intake"],
        json!({"enabled": true, "sandbox_id": "s1"})
    );
    let alice = user_id(&w, "alice").await?;
    assert_eq!(audits(&w, &alice, "telegram.intake").await?, ["on s1"]);
    Ok(())
}

/// With intake on, a message starts a task in the default project as
/// the person, labelled from Telegram; a photo with a caption becomes its attachment; `/task`
/// names another project; an unknown project is refused.
#[tokio::test]
async fn with_intake_a_message_starts_a_task_and_a_photo_is_its_attachment() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    signed_in_subscription(&w).await?;
    crate::support::codex::connect_codex(&w.h.app).await?;
    // The project's folder, where its attachments live.
    std::fs::create_dir_all(w.h.app.config.data_dir.join("sandboxes/s1"))?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let before = w.fake.sent_to(100).len();
    w.fake.message(100, true, "Fix the header");
    let reply = next_reply(&w, 100, before).await?;
    assert!(reply.starts_with("Started #"), "{reply}");
    assert!(reply.contains("https://yulius.test/s/app/t/"), "{reply}");
    let tasks = tasks_of(&w, "s1").await?;
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0]["origin"], "telegram");
    assert_eq!(tasks[0]["owner"]["login"], "alice", "{}", tasks[0]);
    // A message names no agent, so the task runs Claude Code, chosen and not
    // drawn, even with Codex usable too.
    assert_eq!(
        (&tasks[0]["harness"], &tasks[0]["harness_choice"]),
        (&json!("claude"), &json!("chosen")),
        "{}",
        tasks[0]
    );

    let before = w.fake.sent_to(100).len();
    w.fake.raw(
        100,
        json!({"caption": "What is wrong here?", "photo": [
            {"file_id": "ph1", "file_unique_id": "u1", "file_size": 3}
        ]}),
    );
    let reply = next_reply(&w, 100, before).await?;
    assert!(reply.starts_with("Started #"), "{reply}");
    let mut prompts = Vec::new();
    for task in tasks_of(&w, "s1").await? {
        let id = TaskId::parse(task["id"].as_str().unwrap_or_default())?;
        prompts.extend(w.h.store.task(&id).await?.map(|t| t.prompt));
    }
    let prompt = prompts
        .iter()
        .find(|p| p.starts_with("What is wrong here?"))
        .ok_or("photo task")?;
    assert!(prompt.contains("Attached from Telegram:\n- "), "{prompt}");
    let path = prompt.rsplit("- ").next().unwrap_or_default().trim();
    assert_eq!(std::fs::read(path)?, b"png", "the photo's bytes at {path}");

    let before = w.fake.sent_to(100).len();
    w.fake.message(100, true, "/task nowhere Do it");
    let reply = next_reply(&w, 100, before).await?;
    assert!(
        reply.starts_with("No project you may work in is called nowhere"),
        "{reply}"
    );
    let before = w.fake.sent_to(100).len();
    w.fake.message(100, true, "/task app Tidy the README");
    let reply = next_reply(&w, 100, before).await?;
    assert!(reply.starts_with("Started #"), "{reply}");
    assert_eq!(tasks_of(&w, "s1").await?.len(), 3);
    // The full repository name, owner/name, names it too (part 5: owner and name are stored
    // apart).
    let before = w.fake.sent_to(100).len();
    w.fake
        .message(100, true, "/task acme/app Tidy the CHANGELOG");
    let reply = next_reply(&w, 100, before).await?;
    assert!(reply.starts_with("Started #"), "{reply}");
    assert_eq!(tasks_of(&w, "s1").await?.len(), 4);
    Ok(())
}

/// A reply to a notification reaches its task as the person's
/// `task_message`; a teammate's reply to it is refused by the owner rule and changes nothing;
/// `/status` lists what waits for the person, redacted.
#[tokio::test]
async fn a_reply_reaches_its_task_under_the_owner_rule() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "PAYMENT_KEY", "value": SECRET})),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    // Carol may write in s1 too, so she sees alice's task and its notice.
    w.h.github.set_perm("carol", "acme/app", perms("write"));
    link(&w, &w.alice, 100).await?;
    link(&w, &w.carol, 300).await?;
    intake_on(&w, &w.alice, "s1").await?;
    intake_on(&w, &w.carol, "s1").await?;
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    ask(&w, "t1", &format!("Checkout fails with {SECRET}")).await?;
    // The request's action carries the secret too, which `/status` shows (review round 1).
    let id = TaskId::parse("t1")?;
    let mut asked = w.h.store.task_needs_you(&id).await?.ok_or("asked")?;
    asked.action = format!("Rotate {SECRET} first");
    w.h.store
        .set_task_needs_you(&id, Some(&asked), w.h.app.now())
        .await?;
    w.fake.wait_sent(300, 2).await?;
    let to_alice = w.fake.sent_id(100, "Needs you").ok_or("alice's notice")?;
    let to_carol = w.fake.sent_id(300, "Needs you").ok_or("carol's notice")?;

    // Once the request ended, carol's reply is no answer: the owner rule refuses it.
    let asked = w.h.store.task_needs_you(&id).await?;
    w.h.store
        .set_task_needs_you(&id, None, w.h.app.now())
        .await?;
    let before = w.fake.sent_to(300).len();
    w.fake.raw(
        300,
        json!({"text": "Use the blue one", "reply_to_message": {"message_id": to_carol}}),
    );
    let reply = next_reply(&w, 300, before).await?;
    assert!(reply.starts_with("Not sent to #"), "{reply}");
    assert!(reply.contains("alice"), "it says whose task it is: {reply}");
    assert!(audits(&w, "t1", "task.messaged").await?.is_empty());
    w.h.store
        .set_task_needs_you(&id, asked.as_ref(), w.h.app.now())
        .await?;

    // A new request replaces the one carol's notice told of: her reply to that notice answers
    // nothing later (review round 2: the notice keeps its request's time).
    let mut newer = asked.clone().ok_or("asked")?;
    newer.at += 1;
    w.h.store
        .set_task_needs_you(&id, Some(&newer), w.h.app.now())
        .await?;
    let before = w.fake.sent_to(300).len();
    w.fake.raw(
        300,
        json!({"text": "Friday", "reply_to_message": {"message_id": to_carol}}),
    );
    let reply = next_reply(&w, 300, before).await?;
    assert_eq!(
        reply,
        "Not sent to #1: the question you answered on task #1 was replaced by a new one: read it \
         and answer again"
    );
    assert!(
        w.h.store.task_needs_you(&id).await?.is_some(),
        "the new request stays"
    );

    // Alice's reply is her message to her task: the owner rule lets it through, and here (no
    // agent runs in this harness) `task_message` answers that the task has no live session; a
    // live or parked session takes it as `task_message` does (parking and task_message tests).
    let before = w.fake.sent_to(100).len();
    w.fake.raw(
        100,
        json!({"text": "Use the blue one", "reply_to_message": {"message_id": to_alice}}),
    );
    let reply = next_reply(&w, 100, before).await?;
    assert_eq!(reply, "Not sent to #1: task #1 has no live session");
    assert!(
        tasks_of(&w, "s1").await?.len() == 1,
        "a reply starts no task"
    );

    // /status lists the task that waits, redacted.
    let before = w.fake.sent_to(100).len();
    w.fake.message(100, true, "/status");
    let status = next_reply(&w, 100, before).await?;
    assert!(status.starts_with("Waiting for you (1):"), "{status}");
    assert!(
        status.contains("Rotate [secret:PAYMENT_KEY] first"),
        "{status}"
    );
    assert!(!status.contains(SECRET), "redacted: {status}");
    assert!(status.contains("https://yulius.test/s/app/t/"), "{status}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_telegram_reply_keeps_the_selected_park() -> TestResult {
    use yulius_app::ports::notify::SentAbout;
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    support::exhausted_subscription(&w.h).await?;
    let alice = user_id(&w, "alice").await?;
    let target = seed_task(
        &w.h,
        "t-telegram-dispatch-target",
        "s1",
        &alice,
        TaskStatus::Parked,
        None,
    )
    .await?;
    w.h.store
        .record_sent(
            "100",
            "9001",
            &SentAbout {
                task: target.id.clone(),
                asked_at: None,
                command_run: None,
            },
            w.h.app.now(),
        )
        .await?;
    support::replace_park_at(
        &w.h,
        yulius_app::race::Point::SelectedTaskDispatched,
        &target.id,
        &target,
    )
    .await?;
    let before = w.fake.sent_to(100).len();
    w.fake.raw(
        100,
        json!({"text":"For the parked turn","reply_to_message":{"message_id":9001}}),
    );
    // The task can also send its Needs you notice before the reply acknowledgement.
    let answer = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let sent = w.fake.sent_changed.notified();
            if let Some(answer) = w
                .fake
                .sent_to(100)
                .into_iter()
                .skip(before)
                .find(|s| s.starts_with("Sent to #") || s.starts_with("Not sent to #"))
            {
                return answer;
            }
            sent.await;
        }
    })
    .await?;
    assert!(
        answer.starts_with("Not sent to #") && answer.contains("changed"),
        "{answer}"
    );
    support::assert_replacement_parked(&w.h, &target.id).await
}

/// A reply is delivered: to a task that has not started it
/// joins the request as the person's message, labelled from Telegram and audited; a reply to
/// the bot's own answer about a task reaches that task too; a reply to a message about no task
/// starts nothing.
#[tokio::test]
async fn a_reply_is_delivered_and_a_reply_never_starts_a_task() -> TestResult {
    use yulius_app::ports::notify::SentAbout;
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let alice = user_id(&w, "alice").await?;
    let task = seed_task(&w.h, "t-q", "s1", &alice, TaskStatus::Queued, None).await?;
    w.h.store
        .record_sent(
            "100",
            "9001",
            &SentAbout {
                task: task.id.clone(),
                asked_at: None,
                command_run: None,
            },
            w.h.app.now(),
        )
        .await?;
    let before = w.fake.sent_to(100).len();
    w.fake.raw(
        100,
        json!({"text": "Use the DO budget", "reply_to_message": {"message_id": 9001}}),
    );
    let reply = next_reply(&w, 100, before).await?;
    assert_eq!(reply, format!("Sent to #{}.", task.number));
    let request = w.h.store.task(&task.id).await?.ok_or("task")?.prompt;
    assert!(request.ends_with("Use the DO budget"), "{request}");
    let messaged = audits(&w, "t-q", "task.messaged").await?;
    assert!(
        messaged.iter().any(|d| d.contains("telegram")),
        "audited as from Telegram: {messaged:?}"
    );

    // A reply to the bot's own answer reaches the same task.
    let answer_id = w.fake.sent_id(100, "Sent to #").ok_or("the answer")?;
    let before = w.fake.sent_to(100).len();
    w.fake.raw(
        100,
        json!({"text": "And the staging budget", "reply_to_message": {"message_id": answer_id}}),
    );
    let reply = next_reply(&w, 100, before).await?;
    assert_eq!(reply, format!("Sent to #{}.", task.number));
    let request = w.h.store.task(&task.id).await?.ok_or("task")?.prompt;
    assert!(request.ends_with("And the staging budget"), "{request}");

    // A reply to a message about no task starts nothing.
    let before = w.fake.sent_to(100).len();
    w.fake.raw(
        100,
        json!({"text": "also fix the footer", "reply_to_message": {"message_id": 1}}),
    );
    let reply = next_reply(&w, 100, before).await?;
    assert!(
        reply.starts_with("That message is about no task"),
        "{reply}"
    );
    assert_eq!(tasks_of(&w, "s1").await?.len(), 1, "no task started");
    Ok(())
}

/// The hourly bound of tools applies: the 21st task in an
/// hour is refused and creates nothing; at most 10 files, each at most 20 MB, are taken; a
/// project the person may not work in is no project to them.
#[tokio::test]
async fn telegram_tasks_keep_the_hourly_bound_and_the_file_bounds() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    signed_in_subscription(&w).await?;
    std::fs::create_dir_all(w.h.app.config.data_dir.join("sandboxes/s1"))?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let ask = |text: String| {
        let w = &w;
        async move {
            let before = w.fake.sent_to(100).len();
            w.fake.message(100, true, &text);
            next_reply(w, 100, before).await
        }
    };
    let before = w.fake.sent_to(100).len();
    let many: Vec<Value> = (0..11)
        .map(|n| json!({"file_id": format!("d{n}"), "file_unique_id": format!("u{n}"), "file_name": format!("f{n}.txt")}))
        .collect();
    let album: Vec<Value> = many
        .iter()
        .enumerate()
        .map(|(n, doc)| {
            let mut body = json!({"document": doc, "media_group_id": "g11"});
            if n == 0 {
                body["caption"] = json!("Read these");
            }
            body
        })
        .collect();
    w.fake.raw_batch(100, &album);
    let reply = next_reply(&w, 100, before).await?;
    assert!(
        reply.starts_with("At most 10 files start a task"),
        "an album of 11 is one message, refused: {reply}"
    );
    let before = w.fake.sent_to(100).len();
    w.fake.raw(
        100,
        json!({"caption": "Big", "document": {"file_id": "big", "file_unique_id": "ub", "file_name": "big.bin", "file_size": 30_000_000}}),
    );
    let reply = next_reply(&w, 100, before).await?;
    assert!(reply.contains("over Telegram's 20 MB"), "{reply}");
    for n in 0..20 {
        let reply = ask(format!("Task {n}")).await?;
        assert!(reply.starts_with("Started #"), "{n}: {reply}");
    }
    let reply = ask("Task 21".into()).await?;
    assert!(
        reply.starts_with("Not started: tools may create at most 20 tasks an hour"),
        "{reply}"
    );
    assert_eq!(tasks_of(&w, "s1").await?.len(), 20);

    let reply = ask("/task other Do it".into()).await?;
    assert!(
        reply.starts_with("No project you may work in is called other"),
        "alice may not work in acme/other: {reply}"
    );
    assert_eq!(tasks_of(&w, "s1").await?.len(), 20, "nothing more started");
    Ok(())
}

/// The task numbers and prompts of project `sandbox`, by number.
async fn prompts_by_number(
    w: &World,
    sandbox: &str,
) -> Result<Vec<(u64, String)>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for task in tasks_of(w, sandbox).await? {
        let id = TaskId::parse(task["id"].as_str().unwrap_or_default())?;
        let prompt =
            w.h.store
                .task(&id)
                .await?
                .map(|t| t.prompt)
                .unwrap_or_default();
        out.push((task["number"].as_u64().unwrap_or_default(), prompt));
    }
    out.sort();
    Ok(out)
}

/// SPEC-512 TGI-002, TGI-003 (review of task #538): a whole poll answer from one chat (20
/// messages, as after a restart) is handled, each message in its turn: while the first (a
/// photo) is still being fetched, the 19 behind it wait, and every one starts its task in the
/// order it was sent.
#[tokio::test]
async fn a_chats_whole_poll_answer_is_handled_in_order() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    signed_in_subscription(&w).await?;
    std::fs::create_dir_all(w.h.app.config.data_dir.join("sandboxes/s1"))?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let before = w.fake.sent_to(100).len();
    w.fake.hold_files.store(true, Ordering::SeqCst);
    let mut bodies = vec![json!({"caption": "First", "photo": [
        {"file_id": "ph1", "file_unique_id": "u1", "file_size": 3}
    ]})];
    bodies.extend((0..19).map(|n| json!({"text": format!("Task {n}")})));
    w.fake.raw_batch(100, &bodies);
    w.fake.wait_file_asked().await?;
    w.fake.wait_updates_taken().await?;
    w.fake.release_files();
    let sent = w.fake.wait_sent(100, before + 20).await?;
    for reply in &sent[before..] {
        assert!(reply.starts_with("Started #"), "{reply}");
    }
    let prompts: Vec<String> = prompts_by_number(&w, "s1")
        .await?
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    assert_eq!(prompts.len(), 20, "{prompts:?}");
    assert!(prompts[0].starts_with("First"), "{prompts:?}");
    let expected: Vec<String> = (0..19).map(|n| format!("Task {n}")).collect();
    assert_eq!(
        prompts[1..],
        expected[..],
        "one task per message, in the order sent"
    );
    Ok(())
}

/// SPEC-512 TGI-002, TGI-003 (review of task #538): while a chat's message is being handled, 20
/// more wait their turn; one past that is not dropped silently: the chat is told some were not
/// handled, and the waiting ones still run.
#[tokio::test]
async fn a_message_past_the_chats_waiting_bound_is_answered() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    signed_in_subscription(&w).await?;
    std::fs::create_dir_all(w.h.app.config.data_dir.join("sandboxes/s1"))?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let before = w.fake.sent_to(100).len();
    w.fake.hold_files.store(true, Ordering::SeqCst);
    w.fake.raw(
        100,
        json!({"caption": "First", "photo": [
            {"file_id": "ph1", "file_unique_id": "u1", "file_size": 3}
        ]}),
    );
    w.fake.wait_file_asked().await?;
    // Two poll answers (20, then 2): 20 wait behind the photo, the last two do not fit.
    let bodies: Vec<Value> = (0..22)
        .map(|n| json!({"text": format!("Task {n}")}))
        .collect();
    w.fake.raw_batch(100, &bodies);
    let told = next_reply(&w, 100, before).await?;
    assert!(
        told.starts_with("Too many messages at once"),
        "told while the photo is still held: {told}"
    );
    w.fake.release_files();
    // The photo's task and 19 more fill the hourly bound of 20; the 20th waiting is refused by it.
    let sent = w.fake.wait_sent(100, before + 22).await?;
    let started = sent[before..]
        .iter()
        .filter(|r| r.starts_with("Started #"))
        .count();
    assert_eq!(started, 20, "{:?}", &sent[before..]);
    assert!(
        sent[before..]
            .iter()
            .any(|r| r.starts_with("Not started: tools may create at most 20 tasks an hour")),
        "{:?}",
        &sent[before..]
    );
    let prompts: Vec<String> = prompts_by_number(&w, "s1")
        .await?
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    assert!(prompts[0].starts_with("First"), "{prompts:?}");
    let expected: Vec<String> = (0..19).map(|n| format!("Task {n}")).collect();
    assert_eq!(prompts[1..], expected[..], "the waiting ones ran in order");
    Ok(())
}

/// SPEC-512 TGI-004 (review of task #538): `/start` without a code in a chat whose intake is on
/// gets the intake help, not "turn on Telegram intake".
#[tokio::test]
async fn a_start_with_intake_on_gets_the_intake_help() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let before = w.fake.sent_to(100).len();
    w.fake.message(100, true, "/start");
    let reply = next_reply(&w, 100, before).await?;
    assert_eq!(reply, yulius_app::telegram_intake::INTAKE_HELP);
    Ok(())
}

/// Sends `chat` a photo that is held while `texts` wait behind it in the chat's lane; returns the
/// number of messages the chat had before.
async fn photo_held_with_waiting(
    w: &World,
    chat: i64,
    texts: &[&str],
) -> Result<usize, Box<dyn std::error::Error>> {
    let before = w.fake.sent_to(chat).len();
    w.fake.files_asked.store(0, Ordering::SeqCst);
    w.fake.hold_files.store(true, Ordering::SeqCst);
    let mut bodies = vec![json!({"caption": "First", "photo": [
        {"file_id": "ph1", "file_unique_id": "u1", "file_size": 3}
    ]})];
    bodies.extend(texts.iter().map(|t| json!({ "text": t })));
    w.fake.raw_batch(chat, &bodies);
    w.fake.wait_file_asked().await?;
    w.fake.wait_updates_taken().await?;
    Ok(before)
}

/// SPEC-512 TGI-001 (review round 2 of task #538): turning intake off, or `/stop`, ends what
/// waits in the chat's lane too: a message queued behind a slow one starts nothing and gets
/// the answer it would get now (the help with intake off, the unlinked reply after `/stop`).
#[tokio::test]
async fn intake_off_or_stop_ends_what_waits_in_the_lane() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    signed_in_subscription(&w).await?;
    std::fs::create_dir_all(w.h.app.config.data_dir.join("sandboxes/s1"))?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;

    // Intake off while one waits.
    let before = photo_held_with_waiting(&w, 100, &["Waiting 1"]).await?;
    let off = api(
        &w,
        Method::PUT,
        "/api/me/telegram/intake",
        Some(json!({"enabled": false, "sandbox_id": null})),
        &w.alice,
    )
    .await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    w.fake.release_files();
    let sent = w.fake.wait_sent(100, before + 2).await?;
    assert!(
        sent[before].starts_with("Started #"),
        "the running one ends: {sent:?}"
    );
    assert!(
        sent[before + 1].contains("turn on Telegram intake"),
        "the waiting one is answered as intake off: {sent:?}"
    );

    // `/stop` while one waits (another chat, so its quiet reply is due).
    link(&w, &w.alice, 101).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let before = photo_held_with_waiting(&w, 101, &["Waiting 2"]).await?;
    w.fake.message(101, true, "/stop");
    let reply = next_reply(&w, 101, before).await?;
    assert!(reply.starts_with("Unlinked"), "{reply}");
    w.fake.release_files();
    let sent = w.fake.wait_sent(101, before + 3).await?;
    assert!(sent[before + 1].starts_with("Started #"), "{sent:?}");
    assert_eq!(sent[before + 2], UNLINKED_REPLY, "{sent:?}");

    let prompts: Vec<String> = prompts_by_number(&w, "s1")
        .await?
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    assert_eq!(
        prompts.len(),
        2,
        "only the running ones started: {prompts:?}"
    );
    assert!(
        prompts.iter().all(|p| p.starts_with("First")),
        "{prompts:?}"
    );
    Ok(())
}

/// SPEC-512 TGI-001 (review round 3 of task #538): a message that waited while the chat was one
/// person's never runs as the person the chat is linked to by its turn.
#[tokio::test]
async fn what_waits_never_runs_as_the_chats_next_person() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    signed_in_subscription(&w).await?;
    std::fs::create_dir_all(w.h.app.config.data_dir.join("sandboxes/s1"))?;
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    photo_held_with_waiting(&w, 100, &["Waiting as alice"]).await?;
    link(&w, &w.admin, 100).await?;
    let on = intake_on(&w, &w.admin, "s1").await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    let before = w.fake.sent_to(100).len();
    w.fake.release_files();
    let reply = next_reply(&w, 100, before).await?;
    assert!(
        reply.starts_with("Started #"),
        "alice's running one ends: {reply}"
    );
    // Sent after: it runs once the waiting one had its turn.
    w.fake.message(100, true, "Barrier");
    let sent = w.fake.wait_sent(100, before + 2).await?;
    assert!(sent[before + 1].starts_with("Started #"), "{sent:?}");
    let mut owners = Vec::new();
    for task in tasks_of(&w, "s1").await? {
        let id = TaskId::parse(task["id"].as_str().unwrap_or_default())?;
        let prompt =
            w.h.store
                .task(&id)
                .await?
                .map(|t| t.prompt)
                .unwrap_or_default();
        let owner = task["owner"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        owners.push((prompt.lines().next().unwrap_or_default().to_owned(), owner));
    }
    owners.sort();
    assert_eq!(
        owners.len(),
        2,
        "the waiting one started nothing: {owners:?}"
    );
    assert_eq!(owners[1].0, "First", "{owners:?}");
    assert_eq!(owners[1].1, "alice", "{owners:?}");
    assert_eq!(owners[0].0, "Barrier", "{owners:?}");
    assert_ne!(owners[0].1, "alice", "{owners:?}");
    Ok(())
}

/// Chat notifications redact generic credential shapes (such as
/// `ghp_...`) in `needs_you` text in addition to sandbox vault values.
#[tokio::test]
async fn telegram_needs_you_notification_redacts_generic_credential_shapes() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    link(&w, &w.alice, 100).await?;
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;

    let id = TaskId::parse("t1")?;
    let fake_token = concat!("ghp_", "1234567890abcdef1234567890abcdef1234");
    w.h.store
        .set_task_needs_you(
            &id,
            Some(&yulius_app::needs_you::NeedsYouRecord {
                action: format!("Rotate {fake_token} in Settings"),
                happened: Some(format!("Auth rejected {fake_token}")),
                problem: None,
                decision: false,
                link: None,
                at: w.h.app.now().0,
                cause: None,
                named: yulius_app::needs_you::Named::default(),
            }),
            w.h.app.now(),
        )
        .await?;
    w.h.app.publish_task(&id).await;
    // `sent[0]` is the "Linked: ..." confirmation from `link(...)`; `sent[1]` is the Needs You notice.
    let sent = w.fake.wait_sent(100, 2).await?;
    let msg = &sent[1];
    assert!(
        msg.contains("Rotate") && !msg.contains(fake_token),
        "generic credential token must be redacted in Telegram notification: {msg}"
    );
    Ok(())
}

/// A reply `reject` or `approve` to a brokered run's Needs you notice answers
/// that run as the person who replied, who manages the project's secrets; a reply to the notice
/// of a run already answered answers nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_to_a_runs_notice_answers_that_run() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    w.h.github.set_perm("alice", "acme/app", perms("admin"));
    link(&w, &w.alice, 100).await?;
    intake_on(&w, &w.alice, "s1").await?;
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "DEPLOY_KEY", "value": "deploy-key-synthetic-0042"})),
        &w.alice,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/commands",
        Some(json!({"name": "deploy", "binary": "/usr/bin/true", "secret": "DEPLOY_KEY"})),
        &w.alice,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    support::seed_live_attempt(&w.h, "t1", "tok-t1-telegram-0123456789").await?;
    let asked = support::mcp_tool(
        &w.h,
        "tok-t1-telegram-0123456789",
        "command_run",
        json!({"name": "deploy", "args": ["status"]}),
    );
    let reply = async {
        w.fake.wait_sent(100, 1).await?;
        let notice = loop {
            if let Some(id) = w.fake.sent_id(100, "Needs you") {
                break id;
            }
            w.fake.wait_sent(100, w.fake.sent_to(100).len() + 1).await?;
        };
        let text = w.fake.sent_to(100).last().cloned().unwrap_or_default();
        assert!(
            text.contains("/usr/bin/true --key-file") || text.contains("/usr/bin/true status"),
            "{text}"
        );
        let before = w.fake.sent_to(100).len();
        w.fake.raw(
            100,
            json!({"text": "Reject", "reply_to_message": {"message_id": notice}}),
        );
        let answer = next_reply(&w, 100, before).await?;
        assert_eq!(answer, "Rejected: task #1's run of deploy runs nothing.");
        // The same notice answers nothing more, and is no message to the task either.
        let before = w.fake.sent_to(100).len();
        w.fake.raw(
            100,
            json!({"text": "approve", "reply_to_message": {"message_id": notice}}),
        );
        let answer = next_reply(&w, 100, before).await?;
        assert_eq!(answer, "Not answered: this run is rejected already");
        Ok::<_, Box<dyn std::error::Error>>(notice)
    };
    let (ran, replied) = tokio::join!(asked, reply);
    let first_notice = replied?;
    let ran = support::tool_text(&ran?);
    assert!(ran.to_string().contains("rejected"), "{ran}");

    // A newer run of the task, asked while the harness's clock stands still: a reply to the first
    // run's notice never answers it, as each notice names its own run.
    let asked = support::mcp_tool(
        &w.h,
        "tok-t1-telegram-0123456789",
        "command_run",
        json!({"name": "deploy", "args": ["status"]}),
    );
    let reply = async {
        let tid = TaskId::parse("t1")?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while w.h.store.task_needs_you(&tid).await?.is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the second run never waited"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let before = w.fake.sent_to(100).len();
        w.fake.raw(
            100,
            json!({"text": "approve", "reply_to_message": {"message_id": first_notice}}),
        );
        let answer = next_reply(&w, 100, before).await?;
        assert_eq!(answer, "Not answered: this run is rejected already");
        assert!(
            w.h.store.task_needs_you(&tid).await?.is_some(),
            "the newer run still waits"
        );
        // Its own notice answers it.
        let notice = loop {
            if let Some(id) = w
                .fake
                .sent_id(100, "Needs you")
                .filter(|id| *id != first_notice)
            {
                break id;
            }
            w.fake.wait_sent(100, w.fake.sent_to(100).len() + 1).await?;
        };
        let before = w.fake.sent_to(100).len();
        w.fake.raw(
            100,
            json!({"text": "reject", "reply_to_message": {"message_id": notice}}),
        );
        assert_eq!(
            next_reply(&w, 100, before).await?,
            "Rejected: task #1's run of deploy runs nothing."
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (ran, replied) = tokio::join!(asked, reply);
    replied?;
    assert!(support::tool_text(&ran?).to_string().contains("rejected"));
    let details = w.h.store.audit_details("s1", "command.reject", 5).await?;
    assert_eq!(details.len(), 2, "{details:?}");
    assert!(details[0].contains("telegram"), "{details:?}");
    Ok(())
}

/// SPEC-626 BRK-015 (review round 1 of slice 3): a reply to a run's notice is an answer or
/// nothing; a reader may not answer; a line the notice could not show whole is approved on the
/// Secrets page only (a reply rejects it); an approval by reply runs the command.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runs_notice_takes_only_an_answer_its_reader_may_give() -> TestResult {
    let w = world().await?;
    set_bot(&w, TOKEN_A).await?;
    w.h.github.set_perm("alice", "acme/app", perms("admin"));
    // Bob reads this project and writes another, where his Telegram intake is on.
    w.h.github.set_perm("bob", "acme/app", perms("read"));
    w.h.github.set_perm("bob", "acme/other", perms("write"));
    link(&w, &w.alice, 100).await?;
    link(&w, &w.bob, 200).await?;
    let on = intake_on(&w, &w.alice, "s1").await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    let on = intake_on(&w, &w.bob, "s2").await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "DEPLOY_KEY", "value": "deploy-key-synthetic-0042"})),
        &w.alice,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/commands",
        Some(json!({"name": "deploy", "binary": "/usr/bin/true", "secret": "DEPLOY_KEY"})),
        &w.alice,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let alice = user_id(&w, "alice").await?;
    seed_task(&w.h, "t1", "s1", &alice, TaskStatus::Running, None).await?;
    support::seed_live_attempt(&w.h, "t1", "tok-t1-telegram-0123456789").await?;
    // A line longer than a notice shows.
    let long = "x".repeat(400);
    let asked = support::mcp_tool(
        &w.h,
        "tok-t1-telegram-0123456789",
        "command_run",
        json!({"name": "deploy", "args": [long]}),
    );
    let replies = async {
        let to_alice = notice_to(&w, 100).await?;
        let to_bob = notice_to(&w, 200).await?;
        assert!(
            w.fake
                .sent_to(100)
                .iter()
                .any(|t| t.contains("Approve it on the Secrets page")),
            "{:?}",
            w.fake.sent_to(100)
        );
        assert_eq!(
            reply_to(&w, 200, to_bob, "approve").await?,
            "Not answered: only people who manage this project's secrets answer its command runs"
        );
        assert_eq!(
            reply_to(&w, 100, to_alice, "approve").await?,
            "Not answered: this notice could not show the whole command line: approve it on the \
             Secrets page, where it is whole"
        );
        assert!(
            reply_to(&w, 100, to_alice, "ok")
                .await?
                .contains("reply approve or reject")
        );
        // An empty reply too (review of slice 4): it is no message to the task.
        assert!(
            reply_to(&w, 100, to_alice, "")
                .await?
                .contains("reply approve or reject")
        );
        assert_eq!(
            reply_to(&w, 100, to_alice, "reject").await?,
            "Rejected: task #1's run of deploy runs nothing."
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (ran, replied) = tokio::join!(asked, replies);
    replied?;
    assert!(support::tool_text(&ran?).to_string().contains("rejected"));
    assert!(
        audits(&w, "t1", "task.messaged").await?.is_empty(),
        "no reply reached the task"
    );

    // A line holding a backtick, which a notice drops (review round 2 of slice 3): approved on
    // the Secrets page only, as a cut line is.
    let asked = support::mcp_tool(
        &w.h,
        "tok-t1-telegram-0123456789",
        "command_run",
        json!({"name": "deploy", "args": ["--ref=v1.2`.0"]}),
    );
    let replies = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let tid = TaskId::parse("t1")?;
        while w.h.store.task_needs_you(&tid).await?.is_none() {
            assert!(tokio::time::Instant::now() < deadline, "it never waited");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let notice = loop {
            let id = notice_to(&w, 100).await?;
            if w.fake
                .sent_to(100)
                .iter()
                .rev()
                .find(|t| t.starts_with("Needs you"))
                .is_some_and(|t| t.contains("v1.2"))
            {
                break id;
            }
            w.fake.wait_sent(100, w.fake.sent_to(100).len() + 1).await?;
        };
        assert!(
            reply_to(&w, 100, notice, "approve")
                .await?
                .contains("could not show the whole command line")
        );
        assert_eq!(
            reply_to(&w, 100, notice, "reject").await?,
            "Rejected: task #1's run of deploy runs nothing."
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (ran, replied) = tokio::join!(asked, replies);
    replied?;
    assert!(support::tool_text(&ran?).to_string().contains("rejected"));

    // A short line, approved by reply: it runs (this harness has no process host, so it fails).
    let asked = support::mcp_tool(
        &w.h,
        "tok-t1-telegram-0123456789",
        "command_run",
        json!({"name": "deploy", "args": ["status"]}),
    );
    let replies = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let tid = TaskId::parse("t1")?;
        while w.h.store.task_needs_you(&tid).await?.is_none() {
            assert!(tokio::time::Instant::now() < deadline, "it never waited");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let notice = loop {
            let id = notice_to(&w, 100).await?;
            if w.fake
                .sent_to(100)
                .iter()
                .rev()
                .find(|t| t.starts_with("Needs you"))
                .is_some_and(|t| t.contains("status"))
            {
                break id;
            }
            w.fake.wait_sent(100, w.fake.sent_to(100).len() + 1).await?;
        };
        assert_eq!(
            reply_to(&w, 100, notice, "Approve").await?,
            "Approved: task #1's run of deploy starts now."
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (ran, replied) = tokio::join!(asked, replies);
    replied?;
    let ran = support::tool_text(&ran?).to_string();
    assert!(ran.contains("failed") || ran.contains("done"), "{ran}");
    let approvals = w.h.store.audit_details("s1", "command.approve", 5).await?;
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert!(approvals[0].contains("telegram"), "{approvals:?}");
    Ok(())
}

/// The newest Needs you notice chat `chat` got (waiting for one).
async fn notice_to(w: &World, chat: i64) -> Result<usize, Box<dyn std::error::Error>> {
    loop {
        if let Some(id) = w.fake.sent_id(chat, "Needs you") {
            return Ok(id);
        }
        w.fake
            .wait_sent(chat, w.fake.sent_to(chat).len() + 1)
            .await?;
    }
}

/// Replies `text` to message `notice` of chat `chat`; the bot's answer.
async fn reply_to(
    w: &World,
    chat: i64,
    notice: usize,
    text: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let before = w.fake.sent_to(chat).len();
    w.fake.raw(
        chat,
        json!({"text": text, "reply_to_message": {"message_id": notice}}),
    );
    next_reply(w, chat, before).await
}
