//! SPEC-486: a usage limit met while a session runs a dynamic workflow (Claude Code's Workflow
//! tool). The limit refuses the workflow's agents, is written to their transcripts only and
//! pauses the workflow without waking the session's own thread; Yulius moves the task as at any
//! limit and tells the relaunched session to resume the workflow by its run id.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_with, has_status, new_task, refresh, runtime, screen, seed, stop,
    task, temp, usage, wait_for,
};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use yulius_app::context::App;
use yulius_app::model::{AccountStatus, LaunchKind, PauseKind};
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

type BoxError = Box<dyn std::error::Error>;

fn workflow_agent(limit: bool) -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        workflow_limit: limit,
        ..FakeAgentOptions::default()
    }
}

async fn on_screen(app: &App, id: &TaskId, text: &str) -> bool {
    screen(app, id).contains(text)
}

/// The task's second run, once it lives.
async fn second_run_lives(app: &App, id: &TaskId) -> bool {
    let attempts = app.deps.store.attempts(id).await.unwrap_or_default();
    attempts.len() == 2 && attempts[1].state == AttemptState::Live
}

/// What the task's run in `run_dir` was launched with, and the workflow run its prompt names.
fn relaunch(run_dir: &str) -> Result<(String, String), BoxError> {
    let launch = std::fs::read_to_string(Path::new(run_dir).join("fake-launch.txt"))?;
    let run: String = launch
        .split_once("(run ")
        .map(|(_, rest)| rest.chars().take_while(|c| *c != ')').collect())
        .ok_or_else(|| format!("no run named in {launch}"))?;
    Ok((launch, run))
}

/// WFL-004: the relaunch opens with the prompt that resumes the workflow by its run id, in the
/// same session, in place of the silent continuation of SPEC-034.
fn resume_prompt_problems(launch: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for needed in [
        "Resume {",
        "continue_interrupted: false",
        "prompt: Some(\"Yulius continued this session in a new process",
        "resumeFromRunId",
        "Do not start the workflow again from the beginning",
    ] {
        if !launch.contains(needed) {
            problems.push(format!("{needed:?} is not in {launch}"));
        }
    }
    problems
}

/// What the task's summary shows a person: its status, the reset it waits for, its background
/// work.
async fn shown(app: &App, id: &TaskId) -> Result<(String, bool, Option<u32>), BoxError> {
    let summary = app.task_summary(&task(app, id).await?).await?;
    Ok((
        summary.status,
        summary.resumes_at.is_some(),
        summary.background_shells,
    ))
}

