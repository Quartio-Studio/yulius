//! A subscription connected by pasting a token made with `claude setup-token` elsewhere
//! (SPEC-502): created `ok` with no login terminal, or signed in again by a new paste; only the
//! token's shape is taken, a token Claude refuses (401) changes nothing, and the token is sealed
//! like a captured one and never echoed or audited. The last test starts a real task on such a
//! subscription with the fake agent.

use crate::pool_supervision::{ScriptedUsage, build_with, new_task, runtime, screen, seed, temp};
use crate::support;
use crate::support::eventually;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::agents::FakeAgentOptions;
use support::{
    Creds, HarnessOptions, TestResult, call, harness_with, setup_superadmin_creds, user_with_token,
};
use tokio::sync::mpsc;
use yulius_app::auth::AuthContext;
use yulius_app::error::AppResult;
use yulius_app::model::{AccountRecord, AccountStatus, Secret};
use yulius_app::ports::session::{
    AttachedRun, LaunchPlan, RunEvent, RunExit, RunInspection, RunReader, RunSignal, RunWelcome,
    RunWriter, SessionHost, SpawnedRun,
};
use yulius_app::ports::store::{AccountStore, AuditStore};
use yulius_app::ports::usage::{UsageFailure, UsageFailureKind, UsageProbe};
use yulius_contracts::api::admin::CreateAccount;
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::AccountUsage;
use yulius_ptyhost::test_support::locate_fake_agent;

/// Synthetic tokens of the real shape (not credentials): Claude's profile answers 403 for the
/// first (a `setup-token`, the normal case), gives an email for the second and refuses (401) the
/// third.
const PASTED: &str = concat!(
    "sk-ant-oat01-",
    "SYNTHETICpasted0123456789abcdefghijklmnopqrstuv-test"
);
const WITH_EMAIL: &str = concat!(
    "sk-ant-oat01-",
    "SYNTHETICemail00123456789abcdefghijklmnopqrstuv-test"
);
const REFUSED: &str = concat!(
    "sk-ant-oat01-",
    "SYNTHETICrefused123456789abcdefghijklmnopqrstuv-test"
);
/// Claude's profile does not answer for it just now.
const UNANSWERED: &str = concat!(
    "sk-ant-oat01-",
    "SYNTHETICsilent0123456789abcdefghijklmnopqrstuv-test"
);
/// What a sign-in in the terminal prints, when a test has it print a token.
const CAPTURED: &str = concat!(
    "sk-ant-oat01-",
    "SYNTHETICcaptured23456789abcdefghijklmnopqrstuv-test"
);

/// Claude's profile as the fake sees it, per token; it counts its requests.
#[derive(Default)]
struct Profile {
    asked: Mutex<Vec<String>>,
    /// Usage readings asked for.
    fetched: std::sync::atomic::AtomicUsize,
}

impl Profile {
    fn asked(&self) -> usize {
        self.asked.lock().map(|a| a.len()).unwrap_or(0)
    }
}

#[async_trait]
impl UsageProbe for Profile {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
        self.fetched.fetch_add(1, Ordering::SeqCst);
        Err(UsageFailure::not_shared())
    }

    async fn account_email(
        &self,
        _: Provider,
        credential: &Secret,
    ) -> Result<Option<String>, UsageFailure> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(credential.expose().to_owned());
        }
        match credential.expose() {
            PASTED => Err(UsageFailure::not_shared()),
            WITH_EMAIL => Ok(Some("lena@example.com".into())),
            REFUSED => Err(UsageFailure::of(
                UsageFailureKind::Rejected,
                "Claude refused the sign-in.",
            )),
            _ => Err(UsageFailure::new("Claude did not answer just now.")),
        }
    }
}

/// A process host whose sign-ins run until they are stopped, printing `prints` (if set) first.
/// A `stuck` host's sign-ins ignore the stop, and killing them does not return until `release`.
/// A `deaf` host's sign-ins ignore the stop, so they end only when killed, after the 10 s wait.
#[derive(Default)]
struct WaitingHost {
    runs: Mutex<Vec<mpsc::Sender<RunEvent>>>,
    prints: Mutex<Option<&'static str>>,
    stuck: AtomicBool,
    deaf: AtomicBool,
    released: (Mutex<bool>, Condvar),
}

impl WaitingHost {
    fn release(&self) {
        let (lock, released) = &self.released;
        if let Ok(mut done) = lock.lock() {
            *done = true;
        }
        released.notify_all();
    }
}

struct Reader(mpsc::Receiver<RunEvent>);

