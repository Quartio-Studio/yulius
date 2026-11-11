//! A model that runs on usage credits, refused once they are used up (SPEC-464, production
//! 2026-11-11): Claude Code says "You're out of usage credits. /model to switch models." and
//! pauses the goal. That is a usage limit of the subscription for that model: the task moves on
//! by itself, the subscription gets no such task until the allowance resets, and nobody is asked
//! to send "continue". Real processes, as in `pool_supervision.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_with, has_status, new_task, refresh, runtime, screen, seed, stop,
    temp, wait_for,
};
use crate::subscription_moves::{live_run, ready, run_accounts, running_on_first};
use crate::support::TestResult;
use crate::support::agents::{FakeAgentOptions, limit_resume_problems};

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::model::{AccountStatus, PauseKind};
use yulius_contracts::api::tasks::ContinueTask;
use yulius_domain::ids::{AccountId, TaskId};
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

/// Whether a Claude transcript under `data` holds the usage-credits refusal.
fn refusal_written(data: &Path) -> bool {
    let mut dirs = vec![data.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| text.contains("model_requires_usage_credits"))
            {
                return true;
            }
        }
    }
    false
}

/// Yulius is down while Claude Code is refused and pauses the goal. The restart reads both from
/// the transcript: the limit is acted on, and the pause is the limit's there too. Here the move
/// is refused, so the pause the replay held asks a person, once.
#[test]
fn a_usage_credits_refusal_written_while_yulius_was_down_is_read_at_restart() -> TestResult {
    use yulius_app::ports::Crypto;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    // The turn takes long enough for Yulius to be down when the refusal is written.
    let agent = FakeAgentOptions {
        echo_turn_ms: Some(4_000),
        ..refused_for_credits(None)
    };
    let first = runtime()?;
    let id = first.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent.clone())?;
        let (id, _user) = running_on_first(&app, &probe, &data).await?;
        let other = AccountId::parse("b-second")?;
        let foreign = MasterKeyCrypto::from_key(&[9u8; 32])?.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&other, &foreign, AccountStatus::Ok, app.now())
            .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("the typed line", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("hello") }
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(id)
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        wait_for(
            "the refusal in the transcript",
            Duration::from_secs(30),
            || {
                let found = refusal_written(&data);
                async move { found }
            },
        )
        .await?;
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        // Nothing of it was seen before the restart.
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        assert!(app.deps.store.switches(&id).await?.is_empty());
        app.reconcile().await?;
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
        let refused = app.deps.store.switches(&id).await?;
        assert!(
            refused.iter().any(|s| s.outcome == "refused"),
            "{refused:?}"
        );
        assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
