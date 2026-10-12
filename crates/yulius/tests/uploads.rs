//! Dropped files (SPEC-001 EDT-006, UI-048, AC-EDT-5, AC-UI-43; F 2026-10-10): the upload API
//! streams one file into the sandbox repository or a task worktree under the editor's rules
//! (write access, no traversal, no `.git`, no link out of the folder, atomic, never replacing a
//! file unless asked, bounded); terminal attachments get safe names in a folder outside every
//! worktree that the agent reads at the same path (inside bubblewrap on Linux) and that goes
//! with its task or shell.

mod support;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Method, StatusCode};

use serde_json::json;
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{
    Creds, Harness, HarnessOptions, Resp, TestResult, call, harness_configured, harness_with,
    perms, seed_github_user, seed_sandbox, seed_task, setup_superadmin_creds,
};

use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::ports::store::{TaskStore, UserStore};
use yulius_app::ports::store_ext::AutomationStore;
use yulius_app::uploads::MAX_CONCURRENT_UPLOADS;
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;

const LIMIT: u64 = 4096;

/// Uploads `bytes` with their length declared.
async fn upload(
    h: &Harness,
    method: Method,
    path: &str,
    bytes: &[u8],
    creds: &Creds,
) -> Result<Resp, Box<dyn std::error::Error>> {
    support::send(
        &h.router,
        method,
        path,
        Body::from(bytes.to_vec()),
        Some(bytes.len()),
        creds,
    )
    .await
}

/// Files whose names start with the upload's temporary prefix, anywhere below `dir`.
fn leftovers(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".yulius-upload-")
            {
                found.push(path.clone());
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            }
        }
    }
    found
}