#[async_trait]
impl RunReader for Reader {
    async fn next(&mut self) -> AppResult<Option<RunEvent>> {
        Ok(self.0.recv().await)
    }
}

/// Stopping the sign-in makes it exit at once, unless it is deaf.
struct Writer(Option<mpsc::Sender<RunEvent>>, bool);

#[async_trait]
impl RunWriter for Writer {
    async fn input(&mut self, _: &[u8]) -> AppResult<()> {
        Ok(())
    }
    async fn resize(&mut self, _: u16, _: u16) -> AppResult<()> {
        Ok(())
    }
    async fn signal(&mut self, _: RunSignal) -> AppResult<()> {
        if self.1 {
            return Ok(());
        }
        if let Some(tx) = self.0.take() {
            let _ = tx
                .send(RunEvent::Exit(RunExit {
                    code: None,
                    signal: Some(15),
                    spawn_failed: false,
                    usage: None,
                }))
                .await;
        }
        Ok(())
    }
    async fn shutdown(&mut self, _: u32) -> AppResult<()> {
        Ok(())
    }
}

#[async_trait]
impl SessionHost for WaitingHost {
    async fn spawn(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        _: &LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        Ok(SpawnedRun {
            host_pid: 1,
            child_pid: 2,
        })
    }

    async fn attach(&self, _: &Path, _: &[u8; 32]) -> AppResult<AttachedRun> {
        let (tx, rx) = mpsc::channel(8);
        if let Ok(mut runs) = self.runs.lock() {
            runs.push(tx.clone());
        }
        if let Some(token) = self.prints.lock().ok().and_then(|p| *p) {
            let output = format!("Your OAuth token (valid for 1 year):\r\n\r\n{token}\r\n");
            let _ = tx
                .send(RunEvent::Output {
                    seq: 1,
                    bytes: output.into_bytes(),
                })
                .await;
        }
        Ok(AttachedRun {
            welcome: RunWelcome {
                rendered: false,
                child_pid: 2,
                seq: 0,
                cols: 500,
                rows: 40,
                snapshot: None,
            },
            reader: Box::new(Reader(rx)),
            writer: Box::new(Writer(
                Some(tx),
                self.stuck.load(Ordering::SeqCst) || self.deaf.load(Ordering::SeqCst),
            )),
        })
    }

    fn inspect(&self, _: &Path) -> RunInspection {
        RunInspection::Missing
    }

    fn terminate(&self, _: &Path) -> AppResult<()> {
        if self.stuck.load(Ordering::SeqCst) {
            // The worker hands its other tasks on while this one hangs, as a slow kill would.
            tokio::task::block_in_place(|| {
                let (lock, released) = &self.released;
                if let Ok(done) = lock.lock() {
                    let _ =
                        released.wait_timeout_while(done, Duration::from_secs(60), |done| !*done);
                }
            });
        }
        Ok(())
    }
}

struct World {
    h: support::Harness,
    admin: Creds,
    profile: Arc<Profile>,
    host: Arc<WaitingHost>,
}

async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let profile = Arc::new(Profile::default());
    let host = Arc::new(WaitingHost::default());
    let h = harness_with(HarnessOptions {
        session_host: Some(host.clone()),
        usage: Some(profile.clone()),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    Ok(World {
        h,
        admin,
        profile,
        host,
    })
}

async fn admin_ctx(w: &World) -> Result<AuthContext, Box<dyn std::error::Error>> {
    let cookie = w.admin.cookie.as_deref().ok_or("cookie")?;
    Ok(w.h
        .app
        .authenticate_cookie(cookie)
        .await?
        .ok_or("session")?)
}

/// How the sign-in `login` ended (`None` while it runs), read without touching it.
fn login_ended(
    w: &World,
    ctx: &AuthContext,
    login: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let exit = w.h.app.login_terminal(ctx, login)?.exit();
    Ok(exit
        .map(|e| serde_json::to_value(e).map(|v| v["status"].as_str().unwrap_or("").to_owned()))
        .transpose()?)
}

/// Starts a sign-in in the terminal for a new subscription; its id and login id.
async fn signing_in(
    w: &World,
    label: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "anthropic", "label": label})),
        &w.admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok((
        created.body["account"]["id"]
            .as_str()
            .ok_or("id")?
            .to_owned(),
        created.body["login_id"]
            .as_str()
            .ok_or("login_id")?
            .to_owned(),
    ))
}

async fn paste(
    w: &World,
    label: &str,
    token: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &w.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "anthropic", "label": label, "token": token})),
        &w.admin,
    )
    .await
}

