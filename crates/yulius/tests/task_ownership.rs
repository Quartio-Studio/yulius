//! SPEC-070: a task is owned by one person, whose GitHub credentials it uses. Only the owner
//! changes the task; anyone else who may work in the project asks the owner for control, and a
//! hand-over moves the task's GitHub identity to the new owner.

use axum::http::Method;
use serde_json::json;
use yulius_app::auth::AuthContext;
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::{PermissionStore as _, TaskStore as _, UserStore as _};
use yulius_app::ports::store_ext::AutomationStore as _;
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

/// Whether a terminal input was refused for want of write access (not, say, because no run is
/// attached: this fixture's live attempt has no process).
fn forbidden(result: &Result<(), yulius_app::error::AppError>) -> bool {
    matches!(result, Err(yulius_app::error::AppError::Forbidden(_)))
}

/// SPEC-118 KEY-002: the typist's write grant spares each keystroke the permission lookup, yet a
/// hand-over refuses the previous owner's very next keystroke.
#[tokio::test]
async fn a_hand_over_refuses_the_previous_owners_next_keystroke() -> TestResult {
    let f = fixture().await?;
    let (alice, bob) = (auth(&f.h, "u-a").await?, auth(&f.h, "u-b").await?);
    let typing = f.h.app.terminal_open(&alice, id_of(&f), None).await?;
    assert!(typing.can_write);
    let first = f.h.app.terminal_input(&alice, &typing, b"a", false).await;
    assert!(!forbidden(&first), "{first:?}");
    assert!(
        f.h.store
            .request_control(&f.task.id, &bob.user.id, f.h.app.now())
            .await?
    );
    assert!(
        f.h.store
            .pass_control(&f.task.id, &alice.user.id, &bob.user.id, f.h.app.now())
            .await?
    );
    let after = f.h.app.terminal_input(&alice, &typing, b"b", false).await;
    assert!(forbidden(&after), "{after:?}");
    Ok(())
}

/// SPEC-118 KEY-003: a grant older than 30 s is re-confirmed beside the input. A GitHub downgrade
/// found there (the permission cache expired) refuses the keystrokes after the one that found it.
#[tokio::test]
async fn a_downgrade_found_beside_the_input_refuses_the_next_keystroke() -> TestResult {
    let f = fixture().await?;
    let alice = auth(&f.h, "u-a").await?;
    let typing = f.h.app.terminal_open(&alice, id_of(&f), None).await?;
    let first = f.h.app.terminal_input(&alice, &typing, b"a", false).await;
    assert!(!forbidden(&first), "{first:?}");

    f.h.github.set_perm("alice", "acme/repo", perms("read"));
    let before = f.h.store.access_epoch();
    f.h.clock
        .0
        .fetch_add(6 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    let finds = f.h.app.terminal_input(&alice, &typing, b"b", false).await;
    assert!(
        !forbidden(&finds),
        "the keystroke that starts the re-confirmation is not held: {finds:?}"
    );
    // The re-confirmation stores the new level from GitHub (the access epoch grows).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while f.h.store.access_epoch() == before {
        if std::time::Instant::now() > deadline {
            return Err("the grant was not re-confirmed beside the input".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let next = f.h.app.terminal_input(&alice, &typing, b"c", false).await;
    assert!(forbidden(&next), "{next:?}");
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

async fn control(
    f: &Fixture,
    method: Method,
    rest: &str,
    who: &Creds,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        method,
        &format!("/api/tasks/{}/control-request{rest}", id_of(f)),
        None,
        who,
    )
    .await
}

/// How many audit entries of task `t1` record `action`.
async fn audited(h: &Harness, action: &str) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(h.store
        .automation_audit("t1", 50)
        .await?
        .iter()
        .filter(|e| e.action == action)
        .count())
}

/// Notices the task's terminal showed since `watch` subscribed.
fn notices(watch: &mut tokio::sync::broadcast::Receiver<yulius_app::hub::HubFrame>) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let yulius_app::hub::HubFrame::Notice(text) = frame {
            seen.push(text);
        }
    }
    seen
}

