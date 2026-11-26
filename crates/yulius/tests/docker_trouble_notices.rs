//! While a sandbox's Docker front is full (a `docker` connection trouble), each
//! live task of the sandbox is told once in its session not to ask a person, and once the
//! trouble stayed ended, the tasks told hear that it is no longer full. A fake agent echoes what it is
//! typed.

use crate::support::live::UNSERVED;
use crate::support::live::pool::{pool, pool_live};
use std::time::Duration;

use yulius_app::docker_trouble::{DOCKER_BACK, DOCKER_FULL};
use yulius_domain::connection_health::{Limit, Refusal};

use crate::support::eventually;
use crate::support::live::pool::{new_task, runtime, screen, seed, stop, temp};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Passes until one types something (bounded).
async fn until_typed(
    app: &std::sync::Arc<yulius_app::context::App>,
) -> Result<Vec<(yulius_domain::ids::TaskId, &'static str)>, Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let typed = app.tell_docker_troubles().await;
        if !typed.is_empty() {
            return Ok(typed);
        }
        if std::time::Instant::now() > deadline {
            return Err("no notice was typed".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Told once per trouble, then told it is no longer full; another sandbox's trouble says nothing.
#[test]
fn a_sandboxs_tasks_hear_of_its_docker_trouble_once_and_of_its_end() -> TestResult {
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = pool(pool_live(&fake, agent), &data, UNSERVED, |config| {
            config.goal_ready_ms = 2_000
        })?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Docker user").await?;
        eventually("the first answer", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("ECHO: Docker user: answer with an echo") }
        })
        .await?;
        let health = app.connection_health();
        let full = |sandbox: &str| Refusal {
            limit: Limit::Docker,
            sandbox: Some(sandbox.to_owned()),
            held: 256,
            max: 256,
            holders: Vec::new(),
        };
        // Another sandbox's trouble is not this task's.
        health.refused_times(full("another"), 1, 1);
        assert!(app.tell_docker_troubles().await.is_empty());
        // A person must act first (an open Needs you): nothing is typed, the paste would answer
        // it, also once the session takes typed text (its start window is past here).
        let asking = yulius_app::needs_you::NeedsYouRecord {
            action: "Answer the question".into(),
            happened: None,
            problem: None,
            decision: false,
            link: None,
            at: 1,
            cause: None,
            named: Default::default(),
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asking), yulius_domain::time::Timestamp(1))
            .await?;
        health.refused_times(full(sandbox.as_str()), 3, 2);
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            assert!(
                app.tell_docker_troubles().await.is_empty(),
                "typed over a Needs you"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        app.deps
            .store
            .set_task_needs_you(&id, None, yulius_domain::time::Timestamp(2))
            .await?;
        let typed = until_typed(&app).await?;
        assert_eq!(typed, vec![(id.clone(), DOCKER_FULL)]);
        eventually("the notice echoed", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("do not raise a Needs you about it") }
        })
        .await?;
        // Within the same trouble, more refusals and passes tell nothing more.
        health.refused_times(full(sandbox.as_str()), 5, 3);
        assert!(app.tell_docker_troubles().await.is_empty());
        // It ends and begins again before the task heard it ended: one trouble for the task.
        health.held(Limit::Docker, Some(sandbox.as_str()), 10);
        assert!(app.tell_docker_troubles().await.is_empty());
        health.refused_times(full(sandbox.as_str()), 1, 4);
        assert!(app.tell_docker_troubles().await.is_empty());
        // Room again: after the trouble stayed ended for BACK_AFTER passes, the task told hears
        // it once, and not while a person must act first (an open Needs you): the paste would
        // answer it, so it hears it once that is answered.
        let asking = yulius_app::needs_you::NeedsYouRecord {
            action: "Answer the question".into(),
            happened: None,
            problem: None,
            decision: false,
            link: None,
            at: 1,
            cause: None,
            named: Default::default(),
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asking), yulius_domain::time::Timestamp(1))
            .await?;
        health.held(Limit::Docker, Some(sandbox.as_str()), 10);
        for _ in 0..=yulius_app::docker_trouble::BACK_AFTER {
            assert!(
                app.tell_docker_troubles().await.is_empty(),
                "typed over a Needs you"
            );
        }
        app.deps
            .store
            .set_task_needs_you(&id, None, yulius_domain::time::Timestamp(2))
            .await?;
        let mut heard = Vec::new();
        for _ in 0..yulius_app::docker_trouble::BACK_AFTER {
            heard.extend(app.tell_docker_troubles().await);
        }
        assert_eq!(heard, vec![(id.clone(), DOCKER_BACK)]);
        assert!(app.tell_docker_troubles().await.is_empty());
        eventually("the end echoed", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("no longer full") }
        })
        .await?;
        stop(&app, &id).await?;
        Ok(())
    })
}

