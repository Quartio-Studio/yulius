//! The personal Telegram channel (SPEC-498) against a fake Bot API on 127.0.0.1: the bot set and
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
        "sendMessage" => {
            let chat = body["chat_id"].as_i64().unwrap_or_default().to_string();
            let text = body["text"].as_str().unwrap_or_default().to_owned();
            if let Ok(mut sent) = fake.sent.lock() {
                sent.push((chat, text));
            }
            fake.sent_changed.notify_waiters();
            json!({"message_id": 1})
        }
        "getUpdates" => {
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

struct World {
    h: Harness,
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
            .with_state(fake.clone()),
    )
    .await?;
    let bots = yulius::wiring::telegram::TelegramBots::new(format!("http://{}", served.addr), 1);
    h.app.set_channel(ChannelServices::new(
        Arc::new(bots),
        ChannelTuning {
            backoff_min: Duration::from_millis(20),
            backoff_max: Duration::from_millis(200),
            window_ms: MINUTE_MS,
            flush_every: Duration::from_millis(50),
        },
    ));
    h.app.start_channel().await;
    Ok(World {
        h,
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

/// [`ask`] naming a problem of the whole sandbox (`cause`, SPEC-494).
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

/// TGC-001, TGC-002: an instance administrator sets the bot (checked, sealed, audited, never
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

/// TGC-011..TGC-013: a single-use 10-minute code from a browser session links a private chat;
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

/// TGC-014: an unlinked chat gets one fixed reply, at most every 10 minutes, and nothing else.
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

/// TGC-020..TGC-023: a task entering Needs you reaches the linked people who see it, with its
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

/// TGC-024: seven notifications in one minute: five messages, then one summary of the other two.
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

/// TGC-003: after errors the poll retries with backoff, says so, and recovers.
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

/// TGC-022: what was already the case when the bot became active (a start of the server, or a
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

/// Publishes `acc1`'s view with `status`, as the accounts code does after a change.
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
                usage_read: None,
            }),
        },
    );
}

/// Review round 1: a summary held for a chat is never sent after that chat was unlinked (TGC-013).
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

/// TGC-025: a subscription switch ("Continuing") is under the hood, as in the notification center
/// (SPEC-468 CCF-008): it sends nothing, and a task that reads afterwards as it did before (its
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

/// TGC-026: tasks asking about their sandbox's Docker (SPEC-494) send one message per episode,
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

/// TGC-027: a person's request for control of a task (SPEC-490 OWN-008) reaches the task's
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

/// TGC-003 (review round 1): a backlog of long messages (here 100 of 4096 CJK characters, about
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
