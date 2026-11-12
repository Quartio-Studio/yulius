//! A model that runs on usage credits, refused once they are used up (SPEC-464, production
//! 2026-11-11): Claude Code says "You're out of usage credits. /model to switch models." and
//! pauses the goal. That is a usage limit of the subscription for that model: the task moves on
//! by itself, the subscription gets no such task until the allowance resets, and nobody is asked
//! to send "continue". Real processes, as in `pool_supervision.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_with, has_status, new_task, refresh, runtime, seed, stop, temp,
    usage, wait_for,
};
use crate::subscription_moves::{live_run, notices, ready, run_accounts, running_on_first};
use crate::support::agents::{FakeAgentOptions, limit_resume_problems};
use crate::support::{TestResult, hold_point, point_reached};

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

/// Nobody is asked to do anything about `id`: no request is open, it is not under Needs you, and
/// no goal pause of it was ever said to need a person. That audit entry was the production
/// symptom, also for tasks Yulius had moved already; a request opened at the pause and ended by
/// the move would leave nothing else behind (review round 2).
async fn needs_nobody(app: &App, id: &TaskId) -> TestResult {
    let asked = app.deps.store.task_needs_you(id).await?;
    assert!(
        asked.is_none(),
        "nobody has to send \"continue\": {asked:?}"
    );
    let task = app.deps.store.task(id).await?.ok_or("task")?;
    assert_ne!(task.activity, Some(Activity::NeedsAttention));
    assert_eq!(
        pauses_audited(app, id).await,
        0,
        "no goal pause was said to need a person"
    );
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
        // The limit is the model's though nothing says when it ends: its window is full with no
        // reset, and the subscription's own windows and limit are untouched.
        let (status, stored) = stored_usage(&app, "a-first").await?;
        assert_eq!(stored["weekly_fable"]["used_percent"], 100.0, "{stored}");
        assert!(stored["weekly_fable"]["resets_at"].is_null(), "{stored}");
        assert_eq!(stored["limit_reached"], false, "{stored}");
        assert!(stored["secondary"].is_null(), "{stored}");
        assert_eq!(status, AccountStatus::Limited);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The reset comes from the session's transcript, which is the agent's own file (review round
/// 1): one further ahead than any window lasts is not believed. The task still moves, but the
/// subscription is out for the model only until a provider's reading, not until the year the
/// record names.
#[test]
fn a_reset_no_window_could_have_does_not_outlive_a_provider_reading() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(
            &data,
            &fake,
            probe.clone(),
            refused_for_credits(Some(400 * 24 * 3600)),
        )?;
        let (id, user) = running_on_first(&app, &probe, &data).await?;
        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("b-second")
        );
        let (status, stored) = stored_usage(&app, "a-first").await?;
        assert_eq!(stored["weekly_fable"]["used_percent"], 100.0, "{stored}");
        assert!(
            stored["weekly_fable"]["resets_at"].is_null(),
            "a reset 400 days ahead is not believed: {stored}"
        );
        assert_eq!(status, AccountStatus::Limited);
        probe.set("a-first", usage(0.0, 5 * HOUR, 10.0, 100));
        refresh(&app, &user, "a-first").await?;
        let (status, stored) = stored_usage(&app, "a-first").await?;
        assert!(stored["weekly_fable"].is_null(), "{stored}");
        assert_eq!(status, AccountStatus::Ok);
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
        let (read, go_on) = hold_point(Point::UsageRead, first);
        let (recording, pass) = hold_point(Point::LimitRecord, first);
        pass.send(())?;
        let reading = {
            let (app, user) = (app.clone(), user.clone());
            tokio::spawn(async move {
                refresh(&app, &user, first)
                    .await
                    .map_err(|err| err.to_string())
            })
        };
        point_reached(read).await?;
        app.send_input(&id, b"hello\r").await?;
        point_reached(recording).await?;
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
        // The reset is far enough ahead to be read while it is still ahead on a slow machine
        // (a window whose reset passed is not stored).
        let app = build_with(&data, &fake, probe.clone(), refused_for_credits(Some(8)))?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        app.start_capacity_loop();
        let id = new_task(&app, &sandbox, &user, "Waits").await?;
        ready(&app, &id).await?;
        let started = app.now();
        app.send_input(&id, b"hello\r").await?;
        // The reset Claude named, read while the task waits for it.
        wait_for("the pause", Duration::from_secs(30), || async {
            app.deps
                .store
                .pauses_since(started)
                .await
                .is_ok_and(|pauses| !pauses.is_empty())
        })
        .await?;
        let (_, stored) = stored_usage(&app, "a-first").await?;
        let resets = stored["weekly_fable"]["resets_at"]
            .as_i64()
            .ok_or("the reset")?;
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
        let ended = pauses[0].ended_at.ok_or("the pause ended")?;
        assert!(
            ended.0 >= resets,
            "not before the reset: ended {} ms early",
            resets - ended.0
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
        assert_eq!(
            resolved(&app, &id).await,
            ["Yulius takes the session on past the usage limit that cut its turn"]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// How often `action` was audited for `id`.
async fn audited(app: &App, id: &TaskId, action: &str) -> usize {
    app.deps
        .store
        .audit_details(id.as_str(), action, 10)
        .await
        .map_or(0, |entries| entries.len())
}

/// A task Claude refused twice at a model's limit nothing says the end of, with nothing to go
/// to, as [`refused_twice`] leaves it: Yulius asked a person.
struct RefusedTwice {
    app: Arc<App>,
    probe: Arc<ScriptedUsage>,
    agent: FakeAgentOptions,
    id: TaskId,
    user: UserId,
}

/// SPEC-464 UCL-010, up to Yulius's request: a model's limit nothing says the end of (Claude
/// Code 2.1.311's record, or credits switched off; `reset_s` is the reset its record names, if
/// any) with no other subscription to go to. The task waits, and a provider's reading, which
/// ends such a limit, lets the pool try it once. Refused again the same way, it is not tried a
/// second time: the session stays at the refusal and a person is asked, in the terminal too.
/// One blind retry, never a loop of restarts nobody hears of. With `own`, that request of the
/// agent's is open when the second refusal comes.
async fn refused_twice(
    data: &Path,
    fake: &Path,
    reset_s: Option<u64>,
    own: Option<&yulius_app::needs_you::NeedsYouRecord>,
) -> Result<RefusedTwice, Box<dyn std::error::Error>> {
    let probe = Arc::new(ScriptedUsage::default());
    // Refused from the first message of every run, never saying believably when it ends.
    let agent = FakeAgentOptions {
        limit_after: Some(0),
        ..refused_for_credits(reset_s)
    };
    let app = build_with(data, fake, probe.clone(), agent.clone())?;
    // The second subscription needs a new sign-in: nothing can take the task.
    let (sandbox, user) = seed(&app, data, &["a-first", "b-second"]).await?;
    let sealed = app.deps.crypto.seal(b"b-second")?;
    app.deps
        .store
        .set_account_credential(
            &AccountId::parse("b-second")?,
            &sealed,
            AccountStatus::Relogin,
            app.now(),
        )
        .await?;
    let id = new_task(&app, &sandbox, &user, "No end").await?;
    ready(&app, &id).await?;
    app.send_input(&id, b"hello\r").await?;
    wait_for("the pause", Duration::from_secs(30), || {
        has_status(&app, &id, TaskStatus::WaitingCapacity)
    })
    .await?;
    let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
    assert_eq!(pause.kind, PauseKind::Limit);
    needs_nobody(&app, &id).await?;
    let (status, stored) = stored_usage(&app, "a-first").await?;
    assert_eq!(stored["weekly_fable"]["used_percent"], 100.0, "{stored}");
    assert!(stored["weekly_fable"]["resets_at"].is_null(), "{stored}");
    assert_eq!(status, AccountStatus::Limited);

    // A provider's reading ends the limit; the pool tries the task again, once.
    probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
    refresh(&app, &user, "a-first").await?;
    let (status, stored) = stored_usage(&app, "a-first").await?;
    assert!(stored["weekly_fable"].is_null(), "{stored}");
    assert_eq!(status, AccountStatus::Ok);
    let _ = app.resume_round().await;
    live_run(&app, &id, 2).await?;
    needs_nobody(&app, &id).await?;
    if let Some(own) = own {
        app.deps
            .store
            .set_task_needs_you(&id, Some(own), app.now())
            .await?;
    }

    let mut watch = app.runtime(&id).hub.watch();
    app.send_input(&id, b"again\r").await?;
    wait_for("the request", Duration::from_secs(30), || async {
        audited(&app, &id, "task.limit_without_end").await == 1
    })
    .await?;
    let said = notices(&mut watch);
    assert!(
        said.iter()
            .any(|notice| notice.contains("does not say when the limit ends")),
        "the terminal says so too: {said:?}"
    );
    let task = app.deps.store.task(&id).await?.ok_or("task")?;
    assert_eq!(task.status, TaskStatus::Running, "not paused again");
    assert_eq!(task.activity, Some(Activity::NeedsAttention));
    assert!(app.deps.store.open_pause(&id).await?.is_none());
    assert_eq!(
        pauses_audited(&app, &id).await,
        0,
        "one request, Yulius's own"
    );
    Ok(RefusedTwice {
        app,
        probe,
        agent,
        id,
        user,
    })
}

/// The request Yulius opened for a limit nothing says the end of, while it is the open one.
async fn limit_request(
    app: &App,
    id: &TaskId,
) -> Result<Option<yulius_app::needs_you::NeedsYouRecord>, Box<dyn std::error::Error>> {
    Ok(app.deps.store.task_needs_you(id).await?.filter(|open| {
        open.action
            .starts_with("Run the task on another subscription or model")
    }))
}

/// Why requests of `id` ended without a person or its agent (`task.needs_you.resolved`).
async fn resolved(app: &App, id: &TaskId) -> Vec<String> {
    app.deps
        .store
        .audit_details(id.as_str(), "task.needs_you.resolved", 10)
        .await
        .unwrap_or_default()
}

/// A person's context for `user`.
async fn person(app: &App, user: &UserId) -> Result<AuthContext, Box<dyn std::error::Error>> {
    Ok(AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// SPEC-464 UCL-010: after one blind retry a person is asked, and further readings start
/// nothing. A person's move that is refused changes nothing; one to a subscription that works
/// ends the request. A limit met after that is judged afresh: Yulius's earlier word about the
/// first one does not silence the goal pause written at the next.
#[test]
fn a_limit_nothing_says_the_end_of_is_retried_once_then_asks_a_person() -> TestResult {
    use yulius_app::ports::Crypto;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let RefusedTwice { app, id, user, .. } = refused_twice(&data, &fake, None, None).await?;
        let asked = limit_request(&app, &id).await?.ok_or("request")?;
        assert!(
            asked
                .happened
                .as_deref()
                .is_some_and(|happened| happened.contains("did not say when the limit ends")),
            "{asked:?}"
        );

        // Readings start nothing now: the task waits for its person.
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        assert_eq!(run_accounts(&app, &id).await.len(), 2);
        assert!(has_status(&app, &id, TaskStatus::Running).await);
        assert_eq!(audited(&app, &id, "task.limit_without_end").await, 1);

        // A person's move that is refused before it ends the session (the credential cannot be
        // opened) changes nothing: Yulius's request stays, and the goal pause Claude Code wrote
        // at the refusal is not asked about beside it.
        let ctx = person(&app, &user).await?;
        let other = AccountId::parse("b-second")?;
        let to_second = ContinueTask {
            account_id: Some("b-second".into()),
        };
        // Sealed with another instance's key: it cannot be opened here.
        let stranger = MasterKeyCrypto::from_key(&[9u8; 32])?;
        let foreign = stranger.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&other, &foreign, AccountStatus::Ok, app.now())
            .await?;
        let refused = app.switch_task(&ctx, id.as_str(), &to_second).await;
        assert!(refused.is_err(), "the move was not refused");
        assert_eq!(refused_moves(&app, &id).await, 1);
        assert_eq!(
            pauses_audited(&app, &id).await,
            0,
            "one request, Yulius's own"
        );
        assert_eq!(limit_request(&app, &id).await?.as_ref(), Some(&asked));
        assert_eq!(resolved(&app, &id).await, Vec::<String>::new());

        // The subscription is signed in again and the person runs the task on it: the session
        // goes on past the refusal, so Yulius's request ends with the move, and the log says why.
        let sealed = app.deps.crypto.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&other, &sealed, AccountStatus::Ok, app.now())
            .await?;
        app.switch_task(&ctx, id.as_str(), &to_second).await?;
        live_run(&app, &id, 3).await?;
        assert_eq!(
            run_accounts(&app, &id).await[2].as_deref(),
            Some("b-second")
        );
        needs_nobody(&app, &id).await?;
        assert_eq!(
            resolved(&app, &id).await,
            ["Yulius takes the session on past the usage limit that cut its turn"]
        );

        // Refused there too, and the way back cannot be taken (the first subscription's
        // credential cannot be opened): the session stays at this refusal. It is a new limit:
        // the goal pause written at it asks a person, though Yulius spoke for the last one.
        app.deps
            .store
            .set_account_credential(
                &AccountId::parse("a-first")?,
                &stranger.seal(b"a-first")?,
                AccountStatus::Ok,
                app.now(),
            )
            .await?;
        app.send_input(&id, b"once more\r").await?;
        wait_for(
            "the goal pause's request",
            Duration::from_secs(30),
            || async { pauses_audited(&app, &id).await == 1 },
        )
        .await?;
        assert_eq!(refused_moves(&app, &id).await, 2);
        let pause = app.deps.store.task_needs_you(&id).await?.ok_or("request")?;
        assert_eq!(
            pause.happened.as_deref(),
            Some("Goal paused \u{b7} the request was rate limited."),
            "{pause:?}"
        );
        assert_eq!(audited(&app, &id, "task.limit_without_end").await, 1);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A reset that is not believed (further ahead than any window lasts) is no reset: the limit is
/// one nothing says the end of, bounded the same way, not a wait until the year the record names.
#[test]
fn a_limit_whose_reset_is_not_believed_is_retried_once_then_asks_a_person() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let RefusedTwice { app, id, .. } =
            refused_twice(&data, &fake, Some(400 * 24 * 3600), None).await?;
        assert!(limit_request(&app, &id).await?.is_some());
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A request that is open already is kept (the agent's own is its own to end): Yulius adds
/// none, and still says in the terminal and the log that the task does not go on by itself.
#[test]
fn a_request_open_at_a_limit_nothing_says_the_end_of_is_kept() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let own = yulius_app::needs_you::NeedsYouRecord {
            action: "Add DEV_SSH_KEY on the Secrets page".into(),
            happened: None,
            problem: Some("The deploy needs it.".into()),
            decision: false,
            link: None,
            at: 1,
        };
        let RefusedTwice { app, id, .. } = refused_twice(&data, &fake, None, Some(&own)).await?;
        assert_eq!(
            app.deps.store.task_needs_you(&id).await?.as_ref(),
            Some(&own),
            "the agent's request is its own to end"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The request Yulius opens at a limit nothing says the end of says "the task does not go on by
/// itself", so it ends when that stops being true (ADR-0304, as a goal pause's does): the
/// session's agent works again, with no prompt anyone sees. The task then reads Working, and
/// the log says why the request ended.
#[test]
fn a_request_at_a_limit_nothing_says_the_end_of_ends_when_the_agent_works_again() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let RefusedTwice { app, id, .. } = refused_twice(&data, &fake, None, None).await?;
        assert!(limit_request(&app, &id).await?.is_some());
        let transcript = session_transcript(&data, "msg_fake_credits_out").ok_or("transcript")?;
        append_lines(&transcript, "works", &[Line::Work])?;
        wait_for(
            "the request to end with the agent's work",
            Duration::from_secs(30),
            || async {
                matches!(app.deps.store.task_needs_you(&id).await, Ok(None))
                    && app.deps.store.task(&id).await.is_ok_and(|task| {
                        task.is_some_and(|task| task.activity == Some(Activity::Working))
                    })
            },
        )
        .await?;
        assert_eq!(
            resolved(&app, &id).await,
            ["the agent works again after the usage limit"]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Why a request ended when Yulius paused its task at a usage limit.
const WAITS: &str = "Yulius waits for the usage limit to end and resumes the session by itself";

/// A restart of Yulius forgets that the task was retried (the count is in memory), so the task
/// is taken on once more: it waits for the next reading. Its request said "the task does not go
/// on by itself"; that is no longer so, and the request ends with the wait (review round 3).
#[test]
fn a_request_at_a_limit_nothing_says_the_end_of_ends_when_a_restart_retries_the_task() -> TestResult
{
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let first = runtime()?;
    let (probe, agent, id) = first.block_on(async {
        let refused = refused_twice(&data, &fake, None, None).await?;
        assert!(limit_request(&refused.app, &refused.id).await?.is_some());
        Ok::<_, Box<dyn std::error::Error>>((refused.probe, refused.agent, refused.id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build_with(&data, &fake, probe, agent)?;
        app.reconcile().await?;
        // The restart reads the refusal again and takes the task on from there: it waits.
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        wait_for("the request to end", Duration::from_secs(30), || async {
            matches!(app.deps.store.task_needs_you(&id).await, Ok(None))
        })
        .await?;
        assert_eq!(resolved(&app, &id).await, [WAITS]);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The request across a restart of Yulius (SPEC-469 GPW-004 for this request). While Yulius was
/// down the session got a prompt and has not answered it yet, so the limit is no longer the
/// session's last word and nothing takes the task on. The request is still open, whatever the
/// session did before the limit, and the answer that comes then ends it.
#[test]
fn a_request_at_a_limit_nothing_says_the_end_of_ends_with_work_after_a_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let first = runtime()?;
    let (probe, agent, id, asked) = first.block_on(async {
        let refused = refused_twice(&data, &fake, None, None).await?;
        let asked = limit_request(&refused.app, &refused.id)
            .await?
            .ok_or("request")?;
        Ok::<_, Box<dyn std::error::Error>>((refused.probe, refused.agent, refused.id, asked))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let transcript = session_transcript(&data, "msg_fake_credits_out").ok_or("transcript")?;
    append_lines(&transcript, "down", &[Line::Prompt])?;

    let second = runtime()?;
    second.block_on(async {
        let app = build_with(&data, &fake, probe, agent)?;
        app.reconcile().await?;
        let runtime = app.runtime(&id);
        wait_for("the transcript read", Duration::from_secs(30), || async {
            runtime.transcript_rounds() >= 2
        })
        .await?;
        let rounds = runtime.transcript_rounds();
        wait_for(
            "the transcript applied",
            Duration::from_secs(20),
            || async { runtime.transcript_rounds() >= rounds + 2 },
        )
        .await?;
        assert!(has_status(&app, &id, TaskStatus::Running).await);
        assert_eq!(limit_request(&app, &id).await?.as_ref(), Some(&asked));
        assert_eq!(resolved(&app, &id).await, Vec::<String>::new());

        append_lines(&transcript, "works", &[Line::Work])?;
        wait_for(
            "the request to end with the agent's work",
            Duration::from_secs(30),
            || async {
                matches!(app.deps.store.task_needs_you(&id).await, Ok(None))
                    && has_status(&app, &id, TaskStatus::Running).await
            },
        )
        .await?;
        assert_eq!(
            resolved(&app, &id).await,
            ["the agent works again after the usage limit"]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The first turn of a session whose agent only echoes, with its transcript.
async fn first_turn(
    app: &Arc<App>,
    data: &Path,
) -> Result<(TaskId, UserId, PathBuf), Box<dyn std::error::Error>> {
    let (sandbox, user) = seed(app, data, &["a-first"]).await?;
    let id = new_task(app, &sandbox, &user, "Appended").await?;
    ready(app, &id).await?;
    app.send_input(&id, b"hello\r").await?;
    let mut transcript = None;
    wait_for("the first turn", Duration::from_secs(30), || {
        transcript = session_transcript(data, "msg_fake_1");
        let written = transcript.is_some();
        async move { written }
    })
    .await?;
    Ok((id, user, transcript.ok_or("transcript")?))
}

/// A request that says "the task does not go on by itself" ends when Yulius takes the task on
/// at a usage limit, by pausing it as by moving it (SPEC-464 UCL-007; review round 3): the pool
/// resumes the task past the limit by itself, so nobody has to send it a message. Here Claude
/// Code paused the goal with no limit before it, which asks a person. Then a check-in nobody
/// typed is refused for usage credits, with no goal notice (the goal is paused already, as in
/// task #438's records), and no other subscription can take the task: it waits.
#[test]
fn a_goal_pauses_request_ends_when_yulius_pauses_the_task_at_a_limit() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (id, _user, transcript) = first_turn(&app, &data).await?;
        append_lines(
            &transcript,
            "paused",
            &[Line::Prompt, Line::GoalPause, Line::TurnDuration],
        )?;
        wait_for(
            "the goal pause's request",
            Duration::from_secs(30),
            || async {
                pauses_audited(&app, &id).await == 1
                    && app
                        .deps
                        .store
                        .task_needs_you(&id)
                        .await
                        .is_ok_and(|asked| asked.is_some())
            },
        )
        .await?;
        let asked = app.deps.store.task_needs_you(&id).await?.ok_or("request")?;
        assert!(
            asked.action.starts_with("Send the task a message"),
            "{asked:?}"
        );

        append_lines(
            &transcript,
            "refused",
            &[Line::Prompt, Line::CreditsRefusal, Line::TurnDuration],
        )?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        wait_for("the request to end", Duration::from_secs(30), || async {
            matches!(app.deps.store.task_needs_you(&id).await, Ok(None))
        })
        .await?;
        assert_eq!(resolved(&app, &id).await, [WAITS]);
        assert_eq!(pauses_audited(&app, &id).await, 1, "asked once");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The bound is the model's limit's (SPEC-464 UCL-010): a limit of the subscription that names
/// no reset is read by the next provider's reading, which shows its windows, so the pool goes on
/// as before. Refused twice in a row with nothing to go to, the task waits twice; nobody is asked.
#[test]
fn a_subscriptions_own_limit_without_a_reset_waits_as_before() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (id, user, transcript) = first_turn(&app, &data).await?;
        let started = app.now();
        let refused = [Line::Prompt, Line::SessionLimit, Line::TurnDuration];
        append_lines(&transcript, "first", &refused)?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let (status, stored) = stored_usage(&app, "a-first").await?;
        assert_eq!(stored["limit_reached"], true, "{stored}");
        assert!(stored["limit_resets_at"].is_null(), "{stored}");
        assert!(stored["weekly_fable"].is_null(), "{stored}");
        assert_eq!(status, AccountStatus::Limited);

        probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        live_run(&app, &id, 2).await?;
        append_lines(&transcript, "second", &refused)?;
        wait_for("the second pause", Duration::from_secs(30), || async {
            app.deps
                .store
                .pauses_since(started)
                .await
                .is_ok_and(|pauses| pauses.len() == 2)
                && has_status(&app, &id, TaskStatus::WaitingCapacity).await
        })
        .await?;
        assert_eq!(audited(&app, &id, "task.limit_without_end").await, 0);
        needs_nobody(&app, &id).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A task that already waits for capacity keeps its pause (SPEC-464 UCL-010): refused again
/// while it waits (a check-in nobody typed), it asks nobody, since the pool takes it on from
/// its pause. With nothing known to wait for, the subscription that refused it is a candidate
/// again after its next reading.
#[test]
fn a_task_refused_again_while_it_waits_keeps_its_pause() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (id, user, transcript) = first_turn(&app, &data).await?;
        let started = app.now();
        append_refused_turn(&transcript, "first")?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        needs_nobody(&app, &id).await?;

        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        append_lines(
            &transcript,
            "second",
            &[Line::CreditsRefusal, Line::TurnDuration],
        )?;
        wait_for(
            "the second refusal read",
            Duration::from_secs(20),
            || async { runtime.transcript_rounds() >= rounds + 2 },
        )
        .await?;
        assert!(has_status(&app, &id, TaskStatus::WaitingCapacity).await);

        probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        live_run(&app, &id, 2).await?;
        assert_eq!(run_accounts(&app, &id).await[1].as_deref(), Some("a-first"));
        assert_eq!(audited(&app, &id, "task.limit_without_end").await, 0);
        assert_eq!(app.deps.store.pauses_since(started).await?.len(), 1);
        needs_nobody(&app, &id).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// "With no answer from the agent in between" (UCL-010): a limit nothing says the end of that
/// comes after the model answered again is a new one, with its own retry. The task waits for
/// it like for the first; it does not ask a person at once because it met such a limit before.
#[test]
fn a_limit_after_the_model_answered_again_has_its_own_retry() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        // Each run answers one message, then is refused without a word on when that ends.
        let app = build_with(&data, &fake, probe.clone(), refused_for_credits(None))?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Answers between").await?;
        ready(&app, &id).await?;
        let started = app.now();
        app.send_input(&id, b"hello\r").await?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        live_run(&app, &id, 2).await?;

        // The resumed session answers this message, then is refused again.
        app.send_input(&id, b"again\r").await?;
        wait_for(
            "what follows the second refusal",
            Duration::from_secs(30),
            || async {
                audited(&app, &id, "task.limit_without_end").await > 0
                    || app
                        .deps
                        .store
                        .pauses_since(started)
                        .await
                        .is_ok_and(|pauses| pauses.len() == 2)
            },
        )
        .await?;
        assert_eq!(
            audited(&app, &id, "task.limit_without_end").await,
            0,
            "the model answered in between: this limit is waited for like the first"
        );
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Limit);
        assert!(has_status(&app, &id, TaskStatus::WaitingCapacity).await);
        needs_nobody(&app, &id).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Stores `usage` as what is known of `account`, with `status`.
async fn store_usage(app: &App, account: &str, status: AccountStatus, usage: &Value) -> TestResult {
    app.deps
        .store
        .set_account_state(
            &AccountId::parse(account)?,
            status,
            Some(&usage.to_string()),
            None,
            app.now(),
        )
        .await?;
    Ok(())
}

/// UCL-010 when another subscription says when it resets (review round 2): the task refused
/// again at a limit nothing says the end of waits for that reset. It is not started on the
/// refusing subscription at each of its readings meanwhile, which would restart the session
/// every few minutes for days, and nobody is asked: the pool takes the task on at the reset.
#[test]
fn a_task_refused_again_waits_for_a_known_reset_off_that_subscription() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            limit_after: Some(0),
            ..refused_for_credits(None)
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        // The second subscription is out of the model's allowance for 40 more hours.
        let resets = app.now().plus_millis(40 * 3_600_000);
        let room = json!({"used_percent": 10.0, "window_minutes": 300, "resets_at": null});
        store_usage(
            &app,
            "b-second",
            AccountStatus::Limited,
            &json!({"primary": room, "weekly_fable":
                {"used_percent": 100.0, "window_minutes": 10_080, "resets_at": resets.0}}),
        )
        .await?;
        let id = new_task(&app, &sandbox, &user, "Waits for the other").await?;
        ready(&app, &id).await?;
        assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
        let started = app.now();
        app.send_input(&id, b"hello\r").await?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;

        // A reading ends the first subscription's limit: the one retry.
        probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        live_run(&app, &id, 2).await?;
        assert_eq!(run_accounts(&app, &id).await[1].as_deref(), Some("a-first"));
        app.send_input(&id, b"again\r").await?;
        wait_for("the second pause", Duration::from_secs(30), || async {
            app.deps
                .store
                .pauses_since(started)
                .await
                .is_ok_and(|pauses| pauses.len() == 2)
                && has_status(&app, &id, TaskStatus::WaitingCapacity).await
        })
        .await?;

        // Its next reading shows room again, as each one will: the task stays off it.
        refresh(&app, &user, "a-first").await?;
        let (status, stored) = stored_usage(&app, "a-first").await?;
        assert!(stored["weekly_fable"].is_null(), "{stored}");
        assert_eq!(status, AccountStatus::Ok);
        let _ = app.resume_round().await;
        assert_eq!(
            run_accounts(&app, &id).await.len(),
            2,
            "not started there again at a reading"
        );
        assert!(has_status(&app, &id, TaskStatus::WaitingCapacity).await);
        needs_nobody(&app, &id).await?;
        assert_eq!(audited(&app, &id, "task.limit_without_end").await, 0);

        // The other subscription's allowance resets: the pool takes the task on there.
        store_usage(
            &app,
            "b-second",
            AccountStatus::Ok,
            &json!({"primary": room}),
        )
        .await?;
        let _ = app.resume_round().await;
        live_run(&app, &id, 3).await?;
        assert_eq!(
            run_accounts(&app, &id).await[2].as_deref(),
            Some("b-second")
        );
        needs_nobody(&app, &id).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The transcript of the one session under `data`, once a record with `marker` is in it whole.
fn session_transcript(data: &Path, marker: &str) -> Option<PathBuf> {
    let mut dirs = vec![data.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && path.components().any(|c| c.as_os_str() == "projects")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| text.contains(marker) && text.ends_with('\n'))
            {
                return Some(path);
            }
        }
    }
    None
}

/// A record of a session's transcript as Claude Code writes it, for a test to append.
#[derive(Debug, Clone, Copy)]
enum Line {
    /// A prompt someone typed.
    Prompt,
    /// The refusal of a model for its usage credits, as Claude Code 2.1.311 writes it (task
    /// #438's record of 2026-11-11): no window, no reset.
    CreditsRefusal,
    /// The subscription's own 5-hour limit, in a record that names no reset.
    SessionLimit,
    /// The notice of a goal Claude Code paused at a refusal.
    GoalPause,
    /// The record closing a turn.
    TurnDuration,
    /// A line of the model's own work (a tool call).
    Work,
}

/// Appends `lines` to `transcript`, written now, to the millisecond as Claude Code writes them.
/// `tag` tells one call's records from another's.
fn append_lines(transcript: &Path, tag: &str, lines: &[Line]) -> TestResult {
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
    let refusal = |id: &str, text: &str| {
        json!({"id": id, "type": "message", "role": "assistant", "model": "<synthetic>",
            "stop_reason": "stop_sequence", "content": [{"type": "text", "text": text}]})
    };
    let mut out = String::new();
    for (n, line) in lines.iter().enumerate() {
        let uuid = format!("{tag}-{n}");
        let (kind, extra) = match line {
            Line::Prompt => (
                "user",
                json!({"message": {"role": "user", "content": "go on"}}),
            ),
            Line::CreditsRefusal => (
                "assistant",
                json!({"isApiErrorMessage": true, "error": "rate_limit",
                    "apiError": "model_requires_usage_credits", "apiErrorStatus": 429,
                    "message": refusal(&uuid,
                        "You're out of usage credits. /model to switch models.")}),
            ),
            Line::SessionLimit => (
                "assistant",
                json!({"isApiErrorMessage": true, "error": "rate_limit",
                    "apiError": "usage_limit_reached", "apiErrorStatus": 429,
                    "apiErrorParams": {"rate_limit_info":
                        {"status": "rejected", "rateLimitType": "five_hour"}},
                    "message": refusal(&uuid, "You've hit your session limit")}),
            ),
            Line::GoalPause => (
                "system",
                json!({"subtype": "informational", "isMeta": false, "level": "notice", "content":
                    "Goal paused \u{b7} the request was rate limited \u{b7} send a message to retry"}),
            ),
            Line::TurnDuration => (
                "system",
                json!({"subtype": "turn_duration", "durationMs": 250, "isMeta": false}),
            ),
            Line::Work => (
                "assistant",
                json!({"requestId": format!("req-{uuid}"), "message": {"id": format!("msg-{uuid}"),
                    "type": "message", "role": "assistant", "model": "fake-model",
                    "content": [{"type": "tool_use", "id": format!("toolu-{uuid}"),
                        "name": "Bash", "input": {"command": "true"}}],
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 8, "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0, "output_tokens": 6}}}),
            ),
        };
        let mut record = json!({"type": kind, "uuid": uuid, "parentUuid": parent,
            "isSidechain": false, "sessionId": session, "timestamp": at,
            "userType": "external", "version": "2.1.311"});
        if let (Some(map), Some(more)) = (record.as_object_mut(), extra.as_object()) {
            map.extend(more.clone());
        }
        out.push_str(&format!("{record}\n"));
        parent = uuid;
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(transcript)?
        .write_all(out.as_bytes())?;
    Ok(())
}

/// Appends to `transcript` what Claude Code 2.1.311 writes when a prompt is refused for usage
/// credits in a goal session (task #438's records of 2026-11-11, ids replaced): the prompt, the
/// refusal, the goal's pause notice and the record closing the turn.
fn append_refused_turn(transcript: &Path, tag: &str) -> TestResult {
    append_lines(
        transcript,
        tag,
        &[
            Line::Prompt,
            Line::CreditsRefusal,
            Line::GoalPause,
            Line::TurnDuration,
        ],
    )
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
            transcript = session_transcript(&data, "msg_fake_1");
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
    // the replay's settle of the closed turn had landed after the request. The `last` one ends
    // with the session going on by itself.
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
                // The session goes on with no prompt anyone sees (the limit is over): the
                // request, read again and not asked anew, ends with the agent's work, and so
                // does the flag it was put back under (SPEC-469 for a pause at a limit).
                append_lines(&transcript, "works", &[Line::Work])?;
                wait_for(
                    "the request to end with the agent's work",
                    Duration::from_secs(30),
                    || async {
                        matches!(app.deps.store.task_needs_you(&id).await, Ok(None))
                            && app.deps.store.task(&id).await.is_ok_and(|task| {
                                task.is_some_and(|task| task.activity == Some(Activity::Working))
                            })
                    },
                )
                .await?;
                assert_eq!(
                    resolved(&app, &id).await,
                    ["the agent works again after Claude Code paused the goal"]
                );
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
    restart(4, 2, true, true)?;
    Ok(())
}

/// How many moves of `id` were refused before they ended its session.
async fn refused_moves(app: &App, id: &TaskId) -> usize {
    app.deps.store.switches(id).await.map_or(0, |switches| {
        switches.iter().filter(|s| s.outcome == "refused").count()
    })
}
