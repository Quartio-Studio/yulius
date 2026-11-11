//! A model that runs on usage credits, refused once they are used up (SPEC-464, production
//! 2026-11-11): Claude Code says "You're out of usage credits. /model to switch models." and
//! pauses the goal. That is a usage limit of the subscription for that model: the task moves on
//! by itself, the subscription gets no such task until the allowance resets, and nobody is asked
//! to send "continue". Real processes, as in `pool_supervision.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_with, has_status, new_task, refresh, runtime, seed, stop, temp,
    usage, wait_for,
};
use crate::subscription_moves::{live_run, ready, run_accounts, running_on_first};
use crate::support::TestResult;
use crate::support::agents::{FakeAgentOptions, limit_resume_problems};

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::auth::{AuthContext, rfc3339};
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountStatus, PauseKind, TaskMode, TaskOrigin};
use yulius_app::race::Point;
use yulius_contracts::api::tasks::ContinueTask;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::crypto::MasterKeyCrypto;

/// The agent is refused for usage credits after its first turn; `reset_s` as Claude Code 2.1.322
/// names the allowance's reset, `None` as 2.1.311 leaves it out.
fn refused_for_credits(reset_s: Option<u64>) -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        limit_after: Some(1),
        credits_out: true,
        credits_reset_s: reset_s,
        ..FakeAgentOptions::default()
    }
}

/// The stored usage of `account`, as JSON.
async fn stored_usage(
    app: &App,
    account: &str,
) -> Result<(AccountStatus, serde_json::Value), Box<dyn std::error::Error>> {
    let record = app
        .deps
        .store
        .account(&AccountId::parse(account)?)
        .await?
        .ok_or("account")?;
    let usage = serde_json::from_str(record.usage_json.as_deref().ok_or("usage")?)?;
    Ok((record.status, usage))
}

/// How often a goal pause that needs a person was audited for `id`.
async fn pauses_audited(app: &App, id: &TaskId) -> usize {
    app.deps
        .store
        .audit_details(id.as_str(), "task.goal_retries_spent", 10)
        .await
        .map_or(0, |entries| entries.len())
}

/// Nobody is asked to do anything about `id`: no request is open and it is not under Needs you.
async fn needs_nobody(app: &App, id: &TaskId) -> TestResult {
    let asked = app.deps.store.task_needs_you(id).await?;
    assert!(
        asked.is_none(),
        "nobody has to send \"continue\": {asked:?}"
    );
    let task = app.deps.store.task(id).await?.ok_or("task")?;
    assert_ne!(task.activity, Some(Activity::NeedsAttention));
    Ok(())
}

/// The refusal as Claude Code 2.1.311 writes it (no reset time, #438 on 2026-11-11): the task
/// moves to the subscription that works, in the same session and past the refused turn, and no
/// request to send "continue" is opened for the goal pause that follows the refusal.
#[test]
fn a_task_out_of_usage_credits_moves_on_by_itself() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), refused_for_credits(None))?;
        let (id, _user) = running_on_first(&app, &probe, &data).await?;
        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("b-second")
        );
        assert_eq!(
            attempts[0].provider_session_id, attempts[1].provider_session_id,
            "the same session"
        );
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(has_status(&app, &id, TaskStatus::Running).await);
        needs_nobody(&app, &id).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The refusal as Claude Code 2.1.322 writes it, with the allowance's reset (#156, #443 and #159
