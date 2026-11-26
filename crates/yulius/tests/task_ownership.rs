//! A task is owned by one person, whose GitHub credentials it uses. Only the
//! owner steers the task; anyone else who may work in the project stops, archives and deletes
//! it, and asks the owner for control, and a hand-over moves the task's GitHub identity to the
//! new owner.

use axum::http::Method;
use serde_json::json;
use yulius_app::auth::AuthContext;
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::{
    InstanceStore as _, PermissionStore as _, TaskStore as _, UserStore as _,
};
use yulius_app::ports::store_ext::{AutomationStore as _, NeedsYouStore as _};
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::task::TaskStatus;

use crate::support::{
    Creds, Harness, call, harness, mcp_tool, perms, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A task that rests: stopped by a person.
const RESTING: TaskStatus = TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled);

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

/// Who the task with live run token `token` acts as on GitHub: the login whose token opened the
/// issue its agent asked Yulius for (Yulius's own GitHub calls carry the owner's token; the
/// agent's shell never does).
async fn github_actor(h: &Harness, token: &str) -> Result<String, Box<dyn std::error::Error>> {
    let result = mcp_tool(
        h,
        token,
        "issue_create",
        json!({
            "title": "Flaky landing test",
            "body": "Fails 1 in 20 runs.\n\n## Acceptance criteria\n- [ ] Passes 100 runs",
            "labels": ["type: bug", "priority: medium", "area: frontend"]
        }),
    )
    .await?;
    assert_eq!(result["isError"], false, "{result}");
    Ok(h.github
        .actors_of("create_issue")
        .pop()
        .ok_or("no issue opened")?)
}

/// A new task's owner is its creator, and its summary names them.
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
    assert_eq!(github_actor(&f.h, "tt").await?, "alice");
    let _ = &f.alice;
    Ok(())
}

/// What a task does on GitHub uses its owner's token, not its creator's.
#[tokio::test]
async fn a_task_acts_on_github_as_its_owner() -> TestResult {
    let f = fixture().await?;
    let mut owned = f.task.clone();
    owned.id = TaskId::parse("t2")?;
    owned.owner = UserId::parse("u-b")?;
    f.h.store.insert_task(&owned).await?;
    seed_live_attempt(&f.h, "t2", "tt2").await?;
    assert_eq!(github_actor(&f.h, "tt2").await?, "bob");
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
        token: None,
    })
}

/// The ways the web API steers a task, each with a body that passes its
/// validation.
fn steering(task: &str) -> Vec<(Method, String, Option<serde_json::Value>)> {
    let path = |rest: &str| format!("/api/tasks/{task}{rest}");
    vec![
        (Method::POST, path("/continue"), Some(json!({}))),
        (Method::POST, path("/switch"), Some(json!({}))),
        (Method::POST, path("/pause"), None),
        (Method::POST, path("/resume"), None),
        (Method::POST, path("/start-now"), None),
        (
            Method::PUT,
            path("/schedule"),
            Some(json!({"start_at": null})),
        ),
        (Method::PUT, path("/start"), Some(json!({}))),
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
        (Method::POST, path("/control-request/accept"), None),
    ]
}

/// What the refusal of control says: whose the task is, how
/// to get control, and what is open to everyone.
fn says_whose_and_how(message: &str) -> bool {
    message.contains("@alice")
        && message.contains("only they steer it")
        && message.contains("Take control on the task's page")
        && message.contains("You can still stop, archive or delete it")
}