/// R4, R6, AC2, AC4: Bob asks, a second person waits for none, Alice declines; Bob asks again
/// and Alice hands over: Bob owns the task, its Git acts as Bob, and Alice now only watches.
#[tokio::test]
async fn the_owner_hands_control_over_on_request() -> TestResult {
    let f = fixture().await?;
    let carol = seed_github_user(&f.h, "u-c", "carol").await?;
    f.h.github.set_perm("carol", "acme/repo", perms("write"));
    let mut watch = f.h.app.runtime(&f.task.id).hub.watch();

    // The owner has nobody to ask; nothing to accept yet.
    assert_eq!(control(&f, Method::POST, "", &f.alice).await?.status, 409);
    assert_eq!(
        control(&f, Method::POST, "/accept", &f.alice).await?.status,
        409
    );

    let asked = control(&f, Method::POST, "", &f.bob).await?;
    assert_eq!(asked.status, 200, "{}", asked.body);
    assert_eq!(asked.body["control_request"]["by"]["login"], "bob");
    assert_eq!(asked.body["owner"]["login"], "alice");
    // Asking again is harmless; Carol waits for nobody, and is told who asked.
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let second = control(&f, Method::POST, "", &carol).await?;
    assert_eq!(second.status, 409, "{}", second.body);
    assert!(second.body.to_string().contains("@bob"), "{}", second.body);
    // Only the owner hands over; neither Bob nor Carol can drop or accept another's request.
    assert_eq!(
        control(&f, Method::POST, "/accept", &f.bob).await?.status,
        403
    );
    assert_eq!(control(&f, Method::DELETE, "", &carol).await?.status, 403);

    let declined = control(&f, Method::DELETE, "", &f.alice).await?;
    assert_eq!(declined.status, 200, "{}", declined.body);
    assert_eq!(declined.body["control_request"], serde_json::Value::Null);
    assert_eq!(audited(&f.h, "task.control_declined").await?, 1);

    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let passed = control(&f, Method::POST, "/accept", &f.alice).await?;
    assert_eq!(passed.status, 200, "{}", passed.body);
    assert_eq!(passed.body["owner"]["login"], "bob");
    assert_eq!(passed.body["created_by"]["login"], "alice");
    assert_eq!(passed.body["control_request"], serde_json::Value::Null);
    assert_eq!(git_credential(&f.h, "tt").await?, "token-bob");
    assert_eq!(audited(&f.h, "task.control_requested").await?, 2);
    assert_eq!(audited(&f.h, "task.control_passed").await?, 1);
    let said = notices(&mut watch);
    assert!(
        said.iter()
            .any(|n| n.contains("from @alice to @bob") && n.contains("acts on GitHub as @bob")),
        "{said:?}"
    );

    // Alice is now refused like anyone else; Bob writes.
    let stop = format!("/api/tasks/{}/stop", id_of(&f));
    let refused = call(&f.h.router, Method::POST, &stop, None, &f.alice).await?;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("@bob"),
        "{}",
        refused.body
    );
    let (alice, bob) = (auth(&f.h, "u-a").await?, auth(&f.h, "u-b").await?);
    assert!(!f.h.app.can_work(&alice, &f.task.id).await);
    assert!(f.h.app.can_work(&bob, &f.task.id).await);
    let stopped = call(&f.h.router, Method::POST, &stop, None, &f.bob).await?;
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    Ok(())
}

/// R4: the person who asked may withdraw; a request then waits no more.
#[tokio::test]
async fn the_requester_withdraws() -> TestResult {
    let f = fixture().await?;
    assert_eq!(control(&f, Method::DELETE, "", &f.bob).await?.status, 409);
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let withdrawn = control(&f, Method::DELETE, "", &f.bob).await?;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    assert_eq!(withdrawn.body["control_request"], serde_json::Value::Null);
    assert_eq!(audited(&f.h, "task.control_withdrawn").await?, 1);
    assert_eq!(
        control(&f, Method::POST, "/accept", &f.alice).await?.status,
        409
    );
    Ok(())
}

