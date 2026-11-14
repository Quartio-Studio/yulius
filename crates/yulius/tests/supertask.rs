//! Supertasks (SPEC-071, SPEC-001 TSK-018): a task that waits for the running work of its
//! project, then runs alone while every other task of the project waits for it. The scenario
//! with real processes is in `supervision.rs`; these tests use the in-memory harness, where a
//! start that passes the hold fails at once (no Git, no session host), so a task the hold keeps
//! stays `queued`.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, mcp_tool, perms,
    seed_live_attempt, seed_task, tool_text, user_with_token,
};
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::{AttemptUpdate, TaskStore};
use yulius_app::supervisor::RunStart;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};

type Error = Box<dyn std::error::Error>;

const CREATE: &str = "/api/sandboxes/s1/tasks";

/// Sandbox `s1` (studio) where alice may write; her task `t-agent` runs (a live attempt, task
/// token `tok-agent`).
async fn world(max_live_sessions: u32) -> Result<(Harness, Creds), Error> {
    let h = harness_configured(move |c| c.max_live_sessions = max_live_sessions)?;
    support::exhausted_subscription(&h).await?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    seed_task(&h, "t-agent", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-agent", "tok-agent").await?;
    Ok((h, alice))
}

async fn post(h: &Harness, creds: &Creds, body: Value) -> Result<support::Resp, Error> {
    call(&h.router, Method::POST, CREATE, Some(body), creds).await
}

async fn summary(h: &Harness, creds: &Creds, id: &str) -> Result<Value, Error> {
    Ok(call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{id}"),
        None,
        creds,
    )
    .await?
    .body)
}

/// AC-SUP-1: New task and `task_create` make a supertask; it cannot also run after a task or a
/// rule (422, nothing created).
#[tokio::test]
async fn a_task_is_created_as_a_supertask() -> TestResult {
    let (h, alice) = world(0).await?;
    let r = post(
        &h,
        &alice,
        json!({"prompt": "Harden the whole app", "supertask": true}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["supertask"], json!(true), "{:?}", r.body);
    let id = r.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(summary(&h, &alice, &id).await?["supertask"], json!(true));
    let stored = h.store.task(&TaskId::parse(&id)?).await?.ok_or("task")?;
    assert!(stored.supertask);

    // A plain task is not one.
    let r = post(&h, &alice, json!({"prompt": "Small fix"})).await?;
    assert_eq!(r.body["supertask"], json!(false), "{:?}", r.body);

    // Not together with a chain or a rule.
    let before = h.store.tasks(&SandboxId::parse("s1")?, false).await?.len();
    for body in [
        json!({"prompt": "x", "supertask": true, "after_task_id": "t-agent"}),
        json!({"prompt": "x", "supertask": true, "start_rule": "#1 done"}),
    ] {
        let r = post(&h, &alice, body.clone()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {:?}",
            r.body
        );
        assert!(
            r.body.to_string().contains("supertask"),
            "{body}: {:?}",
            r.body
        );
    }
    assert_eq!(
        h.store.tasks(&SandboxId::parse("s1")?, false).await?.len(),
        before
    );

    // An agent creates one for its creator.
    let result = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "Security hardening", "supertask": true}),
    )
    .await?;
    let created = tool_text(&result);
    assert_eq!(result["isError"], json!(false), "{created}");
    let id = TaskId::parse(created["task"]["task_id"].as_str().ok_or("task_id")?)?;
    assert!(h.store.task(&id).await?.ok_or("task")?.supertask);
    let refused = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "x", "supertask": "yes"}),
    )
    .await?;
    assert_eq!(refused["isError"], json!(true), "{refused}");
    Ok(())
}

async fn create(
    h: &Harness,
    creds: &Creds,
    sandbox: &str,
    body: Value,
) -> Result<TaskRecord, Error> {
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/tasks"),
        Some(body),
        creds,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let id = TaskId::parse(r.body["id"].as_str().ok_or("id")?)?;
    Ok(h.store.task(&id).await?.ok_or("task")?)
}

/// Offers `task` a session slot as the queue does; the in-memory harness cannot launch, so a
/// task the hold lets through leaves `queued` (it fails to start).
async fn offer(h: &Harness, task: &TaskRecord) -> Result<TaskStatus, Error> {
    let _ = h
        .app
        .start_run(&task.id, RunStart::Fresh { prompt: None }, None)
        .await;
    Ok(h.store.task(&task.id).await?.ok_or("task")?.status)
}

