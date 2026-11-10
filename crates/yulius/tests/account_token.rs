//! A subscription connected by pasting a token made with `claude setup-token` elsewhere
//! (SPEC-450): created `ok` with no login terminal, or signed in again by a new paste; only the
//! token's shape is taken, a token Claude refuses (401) changes nothing, and the token is sealed
//! like a captured one and never echoed or audited. The last test starts a real task on such a
//! subscription with the fake agent.

use crate::pool_supervision::{
    ScriptedUsage, build_with, new_task, runtime, screen, seed, temp, wait_for,
};
use crate::support;

use std::path::Path;
use std::sync::{Arc, Mutex};
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
}

impl Profile {
    fn asked(&self) -> usize {
        self.asked.lock().map(|a| a.len()).unwrap_or(0)
    }
}

#[async_trait]
impl UsageProbe for Profile {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
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
#[derive(Default)]
struct WaitingHost {
    runs: Mutex<Vec<mpsc::Sender<RunEvent>>>,
    prints: Mutex<Option<&'static str>>,
}

struct Reader(mpsc::Receiver<RunEvent>);

#[async_trait]
impl RunReader for Reader {
    async fn next(&mut self) -> AppResult<Option<RunEvent>> {
        Ok(self.0.recv().await)
    }
}

/// Stopping the sign-in makes it exit at once.
struct Writer(Option<mpsc::Sender<RunEvent>>);

#[async_trait]
impl RunWriter for Writer {
    async fn input(&mut self, _: &[u8]) -> AppResult<()> {
        Ok(())
    }
    async fn resize(&mut self, _: u16, _: u16) -> AppResult<()> {
        Ok(())
    }
    async fn signal(&mut self, _: RunSignal) -> AppResult<()> {
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
                child_pid: 2,
                seq: 0,
                cols: 500,
                rows: 40,
                snapshot: None,
            },
            reader: Box::new(Reader(rx)),
            writer: Box::new(Writer(Some(tx))),
        })
    }

    fn inspect(&self, _: &Path) -> RunInspection {
        RunInspection::Missing
    }

    fn terminate(&self, _: &Path) -> AppResult<()> {
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
    assert_eq!(audit, ["provider=anthropic label=laptop Max via=token"]);
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
        "sk-ant-oat01-SYNTHETICshort".to_owned(),
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

    for token in ["sk-ant-oat01-short", REFUSED] {
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
                },
            )
            .await?;
        assert_eq!(created.account.status, "ok");
        assert!(created.login_id.is_none());

        let id = new_task(&app, &sandbox, &user, "Pasted").await?;
        let want = format!("ENV CLAUDE_CODE_OAUTH_TOKEN=set len={}", PASTED.len());
        wait_for(
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
