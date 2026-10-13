//! SPEC-070: a task is owned by one person, whose GitHub credentials it uses. Only the owner
//! changes the task; anyone else who may work in the project asks the owner for control, and a
//! hand-over moves the task's GitHub identity to the new owner.

use axum::http::Method;
use serde_json::json;
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::TaskStore as _;
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::task::TaskStatus;

use crate::support::{
    Creds, Harness, call, harness, perms, seed_github_user, seed_live_attempt, seed_sandbox,
    seed_task,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Alice (`u-a`) and Bob (`u-b`) may both write to `acme/repo`; Alice created task `t1`, whose
/// live run has the task token `tt`.
struct Fixture {
    h: Harness,
    alice: Creds,
    bob: Creds,
    task: TaskRecord,
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-a", "alice").await?;
    let bob = seed_github_user(&h, "u-b", "bob").await?;
    seed_sandbox(&h, "s1", "repo", "u-a").await?;
    h.github.set_perm("alice", "acme/repo", perms("write"));
    h.github.set_perm("bob", "acme/repo", perms("write"));
    let task = seed_task(&h, "t1", "s1", "u-a", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", "tt").await?;
    Ok(Fixture {
        h,
        alice,
        bob,
        task,
    })
}

async fn git_credential(h: &Harness, token: &str) -> Result<String, Box<dyn std::error::Error>> {
    let resp = call(
        &h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com", "path": "acme/repo.git"})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(resp.status, 200, "{}", resp.body);
    Ok(resp.body["token"].as_str().ok_or("no token")?.to_owned())
}

/// AC1: a new task's owner is its creator, and its summary names them.
#[tokio::test]
async fn a_task_is_owned_by_its_creator() -> TestResult {
    let f = fixture().await?;
    assert_eq!(f.task.owner, f.task.created_by);
    let detail = call(
        &f.h.router,
        Method::GET,
        &format!("/api/tasks/{}", f.task.id),
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(detail.status, 200, "{}", detail.body);
    assert_eq!(detail.body["owner"]["login"], "alice");
    assert_eq!(detail.body["created_by"]["login"], "alice");
    assert_eq!(git_credential(&f.h, "tt").await?, "token-alice");
    let _ = &f.alice;
    Ok(())
}

/// R2: what a task does on GitHub uses its owner's token, not its creator's.
#[tokio::test]
async fn a_task_acts_on_github_as_its_owner() -> TestResult {
    let f = fixture().await?;
    let mut owned = f.task.clone();
    owned.id = TaskId::parse("t2")?;
    owned.owner = UserId::parse("u-b")?;
    f.h.store.insert_task(&owned).await?;
    seed_live_attempt(&f.h, "t2", "tt2").await?;
    assert_eq!(git_credential(&f.h, "tt2").await?, "token-bob");
    let summary = f.h.app.task_summary(&owned).await?;
    assert_eq!(summary.owner.login, "bob");
    assert_eq!(summary.created_by.login, "alice");
    Ok(())
}
