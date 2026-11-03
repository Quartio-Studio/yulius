//! The Editor's shared text (SPEC-411 EDT-016..020): writers join one document per file, their
//! concurrent edits are transformed into one order and published to the sandbox's writers only;
//! the server saves the text with the caller's access and follows the disk, keeping a conflict
//! for a person's choice.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{TestResult, call, perms, seed_github_user, seed_sandbox, seed_task};
use yulius_app::events::Envelope;
use yulius_contracts::events::UiEvent;
use yulius_domain::task::TaskStatus;

const LIVE: &str = "/api/sandboxes/s1/files/live";

/// A repository with `src/main.rs`; alice and dave write, bob reads, carol has nothing.
async fn fixture() -> Result<
    (
        support::Harness,
        support::Creds,
        support::Creds,
        support::Creds,
    ),
    Box<dyn std::error::Error>,
> {
    let h = support::harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let dave = seed_github_user(&h, "u-dave", "dave").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("dave", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    std::fs::create_dir_all(repo(&h).join("src"))?;
    std::fs::write(repo(&h).join("src/main.rs"), "fn main() {}\n")?;
    Ok((h, alice, dave, bob))
}

fn repo(h: &support::Harness) -> std::path::PathBuf {
    h.app.config.data_dir.join("sandboxes/s1/repo")
}

fn on_disk(h: &support::Harness) -> String {
    std::fs::read_to_string(repo(h).join("src/main.rs")).unwrap_or_default()
}

/// The `file.live` events published since the last call, with their audience.
fn live_events(events: &mut tokio::sync::broadcast::Receiver<Envelope>) -> Vec<(bool, Value)> {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter(|envelope| matches!(envelope.event, UiEvent::FileLive { .. }))
        .map(|envelope| {
            (
                envelope.writers_only,
                serde_json::to_value(&envelope.event).unwrap_or_default(),
            )
        })
        .collect()
}

async fn join(
    h: &support::Harness,
    who: &support::Creds,
    body: Value,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(&h.router, Method::POST, LIVE, Some(body), who).await
}

async fn edit(
    h: &support::Harness,
    who: &support::Creds,
    session: &str,
    revision: u64,
    op: Value,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/files/live/edit",
        Some(json!({"path": "src/main.rs", "session": session, "client": "tab1", "revision": revision, "op": op})),
        who,
    )
    .await
}

async fn act(
    h: &support::Harness,
    who: &support::Creds,
    what: &str,
    session: &str,
    force: bool,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::POST,
        &format!("/api/sandboxes/s1/files/live/{what}"),
        Some(json!({"path": "src/main.rs", "session": session, "force": force})),
        who,
    )
    .await
}

