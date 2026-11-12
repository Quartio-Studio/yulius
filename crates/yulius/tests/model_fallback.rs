//! A task of a Fable model runs Opus 5.5 while no subscription can run Fable (SPEC-482, "add
//! fallback for opus 5.5 if fable is done everywhere"): at the limit that cut its turn, on the
//! same subscription when that is the one with room, and at its start. Each run records the
//! model it was launched with, a limit binds that model, and Fable is used again at the next
//! launch once a subscription can run it. Real processes, as in `usage_credits.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_with, has_status, new_task, reading, runtime, seed, stop, temp,
    usage, wait_for,
};
use crate::subscription_moves::{live_run, notices, ready, run_accounts};
use crate::support::TestResult;
use crate::support::agents::{FakeAgentOptions, limit_resume_problems};

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountStatus, PauseKind, TaskMode, TaskOrigin};
use yulius_contracts::api::tasks::ContinueTask;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

const FABLE: &str = "claude-fable-5-2";
const OPUS: &str = "claude-opus-5-5";
const FELL_BACK: &str =
    "Fable 5.2 is used up on every subscription: this session runs on Opus 5.5.";
const BACK: &str = "Fable 5.2 can run again: this session runs on it.";

/// The agent is refused for usage credits after the first turn of each run; `reset_s` as Claude
/// Code 2.1.322 names the allowance's reset, `None` as 2.1.311 leaves it out.
fn refused_for_credits(reset_s: Option<u64>) -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        limit_after: Some(1),
        credits_out: true,
        credits_reset_s: reset_s,
        ..FakeAgentOptions::default()
    }
}

/// The model each run of `id` was launched with, oldest first: what Yulius asked the harness for
/// (`None` = the task's own setting was passed on as it is) and what the run's record says.
async fn launched(
    app: &App,
    id: &TaskId,
) -> Result<Vec<(String, Option<String>)>, Box<dyn std::error::Error>> {
    let mut models = Vec::new();
    for attempt in app.deps.store.attempts(id).await? {
        let launch = std::fs::read_to_string(Path::new(&attempt.run_dir).join("fake-launch.txt"))?;
        let asked = launch
            .lines()
            .find_map(|line| line.strip_prefix("model="))
            .ok_or("the launch names its model")?
            .to_owned();
        models.push((asked, attempt.model));
    }
    Ok(models)
}

fn asked_for(model: &str) -> String {
    format!("Some({model:?})")
}

/// Types `line` into run `n` of the task, once that run is live and takes input: a run is
/// recorded live a moment before its terminal is attached, and input is refused until then.
async fn type_into_run(app: &App, id: &TaskId, n: usize, line: &[u8]) -> TestResult {
    live_run(app, id, n).await?;
    wait_for("the run takes input", Duration::from_secs(30), || async {
        app.send_input(id, line).await.is_ok()
    })
    .await
}

/// The stored usage of `account`, as JSON.
async fn stored_usage(app: &App, account: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let record = app
        .deps
        .store
        .account(&AccountId::parse(account)?)
        .await?
        .ok_or("account")?;
    Ok(serde_json::from_str(
        record.usage_json.as_deref().ok_or("usage")?,
    )?)
}

