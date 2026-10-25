//! Account administration and the native login flow over a scripted fake process host
//! (AC-ROT-1, ROT-001/002/009, ACC-009; SPEC-002 §7.2 "Login", §8 masking/sealing).

use crate::support;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, admit_and_sign_in, call, configure_github_app, harness_with_host,
    setup_superadmin_creds,
};
use tokio::sync::{Semaphore, mpsc};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use yulius_app::auth::AuthContext;
use yulius_app::error::AppResult;
use yulius_app::hub::HubFrame;
use yulius_app::model::{AttemptRecord, LaunchKind, Sealed, TaskOrigin, TaskRecord};
use yulius_app::ports::session::{
    AttachedRun, Containment, LaunchPlan, RunEvent, RunExit, RunInspection, RunReader, RunSignal,
    RunWelcome, RunWriter, SessionHost, SpawnedRun,
};
use yulius_app::ports::store::{AccountStore, TaskStore};
use yulius_contracts::terminal::{KIND_OUTPUT, KIND_SNAPSHOT, decode_binary};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

/// Synthetic token: realistic shape, not a real credential.
const TOKEN: &str = "sk-ant-oat01-SYNTHETICabcdefghijklmnopqrstuvwxyz0123456789_-test";
const MASK: &str = "sk-ant-oat01-[REDACTED]";

/// What one fake login process does.
#[derive(Clone, Default)]
struct Script {
    /// Output chunks, emitted in order.
    chunks: Vec<String>,
    /// Exit code after the chunks; `None` = keep running until signalled.
    exit: Option<i32>,
    /// Wait for [`FakeHost::release`] before emitting anything.
    gated: bool,
}

struct Run {
    tx: mpsc::Sender<RunEvent>,
    rx: Option<mpsc::Receiver<RunEvent>>,
    script: Script,
}

/// A recorded spawn: run directory, plan and PTY size.
type Spawned = (PathBuf, LaunchPlan, (u16, u16));

/// Process host fake: records plans, inputs and signals; plays scripts.
struct FakeHost {
    scripts: Mutex<VecDeque<Script>>,
    runs: Mutex<HashMap<PathBuf, Run>>,
    plans: Mutex<Vec<Spawned>>,
    log: Arc<Mutex<Vec<String>>>,
    release: Arc<Semaphore>,
    input_seen: Arc<tokio::sync::Notify>,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self {
            scripts: Mutex::default(),
            runs: Mutex::default(),
            plans: Mutex::default(),
            log: Arc::default(),
            release: Arc::new(Semaphore::new(0)),
            input_seen: Arc::default(),
        }
    }
}

impl FakeHost {
    fn push(&self, script: Script) {
        if let Ok(mut s) = self.scripts.lock() {
            s.push_back(script);
        }
    }
    fn plans(&self) -> Vec<Spawned> {
        self.plans.lock().map(|p| p.clone()).unwrap_or_default()
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

struct FakeReader(mpsc::Receiver<RunEvent>);

#[async_trait]
impl RunReader for FakeReader {
    async fn next(&mut self) -> AppResult<Option<RunEvent>> {
        Ok(self.0.recv().await)
    }
}

struct FakeWriter {
    tx: mpsc::Sender<RunEvent>,
    log: Arc<Mutex<Vec<String>>>,
    exited: Arc<AtomicBool>,
    input_seen: Arc<tokio::sync::Notify>,
}

impl FakeWriter {
    fn record(&self, entry: String) {
        if let Ok(mut l) = self.log.lock() {
            l.push(entry);
        }
    }
    async fn die(&self) {
        if !self.exited.swap(true, Ordering::SeqCst) {
            let _ = self
                .tx
                .send(RunEvent::Exit(RunExit {
                    code: None,
                    signal: Some(15),
                    spawn_failed: false,
                    usage: None,
                }))
                .await;
        }
    }
}

#[async_trait]
impl RunWriter for FakeWriter {
    async fn input(&mut self, bytes: &[u8]) -> AppResult<()> {
        self.record(format!("input:{}", String::from_utf8_lossy(bytes)));
        self.input_seen.notify_one();
        Ok(())
    }
    async fn resize(&mut self, cols: u16, rows: u16) -> AppResult<()> {
        self.record(format!("resize:{cols}x{rows}"));
        Ok(())
    }
    async fn signal(&mut self, signal: RunSignal) -> AppResult<()> {
        self.record(format!("signal:{signal:?}"));
        if signal != RunSignal::Interrupt {
            self.die().await;
        }
        Ok(())
    }
    async fn shutdown(&mut self, grace_ms: u32) -> AppResult<()> {
        self.record(format!("shutdown:{grace_ms}"));
        self.die().await;
        Ok(())
    }
}

#[async_trait]
impl SessionHost for FakeHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        _attempt_id: &str,
        _task_id: &str,
        plan: &LaunchPlan,
        _token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        for file in &plan.files {
            std::fs::write(&file.path, &file.contents)
                .map_err(yulius_app::error::AppError::internal)?;
        }
        if let Ok(mut p) = self.plans.lock() {
            p.push((run_dir.to_path_buf(), plan.clone(), size));
        }
        let script = self
            .scripts
            .lock()
            .ok()
            .and_then(|mut s| s.pop_front())
            .unwrap_or_default();
        let (tx, rx) = mpsc::channel(64);
        if let Ok(mut runs) = self.runs.lock() {
            runs.insert(
                run_dir.to_path_buf(),
                Run {
                    tx,
                    rx: Some(rx),
                    script,
                },
            );
        }
        Ok(SpawnedRun {
            host_pid: 1,
            child_pid: 2,
        })
    }

