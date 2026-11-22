//! Messages to tasks (SPEC-230): the Superagent's `task_message` types a message into a running
//! task's session (real processes: supervisor → detached `yulius ptyhost` → fake agent), or adds
//! it to the request of a task that has not started, and refuses what cannot take one.

use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yulius_app::auth::{AuthContext, TokenGrant};
use yulius_app::context::App;
use yulius_app::coordination::{Channel, ProjectCaller};
use yulius_app::error::AppError;
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::store::TaskStore;
use yulius_app::remote::RemoteCaller;
use yulius_app::terminal::TerminalAttach;
use yulius_contracts::mcp::RpcRequest;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::TokenScope;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

use yulius_app::model::TaskMode;

use crate::first_message::{
    ShiftedClock, activity, data_dir, hooked, new_task, notices, runtime, screen, start, start_on,
    start_on_listener, start_with, stop, wait_for,
};
use crate::pool_supervision::{HOUR, ScriptedUsage, refresh, seed, usage};
use crate::support::agents::FakeAgentOptions;
use crate::support::{
    self, TestResult, add_sandbox, harness_configured, perms, seed_task, user_with_token,
};

type Error = Box<dyn std::error::Error>;

/// The Superagent of `user` in `sandbox`, as `cockpit_caller` makes it.
pub(crate) fn superagent(sandbox: &SandboxId, user: &UserId, login: &str) -> ProjectCaller {
    ProjectCaller {
        sandbox: sandbox.clone(),
        user: user.clone(),
        task: None,
        channel: Channel::Superagent,
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

/// SPEC-483 slice 5 (review round 4, ADR-0317): a running task whose screen became unknown (a
/// secret stopped opening, output was dropped) gets no message typed, and its person learns it:
/// the first refusal flags the task with what to do, once, however many messages follow.
#[test]
fn a_message_kept_out_of_a_withheld_screen_flags_the_task_once() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, caller) = running(&app, &data).await?;
        // The fake agent echoes its answer before Stop settles the turn to Waiting. Let that
        // transition finish before this test raises and restores the withheld-screen flag.
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let stop_answered =
            Path::new(&attempt.run_dir).join(yulius_app::outage::STOP_ANSWERED_FILE);
        wait_for("the opening Stop", Duration::from_secs(30), || async {
            stop_answered.is_file()
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let add = |name: &'static str| {
            let (app, ctx, sandbox) = (Arc::clone(&app), ctx.clone(), caller.sandbox.clone());
            async move {
                let input = serde_json::from_value(json!({
                    "name": name,
                    "value": format!("{name}-value-0123456"),
                }))?;
                Ok::<_, Error>(
                    app.create_agent_secret(&ctx, sandbox.as_str(), &input)
                        .await?,
                )
            }
        };
        // A secret that no longer opens (a key-encryption key nobody has); a change of a secret
        // makes the terminal read them anew at its next output.
        let key = add("DEPLOY_KEY").await?;
        let seen = app.deps.store.vault_get(&key.id).await?.ok_or("secret")?;
        let mut broken = seen.clone();
        broken.envelope.kek_id = 9_999;
        assert!(app.deps.store.vault_update(&broken, &seen).await?);
        add("LATER_KEY").await?;
        let mut watch = app.runtime(&id).hub.watch();
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        // A key that is not sent as a prompt: the terminal's echo is its next output. A prompt
        // would start a turn of the agent, whose own states (working, then waiting) replace a
        // flag whenever they arrive, before or after the one this test is about.
        app.terminal_input(&ctx, &attach, b"n", false).await?;
        wait_for("the withheld screen", Duration::from_secs(30), || async {
            screen(&app, &id).contains("the screen is withheld")
        })
        .await?;
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));

        let message = json!({"task_id": id.as_str(), "text": "Go on"});
        for _ in 0..2 {
            let refused = app
                .tool_task_message(&caller, &message)
                .await
                .err()
                .ok_or("a message was typed into a withheld screen")?;
            assert!(
                refused.to_string().contains("screen was withheld"),
                "{refused}"
            );
        }
        assert_eq!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let why = support::attention(&app, id.as_str()).await?;
        assert_eq!(
            why["problem"],
            "Agent secrets could not be read, so this terminal's screen was withheld and Yulius \
             typed nothing.",
            "{why}"
        );
        let told = notices(&mut watch)
            .iter()
            .filter(|n| n.contains("screen was withheld and Yulius typed nothing"))
            .count();
        assert_eq!(told, 1, "said once");

        // The agent's own state replaces a flag whenever it arrives (its turn ended): the next
        // message kept out raises the flag again, with its reason, and the terminal is not told
        // twice.
        app.deps
            .store
            .set_activity(&id, Some(Activity::Waiting), app.now())
            .await?;
        assert!(app.tool_task_message(&caller, &message).await.is_err());
        assert_eq!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let why = support::attention(&app, id.as_str()).await?;
        assert!(
            why["problem"]
                .as_str()
                .is_some_and(|problem| problem.contains("screen was withheld")),
            "{why}"
        );
        let told = notices(&mut watch)
            .iter()
            .filter(|n| n.contains("screen was withheld and Yulius typed nothing"))
            .count();
        assert_eq!(told, 0, "not said again");
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
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
            happened: None,
            problem: None,
            decision: false,
            link: None,
            at: 1,
            cause: None,
            named: Default::default(),
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

/// The audit rows of answers to `id`'s `needs_you` (SPEC-516 NYA-005), as (actor, detail).
fn answers_audited(data: &Path, id: &TaskId) -> Result<Vec<(String, Value)>, Error> {
    let db = rusqlite::Connection::open(data.join("yulius.db"))?;
    let mut query = db.prepare(
        "SELECT actor, COALESCE(detail, 'null') FROM audit_log \
         WHERE action = 'task.needs_you.answered' AND target = ?1 ORDER BY id",
    )?;
    let rows = query.query_map([id.as_str()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (actor, detail) = row?;
        out.push((actor, serde_json::from_str(&detail)?));
    }
    Ok(out)
}

/// A person who is not `sandbox`'s tasks' owner and may write to it, as `bob`.
async fn teammate(app: &App, sandbox: &SandboxId) -> Result<AuthContext, Error> {
    let bob = yulius_app::model::UserRecord {
        id: UserId::parse("u-bob")?,
        github_login: Some("bob".into()),
        github_id: None,
        display_name: "Bob".into(),
        avatar_url: None,
        is_superadmin: false,
        is_instance_admin: false,
        password_hash: None,
        created_at: yulius_domain::time::Timestamp(1),
        disabled_at: None,
    };
    let store = &app.deps.store;
    yulius_app::ports::store::UserStore::insert_user(&**store, &bob).await?;
    // Bob may write to the project (a fresh access GitHub gave).
    yulius_app::ports::store::PermissionStore::put_access(
        &**store,
        &bob.id,
        sandbox,
        yulius_domain::permission::AccessLevel::Write,
        app.now(),
    )
    .await?;
    Ok(AuthContext {
        user: bob,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// What the agent asks a person, for the answer tests.
fn region_asked() -> NeedsYouRecord {
    NeedsYouRecord {
        action: "Choose the staging region".into(),
        happened: None,
        problem: None,
        decision: true,
        link: None,
        at: 1,
        cause: None,
        named: Default::default(),
    }
}

/// SPEC-516 NYA-004 (review round 1): a run started with its owner's personal MCP server holds
/// it until it restarts, also once the server is deleted. A teammate's answer is kept out of it;
/// the owner's own answer goes in, labelled as theirs and audited as the owner's.
#[test]
fn a_run_holding_the_owners_personal_server_takes_only_the_owners_answer() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let owner = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("owner")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let input = serde_json::from_value(json!({
            "name": "mail", "transport": "http", "url": "https://mail.admin.test/mcp",
            "scope": "sandbox", "audience": "personal",
        }))?;
        let server = app
            .create_mcp_server(&owner, sandbox.as_str(), &input)
            .await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Budget: answer with an echo",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("the first answer", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Budget: answer with an echo")
        })
        .await?;
        // The configuration no longer lists it; the run still holds it.
        app.delete_mcp_server(&owner, &server.id).await?;
        app.deps
            .store
            .set_task_needs_you(&id, Some(&region_asked()), app.now())
            .await?;
        let bob = teammate(&app, &sandbox).await?;
        let refused = app
            .answer_needs_you(&bob, id.as_str(), "Frankfurt", None)
            .await
            .err();
        assert!(
            matches!(&refused, Some(AppError::Conflict(why)) if why.contains("@admin") && why.contains("personal")),
            "{refused:?}"
        );
        assert!(app.deps.store.task_needs_you(&id).await?.is_some());

        let after = app
            .answer_needs_you(&owner, id.as_str(), "Frankfurt", None)
            .await?;
        assert!(after.needs_you.is_none());
        wait_for("the answer", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ECHO: Answer from admin") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(
            shown.contains("ECHO: Answer from admin to what you asked with needs_you:"),
            "{shown}"
        );
        assert!(!shown.contains("not this task's owner"), "{shown}");
        let rows = answers_audited(&data, &id)?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "admin", "{rows:?}");
        assert_eq!(rows[0].1["by_owner"], true, "{rows:?}");
        stop(&app, &id).await
    })
}

/// SPEC-516 AC-1 (NYA-001, NYA-002, NYA-003, NYA-005): bob, who may work in the project but
/// does not own the task, answers what its agent asked. The agent gets one labelled answer that
/// names him as not the owner, the request ends, the task stays admin's (no control request, no
/// giver), and the audit names him without the text.
#[test]
fn a_teammate_answers_a_needs_you_without_taking_control() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, owner) = running(&app, &data).await?;
        let asked = NeedsYouRecord {
            action: "Choose the staging region".into(),
            happened: None,
            problem: None,
            decision: true,
            link: None,
            at: 1,
            cause: None,
            named: Default::default(),
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asked), app.now())
            .await?;
        let ctx = teammate(&app, &owner.sandbox).await?;

        let after = app
            .answer_needs_you(&ctx, id.as_str(), "Frankfurt, please.\r\nNot Dublin.", None)
            .await?;
        assert_eq!(after.owner.login, "admin");
        assert!(after.needs_you.is_none());
        wait_for("the answer", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ECHO: Answer from bob") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(
            shown.contains("ECHO: Answer from bob to what you asked with needs_you"),
            "{shown}"
        );
        assert!(shown.contains("not this task's owner"), "{shown}");
        assert!(shown.contains("Frankfurt, please."), "{shown}");
        assert!(shown.contains("Not Dublin."), "{shown}");

        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(task.owner, owner.user);
        assert!(app.deps.store.control_request(&id).await?.is_none());
        assert!(app.deps.store.control_from(&id).await?.is_none());
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        let rows = answers_audited(&data, &id)?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "bob", "{rows:?}");
        assert_eq!(rows[0].1["owner"], "admin", "{rows:?}");
        assert_eq!(rows[0].1["by_owner"], false, "{rows:?}");
        assert!(
            !rows[0].1.to_string().contains("Frankfurt"),
            "the audit holds the text: {rows:?}"
        );
        stop(&app, &id).await
    })
}

/// SPEC-322 AC-1 (RML-002, RML-004; #789): a message sent through the remote MCP (a person's own
/// agent, with an `operate` token) is labelled with that channel, not the Superagent; it is
/// audited as the token's person and ends the task's open `needs_you`, as the Superagent's does.
#[test]
fn a_remote_message_names_the_persons_own_agent() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, superagent) = running(&app, &data).await?;
        let asked = NeedsYouRecord {
            action: "Approve spending on DigitalOcean".into(),
            happened: None,
            problem: None,
            decision: false,
            link: None,
            at: 1,
            cause: None,
            named: Default::default(),
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asked), app.now())
            .await?;
        let user = app
            .deps
            .store
            .user(&superagent.user)
            .await?
            .ok_or("the seeded person")?;
        let remote = RemoteCaller {
            ctx: AuthContext {
                user,
                csrf_token: None,
                session_hash: None,
                token: Some(TokenGrant {
                    id: "tok-laptop".into(),
                    scope: TokenScope::Operate,
                }),
            },
            token_id: "tok-laptop".into(),
            scope: TokenScope::Operate,
            login: "admin".into(),
            superagent: None,
        };

        let request = RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/call".into(),
            params: json!({
                "name": "task_message",
                "arguments": {"project": "acme/pool", "task_id": id.as_str(), "text": "Use the DO budget."},
                "_meta": {"yulius/structured": true},
            }),
        };
        let response = app
            .remote_mcp_handle(&remote, request)
            .await
            .ok_or("no answer")?;
        let answer = serde_json::to_value(&response)?;
        let result = &answer["result"];
        assert_ne!(result["isError"], true, "{answer}");
        assert_eq!(result["structuredContent"]["delivered"], "sent", "{answer}");
        wait_for("the message", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ECHO: Message from admin") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(
            shown.contains("ECHO: Message from admin (through their own agent):"),
            "{shown}"
        );
        assert!(!shown.contains("through their Superagent"), "{shown}");
        assert!(shown.contains("Use the DO budget."), "{shown}");

        let rows = audited(&data, &id)?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "remote:admin");
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        stop(&app, &id).await
    })
}