/// A person who may work in the project but does not own
/// the task steers nothing of it, and is told whose it is, how to ask and what they may do; an
/// instance administrator is refused the same; the owner passes the ownership check.
#[tokio::test]
async fn only_the_owner_steers_a_task() -> TestResult {
    let f = fixture().await?;
    let id = f.task.id.as_str();
    let admin = seed_github_user(&f.h, "u-admin", "ada").await?;
    f.h.github.set_perm("ada", "acme/repo", perms("admin"));
    f.h.store
        .update_user_flags(&UserId::parse("u-admin")?, None, Some(true))
        .await?;
    let superadmin = crate::support::setup_superadmin_creds(&f.h).await?;
    for who in [&f.bob, &admin, &superadmin] {
        for (method, path, body) in steering(id) {
            let refused = call(&f.h.router, method.clone(), &path, body, who).await?;
            assert_eq!(refused.status, 403, "{method} {path}: {}", refused.body);
            let message = refused.body["error"]["message"]
                .as_str()
                .unwrap_or_default();
            assert!(says_whose_and_how(message), "{path}: {message}");
        }
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

/// One call of a task route by `who`.
async fn task_call(
    f: &Fixture,
    method: Method,
    rest: &str,
    who: &Creds,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        method,
        &format!("/api/tasks/{}{rest}", id_of(f)),
        None,
        who,
    )
    .await
}

/// A person who may work in the project stops, archives, restores and
/// deletes a task they do not own, with the checks these have for its owner (a running task is
/// not archived or deleted before it is stopped).
#[tokio::test]
async fn anyone_who_may_work_tidies_another_persons_task() -> TestResult {
    let f = fixture().await?;
    // Running: archive and delete say to stop it first, as they tell its owner.
    for (method, rest) in [(Method::POST, "/archive"), (Method::DELETE, "")] {
        let early = task_call(&f, method, rest, &f.bob).await?;
        assert_eq!(early.status, 409, "{rest}: {}", early.body);
        assert!(early.body.to_string().contains("stop the task"), "{rest}");
    }
    let stopped = task_call(&f, Method::POST, "/stop", &f.bob).await?;
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    // The fixture's run has no process to end: the stop is under way.
    assert_eq!(stopped.body["status"], "stopping");
    assert_eq!(stopped.body["owner"]["login"], "alice", "still Alice's");

    // A task of Alice's that rests: archived, restored and deleted by Bob.
    seed_task(&f.h, "t-rest", "s1", "u-a", RESTING, None).await?;
    let resting = |rest: &str| format!("/api/tasks/t-rest{rest}");
    let archived = call(
        &f.h.router,
        Method::POST,
        &resting("/archive"),
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(archived.status, 200, "{}", archived.body);
    assert_eq!(archived.body["archived"], true);
    let restored = call(
        &f.h.router,
        Method::POST,
        &resting("/restore"),
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(restored.status, 200, "{}", restored.body);
    assert_eq!(restored.body["archived"], false);
    assert_eq!(restored.body["owner"]["login"], "alice", "still Alice's");
    let deleted = call(&f.h.router, Method::DELETE, &resting(""), None, &f.bob).await?;
    assert!(deleted.status.is_success(), "{}", deleted.body);
    assert_eq!(f.h.store.task(&TaskId::parse("t-rest")?).await?, None);
    Ok(())
}

/// A stop, an archive and a restore of a task by someone who does not
/// own it are audited under their login with the owner's, and the stop is said in the task's
/// terminal; the owner's own page actions leave no such entry and say nothing.
#[tokio::test]
async fn a_tidy_by_someone_else_names_them_and_the_owner() -> TestResult {
    let f = fixture().await?;
    let mut watch = f.h.app.runtime(&f.task.id).hub.watch();
    let entry = |action: &'static str, task: &'static str| {
        let store = f.h.store.clone();
        async move {
            let found = store
                .automation_audit(task, 50)
                .await?
                .into_iter()
                .find(|e| e.action == action);
            Ok::<_, Box<dyn std::error::Error>>(found)
        }
    };
    assert_eq!(
        task_call(&f, Method::POST, "/stop", &f.bob).await?.status,
        200
    );
    let stopped = entry("task.stopped", "t1")
        .await?
        .ok_or("no stop audited")?;
    assert_eq!(stopped.actor, "bob");
    let detail: serde_json::Value =
        serde_json::from_str(stopped.detail.as_deref().unwrap_or_default())?;
    assert_eq!(detail["owner"], "alice", "{detail}");
    assert!(
        detail["task"].as_str().is_some_and(|t| t.starts_with('#')),
        "{detail}"
    );
    let said = notices(&mut watch);
    assert!(said.iter().any(|n| n == "Stopped by @bob."), "{said:?}");

    seed_task(&f.h, "t-rest", "s1", "u-a", RESTING, None).await?;
    for (rest, action) in [
        ("/archive", "task.archived"),
        ("/restore", "task.unarchived"),
    ] {
        let path = format!("/api/tasks/t-rest{rest}");
        let done = call(&f.h.router, Method::POST, &path, None, &f.bob).await?;
        assert_eq!(done.status, 200, "{rest}: {}", done.body);
        let audited = entry(action, "t-rest").await?.ok_or("not audited")?;
        assert_eq!(audited.actor, "bob", "{rest}");
        assert!(
            audited
                .detail
                .as_deref()
                .is_some_and(|d| d.contains(r#""owner":"alice""#)),
            "{rest}: {:?}",
            audited.detail
        );
    }

    // The owner's own stop and archive: as before, no entry and no notice.
    seed_task(&f.h, "t-own", "s1", "u-a", TaskStatus::Queued, None).await?;
    let mut own = f.h.app.runtime(&TaskId::parse("t-own")?).hub.watch();
    for rest in ["/stop", "/archive"] {
        let path = format!("/api/tasks/t-own{rest}");
        let done = call(&f.h.router, Method::POST, &path, None, &f.alice).await?;
        assert_eq!(done.status, 200, "{rest}: {}", done.body);
    }
    assert_eq!(f.h.store.automation_audit("t-own", 50).await?.len(), 0);
    assert!(notices(&mut own).is_empty());

    // A deletion is audited for everyone; someone else's names the owner too.
    let deleted = call(
        &f.h.router,
        Method::DELETE,
        "/api/tasks/t-rest",
        None,
        &f.bob,
    )
    .await?;
    assert!(deleted.status.is_success(), "{}", deleted.body);
    let audited = entry("task.deleted", "t-rest")
        .await?
        .ok_or("not audited")?;
    assert!(
        audited
            .detail
            .as_deref()
            .is_some_and(|d| d.contains(r#""owner":"alice""#)),
        "{:?}",
        audited.detail
    );
    Ok(())
}

/// A task that waits to start is cancelled by anyone who may work.
#[tokio::test]
async fn anyone_who_may_work_cancels_a_task_that_waits() -> TestResult {
    let f = fixture().await?;
    seed_task(&f.h, "t-q", "s1", "u-a", TaskStatus::Queued, None).await?;
    let cancelled = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t-q/stop",
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    assert_eq!(cancelled.body["status"], "stopped_cancelled");
    Ok(())
}

/// Tidying needs write access. A reader is refused (403) and a person
/// without access to the project does not see the task (404); nothing changes.
#[tokio::test]
async fn a_reader_tidies_nothing() -> TestResult {
    let f = fixture().await?;
    let reader = seed_github_user(&f.h, "u-r", "rita").await?;
    f.h.github.set_perm("rita", "acme/repo", perms("read"));
    let stranger = seed_github_user(&f.h, "u-s", "sam").await?;
    for (method, rest) in [
        (Method::POST, "/stop"),
        (Method::POST, "/archive"),
        (Method::POST, "/restore"),
        (Method::DELETE, ""),
    ] {
        let refused = task_call(&f, method.clone(), rest, &reader).await?;
        assert_eq!(refused.status, 403, "{method} {rest}: {}", refused.body);
        let unseen = task_call(&f, method.clone(), rest, &stranger).await?;
        assert_eq!(unseen.status, 404, "{method} {rest}: {}", unseen.body);
    }
    let task = f.h.store.task(&f.task.id).await?.ok_or("task")?;
    assert_eq!(task, f.task, "nothing changed");
    Ok(())
}

/// The terminal opens read-only for a non-owner, and their input is refused.
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

/// The typist's write grant spares each keystroke the permission lookup, yet a
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

/// Disabling the typist refuses their next keystroke, although the
/// viewer's own copy of the user (taken when the socket opened) is not disabled.
#[tokio::test]
async fn disabling_the_typist_refuses_their_next_keystroke() -> TestResult {
    let f = fixture().await?;
    let alice = auth(&f.h, "u-a").await?;
    let typing = f.h.app.terminal_open(&alice, id_of(&f), None).await?;
    let first = f.h.app.terminal_input(&alice, &typing, b"a", false).await;
    assert!(!forbidden(&first), "{first:?}");
    f.h.store
        .update_user_flags(&alice.user.id, Some(Some(f.h.app.now())), None)
        .await?;
    let after = f.h.app.terminal_input(&alice, &typing, b"b", false).await;
    assert!(forbidden(&after), "{after:?}");
    Ok(())
}

/// Signing out (the web session revoked) refuses the next keystroke of
/// a terminal opened with that session.
#[tokio::test]
async fn signing_out_refuses_the_next_keystroke() -> TestResult {
    use yulius_app::model::WebSessionRecord;
    use yulius_app::ports::store::SessionStore as _;
    let f = fixture().await?;
    let mut alice = auth(&f.h, "u-a").await?;
    let now = f.h.app.now();
    f.h.store
        .insert_web_session(&WebSessionRecord {
            id_hash: "alice-session".into(),
            user_id: alice.user.id.clone(),
            csrf_token: "csrf".into(),
            created_at: now,
            expires_at: now.plus_millis(60 * 60 * 1000),
            revoked_at: None,
        })
        .await?;
    alice.session_hash = Some("alice-session".into());
    let typing = f.h.app.terminal_open(&alice, id_of(&f), None).await?;
    let first = f.h.app.terminal_input(&alice, &typing, b"a", false).await;
    assert!(!forbidden(&first), "{first:?}");
    f.h.store.revoke_web_session("alice-session", now).await?;
    let after = f.h.app.terminal_input(&alice, &typing, b"b", false).await;
    assert!(forbidden(&after), "{after:?}");
    Ok(())
}

/// A grant older than 30 s is re-confirmed beside the input. A GitHub downgrade
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

/// An agent acting for a person (here a task of Bob's) stops, archives
/// and deletes a task Bob does not own, as Bob may; it steers none (a message is refused with
/// the owner's name).
#[tokio::test]
async fn an_agent_tidies_tasks_for_its_person_and_steers_only_theirs() -> TestResult {
    let f = fixture().await?;
    let mut bobs = f.task.clone();
    bobs.id = TaskId::parse("t-bob")?;
    bobs.created_by = UserId::parse("u-b")?;
    bobs.owner = UserId::parse("u-b")?;
    f.h.store.insert_task(&bobs).await?;
    seed_live_attempt(&f.h, "t-bob", "tok-bob").await?;
    let stopped = mcp_tool(&f.h, "tok-bob", "task_stop", json!({"task_id": id_of(&f)})).await?;
    assert_eq!(stopped["isError"], false, "{stopped}");
    assert_eq!(audited(&f.h, "t1", "task.stopped").await?, 1);
    // A task of Alice's that rests: archived, then deleted.
    seed_task(&f.h, "t-rest", "s1", "u-a", RESTING, None).await?;
    for (tool, action) in [
        ("task_archive", "task.archived"),
        ("task_delete", "task.deleted"),
    ] {
        let done = mcp_tool(&f.h, "tok-bob", tool, json!({"task_id": "t-rest"})).await?;
        assert_eq!(done["isError"], false, "{tool}: {done}");
        assert_eq!(audited(&f.h, "t-rest", action).await?, 1, "{tool}");
    }
    assert_eq!(f.h.store.task(&TaskId::parse("t-rest")?).await?, None);
    // Steering stays Alice's: an agent's own tools for it follow the person too.
    let update = mcp_tool(
        &f.h,
        "tok-bob",
        "task_update",
        json!({"task_id": id_of(&f), "start": "now"}),
    )
    .await?;
    assert_eq!(update["isError"], true, "{update}");
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

/// How many audit entries of task `task` record `action`.
async fn audited(
    h: &Harness,
    task: &str,
    action: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(h.store
        .automation_audit(task, 50)
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

/// Bob asks, a second person waits for none, Alice declines; Bob asks again
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
    assert_eq!(audited(&f.h, "t1", "task.control_declined").await?, 1);

    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let passed = control(&f, Method::POST, "/accept", &f.alice).await?;
    assert_eq!(passed.status, 200, "{}", passed.body);
    assert_eq!(passed.body["owner"]["login"], "bob");
    assert_eq!(passed.body["created_by"]["login"], "alice");
    assert_eq!(passed.body["control_request"], serde_json::Value::Null);
    assert_eq!(github_actor(&f.h, "tt").await?, "bob");
    assert_eq!(audited(&f.h, "t1", "task.control_requested").await?, 2);
    assert_eq!(audited(&f.h, "t1", "task.control_passed").await?, 1);
    let said = notices(&mut watch);
    assert!(
        said.iter()
            .any(|n| n.contains("from @alice to @bob") && n.contains("acts on GitHub as @bob")),
        "{said:?}"
    );

    // Alice is now refused control like anyone else; Bob steers.
    let message = format!("/api/tasks/{}/queued-messages", id_of(&f));
    let text = Some(json!({"text": "also this"}));
    let refused = call(&f.h.router, Method::POST, &message, text.clone(), &f.alice).await?;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("@bob"),
        "{}",
        refused.body
    );
    let (alice, bob) = (auth(&f.h, "u-a").await?, auth(&f.h, "u-b").await?);
    assert!(!f.h.app.can_work(&alice, &f.task.id).await);
    assert!(f.h.app.can_work(&bob, &f.task.id).await);
    let queued = call(&f.h.router, Method::POST, &message, text, &f.bob).await?;
    assert_ne!(queued.status, 403, "{}", queued.body);
    Ok(())
}

/// The person who asked may withdraw; a request then waits no more.
#[tokio::test]
async fn the_requester_withdraws() -> TestResult {
    let f = fixture().await?;
    assert_eq!(control(&f, Method::DELETE, "", &f.bob).await?.status, 409);
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let withdrawn = control(&f, Method::DELETE, "", &f.bob).await?;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    assert_eq!(withdrawn.body["control_request"], serde_json::Value::Null);
    assert_eq!(audited(&f.h, "t1", "task.control_withdrawn").await?, 1);
    assert_eq!(
        control(&f, Method::POST, "/accept", &f.alice).await?.status,
        409
    );
    Ok(())
}

/// With the owner gone, a request passes control at once.
#[tokio::test]
async fn an_absent_owners_task_passes_at_once() -> TestResult {
    let f = fixture().await?;
    f.h.store
        .update_user_flags(&UserId::parse("u-a")?, Some(Some(f.h.app.now())), None)
        .await?;
    let taken = control(&f, Method::POST, "", &f.bob).await?;
    assert_eq!(taken.status, 200, "{}", taken.body);
    assert_eq!(taken.body["owner"]["login"], "bob");
    assert_eq!(github_actor(&f.h, "tt").await?, "bob");

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

/// A hand-over to someone who can no longer work in the project is refused and drops
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

/// After a hand-over, the task's worktree commits as the new owner.
#[tokio::test]
async fn a_hand_over_moves_the_commit_identity() -> TestResult {
    let h = crate::support::harness_with(crate::support::HarnessOptions {
        git: Some(std::sync::Arc::new(crate::support::confined_git())),
        ..crate::support::HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-a", "alice").await?;
    let bob = seed_github_user(&h, "u-b", "bob").await?;
    seed_sandbox(&h, "s1", "repo", "u-a").await?;
    h.github.set_perm("alice", "acme/repo", perms("write"));
    h.github.set_perm("bob", "acme/repo", perms("write"));
    let worktree = tempfile::tempdir()?;
    crate::support::git_stdout(worktree.path(), &["init", "-q"])?;
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
        |key: &str| crate::support::git_stdout(worktree.path(), &["config", "--worktree", key]);
    assert_eq!(config("user.name")?.trim(), "BOB");
    assert_eq!(config("user.email")?.trim(), "bob@users.noreply.github.com");
    Ok(())
}

/// The task's worktree is the owner's too: a non-owner reads its files but cannot
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

/// A request that waits never locks a task. When the owner becomes absent, asking
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

/// With the owner present, a waiting requester who can no longer work no longer
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
    assert_eq!(audited(&f.h, "t1", "task.control_dropped").await?, 2);
    Ok(())
}

/// After a hand-over the agent's `gh` (a credential asked without a
/// repository path) still gets the App's agent token, never the new owner's.
#[tokio::test]
async fn the_agents_gh_never_gets_the_new_owners_token() -> TestResult {
    let f = fixture().await?;
    f.h.store.init_instance("https://yulius.test", "x").await?;
    crate::support::configure_github_app(&f.h).await?;
    f.h.store.set_installation(Some(7)).await?;
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
    assert_eq!(gh.body["token"], "agent-inst");
    Ok(())
}

/// When GitHub cannot tell whether the asker may still work, their request
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

/// An asker whose GitHub sign-in can no longer be renewed cannot work; the
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

async fn take_back(
    f: &Fixture,
    who: &Creds,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        Method::POST,
        &format!("/api/tasks/{}/control/take-back", id_of(f)),
        None,
        who,
    )
    .await
}

/// Alice hands her task to Bob on his request and takes it back
/// without asking: it is hers again, acts on GitHub as her, Bob is refused control again, and the
/// terminal and the audit say so. Nobody else takes it back, and once she has, nor does she.
#[tokio::test]
async fn the_giver_takes_a_task_back() -> TestResult {
    let f = fixture().await?;
    let carol = seed_github_user(&f.h, "u-c", "carol").await?;
    f.h.github.set_perm("carol", "acme/repo", perms("write"));
    let mut watch = f.h.app.runtime(&f.task.id).hub.watch();
    // Nobody handed it over: nothing to take back.
    assert_eq!(take_back(&f, &f.alice).await?.status, 409);
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let passed = control(&f, Method::POST, "/accept", &f.alice).await?;
    assert_eq!(
        passed.body["control_from"]["login"], "alice",
        "{}",
        passed.body
    );
    let said = notices(&mut watch);
    assert!(
        said.iter().any(|n| n.contains("@alice can take it back")),
        "{said:?}"
    );

    // Bob owns it now: nothing to take back from himself.
    let own = take_back(&f, &f.bob).await?;
    assert_eq!(own.status, 409, "{}", own.body);
    let refused = take_back(&f, &carol).await?;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("@alice"),
        "{}",
        refused.body
    );
    let mut events = f.h.app.events.subscribe();
    let back = take_back(&f, &f.alice).await?;
    assert_eq!(back.status, 200, "{}", back.body);
    // Bob, from whom it was taken, is told in his own event stream.
    let mut told = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        if let yulius_contracts::events::UiEvent::ControlTakenBack { task_id, by, .. } =
            &envelope.event
        {
            told.push((envelope.user.clone(), task_id.clone(), by.clone()));
        }
    }
    assert_eq!(
        told,
        vec![(
            Some(UserId::parse("u-b")?),
            id_of(&f).to_owned(),
            "alice".to_owned()
        )]
    );
    assert_eq!(back.body["owner"]["login"], "alice");
    assert_eq!(back.body["control_from"], serde_json::Value::Null);
    assert_eq!(github_actor(&f.h, "tt").await?, "alice");
    assert_eq!(audited(&f.h, "t1", "task.control_taken_back").await?, 1);
    let said = notices(&mut watch);
    assert!(
        said.iter()
            .any(|n| n.contains("returned to @alice")
                && n.contains("acts on GitHub as @alice again")),
        "{said:?}"
    );
    let (alice, bob) = (auth(&f.h, "u-a").await?, auth(&f.h, "u-b").await?);
    assert!(f.h.app.can_work(&alice, &f.task.id).await);
    assert!(!f.h.app.can_work(&bob, &f.task.id).await);
    assert_eq!(take_back(&f, &f.alice).await?.status, 409);
    Ok(())
}

/// The right ends when control moves again. Handed on by Bob to
/// Carol, only Bob takes it back; handed back to Alice on her request, nobody does; a giver who
/// can no longer work is refused.
#[tokio::test]
async fn taking_back_ends_when_control_moves_again() -> TestResult {
    let f = fixture().await?;
    let carol = seed_github_user(&f.h, "u-c", "carol").await?;
    f.h.github.set_perm("carol", "acme/repo", perms("write"));
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    assert_eq!(
        control(&f, Method::POST, "/accept", &f.alice).await?.status,
        200
    );
    assert_eq!(control(&f, Method::POST, "", &carol).await?.status, 200);
    let on = control(&f, Method::POST, "/accept", &f.bob).await?;
    assert_eq!(on.body["control_from"]["login"], "bob", "{}", on.body);
    assert_eq!(take_back(&f, &f.alice).await?.status, 403);
    // Bob takes it back: his own again, and nobody gave it to him. He hands it to Alice on her
    // request (he may take it back), and she hands it back to him on his: nobody may then.
    assert_eq!(take_back(&f, &f.bob).await?.status, 200);
    assert_eq!(control(&f, Method::POST, "", &f.alice).await?.status, 200);
    let to_alice = control(&f, Method::POST, "/accept", &f.bob).await?;
    assert_eq!(
        to_alice.body["control_from"]["login"], "bob",
        "{}",
        to_alice.body
    );
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let mut events = f.h.app.events.subscribe();
    let mut watch = f.h.app.runtime(&f.task.id).hub.watch();
    let returned = control(&f, Method::POST, "/accept", &f.alice).await?;
    // Its notice promises no take-back (review round 2).
    let said = notices(&mut watch);
    assert!(
        said.iter().any(|n| n.contains("from @alice to @bob"))
            && !said.iter().any(|n| n.contains("can take it back")),
        "{said:?}"
    );
    // Handed back on request: nobody took anything back.
    while let Ok(envelope) = events.try_recv() {
        assert!(
            !matches!(
                envelope.event,
                yulius_contracts::events::UiEvent::ControlTakenBack { .. }
            ),
            "{:?}",
            envelope.event
        );
    }
    assert_eq!(returned.body["owner"]["login"], "bob", "{}", returned.body);
    assert_eq!(returned.body["control_from"], serde_json::Value::Null);
    assert_eq!(take_back(&f, &f.alice).await?.status, 409);

    // Bob hands it to Alice again; a giver who can no longer work takes nothing back.
    assert_eq!(control(&f, Method::POST, "", &f.alice).await?.status, 200);
    assert_eq!(
        control(&f, Method::POST, "/accept", &f.bob).await?.status,
        200
    );
    f.h.github.set_perm("bob", "acme/repo", perms("read"));
    f.h.store
        .put_access(
            &UserId::parse("u-b")?,
            &f.task.sandbox_id,
            yulius_domain::permission::AccessLevel::Read,
            f.h.app.now(),
        )
        .await?;
    assert_eq!(take_back(&f, &f.bob).await?.status, 403);
    let task = f.h.store.task(&f.task.id).await?.ok_or("task")?;
    assert_eq!(task.owner.as_str(), "u-a");
    Ok(())
}

async fn take(
    f: &Fixture,
    who: &Creds,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        Method::POST,
        &format!("/api/tasks/{}/control/take", id_of(f)),
        None,
        who,
    )
    .await
}

async fn ask_first(
    h: &Harness,
    who: &Creds,
    on: bool,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::PUT,
        "/api/me/control",
        Some(json!({"ask_first": on})),
        who,
    )
    .await
}

/// Bob takes Alice's task without asking. It acts on GitHub as
/// him, Carol's waiting request is dropped, Alice is told wherever she is and takes it back, and
/// the terminal and the audit say so.
#[tokio::test]
async fn a_teammate_takes_control_at_once() -> TestResult {
    let f = fixture().await?;
    let carol = seed_github_user(&f.h, "u-c", "carol").await?;
    f.h.github.set_perm("carol", "acme/repo", perms("write"));
    assert_eq!(control(&f, Method::POST, "", &carol).await?.status, 200);
    let mut watch = f.h.app.runtime(&f.task.id).hub.watch();
    let mut events = f.h.app.events.subscribe();
    let taken = take(&f, &f.bob).await?;
    assert_eq!(taken.status, 200, "{}", taken.body);
    assert_eq!(taken.body["owner"]["login"], "bob");
    assert_eq!(taken.body["control_from"]["login"], "alice");
    assert_eq!(taken.body["control_request"], serde_json::Value::Null);
    assert_eq!(taken.body["owner_asks_first"], false);
    assert_eq!(github_actor(&f.h, "tt").await?, "bob");
    assert_eq!(audited(&f.h, "t1", "task.control_taken").await?, 1);
    let said = notices(&mut watch);
    assert!(
        said.iter()
            .any(|n| n.contains("@bob took control of this task from @alice")
                && n.contains("acts on GitHub as @bob")
                && n.contains("@alice can take it back")),
        "{said:?}"
    );
    let mut told = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        if let yulius_contracts::events::UiEvent::ControlTaken { task_id, by, .. } = &envelope.event
        {
            told.push((envelope.user.clone(), task_id.clone(), by.clone()));
        }
    }
    assert_eq!(
        told,
        vec![(
            Some(UserId::parse("u-a")?),
            id_of(&f).to_owned(),
            "bob".to_owned()
        )]
    );
    let (alice, bob) = (auth(&f.h, "u-a").await?, auth(&f.h, "u-b").await?);
    assert!(f.h.app.can_work(&bob, &f.task.id).await);
    assert!(!f.h.app.can_work(&alice, &f.task.id).await);
    let back = take_back(&f, &f.alice).await?;
    assert_eq!(back.status, 200, "{}", back.body);
    assert_eq!(github_actor(&f.h, "tt").await?, "alice");
    Ok(())
}

/// Alice chooses to be asked. Bob's take is refused naming her and the
/// task stays hers; his request waits for her answer. The choice is hers alone and is read back
/// from `/api/me`.
#[tokio::test]
async fn an_owner_who_asks_first_is_asked() -> TestResult {
    let f = fixture().await?;
    let me = call(&f.h.router, Method::GET, "/api/me", None, &f.alice).await?;
    assert_eq!(me.body["user"]["control_ask_first"], false, "{}", me.body);
    let saved = ask_first(&f.h, &f.alice, true).await?;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(saved.body["ask_first"], true);
    let me = call(&f.h.router, Method::GET, "/api/me", None, &f.alice).await?;
    assert_eq!(me.body["user"]["control_ask_first"], true, "{}", me.body);
    let detail = call(
        &f.h.router,
        Method::GET,
        &format!("/api/tasks/{}", id_of(&f)),
        None,
        &f.bob,
    )
    .await?;
    assert_eq!(detail.body["owner_asks_first"], true, "{}", detail.body);

    let refused = take(&f, &f.bob).await?;
    assert_eq!(refused.status, 409, "{}", refused.body);
    let message = refused.body["error"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("@alice") && message.contains("request control"),
        "{message}"
    );
    assert_eq!(github_actor(&f.h, "tt").await?, "alice");
    // Steering her task, Bob is told to ask her.
    let continued = call(
        &f.h.router,
        Method::POST,
        &format!("/api/tasks/{}/continue", id_of(&f)),
        Some(json!({})),
        &f.bob,
    )
    .await?;
    assert_eq!(continued.status, 403, "{}", continued.body);
    assert!(
        continued.body.to_string().contains("Ask them for control"),
        "{}",
        continued.body
    );
    assert_eq!(control(&f, Method::POST, "", &f.bob).await?.status, 200);
    let handed = control(&f, Method::POST, "/accept", &f.alice).await?;
    assert_eq!(handed.body["owner"]["login"], "bob", "{}", handed.body);
    // She changes her mind: teammates take control of her tasks again.
    assert_eq!(ask_first(&f.h, &f.alice, false).await?.status, 200);
    let me = call(&f.h.router, Method::GET, "/api/me", None, &f.alice).await?;
    assert_eq!(me.body["user"]["control_ask_first"], false, "{}", me.body);
    Ok(())
}

/// An owner who can no longer act holds no task, whatever they chose.
#[tokio::test]
async fn an_absent_owner_who_asks_first_loses_the_task_to_a_take() -> TestResult {
    let f = fixture().await?;
    assert_eq!(ask_first(&f.h, &f.alice, true).await?.status, 200);
    disable(&f.h, "u-a").await?;
    let taken = take(&f, &f.bob).await?;
    assert_eq!(taken.status, 200, "{}", taken.body);
    assert_eq!(taken.body["owner"]["login"], "bob");
    // Nobody gone takes anything back.
    assert_eq!(taken.body["control_from"], serde_json::Value::Null);
    assert_eq!(github_actor(&f.h, "tt").await?, "bob");
    Ok(())
}

/// The owner, a reader and a disabled account take nothing.
#[tokio::test]
async fn a_take_is_refused_to_whoever_may_not_take() -> TestResult {
    let f = fixture().await?;
    let own = take(&f, &f.alice).await?;
    assert_eq!(own.status, 409, "{}", own.body);
    let reader = seed_github_user(&f.h, "u-r", "rita").await?;
    f.h.github.set_perm("rita", "acme/repo", perms("read"));
    let read = take(&f, &reader).await?;
    assert!(
        read.status == 403 || read.status == 404,
        "{}: {}",
        read.status,
        read.body
    );
    disable(&f.h, "u-b").await?;
    let disabled = take(&f, &f.bob).await?;
    assert!(
        disabled.status == 401 || disabled.status == 403,
        "{}: {}",
        disabled.status,
        disabled.body
    );
    assert_eq!(github_actor(&f.h, "tt").await?, "alice");
    assert_eq!(audited(&f.h, "t1", "task.control_taken").await?, 0);
    Ok(())
}

/// Alice takes back by taking control herself. She was
/// the giver, so it is a take-back: nobody may take it back afterwards, nothing promises it, and
/// Bob, from whom it was taken, is told it was taken back.
#[tokio::test]
async fn taking_control_back_from_the_taker_leaves_no_giver() -> TestResult {
    let f = fixture().await?;
    assert_eq!(take(&f, &f.bob).await?.status, 200);
    // Bob asking first does not keep the task from whoever it was taken from.
    assert_eq!(ask_first(&f.h, &f.bob, true).await?.status, 200);
    let mut watch = f.h.app.runtime(&f.task.id).hub.watch();
    let mut events = f.h.app.events.subscribe();
    let back = take(&f, &f.alice).await?;
    assert_eq!(back.status, 200, "{}", back.body);
    assert_eq!(back.body["owner"]["login"], "alice");
    assert_eq!(back.body["control_from"], serde_json::Value::Null);
    let said = notices(&mut watch);
    assert!(
        said.iter().any(|n| n.contains("returned to @alice"))
            && !said.iter().any(|n| n.contains("can take it back")),
        "{said:?}"
    );
    assert_eq!(audited(&f.h, "t1", "task.control_taken_back").await?, 1);
    let mut told = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        use yulius_contracts::events::UiEvent;
        match &envelope.event {
            UiEvent::ControlTakenBack { by, .. } => {
                told.push((envelope.user.clone(), "back", by.clone()));
            }
            UiEvent::ControlTaken { by, .. } => {
                told.push((envelope.user.clone(), "taken", by.clone()));
            }
            _ => {}
        }
    }
    assert_eq!(
        told,
        vec![(Some(UserId::parse("u-b")?), "back", "alice".to_owned())]
    );
    // Nobody gave it to Alice: there is nothing to take back.
    assert_eq!(take_back(&f, &f.bob).await?.status, 409);
    Ok(())
}

/// What the agent of task `id` asks a person, as the `needs_you` tool records it.
async fn ask(h: &Harness, id: &str) -> TestResult {
    let asked = yulius_app::needs_you::NeedsYouRecord {
        action: "Choose the staging region".into(),
        happened: None,
        problem: None,
        decision: true,
        link: None,
        at: 1,
        cause: None,
        named: Default::default(),
    };
    h.store
        .set_task_needs_you(&TaskId::parse(id)?, Some(&asked), h.app.now())
        .await?;
    Ok(())
}

/// `POST /api/tasks/{id}/needs-you/answer` by `who`.
async fn answer(
    h: &Harness,
    id: &str,
    who: &Creds,
    text: &str,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{id}/needs-you/answer"),
        Some(json!({"text": text})),
        who,
    )
    .await
}

/// An answer is refused, saying why, with nothing to answer, an empty
/// text, a task that does not run, an agent asking in its terminal, the owner's personal secret
/// within the run's reach, a reader and a disabled account; the request stays open and the task
/// stays its owner's.
#[tokio::test]
async fn a_needs_you_answer_is_refused_where_it_must_not_reach_the_agent() -> TestResult {
    let f = fixture().await?;
    let t1 = id_of(&f).to_owned();
    let status = |r: &crate::support::Resp| r.status;
    let says = |r: &crate::support::Resp, what: &str| {
        r.body
            .to_string()
            .to_lowercase()
            .contains(&what.to_lowercase())
    };

    let none = answer(&f.h, &t1, &f.bob, "Frankfurt").await?;
    assert_eq!(status(&none), 409, "{}", none.body);
    assert!(says(&none, "nothing to answer"), "{}", none.body);

    ask(&f.h, &t1).await?;
    let empty = answer(&f.h, &t1, &f.bob, " \u{7} ").await?;
    assert_eq!(status(&empty), 422, "{}", empty.body);
    let long = answer(&f.h, &t1, &f.bob, &"x".repeat(64 * 1024 + 1)).await?;
    assert_eq!(status(&long), 422, "{}", long.body);
    assert!(says(&long, "longer than 64 KiB"), "{}", long.body);

    seed_task(&f.h, "t2", "s1", "u-a", RESTING, None).await?;
    ask(&f.h, "t2").await?;
    let resting = answer(&f.h, "t2", &f.bob, "Frankfurt").await?;
    assert_eq!(status(&resting), 409, "{}", resting.body);
    assert!(says(&resting, "not running"), "{}", resting.body);

    f.h.store
        .set_activity(
            &f.task.id,
            Some(yulius_domain::task::Activity::NeedsAttention),
            f.h.app.now(),
        )
        .await?;
    let terminal = answer(&f.h, &t1, &f.bob, "Frankfurt").await?;
    assert_eq!(status(&terminal), 409, "{}", terminal.body);
    assert!(says(&terminal, "terminal"), "{}", terminal.body);
    f.h.store
        .set_activity(
            &f.task.id,
            Some(yulius_domain::task::Activity::Waiting),
            f.h.app.now(),
        )
        .await?;

    let carol = seed_github_user(&f.h, "u-c", "carol").await?;
    f.h.github.set_perm("carol", "acme/repo", perms("read"));
    let reader = answer(&f.h, &t1, &carol, "Frankfurt").await?;
    assert_eq!(status(&reader), 403, "{}", reader.body);

    let secret = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"scope": "personal", "name": "ALICE_TOKEN", "value": "alice-personal-71c0ffee",
            "hosts": ["api.alice.test"], "injection": "bearer"}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(status(&secret), 201, "{}", secret.body);
    let personal = answer(&f.h, &t1, &f.bob, "Frankfurt").await?;
    assert_eq!(status(&personal), 409, "{}", personal.body);
    assert!(
        says(&personal, "@alice") && says(&personal, "personal"),
        "{}",
        personal.body
    );

    // A disabled account is signed out: its session no longer authenticates.
    disable(&f.h, "u-b").await?;
    let disabled = answer(&f.h, &t1, &f.bob, "Frankfurt").await?;
    assert_eq!(status(&disabled), 401, "{}", disabled.body);

    assert!(f.h.store.task_needs_you(&f.task.id).await?.is_some());
    let task = f.h.store.task(&f.task.id).await?.ok_or("task")?;
    assert_eq!(task.owner, UserId::parse("u-a")?);
    assert_eq!(audited(&f.h, &t1, "task.needs_you.answered").await?, 0);
    Ok(())
}

/// The owner's personal MCP server for the project keeps a
/// teammate's answer out as their personal secret does.
#[tokio::test]
async fn the_owners_personal_mcp_server_keeps_a_teammates_answer_out() -> TestResult {
    let f = fixture().await?;
    let t1 = id_of(&f).to_owned();
    ask(&f.h, &t1).await?;
    let server = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(
            json!({"name": "mail", "transport": "http", "url": "https://mail.alice.test/mcp",
            "scope": "sandbox", "audience": "personal"}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(server.status, 201, "{}", server.body);
    let refused = answer(&f.h, &t1, &f.bob, "Frankfurt").await?;
    assert_eq!(refused.status, 409, "{}", refused.body);
    let said = refused.body.to_string();
    assert!(
        said.contains("@alice") && said.contains("personal"),
        "{said}"
    );
    assert!(f.h.store.task_needs_you(&f.task.id).await?.is_some());
    assert_eq!(audited(&f.h, &t1, "task.needs_you.answered").await?, 0);
    Ok(())
}

/// An automation's task holds no person's secrets or servers,
/// so its owner's personal secret does not keep a teammate's answer out: it goes on to the
/// session (here none is live, which is the refusal it meets).
#[tokio::test]
async fn an_automations_task_is_answered_whatever_its_owners_personal_secrets() -> TestResult {
    let f = fixture().await?;
    let mut automated = seed_task(&f.h, "t0", "s1", "u-a", TaskStatus::Running, None).await?;
    automated.id = TaskId::parse("t3")?;
    automated.number += 100;
    automated.origin = yulius_app::model::TaskOrigin::Automation;
    f.h.store.insert_task(&automated).await?;
    ask(&f.h, "t3").await?;
    let secret = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"scope": "personal", "name": "ALICE_TOKEN", "value": "alice-personal-71c0ffee",
            "hosts": ["api.alice.test"], "injection": "bearer"}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(secret.status, 201, "{}", secret.body);
    let reached = answer(&f.h, "t3", &f.bob, "Frankfurt").await?;
    assert_eq!(reached.status, 409, "{}", reached.body);
    let said = reached.body.to_string();
    assert!(said.contains("no live session"), "{said}");
    assert!(!said.contains("personal"), "{said}");
    // Refused where it is typed: the request stays open and nothing is audited (review round 2).
    assert!(
        f.h.store
            .task_needs_you(&TaskId::parse("t3")?)
            .await?
            .is_some()
    );
    assert_eq!(audited(&f.h, "t3", "task.needs_you.answered").await?, 0);
    Ok(())
}

/// The access check may wait on GitHub; what the task is
/// doing is decided from a read after it. The agent opens a question in its terminal meanwhile,
/// and the answer is refused for it rather than typed into the menu.
#[tokio::test]
async fn an_answer_decides_on_the_task_as_it_is_after_the_access_check() -> TestResult {
    let f = fixture().await?;
    let t1 = id_of(&f).to_owned();
    ask(&f.h, &t1).await?;
    let hold = f.h.github.hold_access();
    let request = answer(&f.h, &t1, &f.bob, "Frankfurt");
    tokio::pin!(request);
    let entered = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::select! {
            done = &mut request => Err(format!("answered while GitHub was held: {:?}",
                done.map(|r| r.body).map_err(|e| e.to_string()))),
            () = f.h.github.access_entered.notified() => Ok(()),
        }
    })
    .await?;
    entered?;
    f.h.store
        .set_activity(
            &f.task.id,
            Some(yulius_domain::task::Activity::NeedsAttention),
            f.h.app.now(),
        )
        .await?;
    f.h.github.release_access(&hold);
    let refused = request.await?;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("asks in its terminal"),
        "{}",
        refused.body
    );
    Ok(())
}

