//! A subscription's name and the email of its Claude account (SPEC-244): the email is read from
//! Claude's profile when a sign-in stores a credential and once for subscriptions signed in
//! before; only administrators rename; every view that names a subscription carries its email.

use crate::support;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    HarnessOptions, TestResult, admit_and_sign_in, call, configure_github_app, harness_with,
    setup_superadmin_creds,
};
use tokio::sync::mpsc;
use yulius_app::error::AppResult;
use yulius_app::model::{
    AccountRecord, AccountStatus, AttemptRecord, LaunchKind, Sealed, Secret, TaskOrigin, TaskRecord,
};
use yulius_app::ports::session::{
    AttachedRun, LaunchPlan, RunEvent, RunExit, RunInspection, RunReader, RunSignal, RunWelcome,
    RunWriter, SessionHost, SpawnedRun,
};
use yulius_app::ports::store::{AccountStore, TaskStore};
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::AccountUsage;

/// Synthetic tokens (realistic shape, not real credentials): one whose profile has an email, one
/// whose profile is refused (a `setup-token` without `user:profile`).
const WITH_EMAIL: &str = concat!(
    "sk-ant-oat01-",
    "SYNTHETICwithemail0123456789abcdefghijklmnopqrstu-test"
);
const REFUSED: &str = concat!(
    "sk-ant-oat01-",
    "SYNTHETICrefused000123456789abcdefghijklmnopqrstu-test"
);

/// Claude's profile as the fake sees it: per credential, an email, a refusal or no answer.
#[derive(Default)]
struct FakeProfile {
    calls: Mutex<HashMap<String, usize>>,
}

impl FakeProfile {
    fn calls(&self, credential: &str) -> usize {
        self.calls
            .lock()
            .map(|c| c.get(credential).copied().unwrap_or(0))
            .unwrap_or(0)
    }
}

#[async_trait]
impl UsageProbe for FakeProfile {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
        Err(UsageFailure::not_shared())
    }

    async fn account_email(
        &self,
        _: Provider,
        credential: &Secret,
    ) -> Result<Option<String>, UsageFailure> {
        let token = credential.expose().to_owned();
        if let Ok(mut calls) = self.calls.lock() {
            *calls.entry(token.clone()).or_default() += 1;
        }
        match token.as_str() {
            WITH_EMAIL | "old-with-email" => Ok(Some("pawel@example.com".into())),
            REFUSED | "old-refused" => Err(UsageFailure::not_shared()),
            "old-no-email" => Ok(None),
            "old-bad-email" => Ok(Some("not an address".into())),
            _ => Err(UsageFailure::new("Claude did not answer just now.")),
        }
    }
}

/// A process host whose every sign-in prints the next queued token and exits 0.
#[derive(Default)]
struct SignInHost {
    tokens: Mutex<Vec<&'static str>>,
    runs: Mutex<HashMap<std::path::PathBuf, &'static str>>,
}

struct Reader(mpsc::Receiver<RunEvent>);

#[async_trait]
impl RunReader for Reader {
    async fn next(&mut self) -> AppResult<Option<RunEvent>> {
        Ok(self.0.recv().await)
    }
}

struct Writer;

#[async_trait]
impl RunWriter for Writer {
    async fn input(&mut self, _: &[u8]) -> AppResult<()> {
        Ok(())
    }
    async fn resize(&mut self, _: u16, _: u16) -> AppResult<()> {
        Ok(())
    }
    async fn signal(&mut self, _: RunSignal) -> AppResult<()> {
        Ok(())
    }
    async fn shutdown(&mut self, _: u32) -> AppResult<()> {
        Ok(())
    }
}

#[async_trait]
impl SessionHost for SignInHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        _: &str,
        _: &str,
        _: &LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        let token = self
            .tokens
            .lock()
            .ok()
            .and_then(|mut t| (!t.is_empty()).then(|| t.remove(0)))
            .unwrap_or(REFUSED);
        if let Ok(mut runs) = self.runs.lock() {
            runs.insert(run_dir.to_path_buf(), token);
        }
        Ok(SpawnedRun {
            host_pid: 1,
            child_pid: 2,
        })
    }

    async fn attach(&self, run_dir: &Path, _: &[u8; 32]) -> AppResult<AttachedRun> {
        let token = self
            .runs
            .lock()
            .ok()
            .and_then(|r| r.get(run_dir).copied())
            .unwrap_or(REFUSED);
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let output = format!("Your OAuth token (valid for 1 year):\r\n\r\n{token}\r\n\r\n");
            let _ = tx
                .send(RunEvent::Output {
                    seq: 1,
                    bytes: output.into_bytes(),
                })
                .await;
            let _ = tx
                .send(RunEvent::Exit(RunExit {
                    code: Some(0),
                    signal: None,
                    spawn_failed: false,
                    usage: None,
                }))
                .await;
        });
        Ok(AttachedRun {
            welcome: RunWelcome {
                child_pid: 2,
                seq: 0,
                cols: 500,
                rows: 40,
                snapshot: None,
            },
            reader: Box::new(Reader(rx)),
            writer: Box::new(Writer),
        })
    }

    fn inspect(&self, _: &Path) -> RunInspection {
        RunInspection::Missing
    }

    fn terminate(&self, _: &Path) -> AppResult<()> {
        Ok(())
    }
}