/// Requests about the sandbox's Docker are one shared problem. A person's
/// answer to one task (a message through the Superagent) answers the others asked before and
/// tells them so; the end of the Docker trouble (Yulius's repair) answers those asked before its
/// end, and they hear that Docker works again. A request without the cause, and one asked after
/// the end, stay open.
#[test]
fn requests_naming_the_sandboxs_docker_are_answered_together() -> TestResult {
    use yulius_app::docker_trouble::DOCKER_ANSWERED;
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = pool(pool_live(&fake, agent), &data, UNSERVED, |config| {
            config.goal_ready_ms = 2_000
        })?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let first = new_task(&app, &sandbox, &user, "First").await?;
        let second = new_task(&app, &sandbox, &user, "Second").await?;
        let other = new_task(&app, &sandbox, &user, "Other").await?;
        for (id, title) in [(&first, "First"), (&second, "Second"), (&other, "Other")] {
            eventually("the first answer", Duration::from_secs(30), || {
                let shown = screen(&app, id);
                let expected = format!("ECHO: {title}: answer with an echo");
                async move { shown.contains(&expected) }
            })
            .await?;
        }
        let request =
            |action: &str, at: i64, cause: Option<&str>| yulius_app::needs_you::NeedsYouRecord {
                action: action.into(),
                happened: None,
                problem: None,
                decision: false,
                link: None,
                at,
                cause: cause.map(str::to_owned),
                named: Default::default(),
            };
        let ask = |id: &yulius_domain::ids::TaskId, at: i64, cause: Option<&'static str>| {
            let app = std::sync::Arc::clone(&app);
            let id = id.clone();
            async move {
                app.deps
                    .store
                    .set_task_needs_you(
                        &id,
                        Some(&request("Restart Docker", at, cause)),
                        yulius_domain::time::Timestamp(at),
                    )
                    .await
            }
        };
        let open = |id: &yulius_domain::ids::TaskId| {
            let app = std::sync::Arc::clone(&app);
            let id = id.clone();
            async move {
                app.deps
                    .store
                    .task_needs_you(&id)
                    .await
                    .map(|r| r.is_some())
            }
        };
        // Passes until each of `ids` heard `text` once (sessions in their start window or behind
        // a request of their own are told at a later pass).
        let hear = |ids: Vec<yulius_domain::ids::TaskId>, text: &'static str| {
            let app = std::sync::Arc::clone(&app);
            async move {
                let deadline = std::time::Instant::now() + Duration::from_secs(30);
                let mut heard = Vec::new();
                while ids.iter().any(|id| !heard.contains(&(id.clone(), text)))
                    && std::time::Instant::now() < deadline
                {
                    heard.extend(app.tell_docker_troubles().await);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                heard
            }
        };
        let now = app.now().0;
        ask(&first, now - 2_000, Some("docker")).await?;
        ask(&second, now - 1_000, Some("docker")).await?;
        ask(&other, now - 1_000, None).await?;
        // One episode for both, named by the oldest request (one notification).
        let episode = |id: &yulius_domain::ids::TaskId| {
            let app = std::sync::Arc::clone(&app);
            let id = id.clone();
            async move {
                let task = app.deps.store.task(&id).await?.ok_or("task")?;
                let summary = app.task_summary(&task).await?;
                Ok::<_, Box<dyn std::error::Error>>(summary.needs_you.and_then(|n| n.episode))
            }
        };
        // A stopping task's older request is no longer shown: it names no episode (review
        // round 1 of slices 5 to 7).
        let stopping = {
            let mut record = app.deps.store.task(&first).await?.ok_or("first")?;
            record.id = yulius_domain::ids::TaskId::parse("t-stopping")?;
            record.number = 0;
            record.branch = None;
            record.worktree_path = None;
            record.status = yulius_domain::task::TaskStatus::Stopping;
            app.deps.store.insert_task(&record).await?.id
        };
        ask(&stopping, now - 5_000, Some("docker")).await?;
        let first_episode = episode(&first).await?;
        let first_asked = {
            let task = app.deps.store.task(&first).await?.ok_or("first")?;
            app.task_summary(&task)
                .await?
                .needs_you
                .map(|request| request.at)
        };
        assert_eq!(first_episode, first_asked, "the oldest shown request");
        assert_eq!(episode(&second).await?, first_episode);
        assert_eq!(episode(&other).await?, None, "no cause, no episode");

        // A person answers the first through the Superagent: the second is answered and told.
        let caller = yulius_app::coordination::ProjectCaller {
            sandbox: sandbox.clone(),
            user: user.clone(),
            task: None,
            channel: yulius_app::coordination::Channel::Superagent,
            actor: "cockpit:admin".into(),
            detail: serde_json::json!({"via": "cockpit"}),
        };
        // A session in its start window takes no message yet: sent again until it does.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let sent = loop {
            match app
                .tool_task_message(
                    &caller,
                    &serde_json::json!({"task_id": first.as_str(), "text": "Docker is fine now"}),
                )
                .await
            {
                Ok(sent) => break sent,
                Err(yulius_app::error::AppError::Conflict(_))
                    if std::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(err) => return Err(err.into()),
            }
        };
        assert_eq!(sent["delivered"], "sent", "{sent}");
        assert!(
            !open(&first).await? && !open(&second).await?,
            "both answered"
        );
        assert!(open(&other).await?, "a request without the cause stays");
        // The request ended through another is audited with its source; the one answered
        // directly is not.
        let audited = |id: &yulius_domain::ids::TaskId| {
            let app = std::sync::Arc::clone(&app);
            let id = id.clone();
            async move {
                app.deps
                    .store
                    .audit_details(id.as_str(), "task.needs_you.answered_shared", 10)
                    .await
            }
        };
        let rows = audited(&second).await?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(
            rows[0].starts_with("docker: a person's answer to task #"),
            "{rows:?}"
        );
        assert!(audited(&first).await?.is_empty());

        let heard = hear(vec![second.clone()], DOCKER_ANSWERED).await;
        assert_eq!(
            heard.iter().filter(|(id, _)| *id == second).count(),
            1,
            "{heard:?}"
        );
        // Asked again; the sandbox's Docker trouble begins and ends (a repair): both are answered
        // and hear that Docker works again; a request asked after the end stays.
        let now = app.now().0;
        ask(&first, now - 2_000, Some("docker")).await?;
        ask(&second, now - 1_000, Some("docker")).await?;
        let health = app.connection_health();
        health.refused_times(
            Refusal {
                limit: Limit::Docker,
                sandbox: Some(sandbox.as_str().to_owned()),
                held: 256,
                max: 256,
                holders: Vec::new(),
            },
            1,
            5,
        );
        app.tell_docker_troubles().await;
        health.held(Limit::Docker, Some(sandbox.as_str()), 0);
        app.tell_docker_troubles().await;
        ask(&other, app.now().0 + 60_000, Some("docker")).await?;
        let heard = hear(vec![first.clone(), second.clone()], DOCKER_BACK).await;
        assert!(
            !open(&first).await? && !open(&second).await?,
            "the repair answered both"
        );
        assert!(open(&other).await?, "asked after the end: stays");
        let back: Vec<_> = heard
            .iter()
            .filter(|(_, text)| *text == DOCKER_BACK)
            .map(|(id, _)| id.clone())
            .collect();
        assert!(
            back.contains(&first) && back.contains(&second) && back.len() == 2,
            "{heard:?}"
        );
        stop(&app, &first).await?;
        stop(&app, &second).await?;
        stop(&app, &other).await?;
        Ok(())
    })
}