/// on 2026-11-11): the limit is the model's, kept with its reset beside the week's own usage; a
/// usage reading, which cannot see it, leaves it in place; and the pool places the next task of
/// that model elsewhere (#159 was moved onto the exhausted subscription once a reading had
/// wiped its limit).
#[test]
fn a_subscription_out_of_usage_credits_gets_no_task_until_they_reset() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(
            &data,
            &fake,
            probe.clone(),
            refused_for_credits(Some(40 * 3600)),
        )?;
        let (id, user) = running_on_first(&app, &probe, &data).await?;
        let refused_at = app.now();
        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("b-second")
        );
        needs_nobody(&app, &id).await?;

        let (status, usage) = stored_usage(&app, "a-first").await?;
        assert_eq!(status, AccountStatus::Limited);
        assert_eq!(usage["weekly_fable"]["used_percent"], 100.0, "{usage}");
        let resets = usage["weekly_fable"]["resets_at"].as_i64().ok_or("reset")?;
        let named = refused_at.plus_millis(40 * HOUR).0;
        assert!(
            (named - 60_000..=named + 60_000).contains(&resets),
            "the reset Claude named: {usage}"
        );
        assert!(
            usage["secondary"].is_null(),
            "the week's own usage is not the model's allowance: {usage}"
        );

        // A usage reading of the subscription: its week is at 61 %, and nothing in it says the
        // model's allowance is used up.
        let week = AccountUsage {
            secondary: Some(UsageWindow {
                used_percent: Some(61.0),
                window_minutes: Some(10_080),
                resets_at: Some(Timestamp(app.now().0 + 100 * HOUR)),
            }),
            ..AccountUsage::default()
        };
        probe.set("a-first", week);
        refresh(&app, &user, "a-first").await?;
        let (status, usage) = stored_usage(&app, "a-first").await?;
        assert_eq!(usage["secondary"]["used_percent"], 61.0, "{usage}");
        assert_eq!(usage["weekly_fable"]["used_percent"], 100.0, "{usage}");
        assert_eq!(usage["weekly_fable"]["resets_at"], resets, "{usage}");
        assert_eq!(status, AccountStatus::Limited);

        // The next task goes where its model can run, though the refused subscription has the
        // most of its week left.
        let sandbox = app.deps.store.task(&id).await?.ok_or("task")?.sandbox_id;
        let next = new_task(&app, &sandbox, &user, "Next").await?;
        ready(&app, &next).await?;
        assert_eq!(
            run_accounts(&app, &next).await,
            [Some("b-second".to_owned())]
        );
        stop(&app, &next).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A task set to `model` (the default one with `None`), started now.
async fn task_of_model(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    model: Option<&str>,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title}: answer with an echo"),
            harness: Harness::ClaudeCode,
            title: Some(title.into()),
            workflow_id: None,
            steps: Vec::new(),
            model: model.map(str::to_owned),
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode: TaskMode::Prompt,
            supertask: false,
        })
        .await?;
    Ok(record.id)
}