async fn held_by(h: &Harness, creds: &Creds, task: &TaskRecord) -> Result<Value, Error> {
    Ok(summary(h, creds, task.id.as_str()).await?["held_by"].clone())
}

/// AC-SUP-2..4 (the rule, SUP-002, SUP-003, SUP-005): a supertask waits for the project's
/// working task; every other start of the project waits for the supertask (a new task, a
/// continued one, Start now on a chained one); another project is not held; once the working
/// task is Done the supertask goes, and once it ended the others go.
#[tokio::test]
async fn a_supertask_waits_for_working_tasks_and_holds_its_project() -> TestResult {
    let (h, alice) = world(8).await?;
    add_sandbox(&h, "s2", "other", 20100).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/other", perms("write"));
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    let closed = seed_task(&h, "t-closed", "s1", "u-alice", TaskStatus::Closed, None).await?;

    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    assert_eq!(
        offer(&h, &s).await?,
        TaskStatus::Queued,
        "waits for #{}",
        agent.number
    );
    assert_eq!(held_by(&h, &alice, &s).await?, json!([agent.number]));

    let b = create(&h, &alice, "s1", json!({"prompt": "Small fix"})).await?;
    assert_eq!(
        offer(&h, &b).await?,
        TaskStatus::Queued,
        "waits for the supertask"
    );
    assert_eq!(held_by(&h, &alice, &b).await?, json!([s.number]));

    // Continue of an ended task and Start now of a chained one wait too.
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{}/continue", closed.id),
        Some(json!({})),
        &alice,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    assert_eq!(r.body["status"], "queued", "{:?}", r.body);
    assert_eq!(held_by(&h, &alice, &closed).await?, json!([s.number]));
    let chained = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "After the agent", "after_task_id": "t-agent"}),
    )
    .await?;
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{}/start-now", chained.id),
        None,
        &alice,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    assert_eq!(
        summary(&h, &alice, chained.id.as_str()).await?["status"],
        "queued"
    );
    assert_eq!(held_by(&h, &alice, &chained).await?, json!([s.number]));

    // Another project is not held.
    let other = create(&h, &alice, "s2", json!({"prompt": "Elsewhere"})).await?;
    assert_ne!(offer(&h, &other).await?, TaskStatus::Queued);
    assert_eq!(held_by(&h, &alice, &other).await?, json!([]));

    // The agent is Done: the supertask goes (a Done task holds nothing), the others still wait.
    h.store
        .set_activity(&agent.id, Some(Activity::Done), h.app.now())
        .await?;
    assert_eq!(held_by(&h, &alice, &b).await?, json!([s.number]));
    assert_ne!(
        offer(&h, &s).await?,
        TaskStatus::Queued,
        "the supertask starts"
    );
    // It ended (here: it could not start): the hold is over.
    assert_ne!(offer(&h, &b).await?, TaskStatus::Queued, "the others start");
    Ok(())
}

/// SUP-003: supertasks of one project go one at a time, by task number; a supertask without
/// working tasks starts at once.
#[tokio::test]
async fn supertasks_go_one_at_a_time() -> TestResult {
    let (h, alice) = world(8).await?;
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    let first = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "One", "supertask": true}),
    )
    .await?;
    let second = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Two", "supertask": true}),
    )
    .await?;
    assert_eq!(held_by(&h, &alice, &first).await?, json!([agent.number]));
    assert_eq!(
        held_by(&h, &alice, &second).await?,
        json!([agent.number, first.number])
    );
    // The agent's run ends.
    h.store
        .transition_task(
            &agent.id,
            None,
            TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled),
            None,
            h.app.now(),
        )
        .await?;
    end_attempt(&h, "t-agent").await?;
    assert_eq!(held_by(&h, &alice, &first).await?, json!([]));
    assert_eq!(held_by(&h, &alice, &second).await?, json!([first.number]));
    assert_eq!(offer(&h, &second).await?, TaskStatus::Queued);
    assert_ne!(
        offer(&h, &first).await?,
        TaskStatus::Queued,
        "no working task: it starts"
    );
    Ok(())
}

