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
