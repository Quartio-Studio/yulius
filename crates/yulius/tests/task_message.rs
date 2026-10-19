//! Messages to tasks (SPEC-230): the Superagent's `task_message` types a message into a running
//! task's session (real processes: supervisor → detached `yulius ptyhost` → fake agent), or adds
//! it to the request of a task that has not started, and refuses what cannot take one.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::context::App;
use yulius_app::coordination::ProjectCaller;
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_ptyhost::test_support::locate_fake_agent;

use crate::pool_supervision::{
    ScriptedUsage, build_with, new_task, runtime, screen, seed, stop, temp, wait_for,
};
use crate::support::agents::FakeAgentOptions;
use crate::support::{
    self, TestResult, add_sandbox, harness_configured, perms, seed_task, user_with_token,
};

type Error = Box<dyn std::error::Error>;

/// The Superagent of `user` in `sandbox`, as `cockpit_caller` makes it.
fn superagent(sandbox: &SandboxId, user: &UserId, login: &str) -> ProjectCaller {
    ProjectCaller {
        sandbox: sandbox.clone(),
        user: user.clone(),
        task: None,
        actor: format!("cockpit:{login}"),
        detail: json!({"via": "cockpit", "shell": "sh-test"}),
    }
}

/// The audit rows of messages to `id`, as (actor, detail).
fn audited(data: &Path, id: &TaskId) -> Result<Vec<(String, String)>, Error> {
    let db = rusqlite::Connection::open(data.join("yulius.db"))?;
    let mut query = db.prepare(
        "SELECT actor, COALESCE(detail, '') FROM audit_log WHERE action = 'task.messaged' \
         AND target = ?1 ORDER BY id",
    )?;
    let rows = query.query_map([id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// A running task whose agent answered its first message.
async fn running(app: &Arc<App>, data: &Path) -> Result<(TaskId, ProjectCaller), Error> {
    let (sandbox, user) = seed(app, data, &["a-only"]).await?;
    let id = new_task(app, &sandbox, &user, "Budget").await?;
    wait_for("the first answer", Duration::from_secs(30), || {
        let app = app.clone();
        let id = id.clone();
        async move { screen(&app, &id).contains("ECHO: Budget: answer with an echo") }
    })
    .await?;
    Ok((id, superagent(&sandbox, &user, "admin")))
}

fn fake_agent() -> FakeAgentOptions {
    FakeAgentOptions {
        answers_first_message: true,
        ..FakeAgentOptions::default()
    }
}

/// AC-1, AC-2, AC-7 (TMS-003, TMS-005, TMS-006): a running task gets the message in its session,
/// labelled, multi-line, with no escape sequence of the text reaching the TUI; the answer says
/// `sent`, the audit names the Superagent without the text, and the task's open `needs_you` ends.
#[test]
fn a_running_task_gets_the_message_typed_into_its_session() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            fake_agent(),
        )?;
        let (id, caller) = running(&app, &data).await?;
        let asked = NeedsYouRecord {
            action: "Approve spending on DigitalOcean".into(),
            why: None,
            link: None,
            at: 1,
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asked), app.now())
            .await?;

        let text = "You may use the DO budget.\r\nUp to $250 \x1b[201~ in all.\x07";
        let answer = app
            .tool_task_message(&caller, &json!({"task_id": id.as_str(), "text": text}))
            .await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        wait_for("the message", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("in all.") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(
            shown.contains("ECHO: Message from admin (through their Superagent):"),
            "{shown}"
        );
        assert!(shown.contains("You may use the DO budget."), "{shown}");
        // The ESC was removed: the paste did not end inside the text, which reached the agent
        // whole, the bell gone.
        assert!(shown.contains("Up to $250 [201~ in all."), "{shown}");
        assert!(!shown.contains('\u{7}'), "{shown}");

        let rows = audited(&data, &id)?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "cockpit:admin");
        assert!(rows[0].1.contains("sent"), "{rows:?}");
        assert!(
            !rows[0].1.contains("budget"),
            "the audit holds the text: {rows:?}"
        );
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        stop(&app, &id).await
    })
}