fn session_of(response: &support::Resp) -> String {
    response.body["session"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// AC-411-2: two writers edit at the same revision; both edits land, the later one transformed,
/// and both steps go to the sandbox's writers only, in order.
#[tokio::test]
async fn concurrent_edits_of_two_writers_land_in_one_order_for_writers_only() -> TestResult {
    let (h, alice, dave, _) = fixture().await?;
    let mut events = h.app.events.subscribe();
    let first = join(&h, &alice, json!({"path": "src/main.rs", "client": "a1"})).await?;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(first.body["text"], "fn main() {}\n");
    assert_eq!(first.body["revision"], 0);
    assert_eq!(first.body["saved"], true);
    assert_eq!(first.body["eol"], "\n");
    let session = session_of(&first);
    // Dave joins the same document, whatever text he brings.
    let second = join(
        &h,
        &dave,
        json!({"path": "./src/main.rs", "client": "d1", "text": "other", "saved": false}),
    )
    .await?;
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.body);
    assert_eq!(session_of(&second), session);
    assert_eq!(second.body["text"], "fn main() {}\n");

    let mine = edit(&h, &alice, &session, 0, json!(["// a\n", 13])).await?;
    assert_eq!(mine.status, StatusCode::OK, "{:?}", mine.body);
    assert_eq!(mine.body["revision"], 1);
    let his = edit(&h, &dave, &session, 0, json!([13, "// d\n"])).await?;
    assert_eq!(his.status, StatusCode::OK, "{:?}", his.body);
    assert_eq!(his.body["revision"], 2);

    let steps = live_events(&mut events);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert!(steps.iter().all(|(writers_only, _)| *writers_only));
    assert_eq!(steps[0].1["revision"], 1);
    assert_eq!(steps[0].1["op"], json!(["// a\n", 13]));
    assert_eq!(steps[1].1["revision"], 2);
    assert_eq!(
        steps[1].1["op"],
        json!([18, "// d\n"]),
        "transformed past alice's"
    );
    assert_eq!(steps[1].1["path"], "src/main.rs");
    assert_eq!(steps[1].1["saved"], false);

    // Catching up: the steps after a revision, or the whole text for another session.
    let since = call(
        &h.router,
        Method::GET,
        &format!("{LIVE}?path=src/main.rs&session={session}&since=1"),
        None,
        &dave,
    )
    .await?;
    assert_eq!(since.status, StatusCode::OK, "{:?}", since.body);
    assert_eq!(since.body["text"], Value::Null);
    assert_eq!(
        since.body["steps"],
        json!([{"revision": 2, "client": "tab1", "op": [18, "// d\n"]}])
    );
    let whole = call(
        &h.router,
        Method::GET,
        &format!("{LIVE}?path=src/main.rs&session=other&since=0"),
        None,
        &dave,
    )
    .await?;
    assert_eq!(whole.body["text"], "// a\nfn main() {}\n// d\n");
    assert_eq!(whole.body["steps"], Value::Null);
    // Nothing is written until someone saves.
    assert_eq!(on_disk(&h), "fn main() {}\n");
    Ok(())
}

/// AC-411-2 (EDT-018): only the people who may read the folder see the text; a task worktree's
/// text is changed by its owner only, and nobody else starts it from their own text.
#[tokio::test]
async fn only_writers_see_the_text_and_only_a_worktrees_owner_changes_it() -> TestResult {
    let (h, alice, dave, bob) = fixture().await?;
    let joined = join(&h, &alice, json!({"path": "src/main.rs"})).await?;
    let session = session_of(&joined);
    for refused in [
        join(&h, &bob, json!({"path": "src/main.rs"})).await?,
        call(
            &h.router,
            Method::GET,
            &format!("{LIVE}?path=src/main.rs&session={session}&since=0"),
            None,
            &bob,
        )
        .await?,
        edit(&h, &bob, &session, 0, json!(["x", 13])).await?,
        act(&h, &bob, "save", &session, false).await?,
    ] {
        assert!(
            matches!(
                refused.status,
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ),
            "{:?}",
            refused.status
        );
        assert!(
            !refused.body.to_string().contains("fn main"),
            "{:?}",
            refused.body
        );
    }

    let worktree = h.app.config.data_dir.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(&worktree)?;
    std::fs::write(worktree.join("task.txt"), "task file\n")?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
    // Dave reads alice's worktree live, from the disk, never from his own text…
    let his = join(
        &h,
        &dave,
        json!({"root": "task:t1", "path": "task.txt", "text": "planted", "saved": false}),
    )
    .await?;
    assert_eq!(his.status, StatusCode::OK, "{:?}", his.body);
    assert_eq!(his.body["text"], "task file\n");
    let task_session = session_of(&his);
    // …and cannot change it; alice can.
    let refused = call(&h.router, Method::POST, "/api/sandboxes/s1/files/live/edit",
        Some(json!({"root": "task:t1", "path": "task.txt", "session": task_session, "client": "d1", "revision": 0, "op": ["x", 10]})), &dave).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.body);
    let hers = call(&h.router, Method::POST, "/api/sandboxes/s1/files/live/edit",
        Some(json!({"root": "task:t1", "path": "task.txt", "session": task_session, "client": "a1", "revision": 0, "op": ["x", 10]})), &alice).await?;
    assert_eq!(hers.status, StatusCode::OK, "{:?}", hers.body);
    Ok(())
}

