//! Permanent task deletion (SPEC-001 TSK-009, AC-TSK-8): write users delete resting tasks with
//! their records and directories; live tasks and read users are refused.

mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{TestResult, call, harness, perms, seed_github_user, seed_sandbox, seed_task};
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

#[tokio::test]
async fn write_users_delete_resting_tasks_with_their_records() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let data = h.app.config.data_dir.clone();
    let worktree = data.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(&worktree)?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Closed,
        Some(&worktree),
    )
    .await?;
    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Running, None).await?;
    let fb = call(
        &h.router,
        Method::POST,
        "/api/tasks/t1/feedback",
        Some(json!({"verdict": "correct", "comment": "ok"})),
        &alice,
    )
    .await?;
    assert_eq!(fb.status, StatusCode::NO_CONTENT, "{:?}", fb.body);

    // AC-ANL-2: feedback is stored per user and shown with the task; a second answer replaces
    // the first; a read user may answer too; an unknown verdict is refused.
    for (who, body) in [
        (
            &alice,
            json!({"verdict": "incorrect", "comment": "misses the empty cart"}),
        ),
        (&bob, json!({"verdict": "correct"})),
    ] {
        let answered = call(
            &h.router,
            Method::POST,
            "/api/tasks/t1/feedback",
            Some(body),
            who,
        )
        .await?;
        assert_eq!(
            answered.status,
            StatusCode::NO_CONTENT,
            "{:?}",
            answered.body
        );
    }
    let bad = call(
        &h.router,
        Method::POST,
        "/api/tasks/t1/feedback",
        Some(json!({"verdict": "maybe"})),
        &alice,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = call(&h.router, Method::GET, "/api/tasks/t1", None, &alice).await?;
    let feedback = detail.body["feedback"].as_array().ok_or("feedback")?;
    assert_eq!(feedback.len(), 2, "one answer per user: {feedback:?}");
    let by = |login: &str| feedback.iter().find(|f| f["login"] == login).cloned();
    let alices = by("alice").ok_or("alice's feedback")?;
    assert_eq!(alices["verdict"], "incorrect");
    assert_eq!(alices["comment"], "misses the empty cart");
    assert_eq!(by("bob").ok_or("bob's feedback")?["verdict"], "correct");

    let refused = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &bob).await?;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "read users cannot delete"
    );
    let live = call(&h.router, Method::DELETE, "/api/tasks/t2", None, &alice).await?;
    assert_eq!(
        live.status,
        StatusCode::CONFLICT,
        "live tasks must be stopped first"
    );

    let deleted = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{:?}", deleted.body);
    assert!(h.store.task(&TaskId::parse("t1")?).await?.is_none());
    let gone = call(&h.router, Method::GET, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    let again = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    // The other task is untouched; the archive list does not show the deleted one.
    assert!(h.store.task(&TaskId::parse("t2")?).await?.is_some());
    let archived = call(&h.router, Method::GET, "/api/archived-tasks", None, &alice).await?;
    assert!(!archived.body.to_string().contains("\"t1\""));
    Ok(())
}

/// Yulius runs Claude Code only (F 2026-10-08): no harness choice on tasks and no setting for
/// harness switching.
#[tokio::test]
async fn tasks_run_claude_code_only() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let codex = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "x", "harness": "codex"})),
        &alice,
    )
    .await?;
    assert_eq!(
        codex.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        codex.body
    );
    let settings = call(&h.router, Method::GET, "/api/admin/settings", None, &alice).await?;
    assert_eq!(settings.status, StatusCode::NOT_FOUND);
    Ok(())
}
