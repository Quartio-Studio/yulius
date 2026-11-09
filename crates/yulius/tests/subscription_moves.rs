//! A person moves a task to another subscription, and a removed subscription is never used again
//! (SPEC-447, F 2026-11-09), with real processes as in `pool_supervision.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, add_account, build_with, has_status, new_task, refresh, runtime, screen,
    seed, stop, temp, usage, wait_for,
};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::hub::HubFrame;
use yulius_contracts::api::tasks::ContinueTask;
use yulius_domain::ids::{AccountId, TaskId, UserId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

async fn ctx(app: &App, user: &UserId) -> Result<AuthContext, Box<dyn std::error::Error>> {
    Ok(AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// The subscription of every run of `id`, oldest first.
async fn run_accounts(app: &App, id: &TaskId) -> Vec<Option<String>> {
    app.deps
        .store
        .attempts(id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.account_id.map(|acc| acc.as_str().to_owned()))
        .collect()
}

/// Waits until the task's run number `n` (from 1) is live.
async fn live_run(app: &App, id: &TaskId, n: usize) -> TestResult {
    wait_for("the next run", Duration::from_secs(60), || async {
        let attempts = app.deps.store.attempts(id).await.unwrap_or_default();
        attempts.len() == n && attempts[n - 1].state == AttemptState::Live
    })
    .await
}

/// The notices said so far on a watch of the task's terminal.
fn notices(watch: &mut broadcast::Receiver<HubFrame>) -> Vec<String> {
    let mut said = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let HubFrame::Notice(text) = frame {
            said.push(text);
        }
    }
    said
}

async fn ready(app: &Arc<App>, id: &TaskId) -> TestResult {
    wait_for("the run", Duration::from_secs(30), || {
        let app = app.clone();
        let id = id.clone();
        async move { screen(&app, &id).contains("FAKE-AGENT READY") }
    })
    .await
}

/// A task on `a-first` (placed there while `b-second` was nearly full), with `b-second` free
/// again.
async fn running_on_first(
    app: &Arc<App>,
    probe: &ScriptedUsage,
    data: &std::path::Path,
) -> Result<(TaskId, UserId), Box<dyn std::error::Error>> {
    let (sandbox, user) = seed(app, data, &["a-first", "b-second"]).await?;
    probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    let id = new_task(app, &sandbox, &user, "Moves").await?;
    ready(app, &id).await?;
    assert_eq!(run_accounts(app, &id).await, [Some("a-first".to_owned())]);
    probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    Ok((id, user))
}

fn agent() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    }
}

