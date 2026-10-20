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
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::AccountUsage;

/// Synthetic tokens (realistic shape, not real credentials): one whose profile has an email, one
/// whose profile is refused (a `setup-token` without `user:profile`).
const WITH_EMAIL: &str = "sk-ant-oat01-SYNTHETICwithemail0123456789abcdefghijklmnopqrstu-test";
const REFUSED: &str = "sk-ant-oat01-SYNTHETICrefused000123456789abcdefghijklmnopqrstu-test";

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
    // Task lists and live updates go to everyone who sees the task: no email there.
    let listed = h
        .app
        .task_summary(&h.store.task(&task).await?.ok_or("task")?)
        .await?;
    assert_eq!(listed.account_label.as_deref(), Some("work Max"));
    assert_eq!(listed.account_email, None);
    Ok(())
}
