//! Forked tasks: a fork continues a copy of its source's conversation (a fixture
//! conversation in the source's transcript) from the source branch's head, on its own branch and
//! worktree, and the source goes on untouched.
//!
//! End to end: supervisor → detached `yulius ptyhost` → fake agent, which copies the forked
//! transcript under its new session id as `claude --resume <id> --fork-session` does; real
//! worktrees over a local origin.

use crate::support;
use crate::support::live::pool::{
    ScriptedUsage, build_with, new_task, runtime, screen, seed, stop, task, temp,
};
use crate::support::{eventually, git};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use support::TestResult;
use support::agents::FakeAgentOptions;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::error::AppError;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{StepRecord, TaskMode, TaskOrigin, TaskRecord};
use yulius_contracts::api::tasks::ForkTask;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, StepId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;

type BoxError = Box<dyn std::error::Error>;

/// What the source said before it was forked: the fork's agent must have it.
const FIXTURE_WORD: &str = "pelican-fixture-628";

async fn ctx(app: &App, user: &UserId) -> Result<AuthContext, BoxError> {
    Ok(AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

async fn ready(app: &Arc<App>, id: &TaskId) -> TestResult {
    eventually("the run", Duration::from_secs(30), || {
        let (app, id) = (app.clone(), id.clone());
        async move { screen(&app, &id).contains("FAKE-AGENT READY") }
    })
    .await
}

/// The Claude transcript of `session` in `task`'s worktree.
fn transcript(app: &App, task: &TaskRecord, session: &str) -> Result<PathBuf, BoxError> {
    let worktree = task.worktree_path.as_deref().ok_or("no worktree")?;
    let sandboxes = app.config.data_dir.join("sandboxes");
    Ok(app.deps.harness.claude_transcript(
        &sandboxes.join(task.sandbox_id.as_str()).join("home"),
        Path::new(worktree),
        session,
    ))
}

async fn latest_session(app: &App, id: &TaskId) -> Result<String, BoxError> {
    Ok(app
        .deps
        .store
        .provider_sessions(id)
        .await?
        .pop()
        .ok_or("no session")?
        .session_id)
}

/// The fork resumes a copy of the source's conversation, with `cwd` its
/// own worktree, from the source branch's head commit; the source's record, plan, transcript,
/// branch and live run are unchanged; both pages link the two.
#[test]
fn a_fork_continues_the_sources_conversation_and_branch_and_leaves_the_source_alone() -> TestResult
{
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe, agent)?;
        let (sandbox, user) = seed(&app, &data, &["a1"]).await?;
        let source_id = new_task(&app, &sandbox, &user, "Parser").await?;
        ready(&app, &source_id).await?;
        let mut source = task(&app, &source_id).await?;
        let source_session = latest_session(&app, &source_id).await?;
        let source_transcript = transcript(&app, &source, &source_session)?;
        // The fixture conversation in the source's transcript (the fake writes one only for a
        // turn), ending in a line still being written.
        let fixture = [
            serde_json::json!({
                "type": "user", "sessionId": source_session,
                "cwd": source.worktree_path, "timestamp": "2026-11-22T10:00:00.000Z",
                "uuid": "fixture-user-1",
                "message": {"role": "user", "content": format!("remember {FIXTURE_WORD}")}
            }),
            serde_json::json!({
                "type": "assistant", "sessionId": source_session,
                "cwd": source.worktree_path, "timestamp": "2026-11-22T10:00:01.000Z",
                "uuid": "fixture-assistant-1", "parentUuid": "fixture-user-1",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}
            }),
        ];
        let mut text = std::fs::read_to_string(&source_transcript).unwrap_or_default();
        for record in &fixture {
            text.push_str(&format!("{record}\n"));
        }
        text.push_str("{\"type\":\"assistant\",\"partial");
        std::fs::create_dir_all(source_transcript.parent().ok_or("folder")?)?;
        std::fs::write(&source_transcript, &text)?;
        // Committed and uncommitted work in the source's worktree.
        let worktree = PathBuf::from(source.worktree_path.as_deref().ok_or("worktree")?);
        std::fs::write(worktree.join("work.txt"), "committed\n")?;
        git(&worktree, &["add", "work.txt"])?;
        git(&worktree, &["commit", "-q", "-m", "source work"])?;
        std::fs::write(worktree.join("draft.txt"), "uncommitted\n")?;
        let head = git(&worktree, &["rev-parse", "HEAD"])?;
        let step = StepRecord {
            id: StepId::parse("step-src-1")?,
            task_id: source_id.clone(),
            ordinal: 1,
            title: "Parse the header".into(),
            status: "done".into(),
            note: Some("kept".into()),
            updated_by: Some("agent".into()),
            updated_at: Some(Timestamp(5)),
            effort: 2,
        };
        app.deps
            .store
            .replace_steps(
                &source_id,
                std::slice::from_ref(&step),
                "agent",
                Timestamp(5),
            )
            .await?;
        source = task(&app, &source_id).await?;
        let source_attempt = app
            .deps
            .store
            .live_attempt(&source_id)
            .await?
            .ok_or("source run")?;

        let summary = app
            .fork_task(
                &ctx(&app, &user).await?,
                source_id.as_str(),
                &ForkTask {
                    message: Some("Try a hand-written parser instead.".into()),
                    ..ForkTask::default()
                },
            )
            .await?;
        let fork_id = TaskId::parse(&summary.id)?;
        assert_eq!(summary.title, "Fork of #1: Parser");
        assert_eq!(summary.harness, "claude");
        ready(&app, &fork_id).await?;
        let fork = task(&app, &fork_id).await?;

        // A copy of the source's session under a new id Yulius named.
        let attempt = app
            .deps
            .store
            .live_attempt(&fork_id)
            .await?
            .ok_or("fork run")?;
        let launch = std::fs::read_to_string(Path::new(&attempt.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains(&format!("session_id: \"{source_session}\"")),
            "{launch}"
        );
        assert!(
            launch.contains("fork: Some(SessionFork { session_id: Some("),
            "{launch}"
        );
        let sessions = app.deps.store.provider_sessions(&fork_id).await?;
        assert_eq!(
            sessions.len(),
            1,
            "only the fork's own session: {sessions:?}"
        );
        let fork_session = sessions[0].session_id.clone();
        assert_ne!(fork_session, source_session);
        // The agent read the fixture conversation, its `cwd` now the fork's worktree, and got
        // the opening message after it; the line still being written stayed out.
        let fork_text = std::fs::read_to_string(transcript(&app, &fork, &fork_session)?)?;
        let records: Vec<serde_json::Value> = fork_text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        let carried = records
            .iter()
            .find(|r| r["uuid"] == "fixture-user-1")
            .ok_or("the fixture conversation is not in the fork")?;
        assert_eq!(carried["sessionId"], fork_session.as_str());
        assert_eq!(
            carried["cwd"],
            fork.worktree_path.as_deref().ok_or("worktree")?
        );
        assert!(!fork_text.contains("partial"));
        eventually("the opening turn", Duration::from_secs(30), || {
            let (app, id) = (app.clone(), fork_id.clone());
            async move { screen(&app, &id).contains("Try a hand-written parser instead.") }
        })
        .await?;
        let opening = screen(&app, &fork_id);
        assert!(opening.contains("a fork of task #1"), "{opening}");

        // Its own branch and worktree, at the source's head; no uncommitted work.
        let fork_tree = PathBuf::from(fork.worktree_path.as_deref().ok_or("worktree")?);
        assert_ne!(fork.branch, source.branch);
        assert_ne!(fork_tree, worktree);
        assert_eq!(git(&fork_tree, &["rev-parse", "HEAD"])?, head);
        assert!(fork_tree.join("work.txt").exists());
        assert!(!fork_tree.join("draft.txt").exists());
        // A later commit of the source leaves the fork where it started.
        std::fs::write(worktree.join("more.txt"), "later\n")?;
        git(&worktree, &["add", "more.txt"])?;
        git(&worktree, &["commit", "-q", "-m", "later"])?;
        let fork_branch = fork.branch.clone().ok_or("branch")?;
        assert_eq!(git(&fork_tree, &["rev-parse", &fork_branch])?, head);

        // The plan is a copy, statuses and notes kept.
        let steps = app.deps.store.steps(&fork_id).await?;
        assert_eq!(steps.len(), 1);
        assert_eq!(
            (
                steps[0].title.as_str(),
                steps[0].status.as_str(),
                steps[0].note.as_deref()
            ),
            ("Parse the header", "done", Some("kept"))
        );
        assert_ne!(steps[0].id, step.id);

        // The source is untouched and its run goes on.
        let after = task(&app, &source_id).await?;
        assert_eq!(
            (
                &after.status,
                &after.branch,
                &after.worktree_path,
                &after.prompt
            ),
            (
                &source.status,
                &source.branch,
                &source.worktree_path,
                &source.prompt
            )
        );
        assert_eq!(std::fs::read_to_string(&source_transcript)?, text);
        assert_eq!(app.deps.store.steps(&source_id).await?, vec![step]);
        assert_eq!(latest_session(&app, &source_id).await?, source_session);
        let live = app
            .deps
            .store
            .live_attempt(&source_id)
            .await?
            .ok_or("source run")?;
        assert_eq!(live.id, source_attempt.id);

        // Each page links the other.
        let fork_summary = app.task_summary(&fork).await?;
        let origin = fork_summary.forked_from.ok_or("forked_from")?;
        assert_eq!(
            (
                origin.task_id.as_deref(),
                origin.number,
                origin.commit.as_str()
            ),
            (Some(source_id.as_str()), 1, head.as_str())
        );
        assert_eq!(origin.session_id, source_session);
        let source_summary = app.task_summary(&after).await?;
        assert_eq!(source_summary.forks.len(), 1);
        assert_eq!(source_summary.forks[0].task_id, fork_id.as_str());

        stop(&app, &fork_id).await?;
        stop(&app, &source_id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn closed_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    harness: Harness,
) -> Result<TaskRecord, BoxError> {
    Ok(app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: "Never ran".into(),
            harness,
            title: None,
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: false,
            mode: TaskMode::Prompt,
            supertask: false,
        })
        .await?)
}