/// SPEC-427 GSA-003: a message the person's Global Superagent sends over the remote tools is
/// labelled as the Superagent's, not as the person's own agent, and audited as
/// `superagent:<login>`.
#[test]
fn a_global_superagent_message_names_the_superagent() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, superagent) = running(&app, &data).await?;
        let user = app
            .deps
            .store
            .user(&superagent.user)
            .await?
            .ok_or("the seeded person")?;
        let global = RemoteCaller {
            ctx: AuthContext {
                user,
                csrf_token: None,
                session_hash: None,
                token: Some(TokenGrant {
                    id: "superagent:sh-global".into(),
                    scope: TokenScope::Operate,
                }),
            },
            token_id: "superagent:sh-global".into(),
            scope: TokenScope::Operate,
            login: "admin".into(),
            superagent: Some("sh-global".into()),
        };
        let request = RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/call".into(),
            params: json!({
                "name": "task_message",
                "arguments": {"project": "acme/pool", "task_id": id.as_str(), "text": "Merge dev first."},
            }),
        };
        let response = app
            .remote_mcp_handle(&global, request)
            .await
            .ok_or("no answer")?;
        let answer = serde_json::to_value(&response)?;
        assert_ne!(answer["result"]["isError"], true, "{answer}");
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
        let rows = audited(&data, &id)?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "superagent:admin");
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

const MINUTE: i64 = 60_000;

/// What a task says while a message of Yulius's own waits for its person's draft (SPEC-480
/// DRF-006).
fn held_message_flag() -> Value {
    json!({
        "kind": "yulius",
        "problem": "Yulius has a message for this session and does not type it: someone typed \
                    in the terminal and sent no prompt since, so their text may still be in the \
                    prompt box.",
        "action": "Send what you typed in the terminal, or clear it and send any prompt. Yulius \
                   types its message after the next prompt sent from there.",
        "link": null,
    })
}

/// The one line the session's terminal gets with that flag.
const HELD_MESSAGE_NOTICE: &str = "Yulius holds a message for this session: text typed here was \
    not sent and may still be in the prompt box. Send it, or clear it and send any prompt, and \
    Yulius types its message.";

/// [`app`] on a clock the test moves, which starts `behind_ms` behind the system's.
async fn app_behind(data: &Path, behind_ms: i64) -> Result<(Arc<App>, Arc<ShiftedClock>), Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let clock = Arc::new(ShiftedClock(AtomicI64::new(-behind_ms)));
    let probe = Arc::new(ScriptedUsage::default());
    let (app, _) = start_on(data, &fake, fake_agent(), 0, probe, clock.clone(), |_| {}).await?;
    Ok((app, clock))
}

/// A writer's terminal on `id` for the person `caller` acts for: what they type goes the way a
/// viewer's keys go.
async fn terminal(
    app: &Arc<App>,
    caller: &ProjectCaller,
    id: &TaskId,
) -> Result<(AuthContext, TerminalAttach), Error> {
    let ctx = AuthContext {
        user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let attach = app
        .terminal_open(&ctx, id.as_str(), Some((120, 40)))
        .await?;
    Ok((ctx, attach))
}

/// Whether the agent read `line` as a prompt of its own (the fake agent answers each prompt
/// with `ECHO: <prompt>`).
fn read_alone(app: &App, id: &TaskId, line: &str) -> bool {
    let answer = format!("ECHO: {line}");
    screen(app, id).lines().any(|shown| shown.trim() == answer)
}

/// SPEC-480 AC1, AC4 (DRF-001, DRF-004; issue #1199): text a person typed into the prompt box
/// and did not send keeps a message out however long ago they typed it. Two minutes later, and
/// eleven, the Superagent's message is refused with the reason, the agent got neither text,
/// and the task is not flagged (the sender was told). Once the person sent theirs, the message
/// arrives as a prompt of its own. The message used to be pasted behind a draft older than a
/// minute and submitted with it.
#[test]
fn a_persons_draft_keeps_the_message_out_however_old() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock) = app_behind(&data, 11 * MINUTE).await?;
        let (id, caller) = running(&app, &data).await?;
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        // They type half a prompt and leave it; two minutes pass.
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        clock.0.store(-9 * MINUTE, Ordering::SeqCst);

        let message = json!({"task_id": id.as_str(), "text": "Go on"});
        let Err(refused) = app.tool_task_message(&caller, &message).await else {
            // What the agent read: the person's text and the message, as one prompt.
            wait_for("what the agent read", Duration::from_secs(30), || async {
                screen(&app, &id).contains("ECHO: half a thought")
            })
            .await?;
            return Err(format!(
                "the message was typed onto the person's draft and submitted with it:\n{}",
                screen(&app, &id)
            )
            .into());
        };
        let why = format!(
            "a person typed into task #{}'s terminal and sent no prompt since, so their text may \
             still be in its prompt box and nothing is typed behind it: tell the person to send \
             it, or to clear it and send any prompt, then try again (when the task's owner says \
             to send past it: task_message with with_draft)",
            app.deps.store.task(&id).await?.ok_or("task")?.number
        );
        assert!(
            matches!(&refused, AppError::Conflict(said) if *said == why),
            "{refused:?}"
        );
        // Eleven minutes after their last key it is refused all the same, and its sender is
        // the one who was told: no flag, no line in the terminal.
        clock.0.store(0, Ordering::SeqCst);
        let again = app.tool_task_message(&caller, &message).await.err();
        assert!(
            matches!(&again, Some(AppError::Conflict(why)) if why.contains("prompt box")),
            "{again:?}"
        );
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        assert_eq!(notices(&mut watch), Vec::<String>::new());
        // Whatever reaches the agent shows within a second; nothing did.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let shown = screen(&app, &id);
        assert!(!shown.contains("ECHO: half a thought"), "{shown}");
        assert!(!shown.contains("Message from admin"), "{shown}");
        assert!(audited(&data, &id)?.is_empty());

        // They send it: the agent reads it alone, and the prompt box is empty again.
        app.terminal_input(&ctx, &attach, b"\r", false).await?;
        wait_for("the person's prompt", Duration::from_secs(30), || async {
            read_alone(&app, &id, "half a thought")
        })
        .await?;
        // The minute after their Enter passes (DRF-002), and the same message is typed.
        clock.0.store(MINUTE + 1_000, Ordering::SeqCst);
        let answer = app.tool_task_message(&caller, &message).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        wait_for("the message", Duration::from_secs(30), || async {
            read_alone(&app, &id, "Message from admin (through their Superagent):")
        })
        .await?;
        assert_eq!(audited(&data, &id)?.len(), 1);
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-487 TCT-006 (wall 2: "no clearing of a queue blocked by unsent terminal text"): the
/// task's owner tells their Superagent to send past what they left in the prompt box. With
/// `with_draft` the message is typed behind it and both are submitted as one prompt, which
/// empties the box, so Yulius's own messages go on. Without it the message is refused as
/// before; another person's Superagent cannot send past the owner's text; and a person who is
/// typing right now is not typed over.
#[test]
fn the_owner_sends_a_message_past_their_own_draft() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock) = app_behind(&data, 11 * MINUTE).await?;
        let (id, caller) = running(&app, &data).await?;
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        // They are at the keyboard: nothing is typed over them, with_draft or not.
        let past = json!({"task_id": id.as_str(), "text": "Go on", "with_draft": true});
        let typing = app.tool_task_message(&caller, &past).await.err();
        assert!(
            matches!(&typing, Some(AppError::Conflict(why)) if why.contains("in the last minute")),
            "{typing:?}"
        );
        clock.0.store(-9 * MINUTE, Ordering::SeqCst);
        // Without it: refused, and told how to send past it.
        let plain = json!({"task_id": id.as_str(), "text": "Go on"});
        let refused = app.tool_task_message(&caller, &plain).await.err();
        assert!(
            matches!(&refused, Some(AppError::Conflict(why)) if why.contains("with_draft")),
            "{refused:?}"
        );
        // The owner alone decides about their text: while the task is someone else's, the
        // same Superagent cannot send past it.
        let bob = yulius_app::model::UserRecord {
            id: UserId::parse("u-bob")?,
            github_login: Some("bob".into()),
            github_id: None,
            display_name: "Bob".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: yulius_domain::time::Timestamp(1),
            disabled_at: None,
        };
        let store = &app.deps.store;
        yulius_app::ports::store::UserStore::insert_user(&**store, &bob).await?;
        assert!(
            store
                .take_control(&id, &caller.user, &bob.id, app.now())
                .await?
        );
        let theirs = app.tool_task_message(&caller, &past).await.err();
        assert!(
            matches!(&theirs, Some(AppError::Forbidden(why))
                if why.contains("@bob") && why.contains("only they steer it")),
            "{theirs:?}"
        );
        assert!(
            store
                .take_control(&id, &bob.id, &caller.user, app.now())
                .await?
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!screen(&app, &id).contains("ECHO: half a thought"));
        assert!(audited(&data, &id)?.is_empty());
        // `with_draft` must be true or false.
        let odd = json!({"task_id": id.as_str(), "text": "Go on", "with_draft": "yes"});
        assert!(matches!(
            app.tool_task_message(&caller, &odd).await,
            Err(AppError::Invalid(_))
        ));

        // A prompt Yulius held for the person's decision is back in the box (TSK-013): it is
        // theirs to decide on its card, and no message carries it to the agent (review round
        // 1, F6).
        let held = yulius_app::intake::HoldRecord {
            id: "h-1".into(),
            task_id: id.clone(),
            class: yulius_domain::intake::PromptClass::NewScope,
            prompt: "half a thought".into(),
            prompt_sha256: "0".repeat(64),
            percent: 90,
            typed_by: Some(caller.user.clone()),
            state: yulius_app::intake::hold_state::PENDING.into(),
            created_at: app.now(),
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        };
        {
            store.insert_hold(&held).await?;
            let refused = app.tool_task_message(&caller, &past).await.err();
            assert!(
                matches!(&refused, Some(AppError::Conflict(why)) if why.contains("held")),
                "{refused:?}"
            );
            assert!(
                store
                    .resolve_hold(
                        "h-1",
                        yulius_app::intake::hold_state::PENDING,
                        yulius_app::intake::hold_state::DISMISSED,
                        "user:admin",
                        app.now(),
                        None,
                    )
                    .await?
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!screen(&app, &id).contains("ECHO: half a thought"));
        assert!(audited(&data, &id)?.is_empty());

        // The owner's: typed behind their text, one prompt.
        let answer = app.tool_task_message(&caller, &past).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        assert!(
            answer["note"]
                .as_str()
                .is_some_and(|note| note.contains("prompt box")),
            "{answer}"
        );
        // One prompt: their text, a blank line, the labelled message (review round 1, F9: the
        // two used to be glued, "half a thoughtMessage from admin").
        wait_for(
            "the draft and the message",
            Duration::from_secs(30),
            || async {
                let shown = screen(&app, &id);
                let mut lines = shown.lines().map(str::trim);
                lines.any(|line| line == "ECHO: half a thought")
                    && lines.next() == Some("")
                    && lines.next() == Some("Message from admin (through their Superagent):")
            },
        )
        .await?;
        assert!(
            !screen(&app, &id).contains("ECHO: Message from admin"),
            "the message went as a prompt of its own:\n{}",
            screen(&app, &id)
        );
        assert_eq!(audited(&data, &id)?.len(), 1);
        assert!(audited(&data, &id)?[0].1.contains("with_draft"));
        // The box is empty again: the next plain message is typed, a minute later.
        clock.0.store(-7 * MINUTE, Ordering::SeqCst);
        let again = app
            .tool_task_message(
                &caller,
                &json!({"task_id": id.as_str(), "text": "And then"}),
            )
            .await?;
        assert_eq!(again["delivered"], "sent", "{again}");
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-487 TCT-006 (review round 1, F1): a draft typed before a restart of the server is
/// known from its mark alone (SPEC-480 DRF-009), and a prompt Yulius types does not end such
/// a draft. The owner's message sent past it does: it carried the text to the agent, so the
/// mark leaves the run's folder and the next plain message is typed. It used to answer "the
/// box is empty again" while every later message was refused for the same draft.
#[test]
fn a_message_sent_past_a_draft_from_before_a_restart_ends_that_draft() -> TestResult {
    let (_dir, data) = data_dir()?;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let first = runtime()?;
    let (port, id, caller) = first.block_on(async {
        // The first server's clock is five minutes behind: the draft is that old afterwards.
        let clock = Arc::new(ShiftedClock(AtomicI64::new(-5 * MINUTE)));
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start_on(&data, &fake, fake_agent(), 0, probe, clock, |_| {}).await?;
        let (id, caller) = running(&app, &data).await?;
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        app.terminal_leave(&attach).await;
        wait_for(
            "the screen before the restart",
            Duration::from_secs(60),
            || async { screen(&app, &id).contains("half a thought") },
        )
        .await?;
        Ok::<_, Error>((port, id, caller))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_with(&data, &fake, fake_agent(), port, probe, |config| {
            config.goal_ready_ms = 600_000;
        })
        .await?;
        app.reconcile().await?;
        wait_for("the reattached screen", Duration::from_secs(30), || async {
            screen(&app, &id).contains("half a thought")
        })
        .await?;
        let run_dir = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("run")?
            .run_dir;
        let mark = Path::new(&run_dir).join("person-typed");
        assert!(mark.exists(), "the draft's mark is kept across the restart");
        let refused = send(&app, &caller, &id).await.err();
        assert!(
            matches!(&refused, Some(AppError::Conflict(why)) if why.contains("with_draft")),
            "{refused:?}"
        );
        let past = json!({"task_id": id.as_str(), "text": "Go on", "with_draft": true});
        let answer = app.tool_task_message(&caller, &past).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        wait_for(
            "the draft and the message",
            Duration::from_secs(30),
            || async {
                screen(&app, &id)
                    .lines()
                    .any(|line| line.trim() == "ECHO: half a thought")
            },
        )
        .await?;
        let ended = wait_for(
            "the draft's mark to go",
            Duration::from_secs(30),
            || async { !mark.exists() },
        )
        .await;
        let again = send(&app, &caller, &id).await;
        stop(&app, &id).await?;
        ended?;
        assert_eq!(again?["delivered"], "sent");
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-487 TCT-004 (wall 2: "set it to done / close out a merged task"): the owner closes a
/// running task whose agent is idle; its session ends gracefully and the task is `closed`, as
/// Yulius closes a finished task. Audited under the person's name.
#[test]
fn the_owner_closes_out_an_idle_task() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, caller) = running(&app, &data).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        wait_for("the turn's end", Duration::from_secs(30), || async {
            matches!(activity(&app, &id).await, Some(idle) if idle != Activity::Working)
        })
        .await?;
        let summary = app.close_task_for_person(&ctx, id.as_str()).await?;
        assert_eq!(summary.id, id.as_str());
        wait_for("the task to close", Duration::from_secs(30), || async {
            matches!(
                app.deps.store.task(&id).await,
                Ok(Some(task)) if task.status == TaskStatus::Closed
            )
        })
        .await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        let closed: i64 = db.query_row(
            "SELECT count(*) FROM audit_log WHERE action = 'task.closed_by_person' AND target = ?1",
            [id.as_str()],
            |row| row.get(0),
        )?;
        assert_eq!(closed, 1);
        // It has ended: a second close says so.
        let again = app.close_task_for_person(&ctx, id.as_str()).await.err();
        assert!(
            matches!(&again, Some(AppError::Conflict(why)) if why.contains("has ended")),
            "{again:?}"
        );
        Ok(())
    })
}

