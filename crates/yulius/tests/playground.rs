//! Playgrounds (SPEC-099): a team tries Yulius on its own code without GitHub. A playground's Git
//! lives in a bare repository on the server, outside its sandbox; nothing reaches GitHub. Real Git, a real
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

/// A playground's `origin`: a bare repository of the server, outside the sandbox (ADR-0081).
fn origin_of(h: &Harness, body: &Value) -> PathBuf {
    h.app
        .config
        .data_dir
        .join("playgrounds")
        .join(format!("{}.git", body["id"].as_str().unwrap_or_default()))
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
    let origin = origin_of(&h, body);
    assert_eq!(
        git(&origin, &["rev-parse", "--is-bare-repository"])?.trim(),
        "true"
    );
    // Outside the sandbox, which its processes may write: its hooks and configuration are the
    // server's alone, and the server's Git runs them outside containment (review round 2).
    assert!(
        !origin.starts_with(&dir),
        "{} is inside {}",
        origin.display(),
        dir.display()
    );
    assert!(!dir.join("origin.git").exists());
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

    let origin = origin_of(&h, &made.body);
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
    let origin = origin_of(&h, &made.body);
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
    let origin_config = std::fs::read_to_string(origin_of(&h, &made.body).join("config"))?;
    assert!(!origin_config.contains("fsmonitor") && !origin_config.contains("filter"));
    assert!(!origin_of(&h, &made.body).join("info/attributes").exists());
    assert!(
        !origin_of(&h, &made.body)
            .join("hooks/post-checkout")
            .exists()
    );
    assert!(!dir.join("repo/.git/hooks/post-checkout").exists());
    assert!(!dir.join("repo/sub/.git").exists());
    assert_eq!(
        file_at(&origin_of(&h, &made.body), "main", "README.md")?,
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
    let origin = origin_of(&h, &made.body);
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
    let origin = origin_of(&h, &made.body);
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
        let origin = origin_of(&h, &made.body);
        assert!(
            git(&origin, &["cat-file", "-e", &secret]).is_err(),
            "the other repository's commit reached the playground"
        );
    }
    let list = call(&h.router, Method::GET, "/api/sandboxes", None, &owner).await?;
    let leaked = list.body.as_array().is_some_and(|items| {
        items.iter().any(|s| {
            let origin = origin_of(&h, s);
            git(&origin, &["cat-file", "-e", &secret]).is_ok()
        })
    });
    assert!(!leaked, "no playground holds the other repository's commit");
    Ok(())
}

/// Calls an MCP tool as the run whose task token is `token`.
async fn agent_tool(h: &Harness, token: &str, name: &str, arguments: Value) -> BoxResult<Value> {
    let agent = Creds {
        bearer: Some(token.into()),
        ..Creds::default()
    };
    let called = call(
        &h.internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}})),
        &agent,
    )
    .await?;
    Ok(called.body["result"].clone())
}

/// Review B1 (PLY-005, PLY-006): a playground has no deploy, so its maker — anyone signed in —
/// can never make the server run a command: not through the profile, not through an uploaded
/// `yulius.toml`, not through an environment run.
#[tokio::test]
async fn a_playground_runs_no_environment_command() -> TestResult {
    let (h, _owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    let tmp = tempfile::tempdir()?;
    let stage = tmp.path().join("stage");
    std::fs::create_dir_all(stage.join("files"))?;
    std::fs::write(stage.join("files/README.md"), "# Lab\n")?;
    let marker = tmp.path().join("ran");
    std::fs::write(
        stage.join("files/yulius.toml"),
        format!(
            "[delivery]\npreset = \"standard\"\n\n[[delivery.environments]]\nname = \"dev-vm\"\n\
             after = \"integration\"\ncommand = \"touch '{}'\"\nverify = \"command\"\n",
            marker.display()
        ),
    )?;
    let made = upload(&h, &alice, "lab", pack(&stage)?).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let environments = made.body["profile"]["environments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        environments.iter().all(|e| e["command"].is_null()),
        "an uploaded command is never kept: {environments:?}"
    );
    let sid = made.body["id"].as_str().ok_or("id")?;
    let patched = call(
        &h.router,
        Method::PATCH,
        &format!("/api/sandboxes/{sid}"),
        Some(json!({"profile": {
            "preset": "standard", "integration_branch": "dev", "production_branch": "main",
            "merge": "squash", "promote": "merge_commit", "auto_promote": false,
            "environments": [{"name": "dev-vm", "after": "integration", "verify": "command",
                "command": format!("touch '{}'", marker.display())}]
        }})),
        &alice,
    )
    .await?;
    assert_eq!(
        patched.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        patched.body
    );
    let run = call(
        &h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/delivery/environments/dev-vm/runs"),
        Some(json!({"confirm": true})),
        &alice,
    )
    .await?;
    assert!(run.status.is_client_error(), "{:?}", run.body);
    assert!(!marker.exists(), "no command ran");
    Ok(())
}

/// Review B2, M4 (PLY-002): a playground's agent gets no GitHub credential, and its GitHub tools
/// (issues, promotion) answer with the playground note instead of calling GitHub.
#[tokio::test]
async fn a_playgrounds_agent_gets_no_github_token_or_tool() -> TestResult {
    let (h, owner) = world().await?;
    support::configure_github_app(&h).await?;
    let made = from_template(&h, &owner, "shop").await?;
    playground_task(&h, &made.body, "t1", &[("a.txt", "a\n")], &["done"]).await?;
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let agent = Creds {
        bearer: Some("tok-t1".into()),
        ..Creds::default()
    };
    for body in [
        json!({"protocol": "https", "host": "github.com"}),
        json!({"protocol": "https", "host": "github.com", "path": "Quartio-Studio/shop.git"}),
    ] {
        let credential = call(
            &h.internal,
            Method::POST,
            "/internal/git-credential",
            Some(body),
            &agent,
        )
        .await?;
        assert_eq!(
            credential.status,
            StatusCode::FORBIDDEN,
            "{:?}",
            credential.body
        );
        assert!(credential.body.get("token").is_none());
    }
    let issue = agent_tool(&h, "tok-t1", "issue_create", json!({"title": "x", "body": "Summary.\n\n## Acceptance criteria\n- [ ] It works", "labels": ["type: bug", "priority: low", "area: x"]})).await?;
    assert_eq!(issue["isError"], true, "{issue}");
    assert!(issue.to_string().contains("playground"), "{issue}");
    let deploy = agent_tool(&h, "tok-t1", "cd_deploy", json!({"environment": "dev"})).await?;
    assert_eq!(deploy["isError"], true, "{deploy}");
    assert!(deploy.to_string().contains("playground"), "{deploy}");
    let sid = made.body["id"].as_str().ok_or("id")?;
    let promoted = call(
        &h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/delivery/promote"),
        None,
        &owner,
    )
    .await?;
    assert_eq!(promoted.status, StatusCode::CONFLICT, "{:?}", promoted.body);
    assert!(
        text(&promoted).contains("playground"),
        "{:?}",
        promoted.body
    );
    no_github_writes(&h)?;
    Ok(())
}

/// Review M1 (PLY-003): a browser cannot read file modes or links, so a repository's executable
/// files and links stay as committed when only that differs in the upload.
#[tokio::test]
async fn an_upload_keeps_executable_bits_and_links_of_the_repository() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join("scripts"))?;
    git(&project, &["init", "-q", "-b", "main"])?;
    std::fs::write(project.join("scripts/run.sh"), "#!/bin/sh\necho hi\n")?;
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(project.join("scripts/run.sh"))
        .status()?;
    std::os::unix::fs::symlink("scripts/run.sh", project.join("run"))?;
    git(&project, &["add", "-A"])?;
    git(&project, &["commit", "-q", "-m", "first"])?;
    // As the browser packs it: links followed, every mode 0644.
    let stage = tmp.path().join("stage");
    stage_folder(&project, &stage)?;
    std::fs::remove_file(stage.join("files/run"))?;
    std::fs::write(stage.join("files/run"), "#!/bin/sh\necho hi\n")?;
    std::process::Command::new("chmod")
        .args(["-R", "a-x,u+rwX"])
        .arg(stage.join("files"))
        .status()?;

    let made = upload(&h, &owner, "proj", pack(&stage)?).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let origin = origin_of(&h, &made.body);
    assert_eq!(
        git(&origin, &["rev-list", "--count", "main"])?.trim(),
        "1",
        "nothing but modes and links differed: no extra commit"
    );
    let tree = git(&origin, &["ls-tree", "-r", "main"])?;
    assert!(
        tree.contains("100755 blob") && tree.contains("scripts/run.sh"),
        "{tree}"
    );
    assert!(
        tree.lines()
            .any(|l| l.starts_with("120000") && l.ends_with("\trun")),
        "{tree}"
    );
    Ok(())
}