/// AC-411-2: edits that do not fit, cut a character, name another or an ended document, come
/// too late or too often are refused, and nothing of them is applied.
#[tokio::test]
async fn edits_that_do_not_fit_are_refused() -> TestResult {
    let (h, alice, _, _) = fixture().await?;
    std::fs::write(repo(&h).join("src/main.rs"), "a😀b")?;
    let joined = join(&h, &alice, json!({"path": "src/main.rs"})).await?;
    let session = session_of(&joined);
    assert_eq!(
        edit(&h, &alice, &session, 0, json!([3])).await?.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "short"
    );
    assert_eq!(
        edit(&h, &alice, &session, 0, json!([2, "x", 2]))
            .await?
            .status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "inside a pair"
    );
    assert_eq!(
        edit(&h, &alice, &session, 0, json!([1, -1, 2]))
            .await?
            .status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "half a pair"
    );
    assert_eq!(
        edit(
            &h,
            &alice,
            &session,
            0,
            json!([4, "x".repeat(2 * 1024 * 1024)])
        )
        .await?
        .status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "too large"
    );
    assert_eq!(
        edit(&h, &alice, "nope", 0, json!([4])).await?.status,
        StatusCode::CONFLICT,
        "another document"
    );
    assert_eq!(
        edit(&h, &alice, &session, 5, json!([4])).await?.status,
        StatusCode::CONFLICT,
        "a revision not made yet"
    );
    assert_eq!(
        edit(&h, &alice, &session, 0, json!([4])).await?.status,
        StatusCode::OK
    );
    let text = call(
        &h.router,
        Method::GET,
        &format!("{LIVE}?path=src/main.rs&session=x&since=0"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(text.body["text"], "a😀b");
    assert_eq!(text.body["revision"], 1);

    // Past the limit (30 a second per person and file): 429, and nothing applied.
    let mut statuses = Vec::new();
    for revision in 1..=40 {
        statuses.push(
            edit(&h, &alice, &session, revision, json!([4]))
                .await?
                .status,
        );
        if statuses.last() == Some(&StatusCode::TOO_MANY_REQUESTS) {
            break;
        }
    }
    assert_eq!(
        statuses.last(),
        Some(&StatusCode::TOO_MANY_REQUESTS),
        "{statuses:?}"
    );

    // A revision older than the kept steps (256) must catch up first; the catch-up is the text.
    for second in 0..10 {
        h.clock
            .0
            .fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
        for _ in 0..28 {
            let now = call(
                &h.router,
                Method::GET,
                &format!("{LIVE}?path=src/main.rs&session={session}&since=0"),
                None,
                &alice,
            )
            .await?;
            let revision = now.body["revision"].as_u64().unwrap_or_default();
            assert_eq!(
                edit(&h, &alice, &session, revision, json!([4]))
                    .await?
                    .status,
                StatusCode::OK,
                "second {second}"
            );
        }
    }
    h.clock
        .0
        .fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        edit(&h, &alice, &session, 1, json!([4])).await?.status,
        StatusCode::CONFLICT,
        "steps no longer kept"
    );
    let late = call(
        &h.router,
        Method::GET,
        &format!("{LIVE}?path=src/main.rs&session={session}&since=1"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(late.body["text"], "a😀b");
    assert_eq!(late.body["steps"], Value::Null);

    // A saved document left alone for 10 minutes is dropped: edits must join again.
    h.clock
        .0
        .fetch_add(11 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    let again = join(&h, &alice, json!({"path": "src/main.rs"})).await?;
    assert_ne!(session_of(&again), session);
    assert_eq!(
        edit(&h, &alice, &session, 0, json!([4])).await?.status,
        StatusCode::CONFLICT
    );
    Ok(())
}

/// AC-411-3 (EDT-019): a save writes the shared text with the document's base and marks it saved
/// for everyone; text already saved writes nothing.
#[tokio::test]
async fn a_save_writes_the_shared_text_once() -> TestResult {
    let (h, alice, dave, _) = fixture().await?;
    let session = session_of(&join(&h, &alice, json!({"path": "src/main.rs"})).await?);
    assert_eq!(
        edit(&h, &dave, &session, 0, json!([13, "// saved\n"]))
            .await?
            .status,
        StatusCode::OK
    );
    let mut events = h.app.events.subscribe();
    let saved = act(&h, &alice, "save", &session, false).await?;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    assert_eq!(on_disk(&h), "fn main() {}\n// saved\n");
    assert_eq!(saved.body["saved"], true);
    assert_eq!(saved.body["revision"], 1);
    let sha = saved.body["sha256"].as_str().unwrap_or_default().to_owned();
    assert_eq!(sha.len(), 64);
    let published = live_events(&mut events);
    assert_eq!(published.len(), 1, "{published:?}");
    assert_eq!(published[0].1["op"], Value::Null);
    assert_eq!(published[0].1["saved"], true);
    assert_eq!(published[0].1["sha256"], sha.as_str());

    // Saved text writes nothing: an agent's later write stays.
    std::fs::write(repo(&h).join("src/main.rs"), "agent\n")?;
    let again = act(&h, &dave, "save", &session, false).await?;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(on_disk(&h), "agent\n");
    assert!(live_events(&mut events).is_empty());
    Ok(())
}

/// AC-411-3 (EDT-020): a disk change under saved text becomes one step; under unsaved text it is
/// kept as a conflict, which a save reports without writing, Keep mine writes over and Take disk
/// version replaces.
#[tokio::test]
async fn the_disk_is_followed_and_a_conflict_waits_for_a_choice() -> TestResult {
    let (h, alice, dave, _) = fixture().await?;
    let session = session_of(&join(&h, &alice, json!({"path": "src/main.rs"})).await?);
    let mut events = h.app.events.subscribe();

    // Saved text takes the disk's version as one step, with the disk's line ending.
    std::fs::write(repo(&h).join("src/main.rs"), "fn main() { agent() }\r\n")?;
    let taken = act(&h, &dave, "disk", &session, false).await?;
    assert_eq!(taken.status, StatusCode::OK, "{:?}", taken.body);
    assert_eq!(taken.body["revision"], 1);
    assert_eq!(taken.body["saved"], true);
    let published = live_events(&mut events);
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].1["client"], "");
    assert_eq!(published[0].1["op"], json!([11, " agent() }\r", -1, 1]));
    assert_eq!(published[0].1["eol"], "\r\n");
    let whole = call(
        &h.router,
        Method::GET,
        &format!("{LIVE}?path=src/main.rs&session=x&since=0"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(whole.body["text"], "fn main() { agent() }\r\n");
    assert_eq!(whole.body["eol"], "\r\n");
    // Unchanged: nothing.
    assert_eq!(
        act(&h, &dave, "disk", &session, false).await?.body["revision"],
        1
    );
    assert!(live_events(&mut events).is_empty());

    // Unsaved text meets a disk change: kept, the conflict published, no save written.
    assert_eq!(
        edit(&h, &alice, &session, 1, json!(["// mine\r\n", 23]))
            .await?
            .status,
        StatusCode::OK
    );
    std::fs::write(repo(&h).join("src/main.rs"), "theirs\n")?;
    let conflict = act(&h, &dave, "disk", &session, false).await?;
    assert_eq!(conflict.body["saved"], false);
    let theirs = conflict.body["disk_sha256"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert_eq!(theirs.len(), 64);
    let save = act(&h, &alice, "save", &session, false).await?;
    assert_eq!(save.status, StatusCode::OK);
    assert_eq!(save.body["disk_sha256"], theirs.as_str());
    assert_eq!(on_disk(&h), "theirs\n");
    let published = live_events(&mut events);
    assert!(
        published
            .iter()
            .any(|(_, e)| e["disk_sha256"] == theirs.as_str()),
        "{published:?}"
    );

    // Keep mine: the shared text over the disk.
    let kept = act(&h, &alice, "save", &session, true).await?;
    assert_eq!(kept.status, StatusCode::OK, "{:?}", kept.body);
    assert_eq!(kept.body["saved"], true);
    assert_eq!(kept.body["disk_sha256"], Value::Null);
    assert_eq!(on_disk(&h), "// mine\r\nfn main() { agent() }\r\n");

    // A save that finds the disk moved on keeps the conflict too.
    assert_eq!(
        edit(&h, &alice, &session, 2, json!([32, "// more\r\n"]))
            .await?
            .status,
        StatusCode::OK
    );
    std::fs::write(repo(&h).join("src/main.rs"), "again\n")?;
    let met = act(&h, &alice, "save", &session, false).await?;
    assert_eq!(met.body["saved"], false);
    assert!(met.body["disk_sha256"].is_string());
    assert_eq!(on_disk(&h), "again\n");

    // Take disk version: the disk's text for everyone, saved.
    let replaced = act(&h, &dave, "disk", &session, true).await?;
    assert_eq!(replaced.status, StatusCode::OK, "{:?}", replaced.body);
    assert_eq!(replaced.body["saved"], true);
    assert_eq!(replaced.body["disk_sha256"], Value::Null);
    let whole = call(
        &h.router,
        Method::GET,
        &format!("{LIVE}?path=src/main.rs&session=x&since=0"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(whole.body["text"], "again\n");
    assert_eq!(on_disk(&h), "again\n");

    // A file deleted under the text is a conflict too; Keep mine writes it again.
    assert_eq!(
        edit(&h, &alice, &session, 4, json!([6, "!"])).await?.status,
        StatusCode::OK
    );
    std::fs::remove_file(repo(&h).join("src/main.rs"))?;
    let gone = act(&h, &dave, "disk", &session, false).await?;
    assert_eq!(gone.body["disk_sha256"], "");
    assert_eq!(
        act(&h, &alice, "save", &session, true).await?.body["saved"],
        true
    );
    assert_eq!(on_disk(&h), "again\n!");
    Ok(())
}

/// EDT-018, EDT-022: a writer who joins a file nobody edits starts it from their own text (a
/// restart lost the document); clean text from an older version gives way to the disk's.
#[tokio::test]
async fn a_writer_starts_the_document_from_their_own_text() -> TestResult {
    let (h, alice, dave, _) = fixture().await?;
    let sha = {
        let file = call(
            &h.router,
            Method::GET,
            "/api/sandboxes/s1/files/content?path=src/main.rs",
            None,
            &alice,
        )
        .await?;
        file.body["sha256"].as_str().unwrap_or_default().to_owned()
    };
    let mine = join(&h, &alice, json!({"path": "src/main.rs", "text": "fn main() { unsaved }\r\n", "sha256": sha, "saved": false, "eol": "\n"})).await?;
    assert_eq!(mine.status, StatusCode::OK, "{:?}", mine.body);
    assert_eq!(mine.body["text"], "fn main() { unsaved }\n");
    assert_eq!(mine.body["saved"], false);
    assert_eq!(mine.body["sha256"], sha.as_str());
    let session = session_of(&mine);
    assert_eq!(
        act(&h, &dave, "save", &session, false).await?.body["saved"],
        true
    );
    assert_eq!(on_disk(&h), "fn main() { unsaved }\n");

    // A malformed sha256 or eol is refused.
    let bad = join(&h, &alice, json!({"path": "README.md", "sha256": "XYZ"})).await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let bad = join(&h, &alice, json!({"path": "src/main.rs", "eol": "\r"})).await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Another file: clean text read at an older version gives way to the disk's.
    std::fs::write(repo(&h).join("README.md"), "new\n")?;
    let stale = join(
        &h,
        &dave,
        json!({"path": "README.md", "text": "old\n", "sha256": "0".repeat(64), "saved": true}),
    )
    .await?;
    assert_eq!(stale.body["text"], "new\n");
    assert_eq!(stale.body["saved"], true);
    // A binary file is not text the editor opens.
    std::fs::write(repo(&h).join("logo.bin"), [0_u8, 1, 2])?;
    let binary = join(&h, &dave, json!({"path": "logo.bin"})).await?;
    assert_eq!(binary.status, StatusCode::UNPROCESSABLE_ENTITY);
    Ok(())
}

/// EDT-021: presence may name the live document and revision its offsets are in.
#[tokio::test]
async fn presence_carries_the_live_revision() -> TestResult {
    let (h, alice, _, _) = fixture().await?;
    let mut events = h.app.events.subscribe();
    let sent = call(&h.router, Method::PUT, "/api/sandboxes/s1/files/presence",
        Some(json!({"path": "src/main.rs", "selection": {"anchor": 1, "head": 2}, "session": "abc123", "revision": 7})), &alice).await?;
    assert_eq!(sent.status, StatusCode::NO_CONTENT, "{:?}", sent.body);
    let relayed: Vec<Value> = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|e| {
            matches!(e.event, UiEvent::FilePresence { .. })
                .then(|| serde_json::to_value(&e.event).unwrap_or_default())
        })
        .collect();
    assert_eq!(relayed.len(), 1);
    assert_eq!(relayed[0]["session"], "abc123");
    assert_eq!(relayed[0]["revision"], 7);
    let bad = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"path": "src/main.rs", "selection": null, "session": "no/way"})),
        &alice,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    Ok(())
}