/// The allowance is a bucket per model, and Claude names no family with it: the limit is the
/// limit of the model the refused task is launched with, here Opus, never Fable by name. The
/// subscription stays usable for the default model (it reads Ready and gets the next such
/// task) and gets no further task of the refused model until the reset.
#[test]
fn a_usage_credits_limit_binds_the_model_of_the_task_that_hit_it() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(
            &data,
            &fake,
            probe.clone(),
            refused_for_credits(Some(40 * 3600)),
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        // Placed on a-first while b-second is nearly full; b-second is free again afterwards.
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let opus = Some("claude-opus-5-5");
        let id = task_of_model(&app, &sandbox, &user, "Opus", opus).await?;
        ready(&app, &id).await?;
        assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;

        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("b-second")
        );
        needs_nobody(&app, &id).await?;
        let (status, stored) = stored_usage(&app, "a-first").await?;
        assert_eq!(stored["weekly_opus"]["used_percent"], 100.0, "{stored}");
        assert!(stored["weekly_fable"].is_null(), "{stored}");
        assert_eq!(status, AccountStatus::Ok, "the default model still runs");

        // a-first has the most room of the week, and a reading of it keeps the model's limit.
        probe.set("a-first", usage(0.0, 5 * HOUR, 10.0, 50));
        refresh(&app, &user, "a-first").await?;
        let (status, stored) = stored_usage(&app, "a-first").await?;
        assert_eq!(stored["weekly_opus"]["used_percent"], 100.0, "{stored}");
        assert_eq!(stored["secondary"]["used_percent"], 10.0, "{stored}");
        assert_eq!(status, AccountStatus::Ok);
        let fable = task_of_model(&app, &sandbox, &user, "Default", None).await?;
        ready(&app, &fable).await?;
        assert_eq!(
            run_accounts(&app, &fable).await,
            [Some("a-first".to_owned())],
            "a task of the default model goes where the week has the most room"
        );
        let other = task_of_model(&app, &sandbox, &user, "Opus again", opus).await?;
        ready(&app, &other).await?;
        assert_eq!(
            run_accounts(&app, &other).await,
            [Some("b-second".to_owned())],
            "a task of the refused model does not"
        );
        for task in [&other, &fable, &id] {
            stop(&app, task).await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Holds the server at `point` for the subscription `account` until the returned sender is used
/// (or 60 s pass); the receiver says when it got there.
fn hold_at(
    point: Point,
    account: &str,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (reached, at) = std::sync::mpsc::channel();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    yulius_app::race::on(point, Path::new(account), move || {
        let _ = reached.send(());
        let _ = wait.recv_timeout(Duration::from_secs(60));
    });
    (at, go)
}

/// Waits until the server reached the point `at` stands for.
async fn reached(at: std::sync::mpsc::Receiver<()>) -> TestResult {
    tokio::task::spawn_blocking(move || at.recv_timeout(Duration::from_secs(60)))
        .await?
        .map_err(|_| "the server never reached the point")?;
    Ok(())
}

/// A limit Claude reports while a usage reading of the same subscription is under way is not
/// written over by that reading (a reading wiped #159's limit on 2026-11-11). The reading is
/// held once it has read what is stored, which knows no limit yet; the refusal arrives; the
/// reading goes on. A limit written meanwhile would be lost to the reading's write, so the limit
/// waits for the reading instead and is added to what the reading stored.
///
/// Half a second is the limit's chance to be written while the reading is held: unguarded it is
/// written at once. Guarded it waits, so that wait always runs out.
#[test]
fn a_limit_reported_during_a_usage_reading_is_kept() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(
            &data,
            &fake,
            probe.clone(),
            refused_for_credits(Some(40 * 3600)),
        )?;
        // Subscriptions of this test's own, so its holds meet no other test's reading.
        let (first, second) = ("held-reading-first", "held-reading-second");
        let (sandbox, user) = seed(&app, &data, &[first, second]).await?;
        probe.set(second, usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, second).await?;
        let id = new_task(&app, &sandbox, &user, "Read while limited").await?;
        ready(&app, &id).await?;
        assert_eq!(run_accounts(&app, &id).await, [Some(first.to_owned())]);
        probe.set(second, usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, second).await?;

        probe.set(first, usage(10.0, 4 * HOUR, 61.0, 100));
        let (read, go_on) = hold_at(Point::UsageRead, first);
        let (recording, pass) = hold_at(Point::LimitRecord, first);
        pass.send(())?;
        let reading = {
            let (app, user) = (app.clone(), user.clone());
            tokio::spawn(async move {
                refresh(&app, &user, first)
                    .await
                    .map_err(|err| err.to_string())
            })
        };
        reached(read).await?;
        app.send_input(&id, b"hello\r").await?;
        reached(recording).await?;
        let written = wait_for("the limit's write", Duration::from_millis(500), || async {
            stored_usage(&app, first)
                .await
                .is_ok_and(|(_, usage)| !usage["weekly_fable"].is_null())
        })
        .await
        .is_ok();
        go_on.send(())?;
        reading.await??;
        live_run(&app, &id, 2).await?;
        assert_eq!(run_accounts(&app, &id).await[1].as_deref(), Some(second));
        let (status, usage) = stored_usage(&app, first).await?;
        assert_eq!(
            usage["weekly_fable"]["used_percent"], 100.0,
            "the reading wrote over the limit (written while it was held: {written}): {usage}"
        );
        assert_eq!(usage["secondary"]["used_percent"], 61.0, "{usage}");
        assert_eq!(status, AccountStatus::Limited);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// No other subscription can take the task: it waits at its limit and goes on at the reset
/// Claude named, by itself, past the refused turn: no usage reading and no message (the person's
/// requirement of 2026-11-11: "tasks paused for capacity resume at the reset by themselves").
#[test]
fn a_task_paused_for_usage_credits_resumes_at_their_reset_by_itself() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), refused_for_credits(Some(4)))?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        app.start_capacity_loop();
        let id = new_task(&app, &sandbox, &user, "Waits").await?;
        ready(&app, &id).await?;
        let started = app.now();
        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("a-first"),
            "the same subscription, once its allowance reset"
        );
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        // It waited at its limit, and only until the reset.
        let pauses = app.deps.store.pauses_since(started).await?;
        assert_eq!(pauses.len(), 1, "{pauses:?}");
        assert_eq!(pauses[0].kind, PauseKind::Limit);
        let waited = pauses[0]
            .ended_at
            .ok_or("the pause ended")?
            .millis_since(pauses[0].started_at);
        assert!(
            waited < 60_000,
            "resumed at the reset, not at a recheck: {waited} ms"
        );
        needs_nobody(&app, &id).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The move past the limit is refused before it ends the session (the other subscription's
/// credential cannot be opened), so the session stays at Claude Code's refusal, where nothing
/// goes on by itself: the goal pause Claude Code wrote there needs a person after all, as a pause
/// with no limit before it does (ADR-0225). Only a task Yulius took on asks nobody.
#[test]
fn a_refused_move_at_a_usage_credits_limit_still_asks_a_person() -> TestResult {
    use yulius_app::ports::Crypto;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), refused_for_credits(None))?;
        let (id, user) = running_on_first(&app, &probe, &data).await?;
        let other = AccountId::parse("b-second")?;
        let foreign = MasterKeyCrypto::from_key(&[9u8; 32])?.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&other, &foreign, AccountStatus::Ok, app.now())
            .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("the refused move", Duration::from_secs(30), || async {
            app.deps
                .store
                .switches(&id)
                .await
                .is_ok_and(|s| s.iter().any(|s| s.outcome == "refused"))
        })
        .await?;
        wait_for("the request", Duration::from_secs(30), || async {
            app.deps
                .store
                .task_needs_you(&id)
                .await
                .is_ok_and(|asked| asked.is_some())
        })
        .await?;
        let asked = app.deps.store.task_needs_you(&id).await?.ok_or("request")?;
        assert_eq!(
            asked.happened.as_deref(),
            Some("Goal paused \u{b7} the request was rate limited."),
            "{asked:?}"
        );
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.activity, Some(Activity::NeedsAttention));
        assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
        assert!(app.deps.store.open_pause(&id).await?.is_none(), "no pause");
        assert_eq!(pauses_audited(&app, &id).await, 1);

        // A message that is no person's answer (Yulius types its pull request notices into
        // the session) is refused the same way. The request stays as it is, and the task is
        // under Needs you again: it does not read Working at a refusal nobody takes on.
        app.send_input(&id, b"again\r").await?;
        wait_for("the second refusal", Duration::from_secs(30), || async {
            pauses_audited(&app, &id).await == 2
        })
        .await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.activity, Some(Activity::NeedsAttention));
        assert_eq!(
            app.deps.store.task_needs_you(&id).await?.as_ref(),
            Some(&asked),
            "the request is not asked twice"
        );

        // The subscription works again and a person runs the task on it: the session goes on
        // past the refusal by itself, so the request to send it a message ends with the move.
        let sealed = app.deps.crypto.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&other, &sealed, AccountStatus::Ok, app.now())
            .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &ContinueTask {
                account_id: Some("b-second".into()),
            },
        )
        .await?;
        live_run(&app, &id, 2).await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(
            app.deps.store.task_needs_you(&id).await?.is_none(),
            "the request ended with the move"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The transcript of the one session under `data`, once its first turn is in it whole.
fn session_transcript(data: &Path) -> Option<PathBuf> {
    let mut dirs = vec![data.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && path.components().any(|c| c.as_os_str() == "projects")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| text.contains("msg_fake_1") && text.ends_with('\n'))
            {
                return Some(path);
            }
        }
    }
    None
}