async fn record(w: &World, id: &str) -> Result<AccountRecord, Box<dyn std::error::Error>> {
    Ok(w.h
        .store
        .account(&AccountId::parse(id)?)
        .await?
        .ok_or("account")?)
}

fn opened(w: &World, account: &AccountRecord) -> Result<String, Box<dyn std::error::Error>> {
    Ok(String::from_utf8(
        w.h.app
            .deps
            .crypto
            .open(account.credential.as_ref().ok_or("credential")?)?,
    )?)
}

/// No answer may carry a token, whole or in part.
fn assert_no_token(body: &Value) {
    let text = body.to_string();
    assert!(!text.contains("SYNTHETIC"), "{text}");
}

/// AC-1 (TKP-001, TKP-004, TKP-005, TKP-007): a pasted token makes a subscription `ok` at once,
/// with no login, sealed like a captured token; the profile's 403 is "no email"; an email is
/// stored; the audit says `via=token` and never holds the token.
#[tokio::test]
async fn a_pasted_token_connects_a_subscription_at_once() -> TestResult {
    let w = world().await?;
    let mut events = w.h.app.events.subscribe();
    let created = paste(&w, "laptop Max", &format!("  {PASTED}\n")).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_no_token(&created.body);
    assert!(created.body.get("login_id").is_none(), "{}", created.body);
    let account = &created.body["account"];
    assert_eq!(account["status"], "ok");
    assert_eq!(account["label"], "laptop Max");
    assert!(account["email"].is_null());
    let id = account["id"].as_str().ok_or("id")?.to_owned();
    let stored = record(&w, &id).await?;
    assert_eq!(stored.status, AccountStatus::Ok);
    assert_eq!(opened(&w, &stored)?, PASTED, "trimmed and sealed");
    assert!(stored.email_checked_at.is_some(), "the 403 is an answer");
    assert!(
        !w.h.app.config.data_dir.join("logins").exists(),
        "no sign-in ran"
    );
    let audit = w.h.store.audit_details(&id, "account.created", 5).await?;
    assert_eq!(audit, ["provider=anthropic label=\"laptop Max\" via=token"]);
    // The UI hears of it at once.
    let published = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = serde_json::to_value(&events.recv().await?.event)?;
            if event["type"] == "account.updated" && event["account"]["id"] == id.as_str() {
                return Ok::<Value, Box<dyn std::error::Error>>(event["account"].clone());
            }
        }
    })
    .await??;
    assert_eq!(published["status"], "ok");
    assert_no_token(&published);

    let with_email = paste(&w, "work Max", WITH_EMAIL).await?;
    assert_eq!(
        with_email.status,
        StatusCode::CREATED,
        "{}",
        with_email.body
    );
    assert_eq!(with_email.body["account"]["email"], "lena@example.com");
    // No answer for now accepts the token and leaves the email to the background round.
    let unanswered = paste(&w, "spare Max", UNANSWERED).await?;
    assert_eq!(
        unanswered.status,
        StatusCode::CREATED,
        "{}",
        unanswered.body
    );
    assert_eq!(unanswered.body["account"]["status"], "ok");
    let spare = record(&w, unanswered.body["account"]["id"].as_str().ok_or("id")?).await?;
    assert!(spare.email_checked_at.is_none(), "asked again later");
    assert_eq!(w.profile.asked(), 3, "one profile request per paste");
    // Listed last: a listing starts the background email round, which may ask again about the
    // subscriptions without an email; counted before it, the pastes' requests are exact.
    let list = call(
        &w.h.router,
        Method::GET,
        "/api/admin/accounts",
        None,
        &w.admin,
    )
    .await?;
    assert_no_token(&list.body);
    assert_eq!(list.body[0]["status"], "ok", "{}", list.body);
    Ok(())
}