/// R5, AC5: with the owner gone, a request passes control at once.
#[tokio::test]
async fn an_absent_owners_task_passes_at_once() -> TestResult {
    let f = fixture().await?;
    f.h.store
        .update_user_flags(&UserId::parse("u-a")?, Some(Some(f.h.app.now())), None)
        .await?;
    let taken = control(&f, Method::POST, "", &f.bob).await?;
    assert_eq!(taken.status, 200, "{}", taken.body);
    assert_eq!(taken.body["owner"]["login"], "bob");
    assert_eq!(git_credential(&f.h, "tt").await?, "token-bob");

    // An owner who lost write access to the project counts as absent too.
    let g = fixture().await?;
    g.h.github.set_perm("alice", "acme/repo", perms("read"));
    g.h.store
        .put_access(
            &UserId::parse("u-a")?,
            &g.task.sandbox_id,
            yulius_domain::permission::AccessLevel::Read,
            g.h.app.now(),
        )
        .await?;
    let taken = control(&g, Method::POST, "", &g.bob).await?;
    assert_eq!(taken.status, 200, "{}", taken.body);
    assert_eq!(taken.body["owner"]["login"], "bob");
    Ok(())
}

/// R4, AC6: a hand-over to someone who can no longer work in the project is refused and drops
/// their request; a person who may only read cannot ask.
#[tokio::test]
async fn control_passes_only_to_someone_who_may_work() -> TestResult {
    let f = fixture().await?;
    let dave = seed_github_user(&f.h, "u-d", "dave").await?;
    f.h.github.set_perm("dave", "acme/repo", perms("read"));
    assert_eq!(control(&f, Method::POST, "", &dave).await?.status, 403);

    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    f.h.github.set_perm("bob", "acme/repo", perms("read"));
    f.h.store
        .put_access(
            &UserId::parse("u-b")?,
            &f.task.sandbox_id,
            yulius_domain::permission::AccessLevel::Read,
            f.h.app.now(),
        )
        .await?;
    let refused = control(&f, Method::POST, "/accept", &f.alice).await?;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("@bob"),
        "{}",
        refused.body
    );
    let task = f.h.store.task(&f.task.id).await?.ok_or("task")?;
    assert_eq!(task.owner.as_str(), "u-a");
    assert_eq!(f.h.store.control_request(&f.task.id).await?, None);
    Ok(())
}

