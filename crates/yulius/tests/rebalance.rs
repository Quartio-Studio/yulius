//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629): placement
//! past a harness's rebalance threshold, idle moves, pins and "nothing has room", with real
//! processes as in `subscription_moves.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_served_tuned, build_with, new_task, refresh, runtime, seed, stop,
    task, temp, usage, wait_for,
};
use crate::subscription_moves::{live_run, notices, ready, run_accounts};
use crate::support::agents::FakeAgentOptions;
use crate::support::{FakeGitHub, TestResult};

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::events::Envelope;
use yulius_contracts::api::tasks::ContinueTask;
use yulius_contracts::events::UiEvent;
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

/// One rebalance event as sent: (the person it went to, from, to, said, key).
type Told = (Option<String>, String, Option<String>, String, String);

/// The rebalance events sent so far.
fn rebalanced(events: &mut broadcast::Receiver<Envelope>) -> Vec<Told> {
    let mut seen = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        if let UiEvent::TaskRebalanced {
            from,
            to,
            said,
            key,
            ..
        } = envelope.event
        {
            let person = envelope.user.map(|u| u.as_str().to_owned());
            seen.push((person, from, to, said, key));
        }
    }
    seen
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

/// A task on `a-first` (placed there while `b-second` read 99 %), idle, with `b-second` at 30 %
/// and `a-first` then reading 96 %.
async fn idle_on_a_full_first(
    app: &Arc<App>,
    probe: &ScriptedUsage,
    data: &std::path::Path,
) -> Result<(TaskId, UserId), Box<dyn std::error::Error>> {
    let (sandbox, user) = seed(app, data, &["a-first", "b-second"]).await?;
    probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    let id = new_task(app, &sandbox, &user, "Idle").await?;
    ready(app, &id).await?;
    assert_eq!(run_accounts(app, &id).await, [Some("a-first".to_owned())]);
    probe.set("b-second", usage(30.0, 5 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    probe.set("a-first", usage(96.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "a-first").await?;
    idle(app, &id).await?;
    Ok((id, user))
}

/// RBL-003, AC-2: an idle task on a subscription at 96 % moves at the next pass to the one at
/// 30 %, in the same session; its info lists the switch with why.
#[test]
fn an_idle_task_leaves_a_subscription_past_the_threshold() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, user) = idle_on_a_full_first(&app, &probe, &data).await?;
        let mut watch = app.runtime(&id).hub.watch();
        let mut events = app.events.subscribe();
        let left = app.deps.store.live_attempt(&id).await?.ok_or("run")?.id;
        let pass = app.rebalance_now().await;
        assert_eq!(pass.moved, 1, "{pass:?}");
        live_run(&app, &id, 2).await?;
        // Told once (RBL-006): its terminal, and its person's notification center.
        // The switch's own lines say that the session goes on; this one says why, once.
        let said: Vec<String> = notices(&mut watch)
            .into_iter()
            .filter(|n| n.contains("a-first"))
            .collect();
        assert_eq!(
            said,
            [
                "↻ Claude a-first is at 96% of its 5-hour window: this idle session continues on \
              Claude b-second."
            ]
        );
        assert_eq!(
            rebalanced(&mut events),
            [(
                Some(user.as_str().to_owned()),
                "Claude a-first".to_owned(),
                Some("Claude b-second".to_owned()),
                "is at 96% of its 5-hour window".to_owned(),
                format!("rebalance:{id}:{left}"),
            )]
        );
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(|a| a.as_str()),
            Some("b-second")
        );
        assert_eq!(
            attempts[0].provider_session_id, attempts[1].provider_session_id,
            "the same session"
        );
        let switches = app.deps.store.switches(&id).await?;
        assert_eq!(
            switches.last().map(|s| s.reason.as_str()),
            Some("rebalance: Claude a-first is at 96% of its 5-hour window")
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// RBL-003, AC-3: a task mid-turn stays through a pass; once its turn ended, the next pass
/// moves it.
#[test]
fn a_task_mid_turn_stays_until_its_turn_ends() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, _user) = idle_on_a_full_first(&app, &probe, &data).await?;
        for mid_turn in [Activity::Working, Activity::NeedsAttention] {
            app.deps
                .store
                .set_activity(&id, Some(mid_turn), app.now())
                .await?;
            let pass = app.rebalance_now().await;
            assert_eq!(pass, Default::default(), "{mid_turn:?}");
            assert_eq!(run_accounts(&app, &id).await.len(), 1, "{mid_turn:?}");
        }
        idle(&app, &id).await?;
        assert_eq!(app.rebalance_now().await.moved, 1);
        live_run(&app, &id, 2).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// RBL-005, AC-4: a task a person moved with "Run on…" stays on that subscription through
/// passes while it reads 96 %; a plain move by the pool ends the pin.
#[test]
fn a_pinned_task_stays_on_its_subscription() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, user) = idle_on_a_full_first(&app, &probe, &data).await?;
        // The person chooses the full one's neighbour, then it fills up too.
        run_on(&app, &user, &id, "b-second", 2).await?;
        probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        probe.set("b-second", usage(96.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        for _ in 0..2 {
            let pass = app.rebalance_now().await;
            assert_eq!((pass.moved, pass.pinned), (0, 1), "{pass:?}");
        }
        assert_eq!(run_accounts(&app, &id).await.len(), 2, "never moved");
        // "Run on…" with no subscription named lets the pool choose: the pin ends, and the
        // next pass may move it.
        app.switch_task(
            &ctx(&app, &user).await?,
            id.as_str(),
            &ContinueTask::default(),
        )
        .await?;
        live_run(&app, &id, 3).await?;
        assert_eq!(app.deps.store.task_pin(&id).await?, None);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// RBL-007, AC-5: with every other subscription at or above 95 %, an idle task on one at 97 %
/// stays, and its terminal and its person are told once; later passes tell nothing more until
/// the episode ends, when one has room again and the task moves.
#[test]
fn nothing_has_room_is_told_once() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, user) = idle_on_a_full_first(&app, &probe, &data).await?;
        probe.set("a-first", usage(10.0, 4 * HOUR, 97.0, 100));
        refresh(&app, &user, "a-first").await?;
        probe.set("b-second", usage(95.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let mut watch = app.runtime(&id).hub.watch();
        let mut events = app.events.subscribe();
        for _ in 0..3 {
            let pass = app.rebalance_now().await;
            assert_eq!((pass.moved, pass.no_room), (0, 1), "{pass:?}");
        }
        assert_eq!(run_accounts(&app, &id).await.len(), 1, "it stays");
        let said: Vec<String> = notices(&mut watch)
            .into_iter()
            .filter(|n| n.starts_with('⏸'))
            .collect();
        assert_eq!(
            said,
            [
                "⏸ Claude a-first is at 97% of its week and no other Claude subscription has room: \
              this task stays on it and moves when one has."
            ]
        );
        let told = rebalanced(&mut events);
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].0.as_deref(), Some(user.as_str()));
        assert_eq!(told[0].2, None);
        assert!(
            told[0].4.starts_with(&format!("rebalance:{id}:a-first:")),
            "{told:?}"
        );
        // Room again: the task moves, and that ends the episode.
        probe.set("b-second", usage(20.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        assert_eq!(app.rebalance_now().await.moved, 1);
        live_run(&app, &id, 2).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// RBL-005: a pin ends once the pool moves the task to another subscription (here as at a usage
/// limit): back on the old one later, it is not pinned there by chance.
#[test]
fn the_pools_move_ends_a_pin() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (id, user) = idle_on_a_full_first(&app, &probe, &data).await?;
        run_on(&app, &user, &id, "b-second", 2).await?;
        assert!(app.deps.store.task_pin(&id).await?.is_some());
        app.switch_with(
            &id,
            Some(yulius_domain::ids::AccountId::parse("a-first")?),
            "LimitReached",
            yulius_app::supervisor::RunStart::Resume { prompt: None },
        )
        .await?;
        live_run(&app, &id, 3).await?;
        assert_eq!(app.deps.store.task_pin(&id).await?, None);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