/// AC-2 (TKP-003, TKP-006): anything but the shape `claude setup-token` prints is 422, says what
/// a token looks like without echoing it, and creates nothing; a member may not paste.
#[tokio::test]
async fn a_malformed_token_is_refused_and_creates_nothing() -> TestResult {
    let w = world().await?;
    let body = "SYNTHETICmalformed0123456789abcdef";
    for token in [
        String::new(),
        body.to_owned(),
        format!("sk-ant-api03-{body}"),
        concat!("sk-ant-oat01-", "SYNTHETICshort").to_owned(),
        format!("sk-ant-oat01-{body} trailing"),
        format!("sk-ant-oat01-{body}{}", "x".repeat(512)),
        format!("token=sk-ant-oat01-{body}"),
    ] {
        let refused = paste(&w, "Claude", &token).await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{token:?}"
        );
        assert_eq!(refused.body["error"]["code"], "invalid");
        let message = refused.body["error"]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(message.contains("claude setup-token"), "{message}");
        assert_no_token(&refused.body);
    }
    assert_eq!(w.profile.asked(), 0, "nothing malformed reaches Claude");
    let member = user_with_token(&w.h, "u-bob", "bob", false).await?;
    let forbidden = call(
        &w.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "anthropic", "label": "Claude", "token": PASTED})),
        &member,
    )
    .await?;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert_eq!(
        w.profile.asked(),
        0,
        "a member's token never reaches Claude"
    );
    assert!(w.h.store.accounts().await?.is_empty());
    Ok(())
}

/// TKP-006: a body that is not the request's shape is refused without quoting it, so a token sent
/// as a bare string (or anywhere serde would quote it) never comes back in the answer.
#[tokio::test]
async fn a_malformed_body_never_echoes_the_token() -> TestResult {
    let w = world().await?;
    let (id, _) = signing_in(&w, "Claude #5").await?;
    for (method, path) in [
        (Method::POST, "/api/admin/accounts".to_owned()),
        (Method::PUT, format!("/api/admin/accounts/{id}/token")),
    ] {
        for body in [
            json!(PASTED),
            json!({"token": [PASTED]}),
            json!({"tokens": PASTED}),
            // A complete request with the token under a mistyped name: refused, not taken for a
            // request without a token (which would start a sign-in in a terminal).
            json!({"provider": "anthropic", "label": "typo", "tokn": PASTED}),
        ] {
            let refused = call(&w.h.router, method.clone(), &path, Some(body), &w.admin).await?;
            assert_eq!(
                refused.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{method} {path}"
            );
            assert_eq!(refused.body["error"]["code"], "invalid", "{method} {path}");
            assert_no_token(&refused.body);
        }
    }
    assert!(
        record(&w, &id).await?.credential.is_none(),
        "nothing was stored"
    );
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/admin/accounts",
        None,
        &w.admin,
    )
    .await?;
    let labels: Vec<&str> = listed
        .body
        .as_array()
        .ok_or("list")?
        .iter()
        .filter_map(|a| a["label"].as_str())
        .collect();
    assert_eq!(labels, ["Claude #5"], "nothing was created");
    Ok(())
}

/// TKP-005: a pasted token's usage is read right after it is stored, a new subscription's and a
/// replaced one's alike, as after a sign-in in the terminal.
#[tokio::test]
async fn a_pasted_token_has_its_usage_read_at_once() -> TestResult {
    let w = world().await?;
    let created = paste(&w, "read me", PASTED).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let read = || {
        let n = w.profile.fetched.load(Ordering::SeqCst);
        async move { n }
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while read().await < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| "the new subscription's usage was not read")?;
    let id = created.body["account"]["id"].as_str().ok_or("id")?;
    let replaced = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/admin/accounts/{id}/token"),
        Some(json!({"token": WITH_EMAIL})),
        &w.admin,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.body);
    tokio::time::timeout(Duration::from_secs(10), async {
        while read().await < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| "the replaced token's usage was not read")?;
    Ok(())
}

/// AC-3 (TKP-004): a token Claude refuses (401) is 422 and creates nothing.
#[tokio::test]
async fn a_token_claude_refuses_creates_nothing() -> TestResult {
    let w = world().await?;
    let refused = paste(&w, "Claude", REFUSED).await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    let message = refused.body["error"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("Claude did not accept this token"),
        "{message}"
    );
    assert_no_token(&refused.body);
    assert!(w.h.store.accounts().await?.is_empty());
    Ok(())
}