/// Review M2 (PLY-003): a shallow repository keeps its branches and history.
#[tokio::test]
async fn a_shallow_repository_keeps_its_branches() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let full = tmp.path().join("full");
    std::fs::create_dir_all(&full)?;
    git(&full, &["init", "-q", "-b", "main"])?;
    for n in 1..=3 {
        std::fs::write(full.join("n.txt"), format!("{n}\n"))?;
        git(&full, &["add", "-A"])?;
        git(&full, &["commit", "-q", "-m", &format!("commit {n}")])?;
    }
    let shallow = tmp.path().join("shallow");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--depth",
            "2",
            &format!("file://{}", full.display()),
            shallow.to_str().ok_or("path")?,
        ],
    )?;
    let stage = tmp.path().join("stage");
    stage_folder(&shallow, &stage)?;
    let made = upload(&h, &owner, "shallow", pack(&stage)?).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let origin = origin_of(&h, &made.body);
    let log = git(&origin, &["log", "--format=%s", "main"])?;
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        vec!["commit 3", "commit 2"],
        "{log}"
    );
    Ok(())
}

/// Review minor 3: a squash that landed before its stage was recorded (a crash in between) is
/// recorded as merged when merged again, instead of "nothing to deliver" forever.
#[tokio::test]
async fn a_merge_landed_before_its_stage_was_recorded_is_recorded() -> TestResult {
    use yulius_app::ports::store_ext::DeliveryStore;
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "shop").await?;
    playground_task(&h, &made.body, "t1", &[("a.txt", "a\n")], &["done"]).await?;
    assert_eq!(merge(&h, &owner, "t1").await?.status, StatusCode::OK);
    let id = yulius_domain::ids::TaskId::parse("t1")?;
    let merged = h.store.delivery(&id).await?.ok_or("delivery")?;
    let mut lost = merged.clone();
    lost.stage = yulius_domain::delivery::DeliveryStage::None;
    assert!(h.store.transition_delivery(&merged.stage, &lost).await?);
    let again = merge(&h, &owner, "t1").await?;
    assert_eq!(again.status, StatusCode::OK, "{:?}", again.body);
    assert_eq!(again.body["stage"], "merged_integration");
    Ok(())
}

/// Review minor 6: a person makes at most 10 playgrounds (each holds a repository on the
/// server's disk); the superadmin's are not counted.
#[tokio::test]
async fn a_person_makes_at_most_ten_playgrounds() -> TestResult {
    let (h, _owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    for n in 0..10 {
        let made = from_template(&h, &alice, &format!("lab {n}")).await?;
        assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    }
    let refused = from_template(&h, &alice, "lab 10").await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert!(
        text(&refused).contains("10 playgrounds"),
        "{:?}",
        refused.body
    );
    Ok(())
}

/// Review M3 (PLY-004): a client that goes away while the playground is being set up (a closed
/// tab) leaves no directory without its playground: the setup ends on its own.
#[tokio::test]
async fn a_closed_tab_during_setup_leaves_no_stray_directory() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project)?;
    git(&project, &["init", "-q", "-b", "main"])?;
    for n in 0..200 {
        std::fs::write(project.join(format!("f{n}.txt")), format!("{n}\n"))?;
    }
    git(&project, &["add", "-A"])?;
    git(&project, &["commit", "-q", "-m", "first"])?;
    let stage = tmp.path().join("stage");
    stage_folder(&project, &stage)?;
    let body = pack(&stage)?;
    let sandboxes = h.app.config.data_dir.join("sandboxes");
    let router = h.router.clone();
    let request = tokio::spawn(async move {
        support::repos::call_raw(
            &router,
            Method::POST,
            "/api/playgrounds?name=proj",
            body,
            &[],
            &owner,
        )
        .await
        .map(|r| r.status)
        .map_err(|e| e.to_string())
    });
    // Close the "tab" once the setup has started.
    let started = std::time::Instant::now();
    while !std::fs::read_dir(&sandboxes)
        .is_ok_and(|mut dirs| dirs.any(|d| d.is_ok_and(|d| d.path().join("upload").exists())))
    {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the setup never started"
        );
        tokio::task::yield_now().await;
    }
    request.abort();
    // Every directory left belongs to a recorded playground, once the setup ended by itself.
    use yulius_app::ports::store::SandboxStore;
    let settled = std::time::Instant::now();
    loop {
        let dirs = std::fs::read_dir(&sandboxes).map_or(0, Iterator::count);
        let recorded = h.store.sandboxes().await?.len();
        let staging = std::fs::read_dir(&sandboxes)?
            .any(|d| d.is_ok_and(|d| d.path().join("upload").exists()));
        if !staging && dirs == recorded {
            break;
        }
        assert!(
            settled.elapsed() < std::time::Duration::from_secs(60),
            "{dirs} directories, {recorded} playgrounds"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

/// Review round 2 (PLY-002): the clone's `origin` URL is the sandbox's to write, so the server
/// fetches and lands a playground's merges by the playground's own path, never by that URL: a
/// clone pointed at another playground neither reads it nor changes its `dev`.
#[tokio::test]
async fn a_rewritten_origin_url_is_never_followed() -> TestResult {
    let (h, owner) = world().await?;
    let victim = from_template(&h, &owner, "victim").await?;
    let victim_origin = origin_of(&h, &victim.body);
    let victim_dev = git(&victim_origin, &["rev-parse", "dev"])?;
    let made = from_template(&h, &owner, "shop").await?;
    playground_task(&h, &made.body, "t1", &[("a.txt", "a\n")], &["done"]).await?;
    let repo = sandbox_dir(&h, &made.body).join("repo");
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            victim_origin.to_str().ok_or("path")?,
        ],
    )?;

    let merged = merge(&h, &owner, "t1").await?;
    assert_eq!(merged.status, StatusCode::OK, "{:?}", merged.body);
    assert_eq!(
        git(&victim_origin, &["rev-parse", "dev"])?,
        victim_dev,
        "the victim's dev is untouched"
    );
    assert_eq!(file_at(&origin_of(&h, &made.body), "dev", "a.txt")?, "a\n");
    Ok(())
}

/// Review round 2: a removed playground keeps its repository on the server's disk, so it still
/// counts toward its maker's ten.
#[tokio::test]
async fn a_removed_playground_still_counts_toward_the_ten() -> TestResult {
    let (h, _owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    let mut last = Value::Null;
    for n in 0..10 {
        let made = from_template(&h, &alice, &format!("lab {n}")).await?;
        assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
        last = made.body;
    }
    let sid = last["id"].as_str().ok_or("id")?;
    let removed = call(
        &h.router,
        Method::DELETE,
        &format!("/api/sandboxes/{sid}"),
        None,
        &alice,
    )
    .await?;
    assert!(removed.status.is_success(), "{:?}", removed.body);
    let refused = from_template(&h, &alice, "lab again").await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    Ok(())
}

/// Review round 2 (PLY-005, PLY-008): a playground's profile has no environment and no
/// automatic promotion, whose confirmation would promote through GitHub.
#[tokio::test]
async fn a_playground_profile_has_no_environment_or_automatic_promotion() -> TestResult {
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "shop").await?;
    let sid = made.body["id"].as_str().ok_or("id")?;
    for (environments, auto_promote) in [
        (
            json!([{"name": "review", "after": "integration", "verify": "manual", "command": null}]),
            false,
        ),
        (json!([]), true),
    ] {
        let patched = call(
            &h.router,
            Method::PATCH,
            &format!("/api/sandboxes/{sid}"),
            Some(json!({"profile": {
                "preset": "standard", "integration_branch": "dev", "production_branch": "main",
                "merge": "squash", "promote": "merge_commit", "auto_promote": auto_promote,
                "environments": environments
            }})),
            &owner,
        )
        .await?;
        assert_eq!(
            patched.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            patched.body
        );
    }
    Ok(())
}

/// Review round 3 (PLY-002): `yulius onboard`'s import is for GitHub repositories; an import or a
/// plan naming a playground (its empty owner) is refused before anything asks GitHub.
#[tokio::test]
async fn the_cli_import_never_reaches_a_playground() -> TestResult {
    let (h, owner) = world().await?;
    support::configure_github_app(&h).await?;
    let made = from_template(&h, &owner, "shop").await?;
    let slug = made.body["slug"].as_str().ok_or("slug")?;
    let import = call(
        &h.router,
        Method::POST,
        "/api/import",
        Some(json!({"manifest": {
            "version": 1,
            "repo": {"remote_url": format!("https://github.com//{slug}"), "owner": "", "name": slug,
                     "head_branch": "main", "head_sha": "0123456789abcdef0123456789abcdef01234567"},
            "bundle": null, "snapshot": null, "sessions": [], "since": "2026-09-23T00:00:00.000Z"
        }})),
        &owner,
    )
    .await?;
    assert!(import.status.is_client_error(), "{:?}", import.body);
    let plan = call(
        &h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(json!({"owner": "", "name": slug, "sessions": []})),
        &owner,
    )
    .await?;
    assert!(plan.status.is_client_error(), "{:?}", plan.body);
    no_github_writes(&h)?;
    Ok(())
}

