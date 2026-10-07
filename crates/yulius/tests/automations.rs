//! Workflows and automations end to end: CRUD and permissions, the scheduler with a
//! controllable clock (due run, restart deduplication, catch-up, disabled and previous-active
//! skips, failures) and the agent tools (AC-AUT-1..4).

mod support;

use std::net::SocketAddr;
use std::sync::atomic::Ordering;

use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, TestResult, add_sandbox, call, harness_configured, perms, user_with_token};
use yulius_app::automations::{AutomationRunRecord, RunState, run_key};
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AttemptRecord, LaunchKind, Sealed, TaskOrigin, TaskRecord};
use yulius_app::ports::store::{SandboxStore, TaskStore};
use yulius_app::ports::store_ext::AutomationStore;
use yulius_contracts::events::UiEvent;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type Error = Box<dyn std::error::Error>;

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;
/// The harness clock starts at 2026-10-07T00:00:00Z.
const BASE: i64 = 1_791_331_200_000;

struct World {
    h: support::Harness,
    alice: Creds,
    bob: Creds,
    root: Creds,
}

/// Sandbox `s1` (studio) with alice (write), bob (read) and root (instance admin, write).
/// Created tasks stay queued (no live-session capacity), which counts as active.
async fn world() -> Result<World, Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    let bob = user_with_token(&h, "u-bob", "bob", false).await?;
    let root = user_with_token(&h, "u-root", "root", true).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    h.github
        .set_perm("bob", "Quartio-Studio/studio", perms("read"));
    h.github
        .set_perm("root", "Quartio-Studio/studio", perms("write"));
    Ok(World {
        h,
        alice,
        bob,
        root,
    })
}

fn set_clock(h: &support::Harness, ms: i64) {
    h.clock.0.store(ms, Ordering::SeqCst);
}

async fn create_automation(w: &World, body: Value) -> Result<String, Error> {
    let r = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/automations",
        Some(body),
        &w.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    Ok(r.body["id"].as_str().ok_or("id")?.to_owned())
}

fn daily(name: &str, cron: &str) -> Value {
    json!({"name": name, "cron": cron, "utc_offset_minutes": 0, "prompt": format!("{name}: implement yesterday's customer feedback"), "harness": "claude"})
}

async fn automation_tasks(
    h: &support::Harness,
    automation: &str,
) -> Result<Vec<TaskRecord>, Error> {
    let sid = SandboxId::parse("s1")?;
    let mut tasks = h.store.tasks(&sid, false).await?;
    tasks.extend(h.store.tasks(&sid, true).await?);
    tasks.retain(|t| {
        t.automation_run
            .as_deref()
            .is_some_and(|k| k.starts_with(&format!("{automation}@")))
    });
    Ok(tasks)
}

