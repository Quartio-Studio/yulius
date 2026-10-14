//! Playgrounds (SPEC-099): a team tries Yulius on its own code without GitHub. A playground's Git
//! lives in a bare `origin.git` in its sandbox directory; nothing reaches GitHub. Real Git, a real
//! store, the fake GitHub (which records every state-changing call) and no GitHub App.

use crate::support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::git;
use support::{Creds, Harness, HarnessOptions, TestResult, call, harness_with};
use yulius::wiring::git::GitAdapter;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

/// A harness with the real Git adapter and no GitHub App, and the superadmin signed in.
async fn world() -> BoxResult<(Harness, Creds)> {
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        ..HarnessOptions::default()
    })?;
    let owner = support::setup_superadmin_creds(&h).await?;
    Ok((h, owner))
}

async fn from_template(h: &Harness, creds: &Creds, name: &str) -> BoxResult<support::Resp> {
    call(
        &h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "playground", "name": name, "template": "empty"})),
        creds,
    )
    .await
}

fn sandbox_dir(h: &Harness, body: &Value) -> PathBuf {
    h.app
        .config
        .data_dir
        .join("sandboxes")
        .join(body["id"].as_str().unwrap_or_default())
}

/// Branch names of a repository (`refs/heads/*`), sorted.
fn branches(repo: &Path) -> BoxResult<Vec<String>> {
    let out = git(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?;
    let mut names: Vec<String> = out.lines().map(str::to_owned).collect();
    names.sort();
    Ok(names)
}

fn no_github_writes(h: &Harness) -> BoxResult<()> {
    let writes = h.github.writes.lock().map_err(|_| "lock")?.clone();
    assert!(writes.is_empty(), "GitHub was written to: {writes:?}");
    Ok(())
}

/// PLY-001, PLY-002: without any GitHub App, a template becomes a playground whose Git is a bare
/// repository in its sandbox directory, with `main` and `dev`; tasks may start at once.
#[tokio::test]
async fn a_playground_starts_from_a_template_without_github() -> TestResult {
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "My Shop").await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let body = &made.body;
    assert_eq!(body["slug"], "my-shop");
    assert_eq!(body["name"], "my-shop");
    assert_eq!(body["playground"], true);
    assert_eq!(body["repo"]["owner"], "");
    assert_eq!(body["repo"]["url"], "");
    assert_eq!(body["access"], "admin");
    assert_eq!(body["onboarding"], "not_needed");
    assert_eq!(body["default_branch"], "main");

    let dir = sandbox_dir(&h, body);
    let origin = dir.join("origin.git");
    assert_eq!(
        git(&origin, &["rev-parse", "--is-bare-repository"])?.trim(),
        "true"
    );
    assert_eq!(branches(&origin)?, vec!["dev", "main"]);
    let repo = dir.join("repo");
    assert_eq!(
        git(&repo, &["remote", "get-url", "origin"])?.trim(),
        origin.to_string_lossy(),
        "the clone's origin is the playground's own bare repository"
    );
    git(&repo, &["rev-parse", "--verify", "origin/dev"])?;
    assert!(
        repo.join("AGENTS.md").exists(),
        "the template's scaffold is checked out"
    );

    // The same name again: a second playground, its own slug.
    let again = from_template(&h, &owner, "My Shop").await?;
    assert_eq!(again.status, StatusCode::CREATED, "{:?}", again.body);
    assert_eq!(again.body["slug"], "my-shop-2");

    // Unknown templates and names that leave nothing are refused.
    let bad = call(
        &h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "playground", "name": "x", "template": "nope"})),
        &owner,
    )
    .await?;
    assert_eq!(
        bad.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        bad.body
    );
    no_github_writes(&h)?;
    Ok(())
}

/// PLY-005: a playground is its maker's and the superadmin's; nobody else sees it.
#[tokio::test]
async fn only_its_maker_and_the_superadmin_see_a_playground() -> TestResult {
    let (h, owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    let bob = support::seed_github_user(&h, "u-bob", "bob").await?;
    let made = from_template(&h, &alice, "alices-lab").await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["access"], "admin");
    let path = format!("/api/sandboxes/{}", made.body["id"].as_str().ok_or("id")?);

    let seen = |list: &Value| {
        list.as_array()
            .is_some_and(|items| items.iter().any(|s| s["slug"] == "alices-lab"))
    };
    let mine = call(&h.router, Method::GET, "/api/sandboxes", None, &alice).await?;
    assert!(seen(&mine.body), "{:?}", mine.body);
    let theirs = call(&h.router, Method::GET, "/api/sandboxes", None, &bob).await?;
    assert!(!seen(&theirs.body), "{:?}", theirs.body);
    let direct = call(&h.router, Method::GET, &path, None, &bob).await?;
    assert_eq!(direct.status, StatusCode::NOT_FOUND);
    let all = call(&h.router, Method::GET, "/api/sandboxes", None, &owner).await?;
    assert!(seen(&all.body), "the superadmin sees every sandbox");
    no_github_writes(&h)?;
    Ok(())
}