/// SPEC-490 OWN-002, OWN-005 (review round 1): a person who does not own a task closes it out
/// too; its audit names them and the owner, and its terminal says who closed it.
#[test]
fn someone_else_closes_out_an_idle_task_and_is_named() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, caller) = running(&app, &data).await?;
        let owner = app.deps.store.user(&caller.user).await?.ok_or("user")?;
        let other = yulius_app::model::UserRecord {
            id: UserId::parse("u-other")?,
            github_login: Some("other".into()),
            github_id: Some(4242),
            display_name: "Other".into(),
            password_hash: None,
            is_superadmin: false,
            is_instance_admin: false,
            ..owner.clone()
        };
        yulius_app::ports::store::UserStore::insert_user(&*app.deps.store, &other).await?;
        // They may work in the project (a cached GitHub permission, as a sign-in leaves).
        let sandbox = app.deps.store.task(&id).await?.ok_or("task")?.sandbox_id;
        yulius_app::ports::store::PermissionStore::put_access(
            &*app.deps.store,
            &other.id,
            &sandbox,
            yulius_domain::permission::AccessLevel::Write,
            app.now(),
        )
        .await?;
        let ctx = AuthContext {
            user: other,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        wait_for("the turn's end", Duration::from_secs(30), || async {
            matches!(activity(&app, &id).await, Some(idle) if idle != Activity::Working)
        })
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.close_task_for_person(&ctx, id.as_str()).await?;
        wait_for("the task to close", Duration::from_secs(30), || async {
            matches!(
                app.deps.store.task(&id).await,
                Ok(Some(task)) if task.status == TaskStatus::Closed
            )
        })
        .await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        let detail: String = db.query_row(
            "SELECT detail FROM audit_log WHERE action = 'task.closed_by_person' AND target = ?1",
            [id.as_str()],
            |row| row.get(0),
        )?;
        let detail: Value = serde_json::from_str(&detail)?;
        assert!(detail["owner"].is_string(), "{detail}");
        let actor: String = db.query_row(
            "SELECT actor FROM audit_log WHERE action = 'task.closed_by_person' AND target = ?1",
            [id.as_str()],
            |row| row.get(0),
        )?;
        assert_ne!(
            Some(&actor),
            detail["owner"].as_str().map(str::to_owned).as_ref(),
            "{detail}"
        );
        let said = notices(&mut watch);
        assert!(
            said.iter().any(|n| *n == format!("Closed by @{actor}.")),
            "{said:?}"
        );
        Ok(())
    })
}

/// SPEC-480 AC2, AC4 (DRF-003, DRF-005, DRF-006, DRF-007): a message of Yulius's own (here the
/// MCP watch's `/mcp reconnect yulius`, a command) waits for a person's draft. Two minutes
/// after their last key nothing is typed and nobody is told; eleven minutes after it the task
/// is flagged with the reason and its terminal says so, once. After the person sent their text
/// the next pass types the command, which the agent reads alone.
#[test]
fn a_message_of_yulius_waits_for_a_persons_draft_and_tells_them() -> TestResult {
    use yulius_app::mcp_watch::{RECONNECT_TEXT, WatchAction, WatchBounds};
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock) = app_behind(&data, 13 * MINUTE).await?;
        let (id, caller) = running(&app, &data).await?;
        wait_for("the first turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await.is_some_and(Activity::is_idle)
        })
        .await?;
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        let bounds = WatchBounds {
            silent_after_ms: 1_000,
            reconnect_gap_ms: 60_000,
            ..WatchBounds::new(yulius_domain::time::Timestamp(0))
        };
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;

        // Two minutes later its Yulius MCP has long been silent, and the reconnect waits.
        clock.0.store(-11 * MINUTE, Ordering::SeqCst);
        assert!(app.watch_mcp(&bounds).await.is_empty());
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        let mut said = notices(&mut watch);
        assert!(said.is_empty(), "told within ten minutes: {said:?}");

        // Eleven minutes after their last key the person is told, once.
        clock.0.store(-2 * MINUTE, Ordering::SeqCst);
        for _ in 0..2 {
            assert!(app.watch_mcp(&bounds).await.is_empty());
        }
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            held_message_flag()
        );
        said.extend(notices(&mut watch));
        assert_eq!(said, [HELD_MESSAGE_NOTICE]);
        // A flag that ended while the command still waits (a turn's end settles a flagged
        // task) is raised again at the next pass; the terminal has its line already.
        let flagged = support::attention(&app, id.as_str()).await?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Waiting), app.now())
            .await?;
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        assert!(app.watch_mcp(&bounds).await.is_empty());
        assert_eq!(support::attention(&app, id.as_str()).await?, flagged);
        assert_eq!(notices(&mut watch), Vec::<String>::new());
        // Whatever reaches the agent shows within a second; nothing did.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let shown = screen(&app, &id);
        assert!(!shown.contains("ECHO: half a thought"), "{shown}");
        assert!(!shown.contains("ECHO: /mcp"), "{shown}");

        // They send it: the flag ends with the hold. The minute after their Enter passes, and
        // the next pass types the command.
        app.terminal_input(&ctx, &attach, b"\r", false).await?;
        wait_for("the person's prompt", Duration::from_secs(30), || async {
            read_alone(&app, &id, "half a thought")
        })
        .await?;
        wait_for("the flag's end", Duration::from_secs(30), || async {
            matches!(support::attention(&app, id.as_str()).await, Ok(Value::Null))
        })
        .await?;
        clock.0.store(-MINUTE + 1_000, Ordering::SeqCst);
        // Passes run until the turn of the person's prompt has closed.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut typed = Vec::new();
        while typed.is_empty() && Instant::now() < deadline {
            typed = app.watch_mcp(&bounds).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(typed, vec![(id.clone(), WatchAction::Reconnect)]);
        wait_for(
            "the command read alone",
            Duration::from_secs(30),
            || async { read_alone(&app, &id, RECONNECT_TEXT) },
        )
        .await?;
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A fake agent that draws full screen as Claude Code does: its prompt box (a row `❯` between
/// two rules) with the cursor in it. The PTY echoes what a person types into that row, so a
/// draft shows in the box, and `Ctrl+U` erases it there.
pub(super) fn full_screen() -> FakeAgentOptions {
    FakeAgentOptions {
        tui: true,
        ..fake_agent()
    }
}

/// [`app_behind`] with the full-screen agent, and a running task whose first turn ended: its
/// prompt box is drawn empty.
async fn idle_full_screen(
    data: &Path,
    behind_ms: i64,
) -> Result<(Arc<App>, Arc<ShiftedClock>, TaskId, ProjectCaller), Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let clock = Arc::new(ShiftedClock(AtomicI64::new(-behind_ms)));
    let probe = Arc::new(ScriptedUsage::default());
    let (app, _) = start_on(data, &fake, full_screen(), 0, probe, clock.clone(), |_| {}).await?;
    let (id, caller) = running(&app, data).await?;
    wait_for("the first turn's end", Duration::from_secs(30), || async {
        activity(&app, &id).await.is_some_and(Activity::is_idle)
    })
    .await?;
    wait_for("the empty prompt box", Duration::from_secs(30), || async {
        fixture_box_empty(&app, &id)
    })
    .await?;
    Ok((app, clock, id, caller))
}

/// The mark of a possible draft in the folder of `id`'s live run (SPEC-480 DRF-009).
async fn draft_mark(app: &App, id: &TaskId) -> Result<std::path::PathBuf, Error> {
    let run_dir = app.deps.store.live_attempt(id).await?.ok_or("run")?.run_dir;
    Ok(Path::new(&run_dir).join("person-typed"))
}

/// Read the fake agent's rendered input independently of the production screen recognizer.
fn fixture_box_empty(app: &App, id: &TaskId) -> bool {
    screen(app, id).lines().any(|row| row.trim_end() == "❯")
}

/// Wait for the fake agent's SIGWINCH redraw before sending PTY-echoed draft text.
pub(super) async fn full_screen_terminal(
    app: &Arc<App>,
    caller: &ProjectCaller,
    id: &TaskId,
) -> Result<(AuthContext, TerminalAttach), Error> {
    let terminal = terminal(app, caller, id).await?;
    wait_for("the resized box", Duration::from_secs(30), || async {
        fixture_box_empty(app, id) && screen(app, id).contains("120×40")
    })
    .await?;
    Ok(terminal)
}

/// A key that leaves nothing in the prompt box: Backspace in an empty one.
const KEY_WITHOUT_TEXT: &[u8] = b"\x7f";

/// SPEC-572 AC1: a person presses a key that leaves nothing in the
/// prompt box (Escape, an arrow, an answered dialog; here Backspace). Within the minute a
/// message is kept out as for anyone at the keyboard. After it, the screen shows the box
/// empty and the message is typed: the agent reads it as a prompt of its own, and the possible
/// draft's mark is gone. Such a key used to keep every message out until a person sent a
/// prompt (SPEC-480 took every keystroke for a possible draft).
#[test]
fn a_key_that_left_nothing_in_the_prompt_box_holds_no_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock, id, caller) = idle_full_screen(&data, 13 * MINUTE).await?;
        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, KEY_WITHOUT_TEXT, false)
            .await?;
        let mark = draft_mark(&app, &id).await?;
        assert!(mark.exists(), "the key is a possible draft until looked at");

        // Half a minute later the person may still be there: refused, whatever the screen shows.
        let message = json!({"task_id": id.as_str(), "text": "Go on"});
        clock.0.store(-13 * MINUTE + 30_000, Ordering::SeqCst);
        let young = app.tool_task_message(&caller, &message).await.err();
        assert!(
            matches!(&young, Some(AppError::Conflict(why)) if why.contains("prompt box")),
            "{young:?}"
        );
        assert!(mark.exists());

        // Two minutes after the key the box is seen empty, and the message is typed.
        clock.0.store(-11 * MINUTE, Ordering::SeqCst);
        let answer = app.tool_task_message(&caller, &message).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        wait_for("the message", Duration::from_secs(30), || async {
            read_alone(&app, &id, "Message from admin (through their Superagent):")
        })
        .await?;
        assert!(!mark.exists(), "a box seen empty keeps no draft's mark");
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        assert_eq!(notices(&mut watch), Vec::<String>::new());
        assert_eq!(audited(&data, &id)?.len(), 1);
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-572 AC1 (the reported screen): a task whose person pressed a key that left
/// nothing in the prompt box eleven minutes ago gets a message of Yulius's own (here the MCP
/// watch's `/mcp reconnect yulius`) at the first pass, and nobody is asked for anything. The
/// task used to read Needs your action, "Yulius has a message for this session and does not
/// type it", with the command never typed.
#[test]
fn a_message_of_yulius_is_typed_into_a_box_a_key_left_empty() -> TestResult {
    use yulius_app::mcp_watch::{RECONNECT_TEXT, WatchAction, WatchBounds};
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock, id, caller) = idle_full_screen(&data, 13 * MINUTE).await?;
        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        let bounds = WatchBounds {
            silent_after_ms: 1_000,
            reconnect_gap_ms: 60_000,
            ..WatchBounds::new(yulius_domain::time::Timestamp(0))
        };
        app.terminal_input(&ctx, &attach, KEY_WITHOUT_TEXT, false)
            .await?;

        clock.0.store(-2 * MINUTE, Ordering::SeqCst);
        let typed = app.watch_mcp(&bounds).await;
        assert_eq!(
            typed,
            vec![(id.clone(), WatchAction::Reconnect)],
            "attention: {}",
            support::attention(&app, id.as_str()).await?
        );
        wait_for(
            "the command read alone",
            Duration::from_secs(30),
            || async { read_alone(&app, &id, RECONNECT_TEXT) },
        )
        .await?;
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        assert_eq!(notices(&mut watch), Vec::<String>::new());
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-572 AC2: with the prompt box on screen, text a person typed into it
/// and left there holds a message of Yulius's own as SPEC-480 requires: eleven minutes later
/// nothing is typed, the task is flagged and its terminal says so. The person then clears the
/// text and sends nothing. Within the minute the command still waits; after it the next pass
/// types the command, which the agent reads alone, and the flag ends with the hold.
#[test]
fn a_draft_the_screen_shows_holds_until_it_is_cleared() -> TestResult {
    use yulius_app::mcp_watch::{RECONNECT_TEXT, WatchAction, WatchBounds};
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock, id, caller) = idle_full_screen(&data, 13 * MINUTE).await?;
        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        let bounds = WatchBounds {
            silent_after_ms: 1_000,
            reconnect_gap_ms: 60_000,
            ..WatchBounds::new(yulius_domain::time::Timestamp(0))
        };
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        wait_for("the draft in the box", Duration::from_secs(30), || async {
            screen(&app, &id).contains("❯ half a thought")
        })
        .await?;

        // Eleven minutes later: held, flagged, said once.
        clock.0.store(-2 * MINUTE, Ordering::SeqCst);
        assert!(app.watch_mcp(&bounds).await.is_empty());
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            held_message_flag()
        );
        assert_eq!(notices(&mut watch), [HELD_MESSAGE_NOTICE]);

        // They clear it. Half a minute later the command still waits and the flag stands.
        app.terminal_input(&ctx, &attach, b"\x15", false).await?;
        wait_for("the cleared box", Duration::from_secs(30), || async {
            fixture_box_empty(&app, &id)
        })
        .await?;
        clock.0.store(-2 * MINUTE + 30_000, Ordering::SeqCst);
        assert!(app.watch_mcp(&bounds).await.is_empty());
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            held_message_flag()
        );

        // Past the minute the box is seen empty: the command is typed, the flag ends.
        clock.0.store(-MINUTE + 1_000, Ordering::SeqCst);
        assert_eq!(
            app.watch_mcp(&bounds).await,
            vec![(id.clone(), WatchAction::Reconnect)]
        );
        wait_for(
            "the command read alone",
            Duration::from_secs(30),
            || async { read_alone(&app, &id, RECONNECT_TEXT) },
        )
        .await?;
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        let shown = screen(&app, &id);
        assert!(!shown.contains("ECHO: half a thought"), "{shown}");
        assert!(!draft_mark(&app, &id).await?.exists());
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-572 AC3: a box seen empty ends the hold by itself, with no prompt hook to do it
/// (a command Claude Code runs without one, a CLI without hooks; here an agent that runs none):
/// a possible draft is flagged, its person clears it, and the pass that types the command has
/// by then ended the flag and removed the draft's mark.
#[test]
fn a_box_seen_empty_ends_the_hold_its_mark_and_its_flag_without_a_prompt_hook() -> TestResult {
    use yulius_app::mcp_watch::{WatchAction, WatchBounds};
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let clock = Arc::new(ShiftedClock(AtomicI64::new(-13 * MINUTE)));
        let probe = Arc::new(ScriptedUsage::default());
        let hookless = FakeAgentOptions {
            hooks: false,
            ..full_screen()
        };
        let (app, _) = start_on(&data, &fake, hookless, 0, probe, clock.clone(), |config| {
            config.goal_ready_ms = 2_000;
        })
        .await?;
        let (id, caller) = running(&app, &data).await?;
        // Without a Stop hook the task never reads idle; the watch asks the transcript whether
        // a turn is open, so its passes are repeated until the first turn has closed.
        wait_for("the empty prompt box", Duration::from_secs(30), || async {
            fixture_box_empty(&app, &id)
        })
        .await?;
        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        let bounds = WatchBounds {
            silent_after_ms: 1_000,
            reconnect_gap_ms: 60_000,
            ..WatchBounds::new(yulius_domain::time::Timestamp(0))
        };
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        wait_for("the draft in the box", Duration::from_secs(30), || async {
            screen(&app, &id).contains("❯ half a thought")
        })
        .await?;
        let mark = draft_mark(&app, &id).await?;
        clock.0.store(-2 * MINUTE, Ordering::SeqCst);
        wait_for("the flag", Duration::from_secs(30), || async {
            app.watch_mcp(&bounds).await.is_empty()
                && matches!(
                    support::attention(&app, id.as_str()).await,
                    Ok(flag) if flag == held_message_flag()
                )
        })
        .await?;
        assert!(mark.exists());

        app.terminal_input(&ctx, &attach, b"\x15", false).await?;
        wait_for("the cleared box", Duration::from_secs(30), || async {
            fixture_box_empty(&app, &id)
        })
        .await?;
        clock.0.store(-MINUTE + 1_000, Ordering::SeqCst);
        assert_eq!(
            app.watch_mcp(&bounds).await,
            vec![(id.clone(), WatchAction::Reconnect)]
        );
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        assert!(!mark.exists(), "the mark went with the hold");
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-572 AC3: a key typed after the screen was read, while the hold's
/// mark and flag are ended, is a possible draft of its own: the hold is judged again before
/// anything is pasted, and the message is refused. The paste used to go in behind that key.
#[test]
fn a_key_typed_while_the_empty_box_is_taken_in_holds_the_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock, id, caller) = idle_full_screen(&data, 13 * MINUTE).await?;
        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, KEY_WITHOUT_TEXT, false)
            .await?;
        clock.0.store(-11 * MINUTE, Ordering::SeqCst);
        // The person's next key arrives exactly there (recorded as a viewer's key is, before
        // it is passed on).
        let mark = draft_mark(&app, &id).await?;
        let interleaved = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = interleaved.clone();
        let (typist, task, person) = (app.clone(), id.clone(), caller.user.clone());
        yulius_app::race::on(
            yulius_app::race::Point::BoxSeenEmpty,
            Path::new(id.as_str()),
            move || {
                typist.record_typing(&task, &person);
                observed.store(true, Ordering::SeqCst);
            },
        );
        let message = json!({"task_id": id.as_str(), "text": "Go on"});
        let refused = app.tool_task_message(&caller, &message).await.err();
        assert!(
            matches!(&refused, Some(AppError::Conflict(why)) if why.contains("prompt box")),
            "{refused:?}"
        );
        assert!(
            interleaved.load(Ordering::SeqCst),
            "the new key arrived during the empty-box check"
        );
        assert!(mark.exists(), "the later key keeps the recovery mark");
        // Refused before anything is written to the session, and so not audited.
        assert!(audited(&data, &id)?.is_empty());
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A stash hint drawn during mark lookup invalidates the earlier empty frame.
#[test]
fn a_stash_hint_that_replaces_the_empty_box_proof_keeps_the_mark() -> TestResult {
    runtime()?.block_on(async {
        let (_dir, data) = data_dir()?;
        let (app, clock, id, caller) = idle_full_screen(&data, 13 * MINUTE).await?;
        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, KEY_WITHOUT_TEXT, false)
            .await?;
        clock.0.store(-11 * MINUTE, Ordering::SeqCst);
        let runtime = app.runtime(&id);
        yulius_app::race::on(
            yulius_app::race::Point::BoxSeenEmpty,
            Path::new(id.as_str()),
            move || {
                let hub = &runtime.hub;
                let (_, rows) = hub.size();
                // Retain the input cursor while changing only the hint above its upper rule.
                hub.output(
                    hub.output_seq() + 1,
                    format!("\x1b7\x1b[{};1H› stashed\x1b8", rows - 4).as_bytes(),
                );
            },
        );
        let reply = send(&app, &caller, &id).await;
        assert!(
            screen(&app, &id).contains("› stashed"),
            "the redraw occurred"
        );
        assert!(matches!(reply, Err(AppError::Conflict(_))));
        assert!(draft_mark(&app, &id).await?.exists());
        assert!(audited(&data, &id)?.is_empty());
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })
}

