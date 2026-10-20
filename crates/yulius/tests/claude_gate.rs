//! The Claude account gate (SPEC-246): a person's start of an agent with no usable Claude account
//! is refused before anything changes, with `claude_account_required` and where to connect one; a
//! start by machinery waits, saying why; `/api/me` tells the UI whether one is connected. These
//! tests use the in-memory harness with no session slot (`max_live_sessions = 0`): a start that
//! passes the gate queues.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, mcp_tool, perms,
    seed_live_attempt, seed_task, tool_text, user_with_token,
};
use yulius_app::hub::HubFrame;
use yulius_app::model::{AccountRecord, AccountStatus, PauseKind};
use yulius_app::ports::store::{AccountStore, TaskStore};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, SandboxId, TaskId};
use yulius_domain::task::{FailReason, TaskStatus};
use yulius_domain::time::Timestamp;

type Error = Box<dyn std::error::Error>;

const CREATE: &str = "/api/sandboxes/s1/tasks";

/// Sandbox `s1` where alice may write; her task `t-agent` runs (task token `tok-agent`).
async fn world() -> Result<(Harness, Creds), Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    seed_task(&h, "t-agent", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-agent", "tok-agent").await?;
    Ok((h, alice))
}

/// A Claude account `id` in `status`, signed in when `signed_in`.
async fn account(h: &Harness, id: &str, status: AccountStatus, signed_in: bool) -> TestResult {
    let credential = if signed_in {
        Some(
            h.app
                .deps
                .crypto
                .seal(b"synthetic-subscription-credential")?,
        )
    } else {
        None
    };
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential,
            status,
            usage_json: None,
            last_error: None,
            email: None,
            email_checked_at: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(())
}

/// The refusal: 409 `claude_account_required`, naming the reason and Subscriptions.
fn assert_refused(resp: &support::Resp, reason: &str) {
    assert_eq!(resp.status, StatusCode::CONFLICT, "{:?}", resp.body);
    assert_eq!(
        resp.body["error"]["code"], "claude_account_required",
        "{:?}",
        resp.body
    );
    let message = resp.body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(reason), "{message}");
    assert!(message.contains("/admin/accounts"), "{message}");
}

async fn task_count(h: &Harness) -> Result<usize, Error> {
    Ok(h.store.tasks(&SandboxId::parse("s1")?, true).await?.len())
}

async fn status(h: &Harness, id: &str) -> Result<TaskStatus, Error> {
    Ok(h.store
        .task(&TaskId::parse(id)?)
        .await?
        .ok_or("task")?
        .status)
}

/// CLA-002: New task (now, after a task, by a rule; from the UI and the `task_create` tool) is
/// refused with no account and creates nothing; a prepared task is written, and Start now on it is
/// refused. A pending sign-in connects nothing; once an account is signed in the same requests work.
#[tokio::test]
async fn a_new_task_without_a_claude_account_is_refused_and_creates_nothing() -> TestResult {
    let (h, alice) = world().await?;
    let before = task_count(&h).await?;
    account(&h, "a-pending", AccountStatus::PendingLogin, false).await?;
    for body in [
        json!({"prompt": "Tidy the docs"}),
        json!({"prompt": "Tidy the docs", "after_task_id": "t-agent"}),
        json!({"prompt": "Tidy the docs", "supertask": true}),
    ] {
        let refused = call(&h.router, Method::POST, CREATE, Some(body.clone()), &alice).await?;
        assert_refused(&refused, "no Claude account is connected");
    }
    let tool = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "Tidy the docs"}),
    )
    .await?;
    let text = tool.to_string();
    assert!(text.contains("Connect a Claude account first"), "{text}");
    assert_eq!(task_count(&h).await?, before, "nothing was created");

    // Writing a task for later is not a start; starting it is.
    let prepared = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "Tidy the docs", "prepared": true})),
        &alice,
    )
    .await?;
    assert_eq!(prepared.status, StatusCode::CREATED, "{:?}", prepared.body);
    let id = prepared.body["id"].as_str().ok_or("id")?.to_owned();
    let start = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{id}/start-now"),
        None,
        &alice,
    )
    .await?;
    assert_refused(&start, "no Claude account is connected");
    let change = call(
        &h.router,
        Method::PUT,
        &format!("/api/tasks/{id}/start"),
        Some(json!({})),
        &alice,
    )
    .await?;
    assert_refused(&change, "no Claude account is connected");
    assert_eq!(status(&h, &id).await?, TaskStatus::Prepared);

    account(&h, "a-ok", AccountStatus::Ok, true).await?;
    let created = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "Tidy the docs"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let tool = tool_text(
        &mcp_tool(
            &h,
            "tok-agent",
            "task_create",
            json!({"prompt": "Tidy the docs"}),
        )
        .await?,
    );
    assert_eq!(tool["task"]["status"], "queued", "{tool:?}");
    Ok(())
}

/// CLA-002: accounts that must all sign in again are no usable account, and the refusal says so.
#[tokio::test]
async fn accounts_that_must_sign_in_again_refuse_the_start_saying_so() -> TestResult {
    let (h, alice) = world().await?;
    account(&h, "a-stale", AccountStatus::Relogin, true).await?;
    let refused = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "Tidy the docs"})),
        &alice,
    )
    .await?;
    assert_refused(&refused, "every Claude account must sign in again");
    // A limited account still runs (it waits for capacity): not refused.
    account(&h, "a-limited", AccountStatus::Limited, true).await?;
    let created = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "Tidy the docs"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    Ok(())
}