#[tokio::test]
async fn editor_uploads_stay_in_the_folder_and_never_replace_a_file_silently() -> TestResult {
    let h = harness_configured(|c| c.upload_max_bytes = LIMIT)?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let data = h.app.config.data_dir.clone();
    let repo = data.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.join("src"))?;
    std::fs::create_dir_all(repo.join(".git/hooks"))?;
    std::fs::write(repo.join("README.md"), "# Fixture\n")?;
    let outside = data.join("outside");
    std::fs::create_dir_all(&outside)?;
    std::os::unix::fs::symlink(&outside, repo.join("escape"))?;
    std::os::unix::fs::symlink(repo.join(".git"), repo.join("gitlink"))?;
    std::os::unix::fs::symlink(repo.join("README.md"), repo.join("readme-link.md"))?;
    let worktree = data.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(&worktree)?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;

    let logo: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
    let base = "/api/sandboxes/s1/files/upload";

    // Read access to the sandbox is not enough; no session is nothing.
    let refused = upload(
        &h,
        Method::PUT,
        &format!("{base}?path=logo.bin"),
        &logo,
        &bob,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.body);
    let anonymous = upload(
        &h,
        Method::PUT,
        &format!("{base}?path=logo.bin"),
        &logo,
        &Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    // A new file in folders that do not exist yet: created, stored byte for byte, hashed.
    let stored = upload(
        &h,
        Method::PUT,
        &format!("{base}?path=docs/img/logo.bin"),
        &logo,
        &alice,
    )
    .await?;
    assert_eq!(stored.status, StatusCode::CREATED, "{:?}", stored.body);
    assert_eq!(stored.body["path"], "docs/img/logo.bin");
    assert_eq!(stored.body["name"], "logo.bin");
    assert_eq!(stored.body["size"], 1000);
    assert_eq!(stored.body["replaced"], false);
    assert_eq!(stored.body["sha256"], h.app.deps.crypto.sha256_hex(&logo));
    assert_eq!(std::fs::read(repo.join("docs/img/logo.bin"))?, logo);

    // An existing name is refused (409) and left alone, unless the overwrite was confirmed; the
    // replaced file keeps its permissions.
    std::fs::set_permissions(
        repo.join("README.md"),
        std::fs::Permissions::from_mode(0o755),
    )?;
    let again = upload(
        &h,
        Method::PUT,
        &format!("{base}?path=README.md"),
        b"new",
        &alice,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{:?}", again.body);
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md"))?,
        "# Fixture\n"
    );
    let replaced = upload(
        &h,
        Method::PUT,
        &format!("{base}?path=README.md&overwrite=true"),
        b"# Replaced\n",
        &alice,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::CREATED, "{:?}", replaced.body);
    assert_eq!(replaced.body["replaced"], true);
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md"))?,
        "# Replaced\n"
    );
    assert_eq!(
        std::fs::metadata(repo.join("README.md"))?
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    // A folder or a link is never replaced, not even when asked.
    for path in ["src", "readme-link.md"] {
        let response = upload(
            &h,
            Method::PUT,
            &format!("{base}?path={path}&overwrite=true"),
            b"x",
            &alice,
        )
        .await?;
        assert_eq!(
            response.status,
            StatusCode::CONFLICT,
            "{path}: {:?}",
            response.body
        );
    }
    assert!(repo.join("src").is_dir());
    assert!(
        std::fs::symlink_metadata(repo.join("readme-link.md"))?
            .file_type()
            .is_symlink()
    );

    // Traversal, absolute paths, control characters, `.git` (also through a link) and links out of
    // the folder are refused; nothing is written outside.
    for (path, status) in [
        ("../outside.txt", StatusCode::UNPROCESSABLE_ENTITY),
        ("%2Fetc%2Fpasswd", StatusCode::UNPROCESSABLE_ENTITY),
        ("a%0Ab.txt", StatusCode::UNPROCESSABLE_ENTITY),
        ("", StatusCode::UNPROCESSABLE_ENTITY),
        (".git/config", StatusCode::FORBIDDEN),
        ("src/.git/config", StatusCode::FORBIDDEN),
        ("gitlink/hooks/pre-commit", StatusCode::FORBIDDEN),
        ("escape/planted.txt", StatusCode::FORBIDDEN),
        ("escape/deeper/planted.txt", StatusCode::FORBIDDEN),
    ] {
        let response = upload(
            &h,
            Method::PUT,
            &format!("{base}?path={path}"),
            b"x",
            &alice,
        )
        .await?;
        assert_eq!(response.status, status, "{path}: {:?}", response.body);
    }
    assert_eq!(
        std::fs::read_dir(&outside)?.count(),
        0,
        "nothing outside the folder"
    );
    assert!(!repo.join(".git/hooks/pre-commit").exists());
    assert!(!repo.join(".git/config").exists());

    // A task worktree is a root too.
    let in_task = upload(
        &h,
        Method::PUT,
        &format!("{base}?root=task:t1&path=notes/todo.txt"),
        b"todo\n",
        &alice,
    )
    .await?;
    assert_eq!(in_task.status, StatusCode::CREATED, "{:?}", in_task.body);
    assert_eq!(
        std::fs::read_to_string(worktree.join("notes/todo.txt"))?,
        "todo\n"
    );

    // A cookie session needs the CSRF token, as for every state-changing request.
    let admin = setup_superadmin_creds(&h).await?;
    let without_csrf = Creds {
        csrf: None,
        ..admin.clone()
    };
    let csrf = upload(
        &h,
        Method::PUT,
        &format!("{base}?path=csrf.txt"),
        b"x",
        &without_csrf,
    )
    .await?;
    assert_eq!(csrf.status, StatusCode::FORBIDDEN, "{:?}", csrf.body);
    let with_csrf = upload(
        &h,
        Method::PUT,
        &format!("{base}?path=csrf.txt"),
        b"x",
        &admin,
    )
    .await?;
    assert_eq!(
        with_csrf.status,
        StatusCode::CREATED,
        "{:?}",
        with_csrf.body
    );

    // Audited, and no temporary file is left anywhere.
    let audit = h.store.automation_audit("s1:sandbox:README.md", 5).await?;
    assert!(
        audit.iter().any(|e| e.action == "file.uploaded"
            && e.actor == "alice"
            && e.detail.as_deref() == Some("11 bytes, replaced")),
        "{audit:?}"
    );
    assert!(leftovers(&data).is_empty(), "{:?}", leftovers(&data));
    Ok(())
}