/// Review round 3: a long path holding ` size=` is a name, not a PAX size override.
#[tokio::test]
async fn a_long_path_with_size_in_its_name_is_uploaded() -> TestResult {
    let (h, owner) = world().await?;
    let tmp = tempfile::tempdir()?;
    let stage = tmp.path().join("stage");
    let deep = format!("files/{}", "long-folder-name/".repeat(6));
    std::fs::create_dir_all(stage.join(&deep))?;
    std::fs::write(stage.join(&deep).join("font size=12.md"), "big\n")?;
    let output = std::process::Command::new("tar")
        .args(["--format=pax", "-czf", "-", "-C"])
        .arg(&stage)
        .arg("files")
        .output()?;
    assert!(output.status.success());
    let made = upload(&h, &owner, "fonts", output.stdout).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    Ok(())
}

// ------------------------------------------------------------------ Yulius CI (SPEC-163)

/// The CI of [`ci_playground`]: one job that fails when the commit has a file named `broken`.
const CI_JOBS: &str =
    "\n[[ci.jobs]]\nname = \"check\"\ncommand = \"test ! -f broken && echo check-ok\"\n";

/// A harness whose CI jobs run for real (the detached process host), with the real Git adapter,
/// the superadmin signed in and a GitHub App configured, so a GitHub call would be recorded.
async fn ci_world() -> BoxResult<(Harness, Creds)> {
    // A short base: job sockets live at <data>/runs/ci-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ypg").tempdir_in("/tmp")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(yulius::wiring::session::PtySessionHost::new(exe))),
        git: Some(Arc::new(GitAdapter::new("git"))),
        dir: Some(dir),
        configure: Some(Box::new(|config| {
            config.ci.job_timeout = std::time::Duration::from_secs(30);
        })),
        ..HarnessOptions::default()
    })?;
    let owner = support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    Ok((h, owner))
}

/// Commits `files` onto the playground's `dev` in its `origin` (another person's merge), and
/// returns the new `dev` head.
fn push_to_dev(h: &Harness, body: &Value, files: &[(&str, &str)]) -> BoxResult<String> {
    let origin = origin_of(h, body);
    let scratch = h.app.config.data_dir.join(format!(
        "push-{}",
        git(&origin, &["rev-parse", "dev"])?.trim()
    ));
    git(
        &h.app.config.data_dir,
        &[
            "clone",
            "-q",
            "-b",
            "dev",
            origin.to_str().ok_or("path")?,
            scratch.to_str().ok_or("path")?,
        ],
    )?;
    for (path, text) in files {
        let path = scratch.join(path);
        let mut current = std::fs::read_to_string(&path).unwrap_or_default();
        current.push_str(text);
        std::fs::write(path, current)?;
    }
    git(&scratch, &["add", "-A"])?;
    git(&scratch, &["commit", "-q", "-m", "chore: on dev"])?;
    git(&scratch, &["push", "-q", "origin", "HEAD:dev"])?;
    std::fs::remove_dir_all(&scratch)?;
    Ok(git(&origin, &["rev-parse", "dev"])?.trim().to_owned())
}

/// A playground (template) whose `dev` has Yulius CI ([`CI_JOBS`] in its `yulius.toml`).
async fn ci_playground(h: &Harness, creds: &Creds) -> BoxResult<Value> {
    let made = from_template(h, creds, "shop").await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    push_to_dev(h, &made.body, &[("yulius.toml", CI_JOBS)])?;
    Ok(made.body)
}

/// The sandbox's CI overview as `creds` sees it.
async fn ci_overview(h: &Harness, creds: &Creds, body: &Value) -> BoxResult<Value> {
    let sid = body["id"].as_str().ok_or("id")?;
    let got = call(
        &h.router,
        Method::GET,
        &format!("/api/sandboxes/{sid}/ci"),
        None,
        creds,
    )
    .await?;
    assert_eq!(got.status, StatusCode::OK, "{:?}", got.body);
    Ok(got.body)
}

