//! Queued messages (SPEC-052 QMS-001..005): a task that has not started takes messages into its
//! request — queued, chained or paused before its first start — and refuses them once it has a
//! run or a session; the detail carries the request and whether the task started.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{
    Harness, HarnessOptions, TestResult, call, harness_with, perms, seed_github_user,
    seed_live_attempt, seed_sandbox, seed_task,
};
use yulius_app::model::ProviderSessionRecord;
use yulius_app::ports::store::TaskStore;
use yulius_contracts::events::UiEvent;
use yulius_domain::harness::Harness as AgentHarness;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::TaskStatus;

/// No session slot, so nothing a test queues starts by itself.
fn harness() -> Result<Harness, Box<dyn std::error::Error>> {
    harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })
}

async fn prompt(h: &Harness, id: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(TaskStore::task(&h.store, &TaskId::parse(id)?)
        .await?
        .ok_or("task")?
        .prompt)
}

async fn queue(
    h: &Harness,
    id: &str,
    text: &str,
    who: &support::Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{id}/queued-messages"),
        Some(json!({ "text": text })),
        who,
    )
    .await
}

/// AC-QMS-1: each waiting state that never started takes a message, under a line saying who
/// added it and when; the detail shows the request and that the task has not started.
#[tokio::test]
async fn a_task_that_has_not_started_takes_a_message_into_its_request() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    for (id, status) in [
        ("t-queued", TaskStatus::Queued),
        ("t-chained", TaskStatus::Chained),
        ("t-prepared", TaskStatus::Prepared),
        ("t-paused", TaskStatus::WaitingCapacity),
    ] {
        seed_task(&h, id, "s1", "u-alice", status, None).await?;
        let mut events = h.app.events.subscribe();
        let resp = queue(&h, id, "  Use the blue header, not the red one.\n", &alice).await?;
        assert_eq!(resp.status, StatusCode::OK, "{id}: {:?}", resp.body);
        let stored = prompt(&h, id).await?;
        assert!(
            stored.starts_with("do things\n\n**Added by alice before the task started ("),
            "{id}: {stored}"
        );
        assert!(
            stored.ends_with(" UTC):**\n\nUse the blue header, not the red one."),
            "{id}: {stored}"
        );
        assert_eq!(resp.body["request"], stored.as_str(), "{id}");
        // The task change is announced, so other viewers reload it.
        let envelope = events.try_recv()?;
        assert!(
            matches!(&envelope.event, UiEvent::TaskUpdated { task } if task.id == id),
            "{id}: {:?}",
            envelope.event
        );

        let detail = call(
            &h.router,
            Method::GET,
            &format!("/api/tasks/{id}"),
            None,
            &alice,
        )
        .await?;
        assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.body);
        assert_eq!(detail.body["request"], stored.as_str(), "{id}");
        assert_eq!(detail.body["started"], false, "{id}");
    }

    // A second message follows the first; later lines correct earlier ones.
    queue(&h, "t-queued", "And keep the logo.", &alice).await?;
    let stored = prompt(&h, "t-queued").await?;
    let first = stored.find("Use the blue header").ok_or("first")?;
    let second = stored.find("And keep the logo.").ok_or("second")?;
    assert!(first < second, "{stored}");
    assert_eq!(
        stored.matches("before the task started").count(),
        2,
        "{stored}"
    );
    Ok(())
}

/// AC-QMS-1, AC-QMS-2: a task that has started — running, ended after a run, an imported session,
/// or moved to `starting` a moment ago — refuses a message and keeps its request.
#[tokio::test]
async fn a_task_that_has_started_refuses_a_message() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));

    seed_task(&h, "t-running", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_task(&h, "t-starting", "s1", "u-alice", TaskStatus::Queued, None).await?;
    // Continued after a run and queued again: its next start resumes, never re-reading the request.
    seed_task(&h, "t-requeued", "s1", "u-alice", TaskStatus::Queued, None).await?;
    seed_live_attempt(&h, "t-requeued", "token-requeued").await?;
    // An imported session waiting for its first run here.
    seed_task(&h, "t-imported", "s1", "u-alice", TaskStatus::Queued, None).await?;
    h.store
        .upsert_provider_session(&ProviderSessionRecord {
            id: "ps-1".into(),
            sandbox_id: SandboxId::parse("s1")?,
            task_id: TaskId::parse("t-imported")?,
            harness: AgentHarness::ClaudeCode,
            session_id: "session-1".into(),
            transcript_ref: None,
            origin: "imported".into(),
            first_seen: h.app.now(),
            last_seen: h.app.now(),
        })
        .await?;
    // The start's compare-and-set happened just before the message arrived.
    assert!(
        h.store
            .transition_task(
                &TaskId::parse("t-starting")?,
                Some(TaskStatus::Queued),
                TaskStatus::Starting,
                None,
                h.app.now(),
            )
            .await?
    );

    for id in ["t-running", "t-starting", "t-requeued", "t-imported"] {
        let resp = queue(&h, id, "too late", &alice).await?;
        assert_eq!(resp.status, StatusCode::CONFLICT, "{id}: {:?}", resp.body);
        assert!(
            resp.body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("type into its session instead"),
            "{id}: {:?}",
            resp.body
        );
        assert_eq!(prompt(&h, id).await?, "do things", "{id}");
    }
    let detail = call(
        &h.router,
        Method::GET,
        "/api/tasks/t-requeued",
        None,
        &alice,
    )
    .await?;
    assert_eq!(detail.body["started"], true);
    Ok(())
}

/// AC-QMS-1: read access cannot queue; empty text and a request past the creation limit are
/// refused with the request unchanged.
#[tokio::test]
async fn a_message_needs_write_access_and_fitting_text() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    seed_task(&h, "t-1", "s1", "u-alice", TaskStatus::Queued, None).await?;

    let read = queue(&h, "t-1", "from a reader", &bob).await?;
    assert_eq!(read.status, StatusCode::FORBIDDEN, "{:?}", read.body);
    let empty = queue(&h, "t-1", " \n\t ", &alice).await?;
    assert_eq!(
        empty.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        empty.body
    );
    let long = queue(&h, "t-1", &"x".repeat(64 * 1024), &alice).await?;
    assert_eq!(
        long.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        long.body
    );
    assert_eq!(prompt(&h, "t-1").await?, "do things");
    // Read access still sees the request.
    let detail = call(&h.router, Method::GET, "/api/tasks/t-1", None, &bob).await?;
    assert_eq!(detail.body["request"], "do things");
    Ok(())
}

/// A task that has started says so, also when the message would not have fitted: the session is
/// the way in, whatever the request's size.
#[tokio::test]
async fn a_started_task_with_a_long_request_says_it_has_started() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    seed_task(&h, "t-long", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let id = TaskId::parse("t-long")?;
    assert!(
        h.store
            .append_unstarted_prompt(&id, &"x".repeat(60 * 1024), 64 * 1024, h.app.now())
            .await?
    );
    assert!(
        h.store
            .transition_task(&id, None, TaskStatus::Running, None, h.app.now())
            .await?
    );
    let resp = queue(&h, "t-long", &"y".repeat(8 * 1024), &alice).await?;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{:?}", resp.body);
    Ok(())
}