#[tokio::test]
async fn a_repository_folder_replaced_by_a_link_leads_nowhere() -> TestResult {
    let h = harness_configured(|c| c.upload_max_bytes = LIMIT)?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let data = h.app.config.data_dir.clone();
    // A contained shell is root in its sandbox and could swap its repository for a link to
    // another sandbox's: neither the editor nor an upload may follow it there.
    let other = data.join("sandboxes/s2/repo");
    std::fs::create_dir_all(&other)?;
    std::fs::write(other.join("secret.txt"), "another sandbox\n")?;
    std::fs::create_dir_all(data.join("sandboxes/s1"))?;
    std::os::unix::fs::symlink(&other, data.join("sandboxes/s1/repo"))?;
    let listing = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/files",
        None,
        &alice,
    )
    .await?;
    assert_eq!(listing.status, StatusCode::FORBIDDEN, "{:?}", listing.body);
    let planted = upload(
        &h,
        Method::PUT,
        "/api/sandboxes/s1/files/upload?path=planted.txt",
        b"x",
        &alice,
    )
    .await?;
    assert_eq!(planted.status, StatusCode::FORBIDDEN, "{:?}", planted.body);
    assert!(!other.join("planted.txt").exists());
    Ok(())
}

#[tokio::test]
async fn uploads_are_bounded_and_leave_nothing_behind() -> TestResult {
    let h = harness_configured(|c| c.upload_max_bytes = LIMIT)?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let data = h.app.config.data_dir.clone();
    let repo = data.join("sandboxes/s1/repo");
    std::fs::create_dir_all(&repo)?;
    let too_big = vec![b'x'; usize::try_from(LIMIT)? + 1];

    // A declared length above the limit is refused before the body is read.
    let declared = upload(
        &h,
        Method::PUT,
        "/api/sandboxes/s1/files/upload?path=big.bin",
        &too_big,
        &alice,
    )
    .await?;
    assert_eq!(
        declared.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        declared.body
    );
    assert!(
        declared.body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("upload limit"))
    );

    // A streamed body without a length is counted and cut at the limit.
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        too_big.chunks(1000).map(|c| Ok(c.to_vec())).collect();
    let streamed = support::send(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/upload?path=big.bin",
        Body::from_stream(futures::stream::iter(chunks)),
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        streamed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        streamed.body
    );
    assert!(!repo.join("big.bin").exists());

    // A body that breaks off mid-way leaves no file and no temporary file.
    let broken: Vec<Result<Vec<u8>, std::io::Error>> = vec![
        Ok(vec![b'y'; 100]),
        Err(std::io::Error::other("connection reset")),
    ];
    let interrupted = support::send(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/files/upload?path=partial.bin",
        Body::from_stream(futures::stream::iter(broken)),
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        interrupted.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        interrupted.body
    );
    assert!(!repo.join("partial.bin").exists());
    assert!(leftovers(&data).is_empty(), "{:?}", leftovers(&data));

    // At most MAX_CONCURRENT_UPLOADS at once; the next is told to retry (429), and a slot frees
    // as soon as an upload ends (its temporary file goes with it).
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let ctx = AuthContext {
        user,
        csrf_token: None,
        session_hash: None,
    };
    let mut open = Vec::new();
    for n in 0..MAX_CONCURRENT_UPLOADS {
        open.push(
            h.app
                .begin_file_upload(&ctx, "s1", None, &format!("slot-{n}.txt"), false, None)
                .await?,
        );
    }
    assert_eq!(leftovers(&data).len(), MAX_CONCURRENT_UPLOADS);
    let busy = upload(
        &h,
        Method::PUT,
        "/api/sandboxes/s1/files/upload?path=busy.txt",
        b"x",
        &alice,
    )
    .await?;
    assert_eq!(
        busy.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{:?}",
        busy.body
    );
    drop(open.pop());
    let freed = upload(
        &h,
        Method::PUT,
        "/api/sandboxes/s1/files/upload?path=busy.txt",
        b"x",
        &alice,
    )
    .await?;
    assert_eq!(freed.status, StatusCode::CREATED, "{:?}", freed.body);
    drop(open);
    assert!(leftovers(&data).is_empty(), "{:?}", leftovers(&data));
    for n in 0..MAX_CONCURRENT_UPLOADS {
        assert!(!repo.join(format!("slot-{n}.txt")).exists());
    }
    Ok(())
}