/// Dispatches until no run of the playground is queued or running (the background loops do not
/// run here); the overview then.
async fn ci_settle(h: &Harness, creds: &Creds, body: &Value) -> BoxResult<Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        h.app.ci_dispatch().await;
        let overview = ci_overview(h, creds, body).await?;
        let active = overview["runs"]
            .as_array()
            .ok_or("runs")?
            .iter()
            .any(|r| r["state"] == "queued" || r["state"] == "running");
        if !active {
            return Ok(overview);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("CI did not settle: {overview}").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

/// No commit status or other write reached GitHub.
fn nothing_on_github(h: &Harness) -> BoxResult<()> {
    no_github_writes(h)?;
    assert!(
        h.github.statuses().is_empty(),
        "a commit status was published: {:?}",
        h.github.statuses()
    );
    Ok(())
}

/// PCI-001, PCI-005 (#411): a playground's `dev` is tested once per commit with its
/// `[[ci.jobs]]`, without any GitHub call; its run publishes no commit status and says why.
#[tokio::test]
async fn a_playgrounds_dev_is_tested_without_github() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let dev = git(&origin_of(&h, &body), &["rev-parse", "dev"])?
        .trim()
        .to_owned();

    h.app.ci_sync_all().await;
    let overview = ci_settle(&h, &owner, &body).await?;
    assert_eq!(overview["gate"]["jobs"][0]["name"], "check", "{overview}");
    let runs = overview["runs"].as_array().ok_or("runs")?;
    assert_eq!(runs.len(), 1, "only dev is tested: {overview}");
    assert_eq!(runs[0]["reference"], "branch/dev", "{overview}");
    assert_eq!(runs[0]["sha"], dev.as_str(), "{overview}");
    assert_eq!(runs[0]["state"], "passed", "{overview}");
    assert!(
        runs[0]["status"]
            .as_str()
            .is_some_and(|s| s.contains("playground")),
        "{overview}"
    );

    // Once per commit; a new dev head gets its own run.
    h.app.ci_sync_all().await;
    assert_eq!(
        ci_settle(&h, &owner, &body).await?["runs"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let moved = push_to_dev(&h, &body, &[("notes.txt", "more\n")])?;
    h.app.ci_sync_all().await;
    let overview = ci_settle(&h, &owner, &body).await?;
    assert_eq!(overview["runs"][0]["sha"], moved.as_str(), "{overview}");
    assert_eq!(overview["runs"][0]["state"], "passed", "{overview}");
    nothing_on_github(&h)?;
    Ok(())
}

/// The text a successful tool call answered.
fn answer(result: &Value) -> String {
    assert_eq!(result["isError"], false, "{result}");
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// What `yulius wait` reads now for the agent whose task token is `token`, at `head`.
async fn wait_state(h: &Harness, token: &str, head: &str) -> BoxResult<Value> {
    let agent = Creds {
        bearer: Some(token.into()),
        ..Creds::default()
    };
    let r = call(
        &h.internal,
        Method::POST,
        "/internal/agent/wait",
        Some(json!({"head": head, "since": Value::Null, "timeout_secs": 1})),
        &agent,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// The newest run of a playground task (`task/<n>`) in `overview`.
fn task_run(overview: &Value) -> BoxResult<Value> {
    overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| {
            r["reference"]
                .as_str()
                .is_some_and(|k| k.starts_with("task/"))
        })
        .cloned()
        .ok_or_else(|| format!("no task run: {overview}").into())
}

/// Waits until the playground's `dev` (in its `origin`) is no longer `from`; its new head.
async fn dev_moves(h: &Harness, body: &Value, from: &str) -> BoxResult<String> {
    let origin = origin_of(h, body);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let sid = yulius_domain::ids::SandboxId::parse(body["id"].as_str().ok_or("id")?)?;
        h.app.merge_armed(&sid).await;
        let dev = git(&origin, &["rev-parse", "dev"])?.trim().to_owned();
        if dev != from {
            return Ok(dev);
        }
        if tokio::time::Instant::now() > deadline {
            return Err("dev never moved".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

/// PCI-002, PCI-003, PCI-006 (#411): the agent's `delivery_merge` in a playground with CI tests
/// its head merged into `dev` as `dev` is now, arms the merge and leaves `dev` alone while the run
/// goes; `yulius wait` follows that run; once it passed Yulius squashes the head, and the new
/// `dev` head reuses the result instead of running again. GitHub hears nothing.
#[tokio::test]
async fn delivery_merge_tests_the_head_merged_into_dev_first() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let origin = origin_of(&h, &body);
    let worktree = playground_task(&h, &body, "t1", &[("feature.txt", "one\n")], &["done"]).await?;
    // dev moves on after the task branched off: the run tests the head merged into the new dev.
    let dev = push_to_dev(&h, &body, &[("other.txt", "two\n")])?;
    h.app.ci_sync_all().await;
    ci_settle(&h, &owner, &body).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?.trim().to_owned();
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;

    let armed = answer(
        &agent_tool(
            &h,
            "tok-t1",
            "delivery_merge",
            json!({"reviewed_head": head, "review_summary": "approve"}),
        )
        .await?,
    );
    assert!(armed.contains("armed: true"), "{armed}");
    assert_eq!(
        git(&origin, &["rev-parse", "dev"])?.trim(),
        dev,
        "nothing lands before the checks pass"
    );
    let run = task_run(&ci_overview(&h, &owner, &body).await?)?;
    assert!(
        run["label"]
            .as_str()
            .is_some_and(|l| l.starts_with("Task #")),
        "{run}"
    );
    let candidate = run["sha"].as_str().ok_or("sha")?.to_owned();
    let repo = sandbox_dir(&h, &body).join("repo");
    for contained in [head.as_str(), dev.as_str()] {
        git(
            &repo,
            &["merge-base", "--is-ancestor", contained, &candidate],
        )?;
    }
    let state = wait_state(&h, "tok-t1", &head).await?;
    assert!(
        ["queued", "running"].contains(&state["checks"].as_str().unwrap_or_default()),
        "the wait follows the task's run: {state}"
    );
    assert_eq!(state["merge_armed"], true, "{state}");

    ci_settle(&h, &owner, &body).await?;
    let merged = dev_moves(&h, &body, &dev).await?;
    assert_eq!(git(&origin, &["rev-parse", "dev^"])?.trim(), dev);
    assert_eq!(file_at(&origin, "dev", "feature.txt")?, "one\n");
    assert_eq!(
        git(&origin, &["rev-parse", "dev^{tree}"])?.trim(),
        git(&repo, &["rev-parse", &format!("{candidate}^{{tree}}")])?.trim(),
        "the squash lands the tree that was tested"
    );
    let state = wait_state(&h, "tok-t1", &head).await?;
    assert_eq!(state["merge_armed"], false, "{state}");
    assert_eq!(state["stage"], "merged_integration", "{state}");
    assert!(
        state["merge_outcome"]
            .as_str()
            .is_some_and(|o| o.contains("passed")),
        "{state}"
    );

    h.app.ci_sync_all().await;
    let overview = ci_settle(&h, &owner, &body).await?;
    let newest = &overview["runs"][0];
    assert_eq!(newest["sha"], merged.as_str(), "{overview}");
    assert_eq!(newest["state"], "passed", "{overview}");
    assert!(
        newest["reason"]
            .as_str()
            .is_some_and(|r| r.contains("same tree")),
        "the merged dev reuses the task's run: {overview}"
    );
    // `ci_status` lists the task's own run first, before newer runs of dev (PCI-006).
    let status = answer(&agent_tool(&h, "tok-t1", "ci_status", json!({})).await?);
    let first = status
        .lines()
        .skip_while(|line| !line.starts_with("Recent runs"))
        .nth(1)
        .unwrap_or_default();
    assert!(
        first.contains("Task #") && first.ends_with("(yours)"),
        "{status}"
    );
    assert!(status.contains("This is a playground"), "{status}");
    assert!(!status.contains("pull request"), "{status}");
    nothing_on_github(&h)?;
    Ok(())
}

/// `delivery_merge` of `head` by the agent whose task token is `token`, reviewed.
async fn agent_merge(h: &Harness, token: &str, head: &str) -> BoxResult<Value> {
    agent_tool(
        h,
        token,
        "delivery_merge",
        json!({"reviewed_head": head, "review_summary": "approve"}),
    )
    .await
}

/// PCI-003 (#411): a head whose merge into `dev` fails its checks never lands: the armed merge is
/// dropped with why when the run ends, and the next `delivery_merge` is refused (merge on green
/// only).
#[tokio::test]
async fn a_failing_head_is_refused_and_its_armed_merge_dropped() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let origin = origin_of(&h, &body);
    let dev = git(&origin, &["rev-parse", "dev"])?.trim().to_owned();
    let worktree = playground_task(&h, &body, "t1", &[("broken", "yes\n")], &["done"]).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?.trim().to_owned();
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let armed = answer(&agent_merge(&h, "tok-t1", &head).await?);
    assert!(armed.contains("armed: true"), "{armed}");

    let overview = ci_settle(&h, &owner, &body).await?;
    assert_eq!(task_run(&overview)?["state"], "failed", "{overview}");
    let sid = yulius_domain::ids::SandboxId::parse(body["id"].as_str().ok_or("id")?)?;
    h.app.merge_armed(&sid).await;
    let state = wait_state(&h, "tok-t1", &head).await?;
    assert_eq!(state["checks"], "failed", "{state}");
    assert_eq!(state["merge_armed"], false, "{state}");
    assert!(
        state["merge_outcome"]
            .as_str()
            .is_some_and(|o| o.contains("merge on green only")),
        "{state}"
    );

    let refused = agent_merge(&h, "tok-t1", &head).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        refused.to_string().contains("merge on green only"),
        "{refused}"
    );
    assert_eq!(
        git(&origin, &["rev-parse", "dev"])?.trim(),
        dev,
        "nothing landed"
    );
    nothing_on_github(&h)?;
    Ok(())
}

/// PCI-003 (#411): when `dev` moves while a merge is armed, the next decision tests the head
/// merged into the new `dev` (the older run is cancelled as superseded) and lands that.
#[tokio::test]
async fn an_armed_merge_follows_a_moved_dev() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let origin = origin_of(&h, &body);
    let worktree = playground_task(&h, &body, "t1", &[("feature.txt", "one\n")], &["done"]).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?.trim().to_owned();
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let armed = answer(&agent_merge(&h, "tok-t1", &head).await?);
    assert!(armed.contains("armed: true"), "{armed}");
    // Nothing is dispatched yet: the first run is still queued when dev moves.
    let first = task_run(&ci_overview(&h, &owner, &body).await?)?;
    assert_eq!(first["state"], "queued", "{first}");

    let moved = push_to_dev(&h, &body, &[("other.txt", "two\n")])?;
    let sid = yulius_domain::ids::SandboxId::parse(body["id"].as_str().ok_or("id")?)?;
    h.app.merge_armed(&sid).await;
    let overview = ci_overview(&h, &owner, &body).await?;
    let second = task_run(&overview)?;
    assert_ne!(second["id"], first["id"], "{overview}");
    let repo = sandbox_dir(&h, &body).join("repo");
    git(
        &repo,
        &[
            "merge-base",
            "--is-ancestor",
            &moved,
            second["sha"].as_str().ok_or("sha")?,
        ],
    )?;
    let old = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["id"] == first["id"])
        .ok_or("the first run")?;
    assert_eq!(old["state"], "cancelled", "{overview}");
    assert!(
        old["reason"]
            .as_str()
            .is_some_and(|r| r.contains("superseded")),
        "{overview}"
    );

    ci_settle(&h, &owner, &body).await?;
    dev_moves(&h, &body, &moved).await?;
    assert_eq!(git(&origin, &["rev-parse", "dev^"])?.trim(), moved);
    assert_eq!(file_at(&origin, "dev", "feature.txt")?, "one\n");
    assert_eq!(file_at(&origin, "dev", "other.txt")?, "two\n");
    nothing_on_github(&h)?;
    Ok(())
}

/// PCI-004 (#411): a person's merge in a playground with CI never arms: while Yulius CI tests the
/// head merged into `dev` it is a 409 saying so, and once that run passed it squashes.
#[tokio::test]
async fn a_persons_merge_waits_for_green() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let origin = origin_of(&h, &body);
    let dev = git(&origin, &["rev-parse", "dev"])?.trim().to_owned();
    playground_task(&h, &body, "t1", &[("feature.txt", "one\n")], &["done"]).await?;

    let waiting = merge(&h, &owner, "t1").await?;
    assert_eq!(waiting.status, StatusCode::CONFLICT, "{:?}", waiting.body);
    assert!(
        text(&waiting).contains("Yulius CI is testing"),
        "{:?}",
        waiting.body
    );
    assert_eq!(git(&origin, &["rev-parse", "dev"])?.trim(), dev);
    let again = merge(&h, &owner, "t1").await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{:?}", again.body);
    let runs = ci_overview(&h, &owner, &body).await?;
    assert_eq!(
        runs["runs"]
            .as_array()
            .map(|r| r.iter().filter(|r| r["reference"] != "branch/dev").count()),
        Some(1),
        "asking again tests nothing more: {runs}"
    );

    ci_settle(&h, &owner, &body).await?;
    let merged = merge(&h, &owner, "t1").await?;
    assert_eq!(merged.status, StatusCode::OK, "{:?}", merged.body);
    assert_eq!(
        merged.body["stage"], "merged_integration",
        "{:?}",
        merged.body
    );
    assert_eq!(git(&origin, &["rev-parse", "dev^"])?.trim(), dev);
    nothing_on_github(&h)?;
    Ok(())
}

/// Review round 1, major 1 (PCI-002): where both sides changed a `merge=union` file, the squash
/// lands exactly the tree Yulius CI tested (Git orders a union by which side is "ours").
#[tokio::test]
async fn a_union_merged_file_lands_the_tested_tree() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let origin = origin_of(&h, &body);
    let repo = sandbox_dir(&h, &body).join("repo");
    std::fs::create_dir_all(repo.join(".git/info"))?;
    std::fs::write(repo.join(".git/info/attributes"), "/notes.md merge=union\n")?;
    push_to_dev(&h, &body, &[("notes.md", "base\n")])?;
    let worktree = playground_task(
        &h,
        &body,
        "t1",
        &[("notes.md", "base\ntask entry\n")],
        &["done"],
    )
    .await?;
    let dev = push_to_dev(&h, &body, &[("notes.md", "dev entry\n")])?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?.trim().to_owned();
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let armed = answer(&agent_merge(&h, "tok-t1", &head).await?);
    assert!(armed.contains("armed: true"), "{armed}");
    let candidate = task_run(&ci_overview(&h, &owner, &body).await?)?["sha"]
        .as_str()
        .ok_or("sha")?
        .to_owned();

    ci_settle(&h, &owner, &body).await?;
    dev_moves(&h, &body, &dev).await?;
    let notes = file_at(&origin, "dev", "notes.md")?;
    assert!(
        notes.contains("task entry") && notes.contains("dev entry"),
        "{notes}"
    );
    assert_eq!(
        git(&origin, &["rev-parse", "dev^{tree}"])?.trim(),
        git(&repo, &["rev-parse", &format!("{candidate}^{{tree}}")])?.trim(),
        "the tested tree lands: {notes}"
    );
    nothing_on_github(&h)?;
    Ok(())
}