/// Slice 4 review round 2: a person continuing a stopped task whose stored request names the
/// sandbox's Docker answers no other task's request: that request was not shown.
#[test]
fn continuing_a_stopped_task_answers_no_other_tasks_request() -> TestResult {
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = pool(pool_live(&fake, agent), &data, UNSERVED, |config| {
            config.goal_ready_ms = 2_000
        })?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let stopped = new_task(&app, &sandbox, &user, "Stopped").await?;
        let working = new_task(&app, &sandbox, &user, "Working").await?;
        for (id, title) in [(&stopped, "Stopped"), (&working, "Working")] {
            eventually("the first answer", Duration::from_secs(30), || {
                let shown = screen(&app, id);
                let expected = format!("ECHO: {title}: answer with an echo");
                async move { shown.contains(&expected) }
            })
            .await?;
        }
        stop(&app, &stopped).await?;
        let docker = |at: i64| yulius_app::needs_you::NeedsYouRecord {
            action: "Restart Docker".into(),
            happened: None,
            problem: None,
            decision: false,
            link: None,
            at,
            cause: Some("docker".into()),
            named: Default::default(),
        };
        let now = app.now().0;
        for (id, at) in [(&stopped, now - 2_000), (&working, now - 1_000)] {
            app.deps
                .store
                .set_task_needs_you(id, Some(&docker(at)), yulius_domain::time::Timestamp(at))
                .await?;
        }
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: Some("csrf".into()),
            session_hash: None,
            token: None,
        };
        let req: yulius_contracts::api::tasks::ContinueTask =
            serde_json::from_value(serde_json::json!({}))?;
        app.continue_task(&ctx, stopped.as_str(), &req).await?;
        assert!(app.deps.store.task_needs_you(&stopped).await?.is_none());
        assert!(
            app.deps.store.task_needs_you(&working).await?.is_some(),
            "a stopped task's old request answers no other"
        );
        stop(&app, &stopped).await?;
        stop(&app, &working).await?;
        Ok(())
    })
}