/// R6, AC2: after a hand-over, the task's worktree commits as the new owner.
#[tokio::test]
async fn a_hand_over_moves_the_commit_identity() -> TestResult {
    let h = crate::support::harness_with(crate::support::HarnessOptions {
        git: Some(std::sync::Arc::new(yulius::wiring::git::GitAdapter::new(
            "git",
        ))),
        ..crate::support::HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-a", "alice").await?;
    let bob = seed_github_user(&h, "u-b", "bob").await?;
    seed_sandbox(&h, "s1", "repo", "u-a").await?;
    h.github.set_perm("alice", "acme/repo", perms("write"));
    h.github.set_perm("bob", "acme/repo", perms("write"));
    let worktree = tempfile::tempdir()?;
    crate::support::repos::git(worktree.path(), &["init", "-q"])?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-a",
        TaskStatus::Closed,
        Some(worktree.path()),
    )
    .await?;
    for (rest, who) in [("", &bob), ("/accept", &alice)] {
        let r = call(
            &h.router,
            Method::POST,
            &format!("/api/tasks/t1/control-request{rest}"),
            None,
            who,
        )
        .await?;
        assert_eq!(r.status, 200, "{rest}: {}", r.body);
    }
    let config =
        |key: &str| crate::support::repos::git(worktree.path(), &["config", "--worktree", key]);
    assert_eq!(config("user.name")?.trim(), "BOB");
    assert_eq!(config("user.email")?.trim(), "bob@users.noreply.github.com");
    Ok(())
}

/// R3 (review): the task's worktree is the owner's too: a non-owner reads its files but cannot
/// write, create, delete or upload there, nor open a shell in it; the owner can.
#[tokio::test]
async fn a_non_owner_cannot_write_into_the_task_worktree() -> TestResult {
    let f = fixture().await?;
    let worktree = f.h.app.config.data_dir.join("sandboxes/s1/worktrees/t-wt");
    std::fs::create_dir_all(&worktree)?;
    std::fs::write(worktree.join("README.md"), "# hi\n")?;
    seed_task(
        &f.h,
        "t-wt",
        "s1",
        "u-a",
        TaskStatus::Closed,
        Some(&worktree),
    )
    .await?;
    let root = "task:t-wt";

    let read = call(
        &f.h.router,
        Method::GET,
        &format!("/api/sandboxes/s1/files/content?root={root}&path=README.md"),
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(read.status, 200, "{}", read.body);
    let writes = [
        (
            Method::PUT,
            "/api/sandboxes/s1/files/content".to_owned(),
            Some(json!({"root": root, "path": "README.md", "content": "# bob\n"})),
        ),
        (
            Method::POST,
            "/api/sandboxes/s1/files".to_owned(),
            Some(json!({"root": root, "path": "new.txt", "kind": "file"})),
        ),
        (
            Method::POST,
            "/api/sandboxes/s1/files".to_owned(),
            Some(json!({"root": root, "path": "dir", "kind": "dir"})),
        ),
        (
            Method::DELETE,
            format!("/api/sandboxes/s1/files?root={root}&path=README.md"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/sandboxes/s1/files/upload?root={root}&path=up.txt"),
            Some(json!("data")),
        ),
        (
            Method::POST,
            "/api/sandboxes/s1/shells".to_owned(),
            Some(json!({"task_id": "t-wt"})),
        ),
    ];
    for (method, path, body) in writes {
        let refused = call(&f.h.router, method.clone(), &path, body, &f.bob).await?;
        assert_eq!(refused.status, 403, "{method} {path}: {}", refused.body);
        assert!(
            refused.body.to_string().contains("@alice"),
            "{path}: {}",
            refused.body
        );
    }
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md"))?,
        "# hi\n"
    );
    assert!(!worktree.join("new.txt").exists() && !worktree.join("up.txt").exists());
    let created = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/files",
        Some(json!({"root": root, "path": "alice.txt", "kind": "file"})),
        &f.alice,
    )
    .await?;
    assert_eq!(created.status, 201, "{}", created.body);
    Ok(())
}

async fn disable(h: &Harness, id: &str) -> TestResult {
    h.store
        .update_user_flags(&UserId::parse(id)?, Some(Some(h.app.now())), None)
        .await?;
    Ok(())
}

/// R5 (review): a request that waits never locks a task. When the owner becomes absent, asking
/// again passes control whatever request waits; a waiting requester who can no longer work is
/// replaced by the next person who asks.
#[tokio::test]
async fn a_waiting_request_never_locks_the_task() -> TestResult {
    let f = fixture().await?;

    // Bob waits; Alice is then disabled: Bob asking again takes control.
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    disable(&f.h, "u-a").await?;
    let again = control(&f, Method::POST, "", &f.bob).await?;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.body["owner"]["login"], "bob");

    // Bob waits, then Alice and Bob are both disabled: Carol takes control past his request.
    let g = fixture().await?;
    let carol = seed_github_user(&g.h, "u-c", "carol").await?;
    g.h.github.set_perm("carol", "acme/repo", perms("write"));
    assert_eq!(control(&g, Method::POST, "", &g.bob).await?.status, 200);
    disable(&g.h, "u-a").await?;
    disable(&g.h, "u-b").await?;
    let taken = control(&g, Method::POST, "", &carol).await?;
    assert_eq!(taken.status, 200, "{}", taken.body);
    assert_eq!(taken.body["owner"]["login"], "carol");
    assert_eq!(taken.body["control_request"], serde_json::Value::Null);
    Ok(())
}