/// Commits `files` in `worktree`; its new head.
fn commit_in(worktree: &Path, files: &[(&str, Option<&str>)]) -> BoxResult<String> {
    for (path, text) in files {
        match text {
            Some(text) => std::fs::write(worktree.join(path), text)?,
            None => std::fs::remove_file(worktree.join(path))?,
        }
    }
    git(worktree, &["add", "-A"])?;
    git(worktree, &["commit", "-q", "-m", "more"])?;
    Ok(git(worktree, &["rev-parse", "HEAD"])?.trim().to_owned())
}

/// Review round 1, major 2 (PCI-002): a head reset to an older commit never takes the verdict of
/// the head it came from. A passed run of a later commit that removed `broken` does not let the
/// older commit with `broken` land, and a failed run of a later commit does not refuse the older
/// one that passes.
#[tokio::test]
async fn a_reset_head_never_takes_a_later_heads_verdict() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let origin = origin_of(&h, &body);
    let dev = git(&origin, &["rev-parse", "dev"])?.trim().to_owned();
    let sid = yulius_domain::ids::SandboxId::parse(body["id"].as_str().ok_or("id")?)?;
    let worktree = playground_task(&h, &body, "t1", &[("broken", "yes\n")], &["done"]).await?;
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let red = git(&worktree, &["rev-parse", "HEAD"])?.trim().to_owned();

    // False green: B (which removes `broken`) passes; reset to A, A must be tested itself.
    let green = commit_in(&worktree, &[("broken", None), ("fine.txt", Some("ok\n"))])?;
    let armed = answer(&agent_merge(&h, "tok-t1", &green).await?);
    assert!(armed.contains("armed: true"), "{armed}");
    // The run passes and Yulius lands nothing yet: the agent resets first.
    git(&worktree, &["reset", "-q", "--hard", &red])?;
    ci_settle(&h, &owner, &body).await?;
    h.app.merge_armed(&sid).await;
    assert_eq!(git(&origin, &["rev-parse", "dev"])?.trim(), dev);
    let again = answer(&agent_merge(&h, "tok-t1", &red).await?);
    assert!(
        again.contains("armed: true"),
        "A is tested, not merged on B's verdict: {again}"
    );
    let overview = ci_settle(&h, &owner, &body).await?;
    assert_eq!(task_run(&overview)?["state"], "failed", "{overview}");
    h.app.merge_armed(&sid).await;
    assert_eq!(
        git(&origin, &["rev-parse", "dev"])?.trim(),
        dev,
        "nothing landed"
    );

    // False red: B (with `broken` again) failed; reset to a commit without it, which passes.
    let fixed = commit_in(&worktree, &[("broken", None)])?;
    let broken = commit_in(&worktree, &[("broken", Some("again\n"))])?;
    let armed = answer(&agent_merge(&h, "tok-t1", &broken).await?);
    assert!(armed.contains("armed: true"), "{armed}");
    ci_settle(&h, &owner, &body).await?;
    h.app.merge_armed(&sid).await;
    git(&worktree, &["reset", "-q", "--hard", &fixed])?;
    let retried = answer(&agent_merge(&h, "tok-t1", &fixed).await?);
    assert!(
        retried.contains("armed: true"),
        "the fixed head is tested, not refused on B's verdict: {retried}"
    );
    ci_settle(&h, &owner, &body).await?;
    dev_moves(&h, &body, &dev).await?;
    assert!(file_at(&origin, "dev", "broken").is_err(), "no broken file");
    nothing_on_github(&h)?;
    Ok(())
}

/// Review round 1 (PCI-004): a person's merge follows the profile's `require_checks` as on
/// GitHub: without required checks it squashes at once.
#[tokio::test]
async fn a_persons_merge_without_required_checks_lands_at_once() -> TestResult {
    let (h, owner) = ci_world().await?;
    let body = ci_playground(&h, &owner).await?;
    let origin = origin_of(&h, &body);
    let dev = git(&origin, &["rev-parse", "dev"])?.trim().to_owned();
    let sid = body["id"].as_str().ok_or("id")?;
    let patched = call(
        &h.router,
        Method::PATCH,
        &format!("/api/sandboxes/{sid}"),
        Some(json!({"profile": {
            "preset": "standard", "integration_branch": "dev", "production_branch": "main",
            "merge": "squash", "promote": "merge_commit", "auto_promote": false,
            "require_checks": false, "environments": []
        }})),
        &owner,
    )
    .await?;
    assert_eq!(patched.status, StatusCode::OK, "{:?}", patched.body);
    playground_task(&h, &body, "t1", &[("feature.txt", "one\n")], &["done"]).await?;
    let merged = merge(&h, &owner, "t1").await?;
    assert_eq!(merged.status, StatusCode::OK, "{:?}", merged.body);
    assert_eq!(git(&origin, &["rev-parse", "dev^"])?.trim(), dev);
    Ok(())
}

// ------------------------------------------------- `yulius onboard --playground` (SPEC-171)

/// The router served on a loopback port as `yulius serve` serves it, for the real CLI client.
async fn serve(h: &Harness) -> BoxResult<(yulius_import::Url, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let service = yulius_server::public_router(h.app.clone())
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, service).await;
    });
    Ok((yulius_import::Url::parse(&format!("http://{addr}"))?, task))
}

/// The terminal of the real CLI in a test: no person answers, the bearer token is given.
struct CliIo {
    ui: yulius_import::PlainUi<std::io::Cursor<Vec<u8>>, Vec<u8>>,
    token: String,
    claude: PathBuf,
}

impl yulius_import::ImportIo for CliIo {
    fn ui(&mut self) -> &mut dyn yulius_import::Ui {
        &mut self.ui
    }
    fn interactive(&self) -> bool {
        false
    }
    fn token(
        &mut self,
        _server: &yulius_import::Url,
    ) -> Result<yulius_import::Secret, yulius_import::ImportError> {
        Ok(yulius_import::Secret::new(&self.token))
    }
    fn session_sources(&self) -> yulius_import::SessionSources {
        yulius_import::SessionSources {
            claude_dir: Some(self.claude.clone()),
        }
    }
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }
}

/// Runs `yulius onboard --playground --yes --sessions all [--name]` (the real client) on
/// `checkout` against `server`; returns the outcome (or its error) and what it printed.
async fn onboard_cli(
    server: &yulius_import::Url,
    token: &str,
    checkout: &Path,
    claude: &Path,
    name: Option<&str>,
) -> BoxResult<(Result<yulius_import::OnboardOutcome, String>, String)> {
    let options = yulius_import::ImportOptions {
        sessions: Some(yulius_import::SessionChoice::All),
        assume_yes: true,
        playground: true,
        playground_name: name.map(str::to_owned),
        ..yulius_import::ImportOptions::new(checkout.to_path_buf(), server.clone())
    };
    let mut io = CliIo {
        ui: yulius_import::PlainUi::new(std::io::Cursor::new(Vec::new()), Vec::new()),
        token: token.to_owned(),
        claude: claude.to_path_buf(),
    };
    let outcome = tokio::task::spawn_blocking(move || {
        let outcome = yulius_import::run_import(&options, &mut io).map_err(|err| err.to_string());
        (
            outcome,
            String::from_utf8_lossy(io.ui.output()).into_owned(),
        )
    })
    .await?;
    Ok(outcome)
}