/// The report of task #182 (2026-11-13): the limit refuses a workflow's agent, the workflow
/// pauses and the session's own transcript shows a finished turn. With another subscription
/// free, the task moves to it by itself, with no pause and no person; the session is relaunched
/// with the prompt that resumes the run by its id, and the run finishes: its first agent's
/// result is kept, the refused agent and the next run (SPEC-486 WFL-002, WFL-004).
#[test]
fn a_limit_met_by_a_workflow_s_agent_moves_the_task_and_resumes_the_workflow() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), workflow_agent(true))?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = new_task(&app, &sandbox, &user, "Audit").await?;
        wait_for("the run", Duration::from_secs(30), || {
            on_screen(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"/workflow\r").await?;
        wait_for(
            "the move to the other subscription",
            Duration::from_secs(90),
            || second_run_lives(&app, &id),
        )
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let (first, second) = (&attempts[0], &attempts[1]);
        assert_ne!(first.account_id, second.account_id, "another subscription");
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(
            first.provider_session_id, second.provider_session_id,
            "same session"
        );
        let (launch, run) = relaunch(&second.run_dir)?;
        let problems = resume_prompt_problems(&launch);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(run.starts_with("wf_fake0001-"), "{run}");
        // The resumed process finds the run unfinished, the prompt resumes it, and it ends.
        let completed = format!("WORKFLOW {run} COMPLETED");
        wait_for("the workflow's end", Duration::from_secs(60), || {
            on_screen(&app, &id, &completed)
        })
        .await?;
        let seen = screen(&app, &id);
        assert!(seen.contains(&format!("WORKFLOW {run} ORPHANED")), "{seen}");
        assert!(seen.contains(&format!("WORKFLOW {run} RESUMED")), "{seen}");
        assert!(!seen.contains("ECHO:"), "nothing else was typed: {seen}");
        assert!(
            app.deps.store.open_pause(&id).await?.is_none(),
            "no pause: a subscription was free"
        );
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        let switches = app.deps.store.switches(&id).await?;
        assert_eq!(switches.len(), 1, "one move: {switches:?}");
        // The limit is the subscription's: its window is full until the reset the agent's
        // record names, so nothing new starts on it.
        let limited = app
            .deps
            .store
            .account(first.account_id.as_ref().ok_or("first account")?)
            .await?
            .ok_or("the first account")?;
        assert_eq!(limited.status, AccountStatus::Limited);
        let window =
            yulius_app::usage_json::from_json(limited.usage_json.as_deref().unwrap_or("{}"))
                .primary
                .ok_or("the window of the observed limit")?;
        assert_eq!(window.used_percent, Some(100.0));
        assert_eq!(window.window_minutes, Some(300));
        assert!(window.resets_at.is_some(), "the reset of the limit");
        // The workflow ended: the session waits for nothing.
        wait_for("no background work", Duration::from_secs(20), || async {
            shown(&app, &id)
                .await
                .is_ok_and(|(_, _, background)| background.unwrap_or(0) == 0)
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// WFL-005: with no subscription that can take the task, the limit a workflow's agent met opens
/// a `limit` pause: the task reads paused with the reset it waits for, not working. When a
/// subscription has room again the task resumes there by itself, with the prompt that resumes
/// the run.
#[test]
fn with_no_subscription_free_the_task_waits_and_then_resumes_the_workflow() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), workflow_agent(true))?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Audit").await?;
        wait_for("the run", Duration::from_secs(30), || {
            on_screen(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"/workflow\r").await?;
        wait_for("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Limit);
        assert_eq!(
            pause.account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );
        let (status, resumes, _) = shown(&app, &id).await?;
        assert_eq!(status, "waiting_capacity", "paused, not working");
        assert!(resumes, "with the reset it waits for");
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "nothing moved"
        );

        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        wait_for("the resumed run", Duration::from_secs(60), || {
            second_run_lives(&app, &id)
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        let (launch, run) = relaunch(&attempts[1].run_dir)?;
        let problems = resume_prompt_problems(&launch);
        assert!(problems.is_empty(), "{problems:?}");
        let completed = format!("WORKFLOW {run} COMPLETED");
        wait_for("the workflow's end", Duration::from_secs(60), || {
            on_screen(&app, &id, &completed)
        })
        .await?;
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        stop(&app, &id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The first screenshot of the report: the session's own thread is refused too, after the
/// launch ("Usage limit reached · continuing automatically"), so the limit is in its own
/// transcript as well. One limit, one move; the relaunch steps the transcript back over the
/// limit and opens with the prompt that resumes the workflow, not with the silent continuation
/// (which would leave the workflow to chance).
#[test]
fn a_limit_that_also_cuts_the_session_s_turn_moves_once_and_resumes_the_workflow() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            limit_after: Some(1),
            ..workflow_agent(true)
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = new_task(&app, &sandbox, &user, "Audit").await?;
        wait_for("the run", Duration::from_secs(30), || {
            on_screen(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"/workflow\r").await?;
        wait_for(
            "the move to the other subscription",
            Duration::from_secs(90),
            || second_run_lives(&app, &id),
        )
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_ne!(attempts[0].account_id, attempts[1].account_id);
        let (launch, run) = relaunch(&attempts[1].run_dir)?;
        let problems = resume_prompt_problems(&launch);
        assert!(problems.is_empty(), "{problems:?}");
        let completed = format!("WORKFLOW {run} COMPLETED");
        wait_for("the workflow's end", Duration::from_secs(60), || {
            on_screen(&app, &id, &completed)
        })
        .await?;
        // Stepped back: the session's own transcript holds no limit record (the refused
        // agent's file keeps its own, which no later run reads again).
        let session = attempts[1]
            .provider_session_id
            .clone()
            .ok_or("the session")?;
        let mut own = Vec::new();
        let mut dirs = vec![data.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    dirs.push(path);
                } else if path.file_name().and_then(|n| n.to_str())
                    == Some(format!("{session}.jsonl").as_str())
                {
                    own.push(path);
                }
            }
        }
        assert_eq!(own.len(), 1, "the session's transcript: {own:?}");
        let text = std::fs::read_to_string(&own[0])?;
        assert!(!text.contains("usage_limit_reached"), "stepped back");
        assert!(text.contains("\"local_workflow\""), "the launch stays");
        let switches = app.deps.store.switches(&id).await?;
        assert_eq!(switches.len(), 1, "one limit, one move: {switches:?}");
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        stop(&app, &id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// WFL-003, WFL-004: a workflow is known from its launch, with no limit at all: nothing moves
/// and nothing pauses while it runs, and a relaunch of the session for another reason (here a
/// move a person asks for) ends the workflow with the old process and tells the session's
/// agent to resume it, after no other words.
#[test]
fn a_move_while_a_workflow_runs_tells_the_agent_to_resume_it() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), workflow_agent(false))?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = new_task(&app, &sandbox, &user, "Audit").await?;
        wait_for("the run", Duration::from_secs(30), || {
            on_screen(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"/workflow\r").await?;
        wait_for("the launch", Duration::from_secs(30), || {
            on_screen(&app, &id, "LAUNCHED")
        })
        .await?;
        // The launch is read from the transcript a moment after it is on the screen.
        wait_for("the workflow, read", Duration::from_secs(30), || async {
            app.dynamic_workflows(&id).len() == 1
        })
        .await?;
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        assert!(app.deps.store.switches(&id).await?.is_empty());
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);

        let other = app.deps.store.attempts(&id).await?[0]
            .account_id
            .clone()
            .map(|first| {
                if first.as_str() == "a-first" {
                    "b-second"
                } else {
                    "a-first"
                }
            })
            .ok_or("the first run's subscription")?;
        app.switch_task_now(&id, Some(AccountId::parse(other)?), "manual", None)
            .await?;
        wait_for("the moved run", Duration::from_secs(60), || {
            second_run_lives(&app, &id)
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let (launch, run) = relaunch(&attempts[1].run_dir)?;
        assert!(launch.contains("resumeFromRunId"), "{launch}");
        assert!(launch.contains("continue_interrupted: false"), "{launch}");
        let completed = format!("WORKFLOW {run} COMPLETED");
        wait_for("the workflow's end", Duration::from_secs(60), || {
            on_screen(&app, &id, &completed)
        })
        .await?;
        wait_for("nothing unfinished", Duration::from_secs(30), || async {
            app.dynamic_workflows(&id).is_empty()
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
