//! The built-in editor's file API (SPEC-001 EDT-001..004, AC-EDT-1): write users list, read and
//! change files of the sandbox repository or a task worktree; paths never leave the folder;
//! `.git` stays hidden; a file changed on disk is not overwritten silently.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{TestResult, call, harness, perms, seed_github_user, seed_sandbox, seed_task};
use yulius_domain::task::TaskStatus;

#[tokio::test]
async fn write_users_edit_files_inside_the_folder_only() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let data = h.app.config.data_dir.clone();
    let repo = data.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.join("src"))?;
    std::fs::create_dir_all(repo.join(".git"))?;
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n")?;
    std::fs::write(repo.join("README.md"), "# Fixture\n")?;
    std::fs::write(repo.join("logo.bin"), [0_u8, 1, 2, 3])?;
    std::fs::write(data.join("secret.txt"), "outside")?;
    std::os::unix::fs::symlink(data.join("secret.txt"), repo.join("escape.txt"))?;
    let worktree = data.join("sandboxes/s1/worktrees/t1");
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

    // Listing: directories first, `.git` hidden, links leaving the folder marked.
    let listing = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/files",
        None,
        &alice,
    )
    .await?;
    assert_eq!(listing.status, StatusCode::OK, "{:?}", listing.body);
    let names: Vec<&str> = listing.body["entries"]
        .as_array()
        .ok_or("entries")?
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(names, ["src", "escape.txt", "logo.bin", "README.md"]);
    assert_eq!(listing.body["entries"][1]["kind"], "symlink");

    // Read access to the sandbox is not enough.
    let refused = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/files",
        None,
        &bob,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    // Reading: text with its hash; binary reported; escapes and .git refused.
    let file = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/files/content?path=src/main.rs",
        None,
        &alice,
    )
    .await?;
    assert_eq!(file.body["content"], "fn main() {}\n");
    let sha = file.body["sha256"].as_str().ok_or("sha")?.to_owned();
    let binary = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/files/content?path=logo.bin",
        None,
        &alice,
    )
    .await?;
    assert_eq!(binary.body["binary"], true);
    assert!(binary.body["content"].is_null());
    for (path, status) in [
        ("../../secret.txt", StatusCode::UNPROCESSABLE_ENTITY),
        ("escape.txt", StatusCode::FORBIDDEN),
        (".git/config", StatusCode::FORBIDDEN),
    ] {
        let response = call(
            &h.router,
            Method::GET,
            &format!("/api/sandboxes/s1/files/content?path={path}"),
            None,
            &alice,
        )
        .await?;
        assert_eq!(response.status, status, "{path}: {:?}", response.body);
    }

    // Writing with the right base succeeds; a stale base conflicts.
    let saved = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/content",
        Some(json!({"path": "src/main.rs", "content": "fn main() { println!(\"hi\"); }\n", "base_sha256": sha})),
        &alice,
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    assert_eq!(
        std::fs::read_to_string(repo.join("src/main.rs"))?,
        "fn main() { println!(\"hi\"); }\n"
    );
    let stale = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/content",
        Some(json!({"path": "src/main.rs", "content": "lost update", "base_sha256": sha})),
        &alice,
    )
    .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    let through_link = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/content",
        Some(json!({"path": "escape.txt", "content": "x", "base_sha256": "0"})),
        &alice,
    )
    .await?;
    assert_eq!(through_link.status, StatusCode::FORBIDDEN);
    assert_eq!(std::fs::read_to_string(data.join("secret.txt"))?, "outside");

    // A link to the Git directory inside the folder does not open it either (#59).
    std::fs::write(repo.join(".git/config"), "[core]\n")?;
    std::os::unix::fs::symlink(".git", repo.join("meta"))?;
    for path in [
        "/api/sandboxes/s1/files?path=meta",
        "/api/sandboxes/s1/files/content?path=meta/config",
    ] {
        let refused = call(&h.router, Method::GET, path, None, &alice).await?;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{path}");
    }
    for (path, base) in [
        ("meta/config", json!("0")),
        ("meta/hooks-note.txt", json!(null)),
    ] {
        let refused = call(
            &h.router,
            Method::PUT,
            "/api/sandboxes/s1/files/content",
            Some(json!({"path": path, "content": "x", "base_sha256": base})),
            &alice,
        )
        .await?;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(
        std::fs::read_to_string(repo.join(".git/config"))?,
        "[core]\n"
    );
    assert!(!repo.join(".git/hooks-note.txt").exists());
    std::fs::remove_file(repo.join("meta"))?;

    // Create and delete entries; the folder itself cannot be deleted.
    let created = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/files",
        Some(json!({"path": "src/lib.rs", "kind": "file"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let again = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/files",
        Some(json!({"path": "src/lib.rs", "kind": "file"})),
        &alice,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CONFLICT);
    let dir = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/files",
        Some(json!({"path": "docs", "kind": "dir"})),
        &alice,
    )
    .await?;
    assert_eq!(dir.status, StatusCode::CREATED);
    let deleted = call(
        &h.router,
        Method::DELETE,
        "/api/sandboxes/s1/files?path=docs",
        None,
        &alice,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(!repo.join("docs").exists());
    let whole = call(
        &h.router,
        Method::DELETE,
        "/api/sandboxes/s1/files?path=",
        None,
        &alice,
    )
    .await?;
    assert_eq!(whole.status, StatusCode::UNPROCESSABLE_ENTITY);
    // Deleting the link removes the link, not its target.
    let link = call(
        &h.router,
        Method::DELETE,
        "/api/sandboxes/s1/files?path=escape.txt",
        None,
        &alice,
    )
    .await?;
    assert_eq!(link.status, StatusCode::NO_CONTENT);
    assert!(data.join("secret.txt").exists());

    // A task worktree is another root.
    let task = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/files?root=task:t1",
        None,
        &alice,
    )
    .await?;
    assert_eq!(task.body["entries"][0]["name"], "task.txt");
    Ok(())
}

/// The Editor looks at the file in front every 2 s with the `sha256` it has (SPEC-181 EDT-014): an
/// unchanged file answers 204 with no body, a changed one answers the file, with a read's rules.
#[tokio::test]
async fn a_read_with_the_known_sha256_answers_no_content_while_unchanged() -> TestResult {
    let (h, alice, bob, _carol) = presence_fixture().await?;
    let repo = h.app.config.data_dir.join("sandboxes/s1/repo");
    let read = |query: String, who: support::Creds| {
        let router = h.router.clone();
        async move {
            call(
                &router,
                Method::GET,
                &format!("/api/sandboxes/s1/files/content?path=src/main.rs{query}"),
                None,
                &who,
            )
            .await
        }
    };
    let first = read(String::new(), alice.clone()).await?;
    let sha = first.body["sha256"].as_str().ok_or("sha")?.to_owned();

    let unchanged = read(format!("&known={sha}"), alice.clone()).await?;
    assert_eq!(
        unchanged.status,
        StatusCode::NO_CONTENT,
        "{:?}",
        unchanged.body
    );
    assert!(unchanged.body.is_null());

    // An agent writes the file: the same `known` now gets the new text and its hash.
    std::fs::write(repo.join("src/main.rs"), "fn main() { agent(); }\n")?;
    let changed = read(format!("&known={sha}"), alice.clone()).await?;
    assert_eq!(changed.status, StatusCode::OK, "{:?}", changed.body);
    assert_eq!(changed.body["content"], "fn main() { agent(); }\n");
    assert_ne!(changed.body["sha256"], sha.as_str());

    // Malformed: not 64 lowercase hex digits.
    for bad in ["abc", &sha.to_uppercase(), &format!("{sha}0")] {
        let refused = read(format!("&known={bad}"), alice.clone()).await?;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    // A reader of the sandbox is refused as for a read, whatever the hash, malformed too.
    for known in [sha.as_str(), "abc"] {
        let reader = read(format!("&known={known}"), bob.clone()).await?;
        assert_eq!(reader.status, StatusCode::FORBIDDEN, "{known}");
    }
    Ok(())
}

/// A repository with one file for the presence tests; alice writes, bob reads, carol has nothing.
async fn presence_fixture() -> Result<
    (
        support::Harness,
        support::Creds,
        support::Creds,
        support::Creds,
    ),
    Box<dyn std::error::Error>,
> {
    let h = support::harness_with(support::HarnessOptions {
        configure: Some(Box::new(|c| c.viewer_recheck_ms = 200)),
        ..support::HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    let carol = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let repo = h.app.config.data_dir.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.join("src"))?;
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n")?;
    Ok((h, alice, bob, carol))
}

fn published(
    events: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
) -> Vec<yulius_app::events::Envelope> {
    std::iter::from_fn(|| events.try_recv().ok()).collect()
}

const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

/// SPEC-127 EDT-008: a writer's selection in a file is relayed, with the path as the files API
/// reads it, to the sandbox's writers only; readers may neither send nor receive it.
#[tokio::test]
async fn presence_is_relayed_to_writers_only() -> TestResult {
    use yulius_contracts::api::notes::NoteSelection;
    use yulius_contracts::events::UiEvent;
    let (h, alice, bob, carol) = presence_fixture().await?;
    let mut events = h.app.events.subscribe();

    let sent = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"path": "./src/main.rs", "sha256": SHA, "selection": {"anchor": 3, "head": 7}, "hello": true, "client": "tab1", "seq": 4})),
        &alice,
    )
    .await?;
    assert_eq!(sent.status, StatusCode::NO_CONTENT, "{:?}", sent.body);
    let envelope = events.try_recv()?;
    assert!(envelope.writers_only && envelope.user.is_none() && !envelope.admin_only);
    assert_eq!(
        envelope
            .sandbox
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("s1")
    );
    assert_eq!(
        envelope.event,
        UiEvent::FilePresence {
            sandbox_id: "s1".into(),
            root: "sandbox".into(),
            path: "src/main.rs".into(),
            user_id: "u-alice".into(),
            client: "tab1".into(),
            name: "alice".into(),
            sha256: SHA.into(),
            selection: Some(NoteSelection { anchor: 3, head: 7 }),
            hello: true,
            seq: 4,
        }
    );

    // Reading files needs write access, and so does saying where one is in them.
    for (who, status) in [
        (&bob, StatusCode::FORBIDDEN),
        (&carol, StatusCode::NOT_FOUND),
    ] {
        let refused = call(
            &h.router,
            Method::PUT,
            "/api/sandboxes/s1/files/presence",
            Some(json!({"path": "src/main.rs", "selection": null})),
            who,
        )
        .await?;
        assert_eq!(refused.status, status, "{:?}", refused.body);
    }
    assert!(published(&mut events).is_empty());

    // On the event stream: the writer gets it, the reader of the same sandbox never does (an
    // event of the sandbox sent after it reaches them first).
    let served = support::serve(h.router.clone()).await?;
    let origin = h.app.config.public_url.clone();
    let mut writer = support::ws_connect(served.addr, "/api/events", &alice, &origin).await?;
    let mut reader = support::ws_connect(served.addr, "/api/events", &bob, &origin).await?;
    let sandbox = yulius_domain::ids::SandboxId::parse("s1")?;
    let marker = |task: &str| {
        h.app.events.publish(
            Some(sandbox.clone()),
            false,
            UiEvent::TaskRemoved {
                task_id: task.into(),
            },
        );
    };
    // The server subscribes once the upgrade completes: publish until both streams have one.
    for socket in [&mut writer, &mut reader] {
        let mut seen = None;
        for _ in 0..20 {
            marker("t-ready");
            seen = next_text(socket, std::time::Duration::from_millis(250)).await;
            if seen.is_some() {
                break;
            }
        }
        assert!(
            matches!(&seen, Some(Ok(text)) if text.contains("t-ready")),
            "{seen:?}"
        );
    }
    for socket in [&mut writer, &mut reader] {
        while let Some(Ok(_)) = next_text(socket, std::time::Duration::from_millis(300)).await {}
    }
    let again = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"path": "src/main.rs", "sha256": SHA, "selection": {"anchor": 1, "head": 1}, "client": "tab1"})),
        &alice,
    )
    .await?;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    marker("t-after");
    let first = next_text(&mut writer, std::time::Duration::from_secs(2)).await;
    assert!(
        matches!(&first, Some(Ok(text)) if text.contains("\"file.presence\"") && text.contains("src/main.rs")),
        "{first:?}"
    );
    let first = next_text(&mut reader, std::time::Duration::from_secs(2)).await;
    assert!(
        matches!(&first, Some(Ok(text)) if text.contains("t-after")),
        "a reader got file presence: {first:?}"
    );

    while let Some(Ok(_)) = next_text(&mut writer, std::time::Duration::from_millis(300)).await {}

    // A task worktree is a root of its own.
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
    let in_task = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"root": "task:t1", "path": "task.txt", "selection": {"anchor": 0, "head": 4}})),
        &alice,
    )
    .await?;
    assert_eq!(in_task.status, StatusCode::NO_CONTENT, "{:?}", in_task.body);
    let seen = next_text(&mut writer, std::time::Duration::from_secs(2)).await;
    assert!(
        matches!(&seen, Some(Ok(text)) if text.contains("\"root\":\"task:t1\"") && text.contains("task.txt")),
        "{seen:?}"
    );
    while let Some(Ok(_)) = next_text(&mut reader, std::time::Duration::from_millis(300)).await {}

    // Alice loses write access: once her stream re-checks, she gets no more presence (Carol, now a
    // writer, sends it), though she still sees the sandbox's other events.
    h.github.set_perm("alice", "acme/fixture", perms("read"));
    h.github.set_perm("carol", "acme/fixture", perms("write"));
    h.clock
        .0
        .fetch_add(6 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let carols = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"path": "src/main.rs", "selection": {"anchor": 0, "head": 0}})),
        &carol,
    )
    .await?;
    assert_eq!(carols.status, StatusCode::NO_CONTENT, "{:?}", carols.body);
    marker("t-demoted");
    let first = next_text(&mut writer, std::time::Duration::from_secs(2)).await;
    assert!(
        matches!(&first, Some(Ok(text)) if text.contains("t-demoted")),
        "a demoted writer got file presence: {first:?}"
    );
    Ok(())
}