/// A laptop checkout named `shop` whose only remote is a local bare repository (not GitHub),
/// with `main` pushed there, a local `feature` branch and tag `v1`, uncommitted work (an edit,
/// a new file, a deletion) and one Claude Code session opened in it. Returns the checkout, the
/// remote and the Claude configuration directory.
fn laptop(dir: &Path) -> BoxResult<(PathBuf, PathBuf, PathBuf)> {
    let remote = dir.join("elsewhere.git");
    let checkout = dir.join("laptop").join("shop");
    std::fs::create_dir_all(&remote)?;
    std::fs::create_dir_all(&checkout)?;
    git(&remote, &["init", "--quiet", "--bare", "-b", "main"])?;
    git(&checkout, &["init", "--quiet", "-b", "main"])?;
    std::fs::write(checkout.join("README.md"), "# shop\n")?;
    std::fs::write(checkout.join("gone.txt"), "deleted later\n")?;
    git(&checkout, &["add", "-A"])?;
    git(&checkout, &["commit", "--quiet", "-m", "initial"])?;
    git(
        &checkout,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    )?;
    git(&checkout, &["push", "--quiet", "origin", "main"])?;
    git(&checkout, &["checkout", "--quiet", "-b", "feature"])?;
    std::fs::write(checkout.join("feature.txt"), "feature\n")?;
    git(&checkout, &["add", "feature.txt"])?;
    git(&checkout, &["commit", "--quiet", "-m", "feature work"])?;
    git(&checkout, &["tag", "v1"])?;
    git(&checkout, &["checkout", "--quiet", "main"])?;
    std::fs::write(checkout.join("README.md"), "# shop, edited on the laptop\n")?;
    std::fs::write(checkout.join("new.txt"), "not committed\n")?;
    std::fs::remove_file(checkout.join("gone.txt"))?;
    let claude = dir.join("claude");
    let cwd = std::fs::canonicalize(&checkout)?;
    let slug: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let session = claude.join("projects").join(slug);
    std::fs::create_dir_all(&session)?;
    let line = |kind: &str, at: &str, content: Value| {
        json!({"type": kind, "cwd": cwd.to_string_lossy(), "sessionId": "5a1e0000-cli",
               "timestamp": at, "gitBranch": "main", "version": "2.1.292",
               "message": {"role": kind, "content": content}})
        .to_string()
    };
    std::fs::write(
        session.join("5a1e0000-cli.jsonl"),
        format!(
            "{}\n{}\n",
            line(
                "user",
                "2026-10-14T10:00:00.000Z",
                json!("Add a basket to the shop")
            ),
            line(
                "assistant",
                "2026-10-14T10:05:00.000Z",
                json!([{"type": "text", "text": "ok"}])
            )
        ),
    )?;
    Ok((checkout, remote, claude))
}

/// Every ref of a repository with its commit, sorted.
fn refs(repo: &Path) -> BoxResult<String> {
    Ok(git(
        repo,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )?)
}

/// The playgrounds a person made, as the store keeps them (not removed).
async fn playgrounds_named(
    h: &Harness,
    name: &str,
) -> BoxResult<Vec<yulius_app::model::SandboxRecord>> {
    use yulius_app::ports::store::SandboxStore;
    Ok(h.store
        .sandboxes()
        .await?
        .into_iter()
        .filter(|s| s.is_playground() && s.repo_name.starts_with(name))
        .collect())
}

/// SPEC-171 PGL-001, PGL-003, PGL-004: the real `yulius onboard --playground` on a checkout whose
/// only remote is not GitHub makes a playground with every branch and tag, the uncommitted work as
/// one commit on the checked-out branch, `dev`, and the session on its Sessions page. Nothing is
/// pushed to the checkout's remote, and GitHub is never written to while its permission lookups
/// fail (a GitHub path would have failed the upload).
#[tokio::test]
async fn onboard_cli_uploads_a_checkout_into_a_new_playground() -> TestResult {
    let (h, owner) = world().await?;
    support::configure_github_app(&h).await?;
    h.github
        .fail_permissions
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let bearer = support::superadmin_bearer(&h).await?;
    let token = bearer.bearer.clone().ok_or("token")?;
    let (server, _serving) = serve(&h).await?;
    let tmp = tempfile::tempdir()?;
    let (checkout, remote, claude) = laptop(tmp.path())?;
    let remote_before = refs(&remote)?;
    let laptop_main = git(&checkout, &["rev-parse", "main"])?.trim().to_owned();
    let laptop_feature = git(&checkout, &["rev-parse", "feature"])?.trim().to_owned();

    let (outcome, output) = onboard_cli(&server, &token, &checkout, &claude, None).await?;
    let outcome = outcome.map_err(|err| format!("{err}\n{output}"))?;
    let result = outcome.result.ok_or("applied")?;
    assert_eq!(result.sessions_imported, 1, "{output}");
    assert_eq!(result.onboarding, "not_needed");
    assert!(output.contains("✓ shop is on Yulius"), "{output}");

    let made = playgrounds_named(&h, "shop").await?;
    assert_eq!(made.len(), 1, "{made:?}");
    let sandbox = json!({"id": made[0].id.as_str()});
    let origin = origin_of(&h, &sandbox);
    assert_eq!(branches(&origin)?, ["dev", "feature", "main"]);
    assert_eq!(
        git(&origin, &["rev-parse", "feature"])?.trim(),
        laptop_feature
    );
    assert_eq!(git(&origin, &["rev-parse", "v1"])?.trim(), laptop_feature);
    // The uncommitted work is one commit on `main`, on top of the laptop's `main`.
    assert_eq!(git(&origin, &["rev-parse", "main~1"])?.trim(), laptop_main);
    assert_eq!(
        file_at(&origin, "main", "README.md")?,
        "# shop, edited on the laptop\n"
    );
    assert_eq!(file_at(&origin, "main", "new.txt")?, "not committed\n");
    assert!(git(&origin, &["cat-file", "-e", "main:gone.txt"]).is_err());
    assert_eq!(
        git(&origin, &["rev-parse", "dev"])?,
        git(&origin, &["rev-parse", "main"])?
    );
    // Its sandbox has the clone, and its Sessions page the session.
    assert!(sandbox_dir(&h, &sandbox).join("repo").join(".git").exists());
    let sessions = call(
        &h.router,
        Method::GET,
        &format!("/api/sandboxes/{}/sessions", made[0].id),
        None,
        &owner,
    )
    .await?;
    assert_eq!(sessions.status, StatusCode::OK, "{:?}", sessions.body);
    let listed = sessions.body["sessions"].to_string();
    assert!(listed.contains("Add a basket to the shop"), "{listed}");

    // Nothing was pushed anywhere, and GitHub was never written to.
    assert_eq!(refs(&remote)?, remote_before);
    no_github_writes(&h)?;

    // The same checkout again lands in the same playground and makes no other.
    let (again, output) = onboard_cli(&server, &token, &checkout, &claude, None).await?;
    let again = again.map_err(|err| format!("{err}\n{output}"))?;
    assert_eq!(again.sessions_sent, 0, "{output}");
    assert_eq!(playgrounds_named(&h, "shop").await?.len(), 1);
    // Review round 1: branches it already has are not imported again under `import/`.
    let clone = sandbox_dir(&h, &sandbox).join("repo");
    let local = branches(&clone)?;
    assert!(!local.iter().any(|b| b.starts_with("import/")), "{local:?}");
    no_github_writes(&h)?;
    Ok(())
}

/// SPEC-171 PGL-002: without the playground flag an empty owner is still refused, and a new
/// playground from the CLI needs the checkout's branches.
#[tokio::test]
async fn a_cli_playground_needs_the_flag_and_a_bundle() -> TestResult {
    let (h, owner) = world().await?;
    let manifest = |playground: bool| {
        json!({"manifest": {
            "version": 1,
            "repo": {"remote_url": "", "owner": "", "name": "shop",
                     "head_branch": "main", "head_sha": "0123456789abcdef0123456789abcdef01234567"},
            "bundle": null, "snapshot": null, "sessions": [], "since": "2026-09-23T00:00:00.000Z",
            "playground": playground
        }})
    };
    for playground in [false, true] {
        let refused = call(
            &h.router,
            Method::POST,
            "/api/import",
            Some(manifest(playground)),
            &owner,
        )
        .await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{playground}: {:?}",
            refused.body
        );
    }
    let plan = call(
        &h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(json!({"owner": "", "name": "shop", "sessions": [], "playground": true})),
        &owner,
    )
    .await?;
    assert_eq!(plan.status, StatusCode::OK, "{:?}", plan.body);
    assert_eq!(plan.body["can_create"], true);
    assert!(plan.body["sandbox"].is_null());
    Ok(())
}