/// An Antigravity task, a task without a conversation, a message over
/// 64 KiB and a model of another harness are refused, and nothing is created.
#[test]
fn forks_that_cannot_continue_are_refused_and_create_nothing() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe, FakeAgentOptions::default())?;
        let (sandbox, user) = seed(&app, &data, &["a1"]).await?;
        let ctx = ctx(&app, &user).await?;
        let agy = closed_task(&app, &sandbox, &user, Harness::Antigravity).await?;
        let unstarted = closed_task(&app, &sandbox, &user, Harness::ClaudeCode).await?;
        // Every task of the sandbox, archived or not.
        let count = || async {
            let store = &app.deps.store;
            Ok::<_, AppError>(
                store.tasks(&sandbox, false).await?.len()
                    + store.tasks(&sandbox, true).await?.len(),
            )
        };
        let before = count().await?;
        let refusal = |result: Result<_, AppError>| match result {
            Err(AppError::Invalid(text)) => Ok(text),
            other => Err(format!("expected a refusal, got {other:?}")),
        };
        let text = refusal(
            app.fork_task(&ctx, agy.id.as_str(), &ForkTask::default())
                .await,
        )?;
        assert!(text.contains("Antigravity cannot fork"), "{text}");
        let text = refusal(
            app.fork_task(&ctx, unstarted.id.as_str(), &ForkTask::default())
                .await,
        )?;
        assert!(text.contains("no conversation to fork yet"), "{text}");
        let long = ForkTask {
            message: Some("x".repeat(64 * 1024 + 1)),
            ..ForkTask::default()
        };
        let text = refusal(app.fork_task(&ctx, unstarted.id.as_str(), &long).await)?;
        assert!(text.contains("64 KiB"), "{text}");
        let codex_model = ForkTask {
            model: Some("gpt-6.2-sol".into()),
            ..ForkTask::default()
        };
        refusal(
            app.fork_task(&ctx, unstarted.id.as_str(), &codex_model)
                .await,
        )?;
        // A subscription of another provider, or one that is gone.
        crate::support::codex::connect_codex(&app).await?;
        for (account, said) in [
            ("a-codex", "is not a Claude subscription"),
            ("gone", "no longer exists"),
        ] {
            let named = ForkTask {
                account_id: Some(account.into()),
                ..ForkTask::default()
            };
            match app.fork_task(&ctx, unstarted.id.as_str(), &named).await {
                Err(AppError::Capacity(text)) => assert!(text.contains(said), "{text}"),
                other => return Err(format!("expected a refusal, got {other:?}").into()),
            }
        }
        assert_eq!(count().await?, before, "nothing was created");
        let staging = app.config.data_dir.join("transcripts/.staging");
        assert!(
            !staging.exists() || std::fs::read_dir(&staging)?.next().is_none(),
            "nothing was staged"
        );
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The launch request the fake recorded for `attempt` (its `fake-launch.txt`).
fn launch_of(attempt: &yulius_app::model::AttemptRecord) -> Result<String, BoxError> {
    Ok(std::fs::read_to_string(
        Path::new(&attempt.run_dir).join("fake-launch.txt"),
    )?)
}