/// SPEC-127 EDT-008: malformed updates are refused and relay nothing.
#[tokio::test]
async fn presence_is_refused_when_malformed() -> TestResult {
    let (h, alice, _, _) = presence_fixture().await?;
    let mut events = h.app.events.subscribe();
    for (body, status) in [
        (
            json!({"path": "src/main.rs", "selection": null, "revision": 1}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (json!({"selection": null}), StatusCode::UNPROCESSABLE_ENTITY),
        (
            json!({"path": "", "selection": null}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"path": "../secret.txt", "selection": null}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"path": ".git/config", "selection": null}),
            StatusCode::FORBIDDEN,
        ),
        (
            json!({"path": "src/main.rs", "root": "elsewhere", "selection": null}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"path": "src/main.rs", "sha256": "abc", "selection": null}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"path": "src/main.rs", "sha256": SHA.to_uppercase(), "selection": null}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"path": "src/main.rs", "selection": null, "client": "tab one"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"path": "src/main.rs", "selection": {"anchor": 0, "head": 2_097_153}}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let refused = call(
            &h.router,
            Method::PUT,
            "/api/sandboxes/s1/files/presence",
            Some(body.clone()),
            &alice,
        )
        .await?;
        assert_eq!(refused.status, status, "{body}: {:?}", refused.body);
    }
    // Made-up targets: a path past 4096 bytes, and a file that is not there unless leaving it.
    let long = format!("src/{}.rs", "a".repeat(4096));
    let refused = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"path": long, "selection": null})),
        &alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let missing = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"path": "src/nothing.rs", "selection": {"anchor": 0, "head": 0}})),
        &alice,
    )
    .await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{:?}", missing.body);
    // A link to a folder is not followed to tell what it is (it may lead outside): leaving it is
    // relayed like leaving a file that is gone (#477).
    let repo = h.app.config.data_dir.join("sandboxes/s1/repo");
    std::os::unix::fs::symlink(repo.join("src"), repo.join("src-link"))?;
    for path in ["src", "src/nothing.rs", "src-link"] {
        let left = call(
            &h.router,
            Method::PUT,
            "/api/sandboxes/s1/files/presence",
            Some(json!({"path": path, "selection": null})),
            &alice,
        )
        .await?;
        let expected = if path == "src" {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::NO_CONTENT
        };
        assert_eq!(left.status, expected, "{path}: {:?}", left.body);
    }
    // The largest file's last offset is fine.
    let edge = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/presence",
        Some(json!({"path": "src/main.rs", "selection": {"anchor": 0, "head": 2_097_152}})),
        &alice,
    )
    .await?;
    assert_eq!(edge.status, StatusCode::NO_CONTENT, "{:?}", edge.body);
    // Leaving a deleted file and a link to a folder, and the largest offset.
    assert_eq!(published(&mut events).len(), 3);
    Ok(())
}