/// What `answer` tells about a call that passed every check and reached the session: this
/// fixture's run has no process, so its typing ends "no live session", or "the task is not
/// running" once something made the task's runtime.
fn reached_the_session(r: &crate::support::Resp) -> bool {
    let said = r.body.to_string();
    r.status == 409
        && (said.contains("no live session") || said.contains("the task is not running"))
}

/// `needs_you` raised by task `t1`'s agent (live run token `tt`) with `args`.
async fn agent_asks(
    h: &Harness,
    args: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    mcp_tool(h, "tt", "needs_you", args).await
}

/// The request's `at_ms` as the task's summary shows it, and whom it names.
async fn asked_of(
    f: &Fixture,
    who: &Creds,
) -> Result<(i64, Vec<String>), Box<dyn std::error::Error>> {
    let shown = call(
        &f.h.router,
        Method::GET,
        &format!("/api/tasks/{}", id_of(f)),
        None,
        who,
    )
    .await?;
    let asked = &shown.body["needs_you"];
    let at = asked["at_ms"].as_i64().ok_or("no request")?;
    let named = asked["who"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok((at, named))
}

/// A request names whom it waits for; a login without write access to
/// the project, or no person of the instance, refuses the whole request, saying which; the owner
/// named is no one extra.
#[tokio::test]
async fn a_needs_you_names_who_it_waits_for() -> TestResult {
    let f = fixture().await?;
    let carol = seed_github_user(&f.h, "u-c", "carol").await?;
    let _ = carol;
    f.h.github.set_perm("carol", "acme/repo", perms("read"));
    let reader = agent_asks(
        &f.h,
        json!({"action": "Decide M1-M6", "decision": true, "who": ["bob", "carol"]}),
    )
    .await?;
    assert_eq!(reader["isError"], true, "{reader}");
    assert!(reader.to_string().contains("@carol"), "{reader}");
    assert!(reader.to_string().contains("write access"), "{reader}");
    let nobody = agent_asks(&f.h, json!({"action": "Decide", "who": ["mallory"]})).await?;
    assert_eq!(nobody["isError"], true, "{nobody}");
    assert!(nobody.to_string().contains("@mallory"), "{nobody}");
    assert!(
        f.h.store.task_needs_you(&f.task.id).await?.is_none(),
        "a refused request records nothing"
    );
    let named = agent_asks(
        &f.h,
        json!({"action": "Decide M1-M6", "decision": true, "who": ["@Bob", "alice", "bob"]}),
    )
    .await?;
    assert_eq!(named["isError"], false, "{named}");
    let (_, who) = asked_of(&f, &f.alice).await?;
    assert_eq!(who, ["bob"], "the owner is dropped, a name counted once");
    Ok(())
}

/// A named request is answered by its owner and whom it
/// names, on the page and through `task_message`; another writer is refused with whom it asks.
/// A request that names nobody stays any writer's.
#[tokio::test]
async fn a_named_request_is_answered_by_whom_it_names() -> TestResult {
    let f = fixture().await?;
    let t1 = id_of(&f).to_owned();
    let dave = seed_github_user(&f.h, "u-d", "dave").await?;
    f.h.github.set_perm("dave", "acme/repo", perms("write"));
    // Unnamed: any writer answers.
    ask(&f.h, &t1).await?;
    assert!(reached_the_session(
        &answer(&f.h, &t1, &dave, "Frankfurt").await?
    ));
    // Named: Bob and the owner answer, Dave is refused.
    let named = agent_asks(&f.h, json!({"action": "Decide M1-M6", "who": ["bob"]})).await?;
    assert_eq!(named["isError"], false, "{named}");
    let refused = answer(&f.h, &t1, &dave, "M1: yes").await?;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("@bob"),
        "{}",
        refused.body
    );
    assert!(reached_the_session(
        &answer(&f.h, &t1, &f.bob, "M1: yes").await?
    ));
    assert!(reached_the_session(
        &answer(&f.h, &t1, &f.alice, "O6: no").await?
    ));
    // Through Bob's own agent or Superagent: the same answer.
    let caller = |user: &str, login: &str| -> Result<_, Box<dyn std::error::Error>> {
        Ok(yulius_app::coordination::ProjectCaller {
            sandbox: yulius_domain::ids::SandboxId::parse("s1")?,
            user: UserId::parse(user)?,
            task: None,
            channel: yulius_app::coordination::Channel::Superagent,
            actor: format!("cockpit:{login}"),
            detail: json!({"via": "cockpit", "shell": "sh-test"}),
        })
    };
    let by_bob =
        f.h.app
            .tool_task_message(
                &caller("u-b", "bob")?,
                &json!({"task_id": t1, "text": "M2: no"}),
            )
            .await;
    assert!(
        matches!(&by_bob, Err(yulius_app::error::AppError::Conflict(m))
            if m.contains("no live session") || m.contains("the task is not running")),
        "{by_bob:?}"
    );
    let by_dave =
        f.h.app
            .tool_task_message(
                &caller("u-d", "dave")?,
                &json!({"task_id": t1, "text": "M2: no"}),
            )
            .await;
    assert!(
        matches!(&by_dave, Err(yulius_app::error::AppError::Forbidden(m)) if m.contains("@bob")),
        "{by_dave:?}"
    );
    // With nothing asked, a message is the owner's as before.
    f.h.store
        .set_task_needs_you(&f.task.id, None, f.h.app.now())
        .await?;
    let plain =
        f.h.app
            .tool_task_message(
                &caller("u-b", "bob")?,
                &json!({"task_id": t1, "text": "go on"}),
            )
            .await;
    assert!(
        matches!(&plain, Err(yulius_app::error::AppError::Forbidden(m)) if m.contains("@alice")),
        "{plain:?}"
    );
    Ok(())
}