/// AC-4 (TKP-002, TKP-007): a new paste signs an existing subscription in again: a `relogin` one
/// becomes `ok` with the new credential, audited `account.token_replaced` without the token; its
/// running sign-in is cancelled first; malformed or refused tokens change nothing (the sign-in
/// runs on, nothing is audited); unknown ids are 404 and members 403.
#[tokio::test]
async fn a_new_token_signs_a_subscription_in_again() -> TestResult {
    let w = world().await?;
    let ctx = admin_ctx(&w).await?;
    let (id, login) = signing_in(&w, "Claude #1").await?;
    w.h.store
        .set_account_state(
            &AccountId::parse(&id)?,
            AccountStatus::Relogin,
            None,
            Some("Claude refused it: expired"),
            Timestamp(2),
        )
        .await?;
    let path = format!("/api/admin/accounts/{id}/token");

    for token in [concat!("sk-ant-oat01-", "short"), REFUSED] {
        let refused = call(
            &w.h.router,
            Method::PUT,
            &path,
            Some(json!({"token": token})),
            &w.admin,
        )
        .await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            refused.body
        );
        assert_no_token(&refused.body);
        let unchanged = record(&w, &id).await?;
        assert_eq!(unchanged.status, AccountStatus::Relogin);
        assert!(unchanged.credential.is_none());
    }
    assert_eq!(
        login_ended(&w, &ctx, &login)?,
        None,
        "a refusal leaves the sign-in running"
    );
    let asked = w.profile.asked();
    let member = user_with_token(&w.h, "u-bob", "bob", false).await?;
    let forbidden = call(
        &w.h.router,
        Method::PUT,
        &path,
        Some(json!({"token": PASTED})),
        &member,
    )
    .await?;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    let unknown = call(
        &w.h.router,
        Method::PUT,
        "/api/admin/accounts/no-such-account/token",
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(
        w.profile.asked(),
        asked,
        "nothing refused before the check asks Claude"
    );
    assert!(
        w.h.store
            .last_audit(&id, "account.token_replaced", Timestamp(0))
            .await?
            .is_none(),
        "a refused paste is not audited"
    );

    let replaced = call(
        &w.h.router,
        Method::PUT,
        &path,
        Some(json!({"token": WITH_EMAIL})),
        &w.admin,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.body);
    assert_no_token(&replaced.body);
    assert_eq!(replaced.body["status"], "ok");
    assert!(replaced.body["last_error"].is_null());
    assert_eq!(replaced.body["email"], "lena@example.com");
    let stored = record(&w, &id).await?;
    assert_eq!(opened(&w, &stored)?, WITH_EMAIL);
    // The paste itself ended the running sign-in, cancelled (no one asked to cancel it).
    assert_eq!(login_ended(&w, &ctx, &login)?.as_deref(), Some("cancelled"));
    assert!(
        w.h.store
            .last_audit(&id, "account.login_cancelled", Timestamp(0))
            .await?
            .is_none()
    );
    let audit =
        w.h.store
            .last_audit(&id, "account.token_replaced", Timestamp(0))
            .await?
            .ok_or("audited")?;
    assert!(audit.detail.is_none(), "no detail, never the token");

    // A working subscription is signed in again the same way.
    let again = call(
        &w.h.router,
        Method::PUT,
        &path,
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    assert!(
        again.body["email"].is_null(),
        "the older sign-in's email is gone"
    );
    assert_eq!(opened(&w, &record(&w, &id).await?)?, PASTED);
    Ok(())
}

/// AC-7 (TKP-005, SPEC-464 UCL-004, SPEC-463 EML-004): a new pasted token is a new sign-in as on
/// the rest of `dev`: the weekly model limits Claude reported to the old one go (its other windows
/// stay), and an email a person typed stays, whatever Claude's profile says of the new token.
#[tokio::test]
async fn a_new_token_drops_the_old_model_limits_and_keeps_a_typed_email() -> TestResult {
    let w = world().await?;
    let created = paste(&w, "team Max", PASTED).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["account"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    let typed = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/admin/accounts/{id}"),
        Some(json!({"email": "team@example.com"})),
        &w.admin,
    )
    .await?;
    assert_eq!(typed.status, StatusCode::OK, "{}", typed.body);
    assert_eq!(typed.body["email_typed"], true);
    let stored = json!({
        "secondary": {"used_percent": 61.0, "window_minutes": 10_080, "resets_at": 4_000_000_000_000_i64},
        "weekly_opus": {"used_percent": 100.0, "window_minutes": 10_080, "resets_at": 4_100_000_000_000_i64},
    });
    w.h.store
        .set_account_state(
            &AccountId::parse(&id)?,
            AccountStatus::Limited,
            Some(&stored.to_string()),
            None,
            Timestamp(2),
        )
        .await?;

    let path = format!("/api/admin/accounts/{id}/token");
    // Claude's profile names another address for this token: the typed one stays.
    let replaced = call(
        &w.h.router,
        Method::PUT,
        &path,
        Some(json!({"token": WITH_EMAIL})),
        &w.admin,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.body);
    assert_no_token(&replaced.body);
    assert_eq!(replaced.body["status"], "ok");
    assert_eq!(replaced.body["email"], "team@example.com");
    assert_eq!(replaced.body["email_typed"], true);
    let record = record(&w, &id).await?;
    assert_eq!(opened(&w, &record)?, WITH_EMAIL);
    let usage: Value = serde_json::from_str(record.usage_json.as_deref().ok_or("usage")?)?;
    assert!(usage["weekly_opus"].is_null(), "{usage}");
    assert_eq!(usage["secondary"]["used_percent"], 61.0, "{usage}");

    // A profile that may not be read (a `setup-token`'s 403) keeps it as well.
    let again = call(
        &w.h.router,
        Method::PUT,
        &path,
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    assert_eq!(again.body["email"], "team@example.com");
    assert_eq!(again.body["email_typed"], true);
    Ok(())
}

/// AC-8 (TKP-011): a sign-in that has not ended 15 s after the paste cancelled it could still
/// store a token it captured over the pasted one, so the paste is refused with 409 and nothing is
/// stored or audited. Here the sign-in ignores its stop and its host does not return when killed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paste_a_sign_in_does_not_let_go_of_is_refused() -> TestResult {
    let w = world().await?;
    let ctx = admin_ctx(&w).await?;
    w.host.stuck.store(true, Ordering::SeqCst);
    let (id, login) = signing_in(&w, "Claude #3").await?;
    let started = tokio::time::Instant::now();
    let refused = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/admin/accounts/{id}/token"),
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await;
    let waited = started.elapsed();
    w.host.release();
    let refused = refused?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert_no_token(&refused.body);
    assert!(waited >= Duration::from_secs(15), "waited {waited:?}");
    let unchanged = record(&w, &id).await?;
    assert_eq!(unchanged.status, AccountStatus::PendingLogin);
    assert!(unchanged.credential.is_none(), "nothing was stored");
    assert!(
        w.h.store
            .last_audit(&id, "account.token_replaced", Timestamp(0))
            .await?
            .is_none(),
        "a refused paste is not audited"
    );
    // Once the sign-in has let go, the same paste goes through.
    tokio::time::timeout(Duration::from_secs(30), async {
        while login_ended(&w, &ctx, &login)?.is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await??;
    let accepted = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/admin/accounts/{id}/token"),
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await?;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.body);
    assert_eq!(opened(&w, &record(&w, &id).await?)?, PASTED);
    Ok(())
}

/// TKP-002 with two sign-ins of one subscription running: "Sign in again" replaced the first,
/// which has captured a token and is still stopping, by a second. The paste cancels and waits out
/// both, so neither runs on after it and the token the first captured never lands over the paste.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paste_waits_out_every_running_sign_in() -> TestResult {
    let w = world().await?;
    let ctx = admin_ctx(&w).await?;
    if let Ok(mut prints) = w.host.prints.lock() {
        *prints = Some(CAPTURED);
    }
    w.h.app
        .set_account_login_token_grace(Duration::from_secs(600));
    // The first sign-in captures a token and takes its full stop wait to end.
    w.host.deaf.store(true, Ordering::SeqCst);
    let (id, first) = signing_in(&w, "Claude #4").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let terminal = w.h.app.login_terminal(&ctx, &first)?;
            let shown = terminal.subscribe().snapshot;
            terminal.unsubscribe();
            if String::from_utf8_lossy(&shown).contains("[REDACTED]") {
                return Ok::<(), Box<dyn std::error::Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    w.host.deaf.store(false, Ordering::SeqCst);
    if let Ok(mut prints) = w.host.prints.lock() {
        *prints = None;
    }
    let again = call(
        &w.h.router,
        Method::POST,
        &format!("/api/admin/accounts/{id}/relogin"),
        None,
        &w.admin,
    )
    .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let second = again.body["login_id"]
        .as_str()
        .ok_or("login_id")?
        .to_owned();
    assert_eq!(
        login_ended(&w, &ctx, &first)?,
        None,
        "the first is still stopping"
    );

    let replaced = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/admin/accounts/{id}/token"),
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.body);
    assert!(
        login_ended(&w, &ctx, &first)?.is_some(),
        "the first sign-in ended before the paste was stored"
    );
    assert!(
        login_ended(&w, &ctx, &second)?.is_some(),
        "the second sign-in ended before the paste was stored"
    );
    assert_eq!(
        opened(&w, &record(&w, &id).await?)?,
        PASTED,
        "the paste came last"
    );
    Ok(())
}

/// TKP-002: a sign-in in the terminal that has already captured a token, and is still running, is
/// ended first: its token is sealed as it ends, and the pasted one is stored after it and stays.
#[tokio::test]
async fn a_paste_wins_over_a_token_the_terminal_captured() -> TestResult {
    let w = world().await?;
    let ctx = admin_ctx(&w).await?;
    if let Ok(mut prints) = w.host.prints.lock() {
        *prints = Some(CAPTURED);
    }
    // The CLI would run on for ten minutes after printing its token.
    w.h.app
        .set_account_login_token_grace(Duration::from_secs(600));
    let (id, login) = signing_in(&w, "Claude #2").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // A viewer looks at the screen and leaves (viewers are bounded).
            let terminal = w.h.app.login_terminal(&ctx, &login)?;
            let shown = terminal.subscribe().snapshot;
            terminal.unsubscribe();
            if String::from_utf8_lossy(&shown).contains("[REDACTED]") {
                return Ok::<(), Box<dyn std::error::Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    assert_eq!(
        login_ended(&w, &ctx, &login)?,
        None,
        "captured, still running"
    );

    let replaced = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/admin/accounts/{id}/token"),
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.body);
    assert_eq!(
        login_ended(&w, &ctx, &login)?.as_deref(),
        Some("ok"),
        "the captured token was kept as the sign-in ended"
    );
    let stored = record(&w, &id).await?;
    assert_eq!(opened(&w, &stored)?, PASTED, "the paste came last");
    assert_eq!(stored.status, AccountStatus::Ok);
    Ok(())
}

/// AC-5 (TKP-010): with real processes, a new task on an instance whose only subscription was
/// pasted starts on it, and its run gets the pasted token as `CLAUDE_CODE_OAUTH_TOKEN`.
#[test]
fn a_task_starts_on_a_pasted_subscription() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec!["CLAUDE_CODE_OAUTH_TOKEN".into()],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &[]).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let created = app
            .create_account(
                &ctx,
                CreateAccount {
                    provider: "anthropic".into(),
                    label: "pasted".into(),
                    token: Some(PASTED.into()),
                    api_key: None,
                    auth_json: None,
                },
            )
            .await?;
        assert_eq!(created.account.status, "ok");
        assert!(created.login_id.is_none());

        let id = new_task(&app, &sandbox, &user, "Pasted").await?;
        let want = format!("ENV CLAUDE_CODE_OAUTH_TOKEN=set len={}", PASTED.len());
        eventually(
            "the run on the pasted subscription",
            Duration::from_secs(30),
            || {
                let (app, id, want) = (app.clone(), id.clone(), want.clone());
                async move { screen(&app, &id).contains(&want) }
            },
        )
        .await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(
            run.account_id.as_ref().map(AccountId::as_str),
            Some(created.account.id.as_str())
        );
        crate::pool_supervision::stop(&app, &id).await
    })
}