/// SPEC-572 AC3: after a restart of the server the possible draft is
/// known from its mark alone, and the screen is the process host's snapshot of what the
/// session showed. Clearing an old draft just before restart still holds the message
/// for the typing minute. After a real repaint and the minute, it arrives alone.
#[test]
fn a_key_before_a_restart_that_left_nothing_holds_no_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let first = runtime()?;
    let (port, id, caller) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let clock = Arc::new(ShiftedClock(AtomicI64::new(-2 * MINUTE)));
        let (app, port) =
            start_on(&data, &fake, full_screen(), 0, probe, clock.clone(), |_| {}).await?;
        let (id, caller) = running(&app, &data).await?;
        wait_for("the first turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await.is_some_and(Activity::is_idle) && fixture_box_empty(&app, &id)
        })
        .await?;
        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"draft", false).await?;
        wait_for("the draft to be drawn", Duration::from_secs(30), || async {
            screen(&app, &id).contains("❯ draft")
        })
        .await?;
        clock.0.store(0, Ordering::SeqCst);
        app.terminal_input(&ctx, &attach, b"\x15", false).await?;
        wait_for("the cleared box", Duration::from_secs(30), || async {
            fixture_box_empty(&app, &id)
        })
        .await?;
        app.terminal_leave(&attach).await;
        assert!(draft_mark(&app, &id).await?.exists());
        Ok::<_, Error>((port, id, caller))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let clock = Arc::new(ShiftedClock(AtomicI64::new(0)));
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_on(
            &data,
            &fake,
            full_screen(),
            port,
            probe,
            clock.clone(),
            |_| {},
        )
        .await?;
        app.reconcile().await?;
        wait_for(
            "the idle host snapshot",
            Duration::from_secs(30),
            || async { fixture_box_empty(&app, &id) },
        )
        .await?;
        let mark = draft_mark(&app, &id).await?;
        // The restarted server knows the key from its mark: within the minute it holds.
        let young = send(&app, &caller, &id).await.err();
        assert!(
            matches!(&young, Some(AppError::Conflict(why)) if why.contains("prompt box")),
            "{young:?}"
        );
        assert!(mark.exists());
        clock.0.store(2 * MINUTE, Ordering::SeqCst);
        let answer = send(&app, &caller, &id).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        wait_for("the message", Duration::from_secs(30), || async {
            read_alone(&app, &id, "Message from admin (through their Superagent):")
        })
        .await?;
        assert!(!mark.exists(), "a box seen empty keeps no draft's mark");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn open_pull_for_news(
    app: &App,
    data: &Path,
    task: &yulius_app::model::TaskRecord,
) -> Result<String, Error> {
    use crate::pr_watch::git;
    use yulius_app::delivery::DeliveryRecord;
    use yulius_domain::delivery::DeliveryStage;
    let seed_dir = data.join("seed");
    git(&seed_dir, &["push", "-q", "origin", "main:dev"])?;
    let repo = data
        .join("sandboxes")
        .join(task.sandbox_id.as_str())
        .join("repo");
    git(&repo, &["fetch", "-q", "origin"])?;
    let worktree = task.worktree_path.clone().ok_or("no worktree")?;
    let worktree = Path::new(&worktree);
    std::fs::write(worktree.join("README.md"), "# the task's line\n")?;
    git(worktree, &["commit", "-q", "-am", "task"])?;
    let head = git(worktree, &["rev-parse", "HEAD"])?;
    let open = DeliveryRecord {
        task_id: task.id.clone(),
        stage: DeliveryStage::PrOpen,
        pr_number: Some(7),
        updated_at: app.now(),
    };
    app.deps
        .store
        .transition_delivery(&DeliveryStage::None, &open)
        .await?;
    Ok(head)
}

// The screen reported in task #587, with Codex's first menu item selected.
const CODEX_RATE_LIMIT_PICKER: &str = concat!(
    "\x1b[?1049h\x1b[H\x1b[J\x1b[?25l\x1b[34;1H",
    "  Approaching rate limits\r\n",
    "  Switch to gpt-6-luna for lower credit usage?\r\n",
    "› 1. Switch to gpt-6-luna                   Fast and affordable model for easier tasks.\r\n",
    "  2. Keep current model\r\n",
    "  3. Keep current model (never show again)  Hide future rate limit reminders about switching models\r\n",
    "  enter select · esc back",
);
const CODEX_EMPTY_COMPOSER: &str = concat!(
    "\x1b[?1049h\x1b[H\x1b[J",
    "\x1b[34;1H\x1b[1m›\x1b[22m \x1b[2mAsk Codex to do anything\x1b[0m",
    "\x1b[40;1H  \x1b[1m←\x1b[22m for agents · \x1b[1m?\x1b[22m for shortcuts",
    "\x1b[34;3H\x1b[?25h",
);