/// SPEC-512 ANS-004 with DFJ-011 (review round 3 of part 2): an answer on the task's page to a
/// question about the sandbox's Docker answers the sandbox's other tasks that asked it before.
#[test]
fn a_page_answer_to_a_shared_docker_question_answers_the_others() -> TestResult {
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = pool(pool_live(&fake, agent), &data, UNSERVED, |config| {
            config.goal_ready_ms = 2_000
        })?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let first = new_task(&app, &sandbox, &user, "First").await?;
        let second = new_task(&app, &sandbox, &user, "Second").await?;
        for (id, title) in [(&first, "First"), (&second, "Second")] {
            eventually("the first answer", Duration::from_secs(30), || {
                let shown = screen(&app, id);
                let expected = format!("ECHO: {title}: answer with an echo");
                async move { shown.contains(&expected) }
            })
            .await?;
        }
        let now = app.now().0;
        for (id, at) in [(&first, now - 2_000), (&second, now - 1_000)] {
            app.deps
                .store
                .set_task_needs_you(
                    id,
                    Some(&yulius_app::needs_you::NeedsYouRecord {
                        action: "Restart Docker".into(),
                        happened: None,
                        problem: None,
                        decision: false,
                        link: None,
                        at,
                        cause: Some("docker".into()),
                        named: Default::default(),
                    }),
                    yulius_domain::time::Timestamp(at),
                )
                .await?;
        }
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        // A session in its start window takes no typed text yet: answered again until it does.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match app
                .answer_needs_you(&ctx, first.as_str(), "Docker is fine now", None)
                .await
            {
                Ok(_) => break,
                Err(yulius_app::error::AppError::Conflict(_))
                    if std::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
        assert!(app.deps.store.task_needs_you(&first).await?.is_none());
        assert!(
            app.deps.store.task_needs_you(&second).await?.is_none(),
            "the shared question is answered for the sandbox"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