/// SPEC-503 CDX-015: a pasted token is Claude's: given for an `openai` subscription, or together
/// with an API key or an `auth.json`, it is refused (422) without a word of it, nothing is created
/// and Claude's profile is not asked.
#[tokio::test]
async fn a_token_is_only_a_claude_subscriptions_credential() -> TestResult {
    let w = world().await?;
    for body in [
        json!({"provider": "openai", "label": "Codex", "token": PASTED}),
        json!({"provider": "anthropic", "label": "both", "token": PASTED,
               "api_key": "sk-SYNTHETIC0123456789abcdefghij"}),
        json!({"provider": "anthropic", "label": "both", "token": PASTED,
               "auth_json": "{}"}),
    ] {
        let refused = call(
            &w.h.router,
            Method::POST,
            "/api/admin/accounts",
            Some(body.clone()),
            &w.admin,
        )
        .await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            refused.body
        );
        assert_no_token(&refused.body);
    }
    assert!(w.h.store.accounts().await?.is_empty(), "nothing created");
    assert_eq!(w.profile.asked(), 0);
    Ok(())
}

const CODEX_DEVICE: &str = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"h.e30.s","access_token":"SYNTHETIC-device-access","refresh_token":"SYNTHETIC-device-refresh","account_id":"acc-synthetic"},"last_refresh":"2026-11-09T10:00:00Z"}"#;
const CODEX_IMPORT: &str = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"h.e30.s","access_token":"SYNTHETIC-import-access","refresh_token":"SYNTHETIC-import-refresh","account_id":"acc-synthetic"},"last_refresh":"2026-11-09T12:00:00Z"}"#;