/// A Codex fork that waits in the queue (the source holds the only
/// session slot) keeps the subscription the person named, launches as `codex fork <source>`
/// once the slot frees, adopts a session of its own, and its next launch resumes that session.
#[test]
fn a_codex_fork_waits_with_its_subscription_then_resumes_its_own_session() -> TestResult {
    use crate::support::live::supervision::{build_full, screen_contains, seed_records, seed_repo};
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            answers_first_message: true,
            resume_prompts: true,
            ..Default::default()
        };
        let app = build_full(&data, Some(&fake), agent, 1)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        crate::support::codex::connect_codex(&app).await?;
        let named = crate::support::codex::add_codex_account(
            &app,
            "a-codex-2",
            concat!(
                r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"h.e30.s","#,
                r#""access_token":"SYNTHETIC-access-2","refresh_token":"SYNTHETIC-refresh","#,
                r#""account_id":"acc-synthetic"},"last_refresh":"2026-11-01T10:00:00Z"}"#
            ),
        )
        .await?;
        let source_id =
            crate::support::codex::new_codex_task(&app, &sandbox, &user, "Parser").await?;
        eventually(
            "the source's conversation",
            Duration::from_secs(30),
            || async {
                app.deps
                    .store
                    .provider_sessions(&source_id)
                    .await
                    .is_ok_and(|rows| !rows.is_empty())
                    && screen_contains(&app, &source_id, "ECHO: Parser").await
            },
        )
        .await?;
        let source_session = latest_session(&app, &source_id).await?;
        let ctx = ctx(&app, &user).await?;
        let summary = app
            .fork_task(
                &ctx,
                source_id.as_str(),
                &ForkTask {
                    message: Some("Try another parser.".into()),
                    account_id: Some(named.as_str().into()),
                    ..ForkTask::default()
                },
            )
            .await?;
        let fork_id = TaskId::parse(&summary.id)?;
        assert_eq!(summary.harness, "codex");
        assert_eq!(task(&app, &fork_id).await?.status, TaskStatus::Queued);
        let kept = app
            .deps
            .store
            .queued_start(&fork_id)
            .await?
            .ok_or("the fork's start is kept")?;
        assert_eq!(kept.account_id.as_ref(), Some(&named));
        support::stop_tasks(&app, &[&source_id]).await?;
        eventually(
            "the fork's own session",
            Duration::from_secs(30),
            || async {
                app.deps
                    .store
                    .provider_sessions(&fork_id)
                    .await
                    .is_ok_and(|rows| !rows.is_empty())
                    && screen_contains(&app, &fork_id, "Try another parser.").await
            },
        )
        .await?;
        let first = app
            .deps
            .store
            .live_attempt(&fork_id)
            .await?
            .ok_or("fork run")?;
        assert_eq!(
            first.account_id.as_ref(),
            Some(&named),
            "the named subscription"
        );
        assert_eq!(
            app.deps.store.task_pin(&fork_id).await?.as_ref(),
            Some(&named),
            "a person's choice pins the fork there (SPEC-629 RBL-005)"
        );
        let launch = launch_of(&first)?;
        assert!(
            launch.contains(&format!("session_id: \"{source_session}\"")),
            "{launch}"
        );
        assert!(
            launch.contains("fork: Some(SessionFork { session_id: None })"),
            "{launch}"
        );
        let fork_session = latest_session(&app, &fork_id).await?;
        assert_ne!(fork_session, source_session);
        assert_eq!(app.deps.store.provider_sessions(&fork_id).await?.len(), 1);
        assert_eq!(latest_session(&app, &source_id).await?, source_session);

        // Its next launch resumes its own session, not another copy of the source's.
        support::stop_tasks(&app, &[&fork_id]).await?;
        app.continue_task(&ctx, fork_id.as_str(), &Default::default())
            .await?;
        eventually("the fork's second run", Duration::from_secs(30), || async {
            app.deps
                .store
                .live_attempt(&fork_id)
                .await
                .is_ok_and(|run| run.is_some_and(|run| run.id != first.id))
        })
        .await?;
        let second = app
            .deps
            .store
            .live_attempt(&fork_id)
            .await?
            .ok_or("second run")?;
        let launch = launch_of(&second)?;
        assert!(
            launch.contains(&format!("session_id: \"{fork_session}\"")),
            "{launch}"
        );
        assert!(launch.contains("fork: None"), "{launch}");
        support::stop_tasks(&app, &[&fork_id]).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A fork whose first launch fails before a process runs has no session of its