/// `POST /api/tasks/t1/needs-you/withdraw` of Bob by `who`.
async fn withdraw_bob(
    f: &Fixture,
    who: &Creds,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        Method::POST,
        &format!("/api/tasks/{}/needs-you/withdraw", id_of(f)),
        Some(json!({"login": "bob"})),
        who,
    )
    .await
}

/// An answer to a request that was replaced, or by a person
/// the owner withdrew, is refused saying which; only the owner withdraws, and it is audited.
#[tokio::test]
async fn an_answer_to_a_replaced_or_withdrawn_request_is_refused() -> TestResult {
    let f = fixture().await?;
    let t1 = id_of(&f).to_owned();
    // An earlier request naming Bob (asked at 1 ms, so the next one's time differs).
    let earlier = yulius_app::needs_you::NeedsYouRecord {
        action: "Decide M1".into(),
        happened: None,
        problem: None,
        decision: true,
        link: None,
        at: 1,
        cause: None,
        named: yulius_app::needs_you::Named {
            who: vec!["bob".into()],
            withdrawn: Vec::new(),
        },
    };
    f.h.store
        .set_task_needs_you(&f.task.id, Some(&earlier), f.h.app.now())
        .await?;
    let (first, _) = asked_of(&f, &f.bob).await?;
    // A new request replaces it.
    agent_asks(&f.h, json!({"action": "Decide M2", "who": ["bob"]})).await?;
    let stale = call(
        &f.h.router,
        Method::POST,
        &format!("/api/tasks/{t1}/needs-you/answer"),
        Some(json!({"text": "M1: yes", "asked_at_ms": first})),
        &f.bob,
    )
    .await?;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert!(
        stale.body.to_string().contains("replaced"),
        "{}",
        stale.body
    );
    let (current, _) = asked_of(&f, &f.bob).await?;
    let fresh = call(
        &f.h.router,
        Method::POST,
        &format!("/api/tasks/{t1}/needs-you/answer"),
        Some(json!({"text": "M2: yes", "asked_at_ms": current})),
        &f.bob,
    )
    .await?;
    assert!(reached_the_session(&fresh), "{}", fresh.body);
    // Only the owner withdraws.
    assert_eq!(withdraw_bob(&f, &f.bob).await?.status, 403);
    let withdrawn = withdraw_bob(&f, &f.alice).await?;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    let (_, named) = asked_of(&f, &f.alice).await?;
    assert!(named.is_empty(), "{named:?}");
    let refused = answer(&f.h, &t1, &f.bob, "M2: yes").await?;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("withdrew"),
        "{}",
        refused.body
    );
    assert!(reached_the_session(
        &answer(&f.h, &t1, &f.alice, "M2: yes").await?
    ));
    assert_eq!(
        audited(&f.h, &t1, "task.needs_you.withdrawn").await?,
        1,
        "the withdrawal is audited"
    );
    Ok(())
}