async fn runs(w: &World, automation: &str) -> Result<Vec<Value>, Error> {
    let r = call(
        &w.h.router,
        Method::GET,
        &format!("/api/automations/{automation}/runs"),
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    Ok(r.body.as_array().cloned().unwrap_or_default())
}

#[tokio::test]
async fn workflows_seed_builtins_scope_permissions_and_task_steps() -> TestResult {
    let w = world().await?;
    let list = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/workflows",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK);
    let names: Vec<&str> = list
        .body
        .as_array()
        .ok_or("list")?
        .iter()
        .filter_map(|w| w["name"].as_str())
        .collect();
    assert_eq!(names, ["Feature", "Quick task"]);
    assert_eq!(
        list.body[0]["steps"],
        json!(["Write spec with plan", "Implement", "Test", "Open PR"])
    );
    assert_eq!(
        list.body[0]["gates"],
        json!({"require_spec": true, "require_tests": true})
    );
    assert_eq!(list.body[1]["steps"], json!([]));
    assert_eq!(list.body[0]["scope"], "instance");

    let recipe = json!({"name": "Release", "steps": ["Bump version", "Changelog"], "instructions": "Follow RELEASING.md.", "gates": {"require_spec": false, "require_tests": true}});
    let denied = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/workflows",
        Some(recipe.clone()),
        &w.bob,
    )
    .await?;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "read access cannot configure"
    );
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/workflows",
        Some(recipe.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["scope"], "sandbox");
    let release = created.body["id"].as_str().ok_or("id")?.to_owned();

    let mut instance_recipe = recipe.clone();
    instance_recipe["scope"] = json!("instance");
    instance_recipe["name"] = json!("Hotfix");
    let not_admin = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/workflows",
        Some(instance_recipe.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(
        not_admin.status,
        StatusCode::FORBIDDEN,
        "instance workflows need an instance admin"
    );
    let hotfix = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/workflows",
        Some(instance_recipe),
        &w.root,
    )
    .await?;
    assert_eq!(hotfix.status, StatusCode::CREATED);
    assert_eq!(hotfix.body["scope"], "instance");
    let hotfix_id = hotfix.body["id"].as_str().ok_or("id")?.to_owned();
    let edit_instance = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/workflows/{hotfix_id}"),
        Some(json!({"name": "Hotfix 2"})),
        &w.alice,
    )
    .await?;
    assert_eq!(edit_instance.status, StatusCode::FORBIDDEN);
    let edit = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/workflows/{release}"),
        Some(json!({"name": "Release train", "steps": ["Tag"]})),
        &w.alice,
    )
    .await?;
    assert_eq!(edit.status, StatusCode::OK);
    assert_eq!(edit.body["name"], "Release train");
    assert_eq!(edit.body["steps"], json!(["Tag"]));
    let invalid = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/workflows",
        Some(json!({"name": "  "})),
        &w.alice,
    )
    .await?;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);

    // AC-AUT-3: a task created from a workflow gets its steps and instructions.
    let task = call(&w.h.router, Method::POST, "/api/sandboxes/s1/tasks", Some(json!({"prompt": "Add a checkout API", "harness": "claude", "workflow_id": "wf-feature"})), &w.alice).await?;
    assert_eq!(task.status, StatusCode::CREATED, "{}", task.body);
    assert_eq!(task.body["steps"]["total"], 4);
    assert_eq!(task.body["steps"]["current"], "Write spec with plan");
    let record =
        w.h.store
            .task(&TaskId::parse(task.body["id"].as_str().ok_or("id")?)?)
            .await?
            .ok_or("task")?;
    let instructions =
        w.h.app
            .workflow_instructions(&record)
            .await
            .ok_or("instructions")?;
    assert!(instructions.contains("spec"), "{instructions}");
    let quick = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "Fix a typo", "workflow_id": "wf-quick-task"})),
        &w.alice,
    )
    .await?;
    assert_eq!(quick.body["steps"]["total"], 0);
    assert_eq!(
        quick.body["harness"], "claude",
        "Claude Code is the default harness"
    );
    let codex = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "x", "harness": "codex"})),
        &w.alice,
    )
    .await?;
    assert_eq!(
        codex.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "Yulius runs Claude Code only"
    );
    let quick_record =
        w.h.store
            .task(&TaskId::parse(quick.body["id"].as_str().ok_or("id")?)?)
            .await?
            .ok_or("task")?;
    assert_eq!(w.h.app.workflow_instructions(&quick_record).await, None);
    let unknown = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "x", "harness": "claude", "workflow_id": "nope"})),
        &w.alice,
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY);

    // A sandbox workflow is not usable from another sandbox.
    add_sandbox(&w.h, "s2", "product", 20020).await?;
    w.h.github
        .set_perm("alice", "Quartio-Studio/product", perms("write"));
    let foreign = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s2/tasks",
        Some(json!({"prompt": "x", "harness": "claude", "workflow_id": release})),
        &w.alice,
    )
    .await?;
    assert_eq!(foreign.status, StatusCode::UNPROCESSABLE_ENTITY);

    let deleted = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/workflows/{release}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let after = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/workflows",
        None,
        &w.alice,
    )
    .await?;
    let names: Vec<&str> = after
        .body
        .as_array()
        .ok_or("list")?
        .iter()
        .filter_map(|w| w["name"].as_str())
        .collect();
    assert_eq!(names, ["Feature", "Hotfix", "Quick task"]);
    Ok(())
}

