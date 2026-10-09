//! The built-in editor's file API (SPEC-001 EDT-001..004, AC-EDT-1): write users list, read and
//! change files of the sandbox repository or a task worktree; paths never leave the folder;
//! `.git` stays hidden; a file changed on disk is not overwritten silently.

mod support;

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