async fn end_attempt(h: &Harness, task: &str) -> TestResult {
    h.store
        .transition_attempt(
            &AttemptId::parse(&format!("a-{task}"))?,
            AttemptState::Exited,
            &AttemptUpdate {
                ended_at: Some(h.app.now()),
                ..AttemptUpdate::default()
            },
        )
        .await?;
    Ok(())
}

/// SPEC-483 QUE-001 (#1278): stopping one of two waiting tasks, neither a supertask, in a world
/// with free sessions runs no pass of the queue: the other stays waiting (a pass would start it
/// before a bulk stop reached it). The check is bounded: it can only miss the pass, never fail
/// without one (this harness has no background tick).
#[tokio::test]
async fn stopping_a_waiting_task_that_is_no_supertask_starts_no_other() -> TestResult {
    let h = support::harness()?;
    support::seed_sandbox(&h, "s1", "shop", "u-alice").await?;
    let first = support::seed_task(&h, "t-q1", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let second = support::seed_task(&h, "t-q2", "s1", "u-alice", TaskStatus::Queued, None).await?;
    h.app.stop_task_now(&first.id).await?;
    let until = tokio::time::Instant::now() + std::time::Duration::from_millis(1000);
    while tokio::time::Instant::now() < until {
        let status = h.store.task(&second.id).await?.ok_or("task")?.status;
        assert_eq!(status, TaskStatus::Queued, "the second task left the queue");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

/// SUP-005 (review): a queued supertask stopped while slots are free lets the tasks it held start
/// without waiting for the next tick of the queue.
#[tokio::test]
async fn stopping_a_queued_supertask_starts_the_tasks_it_held() -> TestResult {
    let (h, alice) = world(8).await?;
    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    let b = create(&h, &alice, "s1", json!({"prompt": "Small fix"})).await?;
    assert_eq!(offer(&h, &b).await?, TaskStatus::Queued);
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{}/stop", s.id),
        None,
        &alice,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    wait_until("the held task to be offered a slot", || async {
        h.store
            .task(&b.id)
            .await
            .is_ok_and(|t| t.is_some_and(|t| t.status != TaskStatus::Queued))
    })
    .await
}

/// SUP-006 (review): at the session limit, a new supertask relabels the project's queued tasks
/// at once: the queue publishes them with what they wait for.
#[tokio::test]
async fn a_new_supertask_relabels_the_queue_at_the_session_limit() -> TestResult {
    let (h, alice) = world(1).await?;
    let b = create(&h, &alice, "s1", json!({"prompt": "Small fix"})).await?;
    assert_eq!(held_by(&h, &alice, &b).await?, json!([]));
    let mut events = h.app.events.subscribe();
    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let envelope = tokio::time::timeout_at(deadline, events.recv()).await??;
        if let UiEvent::TaskUpdated { task } = envelope.event
            && task.id == b.id.as_str()
            && task.held_by == vec![s.number]
        {
            return Ok(());
        }
    }
}

/// A second start of a task that already runs (the queue and a direct start racing) is a no-op,
/// not an error that tells the terminal "Could not start".
#[tokio::test]
async fn a_running_task_is_not_started_twice() -> TestResult {
    let (h, _alice) = world(8).await?;
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    h.app
        .start_run(&agent.id, RunStart::Fresh { prompt: None }, None)
        .await?;
    assert_eq!(
        h.store.task(&agent.id).await?.ok_or("agent")?.status,
        TaskStatus::Running
    );
    Ok(())
}

async fn wait_until<F, Fut>(what: &str, mut check: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if check().await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

/// SUP-002 (review round 2): a task paused for capacity before it ran keeps its pause, with its
/// kind and continuation, while a supertask holds its project; once the hold ends the resume
/// loop starts it.
#[tokio::test]
async fn a_held_capacity_resume_keeps_its_pause() -> TestResult {
    use yulius_app::model::{AccountRecord, AccountStatus, PauseKind, PauseRecord};
    use yulius_app::ports::store::AccountStore;
    use yulius_app::ports::store_ext::CapacityStore;
    let (h, alice) = world(8).await?;
    h.store
        .insert_account(&AccountRecord {
            id: yulius_domain::ids::AccountId::parse("acc-1")?,
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "acc-1".into(),
            credential: Some(h.app.deps.crypto.seal(b"oauth-token")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: yulius_domain::time::Timestamp(1),
            updated_at: yulius_domain::time::Timestamp(1),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    let paused = seed_task(
        &h,
        "t-paused",
        "s1",
        "u-alice",
        TaskStatus::WaitingCapacity,
        None,
    )
    .await?;
    h.store
        .insert_pause(&PauseRecord {
            id: "p-1".into(),
            task_id: paused.id.clone(),
            kind: PauseKind::Limit,
            account_id: None,
            continuation: Some("go on with the migration".into()),
            started_at: h.app.now(),
            ended_at: None,
            outcome: None,
        })
        .await?;

    let _ = h.app.resume_round().await;
    // The resume loop: its first round leaves the task held, then it sleeps (5 minutes) until it
    // is woken. The queue wakes it when the hold ends.
    h.app.start_capacity_loop();
    assert_eq!(
        h.store.task(&paused.id).await?.ok_or("task")?.status,
        TaskStatus::WaitingCapacity
    );
    let pause = h
        .store
        .open_pause(&paused.id)
        .await?
        .ok_or("the pause stays")?;
    assert_eq!(pause.kind, PauseKind::Limit);
    assert_eq!(
        pause.continuation.as_deref(),
        Some("go on with the migration")
    );

    // The supertask is deleted: the hold is over and the loop resumes the task.
    let r = call(
        &h.router,
        Method::DELETE,
        &format!("/api/tasks/{}", s.id),
        None,
        &alice,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    wait_until("the resume loop to resume the task", || async {
        h.store
            .open_pause(&paused.id)
            .await
            .is_ok_and(|pause| pause.is_none())
    })
    .await
}

/// SUP-005 (review round 2): deleting a queued supertask, or a supertask that fails to start,
/// lets the tasks it held go without waiting for the next tick of the queue.
#[tokio::test]
async fn a_deleted_or_failed_supertask_releases_its_project() -> TestResult {
    let (h, alice) = world(8).await?;
    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    let b = create(&h, &alice, "s1", json!({"prompt": "Small fix"})).await?;
    assert_eq!(offer(&h, &b).await?, TaskStatus::Queued);
    let r = call(
        &h.router,
        Method::DELETE,
        &format!("/api/tasks/{}", s.id),
        None,
        &alice,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    wait_until("the task the deleted supertask held to go", || async {
        h.store
            .task(&b.id)
            .await
            .is_ok_and(|t| t.is_some_and(|t| t.status != TaskStatus::Queued))
    })
    .await?;

    // A supertask whose start fails (here: no Git) releases the next one at once.
    let (h, alice) = world(8).await?;
    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    let c = create(&h, &alice, "s1", json!({"prompt": "Small fix"})).await?;
    assert_eq!(offer(&h, &c).await?, TaskStatus::Queued);
    // The agent's run ends: the supertask is offered a slot and fails to start.
    h.store
        .set_activity(
            &TaskId::parse("t-agent")?,
            Some(Activity::Done),
            h.app.now(),
        )
        .await?;
    assert_ne!(offer(&h, &s).await?, TaskStatus::Queued);
    wait_until("the task the failed supertask held to go", || async {
        h.store
            .task(&c.id)
            .await
            .is_ok_and(|t| t.is_some_and(|t| t.status != TaskStatus::Queued))
    })
    .await
}

/// SPEC-487 CHN-002 (review round 1, M3): a task whose pull request is merged while its agent
/// has not heard of it goes on by itself. A supertask does not take it for finished, and Home
/// counts it as working. A task the watch never saw with its pull request open is told
/// nothing, and is Done as before.
#[tokio::test]
async fn a_supertask_waits_for_a_task_that_has_not_heard_of_its_merge() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::ports::store_ext::DeliveryStore;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::delivery::DeliveryStage;

    let (h, alice) = world(8).await?;
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    let unseen = seed_task(&h, "t-unseen", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-unseen", "tok-unseen").await?;
    for task in [&agent, &unseen] {
        h.store
            .set_activity(&task.id, Some(Activity::Waiting), h.app.now())
            .await?;
    }
    let open = |task: &TaskRecord, pr| DeliveryRecord {
        task_id: task.id.clone(),
        stage: DeliveryStage::PrOpen,
        pr_number: Some(pr),
        updated_at: h.app.now(),
    };
    // The watch sees the agent's pull request open (its run is known to this server).
    h.store
        .transition_delivery(&DeliveryStage::None, &open(&agent, 7))
        .await?;
    // (Reading the task makes this server know its run, as the watch needs.)
    summary(&h, &alice, agent.id.as_str()).await?;
    assert!(h.app.watch_prs(&WatchBounds::default()).await.is_empty());
    // Both are merged; the other one's pull request the watch never saw open.
    h.store
        .transition_delivery(&DeliveryStage::None, &open(&unseen, 8))
        .await?;
    for (task, pr) in [(&agent, 7), (&unseen, 8)] {
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            ..open(task, pr)
        };
        h.store
            .transition_delivery(&DeliveryStage::PrOpen, &merged)
            .await?;
    }

    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    assert_eq!(offer(&h, &s).await?, TaskStatus::Queued);
    assert_eq!(
        held_by(&h, &alice, &s).await?,
        json!([agent.number]),
        "the one that will hear of its merge and go on, not the one that is Done"
    );
    assert_eq!(
        summary(&h, &alice, agent.id.as_str()).await?["yulius_wait"],
        true
    );
    assert_eq!(
        summary(&h, &alice, unseen.id.as_str()).await?["yulius_wait"],
        false
    );
    let sandbox = call(&h.router, Method::GET, "/api/sandboxes/s1", None, &alice).await?;
    assert_eq!(
        (
            &sandbox.body["counts"]["running"],
            &sandbox.body["counts"]["queued"]
        ),
        (&json!(1), &json!(1)),
        "{}",
        sandbox.body
    );
    Ok(())
}

/// SPEC-487 CHN-002 (review round 2, R2-2): an update's hold ends by itself and the watch tells
/// the task right after, so a merge that lands while an update holds the runs is still news
/// due: the task is not Done, and a task chained after it keeps waiting.
#[tokio::test]
async fn a_merge_during_an_updates_hold_is_still_news_due() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::ports::store_ext::{DeliveryStore, UpdateStore};
    use yulius_app::pr_watch::WatchBounds;
    use yulius_app::updates::UpdateRunRecord;
    use yulius_domain::delivery::DeliveryStage;
    use yulius_domain::time::Timestamp;
    use yulius_domain::update::RunState;

    let (h, alice) = world(8).await?;
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    h.store
        .set_activity(&agent.id, Some(Activity::Waiting), h.app.now())
        .await?;
    let open = DeliveryRecord {
        task_id: agent.id.clone(),
        stage: DeliveryStage::PrOpen,
        pr_number: Some(7),
        updated_at: h.app.now(),
    };
    h.store
        .transition_delivery(&DeliveryStage::None, &open)
        .await?;
    // (Reading the task makes this server know its run, as the watch needs.)
    summary(&h, &alice, agent.id.as_str()).await?;
    assert!(h.app.watch_prs(&WatchBounds::default()).await.is_empty());
    let next = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "After the agent", "after_task_id": agent.id.as_str()}),
    )
    .await?;
    assert_eq!(next.status, TaskStatus::Chained);
    // An update drains the instance, and the merge queue merges meanwhile.
    let now = Timestamp(1);
    h.store
        .insert_update_run(&UpdateRunRecord {
            id: "update-1".into(),
            target_commit: "b".repeat(40),
            from_commit: None,
            summary: "next".into(),
            state: RunState::Draining,
            outcome: None,
            error: None,
            requested_by: "admin".into(),
            created_at: now,
            state_since: now,
            finished_at: None,
        })
        .await?;
    h.app.restore_update_hold().await;
    let merged = DeliveryRecord {
        stage: DeliveryStage::MergedIntegration,
        ..open
    };
    h.store
        .transition_delivery(&DeliveryStage::PrOpen, &merged)
        .await?;
    assert!(
        h.app.watch_prs(&WatchBounds::default()).await.is_empty(),
        "held: nothing is typed"
    );
    h.app.advance_all_chains().await;
    assert_eq!(
        h.store.task(&next.id).await?.ok_or("next")?.status,
        TaskStatus::Chained
    );
    assert_eq!(
        summary(&h, &alice, agent.id.as_str()).await?["yulius_wait"],
        true
    );
    Ok(())
}
