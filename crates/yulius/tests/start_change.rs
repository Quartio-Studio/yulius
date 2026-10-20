//! Changing how a waiting task starts (SPEC-126): a prepared or chained task takes another start
//! choice before its first run. These tests use the in-memory harness with no session slot, where
//! a released task leaves `prepared` and `chained`.

use crate::support;

use std::sync::atomic::Ordering;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, perms, seed_task,
    user_with_token,
};
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::{SandboxStore, TaskStore};
use yulius_app::ports::store_ext::IntakeStore;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::{Activity, StopReason, TaskStatus};

type Error = Box<dyn std::error::Error>;

const CREATE: &str = "/api/sandboxes/s1/tasks";
const HOUR: i64 = 3_600_000;

/// Sandbox `s1` where alice may write and her task `t-run` runs; bob may read only, carol may
/// write but owns none of alice's tasks (SPEC-070).
async fn world() -> Result<(Harness, Creds, Creds, Creds, TaskRecord), Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    support::exhausted_subscription(&h).await?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    let bob = user_with_token(&h, "u-bob", "bob", false).await?;
    let carol = user_with_token(&h, "u-carol", "carol", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    h.github
        .set_perm("bob", "Quartio-Studio/studio", perms("read"));
    h.github
        .set_perm("carol", "Quartio-Studio/studio", perms("write"));
    let run = seed_task(&h, "t-run", "s1", "u-alice", TaskStatus::Running, None).await?;
    Ok((h, alice, bob, carol, run))
}

fn rfc3339(millis: i64) -> String {
    yulius_app::auth::rfc3339(yulius_domain::time::Timestamp(millis))
}

fn now(h: &Harness) -> i64 {
    h.clock.0.load(Ordering::SeqCst)
}

async fn created(h: &Harness, creds: &Creds, body: Value) -> Result<String, Error> {
    let r = call(&h.router, Method::POST, CREATE, Some(body), creds).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    Ok(r.body["id"].as_str().ok_or("id")?.to_owned())
}

async fn change(h: &Harness, creds: &Creds, id: &str, body: Value) -> Result<support::Resp, Error> {
    call(
        &h.router,
        Method::PUT,
        &format!("/api/tasks/{id}/start"),
        Some(body),
        creds,
    )
    .await
}

async fn changed(h: &Harness, creds: &Creds, id: &str, body: Value) -> Result<Value, Error> {
    let r = change(h, creds, id, body.clone()).await?;
    assert_eq!(r.status, StatusCode::OK, "{body}: {:?}", r.body);
    Ok(r.body)
}

async fn record(h: &Harness, id: &str) -> Result<TaskRecord, Error> {
    Ok(h.store.task(&TaskId::parse(id)?).await?.ok_or("task")?)
}

/// Waits no more: released to the normal start (queued, or further on).
async fn started(h: &Harness, id: &str) -> Result<bool, Error> {
    let status = record(h, id).await?.status;
    Ok(!matches!(
        status,
        TaskStatus::Chained | TaskStatus::Prepared
    ))
}

/// AC-CHS-1: a prepared task becomes chained after a task or on a rule, and back to prepared
/// with or without a time; each change replaces the one before.
#[tokio::test]
async fn a_prepared_task_changes_to_a_chain_a_rule_or_a_time() -> TestResult {
    let (h, alice, _, _, run) = world().await?;
    let id = created(
        &h,
        &alice,
        json!({"prompt": "Later", "start_at": rfc3339(now(&h) + HOUR)}),
    )
    .await?;

    let body = changed(&h, &alice, &id, json!({"after_task_id": "t-run"})).await?;
    assert_eq!(body["status"], "chained", "{body}");
    assert_eq!(body["after"]["number"], run.number, "{body}");
    assert_eq!(body["after"]["rule"], Value::Null, "{body}");
    assert_eq!(body["start_at"], Value::Null, "{body}");
    assert_eq!(h.store.task_schedule(&TaskId::parse(&id)?).await?, None);

    let rule = format!("#{} done", run.number);
    let body = changed(&h, &alice, &id, json!({"start_rule": rule})).await?;
    assert_eq!(body["status"], "chained", "{body}");
    assert_eq!(body["after"]["rule"], json!(rule), "{body}");

    let at = now(&h) + 2 * HOUR;
    let body = changed(&h, &alice, &id, json!({"start_at": rfc3339(at)})).await?;
    assert_eq!(body["status"], "prepared", "{body}");
    assert_eq!(body["start_at"], json!(rfc3339(at)), "{body}");
    assert_eq!(body["after"], Value::Null, "{body}");

    let body = changed(&h, &alice, &id, json!({"prepared": true})).await?;
    assert_eq!(body["status"], "prepared", "{body}");
    assert_eq!(body["start_at"], Value::Null, "{body}");
    // When I decide: no pass starts it.
    h.clock.0.store(at + HOUR, Ordering::SeqCst);
    h.app.release_due_prepared().await;
    h.app.advance_all_chains().await;
    assert_eq!(record(&h, &id).await?.status, TaskStatus::Prepared);
    Ok(())
}