#[tokio::test]
async fn automation_crud_validates_schedule_and_permissions() -> TestResult {
    let w = world().await?;
    let mut body = daily("Daily feedback", "0 9 * * *");
    body["utc_offset_minutes"] = json!(120);
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/automations",
        Some(body.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["created_via"], "ui");
    assert_eq!(created.body["enabled"], true);
    assert_eq!(
        created.body["next_run_at"], "2026-10-07T07:00:00Z",
        "09:00 at UTC+2"
    );
    assert_eq!(created.body["last_run"], Value::Null);
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();

    for (field, value) in [
        ("cron", json!("0 25 * * *")),
        ("cron", json!("every day")),
        ("utc_offset_minutes", json!(19 * 60)),
        ("harness", json!("gpt")),
        ("prompt", json!("   ")),
        ("workflow_id", json!("missing")),
    ] {
        let mut bad = body.clone();
        bad[field] = value;
        let r = call(
            &w.h.router,
            Method::POST,
            "/api/sandboxes/s1/automations",
            Some(bad),
            &w.alice,
        )
        .await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {}",
            r.body
        );
    }
    let hour = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/automations",
        Some(json!({"name": "x", "cron": "0 25 * * *", "prompt": "p", "harness": "claude"})),
        &w.alice,
    )
    .await?;
    assert!(
        hour.body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("hour"),
        "{}",
        hour.body
    );

    let denied = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/automations",
        Some(body.clone()),
        &w.bob,
    )
    .await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/automations",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(listed.body.as_array().map(Vec::len), Some(1));

    let mut disable = body.clone();
    disable["enabled"] = json!(false);
    disable["workflow_id"] = json!("wf-feature");
    let updated = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/automations/{id}"),
        Some(disable),
        &w.alice,
    )
    .await?;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!(updated.body["enabled"], false);
    assert_eq!(updated.body["next_run_at"], Value::Null);
    assert_eq!(updated.body["workflow_id"], "wf-feature");
    assert!(runs(&w, &id).await?.is_empty());

    let bob_delete = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/automations/{id}"),
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(bob_delete.status, StatusCode::FORBIDDEN);
    let unknown = call(
        &w.h.router,
        Method::DELETE,
        "/api/automations/nope",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let deleted = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/automations/{id}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let empty = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/automations",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(empty.body, json!([]));
    Ok(())
}

/// AC-AUT-1: a cron automation creates exactly one task at its scheduled time.
#[tokio::test]
async fn due_run_creates_exactly_one_task() -> TestResult {
    let w = world().await?;
    let id = create_automation(&w, daily("Daily feedback", "0 9 * * *")).await?;
    let mut events = w.h.app.events.subscribe();

    set_clock(&w.h, BASE + 9 * HOUR - 30_000);
    assert_eq!(w.h.app.automation_tick().await?.created, 0);
    assert!(automation_tasks(&w.h, &id).await?.is_empty());

    set_clock(&w.h, BASE + 9 * HOUR + 10_000);
    let report = w.h.app.automation_tick().await?;
    assert_eq!(report.created, 1);
    set_clock(&w.h, BASE + 9 * HOUR + 40_000);
    assert_eq!(
        w.h.app.automation_tick().await?.created,
        0,
        "the slot is handled once"
    );

    let tasks = automation_tasks(&w.h, &id).await?;
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.origin, TaskOrigin::Automation);
    assert_eq!(
        task.automation_run.as_deref(),
        Some(run_key(&id, Timestamp(BASE + 9 * HOUR)).as_str())
    );
    assert_eq!(task.title, "Daily feedback");
    assert!(task.prompt.contains("customer feedback"));
    assert_eq!(task.created_by, UserId::parse("u-alice")?);

    let history = runs(&w, &id).await?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["state"], "task_created");
    assert_eq!(history[0]["scheduled_for"], "2026-10-07T09:00:00Z");
    assert_eq!(history[0]["task_id"], task.id.as_str());
    let view = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/automations",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(view.body[0]["last_run"]["state"], "task_created");
    assert_eq!(view.body[0]["next_run_at"], "2026-10-08T09:00:00Z");

    let mut announced = None;
    while let Ok(envelope) = events.try_recv() {
        if let UiEvent::AutomationRun {
            automation_id,
            state,
            task_id,
        } = envelope.event
        {
            announced = Some((automation_id, state, task_id));
        }
    }
    assert_eq!(
        announced,
        Some((
            id.clone(),
            "task_created".to_owned(),
            Some(task.id.to_string())
        ))
    );

    // The next day runs again.
    set_clock(&w.h, BASE + DAY + 9 * HOUR + 5_000);
    w.h.store
        .transition_task(
            &task.id,
            None,
            TaskStatus::Closed,
            None,
            Timestamp(BASE + DAY),
        )
        .await?;
    assert_eq!(w.h.app.automation_tick().await?.created, 1);
    assert_eq!(automation_tasks(&w.h, &id).await?.len(), 2);
    Ok(())
}