/// AC-3 (TMS-004): a person who typed into the task's terminal in the last minute owns its
/// prompt box: nothing is typed, and the Superagent is told why.
#[test]
fn a_persons_draft_keeps_the_message_out() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            fake_agent(),
        )?;
        let (id, caller) = running(&app, &data).await?;
        app.record_typing(&id, &caller.user);
        let refused = app
            .tool_task_message(&caller, &json!({"task_id": id.as_str(), "text": "Go on"}))
            .await
            .err()
            .ok_or("the message was sent over a person's draft")?;
        assert!(refused.to_string().contains("typed"), "{refused}");
        // Whatever reaches the agent shows within a second; nothing did.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            !screen(&app, &id).contains("Superagent"),
            "{}",
            screen(&app, &id)
        );
        assert!(audited(&data, &id)?.is_empty());
        stop(&app, &id).await
    })
}

/// Sandbox `s1` with alice (write) and bob (read); tasks stay where they are seeded (no
/// live-session capacity).
async fn world() -> Result<support::Harness, Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    user_with_token(&h, "u-alice", "alice", false).await?;
    user_with_token(&h, "u-bob", "bob", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    h.github
        .set_perm("bob", "Quartio-Studio/studio", perms("read"));
    Ok(h)
}

fn alice() -> Result<ProjectCaller, Error> {
    Ok(superagent(
        &SandboxId::parse("s1")?,
        &UserId::parse("u-alice")?,
        "alice",
    ))
}

async fn message(
    h: &support::Harness,
    caller: &ProjectCaller,
    args: Value,
) -> Result<Value, String> {
    h.app
        .tool_task_message(caller, &args)
        .await
        .map_err(|err| err.to_string())
}

/// AC-4 (TMS-002): a task that has not started takes the message into its request, as a queued
/// message of its person.
#[tokio::test]
async fn a_task_that_has_not_started_gets_the_message_in_its_request() -> TestResult {
    let h = world().await?;
    seed_task(&h, "t-queued", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let number = h
        .store
        .task(&TaskId::parse("t-queued")?)
        .await?
        .ok_or("task")?
        .number;
    let answer = message(
        &h,
        &alice()?,
        json!({"number": number, "text": "You may use the DO budget."}),
    )
    .await?;
    assert_eq!(answer["delivered"], "queued", "{answer}");
    let request = h
        .store
        .task(&TaskId::parse("t-queued")?)
        .await?
        .ok_or("task")?
        .prompt;
    assert!(
        request.contains("**Added by alice before the task started ("),
        "{request}"
    );
    assert!(request.ends_with("You may use the DO budget."), "{request}");
    Ok(())
}

/// AC-5 (TMS-004, TMS-005): a task that is neither waiting to start nor running, an empty text,
/// a task of another project and a person who may only read are refused; nothing changes.
#[tokio::test]
async fn what_cannot_take_a_message_is_refused() -> TestResult {
    let h = world().await?;
    seed_task(&h, "t-done", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t-queued", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let caller = alice()?;

    let closed = message(&h, &caller, json!({"task_id": "t-done", "text": "Go on"}))
        .await
        .err()
        .ok_or("a closed task took a message")?;
    assert!(closed.contains("closed"), "{closed}");

    for text in ["", "  \n\x1b\x07 "] {
        let empty = message(&h, &caller, json!({"task_id": "t-queued", "text": text}))
            .await
            .err()
            .ok_or("an empty message was taken")?;
        assert!(empty.contains("empty"), "{empty}");
    }
    let long = "x".repeat(64 * 1024 + 1);
    assert!(
        message(&h, &caller, json!({"task_id": "t-queued", "text": long}))
            .await
            .is_err()
    );

    let elsewhere = superagent(
        &SandboxId::parse("s2")?,
        &UserId::parse("u-alice")?,
        "alice",
    );
    add_sandbox(&h, "s2", "other", 20010).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/other", perms("write"));
    assert!(
        message(
            &h,
            &elsewhere,
            json!({"task_id": "t-queued", "text": "Go on"})
        )
        .await
        .is_err()
    );

    let bob = superagent(&SandboxId::parse("s1")?, &UserId::parse("u-bob")?, "bob");
    assert!(
        message(&h, &bob, json!({"task_id": "t-queued", "text": "Go on"}))
            .await
            .is_err()
    );
    let request = h
        .store
        .task(&TaskId::parse("t-queued")?)
        .await?
        .ok_or("task")?
        .prompt;
    assert_eq!(request, "do things");
    Ok(())
}