/// CLA-002: Continue / Try again of a resting task is refused and leaves it as it was.
#[tokio::test]
async fn continuing_without_a_claude_account_is_refused_and_changes_nothing() -> TestResult {
    let (h, alice) = world().await?;
    seed_task(
        &h,
        "t-failed",
        "s1",
        "u-alice",
        TaskStatus::Failed(FailReason::NoAccount),
        None,
    )
    .await?;
    let refused = call(
        &h.router,
        Method::POST,
        "/api/tasks/t-failed/continue",
        Some(json!({})),
        &alice,
    )
    .await?;
    assert_refused(&refused, "no Claude account is connected");
    assert_eq!(
        status(&h, "t-failed").await?,
        TaskStatus::Failed(FailReason::NoAccount)
    );
    assert!(
        h.store
            .attempts(&TaskId::parse("t-failed")?)
            .await?
            .is_empty()
    );
    // CLA-005: the status reads as such on the wire.
    let read = call(&h.router, Method::GET, "/api/tasks/t-failed", None, &alice).await?;
    assert_eq!(read.body["status"], "failed_no_account", "{:?}", read.body);
    Ok(())
}

/// CLA-001: `/api/me` says whether a Claude account is usable.
#[tokio::test]
async fn me_says_whether_a_claude_account_is_connected() -> TestResult {
    let (h, alice) = world().await?;
    let connected = |me: &Value| me["instance"]["claude_connected"].clone();
    let me = call(&h.router, Method::GET, "/api/me", None, &alice).await?;
    assert_eq!(connected(&me.body), json!(false), "{:?}", me.body);
    account(&h, "a-pending", AccountStatus::PendingLogin, false).await?;
    let me = call(&h.router, Method::GET, "/api/me", None, &alice).await?;
    assert_eq!(connected(&me.body), json!(false), "a sign-in not finished");
    account(&h, "a-ok", AccountStatus::Ok, true).await?;
    let me = call(&h.router, Method::GET, "/api/me", None, &alice).await?;
    assert_eq!(connected(&me.body), json!(true), "{:?}", me.body);
    Ok(())
}

/// CLA-004: a start by machinery that finds no account waits `waiting_capacity` and tells its
/// viewers that no Claude account is connected and where to connect one; with an account that is
/// only out of capacity it says that instead.
#[tokio::test]
async fn a_start_by_machinery_waits_for_an_account_and_says_why() -> TestResult {
    let (h, _) = world().await?;
    let notice_of = |rx: &mut tokio::sync::broadcast::Receiver<HubFrame>| {
        let mut notices = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let HubFrame::Notice(text) = frame {
                notices.push(text);
            }
        }
        notices.join("\n")
    };
    seed_task(&h, "t-chained", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let id = TaskId::parse("t-chained")?;
    let mut rx = h.app.runtime(&id).hub.watch();
    h.app.pause_task(&id, PauseKind::Start, None, None).await?;
    assert_eq!(status(&h, "t-chained").await?, TaskStatus::WaitingCapacity);
    let said = notice_of(&mut rx);
    assert!(said.contains("no Claude account is connected"), "{said}");
    assert!(said.contains("/admin/accounts"), "{said}");
    assert!(said.contains("starts by itself"), "{said}");
    // Its summary says so too, for whoever opens it later (the Paused card, its time label).
    let summary = h
        .app
        .task_summary(&h.store.task(&id).await?.ok_or("task")?)
        .await?;
    assert!(summary.waits_for_account, "{summary:?}");

    // Signed in once but expired: it says the account must sign in again.
    account(&h, "a-stale", AccountStatus::Relogin, true).await?;
    seed_task(&h, "t-stale", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let stale = TaskId::parse("t-stale")?;
    let mut rx = h.app.runtime(&stale).hub.watch();
    h.app
        .pause_task(&stale, PauseKind::Start, None, None)
        .await?;
    let said = notice_of(&mut rx);
    assert!(said.contains("must sign in again"), "{said}");

    account(&h, "a-ok", AccountStatus::Ok, true).await?;
    seed_task(&h, "t-later", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let later = TaskId::parse("t-later")?;
    let mut rx = h.app.runtime(&later).hub.watch();
    h.app
        .pause_task(&later, PauseKind::Start, None, None)
        .await?;
    let said = notice_of(&mut rx);
    assert!(said.contains("no subscription has capacity left"), "{said}");
    let summary = h
        .app
        .task_summary(&h.store.task(&later).await?.ok_or("task")?)
        .await?;
    assert!(!summary.waits_for_account, "only out of capacity");
    Ok(())
}

/// CLA-004 (review round 2): a paused task's `waits_for_account` follows the accounts: once one is
/// connected (here one with no capacity left, so it keeps waiting) the next resume pass shows the
/// task again as waiting for capacity, without the task itself changing.
#[tokio::test]
async fn a_waiting_task_is_shown_again_when_an_account_is_connected() -> TestResult {
    let (h, _) = world().await?;
    seed_task(&h, "t-wait", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let id = TaskId::parse("t-wait")?;
    h.app.pause_task(&id, PauseKind::Start, None, None).await?;
    h.app.resume_round().await;
    let mut events = h.app.events.subscribe();
    support::exhausted_subscription(&h).await?;
    h.app.resume_round().await;
    let mut shown = None;
    while let Ok(envelope) = events.try_recv() {
        if let yulius_contracts::events::UiEvent::TaskUpdated { task } = envelope.event
            && task.id == "t-wait"
        {
            shown = Some(task.waits_for_account);
        }
    }
    assert_eq!(shown, Some(false), "shown again, waiting for capacity now");
    assert_eq!(status(&h, "t-wait").await?, TaskStatus::WaitingCapacity);
    Ok(())
}