/// AC-CHS-2: a waiting task changed to Start now or Supertask is released to the normal start; a
/// prepared task changed to run after a task that is already Done starts at once; while
/// onboarding blocks the sandbox, Start now waits after the preparation as a new task does.
#[tokio::test]
async fn a_waiting_task_changes_to_start_now_or_a_supertask() -> TestResult {
    let (h, alice, _, _, _) = world().await?;
    let now_task = created(&h, &alice, json!({"prompt": "a", "after_task_id": "t-run"})).await?;
    let body = changed(&h, &alice, &now_task, json!({})).await?;
    assert!(started(&h, &now_task).await?, "{body}");
    // As Start now: it keeps its chain, so it builds on its predecessor's branch (review).
    assert_eq!(
        h.store
            .chain(&TaskId::parse(&now_task)?)
            .await?
            .and_then(|c| c.after_task_id),
        Some(TaskId::parse("t-run")?)
    );
    let chained_super = created(
        &h,
        &alice,
        json!({"prompt": "a2", "after_task_id": "t-run"}),
    )
    .await?;
    let body = changed(&h, &alice, &chained_super, json!({"supertask": true})).await?;
    assert_eq!(body["supertask"], true, "{body}");
    assert!(started(&h, &chained_super).await?);

    let superb = created(&h, &alice, json!({"prompt": "b", "prepared": true})).await?;
    let body = changed(&h, &alice, &superb, json!({"supertask": true})).await?;
    assert_eq!(body["supertask"], true, "{body}");
    assert!(started(&h, &superb).await?);
    assert!(record(&h, &superb).await?.supertask);

    let done = seed_task(&h, "t-done", "s1", "u-alice", TaskStatus::Running, None).await?;
    h.store
        .set_activity(&done.id, Some(Activity::Done), h.app.now())
        .await?;
    let after_done = created(&h, &alice, json!({"prompt": "c", "prepared": true})).await?;
    changed(&h, &alice, &after_done, json!({"after_task_id": "t-done"})).await?;
    assert!(started(&h, &after_done).await?);

    let gated = created(&h, &alice, json!({"prompt": "d", "prepared": true})).await?;
    let preparation = seed_task(&h, "t-prep", "s1", "u-alice", TaskStatus::Running, None).await?;
    h.store
        .set_onboarding(&SandboxId::parse("s1")?, "preparing", Some(&preparation.id))
        .await?;
    let body = changed(&h, &alice, &gated, json!({})).await?;
    assert_eq!(body["status"], "chained", "{body}");
    assert_eq!(body["after"]["number"], preparation.number, "{body}");
    // A supertask waits after the preparation as one, as a new task does (review).
    let gated_super = created(&h, &alice, json!({"prompt": "e", "prepared": true})).await?;
    let body = changed(&h, &alice, &gated_super, json!({"supertask": true})).await?;
    assert_eq!(body["status"], "chained", "{body}");
    assert_eq!(body["supertask"], true, "{body}");
    assert!(record(&h, &gated_super).await?.supertask);
    Ok(())
}

