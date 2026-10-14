//! An automation starts its tasks like New task (SPEC-078, SPEC-001 AUT-013): start now (queued),
//! as a supertask, after a task or when a start rule is true. The in-memory harness caps live
//! sessions at 0, so a created task stays where its start choice put it.

use crate::support;

use std::sync::atomic::Ordering;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, mcp_tool, perms,
    seed_live_attempt, seed_task, tool_text, user_with_token,
};
use yulius_app::automations::run_key;
use yulius_app::delivery::DeliveryRecord;
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::TaskStore;
use yulius_app::ports::store_ext::{AutomationStore, DeliveryStore, IntakeStore};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

type Error = Box<dyn std::error::Error>;

const CREATE: &str = "/api/sandboxes/s1/automations";
const HOUR: i64 = 3_600_000;
/// The harness clock starts at 2026-10-07T00:00:00Z.
const BASE: i64 = 1_791_331_200_000;

/// Sandbox `s1` (studio) where alice may write, with task #1 `t-one` running; sandbox `s2` has
/// task `t-foreign`.
async fn world() -> Result<(Harness, Creds), Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    add_sandbox(&h, "s2", "other", 20010).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    seed_task(&h, "t-one", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_task(&h, "t-foreign", "s2", "u-alice", TaskStatus::Running, None).await?;
    Ok((h, alice))
}

fn nightly(extra: &Value) -> Value {
    let mut body =
        json!({"name": "Docs", "cron": "0 3 * * *", "prompt": "Keep the docs up to date"});
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    body
}

async fn send(
    h: &Harness,
    creds: &Creds,
    method: Method,
    path: &str,
    body: Value,
) -> Result<support::Resp, Error> {
    call(&h.router, method, path, Some(body), creds).await
}

async fn stored(h: &Harness, creds: &Creds, id: &str) -> Result<Value, Error> {
    let list = call(&h.router, Method::GET, CREATE, None, creds).await?;
    Ok(list
        .body
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == id).cloned())
        .ok_or("automation")?)
}

fn choice(view: &Value) -> (Value, Value, Value, Value) {
    (
        view["supertask"].clone(),
        view["after_task_id"].clone(),
        view["after_task_number"].clone(),
        view["start_rule"].clone(),
    )
}