/// Stores for `account` a 5-hour window with room and, with `fable_out`, a Fable allowance used
/// up for 40 more hours.
async fn store_usage(app: &App, account: &str, fable_out: bool) -> TestResult {
    let mut usage =
        json!({"primary": {"used_percent": 10.0, "window_minutes": 300, "resets_at": null}});
    if fable_out {
        usage["weekly_fable"] = json!({"used_percent": 100.0, "window_minutes": 10_080,
            "resets_at": app.now().plus_millis(40 * 3_600_000).0});
    }
    let status = if fable_out {
        AccountStatus::Limited
    } else {
        AccountStatus::Ok
    };
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

/// Nobody is asked to do anything about `id` (SPEC-482 FBK-008).
async fn needs_nobody(app: &App, id: &TaskId) -> TestResult {
    let asked = app.deps.store.task_needs_you(id).await?;
    assert!(asked.is_none(), "nobody is asked: {asked:?}");
    let task = app.deps.store.task(id).await?.ok_or("task")?;
    assert_ne!(task.activity, Some(Activity::NeedsAttention));
    for action in ["task.goal_retries_spent", "task.limit_without_end"] {
        let entries = app
            .deps
            .store
            .audit_details(id.as_str(), action, 10)
            .await?;
        assert!(entries.is_empty(), "{action}: {entries:?}");
    }
    Ok(())
}

/// AC-1: one subscription, a default task, and Claude Code refuses Fable for its usage credits
/// without saying when they reset (2.1.311). Before SPEC-482 the task asked a person
/// (`task.limit_without_end`); now the same session goes on, on the same subscription, launched
/// with Opus 5.5 past the refused turn, and says so. Its own setting stays "default".
#[test]
fn a_task_out_of_fable_everywhere_goes_on_with_opus_on_its_subscription() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), refused_for_credits(None))?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Falls back").await?;
        ready(&app, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;

        assert_eq!(
            run_accounts(&app, &id).await,
            [Some("a-only".to_owned()), Some("a-only".to_owned())]
        );
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[0].provider_session_id, attempts[1].provider_session_id,
            "the same session"
        );
        assert_eq!(
            launched(&app, &id).await?,
            [
                ("None".to_owned(), Some(FABLE.to_owned())),
                (asked_for(OPUS), Some(OPUS.to_owned())),
            ]
        );
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(has_status(&app, &id, TaskStatus::Running).await);
        needs_nobody(&app, &id).await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(task.model, None, "the task's own setting is kept");

        let mut said = Vec::new();
        wait_for("the fallback's line", Duration::from_secs(30), || {
            said.extend(notices(&mut watch));
            let found = said.iter().any(|notice| notice == FELL_BACK);
            async move { found }
        })
        .await?;
        let switches = app.deps.store.switches(&id).await?;
        assert_eq!(switches.len(), 1, "{switches:?}");
        assert!(
            switches[0].to_desc.ends_with(", Opus 5.5"),
            "the move names the model: {:?}",
            switches[0].to_desc
        );
        // The limit is Fable's alone: the subscription's own windows are untouched.
        let stored = stored_usage(&app, "a-only").await?;
        assert_eq!(stored["weekly_fable"]["used_percent"], 100.0, "{stored}");
        assert!(stored["weekly_opus"].is_null(), "{stored}");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-5: a limit reported to a run on the fallback is the fallback model's. The run on Opus 5.5
/// is refused for its credits too: that fills `weekly_opus` and leaves Fable's window with its
/// own reset, and with neither model left the task waits at its limit, as before.
#[test]
fn a_limit_on_the_fallback_binds_the_fallback_model() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let reset_s = 40 * 3600;
        let app = build_with(
            &data,
            &fake,
            probe.clone(),
            refused_for_credits(Some(reset_s)),
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Refused twice").await?;
        ready(&app, &id).await?;
        app.send_input(&id, b"hello\r").await?;
        live_run(&app, &id, 2).await?;
        let fable = stored_usage(&app, "a-only").await?["weekly_fable"].clone();
        assert_eq!(fable["used_percent"], 100.0, "{fable}");
        assert!(fable["resets_at"].is_number(), "{fable}");

        type_into_run(&app, &id, 2, b"again\r").await?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let stored = stored_usage(&app, "a-only").await?;
        assert_eq!(stored["weekly_opus"]["used_percent"], 100.0, "{stored}");
        assert_eq!(stored["weekly_fable"], fable, "Fable's window is as it was");
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Limit);
        assert_eq!(run_accounts(&app, &id).await.len(), 2);
        // Neither model runs: nothing is available, and the pool says when one can again.
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let capacity = app.capacity_view(&ctx).await?;
        assert_eq!((capacity.available, &capacity.fallback_model), (0, &None));
        assert!(
            capacity.resumes_at.is_some(),
            "the earliest of the two resets"
        );
        needs_nobody(&app, &id).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-3 and AC-4: with Fable used up on every subscription a new task of a Fable model starts on