/// SPEC-171 PGL-002, PGL-004: into the person's existing playground of that name the CLI upload
/// lands as an import does (branches under `import/<login>/`, the uncommitted work on a snapshot
/// branch, in the sandbox clone): the playground's own branches are untouched, no other is made.
#[tokio::test]
async fn onboard_cli_uploads_into_an_existing_playground() -> TestResult {
    let (h, owner) = world().await?;
    let bearer = support::superadmin_bearer(&h).await?;
    let token = bearer.bearer.clone().ok_or("token")?;
    let (server, _serving) = serve(&h).await?;
    let made = from_template(&h, &owner, "shop").await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let origin = origin_of(&h, &made.body);
    let before = refs(&origin)?;
    let tmp = tempfile::tempdir()?;
    let (checkout, _remote, claude) = laptop(tmp.path())?;
    let feature = git(&checkout, &["rev-parse", "feature"])?.trim().to_owned();

    let (outcome, output) = onboard_cli(&server, &token, &checkout, &claude, Some("Shop")).await?;
    let outcome = outcome.map_err(|err| format!("{err}\n{output}"))?;
    let result = outcome.result.ok_or("applied")?;
    assert_eq!(result.sandbox_id, made.body["id"].as_str().ok_or("id")?);
    assert_eq!(result.sessions_imported, 1, "{output}");
    assert_eq!(playgrounds_named(&h, "shop").await?.len(), 1);
    assert_eq!(
        refs(&origin)?,
        before,
        "the playground's own branches are untouched"
    );
    let clone = sandbox_dir(&h, &made.body).join("repo");
    let imported = branches(&clone)?;
    assert!(
        imported.iter().any(|b| b == "import/user/feature"),
        "{imported:?}"
    );
    assert!(
        imported.iter().any(|b| b == "import/user/main"),
        "{imported:?}"
    );
    let snapshot = imported
        .iter()
        .find(|b| b.starts_with("import/user/snapshot-"))
        .ok_or("snapshot branch")?;
    assert_eq!(
        git(&clone, &["rev-parse", "import/user/feature"])?.trim(),
        feature
    );
    assert_eq!(file_at(&clone, snapshot, "new.txt")?, "not committed\n");
    no_github_writes(&h)?;
    Ok(())
}

/// SPEC-171 PGL-002: a CLI upload makes a playground only within the person's limit of 10; their
/// existing playground of that name still receives it.
#[tokio::test]
async fn a_cli_playground_counts_toward_the_ten() -> TestResult {
    use yulius_app::ports::store::UserStore;
    let (h, _owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    for n in 0..10 {
        let made = from_template(&h, &alice, &format!("lab {n}")).await?;
        assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    }
    let user = h
        .store
        .user(&yulius_domain::ids::UserId::parse("u-alice")?)
        .await?
        .ok_or("alice")?;
    let bearer = Creds {
        bearer: Some(
            h.app
                .issue_api_token(&user, "tests", yulius_domain::permission::TokenScope::Cli)
                .await?
                .expose()
                .to_owned(),
        ),
        ..Creds::default()
    };
    let plan = |name: &str| json!({"owner": "", "name": name, "sessions": [], "playground": true});
    let refused = call(
        &h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(plan("shop")),
        &bearer,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::OK, "{:?}", refused.body);
    assert_eq!(refused.body["can_create"], false);
    assert!(
        refused.body["refusal"]
            .as_str()
            .unwrap_or_default()
            .contains("10 playgrounds")
    );
    let existing = call(
        &h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(plan("lab 3")),
        &bearer,
    )
    .await?;
    assert_eq!(existing.status, StatusCode::OK, "{:?}", existing.body);
    assert_eq!(
        existing.body["sandbox"]["slug"], "lab-3",
        "{:?}",
        existing.body
    );
    assert!(existing.body["refusal"].is_null());

    let (server, _serving) = serve(&h).await?;
    let tmp = tempfile::tempdir()?;
    let (checkout, _remote, claude) = laptop(tmp.path())?;
    let token = bearer.bearer.clone().ok_or("token")?;
    let (outcome, output) = onboard_cli(&server, &token, &checkout, &claude, None).await?;
    let err = outcome.err().ok_or("an eleventh playground was made")?;
    assert!(err.contains("10 playgrounds"), "{err}\n{output}");
    assert!(playgrounds_named(&h, "shop").await?.is_empty());
    Ok(())
}

// ------------------------------------------------- purging a removed playground (SPEC-171)

async fn purge(h: &Harness, creds: &Creds, sid: &str) -> BoxResult<support::Resp> {
    call(
        &h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/purge"),
        None,
        creds,
    )
    .await
}

/// The audit actions recorded on a sandbox, oldest first.
async fn audit_actions(h: &Harness, sid: &str) -> BoxResult<Vec<String>> {
    use yulius_app::ports::store_ext::AutomationStore;
    let mut entries = h.store.automation_audit(sid, 100).await?;
    entries.reverse();
    Ok(entries.into_iter().map(|e| e.action).collect())
}

/// SPEC-171 PGL-006: the purge of a removed playground deletes nothing while a process of its
/// sandbox runs (here: an agent run still ending after the removal stopped its task), and
/// deletes `origin` and the sandbox directory on the first round after it ended.
#[tokio::test]
async fn a_purge_waits_while_a_process_of_the_sandbox_runs() -> TestResult {
    use yulius_app::ports::store::{TaskStore, UserStore};
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "shop").await?;
    let sid = made.body["id"].as_str().ok_or("id")?.to_owned();
    let (origin, dir) = (origin_of(&h, &made.body), sandbox_dir(&h, &made.body));
    let creator = h.store.superadmin().await?.ok_or("superadmin")?.id;
    support::seed_task(
        &h,
        "t1",
        &sid,
        creator.as_str(),
        yulius_domain::task::TaskStatus::Closed,
        None,
    )
    .await?;
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;

    let asked = purge(&h, &owner, &sid).await?;
    assert_eq!(asked.status, StatusCode::OK, "{:?}", asked.body);
    assert_eq!(asked.body["state"], "waiting", "{:?}", asked.body);
    assert_eq!(
        asked.body["waiting_for"][0], "1 agent run",
        "{:?}",
        asked.body
    );
    assert!(
        origin.exists() && dir.exists(),
        "nothing is deleted while a run lives"
    );
    // The playground is removed at once: nobody sees it any more.
    let listed = call(&h.router, Method::GET, "/api/sandboxes", None, &owner).await?;
    assert!(!listed.body.to_string().contains(&sid), "{:?}", listed.body);
    assert_eq!(h.app.run_purges().await, 0);
    assert!(
        origin.exists() && dir.exists(),
        "a round while it runs deletes nothing"
    );

    let attempt = yulius_domain::ids::AttemptId::parse("a-t1")?;
    assert!(
        h.store
            .transition_attempt(
                &attempt,
                yulius_domain::task::AttemptState::Exited,
                &yulius_app::ports::store::AttemptUpdate::default(),
            )
            .await?
    );
    assert_eq!(h.app.run_purges().await, 1);
    assert!(
        !origin.exists() && !dir.exists(),
        "deleted once nothing runs"
    );
    assert_eq!(h.app.run_purges().await, 0, "a purge is done once");
    let actions = audit_actions(&h, &sid).await?;
    assert!(
        actions.ends_with(&[
            "sandbox.removed".into(),
            "sandbox.purge_requested".into(),
            "sandbox.purged".into()
        ]),
        "{actions:?}"
    );
    // Task records stay.
    assert!(
        h.store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .is_some()
    );
    Ok(())
}