async fn codex_with_pr_news(
    data: &Path,
) -> Result<(Arc<App>, TaskId, ProjectCaller, TcpListener), Error> {
    codex_with_pr_fixture(data, fake_agent()).await
}

async fn codex_with_pr_fixture(
    data: &Path,
    options: FakeAgentOptions,
) -> Result<(Arc<App>, TaskId, ProjectCaller, TcpListener), Error> {
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::RunState;

    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let clock = Arc::new(ShiftedClock(AtomicI64::new(-13 * MINUTE)));
    let probe = Arc::new(ScriptedUsage::default());
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let accepting = tokio::net::TcpListener::from_std(listener.try_clone()?)?;
    let app = start_on_listener(data, &fake, options, accepting, probe, clock).await?;
    let (sandbox, user) = seed(&app, data, &[]).await?;
    crate::codex::add_codex_account(&app, "a-codex", r#"{"OPENAI_API_KEY":"synthetic"}"#).await?;
    let id = crate::codex::new_codex_task(&app, &sandbox, &user, "Codex PR news").await?;
    wait_for(
        "first Codex turn ended",
        Duration::from_secs(30),
        || async {
            screen(&app, &id).contains("ECHO: Codex PR news")
                && activity(&app, &id).await.is_some_and(Activity::is_idle)
        },
    )
    .await?;
    let task = app.deps.store.task(&id).await?.ok_or("task")?;
    let head = open_pull_for_news(&app, data, &task).await?;
    assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
    let (red, job) = crate::pr_watch::run(&sandbox, "r1", &head, RunState::Failed, None, 1);
    app.deps.store.insert_ci_run(&red, &[job]).await?;
    Ok((app, id, superagent(&sandbox, &user, "admin"), listener))
}

fn paint_codex(app: &App, id: &TaskId, picture: &str) {
    let runtime = app.runtime(id);
    runtime
        .hub
        .output(runtime.hub.output_seq() + 1, picture.as_bytes());
}

#[test]
fn reattached_idle_codex_repaints_and_receives_its_ci_notice() -> TestResult {
    use yulius_app::pr_watch::WatchBounds;

    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (id, listener) = first.block_on(async {
        let (_app, id, _, listener) = codex_with_pr_fixture(&data, full_screen()).await?;
        Ok::<_, Error>((id, listener))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    runtime()?.block_on(async {
        let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
        let app = start_on_listener(
            &data,
            &fake,
            full_screen(),
            tokio::net::TcpListener::from_std(listener)?,
            Arc::new(ScriptedUsage::default()),
            Arc::new(ShiftedClock(AtomicI64::new(0))),
        )
        .await?;
        app.reconcile().await?;
        let rt = app.runtime(&id);
        let (cols, rows) = rt.hub.size();
        // Legacy hosts supply a snapshot without redraw provenance. Only the child's
        // SIGWINCH handler can replace this synthetic snapshot with a complete frame.
        rt.hub.reset(
            rt.hub.output_seq(),
            CODEX_EMPTY_COMPOSER.as_bytes(),
            cols,
            rows,
        );
        let news = app.watch_prs(&WatchBounds::default()).await;
        assert_eq!(news.len(), 1, "{news:?}");
        wait_for(
            "PR news read by the reattached child",
            Duration::from_secs(10),
            || async { screen(&app, &id).contains("ECHO: Yulius, about your pull request #7") },
        )
        .await?;
        assert_eq!(rt.hub.size(), (cols, rows));
        assert!(screen(&app, &id).contains(&format!("{cols}×{rows}")));
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        stop(&app, &id).await
    })
}

#[test]
fn unreadable_codex_raises_the_undelivered_ci_notice_on_the_task() -> TestResult {
    use yulius_app::pr_watch::WatchBounds;

    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, _, _) = codex_with_pr_news(&data).await?;
        paint_codex(&app, &id, "\x1b[?1049h\x1b[?25l\x1b[H\x1b[J");
        let news = tokio::time::timeout(
            Duration::from_secs(3),
            app.watch_prs(&WatchBounds::default()),
        )
        .await?;
        assert!(news.is_empty());
        let request = app
            .deps
            .store
            .task_needs_you(&id)
            .await?
            .ok_or("no Needs you for unreadable Codex")?;
        assert!(request.action.contains("terminal"), "{request:?}");
        assert!(
            request
                .happened
                .as_deref()
                .is_some_and(|s| s.contains("#7") && s.contains("fail")),
            "{request:?}"
        );
        assert!(!screen(&app, &id).contains("ECHO:"));
        paint_codex(&app, &id, CODEX_EMPTY_COMPOSER);
        assert_eq!(app.watch_prs(&WatchBounds::default()).await.len(), 1);
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        stop(&app, &id).await
    })
}

#[test]
fn cancelling_a_codex_message_during_repaint_restores_the_size_without_typing() -> TestResult {
    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, caller, _) = codex_with_pr_news(&data).await?;
        let rt = app.runtime(&id);
        paint_codex(&app, &id, "\x1b[?1049h\x1b[?25l\x1b[H\x1b[J");
        let original = rt.hub.size();
        let (writing, target) = (Arc::clone(&app), id.clone());
        let sending = tokio::spawn(async move { send(&writing, &caller, &target).await });
        wait_for(
            "temporary repaint width",
            Duration::from_secs(3),
            || async { rt.hub.size() != original },
        )
        .await?;
        sending.abort();
        assert!(sending.await.is_err_and(|error| error.is_cancelled()));
        wait_for("restored PTY geometry", Duration::from_secs(3), || async {
            rt.hub.size() == original
        })
        .await?;
        assert!(audited(&data, &id)?.is_empty());
        assert!(!screen(&app, &id).contains("ECHO:"));
        stop(&app, &id).await
    })
}

#[test]
fn an_unreadable_codex_prompt_asks_its_owner_without_replacing_an_open_question() -> TestResult {
    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, caller, _) = codex_with_pr_news(&data).await?;
        paint_codex(&app, &id, "\x1b[?1049h\x1b[?25l\x1b[H\x1b[J");
        let refused =
            tokio::time::timeout(Duration::from_secs(3), send(&app, &caller, &id)).await?;
        assert!(matches!(refused, Err(AppError::Conflict(_))));
        let mut question = app
            .deps
            .store
            .task_needs_you(&id)
            .await?
            .ok_or("missing owner request")?;
        assert_eq!(
            question.happened.as_deref(),
            Some("A message could not be submitted to Codex.")
        );
        question.action = "Confirm the project choice".into();
        app.deps
            .store
            .set_task_needs_you(&id, Some(&question), app.now())
            .await?;
        assert!(send(&app, &caller, &id).await.is_err());
        assert_eq!(app.deps.store.task_needs_you(&id).await?, Some(question));
        stop(&app, &id).await
    })
}

#[test]
fn a_repeated_codex_notice_write_failure_raises_the_pending_result() -> TestResult {
    use yulius_app::pr_watch::WatchBounds;
    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, _, _) = codex_with_pr_news(&data).await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("attempt")?;
        std::fs::create_dir(Path::new(&attempt.run_dir).join("codex-submit-held"))?;
        let bounds = WatchBounds::default();
        assert!(app.watch_prs(&bounds).await.is_empty());
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        assert!(app.watch_prs(&bounds).await.is_empty());
        let request = app
            .deps
            .store
            .task_needs_you(&id)
            .await?
            .ok_or("missing PR delivery refusal")?;
        assert!(
            request
                .problem
                .as_deref()
                .is_some_and(|s| s.contains("record the Codex paste"))
        );
        assert!(
            request
                .happened
                .as_deref()
                .is_some_and(|s| s.contains("#7"))
        );
        stop(&app, &id).await
    })
}

#[test]
fn the_codex_rate_limit_picker_holds_messages_until_it_is_dismissed() -> TestResult {
    use yulius_app::pr_watch::WatchBounds;

    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, caller, _) = codex_with_pr_news(&data).await?;
        paint_codex(&app, &id, CODEX_RATE_LIMIT_PICKER);
        assert!(screen(&app, &id).contains("› 1. Switch to gpt-6-luna"));
        let bounds = WatchBounds::default();
        let reply = send(&app, &caller, &id).await;
        assert!(
            matches!(&reply, Err(AppError::Conflict(why)) if why.contains("rate-limit model picker")),
            "{reply:?}"
        );
        assert!(app.watch_prs(&bounds).await.is_empty());
        assert!(audited(&data, &id)?.is_empty());
        assert!(!screen(&app, &id).contains("ECHO: "));

        // A quotation in the conversation is ordinary output. The composer has the cursor.
        let conversation = format!(
            "{CODEX_EMPTY_COMPOSER}\x1b7\x1b[2;1HApproaching rate limits\r\n\
             Switch to gpt-6-luna for lower credit usage?\x1b8"
        );
        paint_codex(&app, &id, &conversation);
        let news = app.watch_prs(&bounds).await;
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(news[0].0, id);
        wait_for("deferred PR notice", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Yulius, about your pull request #7")
        })
        .await?;
        assert!(app.watch_prs(&bounds).await.is_empty());
        stop(&app, &id).await
    })
}

#[test]
fn the_codex_rate_limit_picker_between_paste_and_enter_holds_the_submit() -> TestResult {
    use yulius_app::pr_watch::WatchBounds;
    use yulius_app::race::{Point, on};

    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, _, _) = codex_with_pr_news(&data).await?;
        let drawn = (Arc::clone(&app), id.clone());
        on(Point::PrNoticePasted, Path::new(id.as_str()), move || {
            paint_codex(&drawn.0, &drawn.1, CODEX_RATE_LIMIT_PICKER);
        });
        let bounds = WatchBounds::default();
        assert!(
            app.watch_prs(&bounds).await.is_empty(),
            "Enter was withheld"
        );
        assert!(screen(&app, &id).contains("Approaching rate limits"));

        // The picker can restore what was pasted. Wait for a confirmed empty composer
        // before the watcher pastes again.
        paint_codex(
            &app,
            &id,
            "\x1b[H\x1b[J\x1b[34;1H› Yulius, about your pull request #7\x1b[?25h",
        );
        assert!(app.watch_prs(&bounds).await.is_empty());
        // Dismiss and clear in the native TUI, simulated by its repaint in this fixture.
        paint_codex(&app, &id, CODEX_EMPTY_COMPOSER);
        assert_eq!(app.watch_prs(&bounds).await.len(), 1, "notice released");
        assert!(app.watch_prs(&bounds).await.is_empty());
        stop(&app, &id).await
    })
}

#[test]
fn the_codex_rate_limit_picker_holds_input_when_its_heading_wraps() -> TestResult {
    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, caller, _) = codex_with_pr_news(&data).await?;
        app.resize_run(&id, 20, 40).await?;
        paint_codex(
            &app,
            &id,
            concat!(
                "\x1b[?1049h\x1b[H\x1b[J\x1b[?25l\x1b[34;1H",
                "  Approaching\r\n  rate limits\r\n",
                "  Switch to\r\n  gpt-6-luna for\r\n",
                "  › 1. Switch to\r\n",
                "    2. Keep current",
            ),
        );
        assert_eq!(app.runtime(&id).hub.size(), (20, 40));
        let reply = send(&app, &caller, &id).await;
        assert!(
            matches!(&reply, Err(AppError::Conflict(why)) if why.contains("rate-limit model picker")),
            "{reply:?}"
        );
        stop(&app, &id).await
    })
}

#[test]
fn the_codex_rate_limit_picker_holds_enter_when_resize_crops_out_the_picker() -> TestResult {
    use yulius_app::pr_watch::WatchBounds;
    use yulius_app::race::{Point, on};

    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, _, _) = codex_with_pr_news(&data).await?;
        let drawn = (Arc::clone(&app), id.clone());
        on(Point::PrNoticePasted, Path::new(id.as_str()), move || {
            paint_codex(&drawn.0, &drawn.1, CODEX_RATE_LIMIT_PICKER);
            assert!(screen(&drawn.0, &drawn.1).contains("Approaching rate limits"));
            drawn.0.runtime(&drawn.1).hub.resize(120, 24);
            assert!(!screen(&drawn.0, &drawn.1).contains("Approaching rate limits"));
            paint_codex(
                &drawn.0,
                &drawn.1,
                "\x1b[H\x1b[J\x1b[18;1H  Approaching rate limits",
            );
        });
        let bounds = WatchBounds::default();
        assert!(
            app.watch_prs(&bounds).await.is_empty(),
            "the partial repaint kept Enter out"
        );
        assert!(
            app.watch_prs(&bounds).await.is_empty(),
            "the incomplete repaint still holds the notice"
        );
        paint_codex(&app, &id, "\x1b[H\x1b[J");
        assert!(
            app.watch_prs(&bounds).await.is_empty(),
            "erase alone is not a repaint"
        );
        let composer = CODEX_EMPTY_COMPOSER
            .replace("[34;", "[18;")
            .replace("[40;", "[24;");
        paint_codex(&app, &id, &composer);
        assert_eq!(
            app.watch_prs(&bounds).await.len(),
            1,
            "repaint released the notice"
        );
        assert!(app.watch_prs(&bounds).await.is_empty());
        stop(&app, &id).await
    })
}

#[test]
fn the_codex_rate_limit_picker_submit_hold_survives_a_server_restart() -> TestResult {
    codex_paste_across_restart(false)
}