/// own, so its next launch is still a fork of the source's session.
#[test]
fn a_fork_whose_first_launch_failed_launches_as_a_fork_again() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fail = dir.path().join("fail-launch");
        let agent = FakeAgentOptions {
            transcript: true,
            fail_launch_file: Some(fail.clone()),
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a1"]).await?;
        let source_id = new_task(&app, &sandbox, &user, "Parser").await?;
        ready(&app, &source_id).await?;
        let source = task(&app, &source_id).await?;
        let source_session = latest_session(&app, &source_id).await?;
        let path = transcript(&app, &source, &source_session)?;
        std::fs::create_dir_all(path.parent().ok_or("folder")?)?;
        std::fs::write(&path, "{\"type\":\"user\",\"uuid\":\"fixture-user-1\"}\n")?;
        std::fs::write(&fail, "")?;
        let ctx = ctx(&app, &user).await?;
        let summary = app
            .fork_task(&ctx, source_id.as_str(), &ForkTask::default())
            .await?;
        let fork_id = TaskId::parse(&summary.id)?;
        eventually("the failed launch", Duration::from_secs(30), || {
            let (app, id) = (app.clone(), fork_id.clone());
            async move { task(&app, &id).await.is_ok_and(|t| t.status.is_resting()) }
        })
        .await?;
        assert!(app.deps.store.provider_sessions(&fork_id).await?.is_empty());
        std::fs::remove_file(&fail)?;
        app.continue_task(&ctx, fork_id.as_str(), &Default::default())
            .await?;
        ready(&app, &fork_id).await?;
        let run = app
            .deps
            .store
            .live_attempt(&fork_id)
            .await?
            .ok_or("fork run")?;
        let launch = launch_of(&run)?;
        assert!(
            launch.contains(&format!("session_id: \"{source_session}\"")),
            "{launch}"
        );
        assert!(
            launch.contains("fork: Some(SessionFork { session_id: Some("),
            "{launch}"
        );
        stop(&app, &fork_id).await?;
        stop(&app, &source_id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A queue pass between the fork's task and its record starts
/// nothing: the fork waits where the queue does not start it until its record is written, and
/// then launches as a fork.
#[test]
fn a_queue_pass_before_the_forks_record_starts_nothing() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a1"]).await?;
        let source_id = new_task(&app, &sandbox, &user, "Parser").await?;
        ready(&app, &source_id).await?;
        let source = task(&app, &source_id).await?;
        let source_session = latest_session(&app, &source_id).await?;
        let path = transcript(&app, &source, &source_session)?;
        std::fs::create_dir_all(path.parent().ok_or("folder")?)?;
        std::fs::write(&path, "{\"type\":\"user\",\"uuid\":\"fixture-user-1\"}\n")?;
        let (at, reached) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::ForkInserted,
            &app.config.data_dir.join("sandboxes").join(sandbox.as_str()),
            move || {
                let _ = at.send(());
                let _ = wait.recv_timeout(Duration::from_secs(60));
            },
        );
        let ctx = ctx(&app, &user).await?;
        let forking = {
            let (app, id) = (app.clone(), source_id.clone());
            tokio::spawn(
                async move { app.fork_task(&ctx, id.as_str(), &ForkTask::default()).await },
            )
        };
        tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(60)))
            .await??;
        // The task exists, its record does not: the queue starts nothing.
        let waiting: Vec<TaskRecord> = app
            .deps
            .store
            .tasks(&sandbox, false)
            .await?
            .into_iter()
            .filter(|t| t.id != source_id)
            .collect();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].status, TaskStatus::Prepared);
        app.drain_queue_now().await;
        assert_eq!(
            task(&app, &waiting[0].id).await?.status,
            TaskStatus::Prepared
        );
        assert!(app.deps.store.live_attempt(&waiting[0].id).await?.is_none());
        go.send(())?;
        let fork_id = TaskId::parse(&forking.await??.id)?;
        assert_eq!(fork_id, waiting[0].id);
        ready(&app, &fork_id).await?;
        let run = app
            .deps
            .store
            .live_attempt(&fork_id)
            .await?
            .ok_or("fork run")?;
        let launch = launch_of(&run)?;
        assert!(launch.contains("fork: Some(SessionFork"), "{launch}");
        stop(&app, &fork_id).await?;
        stop(&app, &source_id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The `task_fork` tool forks a task of the project for the person the calling
/// agent works for, with origin `agent`, and answers the new task and where it came from.
#[test]
fn the_task_fork_tool_forks_a_task_of_the_project() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a1"]).await?;
        let source_id = new_task(&app, &sandbox, &user, "Parser").await?;
        ready(&app, &source_id).await?;
        let source = task(&app, &source_id).await?;
        let source_session = latest_session(&app, &source_id).await?;
        let path = transcript(&app, &source, &source_session)?;
        std::fs::create_dir_all(path.parent().ok_or("folder")?)?;
        std::fs::write(&path, "{\"type\":\"user\",\"uuid\":\"fixture-user-1\"}\n")?;
        // The calling agent: another task of the project with a run that holds this token.
        let caller = closed_task(&app, &sandbox, &user, Harness::ClaudeCode).await?;
        let token = "tok-fork-caller-0123456789abcdef";
        app.deps
            .store
            .insert_attempt(&yulius_app::model::AttemptRecord {
                id: yulius_domain::ids::AttemptId::parse("a-fork-caller")?,
                task_id: caller.id.clone(),
                seq: 1,
                harness: Harness::ClaudeCode,
                account_id: None,
                provider_session_id: None,
                launch_kind: yulius_app::model::LaunchKind::New,
                model: None,
                state: yulius_domain::task::AttemptState::Live,
                run_dir: "/nonexistent/run".into(),
                token_hash: app.deps.crypto.sha256_hex(token.as_bytes()),
                host_token: app.deps.crypto.seal(&[1u8; 32])?,
                ptyhost_pid: None,
                child_pid: None,
                started_at: app.now(),
                ended_at: None,
                exit_code: None,
                exit_reason: None,
            })
            .await?;
        let request: yulius_contracts::mcp::RpcRequest = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "task_fork", "arguments": {"number": source.number, "message": "Try another parser."},
                "_meta": {"yulius/structured": true}}
        }))?;
        let response = app.mcp_handle(token, request).await.ok_or("no response")?;
        let result = serde_json::to_value(&response)?["result"].clone();
        assert_ne!(result["isError"], serde_json::json!(true), "{result}");
        let answer = &result["structuredContent"];
        assert_eq!(answer["forked_from"]["number"], serde_json::json!(source.number), "{result}");
        assert_eq!(answer["forked_from"]["task_id"], source_id.as_str());
        let fork_id = TaskId::parse(answer["task"]["task_id"].as_str().ok_or("task_id")?)?;
        let fork = task(&app, &fork_id).await?;
        assert_eq!(fork.origin, TaskOrigin::Agent);
        assert_eq!(fork.created_by, user);
        let origin = app.deps.store.fork_of(&fork_id).await?.ok_or("fork record")?;
        assert_eq!(origin.message.as_deref(), Some("Try another parser."));
        ready(&app, &fork_id).await?;
        stop(&app, &fork_id).await?;
        stop(&app, &source_id).await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A fork whose record cannot be written leaves nothing: its