/// A gzip tar of `stage` laid out as the browser packs a folder: `files/…` (the working tree)
/// and `git/…` (its `.git`, whole: the server takes only the history).
fn pack(stage: &Path) -> BoxResult<Vec<u8>> {
    let mut parts = Vec::new();
    for part in ["files", "git"] {
        if stage.join(part).exists() {
            parts.push(part);
        }
    }
    let output = std::process::Command::new("tar")
        .arg("-czf")
        .arg("-")
        .arg("-C")
        .arg(stage)
        .args(&parts)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(output.stdout)
}

/// Copies `from` into `to` (`cp -a`), `.git` included.
fn copy_tree(from: &Path, to: &Path) -> BoxResult<()> {
    std::fs::create_dir_all(to)?;
    let status = std::process::Command::new("cp")
        .arg("-a")
        .arg(format!("{}/.", from.display()))
        .arg(to)
        .status()?;
    if !status.success() {
        return Err("cp failed".into());
    }
    Ok(())
}

/// Stages a folder for [`pack`]: its working tree without `.git` under `files/`, its `.git`
/// under `git/`.
fn stage_folder(project: &Path, stage: &Path) -> BoxResult<()> {
    copy_tree(project, &stage.join("files"))?;
    let dotgit = stage.join("files/.git");
    if dotgit.exists() {
        std::fs::rename(&dotgit, stage.join("git"))?;
    }
    Ok(())
}

async fn upload(h: &Harness, creds: &Creds, name: &str, body: Vec<u8>) -> BoxResult<support::Resp> {
    support::repos::call_raw(
        &h.router,
        Method::POST,
        &format!("/api/playgrounds?name={name}"),
        body,
        &[("content-type", "application/gzip")],
        creds,
    )
    .await
}

fn file_at(repo: &Path, rev: &str, path: &str) -> BoxResult<String> {
    Ok(git(repo, &["show", &format!("{rev}:{path}")])?)
}

/// PLY-001, PLY-003: a folder with `.git` keeps its branches, tags and history; its uncommitted
/// and untracked work lands as one commit on its own branch, which `dev` starts from; ignored
/// files stay out.
#[tokio::test]
async fn a_folder_with_history_keeps_it_and_its_uncommitted_work() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project)?;
    git(&project, &["init", "-q", "-b", "trunk"])?;
    std::fs::write(project.join("README.md"), "v1\n")?;
    std::fs::write(project.join(".gitignore"), "build/\n")?;
    git(&project, &["add", "-A"])?;
    git(&project, &["commit", "-q", "-m", "first"])?;
    git(&project, &["tag", "v1"])?;
    git(&project, &["checkout", "-q", "-b", "feature"])?;
    std::fs::write(project.join("feature.txt"), "wip\n")?;
    git(&project, &["add", "-A"])?;
    git(&project, &["commit", "-q", "-m", "feature work"])?;
    git(&project, &["checkout", "-q", "trunk"])?;
    git(&project, &["gc", "-q"])?; // packed objects and packed-refs, as real repositories have
    std::fs::write(project.join("README.md"), "v2, not committed\n")?;
    std::fs::write(project.join("notes.txt"), "untracked\n")?;
    std::fs::create_dir_all(project.join("build"))?;
    std::fs::write(project.join("build/out.bin"), "ignored\n")?;
    let stage = tmp.path().join("stage");
    stage_folder(&project, &stage)?;

    let made = upload(&h, &owner, "Proj", pack(&stage)?).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["playground"], true);
    assert_eq!(made.body["slug"], "proj");
    assert_eq!(made.body["default_branch"], "trunk");
    assert_eq!(made.body["onboarding"], "not_needed");

    let origin = sandbox_dir(&h, &made.body).join("origin.git");
    assert_eq!(branches(&origin)?, vec!["dev", "feature", "trunk"]);
    git(&origin, &["rev-parse", "--verify", "refs/tags/v1"])?;
    assert_eq!(file_at(&origin, "feature", "feature.txt")?, "wip\n");
    let log = git(&origin, &["log", "--format=%s", "trunk"])?;
    assert_eq!(log.lines().count(), 2, "{log}");
    assert_eq!(log.lines().nth(1), Some("first"));
    assert_eq!(
        file_at(&origin, "trunk", "README.md")?,
        "v2, not committed\n"
    );
    assert_eq!(file_at(&origin, "trunk", "notes.txt")?, "untracked\n");
    assert!(
        file_at(&origin, "trunk", "build/out.bin").is_err(),
        "ignored files stay out"
    );
    assert_eq!(
        git(&origin, &["rev-parse", "dev"])?,
        git(&origin, &["rev-parse", "trunk"])?
    );
    let repo = sandbox_dir(&h, &made.body).join("repo");
    git(&repo, &["rev-parse", "--verify", "origin/dev"])?;
    assert!(
        !sandbox_dir(&h, &made.body).join("upload").exists(),
        "the staging folders are gone"
    );
    no_github_writes(&h)?;
    Ok(())
}