    async fn attach(&self, run_dir: &Path, _token: &[u8; 32]) -> AppResult<AttachedRun> {
        let (tx, rx, script) = {
            let mut runs = self
                .runs
                .lock()
                .map_err(yulius_app::error::AppError::internal)?;
            let run = runs
                .get_mut(run_dir)
                .ok_or(yulius_app::error::AppError::NotFound)?;
            (
                run.tx.clone(),
                run.rx.take().ok_or(yulius_app::error::AppError::NotFound)?,
                run.script.clone(),
            )
        };
        let exited = Arc::new(AtomicBool::new(false));
        let emitter_tx = tx.clone();
        let emitter_exited = exited.clone();
        let release = self.release.clone();
        tokio::spawn(async move {
            if script.gated {
                let Ok(permit) = release.acquire().await else {
                    return;
                };
                permit.forget();
            }
            for (i, chunk) in script.chunks.iter().enumerate() {
                let event = RunEvent::Output {
                    seq: i as u64 + 1,
                    bytes: chunk.as_bytes().to_vec(),
                };
                if emitter_tx.send(event).await.is_err() {
                    return;
                }
            }
            if let Some(code) = script.exit
                && !emitter_exited.swap(true, Ordering::SeqCst)
            {
                let _ = emitter_tx
                    .send(RunEvent::Exit(RunExit {
                        code: Some(code),
                        signal: None,
                        spawn_failed: false,
                        usage: None,
                    }))
                    .await;
            }
        });
        Ok(AttachedRun {
            welcome: RunWelcome {
                child_pid: 2,
                seq: 0,
                cols: 500,
                rows: 40,
                snapshot: None,
            },
            reader: Box::new(FakeReader(rx)),
            writer: Box::new(FakeWriter {
                tx,
                log: self.log.clone(),
                exited,
                input_seen: self.input_seen.clone(),
            }),
        })
    }

    fn inspect(&self, _run_dir: &Path) -> RunInspection {
        RunInspection::Missing
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        if let Ok(mut l) = self.log.lock() {
            l.push(format!("terminate:{}", run_dir.display()));
        }
        Ok(())
    }
}

struct Setup {
    h: support::Harness,
    host: Arc<FakeHost>,
    admin: Creds,
}

async fn setup() -> Result<Setup, Box<dyn std::error::Error>> {
    let host = Arc::new(FakeHost::default());
    let h = harness_with_host(host.clone())?;
    let admin = setup_superadmin_creds(&h).await?;
    Ok(Setup { h, host, admin })
}

async fn ctx_of(
    h: &support::Harness,
    creds: &Creds,
) -> Result<AuthContext, Box<dyn std::error::Error>> {
    let cookie = creds.cookie.as_deref().ok_or("cookie")?;
    Ok(h.app.authenticate_cookie(cookie).await?.ok_or("session")?)
}