/// SPEC-503 IMP-001 (slice C2 review): an import cancels every sign-in of the Codex subscription
/// that runs, not the first found: "Sign in again" replaced one that holds a complete
/// `auth.json` and is still stopping, and neither may write over the imported sign-in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_cancels_every_running_sign_in() -> TestResult {
    let w = world().await?;
    let ctx = admin_ctx(&w).await?;
    w.h.app
        .set_account_login_token_grace(Duration::from_secs(600));
    w.host.deaf.store(true, Ordering::SeqCst);
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "openai", "label": "Codex two"})),
        &w.admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["account"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    let first = created.body["login_id"].as_str().ok_or("login")?.to_owned();
    // The device sign-in finished on OpenAI's side and Codex wrote its file.
    let codex =
        w.h.app
            .config
            .data_dir
            .join("logins")
            .join(&first)
            .join("codex");
    std::fs::create_dir_all(&codex)?;
    std::fs::write(codex.join("auth.json"), CODEX_DEVICE)?;
    w.host.deaf.store(false, Ordering::SeqCst);
    let again = call(
        &w.h.router,
        Method::POST,
        &format!("/api/admin/accounts/{id}/relogin"),
        None,
        &w.admin,
    )
    .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let second = again.body["login_id"]
        .as_str()
        .ok_or("login_id")?
        .to_owned();
    assert_eq!(
        login_ended(&w, &ctx, &first)?,
        None,
        "the first is still stopping"
    );
    let imported = call(
        &w.h.router,
        Method::POST,
        &format!("/api/admin/accounts/{id}/import"),
        Some(json!({"auth_json": CODEX_IMPORT})),
        &w.admin,
    )
    .await?;
    assert_eq!(imported.status, StatusCode::OK, "{}", imported.body);
    tokio::time::timeout(Duration::from_secs(30), async {
        while login_ended(&w, &ctx, &first)?.is_none() || login_ended(&w, &ctx, &second)?.is_none()
        {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await
    .map_err(|_| "a sign-in still runs after the import")??;
    assert_eq!(
        opened(&w, &record(&w, &id).await?)?,
        CODEX_IMPORT,
        "the import was overwritten"
    );
    Ok(())
}

/// SPEC-503 CDX-015 (slice C2 review): a token pasted for a Codex subscription is refused up
/// front in its own words, not as a token Claude refused, and the sign-in running for the
/// subscription goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_for_a_codex_subscription_is_refused_up_front() -> TestResult {
    let w = world().await?;
    let ctx = admin_ctx(&w).await?;
    w.host.deaf.store(true, Ordering::SeqCst);
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "openai", "label": "Codex"})),
        &w.admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["account"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    let login = created.body["login_id"].as_str().ok_or("login")?.to_owned();
    let refused = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/admin/accounts/{id}/token"),
        Some(json!({"token": PASTED})),
        &w.admin,
    )
    .await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    assert_eq!(
        refused.body["error"]["message"],
        "a pasted token is only for an anthropic (Claude) account"
    );
    assert_no_token(&refused.body);
    assert_eq!(w.profile.asked(), 0);
    assert_eq!(login_ended(&w, &ctx, &login)?, None, "the sign-in goes on");
    Ok(())
}