/// AC-AUT-13 and AC-AUT-14: the start choice is stored, read back, changed and cleared; invalid
/// choices answer 422 and change nothing.
#[tokio::test]
async fn an_automation_stores_its_start_choice() -> TestResult {
    let (h, alice) = world().await?;
    h.clock.0.store(BASE, Ordering::SeqCst);

    // None given: Start now, as before.
    let plain = send(&h, &alice, Method::POST, CREATE, nightly(&json!({}))).await?;
    assert_eq!(plain.status, StatusCode::CREATED, "{}", plain.body);
    assert_eq!(
        choice(&plain.body),
        (json!(false), Value::Null, Value::Null, Value::Null)
    );

    let r = send(
        &h,
        &alice,
        Method::POST,
        CREATE,
        nightly(&json!({"supertask": true})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["supertask"], json!(true));

    let r = send(
        &h,
        &alice,
        Method::POST,
        CREATE,
        nightly(&json!({"after_task_id": "t-one"})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(
        choice(&r.body),
        (json!(false), json!("t-one"), json!(1), Value::Null)
    );

    // The rule is stored canonically.
    let r = send(
        &h,
        &alice,
        Method::POST,
        CREATE,
        nightly(&json!({"start_rule": "  #1   DONE "})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let ruled = r.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(r.body["start_rule"], json!("#1 done"));
    assert_eq!(
        stored(&h, &alice, &ruled).await?["start_rule"],
        json!("#1 done")
    );

    // PUT changes the choice, and clears it.
    let path = format!("/api/automations/{ruled}");
    let r = send(
        &h,
        &alice,
        Method::PUT,
        &path,
        nightly(&json!({"after_task_id": "t-one"})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        choice(&stored(&h, &alice, &ruled).await?),
        (json!(false), json!("t-one"), json!(1), Value::Null)
    );
    let r = send(&h, &alice, Method::PUT, &path, nightly(&json!({}))).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        choice(&stored(&h, &alice, &ruled).await?),
        (json!(false), Value::Null, Value::Null, Value::Null)
    );

    // Invalid choices: 422, nothing created or changed.
    let r = send(
        &h,
        &alice,
        Method::PUT,
        &path,
        nightly(&json!({"after_task_id": "t-one"})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let count = |v: &Value| v.as_array().map_or(0, Vec::len);
    let before = count(
        &call(&h.router, Method::GET, CREATE, None, &alice)
            .await?
            .body,
    );
    let invalid = [
        json!({"supertask": true, "after_task_id": "t-one"}),
        json!({"supertask": true, "start_rule": "#1 done"}),
        json!({"after_task_id": "t-one", "start_rule": "#1 done"}),
        json!({"after_task_id": "t-missing"}),
        json!({"after_task_id": "t-foreign"}),
        json!({"start_rule": "#1 done and"}),
        json!({"start_rule": "#9 done"}),
    ];
    for extra in &invalid {
        let r = send(&h, &alice, Method::POST, CREATE, nightly(extra)).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{extra}: {}",
            r.body
        );
        let r = send(&h, &alice, Method::PUT, &path, nightly(extra)).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{extra}: {}",
            r.body
        );
    }
    assert_eq!(
        count(
            &call(&h.router, Method::GET, CREATE, None, &alice)
                .await?
                .body
        ),
        before
    );
    assert_eq!(
        choice(&stored(&h, &alice, &ruled).await?),
        (json!(false), json!("t-one"), json!(1), Value::Null)
    );
    Ok(())
}

/// A stored choice whose task was deleted since does not block other changes: pausing the
/// automation (the list's switch sends the choice back) or renaming it still saves.
#[tokio::test]
async fn a_stored_choice_naming_a_deleted_task_still_saves() -> TestResult {
    let (h, alice) = world().await?;
    let gone = seed_task(&h, "t-gone", "s1", "u-alice", TaskStatus::Running, None).await?;
    let after = create(&h, &alice, &json!({"after_task_id": "t-gone"})).await?;
    let ruled = create(
        &h,
        &alice,
        &json!({"start_rule": format!("#{} done", gone.number)}),
    )
    .await?;
    assert!(h.store.delete_task(&gone.id).await?);

    let mut body = nightly(&json!({"after_task_id": "t-gone", "enabled": false}));
    let r = send(
        &h,
        &alice,
        Method::PUT,
        &format!("/api/automations/{after}"),
        body.clone(),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["enabled"], json!(false));
    assert_eq!(r.body["after_task_id"], json!("t-gone"));

    body = nightly(&json!({"start_rule": format!("#{} done", gone.number), "name": "Renamed"}));
    let r = send(
        &h,
        &alice,
        Method::PUT,
        &format!("/api/automations/{ruled}"),
        body,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["name"], json!("Renamed"));

    // A new choice is still checked.
    let r = send(
        &h,
        &alice,
        Method::PUT,
        &format!("/api/automations/{ruled}"),
        nightly(&json!({"after_task_id": "t-gone"})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    Ok(())
}

/// Marks a task Done by the sidebar's rule: closed with its pull request merged.
async fn merged_and_closed(h: &Harness, id: &str, at: i64) -> Result<(), Error> {
    let id = TaskId::parse(id)?;
    h.store
        .transition_delivery(
            &DeliveryStage::None,
            &DeliveryRecord {
                task_id: id.clone(),
                stage: DeliveryStage::MergedIntegration,
                pr_number: Some(7),
                updated_at: Timestamp(at),
            },
        )
        .await?;
    h.store
        .transition_task(&id, None, TaskStatus::Closed, None, Timestamp(at))
        .await?;
    Ok(())
}

/// AC-AUT-15: after a task that was Done and has ended, a run's task starts at once (the rule
/// `#N done` is true); a rule already true releases the task at once; a full chain bound fails
/// the run with the reason.
#[tokio::test]
async fn a_finished_predecessor_or_a_true_rule_starts_the_task_at_once() -> TestResult {
    let (h, alice) = world().await?;
    h.clock.0.store(BASE, Ordering::SeqCst);
    let after = create(&h, &alice, &json!({"after_task_id": "t-one"})).await?;
    let ruled = create(&h, &alice, &json!({"start_rule": "#1 merged"})).await?;
    merged_and_closed(&h, "t-one", BASE + HOUR).await?;
    h.clock.0.store(BASE + 2 * HOUR, Ordering::SeqCst);

    for automation in [&after, &ruled] {
        let run = run_now(&h, &alice, automation).await?;
        assert_eq!(run["state"], "task_created", "{run}");
        let task = h
            .store
            .task(&TaskId::parse(run["task_id"].as_str().ok_or("task")?)?)
            .await?
            .ok_or("task")?;
        assert_eq!(
            task.status,
            TaskStatus::Queued,
            "released at once: {automation}"
        );
    }

    // A full chain bound: the run fails and says why, and creates no task.
    for i in 0..32 {
        seed_task(
            &h,
            &format!("t-wait-{i}"),
            "s1",
            "u-alice",
            TaskStatus::Chained,
            None,
        )
        .await?;
    }
    let bound = create(&h, &alice, &json!({"start_rule": "#1 done"})).await?;
    let run = run_now(&h, &alice, &bound).await?;
    assert_eq!(run["state"], "failed", "{run}");
    assert!(
        run["error"]
            .as_str()
            .unwrap_or("")
            .contains("waiting in chains"),
        "{run}"
    );
    assert_eq!(run["task_id"], Value::Null);
    Ok(())
}

async fn create(h: &Harness, creds: &Creds, extra: &Value) -> Result<String, Error> {
    let r = send(h, creds, Method::POST, CREATE, nightly(extra)).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    Ok(r.body["id"].as_str().ok_or("id")?.to_owned())
}

/// The task the automation's run at `slot` created.
async fn run_task(h: &Harness, automation: &str, slot: i64) -> Result<TaskRecord, Error> {
    let id = h
        .store
        .task_by_automation_run(&run_key(automation, Timestamp(slot)))
        .await?
        .ok_or("no task for the run")?;
    Ok(h.store.task(&id).await?.ok_or("task")?)
}

async fn run_now(h: &Harness, creds: &Creds, automation: &str) -> Result<Value, Error> {
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/automations/{automation}/run"),
        None,
        creds,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// AC-AUT-15 and AC-AUT-16: each run creates its task with the automation's start choice; a
/// waiting task skips the next run; after a task that has ended, the task waits for `#N done`; a
/// task to run after that was deleted fails the run with the reason.
#[tokio::test]
async fn each_run_starts_its_task_with_the_start_choice() -> TestResult {
    let (h, alice) = world().await?;
    h.clock.0.store(BASE, Ordering::SeqCst);
    let now = create(&h, &alice, &json!({})).await?;
    let supertask = create(&h, &alice, &json!({"supertask": true})).await?;
    let ruled = create(&h, &alice, &json!({"start_rule": "#1 merged"})).await?;
    let after = create(&h, &alice, &json!({"after_task_id": "t-one"})).await?;

    let slot = BASE + 3 * HOUR;
    h.clock.0.store(slot + 10_000, Ordering::SeqCst);
    assert_eq!(h.app.automation_tick().await?.created, 4);

    let task = run_task(&h, &now, slot).await?;
    assert_eq!((task.status, task.supertask), (TaskStatus::Queued, false));
    assert!(h.store.chain(&task.id).await?.is_none());

    let task = run_task(&h, &supertask, slot).await?;
    assert_eq!((task.status, task.supertask), (TaskStatus::Queued, true));

    let task = run_task(&h, &ruled, slot).await?;
    assert_eq!(task.status, TaskStatus::Chained);
    let chain = h.store.chain(&task.id).await?.ok_or("chain")?;
    assert_eq!(chain.rule.as_deref(), Some("#1 merged"));

    let waiting = run_task(&h, &after, slot).await?;
    assert_eq!(waiting.status, TaskStatus::Chained);
    let chain = h.store.chain(&waiting.id).await?.ok_or("chain")?;
    assert_eq!(
        (
            chain.after_task_id.as_ref().map(TaskId::as_str),
            chain.rule.as_deref()
        ),
        (Some("t-one"), None)
    );

    // AC-AUT-16: the waiting task is active, so the next day's run is skipped.
    h.clock.0.store(slot + 24 * HOUR + 10_000, Ordering::SeqCst);
    h.app.automation_tick().await?;
    let history = h.store.runs(&after, 1).await?;
    assert_eq!(history[0].state.as_str(), "skipped_previous_active");

    // #1 ends without being done: a new run waits for `#1 done`, which can no longer be true,
    // so it needs a person instead of failing.
    h.store
        .transition_task(
            &TaskId::parse("t-one")?,
            None,
            TaskStatus::Closed,
            None,
            Timestamp(slot + HOUR),
        )
        .await?;
    h.store
        .transition_task(
            &waiting.id,
            None,
            TaskStatus::Closed,
            None,
            Timestamp(slot + HOUR),
        )
        .await?;
    h.clock.0.store(slot + 25 * HOUR, Ordering::SeqCst);
    let run = run_now(&h, &alice, &after).await?;
    assert_eq!(run["state"], "task_created", "{run}");
    let task = h
        .store
        .task(&TaskId::parse(run["task_id"].as_str().ok_or("task")?)?)
        .await?
        .ok_or("task")?;
    assert_eq!(task.status, TaskStatus::Chained);
    let chain = h.store.chain(&task.id).await?.ok_or("chain")?;
    assert_eq!(chain.rule.as_deref(), Some("#1 done"));
    assert!(
        chain.blocked_reason.is_some(),
        "it waits for a person: {chain:?}"
    );

    // A task to run after that was deleted since: the run fails and says why.
    let gone = seed_task(&h, "t-gone", "s1", "u-alice", TaskStatus::Running, None).await?;
    let orphan = create(&h, &alice, &json!({"after_task_id": "t-gone"})).await?;
    assert!(h.store.delete_task(&gone.id).await?);
    let run = run_now(&h, &alice, &orphan).await?;
    assert_eq!(run["state"], "failed", "{run}");
    assert!(
        run["error"]
            .as_str()
            .unwrap_or("")
            .contains("does not exist"),
        "{run}"
    );
    assert!(
        h.store
            .tasks(&SandboxId::parse("s1")?, false)
            .await?
            .iter()
            .all(|t| !t
                .automation_run
                .as_deref()
                .unwrap_or("")
                .starts_with(&orphan)),
        "no task for the failed run"
    );
    Ok(())
}

/// Run now with a start choice in its body (SPEC-093 AUT-023, AC-AUT-23).
async fn run_with(
    h: &Harness,
    creds: &Creds,
    automation: &str,
    body: Value,
) -> Result<support::Resp, Error> {
    send(
        h,
        creds,
        Method::POST,
        &format!("/api/automations/{automation}/run"),
        body,
    )
    .await
}

/// The task a run created.
async fn task_of(h: &Harness, run: &Value) -> Result<TaskRecord, Error> {
    let id = TaskId::parse(run["task_id"].as_str().ok_or("no task for the run")?)?;
    Ok(h.store.task(&id).await?.ok_or("task")?)
}

/// Closes `task` so the automation's next run is not skipped as previous-active.
async fn close(h: &Harness, task: &TaskRecord, at: i64) -> Result<(), Error> {
    h.store
        .transition_task(&task.id, None, TaskStatus::Closed, None, Timestamp(at))
        .await?;
    Ok(())
}

/// AC-AUT-23 (SPEC-093): Run now creates its task with the start choice it is given, for that
/// run only, checked like a save; without a body it uses the stored choice.
#[tokio::test]
async fn run_now_starts_its_task_with_the_choice_it_is_given() -> TestResult {
    let (h, alice) = world().await?;
    h.clock.0.store(BASE, Ordering::SeqCst);
    let id = create(&h, &alice, &json!({})).await?;

    // Invalid choices answer 422 and record no run.
    for body in [
        json!({"supertask": true, "start_rule": "#1 done"}),
        json!({"after_task_id": "t-one", "start_rule": "#1 done"}),
        json!({"after_task_id": "t-foreign"}),
        json!({"start_rule": "#9 done"}),
        json!({"start_rule": "#1 maybe"}),
    ] {
        let r = run_with(&h, &alice, &id, body.clone()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            r.body
        );
    }
    assert!(h.store.runs(&id, 10).await?.is_empty(), "no run recorded");

    // A supertask for this run; the stored choice stays start now.
    h.clock.0.store(BASE + 1_000, Ordering::SeqCst);
    let r = run_with(&h, &alice, &id, json!({"supertask": true})).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["state"], "task_created", "{}", r.body);
    let task = task_of(&h, &r.body).await?;
    assert_eq!((task.status, task.supertask), (TaskStatus::Queued, true));
    assert_eq!(
        choice(&stored(&h, &alice, &id).await?),
        (json!(false), Value::Null, Value::Null, Value::Null)
    );
    close(&h, &task, BASE + 2_000).await?;

    // A rule for this run.
    h.clock.0.store(BASE + 3_000, Ordering::SeqCst);
    let r = run_with(&h, &alice, &id, json!({"start_rule": "#1  merged"})).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let task = task_of(&h, &r.body).await?;
    assert_eq!(task.status, TaskStatus::Chained);
    let chain = h.store.chain(&task.id).await?.ok_or("chain")?;
    assert_eq!(chain.rule.as_deref(), Some("#1 merged"));
    close(&h, &task, BASE + 4_000).await?;

    // After a task for this run.
    h.clock.0.store(BASE + 5_000, Ordering::SeqCst);
    let r = run_with(&h, &alice, &id, json!({"after_task_id": "t-one"})).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let task = task_of(&h, &r.body).await?;
    let chain = h.store.chain(&task.id).await?.ok_or("chain")?;
    assert_eq!(
        chain.after_task_id.as_ref().map(TaskId::as_str),
        Some("t-one")
    );
    close(&h, &task, BASE + 6_000).await?;

    // Without a body: the stored choice, start now.
    h.clock.0.store(BASE + 7_000, Ordering::SeqCst);
    let run = run_now(&h, &alice, &id).await?;
    let task = task_of(&h, &run).await?;
    assert_eq!((task.status, task.supertask), (TaskStatus::Queued, false));
    assert!(h.store.chain(&task.id).await?.is_none());
    close(&h, &task, BASE + 8_000).await?;

    // The stored choice sent back is not checked again: a deleted predecessor fails the run, as
    // a scheduled run does, instead of refusing the request.
    let gone = seed_task(&h, "t-gone", "s1", "u-alice", TaskStatus::Running, None).await?;
    let orphan = create(&h, &alice, &json!({"after_task_id": "t-gone"})).await?;
    assert!(h.store.delete_task(&gone.id).await?);
    let r = run_with(&h, &alice, &orphan, json!({"after_task_id": "t-gone"})).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["state"], "failed", "{}", r.body);
    Ok(())
}

/// AC-AUT-17: agents set the start choice with `automation_create` and `automation_update`; in an
/// update any of the three replaces the choice; invalid choices change nothing.
#[tokio::test]
async fn agents_set_the_start_choice_with_the_automation_tools() -> TestResult {
    let (h, alice) = world().await?;
    seed_live_attempt(&h, "t-one", "tok-agent").await?;
    let created = mcp_tool(
        &h,
        "tok-agent",
        "automation_create",
        json!({"name": "Harden", "cron": "0 3 * * *", "prompt": "Harden the app", "supertask": true}),
    )
    .await?;
    assert_eq!(created["isError"], json!(false), "{created}");
    let view = tool_text(&created)["automation"].clone();
    assert_eq!(view["supertask"], json!(true), "{view}");
    let id = view["id"].as_str().ok_or("id")?.to_owned();

    let update = |args: Value| {
        let mut args = args;
        args["automation_id"] = json!(id);
        mcp_tool(&h, "tok-agent", "automation_update", args)
    };
    let r = update(json!({"after_task": 1})).await?;
    assert_eq!(r["isError"], json!(false), "{r}");
    assert_eq!(
        choice(&stored(&h, &alice, &id).await?),
        (json!(false), json!("t-one"), json!(1), Value::Null)
    );
    let r = update(json!({"start_rule": "#1 done"})).await?;
    assert_eq!(r["isError"], json!(false), "{r}");
    assert_eq!(
        choice(&stored(&h, &alice, &id).await?),
        (json!(false), Value::Null, Value::Null, json!("#1 done"))
    );
    // A change of something else keeps the choice.
    let r = update(json!({"name": "Harden more"})).await?;
    assert_eq!(r["isError"], json!(false), "{r}");
    assert_eq!(
        stored(&h, &alice, &id).await?["start_rule"],
        json!("#1 done")
    );

    for args in [
        json!({"supertask": true, "start_rule": "#1 done"}),
        json!({"after_task": 1, "start_rule": "#1 done"}),
        json!({"after_task": 99}),
        json!({"after_task": "one"}),
        json!({"supertask": "yes"}),
        json!({"start_rule": "#7 done"}),
    ] {
        let r = update(args.clone()).await?;
        assert_eq!(r["isError"], json!(true), "{args}: {r}");
    }
    assert_eq!(
        choice(&stored(&h, &alice, &id).await?),
        (json!(false), Value::Null, Value::Null, json!("#1 done"))
    );
    let r = mcp_tool(
        &h,
        "tok-agent",
        "automation_create",
        json!({"name": "Bad", "cron": "0 3 * * *", "prompt": "x", "after_task": 99}),
    )
    .await?;
    assert_eq!(r["isError"], json!(true), "{r}");

    // `supertask: false` alone: start now.
    let r = update(json!({"supertask": false})).await?;
    assert_eq!(r["isError"], json!(false), "{r}");
    assert_eq!(
        choice(&stored(&h, &alice, &id).await?),
        (json!(false), Value::Null, Value::Null, Value::Null)
    );
    Ok(())
}