/// PLY-003: a folder without `.git` becomes one commit on `main`, and `dev` starts there.
#[tokio::test]
async fn a_folder_without_git_becomes_one_commit() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let stage = tmp.path().join("stage");
    std::fs::create_dir_all(stage.join("files/src"))?;
    std::fs::write(stage.join("files/README.md"), "# Lab\n")?;
    std::fs::write(stage.join("files/src/app.py"), "print('hi')\n")?;

    let made = upload(&h, &owner, "lab", pack(&stage)?).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["default_branch"], "main");
    let origin = sandbox_dir(&h, &made.body).join("origin.git");
    assert_eq!(branches(&origin)?, vec!["dev", "main"]);
    assert_eq!(git(&origin, &["rev-list", "--count", "main"])?.trim(), "1");
    assert_eq!(file_at(&origin, "dev", "src/app.py")?, "print('hi')\n");
    no_github_writes(&h)?;
    Ok(())
}

/// PLY-003: of an uploaded `.git` only the history is taken: its configuration, hooks and
/// attributes never reach the server's Git.
#[tokio::test]
async fn a_git_config_or_hook_in_the_upload_is_never_taken() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project)?;
    git(&project, &["init", "-q", "-b", "main"])?;
    std::fs::write(project.join("README.md"), "hello\n")?;
    git(&project, &["add", "-A"])?;
    git(&project, &["commit", "-q", "-m", "first"])?;
    let marker = tmp.path().join("ran");
    let hook = format!("#!/bin/sh\ntouch '{}'\n", marker.display());
    for name in [
        "post-checkout",
        "post-merge",
        "pre-commit",
        "reference-transaction",
    ] {
        let path = project.join(".git/hooks").join(name);
        std::fs::write(&path, &hook)?;
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(&path)
            .status()?;
    }
    let mut config = std::fs::read_to_string(project.join(".git/config"))?;
    config.push_str(&format!(
        "[core]\n\tfsmonitor = touch '{m}'\n\thooksPath = .git/hooks\n[filter \"x\"]\n\tclean = touch '{m}'\n",
        m = marker.display()
    ));
    std::fs::write(project.join(".git/config"), config)?;
    std::fs::create_dir_all(project.join(".git/info"))?;
    std::fs::write(project.join(".git/info/attributes"), "* filter=x\n")?;
    let stage = tmp.path().join("stage");
    stage_folder(&project, &stage)?;
    // A `.git` hidden among the working files is never written either.
    std::fs::create_dir_all(stage.join("files/sub/.git"))?;
    std::fs::write(
        stage.join("files/sub/.git/config"),
        "[core]\n\tbare = true\n",
    )?;

    let made = upload(&h, &owner, "proj", pack(&stage)?).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let dir = sandbox_dir(&h, &made.body);
    assert!(!marker.exists(), "nothing of the upload ran");
    let origin_config = std::fs::read_to_string(dir.join("origin.git/config"))?;
    assert!(!origin_config.contains("fsmonitor") && !origin_config.contains("filter"));
    assert!(!dir.join("origin.git/info/attributes").exists());
    assert!(!dir.join("origin.git/hooks/post-checkout").exists());
    assert!(!dir.join("repo/.git/hooks/post-checkout").exists());
    assert!(!dir.join("repo/sub/.git").exists());
    assert_eq!(
        file_at(&dir.join("origin.git"), "main", "README.md")?,
        "hello\n"
    );
    Ok(())
}