/// SPEC-503 CDX-011 (slice C2 review): an import body of another shape is refused in a fixed
/// sentence, never with serde's, which would quote the `auth.json`; and nothing is stored.
#[tokio::test]
async fn a_malformed_import_never_echoes_the_auth_json() -> TestResult {
    let w = world().await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "openai", "label": "Codex", "auth_json": CODEX_DEVICE})),
        &w.admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["account"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    let path = format!("/api/admin/accounts/{id}/import");
    for body in [
        json!(CODEX_IMPORT),
        json!({"auth_json": {"tokens": {"access_token": "SYNTHETIC-import-access"}}}),
        json!({"auth_jsn": CODEX_IMPORT}),
        json!({"auth_json": CODEX_IMPORT, "label": "SYNTHETIC-import-access"}),
    ] {
        let refused = call(
            &w.h.router,
            Method::POST,
            &path,
            Some(body.clone()),
            &w.admin,
        )
        .await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            refused.body
        );
        assert_eq!(
            refused.body["error"]["message"], r#"the body must be JSON like {"auth_json": "…"}"#,
            "{body}"
        );
        assert!(
            !refused.body.to_string().contains("SYNTHETIC"),
            "{}",
            refused.body
        );
    }
    assert_eq!(
        opened(&w, &record(&w, &id).await?)?,
        CODEX_DEVICE,
        "nothing was stored"
    );
    Ok(())
}