/// A person a request names answers it and nothing else: terminal
/// input, Continue and merges stay the owner's; a stop is anyone's who may work there.
#[tokio::test]
async fn a_named_person_steers_nothing_else() -> TestResult {
    let f = fixture().await?;
    agent_asks(&f.h, json!({"action": "Decide M1", "who": ["bob"]})).await?;
    let bob = auth(&f.h, "u-b").await?;
    let watching = f.h.app.terminal_open(&bob, id_of(&f), None).await?;
    assert!(!watching.can_write);
    let typed =
        f.h.app
            .terminal_input(&bob, &watching, b"yes\r", false)
            .await;
    assert!(typed.is_err(), "{typed:?}");
    for (method, path, body) in steering(id_of(&f)) {
        if !(path.ends_with("/continue") || path.ends_with("/delivery/merge")) {
            continue;
        }
        let refused = call(&f.h.router, method.clone(), &path, body, &f.bob).await?;
        assert_eq!(refused.status, 403, "{method} {path}: {}", refused.body);
    }
    let stop = task_call(&f, Method::POST, "/stop", &f.bob).await?;
    assert!(stop.status.is_success(), "{}", stop.body);
    Ok(())
}

/// More than ten names, or a disabled person,
/// refuse the request; a withdrawal with no request, or of a login it does not name, is 409; a
/// withdrawn person's own agent is refused too.
#[tokio::test]
async fn naming_and_withdrawing_refuse_what_cannot_hold() -> TestResult {
    let f = fixture().await?;
    let t1 = id_of(&f).to_owned();
    let eleven: Vec<String> = (0..11).map(|i| format!("p{i}")).collect();
    let many = agent_asks(&f.h, json!({"action": "Decide", "who": eleven})).await?;
    assert_eq!(many["isError"], true, "{many}");
    assert!(many.to_string().contains("at most 10"), "{many}");
    let erin = seed_github_user(&f.h, "u-e", "erin").await?;
    let _ = erin;
    f.h.github.set_perm("erin", "acme/repo", perms("write"));
    disable(&f.h, "u-e").await?;
    let disabled = agent_asks(&f.h, json!({"action": "Decide", "who": ["erin"]})).await?;
    assert_eq!(disabled["isError"], true, "{disabled}");
    assert!(disabled.to_string().contains("@erin"), "{disabled}");
    // Nothing asked: nobody to withdraw.
    assert_eq!(withdraw_bob(&f, &f.alice).await?.status, 409);
    // A request that does not name Bob.
    agent_asks(&f.h, json!({"action": "Decide", "who": []})).await?;
    let not_named = withdraw_bob(&f, &f.alice).await?;
    assert_eq!(not_named.status, 409, "{}", not_named.body);
    assert!(
        not_named.body.to_string().contains("does not name @bob"),
        "{}",
        not_named.body
    );
    // Bob named, then withdrawn: his own agent's message is refused saying so.
    agent_asks(&f.h, json!({"action": "Decide", "who": ["bob"]})).await?;
    assert_eq!(withdraw_bob(&f, &f.alice).await?.status, 200);
    let caller = yulius_app::coordination::ProjectCaller {
        sandbox: yulius_domain::ids::SandboxId::parse("s1")?,
        user: UserId::parse("u-b")?,
        task: None,
        channel: yulius_app::coordination::Channel::Remote,
        actor: "remote:bob".into(),
        detail: json!({"via": "remote"}),
    };
    let refused =
        f.h.app
            .tool_task_message(&caller, &json!({"task_id": t1, "text": "M1: yes"}))
            .await;
    assert!(
        matches!(&refused, Err(yulius_app::error::AppError::Forbidden(m)) if m.contains("withdrew you")),
        "{refused:?}"
    );
    Ok(())
}

