//! SPEC-070, SPEC-490: a task is owned by one person, whose GitHub credentials it uses. Only the
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
use yulius_app::ports::store_ext::AutomationStore as _;
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
/// agent's shell never does, ADR-0114).
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
    assert_eq!(github_actor(&f.h, "tt").await?, "alice");
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

/// The ways the web API steers a task (SPEC-490 OWN-004), each with a body that passes its
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

/// What the refusal of control says (SPEC-490 OWN-004): whose the task is, how to ask, and what
/// is open to everyone.
fn says_whose_and_how(message: &str) -> bool {
    message.contains("@alice")
        && message.contains("only they steer it")
        && message.contains("Ask them for control")
        && message.contains("You can still stop, archive or delete it")
}

/// SPEC-490 OWN-004, AC-3 (SPEC-070 R3): a person who may work in the project but does not own
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
    for who in [&f.bob, &admin] {
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

/// SPEC-490 OWN-002, AC-1: a person who may work in the project stops, archives, restores and
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

/// SPEC-490 OWN-005, AC-6: a stop, an archive and a restore of a task by someone who does not
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

/// SPEC-490 OWN-002: a task that waits to start is cancelled by anyone who may work.
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

/// SPEC-490 OWN-002, AC-2: tidying needs write access. A reader is refused (403) and a person
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

/// SPEC-118 KEY-002 (review): disabling the typist refuses their next keystroke, although the
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

/// SPEC-118 KEY-002 (review): signing out (the web session revoked) refuses the next keystroke of
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

/// SPEC-490 OWN-002, AC-1: an agent acting for a person (here a task of Bob's) stops, archives
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

/// R4: the person who asked may withdraw; a request then waits no more.
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
        git: Some(std::sync::Arc::new(crate::support::confined_git())),
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
    assert_eq!(audited(&f.h, "t1", "task.control_dropped").await?, 2);
    Ok(())
}

/// AC2 (review), ADR-0114: after a hand-over the agent's `gh` (a credential asked without a
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