/// Appends to `transcript` what Claude Code 2.1.311 writes when a prompt is refused for usage
/// credits in a goal session (task #438's records of 2026-11-11, ids replaced): the prompt, the
/// refusal, the goal's pause notice and the record closing the turn, written now, to the
/// millisecond as Claude Code writes them. `tag` tells one such turn's records from another's.
fn append_refused_turn(transcript: &Path, tag: &str) -> TestResult {
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let at = format!(
        "{}.{:03}Z",
        rfc3339(Timestamp(now)).trim_end_matches('Z'),
        now.rem_euclid(1000)
    );
    let text = std::fs::read_to_string(transcript)?;
    let last: Value = serde_json::from_str(text.lines().last().ok_or("an empty transcript")?)?;
    let session = last["sessionId"].as_str().ok_or("a session id")?.to_owned();
    let mut parent = last["uuid"].as_str().ok_or("a uuid")?.to_owned();
    let mut lines = String::new();
    let mut record = |kind: &str, name: &str, extra: Value| {
        let uuid = format!("{tag}-{name}");
        let mut record = json!({"type": kind, "uuid": uuid, "parentUuid": parent,
            "isSidechain": false, "sessionId": session, "timestamp": at,
            "userType": "external", "version": "2.1.311"});
        if let (Some(map), Some(more)) = (record.as_object_mut(), extra.as_object()) {
            map.extend(more.clone());
        }
        lines.push_str(&format!("{record}\n"));
        parent = uuid;
    };
    record(
        "user",
        "prompt",
        json!({"message": {"role": "user", "content": "go on"}}),
    );
    record(
        "assistant",
        "refusal",
        json!({"isApiErrorMessage": true, "error": "rate_limit",
            "apiError": "model_requires_usage_credits", "apiErrorStatus": 429,
            "message": {"id": format!("{tag}-refusal"), "type": "message", "role": "assistant",
                "model": "<synthetic>", "stop_reason": "stop_sequence",
                "content": [{"type": "text",
                    "text": "You're out of usage credits. /model to switch models."}]}}),
    );
    record(
        "system",
        "notice",
        json!({"subtype": "informational", "isMeta": false, "level": "notice", "content":
            "Goal paused \u{b7} the request was rate limited \u{b7} send a message to retry"}),
    );
    record(
        "system",
        "duration",
        json!({"subtype": "turn_duration", "durationMs": 250, "isMeta": false}),
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(transcript)?
        .write_all(lines.as_bytes())?;
    Ok(())
}

/// Yulius is down while Claude Code is refused and pauses the goal: what Claude Code wrote is
/// appended to the session's transcript while no server runs, so none saw it live. The restart
/// reads both from the transcript: the limit is acted on, and the pause is the limit's there
/// too. Here the move is refused, so the pause the replay held asks a person.
///
/// It asks once per limit. Another restart reads the same refusal and pause again and tries
/// the move again, but that limit was asked about: nothing new is asked or audited, whatever
/// the task read meanwhile (the replay also settles the closed turn as Your turn, and that may
/// land after the request), and the task is under Needs you again. A second refusal written
/// while Yulius was down is a new limit: audited once, its request the one still open, and
/// once only however often Yulius restarts after it.
#[test]
fn a_usage_credits_refusal_written_while_yulius_was_down_is_read_at_restart() -> TestResult {
    use yulius_app::ports::Crypto;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let agent = FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (id, transcript) = first.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent.clone())?;
        let (id, _user) = running_on_first(&app, &probe, &data).await?;
        let other = AccountId::parse("b-second")?;
        let foreign = MasterKeyCrypto::from_key(&[9u8; 32])?.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&other, &foreign, AccountStatus::Ok, app.now())
            .await?;
        app.send_input(&id, b"hello\r").await?;
        let mut transcript = None;
        wait_for("the first turn", Duration::from_secs(30), || {
            transcript = session_transcript(&data);
            let written = transcript.is_some();
            async move { written }
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>((id, transcript.ok_or("transcript")?))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    append_refused_turn(&transcript, "down")?;

    let second = runtime()?;
    let asked = second.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent.clone())?;
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        assert_eq!(refused_moves(&app, &id).await, 0);
        app.reconcile().await?;
        // The pause and the refused move reach each other in either order.
        wait_for("the request", Duration::from_secs(30), || async {
            refused_moves(&app, &id).await == 1
                && pauses_audited(&app, &id).await == 1
                && app
                    .deps
                    .store
                    .task_needs_you(&id)
                    .await
                    .is_ok_and(|asked| asked.is_some())
        })
        .await?;
        let asked = app.deps.store.task_needs_you(&id).await?.ok_or("request")?;
        assert_eq!(
            asked.happened.as_deref(),
            Some("Goal paused \u{b7} the request was rate limited."),
            "{asked:?}"
        );
        assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
        Ok::<_, Box<dyn std::error::Error>>(asked)
    })?;
    second.shutdown_timeout(Duration::from_secs(5));

    // Each restart replays the pending limit and is refused the move again. `settled` is as if
    // the replay's settle of the closed turn had landed after the request.
    let restart = |moves: usize, audits: usize, settled: bool, last: bool| -> TestResult {
        let rt = runtime()?;
        rt.block_on(async {
            let app = build_with(&data, &fake, probe.clone(), agent.clone())?;
            if settled {
                app.deps
                    .store
                    .set_activity(&id, Some(Activity::Waiting), app.now())
                    .await?;
            }
            app.reconcile().await?;
            wait_for(
                "the move refused again",
                Duration::from_secs(30),
                || async {
                    refused_moves(&app, &id).await == moves
                        && pauses_audited(&app, &id).await >= audits
                        && app.deps.store.task(&id).await.is_ok_and(|task| {
                            task.is_some_and(|task| task.activity == Some(Activity::NeedsAttention))
                        })
                },
            )
            .await?;
            assert_eq!(
                pauses_audited(&app, &id).await,
                audits,
                "asked once per limit"
            );
            assert_eq!(
                app.deps.store.task_needs_you(&id).await?.as_ref(),
                Some(&asked),
                "the same request"
            );
            if last {
                stop(&app, &id).await?;
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    };
    // The same limit again: nothing new.
    restart(2, 1, true, false)?;
    // Refused once more while Yulius was down: a new limit, asked about once.
    append_refused_turn(&transcript, "down-again")?;
    restart(3, 2, false, false)?;
    restart(4, 2, false, true)?;
    Ok(())
}

/// How many moves of `id` were refused before they ended its session.
async fn refused_moves(app: &App, id: &TaskId) -> usize {
    app.deps.store.switches(id).await.map_or(0, |switches| {
        switches.iter().filter(|s| s.outcome == "refused").count()
    })
}