/// source is deleted before the record is written, so the record's link fails, and the task, its
/// stored copy and the staged copy are removed again.
#[test]
fn a_fork_whose_record_fails_leaves_nothing_behind() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a1"]).await?;
        let source_id = new_task(&app, &sandbox, &user, "Parser").await?;
        ready(&app, &source_id).await?;
        let source = task(&app, &source_id).await?;
        let source_session = latest_session(&app, &source_id).await?;
        let path = transcript(&app, &source, &source_session)?;
        std::fs::create_dir_all(path.parent().ok_or("folder")?)?;
        std::fs::write(&path, "{\"type\":\"user\",\"uuid\":\"fixture-user-1\"}\n")?;
        stop(&app, &source_id).await?;
        let (at, reached) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::ForkInserted,
            &app.config.data_dir.join("sandboxes").join(sandbox.as_str()),
            move || {
                let _ = at.send(());
                let _ = wait.recv_timeout(Duration::from_secs(60));
            },
        );
        let ctx = ctx(&app, &user).await?;
        let forking = {
            let (app, id) = (app.clone(), source_id.clone());
            tokio::spawn(
                async move { app.fork_task(&ctx, id.as_str(), &ForkTask::default()).await },
            )
        };
        tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(60)))
            .await??;
        let fork_id = app
            .deps
            .store
            .tasks(&sandbox, false)
            .await?
            .into_iter()
            .find(|t| t.id != source_id)
            .ok_or("the fork's task")?
            .id;
        assert!(app.deps.store.delete_task(&source_id).await?);
        go.send(())?;
        assert!(forking.await?.is_err(), "the fork fails");
        assert!(
            app.deps.store.task(&fork_id).await?.is_none(),
            "its task is gone"
        );
        let transcripts = app.config.data_dir.join("transcripts");
        assert!(
            !transcripts.join(fork_id.as_str()).exists(),
            "its stored copy is gone"
        );
        let staging = transcripts.join(".staging");
        assert!(
            !staging.exists() || std::fs::read_dir(&staging)?.next().is_none(),
            "nothing stays staged"
        );
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The `task.updated` summaries of `id` delivered on `events` so far.
fn summaries_of(
    events: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
    id: &TaskId,
) -> Vec<yulius_contracts::api::tasks::TaskSummary> {
    let mut found = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        if let yulius_contracts::events::UiEvent::TaskUpdated { task } = envelope.event
            && task.id == id.as_str()
        {
            found.push(*task);
        }
    }
    found
}