/// Waits (bounded) for an `account.updated` event of `account_id` whose status is not
/// `pending_login`, and returns the account JSON.
async fn account_settled(
    rx: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
    account_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let wait = async {
        loop {
            let envelope = rx.recv().await?;
            let event = serde_json::to_value(&envelope.event)?;
            if event["type"] == "account.updated"
                && event["account"]["id"] == account_id
                && event["account"]["status"] != "pending_login"
            {
                assert!(envelope.admin_only, "account events are for administrators");
                return Ok::<Value, Box<dyn std::error::Error>>(event["account"].clone());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await?
}

/// Collects masked viewer output until the login's exit frame.
async fn viewer_output(
    sub: &mut yulius_app::hub::Subscription,
) -> Result<(String, String, Option<i32>), Box<dyn std::error::Error>> {
    let wait = async {
        let mut out = Vec::new();
        loop {
            match sub.rx.recv().await? {
                HubFrame::Output { bytes, .. } => out.extend_from_slice(&bytes),
                HubFrame::Exit { status, code, .. } => {
                    return Ok::<_, Box<dyn std::error::Error>>((
                        String::from_utf8_lossy(&out).into_owned(),
                        status,
                        code,
                    ));
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await?
}

async fn create(
    s: &Setup,
    provider: &str,
    label: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let r = call(
        &s.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": provider, "label": label})),
        &s.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["account"]["status"], "pending_login");
    assert_eq!(r.body["account"]["label"], label);
    let id = r.body["account"]["id"].as_str().ok_or("id")?.to_owned();
    let login = r.body["login_id"].as_str().ok_or("login_id")?.to_owned();
    Ok((id, login))
}

fn login_dir(s: &Setup, login_id: &str) -> PathBuf {
    s.h.app.config.data_dir.join("logins").join(login_id)
}

#[tokio::test]
async fn claude_login_masks_the_token_for_viewers_and_seals_it() -> TestResult {
    let s = setup().await?;
    // The token is split across three chunks; the PTY never wraps it (500 columns).
    s.host.push(Script {
        chunks: vec![
            "Browser didn't open? Use the url below to sign in:\r\nhttps://claude.ai/oauth/authorize?code=true\r\n".into(),
            "Paste code here if prompted > ".into(),
            "\r\n\u{1b}[32mYour OAuth token (valid for 1 year):\u{1b}[0m\r\n\r\nsk-ant-oa".into(),
            TOKEN["sk-ant-oa".len()..30].to_owned(),
            format!("{}\r\n\r\nStore this token securely.\r\n", &TOKEN[30..]),
        ],
        exit: Some(0),
        gated: true,
    });
    let mut events = s.h.app.events.subscribe();
    let (account_id, login_id) = create(&s, "anthropic", "Claude #1").await?;

    // The CLI's own sign-in runs in a wide PTY with a temporary config dir and no containment.
    let plans = s.host.plans();
    let (run_dir, plan, size) = plans.first().ok_or("spawned")?;
    assert_eq!(plan.argv, ["claude", "setup-token"]);
    assert_eq!(*size, (500, 40));
    assert_eq!(*run_dir, login_dir(&s, &login_id));
    assert_eq!(plan.containment, Containment::None);
    assert!(plan.secret_env.is_empty());
    let config = plan
        .env
        .iter()
        .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
        .map(|(_, v)| PathBuf::from(v))
        .ok_or("CLAUDE_CONFIG_DIR")?;
    assert!(config.starts_with(run_dir));
    assert!(
        plan.env
            .iter()
            .any(|(k, v)| k == "DISABLE_AUTOUPDATER" && v == "1"),
        "the sign-in CLI never tries to update itself (#45)"
    );
    assert!(
        run_dir.exists(),
        "temporary directory exists while the login runs"
    );

    // An administrator watches and types the code from the browser.
    let ctx = ctx_of(&s.h, &s.admin).await?;
    let terminal = s.h.app.login_terminal(&ctx, &login_id)?;
    let state = serde_json::to_value(terminal.state())?;
    assert_eq!(
        (state["cols"].as_u64(), state["can_write"].as_bool()),
        (Some(500), Some(true))
    );
    let mut sub = terminal.subscribe();
    terminal.input(b"pasted-code\r").await?;
    assert!(s.host.log().contains(&"input:pasted-code\r".to_owned()));
    s.host.release.add_permits(1);

    let (seen, status, code) = viewer_output(&mut sub).await?;
    assert!(!seen.contains(TOKEN), "token reached a viewer: {seen}");
    assert!(
        !seen.contains("SYNTHETICabcdefghij"),
        "token fragment reached a viewer"
    );
    assert!(seen.contains(MASK), "masked marker shown: {seen}");
    assert!(seen.contains("https://claude.ai/oauth/authorize?code=true"));
    assert!(seen.contains("Store this token securely."));
    assert_eq!((status.as_str(), code), ("ok", Some(0)));

    let account = account_settled(&mut events, &account_id).await?;
    assert_eq!(account["status"], "ok");
    assert!(account["last_error"].is_null());

    // Sealed at rest, opens to exactly the token; never returned by the API.
    let record =
        s.h.store
            .account(&AccountId::parse(&account_id)?)
            .await?
            .ok_or("account")?;
    let sealed = record.credential.ok_or("credential")?;
    assert!(!String::from_utf8_lossy(&sealed.0).contains(TOKEN));
    assert_eq!(s.h.app.deps.crypto.open(&sealed)?, TOKEN.as_bytes());
    let list = call(
        &s.h.router,
        Method::GET,
        "/api/admin/accounts",
        None,
        &s.admin,
    )
    .await?;
    assert!(!list.body.to_string().contains(TOKEN));
    assert_eq!(list.body[0]["status"], "ok");

    // The temporary directory is gone; late viewers get a masked snapshot and the exit.
    assert!(!run_dir.exists());
    let late = s.h.app.login_terminal(&ctx, &login_id)?;
    let snapshot = late.subscribe();
    let screen = String::from_utf8_lossy(&snapshot.snapshot);
    assert!(!screen.contains("SYNTHETICabcdefghij") && screen.contains(MASK));
    late.unsubscribe();
    assert_eq!(
        serde_json::to_value(late.exit())?,
        json!({"type": "exit", "status": "ok", "code": 0})
    );
    assert!(late.input(b"x").await.is_err(), "no input after the end");
    Ok(())
}

#[tokio::test]
async fn only_claude_accounts_can_be_added() -> TestResult {
    let s = setup().await?;
    let refused = call(
        &s.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "openai", "label": "Codex"})),
        &s.admin,
    )
    .await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    assert!(s.host.plans().is_empty(), "no login was started");
    Ok(())
}

#[tokio::test]
async fn failed_logins_record_errors_and_relogin_failures_keep_working_accounts() -> TestResult {
    let s = setup().await?;
    s.host.push(Script {
        chunks: vec!["Error: OAuth flow cancelled\r\n".into()],
        exit: Some(1),
        ..Script::default()
    });
    let mut events = s.h.app.events.subscribe();
    let (account_id, _) = create(&s, "anthropic", "Claude #2").await?;
    let failed = account_settled(&mut events, &account_id).await?;
    assert_eq!(failed["status"], "error");
    let message = failed["last_error"].as_str().ok_or("last_error")?;
    assert!(message.contains("without producing a token"), "{message}");
    let id = AccountId::parse(&account_id)?;
    assert!(
        s.h.store
            .account(&id)
            .await?
            .ok_or("account")?
            .credential
            .is_none()
    );

    // Re-login succeeds.
    s.host.push(Script {
        chunks: vec![format!("token: {TOKEN}\r\n")],
        exit: Some(0),
        ..Script::default()
    });
    let r = call(
        &s.h.router,
        Method::POST,
        &format!("/api/admin/accounts/{account_id}/relogin"),
        None,
        &s.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body["login_id"].is_string());
    let ok = account_settled(&mut events, &account_id).await?;
    assert_eq!(
        (ok["status"].as_str(), ok["last_error"].as_str()),
        (Some("ok"), None)
    );

    // A later failed re-login keeps the working credential in rotation.
    s.host.push(Script {
        chunks: vec!["no token here\r\n".into()],
        exit: Some(2),
        ..Script::default()
    });
    let r = call(
        &s.h.router,
        Method::POST,
        &format!("/api/admin/accounts/{account_id}/relogin"),
        None,
        &s.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    let kept = account_settled(&mut events, &account_id).await?;
    assert_eq!(kept["status"], "ok");
    let message = kept["last_error"].as_str().ok_or("last_error")?;
    assert!(message.starts_with("re-login failed"), "{message}");
    assert!(message.ends_with("(exit code 2)"), "{message}");
    let record = s.h.store.account(&id).await?.ok_or("account")?;
    assert_eq!(
        s.h.app
            .deps
            .crypto
            .open(&record.credential.ok_or("credential")?)?,
        TOKEN.as_bytes()
    );
    Ok(())
}

#[tokio::test]
async fn logins_time_out_and_their_process_is_stopped() -> TestResult {
    let s = setup().await?;
    s.h.app.set_account_login_timeout(Duration::from_millis(50));
    s.host.push(Script {
        chunks: vec!["Waiting for the browser...\r\n".into()],
        exit: None,
        ..Script::default()
    });
    let mut events = s.h.app.events.subscribe();
    let (account_id, login_id) = create(&s, "anthropic", "Claude slow").await?;
    let account = account_settled(&mut events, &account_id).await?;
    assert_eq!(account["status"], "error");
    let message = account["last_error"].as_str().ok_or("last_error")?;
    assert!(message.contains("timed out"), "{message}");
    let log = s.host.log();
    assert!(log.iter().any(|l| l == "signal:Terminate"), "{log:?}");
    assert!(!login_dir(&s, &login_id).exists());
    let ctx = ctx_of(&s.h, &s.admin).await?;
    let exit = serde_json::to_value(s.h.app.login_terminal(&ctx, &login_id)?.exit())?;
    assert_eq!(exit["status"], "error");
    Ok(())
}

/// SPEC-243 LGN-001: a CLI that prints the token and keeps running is stopped after the grace
/// period, and the sign-in ends `ok` with the token sealed (not `running` until the timeout).
#[tokio::test]
async fn a_sign_in_ends_once_its_token_is_captured() -> TestResult {
    let s = setup().await?;
    s.h.app
        .set_account_login_token_grace(Duration::from_millis(50));
    s.host.push(Script {
        chunks: vec![
            "Your OAuth token (valid for 1 year):\r\n\r\n".into(),
            format!("{TOKEN}\r\n\r\nStore this token securely.\r\n"),
        ],
        exit: None,
        ..Script::default()
    });
    let mut events = s.h.app.events.subscribe();
    let (account_id, login_id) = create(&s, "anthropic", "Claude lingering").await?;
    let ctx = ctx_of(&s.h, &s.admin).await?;
    let mut sub = s.h.app.login_terminal(&ctx, &login_id)?.subscribe();
    let (_, status, _) = viewer_output(&mut sub).await?;
    assert_eq!(status, "ok");
    let account = account_settled(&mut events, &account_id).await?;
    assert_eq!(account["status"], "ok");
    let log = s.host.log();
    assert!(log.iter().any(|l| l == "signal:Terminate"), "{log:?}");
    let record =
        s.h.store
            .account(&AccountId::parse(&account_id)?)
            .await?
            .ok_or("account")?;
    assert_eq!(
        s.h.app
            .deps
            .crypto
            .open(&record.credential.ok_or("credential")?)?,
        TOKEN.as_bytes()
    );
    assert!(!login_dir(&s, &login_id).exists());
    Ok(())
}

/// SPEC-243 LGN-002: a cancel that comes after the token was printed (in the grace before the
/// CLI exits) keeps the token: the sign-in had succeeded.
#[tokio::test]
async fn a_cancel_after_the_token_keeps_it() -> TestResult {
    let s = setup().await?;
    s.h.app
        .set_account_login_token_grace(Duration::from_secs(600));
    s.host.push(Script {
        chunks: vec![format!("{TOKEN}\r\nStore this token securely.\r\n")],
        exit: None,
        ..Script::default()
    });
    let (account_id, login_id) = create(&s, "anthropic", "Claude unsure").await?;
    let ctx = ctx_of(&s.h, &s.admin).await?;
    // Wait until the token was seen: its output reached the viewers (snapshot or live).
    let mut sub = s.h.app.login_terminal(&ctx, &login_id)?.subscribe();
    let seen = async {
        let mut out = String::from_utf8_lossy(&sub.snapshot).into_owned();
        while !out.contains("Store this token") {
            if let HubFrame::Output { bytes, .. } = sub.rx.recv().await? {
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    tokio::time::timeout(Duration::from_secs(10), seen).await??;
    let r = call(
        &s.h.router,
        Method::POST,
        &format!("/api/admin/accounts/login/{login_id}/cancel"),
        None,
        &s.admin,
    )
    .await?;
    assert_eq!(r.body, json!({"status": "ok"}), "{}", r.body);
    let record =
        s.h.store
            .account(&AccountId::parse(&account_id)?)
            .await?
            .ok_or("account")?;
    assert_eq!(record.status.as_str(), "ok");
    assert_eq!(
        s.h.app
            .deps
            .crypto
            .open(&record.credential.ok_or("credential")?)?,
        TOKEN.as_bytes()
    );
    Ok(())
}

/// SPEC-243 LGN-001: an incomplete token (fewer than 20 characters after the prefix) does not
/// end the sign-in; it runs on until it really ends.
#[tokio::test]
async fn a_partial_token_does_not_end_the_sign_in() -> TestResult {
    let s = setup().await?;
    s.h.app
        .set_account_login_token_grace(Duration::from_millis(20));
    s.h.app
        .set_account_login_timeout(Duration::from_millis(400));
    s.host.push(Script {
        chunks: vec!["Waiting: sk-ant-oat01-short\r\n".into()],
        exit: None,
        ..Script::default()
    });
    let mut events = s.h.app.events.subscribe();
    let (account_id, _) = create(&s, "anthropic", "Claude partial").await?;
    let account = account_settled(&mut events, &account_id).await?;
    assert_eq!(account["status"], "error");
    let message = account["last_error"].as_str().ok_or("last_error")?;
    assert!(message.contains("timed out"), "{message}");
    Ok(())
}

/// SPEC-243 LGN-002: closing a running sign-in cancels it on the server; the account keeps its
/// state, a second cancel of the ended sign-in changes nothing, and only administrators may.
#[tokio::test]
async fn a_running_sign_in_can_be_cancelled() -> TestResult {
    let s = setup().await?;
    configure_github_app(&s.h).await?;
    s.host.push(Script {
        chunks: vec!["Waiting for the browser...\r\n".into()],
        exit: None,
        ..Script::default()
    });
    let (account_id, login_id) = create(&s, "anthropic", "Claude closed").await?;
    let ctx = ctx_of(&s.h, &s.admin).await?;
    let mut sub = s.h.app.login_terminal(&ctx, &login_id)?.subscribe();
    let path = format!("/api/admin/accounts/login/{login_id}/cancel");

    let alice = admit_and_sign_in(&s.h, &s.admin, "alice", 11, false).await?;
    let refused = call(&s.h.router, Method::POST, &path, None, &alice).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);

    let r = call(&s.h.router, Method::POST, &path, None, &s.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.body,
        json!({"status": "cancelled"}),
        "it waited for the end"
    );
    let (_, status, _) = viewer_output(&mut sub).await?;
    assert_eq!(status, "cancelled");
    let log = s.host.log();
    assert!(log.iter().any(|l| l == "signal:Terminate"), "{log:?}");
    assert!(!login_dir(&s, &login_id).exists());
    let id = AccountId::parse(&account_id)?;
    assert_eq!(
        s.h.store
            .account(&id)
            .await?
            .ok_or("account")?
            .status
            .as_str(),
        "pending_login",
        "a cancelled sign-in does not touch the account"
    );

    let again = call(&s.h.router, Method::POST, &path, None, &s.admin).await?;
    assert_eq!(
        again.status,
        StatusCode::OK,
        "an ended sign-in: nothing to do"
    );
    assert_eq!(again.body, json!({"status": "cancelled"}));
    let unknown = call(
        &s.h.router,
        Method::POST,
        "/api/admin/accounts/login/nope/cancel",
        None,
        &s.admin,
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn one_login_per_account_bounded_logins_and_delete_cancels() -> TestResult {
    let s = setup().await?;
    let ctx = ctx_of(&s.h, &s.admin).await?;
    let (account_id, first) = create(&s, "anthropic", "Claude #3").await?;
    let mut first_view = s.h.app.login_terminal(&ctx, &first)?.subscribe();

    // A second login for the same account replaces the first one.
    let r = call(
        &s.h.router,
        Method::POST,
        &format!("/api/admin/accounts/{account_id}/relogin"),
        None,
        &s.admin,
    )
    .await?;
    let second = r.body["login_id"].as_str().ok_or("login_id")?.to_owned();
    assert_ne!(first, second);
    let (_, status, _) = viewer_output(&mut first_view).await?;
    assert_eq!(status, "cancelled");
    assert!(!login_dir(&s, &first).exists());
    let id = AccountId::parse(&account_id)?;
    assert_eq!(
        s.h.store
            .account(&id)
            .await?
            .ok_or("account")?
            .status
            .as_str(),
        "pending_login",
        "a cancelled login does not touch the account"
    );

    // At most eight logins run at once (the second login above is one of them).
    for i in 0..7 {
        create(&s, "anthropic", &format!("Claude extra {i}")).await?;
    }
    let refused = call(
        &s.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "anthropic", "label": "one too many"})),
        &s.admin,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    let list = call(
        &s.h.router,
        Method::GET,
        "/api/admin/accounts",
        None,
        &s.admin,
    )
    .await?;
    assert_eq!(
        list.body.as_array().map(Vec::len),
        Some(8),
        "the refused account was not left behind"
    );

    // Deleting the account cancels its running login.
    let mut second_view = s.h.app.login_terminal(&ctx, &second)?.subscribe();
    let del = call(
        &s.h.router,
        Method::DELETE,
        &format!("/api/admin/accounts/{account_id}"),
        None,
        &s.admin,
    )
    .await?;
    assert_eq!(del.status, StatusCode::NO_CONTENT);
    let (_, status, _) = viewer_output(&mut second_view).await?;
    assert_eq!(status, "cancelled");
    assert!(s.h.store.account(&id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn account_crud_permissions_usage_and_in_use_guard() -> TestResult {
    let s = setup().await?;
    configure_github_app(&s.h).await?;
    let alice = admit_and_sign_in(&s.h, &s.admin, "alice", 11, false).await?;
    let bob = admit_and_sign_in(&s.h, &s.admin, "bob", 12, true).await?;
    s.host.push(Script {
        chunks: vec![format!("{TOKEN}\r\n")],
        exit: Some(0),
        ..Script::default()
    });
    let mut events = s.h.app.events.subscribe();
    let (account_id, login_id) = create(&s, "anthropic", "Claude #1").await?;
    account_settled(&mut events, &account_id).await?;

    // Non-administrators are forbidden everywhere, including the login terminal.
    let path = format!("/api/admin/accounts/{account_id}");
    for (method, uri, body) in [
        (Method::GET, "/api/admin/accounts".to_owned(), None),
        (
            Method::POST,
            "/api/admin/accounts".to_owned(),
            Some(json!({"provider": "anthropic", "label": "x"})),
        ),
        (Method::PATCH, path.clone(), Some(json!({"label": "x"}))),
        (Method::DELETE, path.clone(), None),
        (Method::POST, format!("{path}/relogin"), None),
    ] {
        let r = call(&s.h.router, method.clone(), &uri, body, &alice).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    let alice_ctx = ctx_of(&s.h, &alice).await?;
    assert!(matches!(
        s.h.app.login_terminal(&alice_ctx, &login_id),
        Err(yulius_app::error::AppError::Forbidden(_))
    ));
    let anonymous = call(
        &s.h.router,
        Method::GET,
        "/api/admin/accounts",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    // An instance administrator manages accounts.
    let list = call(&s.h.router, Method::GET, "/api/admin/accounts", None, &bob).await?;
    assert_eq!(list.status, StatusCode::OK);
    let account = &list.body[0];
    assert_eq!(account["status"], "ok");
    assert!(
        account["usage"]["primary"].is_null(),
        "unknown usage stays null"
    );
    assert!(account["usage"]["observed_at"].is_null());
    assert_eq!(account["in_use_by"], 0);
    // The pool has no priorities or switches to manage (F 2026-10-10).
    for gone in ["priority", "enabled"] {
        assert!(account.get(gone).is_none(), "{gone} is gone: {account}");
    }

    let patched = call(
        &s.h.router,
        Method::PATCH,
        &path,
        Some(json!({"label": "Claude main"})),
        &bob,
    )
    .await?;
    assert_eq!(patched.status, StatusCode::OK);
    assert_eq!(patched.body["label"], "Claude main");
    for bad in [
        json!({"priority": 3}),
        json!({"enabled": false}),
        json!({"label": "Claude main", "priority": 1}),
        json!({"label": "  "}),
        json!({"label": "a\u{7}b"}),
    ] {
        let r = call(&s.h.router, Method::PATCH, &path, Some(bad.clone()), &bob).await?;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    let bad_provider = call(
        &s.h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "gemini", "label": "x"})),
        &bob,
    )
    .await?;
    assert_eq!(bad_provider.status, StatusCode::UNPROCESSABLE_ENTITY);
    let missing = call(
        &s.h.router,
        Method::PATCH,
        "/api/admin/accounts/nope",
        Some(json!({"label": "x"})),
        &bob,
    )
    .await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    // Usage snapshots come from the stored JSON; unknown windows stay null.
    let id = AccountId::parse(&account_id)?;
    s.h.store
        .set_account_state(
            &id,
            yulius_app::model::AccountStatus::Limited,
            Some(r#"{"primary":{"used_percent":42.0,"window_minutes":300,"resets_at":1791334800000},"secondary":null,"observed_at":1791331200000}"#),
            None,
            Timestamp(1_791_331_200_000),
        )
        .await?;

    // A live task uses the account: deletion is refused.
    insert_live_attempt(&s.h, &id).await?;
    let list = call(&s.h.router, Method::GET, "/api/admin/accounts", None, &bob).await?;
    let account = &list.body[0];
    assert_eq!(account["status"], "limited");
    assert_eq!(account["in_use_by"], 1);
    assert_eq!(account["usage"]["primary"]["used_percent"], 42.0);
    assert_eq!(account["usage"]["primary"]["window_minutes"], 300);
    assert_eq!(
        account["usage"]["primary"]["resets_at"],
        "2026-10-07T01:00:00Z"
    );
    assert!(account["usage"]["secondary"].is_null());
    assert_eq!(account["usage"]["observed_at"], "2026-10-07T00:00:00Z");
    let refused = call(&s.h.router, Method::DELETE, &path, None, &bob).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(s.h.store.account(&id).await?.is_some());
    Ok(())
}

async fn insert_live_attempt(h: &support::Harness, account: &AccountId) -> TestResult {
    support::insert_sandbox(h, "s1", "studio", 20000).await?;
    let task = h
        .store
        .insert_task(&TaskRecord {
            id: TaskId::parse("t-live")?,
            sandbox_id: SandboxId::parse("s1")?,
            number: 0,
            title: "live".into(),
            prompt: String::new(),
            harness: Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: UserId::parse("u")?,
            owner: UserId::parse("u")?,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: yulius_app::model::TaskMode::Prompt,
            supertask: false,
        })
        .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a-live")?,
            task_id: task.id,
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: Some(account.clone()),
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/nonexistent".into(),
            token_hash: "live-hash".into(),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(())
}

/// Real loopback listener + WebSocket client: the browser path of the login terminal.
#[tokio::test]
async fn login_terminal_websocket_streams_masked_output_and_accepts_input() -> TestResult {
    let s = setup().await?;
    configure_github_app(&s.h).await?;
    let alice = admit_and_sign_in(&s.h, &s.admin, "alice", 11, false).await?;
    s.host.push(Script {
        chunks: vec![
            "Paste code here if prompted > ".into(),
            format!("\r\n{}", &TOKEN[..20]),
            format!("{}\r\nStore this token securely.\r\n", &TOKEN[20..]),
        ],
        exit: Some(0),
        gated: true,
    });
    let (_, login_id) = create(&s, "anthropic", "Claude WS").await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let router = s.h.router.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await;
    });
    let url = format!("ws://{addr}/api/admin/accounts/login/{login_id}/terminal");
    let request =
        |creds: &Creds,
         origin: &str|
         -> Result<tungstenite::handshake::client::Request, Box<dyn std::error::Error>> {
            let mut req = url.as_str().into_client_request()?;
            let cookie = support::cookie_header(creds.cookie.as_deref().ok_or("cookie")?);
            req.headers_mut().insert("cookie", cookie.parse()?);
            req.headers_mut().insert("origin", origin.parse()?);
            Ok(req)
        };

    // Cross-origin and non-administrator upgrades are refused.
    assert!(
        tokio_tungstenite::connect_async(request(&s.admin, "https://evil.test")?)
            .await
            .is_err()
    );
    assert!(
        tokio_tungstenite::connect_async(request(&alice, "https://yulius.test")?)
            .await
            .is_err()
    );

    let (mut ws, _) =
        tokio_tungstenite::connect_async(request(&s.admin, "https://yulius.test")?).await?;
    let mut output = Vec::new();
    let mut saw_snapshot = false;
    let mut exit = Value::Null;
    let mut released = false;
    let run = async {
        while let Some(message) = ws.next().await {
            match message? {
                tungstenite::Message::Text(text) => {
                    let v: Value = serde_json::from_str(&text)?;
                    if v["type"] == "state" && !released {
                        assert_eq!(v["cols"], 500);
                        assert_eq!(v["can_write"], true);
                        ws.send(tungstenite::Message::Text(
                            json!({"type": "hello", "last_seq": null, "cols": 120, "rows": 30})
                                .to_string()
                                .into(),
                        ))
                        .await?;
                        ws.send(tungstenite::Message::Text(
                            json!({"type": "input", "data": "pasted\r"})
                                .to_string()
                                .into(),
                        ))
                        .await?;
                        // Let the process produce output only after the input arrived.
                        s.host.input_seen.notified().await;
                        released = true;
                        s.host.release.add_permits(1);
                    }
                    if v["type"] == "exit" {
                        exit = v;
                        break;
                    }
                }
                tungstenite::Message::Binary(bytes) => {
                    let (kind, _, payload) = decode_binary(&bytes).ok_or("frame")?;
                    if kind == KIND_SNAPSHOT {
                        saw_snapshot = true;
                    } else if kind == KIND_OUTPUT {
                        output.extend_from_slice(payload);
                    }
                }
                _ => {}
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    tokio::time::timeout(Duration::from_secs(10), run).await??;
    let _ = ws.close(None).await;

    // SPEC-243 LGN-007: an unknown or expired sign-in is said in band, with a close the browser
    // does not retry (4003), instead of a handshake 404 it would retry for minutes.
    let mut gone = url.replace(&login_id, "expired").into_client_request()?;
    let cookie = support::cookie_header(s.admin.cookie.as_deref().ok_or("cookie")?);
    gone.headers_mut().insert("cookie", cookie.parse()?);
    gone.headers_mut()
        .insert("origin", "https://yulius.test".parse()?);
    let (mut ws, _) = tokio_tungstenite::connect_async(gone).await?;
    let refusal = async {
        let mut error = Value::Null;
        let mut close = None;
        while let Some(message) = ws.next().await {
            match message? {
                tungstenite::Message::Text(text) => error = serde_json::from_str(&text)?,
                tungstenite::Message::Close(frame) => {
                    close = frame.map(|f| u16::from(f.code));
                    break;
                }
                _ => {}
            }
        }
        Ok::<_, Box<dyn std::error::Error>>((error, close))
    };
    let (error, close) = tokio::time::timeout(Duration::from_secs(10), refusal).await??;
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "not_found");
    assert_eq!(close, Some(4003));

    let _ = stop_tx.send(());
    let _ = server.await;

    let seen = String::from_utf8_lossy(&output);
    assert!(saw_snapshot, "a snapshot is sent on connect");
    assert!(
        !seen.contains(TOKEN) && !seen.contains("SYNTHETICabcdefghij"),
        "{seen}"
    );
    assert!(seen.contains(MASK), "{seen}");
    assert_eq!(exit["status"], "ok");
    assert!(s.host.log().contains(&"input:pasted\r".to_owned()));
    Ok(())
}

/// An open login terminal over a real loopback connection.
type LoginSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Opens the login terminal of `login_id` with a cookie session or a bearer token.
async fn open_login_terminal(
    addr: std::net::SocketAddr,
    login_id: &str,
    creds: &Creds,
) -> Result<LoginSocket, Box<dyn std::error::Error>> {
    let url = format!("ws://{addr}/api/admin/accounts/login/{login_id}/terminal");
    let mut req = url.into_client_request()?;
    if let Some(cookie) = creds.cookie.as_deref() {
        req.headers_mut()
            .insert("cookie", support::cookie_header(cookie).parse()?);
    }
    if let Some(token) = creds.bearer.as_deref() {
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse()?);
    }
    req.headers_mut()
        .insert("origin", "https://yulius.test".parse()?);
    let (ws, _) = tokio_tungstenite::connect_async(req).await?;
    Ok(ws)
}

/// Waits (bounded) for the socket's close frame and returns its code and reason.
async fn login_terminal_closed(
    ws: &mut LoginSocket,
) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let wait = async {
        while let Some(message) = ws.next().await {
            if let tungstenite::Message::Close(frame) = message? {
                return Ok::<_, Box<dyn std::error::Error>>(
                    frame.map_or((1005, String::new()), |f| {
                        (u16::from(f.code), f.reason.to_string())
                    }),
                );
            }
        }
        Err("the connection ended without a close frame".into())
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .map_err(|_| "the login terminal stayed open")?
}

/// Asserts the socket still answers: a `hello` gets a `state` back (bounded).
async fn login_terminal_answers(ws: &mut LoginSocket) -> TestResult {
    ws.send(tungstenite::Message::Text(
        json!({"type": "hello", "last_seq": null, "cols": 80, "rows": 24})
            .to_string()
            .into(),
    ))
    .await?;
    let wait = async {
        while let Some(message) = ws.next().await {
            match message? {
                tungstenite::Message::Text(text) => {
                    let v: Value = serde_json::from_str(&text)?;
                    if v["type"] == "state" {
                        return Ok::<(), Box<dyn std::error::Error>>(());
                    }
                }
                tungstenite::Message::Close(frame) => {
                    return Err(format!("closed: {frame:?}").into());
                }
                _ => {}
            }
        }
        Err("the connection ended".into())
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await??;
    Ok(())
}

/// #759 (SPEC-295 LTR-001..005): an open login terminal is re-checked like the other sockets. A
/// revoked API token, a signed-out session and a removed instance administration each close their
/// own terminal with 4003 within the re-check interval; the others stay open.
#[tokio::test]
async fn a_login_terminal_closes_once_its_sign_in_or_administration_ends() -> TestResult {
    use yulius_app::ports::store::UserStore as _;
    let host = Arc::new(FakeHost::default());
    let h = support::harness_with(support::HarnessOptions {
        git: Some(Arc::new(yulius::wiring::pending::UnavailableGit)),
        session_host: Some(host.clone()),
        configure: Some(Box::new(|c| c.viewer_recheck_ms = 200)),
        ..support::HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    let s = Setup { h, host, admin };
    configure_github_app(&s.h).await?;
    let alice = admit_and_sign_in(&s.h, &s.admin, "alice", 11, true).await?;
    let bob = admit_and_sign_in(&s.h, &s.admin, "bob", 12, true).await?;
    let made = call(
        &s.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": "laptop", "scope": "operate"})),
        &alice,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let token = Creds {
        bearer: Some(made.body["token"].as_str().ok_or("token")?.to_owned()),
        ..Creds::default()
    };
    let token_id = made.body["view"]["id"]
        .as_str()
        .ok_or("token id")?
        .to_owned();
    // A sign-in that runs until it is stopped.
    s.host.push(Script::default());
    let (_, login_id) = create(&s, "anthropic", "Claude recheck").await?;

    let served = support::serve(s.h.router.clone()).await?;
    let mut by_cookie = open_login_terminal(served.addr, &login_id, &alice).await?;
    let mut by_token = open_login_terminal(served.addr, &login_id, &token).await?;
    let mut by_bob = open_login_terminal(served.addr, &login_id, &bob).await?;
    for ws in [&mut by_cookie, &mut by_token, &mut by_bob] {
        login_terminal_answers(ws).await?;
    }

    // Revoking the operate token ends the terminal opened with it, not the others.
    let revoked = call(
        &s.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{token_id}"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(
        login_terminal_closed(&mut by_token).await?,
        (4003, "signed out".to_owned())
    );
    login_terminal_answers(&mut by_cookie).await?;
    login_terminal_answers(&mut by_bob).await?;

    // Signing out ends the terminal opened with that session.
    let logout = call(&s.h.router, Method::POST, "/api/auth/logout", None, &alice).await?;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert_eq!(
        login_terminal_closed(&mut by_cookie).await?,
        (4003, "signed out".to_owned())
    );
    login_terminal_answers(&mut by_bob).await?;

    // Removing bob's instance administration ends his terminal although he is still signed in.
    let bob_id = s.h.store.user_by_login("bob").await?.ok_or("bob")?.id;
    let demoted = call(
        &s.h.router,
        Method::PATCH,
        &format!("/api/admin/users/{bob_id}"),
        Some(json!({"is_instance_admin": false})),
        &s.admin,
    )
    .await?;
    assert_eq!(demoted.status, StatusCode::OK, "{}", demoted.body);
    assert_eq!(
        login_terminal_closed(&mut by_bob).await?,
        (4003, "access revoked".to_owned())
    );
    Ok(())
}