/// When GitHub does not answer, a named person's access is
/// not judged "none": the request is refused saying it could not be checked now.
#[tokio::test]
async fn a_name_whose_access_cannot_be_checked_is_said_so() -> TestResult {
    let f = fixture().await?;
    f.h.github
        .fail_permissions
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let asked = agent_asks(&f.h, json!({"action": "Decide", "who": ["bob"]})).await?;
    f.h.github
        .fail_permissions
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(asked["isError"], true, "{asked}");
    let said = asked.to_string();
    assert!(said.contains("could not be checked now"), "{said}");
    assert!(!said.contains("has no write access"), "{said}");
    Ok(())
}

/// A person with no usable GitHub sign-in cannot
/// be named until they sign in again; a task whose run holds the owner's personal secrets
/// names nobody else, since only its owner can answer it.
#[tokio::test]
async fn names_that_could_never_answer_are_refused_with_why() -> TestResult {
    let f = fixture().await?;
    // Frank has an account but no GitHub sign-in stored.
    let frank = yulius_app::model::UserRecord {
        id: UserId::parse("u-f")?,
        github_login: Some("frank".into()),
        github_id: None,
        display_name: "Frank".into(),
        avatar_url: None,
        is_superadmin: false,
        is_instance_admin: false,
        password_hash: None,
        created_at: yulius_domain::time::Timestamp(1),
        disabled_at: None,
    };
    f.h.store.insert_user(&frank).await?;
    f.h.github.set_perm("frank", "acme/repo", perms("write"));
    let unsigned = agent_asks(&f.h, json!({"action": "Decide", "who": ["frank"]})).await?;
    assert_eq!(unsigned["isError"], true, "{unsigned}");
    assert!(
        unsigned
            .to_string()
            .contains("must sign in to Yulius again"),
        "{unsigned}"
    );
    // Alice's personal secret reaches the run: nobody else can answer, so nobody is named.
    let secret = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"scope": "personal", "name": "ALICE_TOKEN", "value": "alice-personal-71c0ffee",
            "hosts": ["api.alice.test"], "injection": "bearer"}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(secret.status, 201, "{}", secret.body);
    let personal = agent_asks(&f.h, json!({"action": "Decide", "who": ["bob"]})).await?;
    assert_eq!(personal["isError"], true, "{personal}");
    assert!(personal.to_string().contains("personal"), "{personal}");
    assert!(f.h.store.task_needs_you(&f.task.id).await?.is_none());
    Ok(())
}