/// The source's page learns of its fork without a reload:
/// a fork publishes the source's summary with the fork in `forks`; deleting the fork publishes
/// the source without it, and deleting the source publishes the fork with its origin's id gone.
#[test]
fn a_fork_and_its_deletion_publish_the_other_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a1"]).await?;
        let source_id = new_task(&app, &sandbox, &user, "Parser").await?;
        ready(&app, &source_id).await?;
        let source = task(&app, &source_id).await?;
        let source_session = latest_session(&app, &source_id).await?;
        let path = transcript(&app, &source, &source_session)?;
        std::fs::create_dir_all(path.parent().ok_or("folder")?)?;
        std::fs::write(&path, "{\"type\":\"user\",\"uuid\":\"fixture-user-1\"}\n")?;
        stop(&app, &source_id).await?;
        let ctx = ctx(&app, &user).await?;
        let mut events = app.events.subscribe();
        let make = |app: Arc<App>, ctx: AuthContext, id: TaskId| async move {
            Ok::<_, BoxError>(TaskId::parse(
                &app.fork_task(&ctx, id.as_str(), &ForkTask::default())
                    .await?
                    .id,
            )?)
        };
        let first = make(app.clone(), ctx.clone(), source_id.clone()).await?;
        let published = summaries_of(&mut events, &source_id);
        assert!(
            published
                .last()
                .is_some_and(|s| s.forks.iter().any(|f| f.task_id == first.as_str())),
            "the source is published with its fork: {published:?}"
        );
        let second = make(app.clone(), ctx.clone(), source_id.clone()).await?;
        stop(&app, &first).await?;
        stop(&app, &second).await?;
        let _ = summaries_of(&mut events, &source_id);
        app.delete_task(&ctx, first.as_str()).await?;
        let published = summaries_of(&mut events, &source_id);
        assert!(
            published
                .last()
                .is_some_and(|s| s.forks.len() == 1 && s.forks[0].task_id == second.as_str()),
            "the source is published without the deleted fork: {published:?}"
        );
        let mut events = app.events.subscribe();
        app.delete_task(&ctx, source_id.as_str()).await?;
        let published = summaries_of(&mut events, &second);
        assert!(
            published
                .last()
                .and_then(|s| s.forked_from.as_ref())
                .is_some_and(|origin| origin.task_id.is_none() && origin.number == source.number),
            "the fork is published with its source gone: {published:?}"
        );
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