/// PLY-004: an unreadable or oversized upload is refused with its reason and leaves no sandbox
/// and no directory behind.
#[tokio::test]
async fn an_archive_over_the_bounds_is_refused_and_leaves_nothing() -> TestResult {
    let (h, owner) = world().await?;
    let sandboxes = h.app.config.data_dir.join("sandboxes");
    let count = || std::fs::read_dir(&sandboxes).map_or(0, Iterator::count);
    let before = count();

    let garbage = upload(&h, &owner, "junk", b"not a gzip archive".to_vec()).await?;
    assert_eq!(
        garbage.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        garbage.body
    );
    let blank = tempfile::tempdir()?;
    std::fs::create_dir_all(blank.path().join("files"))?;
    let empty = upload(&h, &owner, "empty", pack(blank.path())?).await?;
    assert_eq!(
        empty.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        empty.body
    );
    let declared = support::repos::call_raw(
        &h.router,
        Method::POST,
        "/api/playgrounds?name=huge",
        b"x".to_vec(),
        &[("content-length", "4294967296")],
        &owner,
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
            .is_some_and(|m| m.contains("1 GB")),
        "{:?}",
        declared.body
    );

    assert_eq!(count(), before, "no directory is left behind");
    let list = call(&h.router, Method::GET, "/api/sandboxes", None, &owner).await?;
    assert_eq!(
        list.body.as_array().map(Vec::len),
        Some(0),
        "{:?}",
        list.body
    );
    let tmp = h.app.config.data_dir.join("tmp");
    assert_eq!(
        std::fs::read_dir(&tmp).map_or(0, Iterator::count),
        0,
        "no upload file stays"
    );
    Ok(())
}

fn valid_spec(task: &str) -> String {
    format!(
        "---\nyulius-task: {task}\nstatus: in-progress\n---\n# Change {task}\n\n\
         ## Goal\nShip change {task}.\n\n\
         ## Context\nA playground.\n\n\
         ## Requirements\n- It lands.\n\n\
         ## Plan\n- [x] Write the spec\n- [x] Make the change\n\n\
         ## Acceptance criteria\n- It is merged.\n\n\
         ## Test plan\n- The playground test.\n\n\
         ## Decisions\n- None yet.\n"
    )
}

/// A running task `id` of the playground `body` (the superadmin's) on its own worktree and branch
/// `feat/<id>` from `origin/dev`, with its spec, `files` committed and its steps at `steps`.
async fn playground_task(
    h: &Harness,
    body: &Value,
    id: &str,
    files: &[(&str, &str)],
    steps: &[&str],
) -> BoxResult<PathBuf> {
    use yulius_app::model::StepRecord;
    use yulius_app::ports::store::{CoordinationStore, UserStore};
    use yulius_domain::ids::{StepId, TaskId};
    let repo = sandbox_dir(h, body).join("repo");
    let worktree = sandbox_dir(h, body).join("worktrees").join(id);
    git(&repo, &["fetch", "-q", "origin"])?;
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("feat/{id}"),
            worktree.to_str().ok_or("path")?,
            "origin/dev",
        ],
    )?;
    std::fs::create_dir_all(worktree.join("docs/specs"))?;
    std::fs::write(worktree.join(format!("docs/specs/{id}.md")), valid_spec(id))?;
    for (path, text) in files {
        std::fs::write(worktree.join(path), text)?;
    }
    git(&worktree, &["add", "-A"])?;
    git(&worktree, &["commit", "-q", "-m", &format!("work on {id}")])?;
    let owner = h.store.superadmin().await?.ok_or("superadmin")?;
    support::seed_task(
        h,
        id,
        body["id"].as_str().ok_or("id")?,
        owner.id.as_str(),
        yulius_domain::task::TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
    let task = TaskId::parse(id)?;
    let records = steps
        .iter()
        .enumerate()
        .map(|(i, status)| {
            Ok(StepRecord {
                id: StepId::parse(&format!("{id}-step-{i}"))?,
                task_id: task.clone(),
                ordinal: u32::try_from(i)?,
                title: format!("Step {i}"),
                status: (*status).to_owned(),
                note: None,
                updated_by: None,
                updated_at: None,
                effort: 1,
            })
        })
        .collect::<BoxResult<Vec<_>>>()?;
    h.store
        .replace_steps(&task, &records, "test", yulius_domain::time::Timestamp(2))
        .await?;
    Ok(worktree)
}

