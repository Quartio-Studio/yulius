//! Messages to tasks (SPEC-230): the Superagent's `task_message` types a message into a running
//! task's session (real processes: supervisor → detached `yulius ptyhost` → fake agent), or adds
//! it to the request of a task that has not started, and refuses what cannot take one.

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
    start_with, stop, wait_for,
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
             it, or to clear it and send any prompt, then try again",
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
fn full_screen() -> FakeAgentOptions {
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
        app.runtime(&id).hub.prompt_box_shown_empty()
    })
    .await?;
    Ok((app, clock, id, caller))
}

/// The mark of a possible draft in the folder of `id`'s live run (SPEC-480 DRF-009).
async fn draft_mark(app: &App, id: &TaskId) -> Result<std::path::PathBuf, Error> {
    let run_dir = app
        .deps
        .store
        .live_attempt(id)
        .await?
        .ok_or("run")?
        .run_dir;
    Ok(Path::new(&run_dir).join("person-typed"))
}

/// A key that leaves nothing in the prompt box: Backspace in an empty one.
const KEY_WITHOUT_TEXT: &[u8] = b"\x7f";

/// SPEC-488 AC1 (EBX-001, EBX-003, EBX-004): a person presses a key that leaves nothing in the
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
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
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

/// SPEC-488 AC2 (EBX-001; the reported screen): a task whose person pressed a key that left
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
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
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

/// SPEC-488 AC3 (EBX-006, EBX-004): with the prompt box on screen, text a person typed into it
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
        let (ctx, attach) = terminal(&app, &caller, &id).await?;
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
            app.runtime(&id).hub.prompt_box_shown_empty()
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

/// SPEC-488 AC6 (EBX-005), with a pull request's news: text left in the prompt box holds the
/// news of a red run, and eleven minutes later the task is flagged for it. The pull-request
/// watch types nothing into a task whose person must act; a flag that only says a message
/// waits does not stop it once the screen shows the box empty. The person clears their text
/// and sends nothing: past the minute the watch types the news and the flag ends. The flag
/// used to stand, with the news untyped, until a prompt was sent.
#[test]
fn a_flag_of_a_held_message_does_not_stop_the_pull_request_watch() -> TestResult {
    use crate::pr_watch::{git, run};
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::RunState;
    use yulius_domain::delivery::DeliveryStage;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, clock, id, caller) = idle_full_screen(&data, 13 * MINUTE).await?;
        // The task's pull request #7 is open at its head, and the head's run failed.
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
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
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(7),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        let bounds = WatchBounds::default();
        let watched = |app: Arc<App>, id: TaskId| async move {
            let typed = app.watch_prs(&WatchBounds::default()).await;
            typed.into_iter().find(|(task, _)| *task == id)
        };
        assert!(app.watch_prs(&bounds).await.is_empty(), "no news yet");

        let (ctx, attach) = terminal(&app, &caller, &id).await?;
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
            app.runtime(&id).hub.prompt_box_shown_empty()
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
            activity(&app, &id).await.is_some_and(Activity::is_idle)
                && app.runtime(&id).hub.prompt_box_shown_empty()
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