/// Waits (bounded) for an `account.updated` of `id` that satisfies `done`.
async fn account_event(
    rx: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
    id: &str,
    done: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    let wait = async {
        loop {
            let event = serde_json::to_value(&rx.recv().await?.event)?;
            if event["type"] == "account.updated"
                && event["account"]["id"] == id
                && done(&event["account"])
            {
                return Ok::<Value, Box<dyn std::error::Error>>(event["account"].clone());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await?
}

async fn signed_in_account(
    h: &support::Harness,
    id: &str,
    credential: &str,
    email: Option<&str>,
) -> TestResult {
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(credential.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            email: email.map(str::to_owned),
            email_checked_at: email.map(|_| Timestamp(1)),
            email_typed: false,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(())
}

/// SUB-003: a sign-in stores the email Claude's profile gives for the new credential; a later
/// sign-in whose profile is refused drops it (it may be another Claude account), and the sign-in
/// itself succeeds either way.
#[tokio::test]
async fn a_sign_in_stores_the_email_of_its_claude_account() -> TestResult {
    let host = Arc::new(SignInHost::default());
    if let Ok(mut tokens) = host.tokens.lock() {
        tokens.extend([WITH_EMAIL, REFUSED]);
    }
    let profile = Arc::new(FakeProfile::default());
    let h = harness_with(HarnessOptions {
        session_host: Some(host.clone()),
        usage: Some(profile.clone()),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    let mut events = h.app.events.subscribe();

    let created = call(
        &h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "anthropic", "label": "work Max"})),
        &admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert!(
        created.body["account"]["email"].is_null(),
        "unknown before the sign-in"
    );
    let id = created.body["account"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    let account = account_event(&mut events, &id, |a| !a["email"].is_null()).await?;
    assert_eq!(account["email"], "pawel@example.com");
    assert_eq!(account["status"], "ok");
    let record = h
        .store
        .account(&AccountId::parse(&id)?)
        .await?
        .ok_or("account")?;
    assert!(record.email_checked_at.is_some());
    assert_eq!(profile.calls(WITH_EMAIL), 1);

    let relogin = call(
        &h.router,
        Method::POST,
        &format!("/api/admin/accounts/{id}/relogin"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(relogin.status, StatusCode::OK, "{}", relogin.body);
    let wait = async {
        loop {
            if profile.calls(REFUSED) == 1 {
                let record = h.store.account(&AccountId::parse(&id)?).await?;
                if record
                    .as_ref()
                    .is_some_and(|r| r.email_checked_at.is_some())
                {
                    return Ok::<_, Box<dyn std::error::Error>>(record);
                }
            }
            tokio::task::yield_now().await;
        }
    };
    let record = tokio::time::timeout(Duration::from_secs(10), wait)
        .await??
        .ok_or("account")?;
    assert_eq!(record.email, None, "the older sign-in's email is gone");
    assert_eq!(
        record.status,
        AccountStatus::Ok,
        "the sign-in still succeeded"
    );
    assert_eq!(
        h.app
            .deps
            .crypto
            .open(record.credential.as_ref().ok_or("credential")?)?,
        REFUSED.as_bytes()
    );
    let list = call(&h.router, Method::GET, "/api/admin/accounts", None, &admin).await?;
    assert!(list.body[0]["email"].is_null(), "{}", list.body);
    Ok(())
}

/// SPEC-464 UCL-004: a model's limit Claude reported to a sign-in is that sign-in's. No usage
/// reading ends it before its reset, so a new sign-in of the subscription (another Claude
/// account, or the same one with credits again) drops it; the subscription's own 5-hour and
/// weekly readings stay until the next reading replaces them.
#[tokio::test]
async fn a_new_sign_in_drops_the_models_limit_of_the_previous_one() -> TestResult {
    let host = Arc::new(SignInHost::default());
    if let Ok(mut tokens) = host.tokens.lock() {
        tokens.push(REFUSED);
    }
    let h = harness_with(HarnessOptions {
        session_host: Some(host.clone()),
        usage: Some(Arc::new(FakeProfile::default())),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    signed_in_account(&h, "a-out", "old-refused", None).await?;
    let id = AccountId::parse("a-out")?;
    let stored = json!({
        "secondary": {"used_percent": 61.0, "window_minutes": 10_080, "resets_at": 4_000_000_000_000_i64},
        "weekly_fable": {"used_percent": 100.0, "window_minutes": 10_080, "resets_at": 4_100_000_000_000_i64},
        "weekly_opus": {"used_percent": 100.0, "window_minutes": 10_080, "resets_at": 4_100_000_000_000_i64},
    });
    h.store
        .set_account_state(
            &id,
            AccountStatus::Limited,
            Some(&stored.to_string()),
            None,
            Timestamp(2),
        )
        .await?;

    let relogin = call(
        &h.router,
        Method::POST,
        "/api/admin/accounts/a-out/relogin",
        None,
        &admin,
    )
    .await?;
    assert_eq!(relogin.status, StatusCode::OK, "{}", relogin.body);
    let wait = async {
        loop {
            let record = h.store.account(&id).await?.ok_or("account")?;
            let credential = record.credential.as_ref().ok_or("credential")?;
            if h.app.deps.crypto.open(credential)? == REFUSED.as_bytes() {
                return Ok::<_, Box<dyn std::error::Error>>(record);
            }
            tokio::task::yield_now().await;
        }
    };
    let record = tokio::time::timeout(Duration::from_secs(10), wait).await??;
    let usage: Value = serde_json::from_str(record.usage_json.as_deref().ok_or("usage")?)?;
    assert!(usage["weekly_fable"].is_null(), "{usage}");
    assert!(usage["weekly_opus"].is_null(), "{usage}");
    assert_eq!(usage["secondary"]["used_percent"], 61.0, "{usage}");
    assert_eq!(record.status, AccountStatus::Ok);
    Ok(())
}

/// A provider that reads every sign-in's usage (the 5-hour window at 42 %) and knows no email.
struct ReadingProbe;

#[async_trait]
impl UsageProbe for ReadingProbe {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
        Ok(AccountUsage {
            primary: Some(yulius_domain::usage::UsageWindow {
                used_percent: Some(42.0),
                window_minutes: Some(300),
                resets_at: None,
            }),
            ..AccountUsage::default()
        })
    }

    /// Asked about a stored limit by a refresh (SPEC-473): the model is still refused, with no
    /// reset named, so the limit stays as it is stored.
    async fn model_answer(
        &self,
        _: Provider,
        _: &Secret,
        _: &str,
    ) -> Result<yulius_domain::usage::ModelAnswer, UsageFailure> {
        Ok(yulius_domain::usage::ModelAnswer {
            refused: Some(yulius_domain::usage::ReportedLimit {
                scope: yulius_domain::usage::LimitScope::Model(None),
                ..yulius_domain::usage::ReportedLimit::default()
            }),
            ..yulius_domain::usage::ModelAnswer::default()
        })
    }

    async fn account_email(&self, _: Provider, _: &Secret) -> Result<Option<String>, UsageFailure> {
        Ok(None)
    }
}

/// SPEC-464 UCL-004: a new sign-in takes the lock every change of the stored usage takes. A
/// provider's reading of the subscription is held once it has read the stored usage, which
/// still has the previous sign-in's model limit; the sign-in finishes meanwhile; the reading
/// goes on. Unlocked, the sign-in would drop the limit during the hold and the reading would
/// put it back: the new sign-in would start with the old one's limit. Locked, the sign-in waits
/// for the reading and drops what it stored.
///
/// Half a second is the sign-in's chance to finish while the reading is held: locked it waits,
/// so that wait always runs out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sign_in_during_a_usage_reading_still_drops_the_models_limit() -> TestResult {
    let host = Arc::new(SignInHost::default());
    if let Ok(mut tokens) = host.tokens.lock() {
        tokens.push(REFUSED);
    }
    let h = harness_with(HarnessOptions {
        session_host: Some(host.clone()),
        usage: Some(Arc::new(ReadingProbe)),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    let name = "acc-held-sign-in";
    signed_in_account(&h, name, "old-refused", None).await?;
    let id = AccountId::parse(name)?;
    let in_two_days = h.app.now().0 + 2 * 24 * 3_600_000;
    let limited = json!({"weekly_fable":
        {"used_percent": 100.0, "window_minutes": 10_080, "resets_at": in_two_days}});
    h.store
        .set_account_state(
            &id,
            AccountStatus::Limited,
            Some(&limited.to_string()),
            None,
            Timestamp(2),
        )
        .await?;
    let signed_in = || async {
        let record = h.store.account(&id).await.ok().flatten()?;
        let opened = h.app.deps.crypto.open(record.credential.as_ref()?).ok()?;
        (opened == REFUSED.as_bytes()).then_some(record)
    };

    let (read, go_on) = support::hold_point(yulius_app::race::Point::UsageRead, name);
    let reading = {
        let (router, admin) = (h.router.clone(), admin.clone());
        tokio::spawn(async move {
            call(
                &router,
                Method::POST,
                &format!("/api/admin/accounts/{name}/usage"),
                None,
                &admin,
            )
            .await
            .map(|answer| answer.status)
            .map_err(|err| err.to_string())
        })
    };
    support::point_reached(read).await?;
    let relogin = call(
        &h.router,
        Method::POST,
        &format!("/api/admin/accounts/{name}/relogin"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(relogin.status, StatusCode::OK, "{}", relogin.body);
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    let mut signed_in_meanwhile = false;
    while !signed_in_meanwhile && std::time::Instant::now() < deadline {
        signed_in_meanwhile = signed_in().await.is_some();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    go_on.send(())?;
    assert_eq!(reading.await??, StatusCode::OK);
    let record = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(record) = signed_in().await {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let usage: Value = serde_json::from_str(record.usage_json.as_deref().ok_or("usage")?)?;
    assert!(
        usage["weekly_fable"].is_null(),
        "the reading put the previous sign-in's limit back (signed in while it was held: \
         {signed_in_meanwhile}): {usage}"
    );
    assert_eq!(usage["primary"]["used_percent"], 42.0, "{usage}");
    Ok(())
}

/// SUB-004: subscriptions signed in before emails were read get theirs once from the background
/// round. A definite answer (an email, none, a refusal, something that is not an address) is not
/// asked again; a temporary failure is, after its back-off; a known email is left alone.
#[tokio::test]
async fn subscriptions_signed_in_before_get_their_email_once() -> TestResult {
    let profile = Arc::new(FakeProfile::default());
    let h = harness_with(HarnessOptions {
        usage: Some(profile.clone()),
        ..HarnessOptions::default()
    })?;
    for (id, credential) in [
        ("a-email", "old-with-email"),
        ("a-refused", "old-refused"),
        ("a-none", "old-no-email"),
        ("a-bad", "old-bad-email"),
        ("a-flaky", "old-flaky"),
    ] {
        signed_in_account(&h, id, credential, None).await?;
    }
    signed_in_account(&h, "a-known", "old-known", Some("known@example.com")).await?;

    let mut not_before = HashMap::new();
    h.app.refresh_usage_round(&mut not_before).await;
    h.app.refresh_usage_round(&mut not_before).await;

    let email = |id: &'static str| {
        let h = &h;
        async move {
            let record = h.store.account(&AccountId::parse(id)?).await?.ok_or(id)?;
            Ok::<_, Box<dyn std::error::Error>>((record.email, record.email_checked_at.is_some()))
        }
    };
    assert_eq!(
        email("a-email").await?,
        (Some("pawel@example.com".into()), true)
    );
    assert_eq!(email("a-refused").await?, (None, true));
    assert_eq!(email("a-none").await?, (None, true));
    assert_eq!(
        email("a-bad").await?,
        (None, true),
        "only an address is stored"
    );
    assert_eq!(email("a-flaky").await?, (None, false));
    assert_eq!(
        email("a-known").await?,
        (Some("known@example.com".into()), true)
    );
    for credential in [
        "old-with-email",
        "old-refused",
        "old-no-email",
        "old-bad-email",
    ] {
        assert_eq!(profile.calls(credential), 1, "{credential} asked once");
    }
    assert_eq!(
        profile.calls("old-flaky"),
        1,
        "backed off after a temporary failure"
    );
    assert_eq!(profile.calls("old-known"), 0);

    // After the back-off (30 minutes), the temporary failure is asked again.
    h.clock
        .0
        .fetch_add(31 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    h.app.refresh_usage_round(&mut not_before).await;
    assert_eq!(profile.calls("old-flaky"), 2);
    assert_eq!(profile.calls("old-with-email"), 1);
    Ok(())
}

static TASKS: AtomicUsize = AtomicUsize::new(0);

async fn task_on(
    h: &support::Harness,
    account: &AccountId,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let n = TASKS.fetch_add(1, Ordering::SeqCst);
    support::insert_sandbox(h, "s1", "studio", 20000).await?;
    let task = h
        .store
        .insert_task(&TaskRecord {
            id: TaskId::parse(&format!("t-email-{n}"))?,
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
            id: AttemptId::parse(&format!("a-email-{n}"))?,
            task_id: task.id.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: Some(account.clone()),
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            model: None,
            state: AttemptState::Live,
            run_dir: "/nonexistent".into(),
            token_hash: format!("email-hash-{n}"),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(task.id)
}

/// SUB-001, SUB-002, SUB-005: an administrator's rename persists and every view that names the
/// subscription shows the new name, with the email for administrators (UI-022); anyone else is
/// refused the rename and sees the new name without the email.
#[tokio::test]
async fn only_administrators_rename_and_every_view_shows_name_and_email() -> TestResult {
    let h = harness_with(HarnessOptions::default())?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let member = admit_and_sign_in(&h, &admin, "member", 4242, false).await?;
    h.github.set_perm(
        "member",
        "Quartio-Studio/studio",
        GitHubPermissions {
            pull: true,
            ..Default::default()
        },
    );
    let instance_admin = admit_and_sign_in(&h, &admin, "operator", 4243, true).await?;
    signed_in_account(&h, "a-work", "token", Some("pawel@example.com")).await?;
    signed_in_account(&h, "a-other", "token-2", None).await?;
    let id = AccountId::parse("a-work")?;
    let task = task_on(&h, &id).await?;

    let refused = call(
        &h.router,
        Method::PATCH,
        "/api/admin/accounts/a-work",
        Some(json!({"label": "mine now"})),
        &member,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let empty = call(
        &h.router,
        Method::PATCH,
        "/api/admin/accounts/a-work",
        Some(json!({"label": "   "})),
        &admin,
    )
    .await?;
    assert_eq!(
        empty.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        empty.body
    );
    // An instance administrator manages subscriptions as the superadmin does (ACC-009).
    let renamed = call(
        &h.router,
        Method::PATCH,
        "/api/admin/accounts/a-work",
        Some(json!({"label": "  work Max "})),
        &instance_admin,
    )
    .await?;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.body);
    assert_eq!(
        (
            renamed.body["label"].as_str(),
            renamed.body["email"].as_str()
        ),
        (Some("work Max"), Some("pawel@example.com"))
    );
    assert_eq!(
        h.store.account(&id).await?.ok_or("account")?.label,
        "work Max"
    );

    let named = |list: &Value| -> Option<(Value, Value)> {
        list.as_array()?
            .iter()
            .find(|a| a["label"] == "work Max")
            .map(|a| (a["label"].clone(), a["email"].clone()))
    };
    let with_email = Some((json!("work Max"), json!("pawel@example.com")));
    let accounts = call(&h.router, Method::GET, "/api/admin/accounts", None, &admin).await?;
    assert_eq!(named(&accounts.body), with_email);
    for (who, expected) in [
        (&admin, with_email.clone()),
        (&member, Some((json!("work Max"), Value::Null))),
    ] {
        let status = call(&h.router, Method::GET, "/api/status", None, who).await?;
        assert_eq!(named(&status.body["accounts"]), expected, "{}", status.body);
        let capacity = call(&h.router, Method::GET, "/api/capacity", None, who).await?;
        assert_eq!(
            named(&capacity.body["accounts"]),
            expected,
            "{}",
            capacity.body
        );
        let other = capacity.body["accounts"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["label"] == "a-other").cloned())
            .ok_or("the other subscription")?;
        assert!(other["email"].is_null(), "unknown stays null");
    }

    let detail = call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{task}"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.body);
    assert_eq!(detail.body["account_label"], "work Max");
    assert_eq!(detail.body["account_email"], "pawel@example.com");
    assert_eq!(detail.body["attempts"][0]["account_label"], "work Max");
    assert_eq!(
        detail.body["attempts"][0]["account_email"],
        "pawel@example.com"
    );
    // A member who reads the task sees the subscription's name without its email (UI-022).
    let seen = call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{task}"),
        None,
        &member,
    )
    .await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(seen.body["account_label"], "work Max");
    assert_eq!(seen.body["attempts"][0]["account_label"], "work Max");
    assert!(
        seen.body.get("account_email").is_none()
            && seen.body["attempts"][0].get("account_email").is_none(),
        "{}",
        seen.body
    );
    // Task lists and live updates go to everyone who sees the task: no email there.
    let listed = h
        .app
        .task_summary(&h.store.task(&task).await?.ok_or("task")?)
        .await?;
    assert_eq!(listed.account_label.as_deref(), Some("work Max"));
    assert_eq!(listed.account_email, None);
    Ok(())
}

/// A profile read that a re-login overtakes: the account is signed in again (perhaps as another
/// Claude account) while Claude answers for the old credential.
struct RacedProfile {
    relogin: Mutex<Option<(yulius_store::SqliteStore, Sealed)>>,
}

#[async_trait]
impl UsageProbe for RacedProfile {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
        Err(UsageFailure::not_shared())
    }

    async fn account_email(
        &self,
        _: Provider,
        credential: &Secret,
    ) -> Result<Option<String>, UsageFailure> {
        if credential.expose() == "old-sign-in" {
            let relogin = self.relogin.lock().ok().and_then(|mut r| r.take());
            if let Some((store, fresh)) = relogin {
                let id = AccountId::parse("a-raced").map_err(|_| UsageFailure::new("id"))?;
                store
                    .set_account_credential(&id, &fresh, AccountStatus::Ok, Timestamp(3))
                    .await
                    .map_err(|_| UsageFailure::new("relogin"))?;
            }
            return Ok(Some("old@example.com".into()));
        }
        Err(UsageFailure::new("Claude did not answer just now."))
    }
}

/// SUB-003: the old sign-in's answer never lands on the new credential; the new sign-in's email
/// stays unknown until it is read for that credential.
#[tokio::test]
async fn an_answer_for_an_older_sign_in_is_not_stored() -> TestResult {
    let probe = Arc::new(RacedProfile {
        relogin: Mutex::new(None),
    });
    let h = harness_with(HarnessOptions {
        usage: Some(probe.clone()),
        ..HarnessOptions::default()
    })?;
    signed_in_account(&h, "a-raced", "old-sign-in", None).await?;
    let fresh = h.app.deps.crypto.seal(b"new-sign-in")?;
    if let Ok(mut relogin) = probe.relogin.lock() {
        *relogin = Some((h.store.clone(), fresh.clone()));
    }
    h.app.refresh_usage_round(&mut HashMap::new()).await;
    let record = h
        .store
        .account(&AccountId::parse("a-raced")?)
        .await?
        .ok_or("account")?;
    assert_eq!(record.credential, Some(fresh));
    assert_eq!(
        (record.email, record.email_checked_at),
        (None, None),
        "the old sign-in's email is not the new one's"
    );
    Ok(())
}

/// SPEC-463 EML-003, EML-005: an administrator types a subscription's email, changes it with the
/// name and removes it. Every view that names the subscription shows it to administrators only.
/// Anyone else is refused, a value that is not an address is refused, and a refused request
/// writes nothing. The audit trail says that the email changed, never the address.
#[tokio::test]
async fn an_administrator_types_changes_and_removes_a_subscriptions_email() -> TestResult {
    use yulius_app::ports::store::AuditStore as _;

    let h = harness_with(HarnessOptions::default())?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let member = admit_and_sign_in(&h, &admin, "member", 4242, false).await?;
    h.github.set_perm(
        "member",
        "Quartio-Studio/studio",
        GitHubPermissions {
            pull: true,
            ..Default::default()
        },
    );
    let instance_admin = admit_and_sign_in(&h, &admin, "operator", 4243, true).await?;
    signed_in_account(&h, "a-team", "token", None).await?;
    signed_in_account(&h, "a-claude", "token-2", Some("pawel@example.com")).await?;
    let id = AccountId::parse("a-team")?;
    let task = task_on(&h, &id).await?;
    let path = "/api/admin/accounts/a-team";

    let refused = call(
        &h.router,
        Method::PATCH,
        path,
        Some(json!({"email": "agent@example.com"})),
        &member,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let long = format!("{}@example.com", "a".repeat(250));
    for bad in [
        json!({}),
        json!({"email": null}),
        json!({"email": "not an address"}),
        json!({"email": "agent@"}),
        json!({"email": "a@b@example.com"}),
        json!({"email": "agent@exa\u{7}mple.com"}),
        json!({"email": long}),
        json!({"email": 7}),
        json!({"email": "agent@example.com", "priority": 1}),
        // Nothing is written when either field is refused.
        json!({"label": "  ", "email": "agent@example.com"}),
        json!({"label": "Agent TEAM", "email": "not an address"}),
    ] {
        let r = call(&h.router, Method::PATCH, path, Some(bad.clone()), &admin).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            r.body
        );
    }
    let untouched = h.store.account(&id).await?.ok_or("account")?;
    assert_eq!(
        (
            untouched.label.as_str(),
            untouched.email,
            untouched.updated_at
        ),
        ("a-team", None, Timestamp(1))
    );
    assert!(
        h.store
            .audit_details("a-team", "account.updated", 5)
            .await?
            .is_empty(),
        "a refused change is not audited"
    );

    // An instance administrator manages subscriptions as the superadmin does (ACC-009).
    let mut events = h.app.events.subscribe();
    let typed = call(
        &h.router,
        Method::PATCH,
        path,
        Some(json!({"email": "  agent@example.com "})),
        &instance_admin,
    )
    .await?;
    assert_eq!(typed.status, StatusCode::OK, "{}", typed.body);
    assert_eq!(
        (typed.body["label"].as_str(), typed.body["email"].as_str()),
        (Some("a-team"), Some("agent@example.com")),
        "trimmed, and the name is left alone"
    );
    assert_eq!(typed.body["email_typed"], true, "{}", typed.body);
    let event = account_event(&mut events, "a-team", |a| !a["email"].is_null()).await?;
    assert_eq!(event["email"], "agent@example.com");
    assert_eq!(event["email_typed"], true);

    // The name and the email in one request.
    let both = call(
        &h.router,
        Method::PATCH,
        path,
        Some(json!({"label": "Agent TEAM", "email": "team@example.com"})),
        &admin,
    )
    .await?;
    assert_eq!(both.status, StatusCode::OK, "{}", both.body);
    assert_eq!(
        (both.body["label"].as_str(), both.body["email"].as_str()),
        (Some("Agent TEAM"), Some("team@example.com"))
    );

    let named = |list: &Value| -> Option<(Value, Value)> {
        list.as_array()?
            .iter()
            .find(|a| a["label"] == "Agent TEAM")
            .map(|a| (a["label"].clone(), a["email"].clone()))
    };
    let with_email = Some((json!("Agent TEAM"), json!("team@example.com")));
    let accounts = call(&h.router, Method::GET, "/api/admin/accounts", None, &admin).await?;
    assert_eq!(named(&accounts.body), with_email);
    // EML-006: the account view says which emails a person typed and which Claude gave.
    let typed_by_person = |label: &str| -> Option<(Value, Value)> {
        accounts
            .body
            .as_array()?
            .iter()
            .find(|a| a["label"] == label)
            .map(|a| (a["email"].clone(), a["email_typed"].clone()))
    };
    assert_eq!(
        typed_by_person("Agent TEAM"),
        Some((json!("team@example.com"), json!(true)))
    );
    assert_eq!(
        typed_by_person("a-claude"),
        Some((json!("pawel@example.com"), json!(false)))
    );
    for (who, expected) in [
        (&admin, with_email.clone()),
        (&member, Some((json!("Agent TEAM"), Value::Null))),
    ] {
        let status = call(&h.router, Method::GET, "/api/status", None, who).await?;
        assert_eq!(named(&status.body["accounts"]), expected, "{}", status.body);
        let capacity = call(&h.router, Method::GET, "/api/capacity", None, who).await?;
        assert_eq!(
            named(&capacity.body["accounts"]),
            expected,
            "{}",
            capacity.body
        );
    }
    let detail = call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{task}"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.body);
    assert_eq!(detail.body["account_email"], "team@example.com");
    assert_eq!(
        detail.body["attempts"][0]["account_email"],
        "team@example.com"
    );
    // A member who reads the task sees the subscription's name without its email (UI-022).
    let seen = call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{task}"),
        None,
        &member,
    )
    .await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(seen.body["account_label"], "Agent TEAM");
    assert!(
        seen.body.get("account_email").is_none()
            && seen.body["attempts"][0].get("account_email").is_none(),
        "{}",
        seen.body
    );
    let listed = h
        .app
        .task_summary(&h.store.task(&task).await?.ok_or("task")?)
        .await?;
    assert_eq!(listed.account_email, None);

    // An empty email removes it: unknown again.
    let removed = call(
        &h.router,
        Method::PATCH,
        path,
        Some(json!({"email": "  "})),
        &admin,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    assert!(removed.body["email"].is_null(), "{}", removed.body);
    assert_eq!(removed.body["email_typed"], false, "{}", removed.body);
    assert_eq!(removed.body["label"], "Agent TEAM");
    let record = h.store.account(&id).await?.ok_or("account")?;
    assert_eq!((record.email, record.email_checked_at), (None, None));

    // The audit trail (newest first) says what changed, without the address.
    assert_eq!(
        h.store
            .audit_details("a-team", "account.updated", 5)
            .await?,
        [
            "email=removed",
            r#"label="Agent TEAM" email=set"#,
            "email=set"
        ]
    );
    Ok(())
}

/// Waits (bounded) until Claude was asked `calls` times about `credential` and its answer for
/// the account's current sign-in is stored.
async fn answered(
    h: &support::Harness,
    profile: &FakeProfile,
    id: &AccountId,
    credential: &str,
    calls: usize,
) -> Result<AccountRecord, Box<dyn std::error::Error>> {
    let wait = async {
        loop {
            if profile.calls(credential) == calls {
                let record = h.store.account(id).await?.ok_or("account")?;
                if record.email_checked_at.is_some() {
                    return Ok::<_, Box<dyn std::error::Error>>(record);
                }
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await?
}

/// SPEC-463 EML-004: what a person typed outlives the subscription's sign-ins and Claude's
/// answers, and costs no further request. Once it is removed, the background round asks Claude
/// again and Claude's answer shows.
#[tokio::test]
async fn a_typed_email_outlives_sign_ins_until_it_is_removed() -> TestResult {
    let host = Arc::new(SignInHost::default());
    if let Ok(mut tokens) = host.tokens.lock() {
        tokens.extend([REFUSED, WITH_EMAIL]);
    }
    let profile = Arc::new(FakeProfile::default());
    let h = harness_with(HarnessOptions {
        session_host: Some(host.clone()),
        usage: Some(profile.clone()),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;

    let created = call(
        &h.router,
        Method::POST,
        "/api/admin/accounts",
        Some(json!({"provider": "anthropic", "label": "Agent TEAM"})),
        &admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = AccountId::parse(created.body["account"]["id"].as_str().ok_or("id")?)?;
    let path = format!("/api/admin/accounts/{id}");
    // Claude refuses the profile to this sign-in, as it does to a `setup-token`: unknown.
    let record = answered(&h, &profile, &id, REFUSED, 1).await?;
    assert_eq!(record.email, None);

    let typed = call(
        &h.router,
        Method::PATCH,
        &path,
        Some(json!({"email": "agent@example.com"})),
        &admin,
    )
    .await?;
    assert_eq!(typed.status, StatusCode::OK, "{}", typed.body);

    // Signed in again, and this time Claude names another address: the typed one stays.
    let relogin = call(
        &h.router,
        Method::POST,
        &format!("{path}/relogin"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(relogin.status, StatusCode::OK, "{}", relogin.body);
    let record = answered(&h, &profile, &id, WITH_EMAIL, 1).await?;
    assert_eq!(record.email.as_deref(), Some("agent@example.com"));
    assert_eq!(record.status, AccountStatus::Ok);
    assert_eq!(
        h.app
            .deps
            .crypto
            .open(record.credential.as_ref().ok_or("credential")?)?,
        WITH_EMAIL.as_bytes(),
        "the new sign-in's credential"
    );
    // Claude answered for this sign-in: the background round has nothing to ask.
    h.app.refresh_usage_round(&mut HashMap::new()).await;
    assert_eq!(profile.calls(WITH_EMAIL), 1);
    let list = call(&h.router, Method::GET, "/api/admin/accounts", None, &admin).await?;
    assert_eq!(list.body[0]["email"], "agent@example.com", "{}", list.body);

    // Removed: Claude is asked once more, and its answer shows.
    let removed = call(
        &h.router,
        Method::PATCH,
        &path,
        Some(json!({"email": ""})),
        &admin,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    assert!(removed.body["email"].is_null(), "{}", removed.body);
    h.app.refresh_usage_round(&mut HashMap::new()).await;
    h.app.refresh_usage_round(&mut HashMap::new()).await;
    assert_eq!(profile.calls(WITH_EMAIL), 2, "asked again, once");
    let list = call(&h.router, Method::GET, "/api/admin/accounts", None, &admin).await?;
    assert_eq!(list.body[0]["email"], "pawel@example.com", "{}", list.body);
    Ok(())
}