async fn merge(h: &Harness, creds: &Creds, task: &str) -> BoxResult<support::Resp> {
    call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{task}/delivery/merge"),
        None,
        creds,
    )
    .await
}

/// PLY-007: in a playground, the task's merge action and the agent's `delivery_merge` squash
/// the branch onto the playground's `dev` as one commit with a conventional title, and the task
/// is merged; no GitHub call.
#[tokio::test]
async fn a_playgrounds_task_merges_into_its_local_dev() -> TestResult {
    use axum::extract::connect_info::MockConnectInfo;
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "shop").await?;
    let origin = sandbox_dir(&h, &made.body).join("origin.git");
    let before = git(&origin, &["rev-parse", "dev"])?.trim().to_owned();
    playground_task(
        &h,
        &made.body,
        "t1",
        &[("feature.txt", "one\n")],
        &["done", "done"],
    )
    .await?;

    let merged = merge(&h, &owner, "t1").await?;
    assert_eq!(merged.status, StatusCode::OK, "{:?}", merged.body);
    assert_eq!(
        merged.body["stage"], "merged_integration",
        "{:?}",
        merged.body
    );
    let dev = git(&origin, &["rev-parse", "dev"])?.trim().to_owned();
    assert_eq!(
        git(&origin, &["rev-parse", "dev^"])?.trim(),
        before,
        "one commit on dev"
    );
    let message = git(&origin, &["log", "-1", "--format=%B", "dev"])?;
    assert!(message.starts_with("feat: task t1\n"), "{message}");
    assert!(message.contains("Task #"), "{message}");
    assert_eq!(file_at(&origin, "dev", "feature.txt")?, "one\n");
    assert_eq!(
        git(&origin, &["rev-parse", "main"])?.trim(),
        before,
        "main waits for a promotion"
    );
    let again = merge(&h, &owner, "t1").await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{:?}", again.body);

    // The agent's own tool, under the task owner's authority.
    let worktree =
        playground_task(&h, &made.body, "t2", &[("second.txt", "two\n")], &["done"]).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?.trim().to_owned();
    support::seed_live_attempt(&h, "t2", "tok-t2").await?;
    let internal = yulius_server::internal_router(h.app.clone()).layer(MockConnectInfo(
        std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
    ));
    let agent = Creds {
        bearer: Some("tok-t2".into()),
        ..Creds::default()
    };
    let called = call(
        &internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "delivery_merge", "arguments": {"reviewed_head": head}}})),
        &agent,
    )
    .await?;
    assert_eq!(called.body["result"]["isError"], false, "{}", called.body);
    assert_eq!(git(&origin, &["rev-parse", "dev^"])?.trim(), dev);
    assert_eq!(file_at(&origin, "dev", "second.txt")?, "two\n");
    no_github_writes(&h)?;
    Ok(())
}

/// PLY-007: an unfinished task is refused with what to do, and nothing lands.
#[tokio::test]
async fn an_unfinished_task_is_not_merged() -> TestResult {
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "shop").await?;
    let origin = sandbox_dir(&h, &made.body).join("origin.git");
    let before = git(&origin, &["rev-parse", "dev"])?;

    playground_task(
        &h,
        &made.body,
        "t1",
        &[("a.txt", "a\n")],
        &["done", "in_progress"],
    )
    .await?;
    let open = merge(&h, &owner, "t1").await?;
    assert_eq!(open.status, StatusCode::CONFLICT, "{:?}", open.body);
    assert!(text(&open).contains("step"), "{:?}", open.body);

    let worktree = playground_task(&h, &made.body, "t2", &[("b.txt", "b\n")], &["done"]).await?;
    std::fs::write(worktree.join("b.txt"), "changed, not committed\n")?;
    let dirty = merge(&h, &owner, "t2").await?;
    assert_eq!(dirty.status, StatusCode::CONFLICT, "{:?}", dirty.body);
    assert!(text(&dirty).contains("uncommitted"), "{:?}", dirty.body);

    assert_eq!(
        git(&origin, &["rev-parse", "dev"])?,
        before,
        "nothing landed"
    );
    Ok(())
}