/// AC-AUT-4: crashes between the run row and the task, or between the task and the row
/// update, and two schedulers racing on one slot never produce a second task.
#[tokio::test]
async fn restarts_and_races_never_duplicate_a_run() -> TestResult {
    let w = world().await?;
    let a = create_automation(&w, daily("Crash before task", "0 9 * * *")).await?;
    let b = create_automation(&w, daily("Crash after task", "30 9 * * *")).await?;
    set_clock(&w.h, BASE + 9 * HOUR + 40 * MIN);

    // A: the run row exists but no task was created.
    w.h.store
        .insert_run(&AutomationRunRecord {
            automation_id: a.clone(),
            scheduled_for: Timestamp(BASE + 9 * HOUR),
            state: RunState::Scheduled,
            task_id: None,
            error: None,
            created_at: Timestamp(BASE + 9 * HOUR),
        })
        .await?;
    // B: the run row exists and the task was created, but the row was never updated.
    let b_slot = Timestamp(BASE + 9 * HOUR + 30 * MIN);
    w.h.store
        .insert_run(&AutomationRunRecord {
            automation_id: b.clone(),
            scheduled_for: b_slot,
            state: RunState::Scheduled,
            task_id: None,
            error: None,
            created_at: b_slot,
        })
        .await?;
    let orphan =
        w.h.app
            .create_task_internal(NewTask {
                sandbox: SandboxId::parse("s1")?,
                created_by: UserId::parse("u-alice")?,
                prompt: "Crash after task".into(),
                harness: Harness::ClaudeCode,
                title: Some("Crash after task".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Automation,
                automation_run: Some(run_key(&b, b_slot)),
                start: true,
                mode: yulius_app::model::TaskMode::Prompt,
            })
            .await?;

    // Restart: a fresh application over the same database.
    let restarted = App::new(w.h.app.config.clone(), w.h.app.deps.clone());
    restarted.automation_startup().await?;
    restarted.automation_tick().await?;
    let a_tasks = automation_tasks(&w.h, &a).await?;
    assert_eq!(
        a_tasks.len(),
        1,
        "the interrupted run created its task once"
    );
    let b_tasks = automation_tasks(&w.h, &b).await?;
    assert_eq!(
        b_tasks.len(),
        1,
        "the existing task was linked, not recreated"
    );
    assert_eq!(b_tasks[0].id, orphan.id);
    let b_runs = runs(&w, &b).await?;
    assert_eq!(b_runs[0]["state"], "task_created");
    assert_eq!(b_runs[0]["task_id"], orphan.id.as_str());

    // A second restart changes nothing.
    let again = App::new(w.h.app.config.clone(), w.h.app.deps.clone());
    again.automation_startup().await?;
    again.automation_tick().await?;
    assert_eq!(automation_tasks(&w.h, &a).await?.len(), 1);
    assert_eq!(automation_tasks(&w.h, &b).await?.len(), 1);

    // Two schedulers tick the same due slot concurrently.
    let c = create_automation(&w, daily("Raced", "0 10 * * *")).await?;
    set_clock(&w.h, BASE + 10 * HOUR + 5_000);
    let other = App::new(w.h.app.config.clone(), w.h.app.deps.clone());
    let (first, second) = tokio::join!(w.h.app.automation_tick(), other.automation_tick());
    let created = first?.created + second?.created;
    assert!(created >= 1, "both passes report the run they saw");
    assert_eq!(
        automation_tasks(&w.h, &c).await?.len(),
        1,
        "exactly one task for the slot"
    );
    assert_eq!(runs(&w, &c).await?.len(), 1);
    Ok(())
}

/// SPEC-002 §6.4: at most one catch-up run, only when missed by less than one interval.
#[tokio::test]
async fn startup_catches_up_at_most_one_recent_run() -> TestResult {
    let w = world().await?;
    let daily_id = create_automation(&w, daily("Daily", "0 9 * * *")).await?;
    let hourly_id = create_automation(&w, daily("Hourly", "0 * * * *")).await?;

    // Down from midnight until 09:30: the daily 09:00 run was missed by less than one interval;
    // the hourly automation missed nine runs and resumes at its next regular time.
    set_clock(&w.h, BASE + 9 * HOUR + 30 * MIN);
    let report = w.h.app.automation_startup().await?;
    assert_eq!(report.created, 1);
    assert_eq!(automation_tasks(&w.h, &daily_id).await?.len(), 1);
    assert!(automation_tasks(&w.h, &hourly_id).await?.is_empty());
    assert_eq!(
        runs(&w, &daily_id).await?[0]["scheduled_for"],
        "2026-10-07T09:00:00Z"
    );

    set_clock(&w.h, BASE + 9 * HOUR + 31 * MIN);
    assert_eq!(
        w.h.app.automation_tick().await?.created,
        0,
        "missed history is not replayed"
    );
    set_clock(&w.h, BASE + 10 * HOUR + 10_000);
    assert_eq!(w.h.app.automation_tick().await?.created, 1);
    assert_eq!(automation_tasks(&w.h, &hourly_id).await?.len(), 1);

    // A suspended process (two hourly slots elapsed between ticks) drops the stale slots.
    for task in automation_tasks(&w.h, &hourly_id).await? {
        w.h.store
            .transition_task(&task.id, None, TaskStatus::Closed, None, Timestamp(BASE))
            .await?;
    }
    set_clock(&w.h, BASE + 12 * HOUR + 10_000);
    let stale = w.h.app.automation_tick().await?;
    assert_eq!((stale.created, stale.stale), (0, 1));
    set_clock(&w.h, BASE + 13 * HOUR + 10_000);
    assert_eq!(w.h.app.automation_tick().await?.created, 1);
    assert_eq!(automation_tasks(&w.h, &hourly_id).await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn disabled_automations_do_not_run_or_replay_after_enabling() -> TestResult {
    let w = world().await?;
    let mut body = daily("Paused", "0 9 * * *");
    body["enabled"] = json!(false);
    let id = create_automation(&w, body.clone()).await?;
    set_clock(&w.h, BASE + 9 * HOUR + 10_000);
    w.h.app.automation_tick().await?;
    w.h.app.automation_startup().await?;
    assert!(automation_tasks(&w.h, &id).await?.is_empty());
    assert!(runs(&w, &id).await?.is_empty());

    set_clock(&w.h, BASE + 9 * HOUR + 5 * MIN);
    body["enabled"] = json!(true);
    let enabled = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/automations/{id}"),
        Some(body),
        &w.alice,
    )
    .await?;
    assert_eq!(enabled.status, StatusCode::OK);
    set_clock(&w.h, BASE + 9 * HOUR + 6 * MIN);
    w.h.app.automation_tick().await?;
    assert!(
        automation_tasks(&w.h, &id).await?.is_empty(),
        "the slot missed while disabled is not replayed"
    );

    set_clock(&w.h, BASE + DAY + 9 * HOUR + 10_000);
    w.h.app.automation_tick().await?;
    assert_eq!(automation_tasks(&w.h, &id).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn previous_active_task_skips_the_run() -> TestResult {
    let w = world().await?;
    let id = create_automation(&w, daily("Nightly", "0 9 * * *")).await?;
    set_clock(&w.h, BASE + 9 * HOUR + 10_000);
    w.h.app.automation_tick().await?;
    let first = automation_tasks(&w.h, &id).await?;
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].status,
        TaskStatus::Queued,
        "queued counts as active"
    );

    set_clock(&w.h, BASE + DAY + 9 * HOUR + 10_000);
    let report = w.h.app.automation_tick().await?;
    assert_eq!((report.created, report.skipped), (0, 1));
    assert_eq!(automation_tasks(&w.h, &id).await?.len(), 1);
    let history = runs(&w, &id).await?;
    assert_eq!(history[0]["state"], "skipped_previous_active");
    assert!(
        history[0]["error"].as_str().unwrap_or("").contains("#1"),
        "{}",
        history[0]
    );

    w.h.store
        .transition_task(
            &first[0].id,
            None,
            TaskStatus::Closed,
            None,
            Timestamp(BASE + DAY),
        )
        .await?;
    set_clock(&w.h, BASE + 2 * DAY + 9 * HOUR + 10_000);
    assert_eq!(w.h.app.automation_tick().await?.created, 1);
    let states: Vec<String> = runs(&w, &id)
        .await?
        .iter()
        .filter_map(|r| r["state"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        states,
        ["task_created", "skipped_previous_active", "task_created"]
    );
    Ok(())
}

#[tokio::test]
async fn run_now_and_failures_are_recorded() -> TestResult {
    let w = world().await?;
    let mut body = daily("On demand", "0 9 1 1 *");
    body["enabled"] = json!(false);
    let id = create_automation(&w, body).await?;
    set_clock(&w.h, BASE + 3 * HOUR);

    let denied = call(
        &w.h.router,
        Method::POST,
        &format!("/api/automations/{id}/run"),
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let run = call(
        &w.h.router,
        Method::POST,
        &format!("/api/automations/{id}/run"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(run.status, StatusCode::OK, "{}", run.body);
    assert_eq!(
        run.body["state"], "task_created",
        "run now works for disabled automations"
    );
    let task_id = run.body["task_id"].as_str().ok_or("task")?.to_owned();
    let repeat = call(
        &w.h.router,
        Method::POST,
        &format!("/api/automations/{id}/run"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(
        repeat.body["task_id"],
        task_id.as_str(),
        "same instant, same run"
    );
    set_clock(&w.h, BASE + 3 * HOUR + 1_000);
    let busy = call(
        &w.h.router,
        Method::POST,
        &format!("/api/automations/{id}/run"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(busy.body["state"], "skipped_previous_active");
    assert_eq!(automation_tasks(&w.h, &id).await?.len(), 1);

    // A failing task creation is recorded on the run (sandbox being prepared).
    let failing = create_automation(&w, daily("Blocked", "0 9 * * *")).await?;
    w.h.store
        .set_onboarding(&SandboxId::parse("s1")?, "pending", None)
        .await?;
    set_clock(&w.h, BASE + 9 * HOUR + 10_000);
    let report = w.h.app.automation_tick().await?;
    assert_eq!(report.failed, 1);
    let history = runs(&w, &failing).await?;
    assert_eq!(history[0]["state"], "failed");
    assert!(
        history[0]["error"]
            .as_str()
            .unwrap_or("")
            .contains("prepared"),
        "{}",
        history[0]
    );
    assert_eq!(history[0]["task_id"], Value::Null);
    assert!(automation_tasks(&w.h, &failing).await?.is_empty());
    Ok(())
}

async fn seed_agent(
    h: &support::Harness,
    task: &str,
    sandbox: &str,
    token: &str,
) -> Result<TaskRecord, Error> {
    let t = h
        .store
        .insert_task(&TaskRecord {
            id: TaskId::parse(task)?,
            sandbox_id: SandboxId::parse(sandbox)?,
            number: 0,
            title: "agent task".into(),
            prompt: "set up automations".into(),
            harness: Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: UserId::parse("u-alice")?,
            created_at: Timestamp(BASE),
            updated_at: Timestamp(BASE),
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: yulius_app::model::TaskMode::Prompt,
        })
        .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse(&format!("a-{task}"))?,
            task_id: t.id.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/tmp/none".into(),
            token_hash: h.app.deps.crypto.sha256_hex(token.as_bytes()),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(BASE),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(t)
}

async fn tool(
    router: &axum::Router,
    token: &str,
    name: &str,
    args: Value,
) -> Result<(Value, bool), Error> {
    let r = call(
        router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": args}})),
        &Creds { bearer: Some(token.into()), ..Creds::default() },
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    let text = r.body["result"]["content"][0]["text"]
        .as_str()
        .ok_or("text")?;
    let is_error = r.body["result"]["isError"].as_bool().unwrap_or(true);
    Ok((
        serde_json::from_str(text).unwrap_or(Value::String(text.to_owned())),
        is_error,
    ))
}

/// AC-AUT-2: an agent creates an automation through the Yulius MCP tool; it appears in the UI
/// and the audit log records `created_via = agent` with the agent's task.
#[tokio::test]
async fn agents_manage_automations_through_mcp_tools() -> TestResult {
    let w = world().await?;
    let agent = seed_agent(&w.h, "agent1", "s1", "tok-agent").await?;
    let internal = yulius_server::internal_router(w.h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));

    let list = call(
        &internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})),
        &Creds {
            bearer: Some("tok-agent".into()),
            ..Creds::default()
        },
    )
    .await?;
    let names: Vec<&str> = list.body["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in [
        "automation_create",
        "automations_list",
        "automation_update",
        "workflows_list",
    ] {
        assert!(names.contains(&expected), "{expected} missing");
    }

    let (bad, err) = tool(
        &internal,
        "tok-agent",
        "automation_create",
        json!({"name": "x", "cron": "daily", "prompt": "p"}),
    )
    .await?;
    assert!(err, "invalid cron refused: {bad}");
    let (created, err) = tool(&internal, "tok-agent", "automation_create", json!({
        "name": "Daily feedback", "cron": "0 8 * * *", "utc_offset_minutes": 120,
        "prompt": "Create a task implementing yesterday's customer feedback from the Quartio MCP", "workflow": "feature"
    })).await?;
    assert!(!err, "{created}");
    let automation = &created["automation"];
    assert_eq!(automation["created_via"], "agent");
    assert_eq!(
        automation["harness"], "claude",
        "defaults to the agent's harness"
    );
    assert_eq!(automation["workflow_id"], "wf-feature");
    assert_eq!(automation["utc_offset_minutes"], 120);
    let id = automation["id"].as_str().ok_or("id")?.to_owned();

    let ui = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/automations",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(ui.body[0]["id"], id.as_str());
    assert_eq!(ui.body[0]["created_via"], "agent");
    let audit = w.h.store.automation_audit(&id, 10).await?;
    let entry = audit
        .iter()
        .find(|e| e.action == "automation.created")
        .ok_or("audit entry")?;
    assert_eq!(entry.actor, format!("task:{}", agent.number));
    let detail: Value = serde_json::from_str(entry.detail.as_deref().unwrap_or("{}"))?;
    assert_eq!(detail["created_via"], "agent");
    assert_eq!(detail["task_id"], "agent1");

    let (updated, err) = tool(
        &internal,
        "tok-agent",
        "automation_update",
        json!({"automation_id": id, "enabled": false, "cron": "0 7 * * MON-FRI"}),
    )
    .await?;
    assert!(!err, "{updated}");
    assert_eq!(updated["automation"]["enabled"], false);
    assert_eq!(updated["automation"]["cron"], "0 7 * * MON-FRI");
    let (listed, _) = tool(&internal, "tok-agent", "automations_list", json!({})).await?;
    assert_eq!(listed["automations"].as_array().map(Vec::len), Some(1));
    let (workflows, _) = tool(&internal, "tok-agent", "workflows_list", json!({})).await?;
    assert_eq!(workflows["workflows"][0]["name"], "Feature");

    // Automations of other sandboxes are invisible to the agent.
    add_sandbox(&w.h, "s2", "product", 20020).await?;
    w.h.github
        .set_perm("alice", "Quartio-Studio/product", perms("write"));
    let other = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s2/automations",
        Some(daily("Other", "0 9 * * *")),
        &w.alice,
    )
    .await?;
    let other_id = other.body["id"].as_str().ok_or("id")?;
    let (denied, err) = tool(
        &internal,
        "tok-agent",
        "automation_update",
        json!({"automation_id": other_id, "enabled": false}),
    )
    .await?;
    assert!(err, "{denied}");
    Ok(())
}
