//! Messages to tasks (SPEC-230): the Superagent's `task_message` types a message into a running
//! task's session (real processes: supervisor → detached `yulius ptyhost` → fake agent), or adds
//! it to the request of a task that has not started, and refuses what cannot take one.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::context::App;
use yulius_app::coordination::ProjectCaller;
use yulius_app::error::AppError;
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_ptyhost::test_support::locate_fake_agent;

use yulius_app::model::TaskMode;

use crate::first_message::{data_dir, hooked, new_task, runtime, screen, start, stop, wait_for};
use crate::pool_supervision::{ScriptedUsage, seed};
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

/// A hooked app (its hook server up, so Claude Code's SessionStart reaches it) whose fake agent
/// runs as `agent`.
async fn app(data: &Path, agent: FakeAgentOptions) -> Result<Arc<App>, Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (app, _) = start(data, &fake, agent, 0, Arc::new(ScriptedUsage::default())).await?;
    Ok(app)
}

/// A running task whose agent answered its first message.
async fn running(app: &Arc<App>, data: &Path) -> Result<(TaskId, ProjectCaller), Error> {
    let (sandbox, user) = seed(app, data, &["a-only"]).await?;
    let id = new_task(
        app,
        &sandbox,
        &user,
        "Budget: answer with an echo",
        TaskMode::Prompt,
    )
    .await?;
    wait_for("the first answer", Duration::from_secs(30), || async {
        screen(app, &id).contains("ECHO: Budget: answer with an echo")
    })
    .await?;
    Ok((id, superagent(&sandbox, &user, "admin")))
}

fn fake_agent() -> FakeAgentOptions {
    FakeAgentOptions {
        answers_first_message: true,
        ..hooked()
    }
}

/// AC-1, AC-2, AC-7 (TMS-003, TMS-005, TMS-006): a running task gets the message in its session,
/// labelled, multi-line, with no escape sequence of the text reaching the TUI; the answer says
/// `sent`, the audit names the Superagent without the text, and the task's open `needs_you` ends.
#[test]
fn a_running_task_gets_the_message_typed_into_its_session() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
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
            async move { screen(&app, &id).contains("ECHO: Message from admin") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(
            shown.contains("ECHO: Message from admin (through their Superagent):"),
            "{shown}"
        );
        assert!(shown.contains("You may use the DO budget."), "{shown}");
        // The ESC was removed: the paste did not end inside the text, which reached the agent
        // whole (the unit tests cover the other control characters).
        assert!(shown.contains("Up to $250 [201~ in all."), "{shown}");

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
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
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

/// SPEC-230 TMS-004: a Superagent's message never goes into Claude Code's folder-trust dialog
/// of a goal run (whose goal is its launch prompt, so no first message is being typed): only a
/// person answers it (ONE-005). The call is refused and nothing is typed.
#[test]
fn a_superagent_message_is_not_typed_into_a_trust_prompt() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let app = app(&data, agent).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make the header blue",
            TaskMode::Goal,
        )
        .await?;
        wait_for("the trust prompt", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Yes, I trust this folder")
        })
        .await?;
        let caller = superagent(&sandbox, &user, "admin");
        let refused = app
            .tool_task_message(&caller, &json!({"task_id": id.as_str(), "text": "Go on"}))
            .await
            .err()
            .ok_or("the message was typed into the trust prompt")?;
        assert!(refused.to_string().contains("trust"), "{refused}");
        // Whatever reaches the fake agent shows within a second; nothing did.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let shown = screen(&app, &id);
        assert!(!shown.contains("TRUST ANSWERED"), "{shown}");
        assert!(!shown.contains("Superagent"), "{shown}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-8 (TMS-004): a run that is running but whose Claude Code session has not started (no
/// SessionStart yet) refuses the message: typed before its prompt box reads, it would be lost or
/// answer a trust dialog about to show. Nothing is typed.
#[test]
fn a_session_that_has_not_started_refuses_the_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            no_session_start: true,
            ..fake_agent()
        };
        let app = app(&data, agent).await?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make the header blue",
            TaskMode::Goal,
        )
        .await?;
        wait_for("the run", Duration::from_secs(30), || async {
            app.deps
                .store
                .task(&id)
                .await
                .ok()
                .flatten()
                .is_some_and(|t| t.status == TaskStatus::Running)
                && !screen(&app, &id).trim().is_empty()
        })
        .await?;
        let caller = superagent(&sandbox, &user, "admin");
        let refused = app
            .tool_task_message(&caller, &json!({"task_id": id.as_str(), "text": "Go on"}))
            .await
            .err()
            .ok_or("the message was typed before the session started")?;
        assert!(
            matches!(&refused, AppError::Conflict(m) if m.contains("starting")),
            "{refused:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        let shown = screen(&app, &id);
        assert!(!shown.contains("Superagent"), "{shown}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
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
) -> Result<Value, AppError> {
    h.app.tool_task_message(caller, &args).await
}

/// The refusal of a call that must fail.
async fn refusal(
    h: &support::Harness,
    caller: &ProjectCaller,
    args: Value,
) -> Result<AppError, Error> {
    match message(h, caller, args.clone()).await {
        Ok(answer) => Err(format!("{args} was taken: {answer}").into()),
        Err(err) => Ok(err),
    }
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

    let closed = refusal(&h, &caller, json!({"task_id": "t-done", "text": "Go on"})).await?;
    assert!(
        matches!(&closed, AppError::Conflict(m) if m.contains("closed")),
        "{closed:?}"
    );

    // Running, but no live session (no run attached here).
    seed_task(&h, "t-running", "s1", "u-alice", TaskStatus::Running, None).await?;
    let detached = refusal(
        &h,
        &caller,
        json!({"task_id": "t-running", "text": "Go on"}),
    )
    .await?;
    assert!(
        matches!(&detached, AppError::Conflict(m) if m.contains("no live session")),
        "{detached:?}"
    );

    for text in ["", "  \n\x1b\x07 "] {
        let empty = refusal(&h, &caller, json!({"task_id": "t-queued", "text": text})).await?;
        assert!(
            matches!(&empty, AppError::Invalid(m) if m.contains("empty")),
            "{empty:?}"
        );
    }
    let long = "x".repeat(64 * 1024 + 1);
    let long = refusal(&h, &caller, json!({"task_id": "t-queued", "text": long})).await?;
    assert!(
        matches!(&long, AppError::Invalid(m) if m.contains("KiB")),
        "{long:?}"
    );

    let elsewhere = superagent(
        &SandboxId::parse("s2")?,
        &UserId::parse("u-alice")?,
        "alice",
    );
    add_sandbox(&h, "s2", "other", 20010).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/other", perms("write"));
    let elsewhere = refusal(
        &h,
        &elsewhere,
        json!({"task_id": "t-queued", "text": "Go on"}),
    )
    .await?;
    assert!(matches!(elsewhere, AppError::NotFound), "{elsewhere:?}");

    let bob = superagent(&SandboxId::parse("s1")?, &UserId::parse("u-bob")?, "bob");
    let bob = refusal(&h, &bob, json!({"task_id": "t-queued", "text": "Go on"})).await?;
    assert!(matches!(bob, AppError::Forbidden(_)), "{bob:?}");
    let request = h
        .store
        .task(&TaskId::parse("t-queued")?)
        .await?
        .ok_or("task")?
        .prompt;
    assert_eq!(request, "do things");
    Ok(())
}