#[test]
fn the_codex_rate_limit_picker_paste_is_recovered_before_enter_runs() -> TestResult {
    codex_paste_across_restart(true)
}

fn codex_paste_across_restart(interrupt_before_enter: bool) -> TestResult {
    use yulius_app::pr_watch::WatchBounds;
    use yulius_app::race::{Point, on};

    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (id, caller, listener) = first.block_on(async {
        let (app, id, caller, listener) = codex_with_pr_news(&data).await?;
        let drawn = (Arc::clone(&app), id.clone());
        if interrupt_before_enter {
            let (pasted, observed) = tokio::sync::oneshot::channel();
            let (leave, paused) = std::sync::mpsc::channel();
            on(Point::PrNoticePasted, Path::new(id.as_str()), move || {
                paint_codex(&drawn.0, &drawn.1, CODEX_RATE_LIMIT_PICKER);
                let _ = pasted.send(());
                let _ = paused.recv_timeout(Duration::from_secs(10));
            });
            let watching = Arc::clone(&app);
            let watch =
                tokio::spawn(async move { watching.watch_prs(&WatchBounds::default()).await });
            tokio::time::timeout(Duration::from_secs(10), observed).await??;
            watch.abort();
            leave.send(())?;
            assert!(matches!(watch.await, Err(err) if err.is_cancelled()));
        } else {
            on(Point::PrNoticePasted, Path::new(id.as_str()), move || {
                paint_codex(&drawn.0, &drawn.1, CODEX_RATE_LIMIT_PICKER);
            });
            assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        }
        assert!(screen(&app, &id).contains("Approaching rate limits"));
        Ok::<_, Error>((id, caller, listener))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    let occupied = TcpListener::bind(listener.local_addr()?);
    assert!(
        matches!(occupied, Err(err) if err.kind() == std::io::ErrorKind::AddrInUse),
        "the saved PTY's hook endpoint remains reserved throughout the restart"
    );

    runtime()?.block_on(async {
        let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
        let app = start_on_listener(
            &data,
            &fake,
            fake_agent(),
            tokio::net::TcpListener::from_std(listener)?,
            Arc::new(ScriptedUsage::default()),
            Arc::new(ShiftedClock(AtomicI64::new(0))),
        )
        .await?;
        app.reconcile().await?;
        paint_codex(
            &app,
            &id,
            "\x1b[?1049h\x1b[H\x1b[J\x1b[34;1H› Yulius, about your pull request #7\x1b[?25h",
        );
        let reply = app
            .tool_task_message(
                &caller,
                &json!({"task_id": id.as_str(), "text": "Go on", "with_draft": true}),
            )
            .await;
        assert!(
            matches!(&reply, Err(AppError::Conflict(why)) if why.contains("unsent automated paste")),
            "{reply:?}"
        );
        paint_codex(&app, &id, CODEX_EMPTY_COMPOSER);
        assert_eq!(send(&app, &caller, &id).await?["delivered"], "sent");
        stop(&app, &id).await
    })
}

#[test]
fn the_codex_rate_limit_picker_requires_recording_a_paste_before_writing_it() -> TestResult {
    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, id, caller, _) = codex_with_pr_news(&data).await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("attempt")?;
        let marker = Path::new(&attempt.run_dir).join("codex-submit-held");
        std::fs::create_dir(&marker)?;
        let reply = send(&app, &caller, &id).await;
        assert!(
            matches!(&reply, Err(AppError::Conflict(why)) if why.contains("could not record the Codex paste")),
            "{reply:?}"
        );
        assert!(audited(&data, &id)?.is_empty());
        std::fs::remove_dir(&marker)?;
        assert_eq!(send(&app, &caller, &id).await?["delivered"], "sent");
        assert!(!marker.exists(), "a successful Enter clears the recorded paste");
        assert_eq!(send(&app, &caller, &id).await?["delivered"], "sent");
        stop(&app, &id).await
    })
}

/// Replay the pinned Codex composer's empty repaint into the PTY grid. The viewer's
/// draft and erase, the PR watch and the message read by the fake agent are live.
#[test]
fn clearing_a_codex_draft_releases_pr_news_and_its_warning() -> TestResult {
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::RunState;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock) = app_behind(&data, 13 * MINUTE).await?;
        let (sandbox, user) = seed(&app, &data, &[]).await?;
        crate::codex::add_codex_account(&app, "a-codex", r#"{"OPENAI_API_KEY":"synthetic"}"#)
            .await?;
        let id = crate::codex::new_codex_task(&app, &sandbox, &user, "Codex PR news").await?;
        let caller = superagent(&sandbox, &user, "admin");
        wait_for("first Codex answer", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Codex PR news")
                && activity(&app, &id).await.is_some_and(Activity::is_idle)
        })
        .await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let head = open_pull_for_news(&app, &data, &task).await?;
        let bounds = WatchBounds::default();
        assert!(app.watch_prs(&bounds).await.is_empty());
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"draft to erase", false)
            .await?;
        wait_for("draft echoed", Duration::from_secs(30), || async {
            screen(&app, &id).contains("draft to erase")
        })
        .await?;
        let (red, job) = crate::pr_watch::run(&sandbox, "r1", &head, RunState::Failed, None, 1);
        app.deps.store.insert_ci_run(&red, &[job]).await?;
        clock.0.store(-2 * MINUTE, Ordering::SeqCst);
        assert!(app.watch_prs(&bounds).await.is_empty());
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            held_message_flag()
        );

        app.terminal_input(&ctx, &attach, b"\x15", false).await?;
        wait_for("PTY erased draft", Duration::from_secs(30), || async {
            !screen(&app, &id).contains("draft to erase")
        })
        .await?;
        let runtime = app.runtime(&id);
        let repaint = concat!(
            "\x1b[?1049h\x1b[1;1H\x1b[J",
            "\x1b[34;1H\x1b[1m›\x1b[22m \x1b[2mAsk Codex to do anything\x1b[0m",
            "\x1b[36;1H  GPT-6.2-Sol default · project",
            "\x1b[40;1H  \x1b[1m←\x1b[22m for agents · \x1b[1m?\x1b[22m for shortcuts",
            "\x1b[34;3H\x1b[?25h",
        );
        runtime
            .hub
            .output(runtime.hub.output_seq() + 1, repaint.as_bytes());
        clock.0.store(-2 * MINUTE + 30_000, Ordering::SeqCst);
        assert!(app.watch_prs(&bounds).await.is_empty());
        let mark = draft_mark(&app, &id).await?;
        assert!(mark.exists());

        clock.0.store(-MINUTE + 1_000, Ordering::SeqCst);
        let news = app.watch_prs(&bounds).await;
        let [(recipient, notice)] = news.as_slice() else {
            return Err(format!("expected one Codex PR notice, got {news:?}").into());
        };
        assert_eq!(recipient, &id);
        assert!(notice.contains("pull request #7"), "{notice}");
        wait_for("PR news read alone", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Yulius, about your pull request #7")
        })
        .await?;
        assert!(!mark.exists());
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        assert!(app.watch_prs(&bounds).await.is_empty());
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })
}

/// SPEC-572 AC2, with a pull request's news: text left in the prompt box holds the
/// news of a red run, and eleven minutes later the task is flagged for it. The pull-request
/// watch types nothing into a task whose person must act; a flag that only says a message
/// waits does not stop it once the screen shows the box empty. The person clears their text
/// and sends nothing: past the minute the watch types the news and the flag ends. The flag
/// used to stand, with the news untyped, until a prompt was sent.
#[test]
fn a_flag_of_a_held_message_does_not_stop_the_pull_request_watch() -> TestResult {
    use crate::pr_watch::run;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::RunState;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock, id, caller) = idle_full_screen(&data, 13 * MINUTE).await?;
        // The task's pull request #7 is open at its head, and the head's run failed.
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let head = open_pull_for_news(&app, &data, &task).await?;
        let bounds = WatchBounds::default();
        let watched = |app: Arc<App>, id: TaskId| async move {
            let typed = app.watch_prs(&WatchBounds::default()).await;
            typed.into_iter().find(|(task, _)| *task == id)
        };
        assert!(app.watch_prs(&bounds).await.is_empty(), "no news yet");

        let (ctx, attach) = full_screen_terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        wait_for("the draft in the box", Duration::from_secs(30), || async {
            screen(&app, &id).contains("❯ half a thought")
        })
        .await?;
        let (red, job) = run(&task.sandbox_id, "r1", &head, RunState::Failed, None, 1);
        app.deps.store.insert_ci_run(&red, &[job]).await?;

        // Eleven minutes after their last key: the news waits and the task is flagged; a
        // second pass leaves it so.
        clock.0.store(-2 * MINUTE, Ordering::SeqCst);
        for _ in 0..2 {
            assert_eq!(watched(app.clone(), id.clone()).await, None);
            assert_eq!(
                support::attention(&app, id.as_str()).await?,
                held_message_flag()
            );
        }

        // They clear it and send nothing; past the minute the watch types the news.
        app.terminal_input(&ctx, &attach, b"\x15", false).await?;
        wait_for("the cleared box", Duration::from_secs(30), || async {
            fixture_box_empty(&app, &id)
        })
        .await?;
        clock.0.store(-MINUTE + 1_000, Ordering::SeqCst);
        let typed = watched(app.clone(), id.clone())
            .await
            .ok_or("the news was not typed into the flagged task")?;
        assert!(
            typed
                .1
                .starts_with("Yulius, about your pull request #7: Its Yulius CI run r1 at "),
            "{typed:?}"
        );
        wait_for("the news read alone", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Yulius, about your pull request #7")
        })
        .await?;
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        let shown = screen(&app, &id);
        assert!(!shown.contains("ECHO: half a thought"), "{shown}");

        // Another reason a person must act still stops the watch, an empty box or not.
        let (again, job) = run(&task.sandbox_id, "r2", &head, RunState::Failed, None, 2);
        app.deps.store.insert_ci_run(&again, &[job]).await?;
        wait_for("the turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await.is_some_and(Activity::is_idle) && fixture_box_empty(&app, &id)
        })
        .await?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::NeedsAttention), app.now())
            .await?;
        clock.0.store(MINUTE, Ordering::SeqCst);
        assert_eq!(watched(app.clone(), id.clone()).await, None);
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-480 AC7 (DRF-006; review round 1): the wake after an outage is given up ten minutes
/// after the start, so at its last pass a wake that a person's draft holds tells them, however
/// young their keystroke is: here two minutes, at which an earlier pass says nothing. The task
/// used to stay idle with nobody told. Their prompt then continues the session, and no wake
/// follows it. The server is restarted, as after an outage: a run's last turn is known from its
/// transcript only when it was read again.
#[test]
fn the_last_wake_after_an_outage_tells_the_person_of_their_draft() -> TestResult {
    let (_dir, data) = data_dir()?;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let first = runtime()?;
    let (port, id, caller) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, fake_agent(), 0, probe).await?;
        let (id, caller) = running(&app, &data).await?;
        wait_for("the first turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await.is_some_and(Activity::is_idle)
        })
        .await?;
        Ok::<_, Error>((port, id, caller))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let clock = Arc::new(ShiftedClock(AtomicI64::new(-3 * MINUTE)));
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_on(
            &data,
            &fake,
            fake_agent(),
            port,
            probe,
            clock.clone(),
            |_| {},
        )
        .await?;
        app.reconcile().await?;
        // Its transcript was read again: its last turn's end is known.
        wait_for(
            "the transcript read again",
            Duration::from_secs(30),
            || async { !app.claude_queue_holds(&id) },
        )
        .await?;
        // As after an outage in which that turn closed: Yulius never answered its Stop.
        let run_dir = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("run")?
            .run_dir;
        std::fs::write(Path::new(&run_dir).join("stop-answered"), "1")?;
        let down_since = yulius_domain::time::Timestamp(1);
        let started = yulius_domain::time::Timestamp(i64::MAX / 2);
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        clock.0.store(-MINUTE, Ordering::SeqCst);

        // A pass that is not the last waits and says nothing; the last one tells the person.
        let woken = app.wake_after_outage(down_since, started, false).await;
        assert!(woken.is_empty(), "{woken:?}");
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        assert_eq!(notices(&mut watch), Vec::<String>::new());
        let woken = app.wake_after_outage(down_since, started, true).await;
        assert!(woken.is_empty(), "{woken:?}");
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            held_message_flag()
        );
        assert_eq!(notices(&mut watch), [HELD_MESSAGE_NOTICE]);
        // Whatever reaches the agent shows within a second; nothing did.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let shown = screen(&app, &id);
        assert!(!shown.contains("ECHO: half a thought"), "{shown}");
        assert!(!shown.contains("Yulius is back"), "{shown}");

        // Their prompt continues the session and ends the flag.
        app.terminal_input(&ctx, &attach, b"\r", false).await?;
        wait_for("the person's prompt", Duration::from_secs(30), || async {
            read_alone(&app, &id, "half a thought")
                && activity(&app, &id).await.is_some_and(Activity::is_idle)
        })
        .await?;
        assert_eq!(support::attention(&app, id.as_str()).await?, Value::Null);
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
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