/// SPEC-127 EDT-009: the notepads' limit, per person and file.
#[tokio::test]
async fn presence_is_rate_limited() -> TestResult {
    let (h, alice, _, _) = presence_fixture().await?;
    std::fs::write(
        h.app.config.data_dir.join("sandboxes/s1/repo/README.md"),
        "# Fixture\n",
    )?;
    let mut events = h.app.events.subscribe();
    let send = |path: &'static str| {
        call(
            &h.router,
            Method::PUT,
            "/api/sandboxes/s1/files/presence",
            Some(json!({"path": path, "selection": {"anchor": 0, "head": 0}})),
            &alice,
        )
    };
    for _ in 0..20 {
        assert_eq!(send("src/main.rs").await?.status, StatusCode::NO_CONTENT);
    }
    assert_eq!(
        send("src/main.rs").await?.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(published(&mut events).len(), 20, "the 21st is not relayed");
    // Each file counts on its own, and the next second starts afresh.
    assert_eq!(send("README.md").await?.status, StatusCode::NO_CONTENT);
    h.clock
        .0
        .fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(send("src/main.rs").await?.status, StatusCode::NO_CONTENT);
    assert_eq!(published(&mut events).len(), 2);
    Ok(())
}

/// Review round 1: one person's many files cannot use up the counters everyone shares.
#[tokio::test]
async fn presence_counters_are_bounded_per_person() -> TestResult {
    let (h, alice, bob, _) = presence_fixture().await?;
    h.github.set_perm("bob", "acme/fixture", perms("write"));
    let repo = h.app.config.data_dir.join("sandboxes/s1/repo");
    for i in 0..80 {
        std::fs::write(repo.join(format!("f{i}.txt")), "x\n")?;
    }
    let send = |path: String, who: &support::Creds| {
        let who = who.clone();
        let router = h.router.clone();
        async move {
            call(
                &router,
                Method::PUT,
                "/api/sandboxes/s1/files/presence",
                Some(json!({"path": path, "selection": {"anchor": 0, "head": 0}})),
                &who,
            )
            .await
        }
    };
    let mut accepted = 0;
    for i in 0..80 {
        if send(format!("f{i}.txt"), &alice).await?.status == StatusCode::NO_CONTENT {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 64, "a person keeps at most 64 counters at once");
    // Someone else still has room, in files and notepads.
    assert_eq!(
        send("f70.txt".into(), &bob).await?.status,
        StatusCode::NO_CONTENT
    );
    let team = call(
        &h.router,
        Method::PUT,
        "/api/notes/presence",
        Some(json!({"revision": 0, "selection": null, "hello": true})),
        &bob,
    )
    .await?;
    assert_eq!(team.status, StatusCode::NO_CONTENT);
    // A second later the person's idle counters make room again.
    h.clock
        .0
        .fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        send("f79.txt".into(), &alice).await?.status,
        StatusCode::NO_CONTENT
    );
    Ok(())
}

/// The next text frame within `wait`: `Ok(text)`, `Err(close code)` when the socket closed, or
/// `None` when nothing arrived.
async fn next_text(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    wait: std::time::Duration,
) -> Option<Result<String, Option<u16>>> {
    use futures::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, socket.next()).await {
            Err(_) => return None,
            Ok(Some(Ok(Message::Text(text)))) => return Some(Ok(text.to_string())),
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Some(Err(frame.map(|f| u16::from(f.code))));
            }
            Ok(None | Some(Err(_))) => return Some(Err(None)),
            Ok(Some(Ok(_))) => {}
        }
    }
}