/// The owner's sequence (F 2026-11-09): stop the task, remove its subscription, continue the
/// task. It continues on the subscription that is left, never on the removed one.
#[test]
fn a_continued_task_never_runs_on_its_deleted_subscription() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, user) = running_on_first(&app, &probe, &data).await?;
        let ctx = ctx(&app, &user).await?;
        stop(&app, &id).await?;
        app.delete_account(&ctx, "a-first").await?;
        app.continue_task(&ctx, id.as_str(), &ContinueTask { account_id: None })
            .await?;
        live_run(&app, &id, 2).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("b-second")
        );
        let record = app.deps.store.task(&id).await?.ok_or("task")?;
        let summary = app.task_summary(&record).await?;
        assert_eq!(summary.account_label.as_deref(), Some("Claude b-second"));
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Removing a subscription a task works on is not refused: the task moves to the subscription
/// that is left, in the same session, and its terminal names both.
#[test]
fn deleting_a_subscription_in_use_moves_its_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, user) = running_on_first(&app, &probe, &data).await?;
        let ctx = ctx(&app, &user).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.delete_account(&ctx, "a-first").await?;
        assert!(
            app.deps
                .store
                .account(&AccountId::parse("a-first")?)
                .await?
                .is_none()
        );
        live_run(&app, &id, 2).await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(
            attempts[0].provider_session_id, attempts[1].provider_session_id,
            "the same session"
        );
        assert!(has_status(&app, &id, TaskStatus::Running).await);
        let said = notices(&mut watch);
        assert!(
            said.iter().any(|n| n.contains("Claude a-first was removed")
                && n.contains("continues on Claude b-second")),
            "{said:?}"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Removing the only subscription a task works on: its run ends, the task waits for one, is under
/// Needs you and says so.
#[test]
fn deleting_the_last_subscription_makes_its_task_wait_for_one() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Alone").await?;
        ready(&app, &id).await?;
        let ctx = ctx(&app, &user).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.delete_account(&ctx, "a-first").await?;
        wait_for("the wait", Duration::from_secs(60), || async {
            has_status(&app, &id, TaskStatus::WaitingCapacity).await
                && app
                    .deps
                    .store
                    .live_attempt(&id)
                    .await
                    .is_ok_and(|a| a.is_none())
        })
        .await?;
        let record = app.deps.store.task(&id).await?.ok_or("task")?;
        let summary = app.task_summary(&record).await?;
        assert!(summary.waits_for_account, "{summary:?}");
        let needs = app
            .deps
            .store
            .task_needs_you(&id)
            .await?
            .ok_or("Needs you")?;
        assert!(needs.action.contains("Subscriptions"), "{needs:?}");
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("No Claude subscription is left")),
            "{said:?}"
        );
        assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);

        // A new subscription takes the task, and the request ends.
        add_account(&app, "b-second").await?;
        let _ = app.resume_round().await;
        live_run(&app, &id, 2).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("b-second")
        );
        assert!(app.deps.store.task_needs_you(&id).await?.is_none());
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A subscription Claude refuses (#1138) while the only other one is gone: the task waits and
/// needs a person; signing a subscription in again would end that.
#[test]
fn a_refused_last_subscription_leaves_the_task_needing_you() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let options = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            access_lost: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), options)?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Refused").await?;
        ready(&app, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.send_input(&id, b"hello\r").await?;
        wait_for("the wait", Duration::from_secs(60), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        wait_for("the request", Duration::from_secs(30), || async {
            app.deps
                .store
                .task_needs_you(&id)
                .await
                .is_ok_and(|n| n.is_some())
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("Claude refused Claude a-first")
                    && n.contains("No Claude subscription is left")),
            "{said:?}"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A refused subscription with another one working: the move is said with both names.
#[test]
fn a_refused_subscription_says_where_its_task_went() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let options = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            access_lost: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), options)?;
        let (id, _user) = running_on_first(&app, &probe, &data).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;
        wait_for("the notice", Duration::from_secs(30), || {
            let said = notices(&mut watch);
            let found = said.iter().any(|n| {
                n.contains("Claude refused Claude a-first")
                    && n.contains("continues on Claude b-second")
            });
            async move { found }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A person moves a running task to another subscription: the next run is there, in the same
/// session.
#[test]
fn a_person_moves_a_running_task_to_another_subscription() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, user) = running_on_first(&app, &probe, &data).await?;
        let ctx = ctx(&app, &user).await?;
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
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(
            attempts[0].provider_session_id,
            attempts[1].provider_session_id
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A person moves a task that waits for capacity (every subscription at 99 %) onto one of them:
/// it starts there. One at its limit or needing a sign-in is refused, naming it.
#[test]
fn a_person_moves_a_waiting_task_to_the_subscription_they_choose() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("a-first", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        probe.set("b-second", usage(100.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Waits").await?;
        wait_for("the wait", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let ctx = ctx(&app, &user).await?;
        let full = app
            .switch_task(
                &ctx,
                id.as_str(),
                &ContinueTask {
                    account_id: Some("b-second".into()),
                },
            )
            .await;
        match full {
            Err(yulius_app::error::AppError::Capacity(text)) => {
                assert!(text.contains("Claude b-second"), "{text}");
            }
            other => return Err(format!("expected a refusal, got {other:?}").into()),
        }
        app.switch_task(
            &ctx,
            id.as_str(),
            &ContinueTask {
                account_id: Some("a-first".into()),
            },
        )
        .await?;
        live_run(&app, &id, 1).await?;
        assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
