//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629): placement
//! past a harness's rebalance threshold, idle moves, pins and "nothing has room", with real
//! processes as in `subscription_moves.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_served_tuned, build_with, new_task, refresh, runtime, seed, stop,
    task, temp, usage, wait_for,
};
use crate::subscription_moves::{live_run, ready, run_accounts};
use crate::support::agents::FakeAgentOptions;
use crate::support::{FakeGitHub, TestResult};

use std::sync::Arc;
use std::time::Duration;

use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_contracts::api::tasks::ContinueTask;
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

fn agent() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    }
}

/// RBL-002, AC-1: a new task goes to the subscription below 95 %, although the one at 96 %,
/// whose week resets in an hour, has the better use-it-or-lose-it score.
#[test]
fn a_new_task_keeps_off_a_subscription_past_the_threshold() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (sandbox, user) = seed(&app, &data, &["a-early", "b-roomier"]).await?;
        probe.set("a-early", usage(10.0, 2 * HOUR, 96.0, 1));
        refresh(&app, &user, "a-early").await?;
        probe.set("b-roomier", usage(10.0, 2 * HOUR, 94.0, 100));
        refresh(&app, &user, "b-roomier").await?;
        let id = new_task(&app, &sandbox, &user, "Placed").await?;
        ready(&app, &id).await?;
        assert_eq!(
            run_accounts(&app, &id).await,
            [Some("b-roomier".to_owned())]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn ctx(app: &App, user: &UserId) -> Result<AuthContext, Box<dyn std::error::Error>> {
    Ok(AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// The task's turn ended: its live run is up and it waits for input, as the Stop hook of a real
/// Claude Code records it (the fake agent here runs no hooks).
async fn idle(app: &App, id: &TaskId) -> TestResult {
    wait_for("the run", Duration::from_secs(30), || async {
        task(app, id)
            .await
            .is_ok_and(|t| t.status == TaskStatus::Running)
            && matches!(app.deps.store.live_attempt(id).await, Ok(Some(_)))
    })
    .await?;
    app.deps
        .store
        .set_activity(id, Some(Activity::Waiting), app.now())
        .await?;
    Ok(())
}

/// A person moves `id` to `account` with "Run on…" (it is pinned there) and waits for it to
/// run there, idle, as its run number `run`.
async fn run_on(
    app: &Arc<App>,
    user: &UserId,
    id: &TaskId,
    account: &str,
    run: usize,
) -> TestResult {
    app.switch_task(
        &ctx(app, user).await?,
        id.as_str(),
        &ContinueTask {
            account_id: Some(account.into()),
            ..Default::default()
        },
    )
    .await?;
    live_run(app, id, run).await?;
    idle(app, id).await
}

/// RBL-005, AC-4: a task a person moved with "Run on…" is pinned there; parked while its
/// subscription reads 96 %, it resumes on its pin, where the pool would place it elsewhere.
#[test]
fn a_pinned_parked_task_resumes_on_its_pin() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_served_tuned(
            &data,
            &fake,
            probe.clone(),
            agent(),
            Arc::new(FakeGitHub::default()),
            "http://127.0.0.1:9",
            |config| config.park_after_ms = 1,
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Pinned").await?;
        ready(&app, &id).await?;
        idle(&app, &id).await?;
        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        run_on(&app, &user, &id, "b-second", 2).await?;
        assert_eq!(
            app.deps
                .store
                .task_pin(&id)
                .await?
                .as_ref()
                .map(|a| a.as_str()),
            Some("b-second")
        );
        probe.set("b-second", usage(96.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        wait_for("the park", Duration::from_secs(30), || async {
            app.park_idle_sessions_now().await;
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Parked)
                && matches!(app.deps.store.live_attempt(&id).await, Ok(None))
        })
        .await?;
        app.continue_task(&ctx(&app, &user).await?, id.as_str(), &Default::default())
            .await?;
        live_run(&app, &id, 3).await?;
        assert_eq!(
            run_accounts(&app, &id).await[2].as_deref(),
            Some("b-second"),
            "resumed on its pin"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