/// AC-CHS-3: a task that started, ended or is in line for a session keeps its start; a reader
/// and a writer who does not own the task change nothing; an invalid choice changes nothing.
#[tokio::test]
async fn a_start_change_is_refused_for_started_tasks_strangers_and_bad_choices() -> TestResult {
    let (h, alice, bob, carol, _) = world().await?;
    let queued = created(&h, &alice, json!({"prompt": "in line"})).await?;
    assert_eq!(record(&h, &queued).await?.status, TaskStatus::Queued);
    let cancelled = created(&h, &alice, json!({"prompt": "gone", "prepared": true})).await?;
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{cancelled}/stop"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    for id in ["t-run", queued.as_str(), cancelled.as_str()] {
        let r = change(&h, &alice, id, json!({"prepared": true})).await?;
        assert_eq!(r.status, StatusCode::CONFLICT, "{id}: {:?}", r.body);
        assert!(r.body.to_string().contains("started"), "{:?}", r.body);
    }
    assert_eq!(record(&h, &queued).await?.status, TaskStatus::Queued);
    assert_eq!(
        record(&h, &cancelled).await?.status,
        TaskStatus::Stopped(StopReason::Cancelled)
    );

    let waiting = created(&h, &alice, json!({"prompt": "wait", "prepared": true})).await?;
    for creds in [&bob, &carol] {
        let r = change(&h, creds, &waiting, json!({"after_task_id": "t-run"})).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
        assert_eq!(record(&h, &waiting).await?.status, TaskStatus::Prepared);
    }
    for (body, says) in [
        (
            json!({"after_task_id": "t-run", "start_rule": "#1 done"}),
            "not both",
        ),
        (
            json!({"prepared": true, "after_task_id": "t-run"}),
            "prepared",
        ),
        (json!({"start_at": rfc3339(now(&h) - 5 * 60_000)}), "passed"),
        (
            json!({"supertask": true, "start_rule": "#1 done"}),
            "supertask",
        ),
        (json!({"start_rule": "#999 done"}), "#999"),
        (json!({"after_task_id": "nope"}), "does not exist"),
    ] {
        let r = change(&h, &alice, &waiting, body.clone()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {:?}",
            r.body
        );
        assert!(r.body.to_string().contains(says), "{body}: {:?}", r.body);
    }
    let kept = record(&h, &waiting).await?;
    assert_eq!(kept.status, TaskStatus::Prepared);
    assert_eq!(h.store.chain(&kept.id).await?, None);
    Ok(())
}

/// AC-CHS-4: a wait never waits for itself, directly or through other waiting tasks' predecessors
/// and rules.
#[tokio::test]
async fn a_wait_never_waits_for_itself() -> TestResult {
    let (h, alice, _, _, run) = world().await?;
    let p = created(&h, &alice, json!({"prompt": "p", "prepared": true})).await?;
    let pn = record(&h, &p).await?.number;
    let after_p = created(&h, &alice, json!({"prompt": "c", "after_task_id": p})).await?;
    let cn = record(&h, &after_p).await?.number;
    let ruled = created(
        &h,
        &alice,
        json!({"prompt": "r", "start_rule": format!("#{cn} done")}),
    )
    .await?;
    let rn = record(&h, &ruled).await?.number;
    for body in [
        json!({"after_task_id": p}),
        json!({"start_rule": format!("#{pn} done")}),
        json!({"after_task_id": after_p}),
        json!({"start_rule": format!("#{} done or #{cn} ended", run.number)}),
        json!({"after_task_id": ruled}),
        json!({"start_rule": format!("#{rn} done")}),
    ] {
        let r = change(&h, &alice, &p, body.clone()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {:?}",
            r.body
        );
        assert!(r.body.to_string().contains("waits"), "{body}: {:?}", r.body);
    }
    assert_eq!(record(&h, &p).await?.status, TaskStatus::Prepared);
    // A wait on an unrelated task is fine.
    changed(&h, &alice, &p, json!({"after_task_id": "t-run"})).await?;
    Ok(())
}

/// AC-CHS-5: the sandbox's bounds count a task moving into chains or prepared tasks, not one
/// changing its chain.
#[tokio::test]
async fn the_bounds_count_only_a_move_into_another_wait() -> TestResult {
    let (h, alice, _, _, run) = world().await?;
    let prepared = created(&h, &alice, json!({"prompt": "p", "prepared": true})).await?;
    let mut chained = Vec::new();
    for n in 0..32 {
        chained.push(
            created(
                &h,
                &alice,
                json!({"prompt": format!("c{n}"), "after_task_id": "t-run"}),
            )
            .await?,
        );
    }
    let r = change(&h, &alice, &prepared, json!({"after_task_id": "t-run"})).await?;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", r.body);
    assert_eq!(record(&h, &prepared).await?.status, TaskStatus::Prepared);
    let body = changed(
        &h,
        &alice,
        &chained[0],
        json!({"start_rule": format!("#{} ended", run.number)}),
    )
    .await?;
    assert_eq!(body["status"], "chained", "{body}");
    Ok(())
}