fn text(resp: &support::Resp) -> String {
    resp.body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// PLY-007: a branch that conflicts with the playground's `dev` is refused naming the files and
/// the remedy.
#[tokio::test]
async fn a_conflicting_branch_is_refused_with_the_files() -> TestResult {
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "shop").await?;
    playground_task(&h, &made.body, "t1", &[("README.md", "# one\n")], &["done"]).await?;
    playground_task(&h, &made.body, "t2", &[("README.md", "# two\n")], &["done"]).await?;
    assert_eq!(merge(&h, &owner, "t1").await?.status, StatusCode::OK);
    let refused = merge(&h, &owner, "t2").await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    let message = text(&refused);
    assert!(message.contains("README.md"), "{message}");
    assert!(message.contains("git merge origin/dev"), "{message}");
    Ok(())
}

/// PLY-008: with a GitHub App configured, a ready playground task still opens no pull request at
/// the end of its turn, and its delivery view and Git page carry the playground note instead of
/// a GitHub error.
#[tokio::test]
async fn a_playground_never_calls_github() -> TestResult {
    let (h, owner) = world().await?;
    support::configure_github_app(&h).await?;
    let made = from_template(&h, &owner, "shop").await?;
    playground_task(&h, &made.body, "t1", &[("a.txt", "a\n")], &["done"]).await?;
    h.app
        .auto_open_pr(&yulius_domain::ids::TaskId::parse("t1")?)
        .await;
    h.app.reconcile_deliveries().await;
    h.app.ci_sync_all().await;
    no_github_writes(&h)?;

    let note = "This playground keeps its Git on this server";
    let delivery = call(
        &h.router,
        Method::GET,
        "/api/tasks/t1/delivery",
        None,
        &owner,
    )
    .await?;
    assert_eq!(delivery.status, StatusCode::OK, "{:?}", delivery.body);
    let notes = delivery.body["notes"].to_string();
    assert!(notes.contains(note), "{notes}");
    assert!(!notes.contains("GitHub is not available"), "{notes}");
    let sid = made.body["id"].as_str().ok_or("id")?;
    let overview = call(
        &h.router,
        Method::GET,
        &format!("/api/sandboxes/{sid}/git"),
        None,
        &owner,
    )
    .await?;
    assert_eq!(overview.status, StatusCode::OK, "{:?}", overview.body);
    let notes = overview.body["notes"].to_string();
    assert!(notes.contains(note), "{notes}");
    assert!(!notes.contains("GitHub"), "{notes}");
    Ok(())
}

/// PLY-003: an uploaded `.git/objects/info/alternates` is never taken, so an upload cannot make
/// the server's Git read another repository on the server (here: a commit only it has).
#[tokio::test]
async fn an_uploaded_alternate_never_reads_another_repository() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&other)?;
    git(&other, &["init", "-q", "-b", "main"])?;
    std::fs::write(other.join("secret.txt"), "not yours\n")?;
    git(&other, &["add", "-A"])?;
    git(&other, &["commit", "-q", "-m", "secret"])?;
    let secret = git(&other, &["rev-parse", "HEAD"])?.trim().to_owned();

    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project)?;
    git(&project, &["init", "-q", "-b", "main"])?;
    std::fs::write(project.join("README.md"), "hello\n")?;
    git(&project, &["add", "-A"])?;
    git(&project, &["commit", "-q", "-m", "first"])?;
    std::fs::create_dir_all(project.join(".git/objects/info"))?;
    std::fs::write(
        project.join(".git/objects/info/alternates"),
        format!("{}\n", other.join(".git/objects").display()),
    )?;
    std::fs::write(
        project.join(".git/refs/heads/stolen"),
        format!("{secret}\n"),
    )?;
    let stage = tmp.path().join("stage");
    stage_folder(&project, &stage)?;

    let made = upload(&h, &owner, "proj", pack(&stage)?).await?;
    if made.status == StatusCode::CREATED {
        let origin = sandbox_dir(&h, &made.body).join("origin.git");
        assert!(
            git(&origin, &["cat-file", "-e", &secret]).is_err(),
            "the other repository's commit reached the playground"
        );
    }
    let list = call(&h.router, Method::GET, "/api/sandboxes", None, &owner).await?;
    let leaked = list.body.as_array().is_some_and(|items| {
        items.iter().any(|s| {
            let origin = sandbox_dir(&h, s).join("origin.git");
            git(&origin, &["cat-file", "-e", &secret]).is_ok()
        })
    });
    assert!(!leaked, "no playground holds the other repository's commit");
    Ok(())
}
