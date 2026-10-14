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