#[tokio::test]
async fn task_attachments_get_safe_names_outside_git_and_go_with_the_task() -> TestResult {
    let h = harness_configured(|c| c.upload_max_bytes = LIMIT)?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    h.github.set_perm("alice", "acme/other", perms("write"));
    let data = h.app.config.data_dir.clone();
    std::fs::create_dir_all(data.join("sandboxes/s1/repo"))?;
    std::fs::create_dir_all(data.join("sandboxes/s2/repo"))?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s2", "u-alice", TaskStatus::Closed, None).await?;
    let folder = data.join("sandboxes/s1/attachments/tasks/t1");

    // Read users cannot drop onto the terminal; unknown tasks are 404.
    let refused = upload(
        &h,
        Method::POST,
        "/api/tasks/t1/attachments?name=a.png",
        b"png",
        &bob,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.body);
    let unknown = upload(
        &h,
        Method::POST,
        "/api/tasks/nope/attachments?name=a.png",
        b"png",
        &alice,
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    // The absolute path the agent opens; a name that needs no quoting; a clash gets a new name.
    let first = upload(
        &h,
        Method::POST,
        "/api/tasks/t1/attachments?name=Screen%20Shot%20(1).png",
        b"\x89PNG one",
        &alice,
    )
    .await?;
    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    assert_eq!(first.body["name"], "Screen-Shot-1.png");
    assert_eq!(
        first.body["path"],
        folder.join("Screen-Shot-1.png").to_string_lossy().as_ref()
    );
    let second = upload(
        &h,
        Method::POST,
        "/api/tasks/t1/attachments?name=Screen%20Shot%20(1).png",
        b"\x89PNG two",
        &alice,
    )
    .await?;
    assert_eq!(
        second.body["name"], "Screen-Shot-1-2.png",
        "{:?}",
        second.body
    );
    assert_eq!(
        std::fs::read(folder.join("Screen-Shot-1.png"))?,
        b"\x89PNG one"
    );
    assert_eq!(
        std::fs::read(folder.join("Screen-Shot-1-2.png"))?,
        b"\x89PNG two"
    );
    // A path as the name keeps its last part only.
    let traversal = upload(
        &h,
        Method::POST,
        "/api/tasks/t1/attachments?name=..%2F..%2Frepo%2FREADME.md",
        b"x",
        &alice,
    )
    .await?;
    assert_eq!(traversal.body["name"], "README.md", "{:?}", traversal.body);
    assert!(!data.join("sandboxes/s1/repo/README.md").exists());
    let too_big = upload(
        &h,
        Method::POST,
        "/api/tasks/t1/attachments?name=big.pdf",
        &vec![0; 5000],
        &alice,
    )
    .await?;
    assert_eq!(too_big.status, StatusCode::UNPROCESSABLE_ENTITY);

    let audit = h.store.automation_audit("task:t1", 10).await?;
    assert!(
        audit.iter().any(|e| e.action == "attachment.uploaded"
            && e.detail.as_deref() == Some("Screen-Shot-1.png (8 bytes)")),
        "{audit:?}"
    );

    // A link planted where the attachments folder goes (a contained shell is root in its
    // sandbox) is refused, and nothing is written through it.
    let target = data.join("sandboxes/s1/repo");
    std::os::unix::fs::symlink(&target, data.join("sandboxes/s2/attachments"))?;
    let planted = upload(
        &h,
        Method::POST,
        "/api/tasks/t2/attachments?name=x.txt",
        b"x",
        &alice,
    )
    .await?;
    assert_eq!(planted.status, StatusCode::FORBIDDEN, "{:?}", planted.body);
    assert!(!target.join("tasks").exists());

    // Deleting the task removes its attachments (TSK-009).
    let deleted = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{:?}", deleted.body);
    assert!(!folder.exists());
    assert!(data.join("sandboxes/s1/attachments/tasks").is_dir());
    assert!(h.store.task(&TaskId::parse("t1")?).await?.is_none());
    assert!(leftovers(&data).is_empty());
    Ok(())
}

async fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) -> TestResult {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

#[tokio::test]
async fn composer_drafts_are_kept_per_user_outside_git_and_need_write_access() -> TestResult {
    let h = harness_configured(|c| c.upload_max_bytes = LIMIT)?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let data = h.app.config.data_dir.clone();
    std::fs::create_dir_all(data.join("sandboxes/s1/repo"))?;
    let folder = data.join("sandboxes/s1/attachments/drafts/u-alice");
    let url = "/api/sandboxes/s1/attachments?name=Screen%20Shot.png";

    let refused = upload(&h, Method::POST, url, b"png", &bob).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.body);
    let unknown = upload(
        &h,
        Method::POST,
        "/api/sandboxes/nope/attachments?name=a.png",
        b"png",
        &alice,
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    let first = upload(&h, Method::POST, url, b"\x89PNG one", &alice).await?;
    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    assert_eq!(first.body["name"], "Screen-Shot.png");
    assert_eq!(
        first.body["path"],
        folder.join("Screen-Shot.png").to_string_lossy().as_ref()
    );
    assert_eq!(
        std::fs::read(folder.join("Screen-Shot.png"))?,
        b"\x89PNG one"
    );
    let second = upload(&h, Method::POST, url, b"\x89PNG two", &alice).await?;
    assert_eq!(
        second.body["name"], "Screen-Shot-2.png",
        "{:?}",
        second.body
    );

    // A draft older than a week is removed by the owner's next upload; a fresh one stays.
    let old = std::fs::File::options()
        .write(true)
        .open(folder.join("Screen-Shot.png"))?;
    old.set_modified(std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 3600))?;
    drop(old);
    let third = upload(&h, Method::POST, url, b"\x89PNG three", &alice).await?;
    assert_eq!(third.status, StatusCode::CREATED, "{:?}", third.body);
    assert_eq!(
        third.body["name"], "Screen-Shot.png",
        "the old draft's name is free again"
    );
    assert_eq!(
        std::fs::read(folder.join("Screen-Shot.png"))?,
        b"\x89PNG three"
    );
    assert_eq!(
        std::fs::read(folder.join("Screen-Shot-2.png"))?,
        b"\x89PNG two"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_attachments_belong_to_the_shell_owner_and_end_with_the_shell() -> TestResult {
    let dir = tempfile::Builder::new().prefix("yup").tempdir_in("/tmp")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        dir: Some(dir),
        configure: Some(Box::new(|c| c.contained = false)),
        ..HarnessOptions::default()
    })?;
    assert!(h.app.set_shell_program(PathBuf::from("/bin/sh")));
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let carol = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("carol", "acme/fixture", perms("write"));
    let data = h.app.config.data_dir.clone();
    std::fs::create_dir_all(data.join("sandboxes/s1/repo"))?;

    let shell = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/shells",
        None,
        &alice,
    )
    .await?;
    assert_eq!(shell.status, StatusCode::CREATED, "{:?}", shell.body);
    let id = shell.body["id"].as_str().ok_or("shell id")?.to_owned();
    let path = format!("/api/sandboxes/s1/shells/{id}/attachments?name=data.csv");

    // Only the shell's owner may drop onto it (another writer does not even see it).
    let foreign = upload(&h, Method::POST, &path, b"a,b\n", &carol).await?;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{:?}", foreign.body);
    let stored = upload(&h, Method::POST, &path, b"a,b\n1,2\n", &alice).await?;
    assert_eq!(stored.status, StatusCode::CREATED, "{:?}", stored.body);
    let folder = data.join(format!("sandboxes/s1/attachments/shells/{id}"));
    assert_eq!(
        stored.body["path"],
        folder.join("data.csv").to_string_lossy().as_ref()
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("data.csv"))?,
        "a,b\n1,2\n"
    );
    // Nothing lands in the repository the shell started in.
    assert_eq!(
        std::fs::read_dir(data.join("sandboxes/s1/repo"))?.count(),
        0
    );

    // Closing the shell removes its attachments.
    let closed = call(
        &h.router,
        Method::DELETE,
        &format!("/api/sandboxes/s1/shells/{id}"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(closed.status, StatusCode::NO_CONTENT);
    wait_for(
        "the shell's attachments to go",
        Duration::from_secs(20),
        || !folder.exists(),
    )
    .await?;

    // A folder left by a shell that ended while no server ran is pruned at the next start.
    let orphan = data.join("sandboxes/s1/attachments/shells/0123456789abcdef");
    std::fs::create_dir_all(&orphan)?;
    std::fs::write(orphan.join("left.txt"), "x")?;
    let reaper = h.app.start_workspace_services().await;
    assert!(!orphan.exists());
    drop(reaper);
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A task running the fake agent (contained or not); returns the harness, alice's credentials
/// and the task id once the agent is ready.
async fn agent_task(
    contained: bool,
) -> Result<(Harness, Creds, TaskId), Box<dyn std::error::Error>> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let fake_dir = fake.parent().ok_or("fake-agent dir")?.to_path_buf();
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let dir = tempfile::Builder::new().prefix("yup").tempdir_in("/tmp")?;
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        session_host: Some(Arc::new(PtySessionHost::new(exe.clone()))),
        harness: Some(Arc::new(FakeAgentHarness::new(
            exe,
            fake,
            FakeAgentOptions::default(),
        ))),
        dir: Some(dir),
        configure: Some(Box::new(move |c| {
            c.contained = contained;
            c.contained_readonly = vec![fake_dir];
        })),
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let sandbox = seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.app
        .deps
        .store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a1")?,
            provider: Provider::Anthropic,
            label: "Claude".into(),
            credential: Some(h.app.deps.crypto.seal(b"sk-ant-oat01-upload-test")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    let data = h.app.config.data_dir.clone();
    let origin = data.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = data.join("seed");
    std::fs::create_dir_all(&seed)?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), "# fixture\n")?;
    git(&seed, &["add", "README.md"])?;
    git(&seed, &["commit", "-q", "-m", "init"])?;
    git(
        &seed,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )?;
    git(&seed, &["push", "-q", "origin", "main"])?;
    let repo = data
        .join("sandboxes")
        .join(sandbox.id.as_str())
        .join("repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        &data,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )?;
    let created = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "Read the attachment", "harness": "claude"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = TaskId::parse(created.body["id"].as_str().ok_or("task id")?)?;
    let app = h.app.clone();
    let ready = id.clone();
    wait_for("the agent", Duration::from_secs(30), move || {
        app.runtime(&ready)
            .hub
            .screen_text()
            .contains("FAKE-AGENT READY")
    })
    .await?;
    Ok((h, alice, id))
}

/// Drops a PDF onto the task terminal, has the agent read it by path, checks that the
/// worktree's `git status` stays clean, then deletes the task and its attachments.
async fn agent_reads_an_attachment(contained: bool) -> TestResult {
    let (h, alice, id) = agent_task(contained).await?;
    let pdf = b"%PDF-1.4 dropped by the user\n";
    let stored = upload(
        &h,
        Method::POST,
        &format!("/api/tasks/{}/attachments?name=brief.pdf", id.as_str()),
        pdf,
        &alice,
    )
    .await?;
    assert_eq!(stored.status, StatusCode::CREATED, "{:?}", stored.body);
    let path = stored.body["path"].as_str().ok_or("path")?.to_owned();
    // What the terminal receives from the UI: the path, as a local terminal pastes it.
    h.app
        .send_input(&id, format!("/read {path}\r").as_bytes())
        .await?;
    let expected = format!("READ brief.pdf OK {}", pdf.len());
    let app = h.app.clone();
    let probe = id.clone();
    let needle = expected.clone();
    let seen = wait_for(&expected, Duration::from_secs(15), move || {
        app.runtime(&probe).hub.screen_text().contains(&needle)
    })
    .await;
    if seen.is_err() {
        eprintln!("screen: {}", h.app.runtime(&id).hub.screen_text());
    }
    seen?;
    let task = h.store.task(&id).await?.ok_or("task")?;
    let worktree = PathBuf::from(task.worktree_path.ok_or("worktree")?);
    assert_eq!(
        git(
            &worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        )?,
        "",
        "the attachment must not show in git status"
    );
    // Leaving the session closes the task (HRN-002); then it can be deleted.
    h.app.send_input(&id, b"/exit\r").await?;
    let store = h.store.clone();
    let stopped = id.clone();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let live = store.live_attempt(&stopped).await?.is_some();
        let status = store.task(&stopped).await?.map(|t| t.status);
        if !live && status.is_some_and(|s| !s.is_live()) {
            break;
        }
        if Instant::now() > deadline {
            return Err("the task did not stop".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let deleted = call(
        &h.router,
        Method::DELETE,
        &format!("/api/tasks/{}", id.as_str()),
        None,
        &alice,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{:?}", deleted.body);
    assert!(
        !Path::new(&path).exists(),
        "deleting the task removes its attachments"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_reads_a_dropped_file_by_path_and_git_stays_clean() -> TestResult {
    agent_reads_an_attachment(false).await
}

/// AC-UI-43 (Linux): the same inside bubblewrap. Skipped with a message where bubblewrap cannot
/// create namespaces.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contained_agent_reads_a_dropped_file_by_its_path() -> TestResult {
    let probe = Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    match probe {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            eprintln!(
                "SKIPPED a_contained_agent_reads_a_dropped_file: bwrap cannot create namespaces: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return Ok(());
        }
        Err(err) => {
            eprintln!("SKIPPED a_contained_agent_reads_a_dropped_file: bwrap not runnable: {err}");
            return Ok(());
        }
    }
    agent_reads_an_attachment(true).await
}