/// AC-CHS-7: a rule written by a change judges the tasks that existed when it was written: a task
/// created after the waiting task, before the change, answers for it.
#[tokio::test]
async fn a_changed_rule_judges_tasks_from_when_it_was_written() -> TestResult {
    let (h, alice, _, _, _) = world().await?;
    let waiting = created(&h, &alice, json!({"prompt": "older", "prepared": true})).await?;
    h.clock.0.fetch_add(60_000, Ordering::SeqCst);
    let newer = seed_task(&h, "t-newer", "s1", "u-alice", TaskStatus::Running, None).await?;
    h.clock.0.fetch_add(60_000, Ordering::SeqCst);
    let body = changed(
        &h,
        &alice,
        &waiting,
        json!({"start_rule": format!("#{} done", newer.number)}),
    )
    .await?;
    assert_eq!(body["status"], "chained", "{body}");
    h.app.advance_all_chains().await;
    let still = record(&h, &waiting).await?;
    assert_eq!(still.status, TaskStatus::Chained);
    assert_eq!(
        h.store
            .chain(&still.id)
            .await?
            .and_then(|c| c.blocked_reason),
        None,
        "#{} has not ended",
        newer.number
    );
    h.store
        .set_activity(&newer.id, Some(Activity::Done), h.app.now())
        .await?;
    h.app.advance_chains(&newer.id).await;
    assert!(started(&h, &waiting).await?);
    Ok(())
}

/// CHS-007: the old predecessor no longer lists the task, the new one does.
#[tokio::test]
async fn the_predecessors_follow_a_changed_chain() -> TestResult {
    let (h, alice, _, _, _) = world().await?;
    seed_task(&h, "t-other", "s1", "u-alice", TaskStatus::Running, None).await?;
    let id = created(&h, &alice, json!({"prompt": "a", "after_task_id": "t-run"})).await?;
    let next = |body: &Value| -> Vec<Value> {
        body["next"]
            .as_array()
            .map(|links| links.iter().map(|l| l["task_id"].clone()).collect())
            .unwrap_or_default()
    };
    let path = |id: &str| format!("/api/tasks/{id}");
    let run = call(&h.router, Method::GET, &path("t-run"), None, &alice).await?;
    assert_eq!(next(&run.body), vec![json!(id)], "{:?}", run.body);
    changed(&h, &alice, &id, json!({"after_task_id": "t-other"})).await?;
    let run = call(&h.router, Method::GET, &path("t-run"), None, &alice).await?;
    assert!(next(&run.body).is_empty(), "{:?}", run.body);
    let other = call(&h.router, Method::GET, &path("t-other"), None, &alice).await?;
    assert_eq!(next(&other.body), vec![json!(id)], "{:?}", other.body);
    Ok(())
}

/// Review of SPEC-126: two changes at once cannot each pass the check and make two tasks wait for
/// each other.
#[tokio::test]
async fn two_changes_at_once_never_make_a_cycle() -> TestResult {
    let (h, alice, _, _, _) = world().await?;
    let a = created(&h, &alice, json!({"prompt": "a", "prepared": true})).await?;
    let b = created(&h, &alice, json!({"prompt": "b", "prepared": true})).await?;
    for _ in 0..8 {
        let (ra, rb) = tokio::join!(
            change(&h, &alice, &a, json!({"after_task_id": b})),
            change(&h, &alice, &b, json!({"after_task_id": a})),
        );
        let statuses = [ra?.status, rb?.status];
        assert!(
            statuses.contains(&StatusCode::OK)
                && statuses.contains(&StatusCode::UNPROCESSABLE_ENTITY),
            "{statuses:?}"
        );
        // Back to waiting for a person, for the next round.
        for id in [&a, &b] {
            changed(&h, &alice, id, json!({"prepared": true})).await?;
        }
    }
    Ok(())
}