/// SPEC-171 PGL-005, PGL-007: purging on removal deletes the code at once when nothing runs, and
/// its maker may make another playground at the limit of 10.
#[tokio::test]
async fn purging_a_playground_deletes_its_code_once_nothing_runs() -> TestResult {
    let (h, _owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    let mut last = Value::Null;
    for n in 0..10 {
        let made = from_template(&h, &alice, &format!("lab {n}")).await?;
        assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
        last = made.body;
    }
    let sid = last["id"].as_str().ok_or("id")?;
    let purged = purge(&h, &alice, sid).await?;
    assert_eq!(purged.status, StatusCode::OK, "{:?}", purged.body);
    assert_eq!(purged.body["state"], "purged", "{:?}", purged.body);
    assert!(!origin_of(&h, &last).exists());
    assert!(!sandbox_dir(&h, &last).exists());
    let again = from_template(&h, &alice, "lab again").await?;
    assert_eq!(again.status, StatusCode::CREATED, "{:?}", again.body);
    // The removal's export of its tasks goes with it (SPEC-276 RDA-011).
    let exports = std::fs::read_dir(h.app.config.data_dir.join("exports"))?.count();
    assert_eq!(exports, 0);
    no_github_writes(&h)?;
    Ok(())
}

/// SPEC-276 RDA-011 (was SPEC-171 PGL-005's refusal): a GitHub sandbox's files on this server are
/// purged too, as soon as nothing of it runs; its repository on GitHub is never written to.
#[tokio::test]
async fn a_github_sandbox_s_files_are_purged_and_its_repository_stays() -> TestResult {
    let (h, owner) = world().await?;
    let creator = {
        use yulius_app::ports::store::UserStore;
        h.store.superadmin().await?.ok_or("superadmin")?.id
    };
    support::seed_sandbox(&h, "s1", "shop", creator.as_str()).await?;
    let dir = h.app.config.data_dir.join("sandboxes/s1");
    std::fs::create_dir_all(dir.join("repo"))?;
    std::fs::write(dir.join("repo/README.md"), "hello")?;
    let purged = purge(&h, &owner, "s1").await?;
    assert_eq!(purged.status, StatusCode::OK, "{:?}", purged.body);
    assert_eq!(purged.body["state"], "purged", "{:?}", purged.body);
    assert!(!dir.exists(), "its files are gone");
    let gone = call(&h.router, Method::GET, "/api/sandboxes/s1", None, &owner).await?;
    assert_eq!(
        gone.status,
        StatusCode::NOT_FOUND,
        "removed: {:?}",
        gone.body
    );
    no_github_writes(&h)?;
    Ok(())
}

/// SPEC-171 PGL-005: only an admin of the playground purges it; a removed playground is purged by
/// the superadmin only (it is invisible to everyone else).
#[tokio::test]
async fn only_an_admin_purges_and_the_superadmin_purges_a_removed_one() -> TestResult {
    let (h, owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    let bob = support::seed_github_user(&h, "u-bob", "bob").await?;
    let made = from_template(&h, &alice, "lab").await?;
    let sid = made.body["id"].as_str().ok_or("id")?;
    assert_eq!(purge(&h, &bob, sid).await?.status, StatusCode::NOT_FOUND);
    assert!(origin_of(&h, &made.body).exists());
    let removed = call(
        &h.router,
        Method::DELETE,
        &format!("/api/sandboxes/{sid}"),
        None,
        &alice,
    )
    .await?;
    assert!(removed.status.is_success(), "{:?}", removed.body);
    assert_eq!(purge(&h, &alice, sid).await?.status, StatusCode::NOT_FOUND);
    assert!(origin_of(&h, &made.body).exists());
    let purged = purge(&h, &owner, sid).await?;
    assert_eq!(purged.status, StatusCode::OK, "{:?}", purged.body);
    assert_eq!(purged.body["state"], "purged");
    assert!(!origin_of(&h, &made.body).exists());
    Ok(())
}

// ------------------------------------------------- review round 1 (SPEC-171)

/// A CLI upload of `checkout` into a new playground `name` as the CLI makes it, through the HTTP
/// API: the job is created and every file uploaded, not completed. Returns the job id.
async fn cli_job(h: &Harness, creds: &Creds, checkout: &Path, name: &str) -> BoxResult<String> {
    let bundle_path = checkout.join("..").join(format!("{name}.bundle"));
    git(
        checkout,
        &[
            "bundle",
            "create",
            "--quiet",
            &bundle_path.to_string_lossy(),
            "--branches",
            "--tags",
        ],
    )?;
    let bundle = std::fs::read(&bundle_path)?;
    let head = git(checkout, &["rev-parse", "HEAD"])?.trim().to_owned();
    let manifest = json!({"manifest": {
        "version": 1,
        "repo": {"remote_url": "", "owner": "", "name": name, "head_branch": "main", "head_sha": head},
        "bundle": {"name": "repo.bundle", "size": bundle.len(), "sha256": h.app.deps.crypto.sha256_hex(&bundle)},
        "snapshot": null, "sessions": [], "since": "1970-01-01T00:00:00.000Z", "playground": true
    }});
    let created = call(
        &h.router,
        Method::POST,
        "/api/import",
        Some(manifest),
        creds,
    )
    .await?;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.body);
    let job = created.body["job_id"].as_str().ok_or("job")?.to_owned();
    if created.body["missing"]
        .as_array()
        .is_some_and(|m| !m.is_empty())
    {
        let sha = h.app.deps.crypto.sha256_hex(&bundle);
        let put = support::repos::call_raw(
            &h.router,
            Method::PUT,
            &format!("/api/import/{job}/files/repo.bundle?offset=0"),
            bundle,
            &[
                ("x-chunk-sha256", &sha),
                ("content-type", "application/octet-stream"),
            ],
            creds,
        )
        .await?;
        assert_eq!(put.status, StatusCode::OK, "{:?}", put.body);
    }
    Ok(job)
}

async fn complete_job(h: &Harness, creds: &Creds, job: &str) -> BoxResult<support::Resp> {
    call(
        &h.router,
        Method::POST,
        &format!("/api/import/{job}/complete"),
        None,
        creds,
    )
    .await
}

/// Review round 1 (major): a new playground that cannot be made when its upload completes (here:
/// another playground took the person's tenth place meanwhile) keeps the uploaded files, so
/// completing again once there is room makes it (PGL-003).
#[tokio::test]
async fn a_cli_upload_refused_at_completion_keeps_its_files_and_completes_later() -> TestResult {
    use yulius_app::ports::store::UserStore;
    let (h, _owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    let user = h
        .store
        .user(&yulius_domain::ids::UserId::parse("u-alice")?)
        .await?
        .ok_or("alice")?;
    let bearer = Creds {
        bearer: Some(
            h.app
                .issue_api_token(&user, "tests", yulius_domain::permission::TokenScope::Cli)
                .await?
                .expose()
                .to_owned(),
        ),
        ..Creds::default()
    };
    let mut first = Value::Null;
    for n in 0..9 {
        let made = from_template(&h, &alice, &format!("lab {n}")).await?;
        assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
        if n == 0 {
            first = made.body;
        }
    }
    let tmp = tempfile::tempdir()?;
    let (checkout, _remote, _claude) = laptop(tmp.path())?;
    let job = cli_job(&h, &bearer, &checkout, "shop").await?;
    let tenth = from_template(&h, &alice, "lab 9").await?;
    assert_eq!(tenth.status, StatusCode::CREATED, "{:?}", tenth.body);

    let refused = complete_job(&h, &bearer, &job).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert!(playgrounds_named(&h, "shop").await?.is_empty());

    let sid = first["id"].as_str().ok_or("id")?;
    let purged = purge(&h, &alice, sid).await?;
    assert_eq!(purged.body["state"], "purged", "{:?}", purged.body);
    let done = complete_job(&h, &bearer, &job).await?;
    assert_eq!(done.status, StatusCode::OK, "{:?}", done.body);
    let made = playgrounds_named(&h, "shop").await?;
    assert_eq!(made.len(), 1);
    let origin = origin_of(&h, &json!({"id": made[0].id.as_str()}));
    assert_eq!(branches(&origin)?, ["dev", "feature", "main"]);
    Ok(())
}

/// Review round 1: a person has one new-playground upload open at a time; starting another
/// supersedes the earlier one, whose files go, so abandoned uploads cannot pile up on the disk.
#[tokio::test]
async fn a_new_playground_upload_supersedes_an_unfinished_one() -> TestResult {
    let (h, _owner) = world().await?;
    let bearer = support::superadmin_bearer(&h).await?;
    let tmp = tempfile::tempdir()?;
    let (checkout, _remote, _claude) = laptop(tmp.path())?;
    let first = cli_job(&h, &bearer, &checkout, "shop").await?;
    let reserved = {
        use yulius_app::ports::store_ext::ImportStore;
        h.store
            .import_job(&first)
            .await?
            .ok_or("job")?
            .sandbox_id
            .ok_or("sandbox")?
    };
    let dir = h
        .app
        .config
        .data_dir
        .join("sandboxes")
        .join(reserved.as_str());
    assert!(
        dir.join("imports")
            .join(&first)
            .join("repo.bundle")
            .exists()
    );
    // The same upload again resumes the same job.
    assert_eq!(cli_job(&h, &bearer, &checkout, "shop").await?, first);

    let second = cli_job(&h, &bearer, &checkout, "lab").await?;
    assert_ne!(second, first);
    assert!(!dir.exists(), "the superseded upload's files are gone");
    let stale = complete_job(&h, &bearer, &first).await?;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{:?}", stale.body);
    assert!(
        stale.body.to_string().contains("superseded"),
        "{:?}",
        stale.body
    );
    let done = complete_job(&h, &bearer, &second).await?;
    assert_eq!(done.status, StatusCode::OK, "{:?}", done.body);
    assert_eq!(playgrounds_named(&h, "lab").await?.len(), 1);
    Ok(())
}

/// Review round 1 (PGL-006): the purge waits for a CI run of the sandbox too (its checkouts are
/// claimed while a run executes), and an already purged playground is not purged or audited again.
#[tokio::test]
async fn a_purge_waits_for_a_ci_run_and_is_done_once() -> TestResult {
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "shop").await?;
    let sid = made.body["id"].as_str().ok_or("id")?.to_owned();
    let id = yulius_domain::ids::SandboxId::parse(&sid)?;
    assert!(h.app.ci_claim(&id), "a CI run holds the checkouts");
    let asked = purge(&h, &owner, &sid).await?;
    assert_eq!(asked.body["state"], "waiting", "{:?}", asked.body);
    assert_eq!(asked.body["waiting_for"][0], "a CI run", "{:?}", asked.body);
    assert!(origin_of(&h, &made.body).exists());
    h.app.ci_release(&id);
    assert_eq!(h.app.run_purges().await, 1);
    assert!(!origin_of(&h, &made.body).exists() && !sandbox_dir(&h, &made.body).exists());

    let again = purge(&h, &owner, &sid).await?;
    assert_eq!(again.status, StatusCode::OK, "{:?}", again.body);
    assert_eq!(again.body["state"], "purged");
    let actions = audit_actions(&h, &sid).await?;
    assert_eq!(
        actions.iter().filter(|a| *a == "sandbox.purged").count(),
        1,
        "{actions:?}"
    );
    assert_eq!(
        actions
            .iter()
            .filter(|a| *a == "sandbox.purge_requested")
            .count(),
        1,
        "{actions:?}"
    );
    Ok(())
}