/// Opus 5.5 at once, and a task set to another model is launched as it is set. Once a
/// subscription can run Fable again, the run on the fallback is left alone; the task's next
/// launch (Continue after a stop) runs its own model again and says so.
#[test]
fn a_task_starts_on_the_fallback_and_its_next_launch_runs_fable_again() -> TestResult {
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
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        for account in ["a-first", "b-second"] {
            store_usage(&app, account, true).await?;
        }
        let id = task_of_model(&app, &sandbox, &user, "Default", None).await?;
        ready(&app, &id).await?;
        let mut watch = app.runtime(&id).hub.watch();
        assert_eq!(
            launched(&app, &id).await?,
            [(asked_for(OPUS), Some(OPUS.to_owned()))]
        );
        // AC-7: the task's detail names the model its run is on, and the capacity view counts
        // the subscriptions that take a default task on the fallback and names it.
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let detail = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(detail.model, FABLE);
        assert!(detail.model_default);
        assert_eq!(detail.fallback_model.as_deref(), Some(OPUS));
        let capacity = app.capacity_view(&ctx).await?;
        assert_eq!(capacity.fallback_model.as_deref(), Some(OPUS));
        assert_eq!(capacity.available, 2);
        assert!(capacity.accounts.iter().all(|account| account.available));
        assert_eq!(capacity.resumes_at, None);
        let older = Some("claude-fable-5-1");
        let chosen = task_of_model(&app, &sandbox, &user, "Fable 5.1", older).await?;
        ready(&app, &chosen).await?;
        assert_eq!(
            launched(&app, &chosen).await?,
            [(asked_for(OPUS), Some(OPUS.to_owned()))],
            "a chosen Fable model falls back too"
        );
        let sonnet = "claude-sonnet-5-5";
        let other = task_of_model(&app, &sandbox, &user, "Sonnet", Some(sonnet)).await?;
        ready(&app, &other).await?;
        assert_eq!(
            launched(&app, &other).await?,
            [(asked_for(sonnet), Some(sonnet.to_owned()))],
            "another model is launched as it is set"
        );
        for task in [&chosen, &other] {
            stop(&app, task).await?;
        }

        // Fable runs again on one subscription: the run on the fallback is not restarted for it.
        store_usage(&app, "b-second", false).await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("the turn", Duration::from_secs(30), || async {
            crate::pool_supervision::echoed(&app, &id)
                .iter()
                .any(|line| line.contains("hello"))
        })
        .await?;
        let _ = app.resume_round().await;
        assert_eq!(run_accounts(&app, &id).await.len(), 1, "the run stays");
        // New tasks would run Fable there: the pool is no longer in fallback, this run still is.
        let capacity = app.capacity_view(&ctx).await?;
        assert_eq!(capacity.fallback_model, None);
        assert_eq!(capacity.available, 1);
        let detail = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(detail.fallback_model.as_deref(), Some(OPUS));

        stop(&app, &id).await?;
        assert_eq!(
            app.task_detail(&ctx, id.as_str()).await?.fallback_model,
            None,
            "no run, no model it runs on"
        );
        app.continue_task(&ctx, id.as_str(), &ContinueTask { account_id: None })
            .await?;
        live_run(&app, &id, 2).await?;
        assert_eq!(
            run_accounts(&app, &id).await[1].as_deref(),
            Some("b-second"),
            "the subscription that runs Fable"
        );
        assert_eq!(
            launched(&app, &id).await?[1],
            ("None".to_owned(), Some(FABLE.to_owned()))
        );
        let mut said = Vec::new();
        wait_for("the line", Duration::from_secs(30), || {
            said.extend(notices(&mut watch));
            let found = said.iter().any(|notice| notice == BACK);
            async move { found }
        })
        .await?;
        assert!(
            !said.iter().any(|notice| notice == FELL_BACK),
            "said only at the launch that fell back: {said:?}"
        );
        assert_eq!(
            app.task_detail(&ctx, id.as_str()).await?.fallback_model,
            None
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A subscription that runs neither model and never says when that ends (its usage credits are
/// switched off): the task falls back, is refused there too and waits; a provider's reading
/// ends both limits and the pool tries once more, Fable first, then the fallback. Refused the
/// same way again, a person is asked, as SPEC-464 UCL-010 asks for a model with no fallback:
/// four refused requests in all, never a loop of restarts.
#[test]
fn a_task_refused_for_both_models_is_retried_once_then_asks_a_person() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        // Refused from the first message of every run, with no word on when that ends.
        let agent = FakeAgentOptions {
            limit_after: Some(0),
            ..refused_for_credits(None)
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Neither model").await?;
        ready(&app, &id).await?;
        app.send_input(&id, b"hello\r").await?;
        type_into_run(&app, &id, 2, b"again\r").await?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let stored = stored_usage(&app, "a-only").await?;
        for window in ["weekly_fable", "weekly_opus"] {
            assert_eq!(stored[window]["used_percent"], 100.0, "{window}: {stored}");
            assert!(stored[window]["resets_at"].is_null(), "{window}: {stored}");
        }
        needs_nobody(&app, &id).await?;

        // A provider's reading ends both limits: the one retry, with the task's own model.
        probe.set("a-only", usage(10.0, 4 * HOUR, 30.0, 100));
        reading(&app, "a-only").await?;
        let _ = app.resume_round().await;
        type_into_run(&app, &id, 3, b"third\r").await?;
        live_run(&app, &id, 4).await?;
        needs_nobody(&app, &id).await?;
        type_into_run(&app, &id, 4, b"fourth\r").await?;
        wait_for("the request", Duration::from_secs(30), || async {
            app.deps
                .store
                .audit_details(id.as_str(), "task.limit_without_end", 10)
                .await
                .is_ok_and(|entries| entries.len() == 1)
        })
        .await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(task.status, TaskStatus::Running, "not paused again");
        assert_eq!(task.activity, Some(Activity::NeedsAttention));
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        let fable = ("None".to_owned(), Some(FABLE.to_owned()));
        let opus = (asked_for(OPUS), Some(OPUS.to_owned()));
        assert_eq!(
            launched(&app, &id).await?,
            [fable.clone(), opus.clone(), fable, opus]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