/// What the first server leaves running before a restart.
#[derive(Clone, Copy)]
enum BeforeRestart {
    /// A running task whose agent printed the trust dialog's words (as an agent reading Yulius'
    /// own code does).
    TrustWords,
    /// A goal task whose Claude Code still asks whether to trust the folder.
    TrustDialog,
    /// A running task.
    Running,
    /// A running task into whose prompt box its person typed "half a thought" and did not
    /// send it.
    Draft,
}

/// Runs `before` on a first server with `agent` (waiting until its run was spawned more than
/// `spawned_for_ms` ago), restarts the server with `goal_ready_ms`, reattaches the run and waits
/// for its screen to show `shown`.
fn restarted(
    data: &Path,
    agent: &FakeAgentOptions,
    before: BeforeRestart,
    spawned_for_ms: i64,
    goal_ready_ms: u64,
    shown: &str,
) -> Result<(tokio::runtime::Runtime, Arc<App>, TaskId, ProjectCaller), Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let first = runtime()?;
    let (port, id, caller) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(data, &fake, agent.clone(), 0, probe).await?;
        let (id, caller) = match before {
            BeforeRestart::TrustWords | BeforeRestart::Running | BeforeRestart::Draft => {
                running(&app, data).await?
            }
            BeforeRestart::TrustDialog => {
                let (sandbox, user) = seed(&app, data, &["a-only"]).await?;
                let id = new_task(&app, &sandbox, &user, "Make it blue", TaskMode::Goal).await?;
                (id, superagent(&sandbox, &user, "admin"))
            }
        };
        if matches!(before, BeforeRestart::TrustWords) {
            app.send_input(&id, b"Yes, I trust this folder\r").await?;
        }
        if matches!(before, BeforeRestart::Draft) {
            let (ctx, attach) = terminal(&app, &caller, &id).await?;
            app.terminal_input(&ctx, &attach, b"half a thought", false)
                .await?;
            app.terminal_leave(&attach).await;
        }
        wait_for(
            "the screen before the restart",
            Duration::from_secs(60),
            || async { screen(&app, &id).contains(shown) },
        )
        .await?;
        wait_for("the run's age", Duration::from_secs(30), || async {
            match app.deps.store.live_attempt(&id).await {
                Ok(Some(attempt)) => app.now().0 - attempt.started_at.0 > spawned_for_ms,
                _ => false,
            }
        })
        .await?;
        Ok::<_, Error>((port, id, caller))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    let app = rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_with(data, &fake, agent.clone(), port, probe, |config| {
            config.goal_ready_ms = goal_ready_ms;
        })
        .await?;
        app.reconcile().await?;
        wait_for("the reattached screen", Duration::from_secs(30), || async {
            screen(&app, &id).contains(shown)
        })
        .await?;
        Ok::<_, Error>(app)
    })?;
    Ok((rt, app, id, caller))
}

/// A Superagent's message to `id`.
async fn send(app: &Arc<App>, caller: &ProjectCaller, id: &TaskId) -> Result<Value, AppError> {
    app.tool_task_message(caller, &json!({"task_id": id.as_str(), "text": "Go on"}))
        .await
}