/// R4 (review): with the owner present, a waiting requester who can no longer work no longer
/// blocks others; a disabled requester's request cannot be accepted and is dropped, audited.
#[tokio::test]
async fn a_requester_who_cannot_work_gives_way() -> TestResult {
    let f = fixture().await?;
    let carol = seed_github_user(&f.h, "u-c", "carol").await?;
    f.h.github.set_perm("carol", "acme/repo", perms("write"));

    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    disable(&f.h, "u-b").await?;
    let replaced = control(&f, Method::POST, "", &carol).await?;
    assert_eq!(replaced.status, 200, "{}", replaced.body);
    assert_eq!(replaced.body["control_request"]["by"]["login"], "carol");
    assert_eq!(replaced.body["owner"]["login"], "alice");

    disable(&f.h, "u-c").await?;
    let refused = control(&f, Method::POST, "/accept", &f.alice).await?;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(f.h.store.control_request(&f.task.id).await?, None);
    assert_eq!(audited(&f.h, "task.control_dropped").await?, 2);
    Ok(())
}

/// AC2 (review): after a hand-over the agent's `gh` (a credential asked without a repository
/// path) is the new owner's too.
#[tokio::test]
async fn the_agents_gh_follows_the_owner() -> TestResult {
    let f = fixture().await?;
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    assert_eq!(
        control(&f, Method::POST, "/accept", &f.alice).await?.status,
        200
    );
    let gh = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com"})),
        &Creds {
            bearer: Some("tt".into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(gh.status, 200, "{}", gh.body);
    assert_eq!(gh.body["token"], "token-bob");
    Ok(())
}

/// R4 (review round 2): when GitHub cannot tell whether the asker may still work, their request
/// is neither dropped nor accepted: the hand-over fails as unreachable and the request waits.
#[tokio::test]
async fn an_unknown_access_keeps_the_request_waiting() -> TestResult {
    let h = harness()?;
    crate::support::setup_superadmin_creds(&h).await?;
    let admin = crate::support::superadmin_bearer(&h).await?;
    let root = h.store.superadmin().await?.ok_or("superadmin")?.id;
    let bob = seed_github_user(&h, "u-b", "bob").await?;
    seed_sandbox(&h, "s1", "repo", root.as_str()).await?;
    h.github.set_perm("bob", "acme/repo", perms("write"));
    seed_task(&h, "t1", "s1", root.as_str(), TaskStatus::Running, None).await?;
    let path = "/api/tasks/t1/control-request";
    assert_eq!(
        call(&h.router, Method::POST, path, None, &bob)
            .await?
            .status,
        200
    );

    // Bob's cached access is past its staleness bound and GitHub fails.
    h.clock
        .0
        .fetch_add(31 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    h.github
        .fail_permissions
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let accept = call(
        &h.router,
        Method::POST,
        &format!("{path}/accept"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(accept.status, 502, "{}", accept.body);
    let waiting = h
        .store
        .control_request(&TaskId::parse("t1")?)
        .await?
        .ok_or("the request was dropped")?;
    assert_eq!(waiting.by.as_str(), "u-b");
    let task = h.store.task(&TaskId::parse("t1")?).await?.ok_or("task")?;
    assert_eq!(task.owner, root);
    Ok(())
}

/// R4 (review round 3): an asker whose GitHub sign-in can no longer be renewed cannot work; the
/// owner's hand-over drops their request (409), never answering 401, which would sign the owner
/// out.
#[tokio::test]
async fn an_asker_signed_out_of_github_gives_way() -> TestResult {
    use yulius_app::model::GitHubTokenRecord;
    let f = fixture().await?;
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    // Bob's GitHub token expires and cannot be refreshed; his cached access is stale.
    let now = f.h.app.now();
    f.h.store
        .save_github_token(
            &GitHubTokenRecord {
                user_id: UserId::parse("u-b")?,
                access: f.h.app.deps.crypto.seal(b"token-bob")?,
                refresh: None,
                expires_at: Some(now.plus_millis(10 * 60 * 1000)),
                refresh_expires_at: None,
            },
            now,
        )
        .await?;
    f.h.clock
        .0
        .fetch_add(31 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    let refused = control(&f, Method::POST, "/accept", &f.alice).await?;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(f.h.store.control_request(&f.task.id).await?, None);
    Ok(())
}