/// The answer of `/internal/git-credential` to the run with task token `token`.
async fn agent_credential(
    h: &Harness,
    token: &str,
) -> Result<crate::support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com", "path": "acme/repo.git"})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await
}

/// [`fixture`] with the GitHub App installed, so a run may be issued its agent token.
async fn fixture_with_app() -> Result<Fixture, Box<dyn std::error::Error>> {
    let f = fixture().await?;
    f.h.store.init_instance("https://yulius.test", "x").await?;
    crate::support::configure_github_app(&f.h).await?;
    f.h.store.set_installation(Some(7)).await?;
    Ok(f)
}

/// Disabling a task's owner stops the task, and its run
/// is refused a git credential from then on; another person's task runs on.
#[tokio::test]
async fn a_disabled_owners_run_gets_no_git_credential_and_stops() -> TestResult {
    let f = fixture_with_app().await?;
    let admin = crate::support::setup_superadmin_creds(&f.h).await?;
    let mut bobs = f.task.clone();
    bobs.id = TaskId::parse("t2")?;
    bobs.number = 2;
    bobs.owner = UserId::parse("u-b")?;
    f.h.store.insert_task(&bobs).await?;
    let before = agent_credential(&f.h, "tt").await?;
    assert_eq!(before.status, 200, "{}", before.body);

    let disable = call(
        &f.h.router,
        Method::PATCH,
        "/api/admin/users/u-a",
        Some(json!({"disabled": true})),
        &admin,
    )
    .await?;
    assert_eq!(disable.status, 200, "{}", disable.body);
    let refused = agent_credential(&f.h, "tt").await?;
    // Refused as a person's own git is after a disable (`workspace_git_credential`).
    assert_eq!(refused.status, 401, "{}", refused.body);
    assert!(refused.body.get("token").is_none(), "{}", refused.body);
    // Asked to stop: its run ends as the supervisor reaps it.
    let status = f.h.store.task(&f.task.id).await?.ok_or("task")?.status;
    assert!(
        status.is_resting() || status == TaskStatus::Stopping,
        "{status:?}"
    );
    assert_eq!(audited(&f.h, "t1", "task.stopped").await?, 1);
    // Bob's task, in the same project, runs on.
    assert_eq!(
        f.h.store.task(&bobs.id).await?.ok_or("t2")?.status,
        TaskStatus::Running
    );
    Ok(())
}

/// An owner who lost write access to the project no longer has their run
/// issued a git credential; Bob, who may still write, gets one for his own task.
#[tokio::test]
async fn an_owner_down_to_read_access_has_their_run_refused_a_git_credential() -> TestResult {
    let f = fixture_with_app().await?;
    let mut bobs = f.task.clone();
    bobs.id = TaskId::parse("t2")?;
    bobs.number = 2;
    bobs.owner = UserId::parse("u-b")?;
    f.h.store.insert_task(&bobs).await?;
    seed_live_attempt(&f.h, "t2", "tt2").await?;

    f.h.github.set_perm("alice", "acme/repo", perms("read"));
    f.h.store
        .put_access(
            &UserId::parse("u-a")?,
            &f.task.sandbox_id,
            yulius_domain::permission::AccessLevel::Read,
            f.h.app.now(),
        )
        .await?;
    let refused = agent_credential(&f.h, "tt").await?;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(refused.body.get("token").is_none(), "{}", refused.body);
    let bob = agent_credential(&f.h, "tt2").await?;
    assert_eq!(bob.status, 200, "{}", bob.body);
    assert_eq!(bob.body["token"], "agent-inst");
    Ok(())
}
