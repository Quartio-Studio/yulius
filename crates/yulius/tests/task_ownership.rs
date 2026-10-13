//! SPEC-070: a task is owned by one person, whose GitHub credentials it uses. Only the owner
//! changes the task; anyone else who may work in the project asks the owner for control, and a
//! hand-over moves the task's GitHub identity to the new owner.

use axum::http::Method;
use serde_json::json;
use yulius_app::auth::AuthContext;
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::{TaskStore as _, UserStore as _};
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::task::TaskStatus;

use crate::support::{
    Creds, Harness, call, harness, mcp_tool, perms, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task, tool_text,
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

async fn auth(h: &Harness, id: &str) -> Result<AuthContext, Box<dyn std::error::Error>> {
    let user = h
        .store
        .user(&UserId::parse(id)?)
        .await?
        .ok_or("no such user")?;
    Ok(AuthContext {
        user,
        csrf_token: None,
        session_hash: None,
    })
}

/// The task writes of the web API (R3), each with a body that passes its validation.
fn writes(task: &str) -> Vec<(Method, String, Option<serde_json::Value>)> {
    let path = |rest: &str| format!("/api/tasks/{task}{rest}");
    vec![
        (Method::POST, path("/stop"), None),
        (Method::POST, path("/continue"), Some(json!({}))),
        (Method::POST, path("/switch"), Some(json!({}))),
        (Method::POST, path("/start-now"), None),
        (Method::POST, path("/archive"), None),
        (Method::POST, path("/restore"), None),
        (Method::DELETE, path(""), None),
        (
            Method::POST,
            path("/queued-messages"),
            Some(json!({"text": "also this"})),
        ),
        (
            Method::POST,
            path("/held-prompts/h1"),
            Some(json!({"action": "dismiss"})),
        ),
        (Method::POST, path("/delivery/pr"), Some(json!({}))),
        (Method::POST, path("/delivery/merge"), None),
        (
            Method::POST,
            path("/attachments?name=notes.txt"),
            Some(json!("notes")),
        ),
    ]
}

/// R3, AC3: a person who may work in the project but does not own the task changes nothing of
/// it, and is told whose it is; the owner passes the ownership check.
#[tokio::test]
async fn only_the_owner_writes_to_a_task() -> TestResult {
    let f = fixture().await?;
    let id = f.task.id.as_str();
    for (method, path, body) in writes(id) {
        let refused = call(&f.h.router, method.clone(), &path, body, &f.bob).await?;
        assert_eq!(refused.status, 403, "{method} {path}: {}", refused.body);
        let message = refused.body["error"]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(
            message.contains("@alice") && message.contains("control"),
            "{path}: {message}"
        );
    }
    let task = f.h.store.task(&f.task.id).await?.ok_or("task")?;
    assert_eq!(task, f.task, "nothing changed");
    // The owner is not refused for ownership: a write that does not fit the task's state is a
    // conflict, not a refusal.
    for rest in ["/start-now", "/queued-messages"] {
        let tried = call(
            &f.h.router,
            Method::POST,
            &format!("/api/tasks/{id}{rest}"),
            Some(json!({"text": "also this"})),
            &f.alice,
        )
        .await?;
        assert_eq!(tried.status, 409, "{rest}: {}", tried.body);
    }
    Ok(())
}

/// R3: the terminal opens read-only for a non-owner, and their input is refused.
#[tokio::test]
async fn a_non_owner_watches_the_terminal_read_only() -> TestResult {
    let f = fixture().await?;
    let (alice, bob) = (auth(&f.h, "u-a").await?, auth(&f.h, "u-b").await?);
    assert!(f.h.app.can_work(&alice, &f.task.id).await);
    assert!(!f.h.app.can_work(&bob, &f.task.id).await);
    let watching = f.h.app.terminal_open(&bob, id_of(&f), None).await?;
    assert!(!watching.can_write);
    let refused =
        f.h.app
            .terminal_input(&bob, &watching, b"rm -rf .\r", false)
            .await;
    assert!(refused.is_err(), "{refused:?}");
    Ok(())
}

fn id_of(f: &Fixture) -> &str {
    f.task.id.as_str()
}

/// R3: an agent acting for a person (here a task of Bob's) cannot stop, archive or delete a
/// task Bob does not own.
#[tokio::test]
async fn an_agent_changes_only_its_persons_tasks() -> TestResult {
    let f = fixture().await?;
    let mut bobs = f.task.clone();
    bobs.id = TaskId::parse("t-bob")?;
    bobs.created_by = UserId::parse("u-b")?;
    bobs.owner = UserId::parse("u-b")?;
    f.h.store.insert_task(&bobs).await?;
    seed_live_attempt(&f.h, "t-bob", "tok-bob").await?;
    for tool in ["task_stop", "task_archive", "task_delete"] {
        let refused = mcp_tool(&f.h, "tok-bob", tool, json!({"task_id": id_of(&f)})).await?;
        assert_eq!(refused["isError"], true, "{tool}: {refused}");
        assert!(
            tool_text(&refused).to_string().contains("@alice"),
            "{tool}: {refused}"
        );
    }
    let task = f.h.store.task(&f.task.id).await?.ok_or("task")?;
    assert_eq!(task, f.task, "nothing changed");
    Ok(())
}