/// Round 2 B, round 4 finding 2: a run whose session started before a restart (its SessionStart
/// left a marker in its run folder) takes messages at once on the new server, the trust dialog's
/// words in its output notwithstanding, and long before any hookless window ends.
#[test]
fn a_run_reattached_after_a_restart_takes_messages() -> TestResult {
    let (_dir, data) = data_dir()?;
    let shown = "ECHO: Yes, I trust this folder";
    let before = BeforeRestart::TrustWords;
    let (rt, app, id, caller) = restarted(&data, &fake_agent(), before, 0, 600_000, shown)?;
    rt.block_on(async {
        let answer = send(&app, &caller, &id).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        wait_for("the message", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Message from admin")
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-480 AC6 (DRF-009; review round 1): a draft typed before a restart of the server still
/// keeps a message out after it. The keystroke that began it is kept in the run folder, which
/// the restarted server reads; the server used to know keystrokes in memory alone, and typed
/// behind such a draft. The person's next prompt removes the mark.
#[test]
fn a_draft_typed_before_a_restart_still_keeps_the_message_out() -> TestResult {
    let (_dir, data) = data_dir()?;
    let shown = "half a thought";
    let before = BeforeRestart::Draft;
    let (rt, app, id, caller) = restarted(&data, &fake_agent(), before, 0, 600_000, shown)?;
    rt.block_on(async {
        let Err(refused) = send(&app, &caller, &id).await else {
            wait_for("what the agent read", Duration::from_secs(30), || async {
                screen(&app, &id).contains("ECHO: half a thought")
            })
            .await?;
            return Err(format!(
                "the message was typed onto a draft from before the restart:\n{}",
                screen(&app, &id)
            )
            .into());
        };
        assert!(
            matches!(&refused, AppError::Conflict(why) if why.contains("prompt box")),
            "{refused:?}"
        );
        let run_dir = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("run")?
            .run_dir;
        let mark = Path::new(&run_dir).join("person-typed");
        assert!(mark.exists(), "the draft's mark is kept in the run folder");
        // They send it: the agent reads it alone, and the mark goes with the draft.
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"\r", false).await?;
        wait_for("the person's prompt", Duration::from_secs(30), || async {
            read_alone(&app, &id, "half a thought") && !mark.exists()
        })
        .await?;
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-480 AC10 (DRF-009; review round 2): a mark that outlived its draft (the server did not
/// remember writing it) leaves the run folder with the next prompt that empties the box, here
/// a message Yulius types: a restarted server would otherwise hold messages for a draft that
/// is not there.
#[test]
fn a_stale_mark_leaves_the_run_folder_with_the_next_prompt() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock) = app_behind(&data, 2 * MINUTE).await?;
        let (id, caller) = running(&app, &data).await?;
        let run_dir = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("run")?
            .run_dir;
        let mark = Path::new(&run_dir).join("person-typed");
        // Their prompt writes the mark and removes it again.
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"one", false).await?;
        assert!(mark.exists(), "the first key of a draft marks it");
        app.terminal_input(&ctx, &attach, b"\r", false).await?;
        wait_for("the person's prompt", Duration::from_secs(30), || async {
            read_alone(&app, &id, "one") && !mark.exists()
        })
        .await?;
        app.terminal_leave(&attach).await;
        // A mark the server does not remember, and the minute after their Enter has passed.
        std::fs::write(&mark, app.now().0.to_string())?;
        clock.0.store(0, Ordering::SeqCst);
        let answer = send(&app, &caller, &id).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
        wait_for("the mark's removal", Duration::from_secs(30), || async {
            !mark.exists()
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-480 AC10 (DRF-009; review round 2): a mark whose time is ahead of the clock (a clock
/// set back, a damaged file) is read as typed now by the server that reattaches the run, so
/// it grows old: eleven minutes later its person is told. A time in the future never did.
#[test]
fn a_mark_ahead_of_the_clock_is_read_as_typed_now() -> TestResult {
    use yulius_app::mcp_watch::WatchBounds;
    let (_dir, data) = data_dir()?;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, fake_agent(), 0, probe).await?;
        let (id, caller) = running(&app, &data).await?;
        wait_for("the first turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await.is_some_and(Activity::is_idle)
        })
        .await?;
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"half a thought", false)
            .await?;
        app.terminal_leave(&attach).await;
        let run_dir = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("run")?
            .run_dir;
        let ahead = app.now().0 + 24 * 60 * MINUTE;
        std::fs::write(Path::new(&run_dir).join("person-typed"), ahead.to_string())?;
        Ok::<_, Error>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let clock = Arc::new(ShiftedClock(AtomicI64::new(0)));
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_on(
            &data,
            &fake,
            fake_agent(),
            port,
            probe,
            clock.clone(),
            |_| {},
        )
        .await?;
        app.reconcile().await?;
        // Its transcript was read again: the watch knows that no turn is open.
        wait_for(
            "the transcript read again",
            Duration::from_secs(30),
            || async {
                screen(&app, &id).contains("half a thought") && !app.claude_queue_holds(&id)
            },
        )
        .await?;
        let bounds = WatchBounds {
            silent_after_ms: 1_000,
            reconnect_gap_ms: 60_000,
            ..WatchBounds::new(yulius_domain::time::Timestamp(0))
        };
        // Eleven minutes later (the run was spawned on the system's clock, so this one goes
        // ahead of it).
        clock.0.store(11 * MINUTE, Ordering::SeqCst);
        assert!(app.watch_mcp(&bounds).await.is_empty());
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            held_message_flag()
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A running task whose person submitted "first" while its only subscription had no room, so
/// the prompt is held at its submit, and then typed "more" behind it without sending it.
async fn a_draft_behind_a_held_prompt(
    app: &Arc<App>,
    data: &Path,
    probe: &ScriptedUsage,
) -> Result<(TaskId, ProjectCaller), Error> {
    let (id, caller) = running(app, data).await?;
    wait_for("the first turn's end", Duration::from_secs(30), || async {
        activity(app, &id).await.is_some_and(Activity::is_idle)
    })
    .await?;
    probe.set("a-only", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &caller.user, "a-only").await?;
    let (ctx, attach) = terminal(app, &caller, &id).await?;
    app.terminal_input(&ctx, &attach, b"first", false).await?;
    app.terminal_input(&ctx, &attach, b"\r", false).await?;
    wait_for("the held prompt", Duration::from_secs(60), || async {
        matches!(
            app.deps.store.open_pause(&id).await,
            Ok(Some(pause)) if pause.kind == yulius_app::model::PauseKind::Prompt
        )
    })
    .await?;
    app.terminal_input(&ctx, &attach, b"more", false).await?;
    app.terminal_leave(&attach).await;
    Ok((id, caller))
}

/// The subscription has room again and the held prompt goes on: the agent reads "first", without
/// the "more" typed behind it.
async fn the_held_prompt_goes_on(
    app: &Arc<App>,
    caller: &ProjectCaller,
    id: &TaskId,
    probe: &ScriptedUsage,
) -> TestResult {
    probe.set("a-only", usage(0.0, 5 * HOUR, 30.0, 100));
    refresh(app, &caller.user, "a-only").await?;
    let _ = app.resume_round().await;
    let went_on = wait_for(
        "the held prompt going on",
        Duration::from_secs(20),
        // The terminal's echo of "more" stands before the answer, on its line.
        || async { screen(app, id).contains("moreECHO: first") },
    )
    .await;
    let pause = app
        .deps
        .store
        .open_pause(id)
        .await
        .map(|p| p.map(|p| p.kind));
    let attempts = app.deps.store.attempts(id).await.map(|a| a.len());
    went_on.map_err(|err| {
        format!(
            "{err}: pause {pause:?}, attempts {attempts:?}, screen:\n{}",
            screen(app, id)
        )
        .into()
    })
}

/// The draft "more" still keeps a message out of `id`, and its mark is in the run folder.
async fn the_draft_behind_it_still_holds(
    app: &Arc<App>,
    data: &Path,
    caller: &ProjectCaller,
    id: &TaskId,
) -> TestResult {
    let Err(refused) = send(app, caller, id).await else {
        wait_for("what the agent read", Duration::from_secs(30), || async {
            screen(app, id).contains("ECHO: more")
        })
        .await?;
        return Err(format!(
            "the message was typed onto the draft behind the held prompt:\n{}",
            screen(app, id)
        )
        .into());
    };
    assert!(
        matches!(&refused, AppError::Conflict(why) if why.contains("prompt box")),
        "{refused:?}"
    );
    let run_dir = app.deps.store.live_attempt(id).await?.ok_or("run")?.run_dir;
    assert!(
        Path::new(&run_dir).join("person-typed").exists(),
        "the draft's mark left the run folder with the held prompt"
    );
    // Whatever reaches the agent shows within a second; nothing did.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let shown = screen(app, id);
    assert!(!shown.contains("ECHO: more"), "{shown}");
    assert!(audited(data, id)?.is_empty());
    Ok(())
}

/// SPEC-480 AC9 (DRF-009; review round 2): what a person types behind a prompt that is held at
/// its submit is a draft of its own. When the held prompt goes on, the box is empty as of its
/// submit, not as of now: the draft still holds a message, and its mark stays in the run
/// folder.
#[test]
fn a_draft_behind_a_held_prompt_keeps_its_mark_when_the_prompt_goes_on() -> TestResult {
    let (_dir, data) = data_dir()?;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, fake_agent(), 0, probe.clone()).await?;
        let (id, caller) = a_draft_behind_a_held_prompt(&app, &data, &probe).await?;
        the_held_prompt_goes_on(&app, &caller, &id, &probe).await?;
        the_draft_behind_it_still_holds(&app, &data, &caller, &id).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-480 AC9 (DRF-009; review round 2): the same across a restart of the server while the
/// prompt is held. The restarted server knows the draft from its mark alone and does not know
/// that the prompt its hook client asks about again was submitted before the draft: it used
/// to read that prompt as a new submit, which emptied the box, and typed behind the draft.
/// A draft known from its mark alone stays one until a prompt follows a key this server saw.
#[test]
fn a_draft_behind_a_prompt_held_across_a_restart_still_keeps_the_message_out() -> TestResult {
    let (_dir, data) = data_dir()?;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let first = runtime()?;
    let (port, id, caller) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, fake_agent(), 0, probe.clone()).await?;
        let (id, caller) = a_draft_behind_a_held_prompt(&app, &data, &probe).await?;
        Ok::<_, Error>((port, id, caller))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, fake_agent(), port, probe.clone()).await?;
        app.reconcile().await?;
        wait_for("the reattached screen", Duration::from_secs(30), || async {
            screen(&app, &id).contains("more")
        })
        .await?;
        the_held_prompt_goes_on(&app, &caller, &id, &probe).await?;
        the_draft_behind_it_still_holds(&app, &data, &caller, &id).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Round 4 finding 2: a run whose Claude Code still asks whether to trust the folder across a
/// restart, past the hookless window, refuses the message: nothing answers the dialog.
#[test]
fn a_trust_dialog_open_across_a_restart_refuses_the_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let agent = FakeAgentOptions {
        trust_prompt: true,
        ..hooked()
    };
    let shown = "Yes, I trust this folder";
    let before = BeforeRestart::TrustDialog;
    let (rt, app, id, caller) = restarted(&data, &agent, before, 1_100, 2_000, shown)?;
    rt.block_on(async {
        let refused = send(&app, &caller, &id)
            .await
            .err()
            .ok_or("the message answered the trust dialog")?;
        assert!(
            matches!(&refused, AppError::Conflict(m) if m.contains("folder-trust question")),
            "{refused:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        let shown = screen(&app, &id);
        assert!(!shown.contains("TRUST ANSWERED"), "{shown}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Round 3 finding 1: a run without a SessionStart reattached within the hookless window of its
/// spawn may not read input yet: the message is refused as still starting.
#[test]
fn a_run_reattached_soon_after_its_spawn_is_still_starting() -> TestResult {
    let (_dir, data) = data_dir()?;
    let agent = FakeAgentOptions {
        no_session_start: true,
        ..fake_agent()
    };
    let shown = "ECHO: Budget: answer with an echo";
    let before = BeforeRestart::Running;
    let (rt, app, id, caller) = restarted(&data, &agent, before, 0, 600_000, shown)?;
    rt.block_on(async {
        let refused = send(&app, &caller, &id)
            .await
            .err()
            .ok_or("the message was typed into a session that may not read yet")?;
        assert!(
            matches!(&refused, AppError::Conflict(m) if m.contains("still starting its session")),
            "{refused:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Round 4 nit 3: the hookless window counts from the run's spawn, not from the restart: a run
/// spawned longer ago than the window (5 s) takes the message right after the restart, sooner
/// than the window after it.
#[test]
fn the_hookless_window_counts_from_the_spawn() -> TestResult {
    let (_dir, data) = data_dir()?;
    let agent = FakeAgentOptions {
        no_session_start: true,
        ..fake_agent()
    };
    let shown = "ECHO: Budget: answer with an echo";
    let before = BeforeRestart::Running;
    let (rt, app, id, caller) = restarted(&data, &agent, before, 5_100, 10_000, shown)?;
    rt.block_on(async {
        let answer = send(&app, &caller, &id).await?;
        assert_eq!(answer["delivered"], "sent", "{answer}");
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

/// SPEC-512 ANS-003 (#1418): bob, whom the request names, answers it through his own agent: the
/// message reaches the agent as his labelled answer, the request ends, the task stays admin's.
#[test]
fn a_named_person_answers_through_their_own_agent() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, owner) = running(&app, &data).await?;
        teammate(&app, &owner.sandbox).await?;
        let asked = NeedsYouRecord {
            action: "Decide M1-M6".into(),
            happened: None,
            problem: None,
            decision: true,
            link: None,
            at: 1,
            cause: None,
            named: yulius_app::needs_you::Named {
                who: vec!["bob".into()],
                withdrawn: Vec::new(),
            },
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asked), app.now())
            .await?;
        let bobs_agent = ProjectCaller {
            sandbox: owner.sandbox.clone(),
            user: UserId::parse("u-bob")?,
            task: None,
            channel: Channel::Remote,
            actor: "remote:bob".into(),
            detail: json!({"via": "remote"}),
        };
        let answer = app
            .tool_task_message(
                &bobs_agent,
                &json!({"task_id": id.as_str(), "text": "M1: yes, M2: no"}),
            )
            .await?;
        assert_eq!(answer["delivered"], "answered", "{answer}");
        wait_for("the answer", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ECHO: Answer from bob") }
        })
        .await?;
        assert!(screen(&app, &id).contains("M1: yes, M2: no"));
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(task.owner, owner.user);
        let rows = answers_audited(&data, &id)?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "bob", "{rows:?}");
        stop(&app, &id).await
    })
}

/// SPEC-512 ANS-004 (review of part 2): an answer ends the question it answered only: one the
/// agent asked while the answer was being typed stays open.
#[test]
fn an_answer_leaves_a_question_asked_while_it_was_typed() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, fake_agent()).await?;
        let (id, owner) = running(&app, &data).await?;
        let ctx = teammate(&app, &owner.sandbox).await?;
        app.deps
            .store
            .set_task_needs_you(&id, Some(&region_asked()), app.now())
            .await?;
        let later = NeedsYouRecord {
            action: "Choose the release date".into(),
            at: 2,
            ..region_asked()
        };
        {
            let (app, id, later) = (app.clone(), id.clone(), later.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::AnswerTyped,
                Path::new(id.clone().as_str()),
                move || {
                    let _ = tokio::task::block_in_place(|| {
                        handle.block_on(app.deps.store.set_task_needs_you(
                            &id,
                            Some(&later),
                            app.now(),
                        ))
                    });
                },
            );
        }
        app.answer_needs_you(&ctx, id.as_str(), "Frankfurt", None)
            .await?;
        assert_eq!(app.deps.store.task_needs_you(&id).await?, Some(later));
        stop(&app, &id).await
    })
}

/// SPEC-537 RSD-001, RSD-002. The fixture restores text on its own and will prepend it to
/// the next submit. A refusal leaves it available to its author.
async fn restored_draft(submit: &str, response: &str) -> TestResult {
    let (_dir, data) = data_dir()?;
    let clock = Arc::new(ShiftedClock(AtomicI64::new(-3 * MINUTE)));
    let agent = FakeAgentOptions {
        tui: true,
        ..fake_agent()
    };
    let fake = locate_fake_agent().ok_or("missing fake-agent")?;
    let (app, _) = start_on(
        &data,
        &fake,
        agent,
        0,
        Arc::new(ScriptedUsage::default()),
        clock.clone(),
        |_| {},
    )
    .await?;
    let (id, caller) = running(&app, &data).await?;
    let (ctx, attach) = terminal(&app, &caller, &id).await?;
    app.terminal_input(&ctx, &attach, b"my draft\x13", false)
        .await?;
    app.terminal_input(&ctx, &attach, format!("{submit}\r").as_bytes(), false)
        .await?;
    wait_for(
        "the CLI to restore its stash",
        Duration::from_secs(30),
        || async {
            let shown = screen(&app, &id);
            shown.contains(response) && shown.lines().any(|row| row.trim_end() == "❯ my draft")
        },
    )
    .await?;
    clock.0.store(0, Ordering::SeqCst);
    let message = json!({"task_id": id.as_str(), "text": "Go on"});
    let reply = app.tool_task_message(&caller, &message).await;
    if reply.is_ok() {
        wait_for("the mixed prompt", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: my draftMessage from admin")
        })
        .await?;
        return Err("the agent received the restored draft and the message as one prompt".into());
    }
    assert!(
        matches!(reply, Err(AppError::Conflict(why)) if why.contains("prompt box") && why.contains("send") && !why.contains("my draft"))
    );
    assert!(audited(&data, &id)?.is_empty());
    // The accepted prompt cleared the keystroke hold. The reconnect watch has no sender
    // waiting for a reply, so its refusal must reach the session's person.
    if submit == "other prompt" {
        use yulius_app::mcp_watch::WatchBounds;
        let bounds = WatchBounds {
            silent_after_ms: 1_000,
            ..WatchBounds::new(yulius_domain::time::Timestamp(0))
        };
        assert!(app.watch_mcp(&bounds).await.is_empty());
        let attention = support::attention(&app, id.as_str()).await?;
        assert!(
            attention["problem"]
                .as_str()
                .is_some_and(|why| why.contains("prompt box") && !why.contains("my draft")),
            "{attention}"
        );
    }
    app.terminal_input(&ctx, &attach, b"\r", false).await?;
    wait_for("the draft alone", Duration::from_secs(30), || async {
        read_alone(&app, &id, "my draft")
    })
    .await?;
    clock.0.store(2 * MINUTE, Ordering::SeqCst);
    assert_eq!(
        app.tool_task_message(&caller, &message).await?["delivered"],
        "sent"
    );
    wait_for(
        "the sender's message alone",
        Duration::from_secs(30),
        || async { read_alone(&app, &id, "Message from admin (through their Superagent):") },
    )
    .await?;
    app.terminal_leave(&attach).await;
    stop(&app, &id).await
}

#[test]
fn restored_draft_after_a_persons_prompt() -> TestResult {
    runtime()?.block_on(restored_draft("other prompt", "ECHO: other prompt"))
}

#[test]
fn restored_draft_after_a_local_command() -> TestResult {
    runtime()?.block_on(restored_draft("/color", "Color changed"))
}

/// SPEC-537 RSD-003. Deliver a new viewer key as the older prompt's hook cleans up.
#[test]
fn a_new_key_keeps_its_mark_during_draft_cleanup() -> TestResult {
    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let (app, clock) = app_behind(&data, 2 * MINUTE).await?;
        let (id, caller) = running(&app, &data).await?;
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        let attach = Arc::new(attach);
        let run = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let run_dir = Path::new(&run.run_dir);
        let (sent, accepted) = tokio::sync::oneshot::channel();
        {
            let (app, ctx, attach) = (app.clone(), ctx.clone(), attach.clone());
            yulius_app::race::on(
                yulius_app::race::Point::DraftMarkCleanup,
                run_dir,
                move || {
                    use futures::FutureExt as _;
                    clock.0.fetch_add(1_000, Ordering::SeqCst);
                    // Start the key in the check/remove gap. If cleanup holds its lock,
                    // let the hook finish and the same pending key proceed afterwards.
                    let mut key = Box::pin(async move {
                        app.terminal_input(&ctx, &attach, b"new draft", false).await
                    });
                    if let Some(result) = key.as_mut().now_or_never() {
                        let _ = sent.send(result);
                    } else {
                        tokio::spawn(async move {
                            let _ = sent.send(key.await);
                        });
                    }
                },
            );
        }
        app.terminal_input(&ctx, &attach, b"older prompt\r", false)
            .await?;
        wait_for(
            "the older submit to finish",
            Duration::from_secs(30),
            // The PTY echoes the new key on the same row as the agent’s reply.
            || async { screen(&app, &id).contains("ECHO: older prompt") },
        )
        .await?;
        tokio::time::timeout(Duration::from_secs(5), accepted).await???;
        assert!(
            run_dir.join("person-typed").is_file(),
            "cleanup erased the newer key's recovery mark"
        );
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })
}

/// The submit was received first, but the next key reaches the terminal before cleanup.
#[test]
fn a_key_after_submit_in_the_same_millisecond_keeps_its_draft() -> TestResult {
    use yulius_app::ports::Clock;
    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        let at = yulius::wiring::clock::SystemClock.now().0 - 4 * MINUTE;
        let clock = Arc::new(support::TestClock(AtomicI64::new(at)));
        let fake = locate_fake_agent().ok_or("missing fake-agent")?;
        let (app, _) = start_on(
            &data,
            &fake,
            fake_agent(),
            0,
            Arc::new(ScriptedUsage::default()),
            clock.clone(),
            |_| {},
        )
        .await?;
        let (id, caller) = running(&app, &data).await?;
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
        let attach = Arc::new(attach);
        let run = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let mark = Path::new(&run.run_dir).join("person-typed");
        let (sent, accepted) = tokio::sync::oneshot::channel();
        {
            let (app, ctx, attach) = (app.clone(), ctx.clone(), attach.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::PromptTurnChecked,
                Path::new(id.as_str()),
                move || {
                    let result = tokio::task::block_in_place(|| {
                        handle.block_on(async {
                            tokio::time::timeout(
                                Duration::from_secs(5),
                                app.terminal_input(&ctx, &attach, b"new text", false),
                            )
                            .await
                        })
                    });
                    let _ = sent.send(result);
                },
            );
        }
        clock.0.fetch_add(1_000, Ordering::SeqCst);
        app.terminal_input(&ctx, &attach, b"old text\r", false)
            .await?;
        tokio::time::timeout(Duration::from_secs(30), accepted).await????;
        wait_for(
            "the older prompt to go on",
            Duration::from_secs(30),
            || async { screen(&app, &id).contains("ECHO: old text") },
        )
        .await?;
        assert!(
            mark.is_file(),
            "the older submit erased the same-ms key's mark"
        );
        clock.0.fetch_add(2 * MINUTE, Ordering::SeqCst);
        let result = send(&app, &caller, &id).await;
        assert!(matches!(result, Err(AppError::Conflict(why)) if why.contains("prompt box")));
        app.terminal_input(&ctx, &attach, b"\r", false).await?;
        wait_for(
            "the new draft to be sent and unmarked",
            Duration::from_secs(30),
            || async { read_alone(&app, &id, "new text") && !mark.exists() },
        )
        .await?;
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })
}
